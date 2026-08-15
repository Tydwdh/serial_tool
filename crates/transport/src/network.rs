//! 网络模拟串口：通过 WebSocket + JSON-RPC 连接 Nexus Prime（Klipper/Moonraker 系
//! 3D 打印机服务器，默认 7125 端口），把发送内容作为 `printer.gcode.script` 执行，
//! 把 `notify_gcode_response` 推送作为 RX 字节流发布到 DataBus，
//! 从而在终端 / 发送器 / 录制 / 回放中表现得像普通串口。
//!
//! 显示策略：只转发 gcode 命令/响应（`notify_gcode_response`），
//! 其余 notify（状态、进程统计、历史等）一律忽略，保持终端干净。

use crate::{RepaintWaker, TransportResult, serial_rx_event, serial_tx_event};
use crossbeam_channel::Receiver;
use serde::{Deserialize, Serialize};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tool_core::{Event, LogLevel};
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

/// 从 JSON-RPC 文本中提取 gcode 响应内容。
/// 只处理 `notify_gcode_response` 推送，其余 notify（状态/进程统计等）一律忽略。
fn parse_gcode_response(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("method").and_then(serde_json::Value::as_str) != Some("notify_gcode_response") {
        return None;
    }
    value
        .get("params")?
        .as_array()?
        .first()?
        .as_str()
        .map(str::to_owned)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_network_worker(
    config: NetworkSerialConfig,
    write_rx: Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    connecting: Arc<AtomicBool>,
    bus: DataBus,
    source: String,
    waker: Option<Arc<dyn RepaintWaker>>,
) -> TransportResult<JoinHandle<()>> {
    let join = thread::spawn(move || {
        network_worker_loop(
            config, write_rx, stop, alive, connecting, bus, source, waker,
        );
    });
    Ok(join)
}

#[allow(clippy::too_many_arguments)]
fn network_worker_loop(
    config: NetworkSerialConfig,
    write_rx: Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    connecting: Arc<AtomicBool>,
    bus: DataBus,
    source: String,
    waker: Option<Arc<dyn RepaintWaker>>,
) {
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
        bus.publish(Event::system_log(
            LogLevel::Error,
            "transport.network",
            format!("{port_name} 连接失败：{reason}"),
        ));
        connecting.store(false, Ordering::Release);
        alive.store(false, Ordering::Release);
    };
    let addr = match (config.host.as_str(), config.port).to_socket_addrs() {
        Ok(mut addrs) => match addrs.next() {
            Some(addr) => addr,
            None => {
                connect_failed(format!("无法解析主机名 {}", config.host));
                return;
            }
        },
        Err(error) => {
            connect_failed(format!("DNS 解析失败：{error}"));
            return;
        }
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
        alive.store(false, Ordering::Release);
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
    let _ = ws.send(Message::text(
        r#"{"jsonrpc":"2.0","method":"server.connection.identify","params":{"client_name":"Hardware Workbench","version":"1.0.0","type":"desktop"},"id":1}"#,
    ));

    let mut next_id: u64 = 2;
    while !stop.load(Ordering::Acquire) {
        // 发送队列：文本 → printer.gcode.script
        while let Ok(bytes) = write_rx.try_recv() {
            let script = String::from_utf8_lossy(&bytes).into_owned();
            if script.trim().is_empty() {
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
                }
                Err(error) => {
                    bus.publish(Event::system_log(
                        LogLevel::Error,
                        "transport.network",
                        format!("{port_name} 发送失败：{error}"),
                    ));
                    alive.store(false, Ordering::Release);
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
                bus.publish(Event::system_log(
                    LogLevel::Error,
                    "transport.network",
                    format!("{port_name} 连接断开：{error}"),
                ));
                alive.store(false, Ordering::Release);
                return;
            }
        }
    }
    // 正常关闭：发送 Close 帧后释放连接
    let _ = ws.close(None);
    alive.store(false, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::parse_gcode_response;
    use crate::{NetworkSerialConfig, TransportManager, serial_topics};
    use std::thread;
    use std::time::Duration;
    use tool_core::Payload;
    use tool_databus::{DataBus, TopicFilter};
    use tungstenite::Message;

    #[test]
    fn extracts_gcode_response() {
        let msg = r#"{"jsonrpc":"2.0","method":"notify_gcode_response","params":["ok BED_SIZE"]}"#;
        assert_eq!(parse_gcode_response(msg).as_deref(), Some("ok BED_SIZE"));
    }

    #[test]
    fn ignores_status_update() {
        let msg = r#"{"jsonrpc":"2.0","method":"notify_status_update","params":[{"print_stats":{"state":"printing"}}]}"#;
        assert_eq!(parse_gcode_response(msg), None);
    }

    #[test]
    fn ignores_plain_responses_and_non_json() {
        let msg = r#"{"jsonrpc":"2.0","result":"ok","id":1}"#;
        assert_eq!(parse_gcode_response(msg), None);
        assert_eq!(parse_gcode_response("not json"), None);
    }

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
