//! 网络模拟串口：通过 WebSocket + JSON-RPC 连接 Nexus Prime（Klipper/Moonraker 系
//! 3D 打印机服务器，默认 7125 端口），把发送内容作为 `printer.gcode.script` 执行，
//! 把 `notify_gcode_response` 推送作为 RX 字节流发布到 DataBus，
//! 从而在终端 / 发送器 / 录制 / 回放中表现得像普通串口。
//!
//! 显示策略：只转发 gcode 命令/响应（`notify_gcode_response`），
//! 其余 notify（状态、进程统计、历史等）一律忽略，保持终端干净。

use crate::{RepaintWaker, SerialCommand, TransportResult, serial_rx_event, serial_tx_event};
use crossbeam_channel::Receiver;
use serde::{Deserialize, Serialize};
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tool_core::{Event, LogLevel, parse_gcode_response};
use tool_databus::DataBus;
use tungstenite::Message;

/// 网络串口配置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkSerialConfig {
    /// 服务器主机名或 IP。
    pub host: String,
    /// 服务器端口（Nexus Prime 默认 7125）。
    pub port: u16,
    /// 可选的 API 密钥（服务器未启用认证时留空）。
    pub api_key: Option<String>,
}

impl Default for NetworkSerialConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 7125,
            api_key: None,
        }
    }
}

impl NetworkSerialConfig {
    /// 端口显示名，例如 `192.168.1.100:7125`。
    pub fn display_name(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// 网络串口的错误一律以 `transport.network` 为来源、以端口名为前缀发布日志。
fn publish_network_error(bus: &DataBus, port_name: &str, detail: impl std::fmt::Display) {
    bus.publish(Event::system_log(
        LogLevel::Error,
        "transport.network",
        format!("{port_name} {detail}"),
    ));
}

/// 网络 worker 线程运行所需的全部环境：一次连接从建立到关闭，worker 拥有的就是这八样。
///
/// 判据与第 8 轮收拢渲染输入时相同：`stop` / `alive` / `connecting` 是三个同型的
/// `Arc<AtomicBool>`，当作相邻位置参数传时任意两个对调编译都不报错。收进命名字段后
/// 拦住的仍是**写法**而不是**取值**：位置排错在这里没有语法，少一个字段 E0063、
/// 类型不对 E0308；而 `alive: Arc::clone(&connecting)` 这种同型对调照样编译得过，
/// 它只是多要求在字段名旁边把矛盾写出来，不是类型层的保证。
pub(crate) struct NetworkWorkerContext {
    pub(crate) config: NetworkSerialConfig,
    pub(crate) command_rx: Receiver<SerialCommand>,
    /// 关闭请求：任一条关闭路径置位，worker 主循环据此退出。
    pub(crate) stop: Arc<AtomicBool>,
    /// worker 存活标记，由 `AliveGuard` 在退出（含 panic 展开）时置 false。
    pub(crate) alive: Arc<AtomicBool>,
    /// 连接中标记：握手完成或连接失败后置 false，`PortHandle` 据此区分
    /// 「连接中」与「已断开」。
    pub(crate) connecting: Arc<AtomicBool>,
    pub(crate) bus: DataBus,
    /// 事件来源，形如 `serial:127.0.0.1:7125`。
    pub(crate) source: String,
    /// worker 每次 publish 之后调用的重绘唤醒器；为 `None`（没有 UI 挂着）时照常发事件，
    /// 只是没人被催着重绘。
    pub(crate) waker: Option<Arc<dyn RepaintWaker>>,
}

/// 把这套运行环境原样搬上 worker 线程；语义全在 [`network_worker_loop`]。
pub(crate) fn spawn_network_worker(
    context: NetworkWorkerContext,
) -> TransportResult<JoinHandle<()>> {
    Ok(thread::spawn(move || network_worker_loop(context)))
}

fn network_worker_loop(context: NetworkWorkerContext) {
    // 解构回原来的名字：三个标记各被不同的退出与兜底分支按不同方式借用，
    // 收拢参数不该把这些借用改成大结构的字段借用。
    let NetworkWorkerContext {
        config,
        command_rx,
        stop,
        alive,
        connecting,
        bus,
        source,
        waker,
    } = context;
    // 覆盖全部退出路径（含 panic 展开）：原本五处显式 `alive.store(false)` 只写在
    // return/尾部，worker 一旦 panic 就整体跳过，网络端口变成不可重开的僵尸。
    let _alive_guard = crate::AliveGuard(Arc::clone(&alive));
    let port_name = crate::extract_port(&source);
    let wake = || {
        if let Some(w) = &waker {
            w.wake();
        }
    };
    let url = format!("ws://{}:{}/websocket", config.host, config.port);

    // 带超时建立 TCP 连接：拒绝时立即失败；无响应（如 Windows 端口释放延迟）
    // 最多等待 2 秒，避免 worker 永久挂起导致端口无法关闭。
    // 所有失败分支保留具体错误原因，便于用户排障。
    let connect_failed = |reason: String| {
        publish_network_error(&bus, &port_name, format_args!("连接失败：{reason}"));
        connecting.store(false, Ordering::Release);
    };
    let mut addrs = match (config.host.as_str(), config.port).to_socket_addrs() {
        Ok(addrs) => addrs,
        Err(error) => {
            connect_failed(format!("DNS 解析失败：{error}"));
            return;
        }
    };
    let Some(addr) = addrs.next() else {
        connect_failed(format!("无法解析主机名 {}", config.host));
        return;
    };
    let tcp = match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
        Ok(tcp) => tcp,
        Err(error) => {
            connect_failed(format!("TCP 连接失败：{error}"));
            return;
        }
    };
    // 连接期间用户点击取消：直接退出
    if stop.load(Ordering::Acquire) {
        return;
    }

    // 建立 WebSocket 连接（握手失败同样走失败分支）
    let mut ws = match tungstenite::client(&url, tcp) {
        Ok((ws, _)) => ws,
        Err(error) => {
            connect_failed(format!("WebSocket 握手失败：{error}"));
            return;
        }
    };
    // 连接成功：清除连接中标记
    connecting.store(false, Ordering::Release);

    // 给底层 TCP 设置读超时，让 read() 周期性返回以便及时响应关闭请求。
    let _ = ws
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(100)));

    // 自我标识（Moonraker 系服务器允许客户端 announce，失败不影响功能）。
    // API key 由旧版 Native UI 已经允许配置，必须和 Web capability 使用
    // 同一字段，否则两个 composition root 的认证行为会不一致。
    let mut identify_params = serde_json::json!({
        "client_name": "Hardware Workbench",
        "version": "1.0.0",
        "type": "desktop",
    });
    if let Some(api_key) = &config.api_key {
        identify_params["access_token"] = serde_json::Value::String(api_key.clone());
    }
    let identify = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "server.connection.identify",
        "params": identify_params,
        "id": 1,
    });
    let _ = ws.send(Message::text(identify.to_string()));

    let mut next_id: u64 = 2;
    while !stop.load(Ordering::Acquire) {
        // 发送队列：文本 → printer.gcode.script
        while let Ok(command) = command_rx.try_recv() {
            let (bytes, completion) = match command {
                SerialCommand::Write { bytes, completion } => (bytes, completion),
                SerialCommand::SetDtr { completion, .. }
                | SerialCommand::SetRts { completion, .. } => {
                    if let Some(completion) = completion {
                        // 这里的 `other` 不是第 42 轮删掉的那种"重新包装"：网络串口从来没有
                        // 过 io::Error 可丢，文案就是它的全部信息，等待方看到的仍是
                        // `IO 错误：网络串口不支持 DTR/RTS`。
                        let _ = completion.send(Err(io::Error::other("网络串口不支持 DTR/RTS")));
                    }
                    continue;
                }
            };
            let script = String::from_utf8_lossy(&bytes).into_owned();
            if script.trim().is_empty() {
                if let Some(completion) = completion {
                    let _ = completion.send(Ok(()));
                }
                continue;
            }
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "printer.gcode.script",
                "params": { "script": script },
                "id": next_id,
            });
            next_id += 1;
            match ws.send(Message::text(request.to_string())) {
                Ok(()) => {
                    bus.publish(serial_tx_event(source.clone(), bytes));
                    wake();
                    if let Some(completion) = completion {
                        let _ = completion.send(Ok(()));
                    }
                }
                Err(error) => {
                    if let Some(completion) = completion {
                        // `tungstenite::Error` 本身不是 `io::Error`：这里第一次造出错误，
                        // kind 一直是 `Other`，等待方那句 `IO 错误：…` 逐字不变。
                        // 想再进一步（`tungstenite::Error::Io(inner)` 时把 inner 的 kind 交给
                        // 等待方）会改动用户可见的框架句，属于产品决定，本轮不顺手做。
                        let _ = completion.send(Err(io::Error::other(error.to_string())));
                    }
                    publish_network_error(&bus, &port_name, format_args!("发送失败：{error}"));
                    return;
                }
            }
        }

        // 接收：只转发 gcode_response
        match ws.read() {
            Ok(Message::Text(text)) => {
                if let Some(content) = parse_gcode_response(text.as_str()) {
                    let mut data = content.into_bytes();
                    data.push(b'\n');
                    bus.publish(serial_rx_event(source.clone(), data));
                    wake();
                }
            }
            Ok(Message::Ping(payload)) => {
                let _ = ws.send(Message::Pong(payload));
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => {
                publish_network_error(&bus, &port_name, format_args!("连接断开：{error}"));
                return;
            }
        }
    }
    // 正常关闭：发送 Close 帧后释放连接
    let _ = ws.close(None);
}

#[cfg(test)]
mod tests {
    use crate::{NetworkSerialConfig, TransportManager, serial_topics};
    use std::thread;
    use std::time::Duration;
    use tool_core::Payload;
    use tool_databus::{DataBus, TopicFilter};
    use tungstenite::Message;

    /// 端到端：本地 WebSocket 服务器模拟 Nexus Prime，验证
    /// 发送 → printer.gcode.script 请求、notify_gcode_response → RX 事件 的完整链路。
    #[test]
    fn e2e_send_gcode_and_receive_response() {
        // ── 服务器端：监听随机端口，模拟 Moonraker/Nexus Prime ──
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            // 第一条：客户端 identify，忽略
            let _ = ws.read().unwrap();
            // 第二条：printer.gcode.script 请求
            let request = match ws.read().unwrap() {
                Message::Text(text) => text.as_str().to_owned(),
                other => panic!("unexpected message: {other:?}"),
            };
            // 回发 gcode 响应
            ws.send(Message::text(
                r#"{"jsonrpc":"2.0","method":"notify_gcode_response","params":["ok"]}"#,
            ))
            .unwrap();
            request
        });

        // ── 客户端：TransportManager 网络串口 ──
        let bus = DataBus::new();
        let tm = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(serial_topics::SERIAL_RX));

        let port = tm
            .open_network_serial(NetworkSerialConfig {
                host: "127.0.0.1".to_owned(),
                port: addr.port(),
                api_key: None,
            })
            .unwrap();
        assert_eq!(port, format!("127.0.0.1:{}", addr.port()));

        // 发送 GCode → 服务器应收到 printer.gcode.script 请求
        tm.send_text_to(&port, "G28").unwrap();
        let request = server.join().unwrap();
        assert!(
            request.contains("printer.gcode.script"),
            "请求应包含 gcode script 方法: {request}"
        );
        assert!(request.contains("G28"), "请求应包含 GCode 内容: {request}");

        // 服务器回发响应 → 应收到 RX 事件（字节 "ok\n"）
        let event = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        match &event.payload {
            Payload::Bytes(bytes) => assert_eq!(bytes, b"ok\n"),
            other => panic!("unexpected payload: {other:?}"),
        }

        tm.close_port(&port);
        // 等待 worker 退出，避免测试进程挂起
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !tm.status_port(&port).open && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// 连接失败时 alive 置 false、端口不进入打开状态。
    #[test]
    fn open_network_serial_reports_closed_on_connect_failure() {
        // 绑定端口后释放，制造连接拒绝。
        // 注意：Windows 上端口绑定释放有短暂延迟，若立即重连同一端口
        // SYN 可能被丢弃导致 connect 挂起（而非快速失败），故等待端口完全释放。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        thread::sleep(Duration::from_millis(200));

        let bus = DataBus::new();
        let tm = TransportManager::new(bus.clone());
        let port = tm
            .open_network_serial(NetworkSerialConfig {
                host: "127.0.0.1".to_owned(),
                port: addr.port(),
                api_key: None,
            })
            .unwrap();

        // worker 异步尝试连接，稍等后应检测到 alive=false → status 关闭。
        // （connect_timeout 2 秒 + 轮询余量）
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut closed = false;
        while std::time::Instant::now() < deadline {
            if !tm.status_port(&port).open {
                closed = true;
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(closed, "连接被拒绝后端口应进入关闭状态");
    }
}
