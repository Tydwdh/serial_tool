use crossbeam_channel::{Sender, bounded};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serialport as sp;
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thiserror::Error;
use tool_core::{Direction, Event, LogLevel, Payload};
use tool_databus::DataBus;

mod network;
pub use network::NetworkSerialConfig;

#[cfg(windows)]
mod windows_native;

/// A command executed by exactly one port worker.
///
/// Keeping data and modem-control operations in the same FIFO is important:
/// `Write(A); SetDtr(false); Write(B)` must reach the device in that order.
/// The optional completion channel is used by the platform backend when its
/// async operation must mean "the worker has completed the command", rather
/// than merely "the command was accepted into a queue".
///
/// 载荷是 `io::Result<()>` —— **错误本体，不是它的 `Display`**。通道另一头的
/// `wait_for_completion` 把结果原样包成 `TransportError::Io`，而 `translate_error`
/// 按 `kind()` 决定给用户哪句可执行提示（第 41 轮）；这里一旦降级成字符串，
/// kind 就在通道上被抹掉，等待方只能重新造一条 `ErrorKind::Other`。
/// `SerialCommand` 是 `pub(crate)`，这个载荷类型不出 crate。
pub(crate) enum SerialCommand {
    Write {
        bytes: Vec<u8>,
        completion: Option<Sender<std::io::Result<()>>>,
    },
    SetDtr {
        value: bool,
        completion: Option<Sender<std::io::Result<()>>>,
    },
    SetRts {
        value: bool,
        completion: Option<Sender<std::io::Result<()>>>,
    },
}

/// UI 重绘唤醒器。由 app 层注入，worker 在 publish RX/TX 事件后调用，
/// 使 UI 立即重绘而非等待 80ms 轮询。
///
/// 实现应为 `Weak::upgrade + has_repaint + request_repaint` 的轻量闭包。
/// 失败（Weak 失效）必须静默忽略，不得 panic。
pub trait RepaintWaker: Send + Sync + 'static {
    fn wake(&self);
}

impl<F: Fn() + Send + Sync + 'static> RepaintWaker for F {
    fn wake(&self) {
        (self)();
    }
}

/// 串口 topic 常量。从 tool_core::topics 上移至此，core 中保留向后兼容 re-export。
pub mod serial_topics {
    pub const SERIAL_RX: &str = "transport.serial.default.rx";
    pub const SERIAL_TX: &str = "transport.serial.default.tx";
}

/// 从 source 字符串中提取端口名（去除 "serial:" 前缀）。
fn extract_port(source: &str) -> String {
    source.strip_prefix("serial:").unwrap_or(source).to_owned()
}

/// 构建串口事件的通用方法。
fn serial_event(
    topic: &str,
    direction: Direction,
    source: impl Into<String>,
    bytes: Vec<u8>,
) -> Event {
    let source = source.into();
    let port = extract_port(&source);
    Event::new(topic, source, direction, Payload::Bytes(bytes))
        .with_metadata(serde_json::json!({ "port": port }))
}

/// 构建串口 RX 事件。
pub fn serial_rx_event(source: impl Into<String>, bytes: Vec<u8>) -> Event {
    serial_event(serial_topics::SERIAL_RX, Direction::Rx, source, bytes)
}

/// 构建串口 TX 事件。
pub fn serial_tx_event(source: impl Into<String>, bytes: Vec<u8>) -> Event {
    serial_event(serial_topics::SERIAL_TX, Direction::Tx, source, bytes)
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("no serial port is open")]
    NoOpenPort,
    #[error("port '{0}' is not open")]
    PortNotOpen(String),
    #[error("invalid hex input: {0}")]
    InvalidHex(String),
    #[error("serial error: {0}")]
    Serial(#[from] sp::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serial worker is closed")]
    WorkerClosed,
    #[error("serial write queue is full — 发送过快，请降低频率")]
    QueueFull,
}

pub type TransportResult<T> = Result<T, TransportError>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataBits {
    Five,
    Six,
    Seven,
    Eight,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopBits {
    One,
    Two,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Parity {
    None,
    Odd,
    Even,
}

/// 解析数据位字符串，无效输入默认 `DataBits::Eight`。
pub fn parse_data_bits(v: &str) -> DataBits {
    match v {
        "5" => DataBits::Five,
        "6" => DataBits::Six,
        "7" => DataBits::Seven,
        _ => DataBits::Eight,
    }
}

/// 解析停止位字符串，无效输入默认 `StopBits::One`。
pub fn parse_stop_bits(v: &str) -> StopBits {
    match v {
        "2" => StopBits::Two,
        _ => StopBits::One,
    }
}

/// 解析校验位字符串，无效输入默认 `Parity::None`。
pub fn parse_parity(v: &str) -> Parity {
    match v {
        "odd" => Parity::Odd,
        "even" => Parity::Even,
        _ => Parity::None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SerialConfig {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: DataBits,
    pub stop_bits: StopBits,
    pub parity: Parity,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port_name: String::new(),
            baud_rate: 115_200,
            data_bits: DataBits::Eight,
            stop_bits: StopBits::One,
            parity: Parity::None,
        }
    }
}

/// 串口类型描述。从 `serialport::SerialPortType` 映射而来。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum PortType {
    /// USB 串口，可选附带产品名。
    #[serde(rename = "usb")]
    Usb(String),
    /// 蓝牙串口。
    #[serde(rename = "bluetooth")]
    Bluetooth,
    /// PCI 串口。
    #[serde(rename = "pci")]
    Pci,
    /// 网络模拟串口（WebSocket + JSON-RPC gcode 桥，Nexus Prime 等 Klipper 服务器）。
    #[serde(rename = "network")]
    Network,
    /// 未知类型。
    #[default]
    #[serde(rename = "unknown")]
    Unknown,
}

impl std::fmt::Display for PortType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usb(product) => {
                if product.is_empty() {
                    write!(f, "USB")
                } else {
                    write!(f, "{product}")
                }
            }
            Self::Bluetooth => write!(f, "Bluetooth"),
            Self::Pci => write!(f, "PCI"),
            Self::Network => write!(f, "网络"),
            Self::Unknown => write!(f, ""),
        }
    }
}

fn from_serialport_type(port_type: sp::SerialPortType) -> PortType {
    match port_type {
        sp::SerialPortType::UsbPort(usb) => {
            PortType::Usb(usb.product.unwrap_or_else(|| "USB".to_owned()))
        }
        sp::SerialPortType::BluetoothPort => PortType::Bluetooth,
        sp::SerialPortType::PciPort => PortType::Pci,
        sp::SerialPortType::Unknown => PortType::Unknown,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SerialPortDescriptor {
    pub port_name: String,
    pub port_type: PortType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportStatus {
    pub open: bool,
    pub port_name: Option<String>,
    pub baud_rate: Option<u32>,
    /// 网络模拟串口连接中（异步连接尚未完成）。
    #[serde(default)]
    pub connecting: bool,
}

impl TransportStatus {
    pub fn closed() -> Self {
        Self {
            open: false,
            port_name: None,
            baud_rate: None,
            connecting: false,
        }
    }
}

/// 串口生命周期管理器。
///
/// # Safety / 所有权
///
/// `TransportManager` **不实现** `Drop`（有意为之）。因为 `Clone` 被
/// `LuaPluginRuntime`、`PluginManager` 多处持有，任意一个 clone 被 drop
/// 会误关所有串口。关闭串口的**唯一安全调用点**是 `WorkbenchApp::drop()`
/// 中调用的 `close_serial()`。
///
/// 如果外部代码意外 drop 了一个 `TransportManager` clone，串口 worker 线程
/// 将继续运行（`Arc` 中的 `PortHandle` 仍存活），线程不会泄漏。
#[derive(Clone)]
pub struct TransportManager {
    bus: DataBus,
    ports: Arc<Mutex<HashMap<String, PortHandle>>>,
    closing: Arc<Mutex<Vec<ClosingHandle>>>,
    /// 上次 reap_closing 的时间戳，用于节流。
    last_reap_time: Arc<AtomicU64>,
    /// UI 重绘唤醒器，app 层注入。worker publish 串口事件后调用以立即重绘。
    /// `Arc<Mutex<Option<...>>>` 让所有 TransportManager clone 共享同一 waker（仅 app 启动时设一次）。
    repaint_waker: Arc<Mutex<Option<Arc<dyn RepaintWaker>>>>,
}

/// 插件场景测试使用的内存串口控制器。它走与真实串口相同的
/// `TransportManager::send_to` 和 DataBus TX/RX 事件链路。
#[derive(Clone)]
pub struct VirtualSerialPort {
    bus: DataBus,
    port_name: String,
}

impl VirtualSerialPort {
    pub fn port_name(&self) -> &str {
        &self.port_name
    }

    pub fn inject_rx(&self, bytes: impl Into<Vec<u8>>) {
        self.bus.publish(serial_rx_event(
            format!("serial:{}", self.port_name),
            bytes.into(),
        ));
    }
}

struct PortHandle {
    config: SerialConfig,
    /// 网络模拟串口连接中标记：worker 异步连接完成前为 true。
    /// 真实串口连接是同步的，恒为 false。
    connecting: Arc<AtomicBool>,
    writer: Sender<SerialCommand>,
    #[cfg(windows)]
    wake: Option<Arc<windows_native::WakeEvent>>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

struct ClosingHandle {
    port_name: String,
    baud_rate: u32,
    join: JoinHandle<()>,
}

impl PortHandle {
    /// 请求 worker 退出并取走它的 `JoinHandle`。
    ///
    /// 三步顺序固定：先以 Release 置 `stop`，再唤醒 Windows 上阻塞等待的 worker，
    /// 最后 `take` 走 join。`close_port` / `close_port_blocking` / `reap_dead_ports`
    /// 三条关闭路径共用这一份实现，顺序也因此只有一处需要审。
    fn request_stop(&mut self) -> Option<JoinHandle<()>> {
        self.stop.store(true, Ordering::Release);
        #[cfg(windows)]
        if let Some(wake) = &self.wake {
            wake.set();
        }
        self.join.take()
    }
}

/// worker 线程退出（正常返回 **或 panic 展开**）时把 `alive` 置 false。
///
/// 显式 `alive.store(false)` 只写在 `return` 路径上，panic 展开会全部跳过，
/// 于是 `reap_dead_ports` 匹配不到、同名重开复用死句柄 —— 端口成为不可恢复
/// 的僵尸。改由 `Drop` 承担后展开路径同样生效。
///
/// Release 与各调用方（`open_serial` / `enqueue_command` / `reap_dead_ports`）的
/// Acquire load 配对，确保弱内存模型（ARM/AArch64）上"worker 已死"可见。
///
/// 本类型服务于两条 cfg 路径（非 Windows 的 `serial_worker_loop_impl`、Windows
/// 的 `NativeWorker::run`），因此**不得**置于任何 `cfg` 之下。
struct AliveGuard(Arc<AtomicBool>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// 端口句柄是否已死亡。两个互相独立的信号，任一成立即判定死亡：
///
/// - `alive`：worker 主动置位，或由 [`AliveGuard`] 在退出/展开时置位；
/// - `join.is_finished()`：线程已结束 —— 含 panic 展开，完全不依赖 worker
///   自己有没有记得写标志位。
///
/// 只信前者时，任何一个遗漏的置位点都会留下用户连"关掉重开"都做不到的僵尸端口。
fn port_is_dead(handle: &PortHandle) -> bool {
    !handle.alive.load(Ordering::Acquire)
        || handle.join.as_ref().is_some_and(|join| join.is_finished())
}

/// 单个句柄的状态快照：已死亡的句柄一律报 closed（不把僵尸端口显示为已打开），
/// 网络模拟串口在 `connecting` 期间同样 `open = false` —— 只有真实连上服务器
/// 之后才算打开，UI 据此区分"连接中"与"已断开"。
fn status_for(handle: &PortHandle) -> TransportStatus {
    if port_is_dead(handle) {
        return TransportStatus::closed();
    }
    let connecting = handle.connecting.load(Ordering::Relaxed);
    TransportStatus {
        open: !connecting,
        port_name: Some(handle.config.port_name.clone()),
        baud_rate: Some(handle.config.baud_rate),
        connecting,
    }
}

/// 等待 worker 回投的完成通知（"命令已被执行"，而非"命令已入队"）。
/// 超时与断连各映射到固定错误变体；`timed_out_message` 只是区分写入/控制信号
/// 两类超时的中文文案，不改变错误类型。
///
/// 通道里那条 `io::Error` **原样进 `TransportError::Io`**：不再 `Error::other(文本)`
/// 重新包装（第 42 轮）。旧写法把 worker 侧的 kind 一律抹成 `ErrorKind::Other`，
/// `translate_error` 按 kind 分派的那几臂（这条通道实际喂到的是 `PermissionDenied`
/// 与 `TimedOut`）对走过这条通道的失败因此全部不可达 —— 也就是第 41 轮记录的那处
/// 收窄的根因。`pub(crate)` 只为让 Windows worker 模块的用例能驱动同一个消费端。
pub(crate) fn wait_for_completion(
    result: crossbeam_channel::Receiver<std::io::Result<()>>,
    timeout: Duration,
    timed_out_message: &str,
) -> TransportResult<()> {
    result
        .recv_timeout(timeout)
        .map_err(|error| match error {
            crossbeam_channel::RecvTimeoutError::Timeout => TransportError::Io(
                std::io::Error::new(std::io::ErrorKind::TimedOut, timed_out_message),
            ),
            crossbeam_channel::RecvTimeoutError::Disconnected => TransportError::WorkerClosed,
        })?
        .map_err(TransportError::Io)
}

/// `send_control_blocking` 只接受 DTR/RTS 两种控制命令。
///
/// 刻意不直接收 `SerialCommand`：后者将来新增变体时，需要配 `completion` 的
/// 位置由**构造侧**（`send_control_blocking` 里的 match）穷尽覆盖，
/// 而不是靠一个运行期 `unreachable!()` 兜着。
enum BlockingControl {
    SetDtr { value: bool },
    SetRts { value: bool },
}

impl TransportManager {
    pub fn new(bus: DataBus) -> Self {
        Self {
            bus,
            ports: Arc::new(Mutex::new(HashMap::new())),
            closing: Arc::new(Mutex::new(Vec::new())),
            last_reap_time: Arc::new(AtomicU64::new(0)),
            repaint_waker: Arc::new(Mutex::new(None)),
        }
    }

    /// 注入 UI 重绘唤醒器。app 层在启动时调用一次，传入捕获 `Weak<egui::Context>` 的闭包。
    pub fn set_repaint_waker(&self, waker: Arc<dyn RepaintWaker>) {
        *self.repaint_waker.lock() = Some(waker);
    }

    /// 大小写不敏感解析已打开端口名。先在 HashMap 精确查找，再大小写宽松匹配。
    fn resolve_open_port_name_locked(
        ports: &HashMap<String, PortHandle>,
        requested: &str,
    ) -> Option<String> {
        if ports.contains_key(requested) {
            return Some(requested.to_owned());
        }
        ports
            .keys()
            .find(|name| name.eq_ignore_ascii_case(requested))
            .cloned()
    }

    /// 公开版本：返回已打开端口的规范名称，供 Lua API 等调用。
    pub fn canonical_open_port_name(&self, requested: &str) -> Option<String> {
        let guard = self.ports.lock();
        Self::resolve_open_port_name_locked(&guard, requested)
    }

    /// 清理已完成关闭的 worker 线程（join 并移除）。
    /// 节流：两次 reap 之间至少间隔 100ms，避免高频调用时反复加锁。
    fn reap_closing(&self) {
        const REAP_INTERVAL_MS: u64 = 100;
        let now_ms = tool_core::now_timestamp_ms();
        let last = self.last_reap_time.load(Ordering::Relaxed);
        if now_ms < last + REAP_INTERVAL_MS {
            return;
        }
        // CAS 更新 last_reap_time，失败说明其他线程已执行
        if self
            .last_reap_time
            .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let mut closing = self.closing.lock();
        let mut i = 0;
        while i < closing.len() {
            if closing[i].join.is_finished() {
                let h = closing.swap_remove(i);
                let _ = h.join.join();
                self.publish_closed(&h.port_name, h.baud_rate);
                // swap_remove 把最后一个元素移到了 i，不递增 i
            } else {
                i += 1;
            }
        }
    }

    fn publish_closed(&self, port_name: &str, baud_rate: u32) {
        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "transport.serial",
            format!("已关闭 {} @ {}", port_name, baud_rate),
        ));
        self.bus.publish(Event::new(
            tool_core::topics::SERIAL_CLOSED,
            format!("serial:{port_name}"),
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "port": port_name,
                "baud_rate": baud_rate,
            })),
        ));
    }

    pub fn list_serial_ports(&self) -> TransportResult<Vec<SerialPortDescriptor>> {
        self.reap_closing();
        self.reap_dead_ports();
        let mut ports: Vec<SerialPortDescriptor> = sp::available_ports()?
            .into_iter()
            .map(|info| SerialPortDescriptor {
                port_name: info.port_name,
                port_type: from_serialport_type(info.port_type),
            })
            .collect();
        ports.sort_by_key(|port| natural_sort_key(&port.port_name));
        Ok(ports)
    }

    // ── 打开端口 ──
    pub fn open_serial(&self, mut config: SerialConfig) -> TransportResult<()> {
        // 大小写不敏感端口名解析（用户可能输入 "com3" 而实际是 "COM3"）
        let resolved = sp::available_ports()
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.port_name.eq_ignore_ascii_case(&config.port_name));
        if let Some(p) = resolved {
            config.port_name = p.port_name;
        }
        // 先收割已完成关闭的旧 worker
        self.reap_closing();

        // 同配置重复打开：直接成功。必须是**活着**的同配置句柄才算成功——
        // 只看 alive 时，worker 忘记置位（或 guard 之前的一切版本）会把死句柄
        // 复用成 Ok，用户连"关掉重开"都做不到。
        {
            let guard = self.ports.lock();
            if let Some(existing) = guard.get(&config.port_name)
                && !port_is_dead(existing)
                && existing.config == config
            {
                return Ok(());
            }
        }

        // 同名端口正在关闭中，返回错误
        {
            let closing = self.closing.lock();
            if closing.iter().any(|h| h.port_name == config.port_name) {
                return Err(TransportError::Io(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    format!("{} 正在关闭中，请稍后重试", config.port_name),
                )));
            }
        }

        // 配置变化时：同步等待旧 worker 退出再打开
        self.close_port_blocking(&config.port_name, Duration::from_millis(100))?;

        let (writer, command_rx) = bounded::<SerialCommand>(1024);
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let thread_stop = Arc::clone(&stop);
        let thread_alive = Arc::clone(&alive);
        let thread_bus = self.bus.clone();
        let source = format!("serial:{}", config.port_name);
        let thread_source = source.clone();
        // 取 UI 重绘唤醒器（app 层注入），传给 worker 在 publish 后调用。
        let thread_waker = self.repaint_waker.lock().clone();

        #[cfg(windows)]
        let (join, wake) = {
            match windows_native::spawn_native_serial_worker(
                &config,
                command_rx,
                thread_stop,
                thread_alive,
                thread_bus,
                thread_source,
                thread_waker.clone(),
            ) {
                Ok((join, wake)) => (join, Some(wake)),
                Err(error) => {
                    self.bus.publish(Event::system_log(
                        LogLevel::Error,
                        "transport.serial",
                        format!(
                            "打开 {} @ {} 失败：{error}",
                            config.port_name, config.baud_rate
                        ),
                    ));
                    return Err(error);
                }
            }
        };

        #[cfg(not(windows))]
        let join = {
            let builder = sp::new(&config.port_name, config.baud_rate)
                .data_bits(config.data_bits.into())
                .stop_bits(config.stop_bits.into())
                .parity(config.parity.into())
                .timeout(Duration::from_millis(1));

            let port = builder.open().map_err(|error| {
                self.bus.publish(Event::system_log(
                    LogLevel::Error,
                    "transport.serial",
                    format!(
                        "打开 {} @ {} 失败：{error}",
                        config.port_name, config.baud_rate
                    ),
                ));
                TransportError::from(error)
            })?;

            thread::spawn(move || {
                serial_worker_loop_impl(
                    port,
                    command_rx,
                    thread_stop,
                    thread_alive,
                    thread_bus,
                    thread_source,
                    thread_waker.clone(),
                );
            })
        };

        self.ports.lock().insert(
            config.port_name.clone(),
            PortHandle {
                config: config.clone(),
                connecting: Arc::new(AtomicBool::new(false)),
                writer,
                #[cfg(windows)]
                wake,
                stop,
                alive,
                join: Some(join),
            },
        );

        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "transport.serial",
            format!("已打开 {} @ {}", config.port_name, config.baud_rate),
        ));

        // 发布结构化生命周期事件，供插件监听
        self.bus.publish(Event::new(
            tool_core::topics::SERIAL_OPENED,
            source,
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "port": config.port_name,
                "baud_rate": config.baud_rate,
            })),
        ));

        Ok(())
    }

    /// 打开不访问硬件的内存串口，供插件场景测试和 CI 使用。
    pub fn open_virtual_serial(
        &self,
        port_name: impl Into<String>,
    ) -> TransportResult<VirtualSerialPort> {
        let port_name = port_name.into();
        if port_name.trim().is_empty() {
            return Err(TransportError::PortNotOpen(port_name));
        }
        self.close_port_blocking(&port_name, Duration::from_millis(100))?;

        let config = SerialConfig {
            port_name: port_name.clone(),
            ..SerialConfig::default()
        };
        let (writer, command_rx) = bounded::<SerialCommand>(1024);
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let thread_stop = Arc::clone(&stop);
        let thread_alive = Arc::clone(&alive);
        let thread_bus = self.bus.clone();
        let source = format!("serial:{port_name}");
        let thread_source = source.clone();
        let join = thread::spawn(move || {
            // `execute_virtual_command` 若 panic，展开会跳过函数尾的显式 store，
            // 虚拟端口同样会变成不可重开的僵尸；统一交给 AliveGuard 收尾。
            let _alive_guard = AliveGuard(thread_alive);
            while !thread_stop.load(Ordering::Acquire) {
                match command_rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(command) => {
                        execute_virtual_command(&thread_bus, &thread_source, command);
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        self.ports.lock().insert(
            port_name.clone(),
            PortHandle {
                config: config.clone(),
                connecting: Arc::new(AtomicBool::new(false)),
                writer,
                #[cfg(windows)]
                wake: None,
                stop,
                alive,
                join: Some(join),
            },
        );
        self.bus.publish(Event::new(
            tool_core::topics::SERIAL_OPENED,
            source,
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "port": port_name,
                "baud_rate": config.baud_rate,
                "virtual": true,
            })),
        ));
        Ok(VirtualSerialPort {
            bus: self.bus.clone(),
            port_name,
        })
    }

    // ── 网络模拟串口（WebSocket + JSON-RPC gcode 桥）──

    /// 打开一个网络模拟串口：通过 WebSocket 连接 Nexus Prime（Klipper/Moonraker 系）
    /// 服务器，发送内容作为 `printer.gcode.script` 执行，`notify_gcode_response`
    /// 推送作为 RX 字节流发布。端口名使用 `host:port`，如 `192.168.1.100:7125`。
    ///
    /// 打开成功后与真实串口完全一致地接入 DataBus（TX/RX 事件、生命周期事件），
    /// 终端 / 发送器 / 录制 / 回放无需任何改动即可复用。
    pub fn open_network_serial(&self, config: NetworkSerialConfig) -> TransportResult<String> {
        self.reap_closing();
        let port_name = config.display_name();

        // 同名已在打开：直接成功（同样要求句柄真的还活着，见 open_serial 的说明）
        {
            let guard = self.ports.lock();
            if let Some(existing) = guard.get(&port_name)
                && !port_is_dead(existing)
            {
                return Ok(port_name);
            }
        }

        // 配置变化时：同步等待旧 worker 退出再打开
        self.close_port_blocking(&port_name, Duration::from_millis(100))?;

        let (writer, command_rx) = bounded::<SerialCommand>(1024);
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let connecting = Arc::new(AtomicBool::new(true));
        let source = format!("serial:{port_name}");

        // worker 拿走的是同一批 Arc 的副本：`stop` / `alive` / `connecting` 之后还要
        // 装进 `PortHandle`，`source` 还要发生命周期事件，所以这里逐个 clone。
        // 取 UI 重绘唤醒器（app 层注入），传给 worker 在 publish 后调用；先落进局部
        // 变量再构造上下文，是为了让锁在语句结束就释放，不要横跨 `thread::spawn`。
        let waker = self.repaint_waker.lock().clone();
        let join = network::spawn_network_worker(network::NetworkWorkerContext {
            config: config.clone(),
            command_rx,
            stop: Arc::clone(&stop),
            alive: Arc::clone(&alive),
            connecting: Arc::clone(&connecting),
            bus: self.bus.clone(),
            source: source.clone(),
            waker,
        })?;

        self.ports.lock().insert(
            port_name.clone(),
            PortHandle {
                // 网络端口把“波特率”位置复用作服务器端口，供日志显示。
                config: SerialConfig {
                    port_name: port_name.clone(),
                    baud_rate: config.port as u32,
                    ..SerialConfig::default()
                },
                connecting,
                writer,
                #[cfg(windows)]
                wake: None,
                stop,
                alive,
                join: Some(join),
            },
        );

        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "transport.network",
            format!("已连接 {port_name}"),
        ));

        // 发布结构化生命周期事件，供插件监听
        self.bus.publish(Event::new(
            tool_core::topics::SERIAL_OPENED,
            source,
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "port": port_name,
                "network": true,
            })),
        ));

        Ok(port_name)
    }

    // ── 关闭所有端口（同步，供 shutdown 使用）──
    pub fn close_serial(&self) {
        self.reap_closing();
        // 取出所有端口，设置 stop，移入 closing
        let names: Vec<String> = self.ports.lock().keys().cloned().collect();
        for name in names {
            self.close_port(&name);
        }
        // shutdown 路径：取出所有 closing handle，释放锁后再 join
        let handles = {
            let mut remaining = self.closing.lock();
            std::mem::take(&mut *remaining)
        };
        for h in handles {
            let _ = h.join.join();
            self.publish_closed(&h.port_name, h.baud_rate);
        }
    }

    // ── 关闭指定端口（异步：设 stop 并移入 closing，不 join）──
    pub fn close_port(&self, port_name: &str) {
        // 先从 ports 中取出 worker，释放锁后再操作 closing
        let closing_info = {
            let mut guard = self.ports.lock();
            let key = Self::resolve_open_port_name_locked(&guard, port_name)
                .unwrap_or_else(|| port_name.to_owned());
            guard.remove(&key).map(|mut worker| {
                let join = worker.request_stop();
                let port_name = worker.config.port_name.clone();
                let baud_rate = worker.config.baud_rate;
                (port_name, baud_rate, join)
            })
        };
        if let Some((port_name, baud_rate, Some(join))) = closing_info {
            // 不发 closing 中间态日志：紧接着会有 closed 日志（reap_closing 时），
            // 且状态栏已显示"已断开"，避免冗余。
            self.closing.lock().push(ClosingHandle {
                port_name,
                baud_rate,
                join,
            });
        }
    }

    /// 同步关闭端口，等待 worker 线程完全退出。仅在重连等场景使用。
    /// 超时时将 JoinHandle 放入 closing 队列回收，避免线程泄漏。
    pub fn close_port_blocking(&self, port_name: &str, timeout: Duration) -> TransportResult<()> {
        let key = self
            .canonical_open_port_name(port_name)
            .unwrap_or_else(|| port_name.to_owned());
        let (old_name, old_baud, join) = {
            let mut guard = self.ports.lock();
            let Some(mut worker) = guard.remove(&key) else {
                return Ok(());
            };
            let join = worker.request_stop();
            (
                worker.config.port_name.clone(),
                worker.config.baud_rate,
                join,
            )
        };
        let Some(join) = join else {
            return Ok(());
        };
        let deadline = std::time::Instant::now() + timeout;
        while !join.is_finished() {
            if std::time::Instant::now() > deadline {
                self.closing.lock().push(ClosingHandle {
                    port_name: old_name,
                    baud_rate: old_baud,
                    join,
                });
                return Err(TransportError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("{port_name} 正在关闭中"),
                )));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = join.join();
        self.publish_closed(&old_name, old_baud);
        Ok(())
    }

    // ── 发送到指定端口 ──
    pub fn send_to(&self, port_name: &str, bytes: Vec<u8>) -> TransportResult<()> {
        self.enqueue_command(
            port_name,
            SerialCommand::Write {
                bytes,
                completion: None,
            },
        )
    }

    /// 入队一次写入并等待唯一的端口 worker 真正执行完毕 —— 这是异步平台后端使用的语义。
    pub fn send_to_blocking(
        &self,
        port_name: &str,
        bytes: Vec<u8>,
        timeout: Duration,
    ) -> TransportResult<()> {
        let (completion, result) = bounded(1);
        self.enqueue_command(
            port_name,
            SerialCommand::Write {
                bytes,
                completion: Some(completion),
            },
        )?;
        wait_for_completion(result, timeout, "串口写入超时")
    }

    fn enqueue_command(&self, port_name: &str, command: SerialCommand) -> TransportResult<()> {
        self.reap_closing();
        let writer = {
            let guard = self.ports.lock();
            let resolved = Self::resolve_open_port_name_locked(&guard, port_name)
                .ok_or_else(|| TransportError::PortNotOpen(port_name.to_owned()))?;
            let worker = guard
                .get(&resolved)
                .ok_or_else(|| TransportError::PortNotOpen(port_name.to_owned()))?;
            // 死亡判定提前于取 writer：这是一处**防御性早退**，不是"UI 一路显示已发"
            // 那个症状的修复。worker 按值持有 `Receiver`（本 crate 内没有 clone，也没有
            // catch_unwind/forget），线程一结束 receiver 就被 drop，旧代码走到底同样在
            // `try_send` 上拿到 `Disconnected` → `WorkerClosed`。换判定真正买到的是：省掉
            // 下面那次为 `wake` 的第二次 `ports.lock()`，以及判死不再依赖单个信号 ——
            // `join` 已被 `take()` 走的句柄由 `alive` 兜住，忘了置位的线程由
            // `is_finished()` 兜住。
            // 仍然漏掉的是**卡住却没退出**的 worker（`alive` 为 true、`is_finished()` 为
            // false，`bounded(1024)` 照旧无声填满）：那需要心跳/超时，不在本改动范围内。
            if port_is_dead(worker) {
                return Err(TransportError::WorkerClosed);
            }
            worker.writer.clone()
        };
        #[cfg(windows)]
        let wake = {
            let guard = self.ports.lock();
            let resolved = Self::resolve_open_port_name_locked(&guard, port_name)
                .ok_or_else(|| TransportError::PortNotOpen(port_name.to_owned()))?;
            guard.get(&resolved).and_then(|worker| worker.wake.clone())
        };
        writer.try_send(command).map_err(|e| match e {
            crossbeam_channel::TrySendError::Full(_) => TransportError::QueueFull,
            crossbeam_channel::TrySendError::Disconnected(_) => TransportError::WorkerClosed,
        })?;
        #[cfg(windows)]
        if let Some(wake) = wake {
            wake.set();
        }
        Ok(())
    }

    // ── 向后兼容：发送到第一个已打开端口（按端口名排序保证确定性） ──
    pub fn send(&self, bytes: Vec<u8>) -> TransportResult<()> {
        let guard = self.ports.lock();
        let name = guard
            .keys()
            .min() // 按字典序取最小，保证确定性而非 HashMap 随机
            .cloned()
            .ok_or(TransportError::NoOpenPort)?;
        drop(guard);
        self.send_to(&name, bytes)
    }

    pub fn send_text_to(&self, port_name: &str, text: &str) -> TransportResult<()> {
        self.send_to(port_name, text.as_bytes().to_vec())
    }

    pub fn send_hex_to(&self, port_name: &str, input: &str) -> TransportResult<()> {
        // HEX 判定唯一真相在 `tool_core`（native/wasm 共用）；这里只把它返回的裸文案
        // 重建成本 crate 的错误变体，`translate_error` 的中文提示不变。
        let bytes = tool_core::parse_hex(input).map_err(TransportError::InvalidHex)?;
        self.send_to(port_name, bytes)
    }

    // ── 状态 ──
    pub fn status_port(&self, port_name: &str) -> TransportStatus {
        self.reap_closing();
        self.reap_dead_ports();
        let guard = self.ports.lock();
        let key = Self::resolve_open_port_name_locked(&guard, port_name)
            .unwrap_or_else(|| port_name.to_owned());
        match guard.get(&key) {
            Some(handle) => status_for(handle),
            None => TransportStatus::closed(),
        }
    }

    pub fn status_all(&self) -> Vec<TransportStatus> {
        self.reap_closing();
        self.reap_dead_ports();
        self.ports.lock().values().map(status_for).collect()
    }

    pub fn open_ports(&self) -> Vec<String> {
        self.reap_closing();
        self.reap_dead_ports();
        self.ports.lock().keys().cloned().collect()
    }

    pub fn set_dtr(&self, port_name: &str, value: bool) -> TransportResult<()> {
        self.enqueue_command(
            port_name,
            SerialCommand::SetDtr {
                value,
                completion: None,
            },
        )
    }

    pub fn set_rts(&self, port_name: &str, value: bool) -> TransportResult<()> {
        self.enqueue_command(
            port_name,
            SerialCommand::SetRts {
                value,
                completion: None,
            },
        )
    }

    pub fn set_dtr_blocking(
        &self,
        port_name: &str,
        value: bool,
        timeout: Duration,
    ) -> TransportResult<()> {
        self.send_control_blocking(port_name, BlockingControl::SetDtr { value }, timeout)
    }

    pub fn set_rts_blocking(
        &self,
        port_name: &str,
        value: bool,
        timeout: Duration,
    ) -> TransportResult<()> {
        self.send_control_blocking(port_name, BlockingControl::SetRts { value }, timeout)
    }

    fn send_control_blocking(
        &self,
        port_name: &str,
        control: BlockingControl,
        timeout: Duration,
    ) -> TransportResult<()> {
        let (sender, result) = bounded(1);
        let command = match control {
            BlockingControl::SetDtr { value } => SerialCommand::SetDtr {
                value,
                completion: Some(sender),
            },
            BlockingControl::SetRts { value } => SerialCommand::SetRts {
                value,
                completion: Some(sender),
            },
        };
        self.enqueue_command(port_name, command)?;
        wait_for_completion(result, timeout, "串口控制信号超时")
    }

    /// 清理已退出 worker 的 stale port handle（[`port_is_dead`] 的两个信号任一成立）。
    /// 可在 status 查询、list/open/close 或定时刷新时调用。
    ///
    /// 在 ports 锁内完成 dead handle 的 remove + stop + 取 join，仅把 push(closing)
    /// 移到锁外。这样消除原实现"释放锁后逐个 close_port"的 TOCTOU 窗口——若另一
    /// 线程在窗口内以同名+新配置重新打开，旧 close_port 会误关全新且 alive 的 handle。
    pub fn reap_dead_ports(&self) {
        // 收集 dead handle 的完整信息（含 join），在锁内移除，避免与并发 reopen 竞态。
        let dead: Vec<(String, u32, Option<JoinHandle<()>>)> = {
            let mut guard = self.ports.lock();
            let dead_names: Vec<String> = guard
                .iter()
                .filter(|(_, h)| port_is_dead(h))
                .map(|(name, _)| name.clone())
                .collect();
            dead_names
                .into_iter()
                .filter_map(|name| {
                    let mut handle = guard.remove(&name)?;
                    // request_stop 里的 stop 已无意义（worker 已死），但保持三条关闭
                    // 路径对称并防御性置位。
                    let join = handle.request_stop();
                    Some((
                        handle.config.port_name.clone(),
                        handle.config.baud_rate,
                        join,
                    ))
                })
                .collect()
        };
        for (name, baud, join) in dead {
            self.bus.publish(Event::system_log(
                LogLevel::Error,
                "transport.serial",
                format!("串口 {name} @ {baud} 已断开连接"),
            ));
            if let Some(join) = join {
                self.closing.lock().push(ClosingHandle {
                    port_name: name,
                    baud_rate: baud,
                    join,
                });
            }
        }
    }
}

// ── 串口 I/O trait ──

/// 串口读写抽象，使 `serial_worker_loop_impl` 可测试。
/// 生产实现：`Box<dyn sp::SerialPort>`（通过 blanket impl 自动满足）。
/// 测试实现：`MockSerialPort`。
#[cfg(any(not(windows), test))]
trait SerialIo {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
    fn write_data_terminal_ready(&mut self, value: bool) -> std::io::Result<()>;
    fn write_request_to_send(&mut self, value: bool) -> std::io::Result<()>;
}

#[cfg(any(not(windows), test))]
impl SerialIo for Box<dyn sp::SerialPort> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        (**self).read(buf)
    }
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        (**self).write_all(buf)
    }
    fn write_data_terminal_ready(&mut self, value: bool) -> std::io::Result<()> {
        (**self)
            .write_data_terminal_ready(value)
            .map_err(|e| e.into())
    }
    fn write_request_to_send(&mut self, value: bool) -> std::io::Result<()> {
        (**self).write_request_to_send(value).map_err(|e| e.into())
    }
}

// ── 串口工作线程 ──

// 入参即串口线程的全部运行环境（端口、命令通道、停止/存活标志、总线、waker），
// 且与 `network.rs` 的 worker loop 签名刻意保持一致；包一层上下文结构体只会把
// 同一批值换个类型再搬一次。7 个参数正好在 clippy 的阈值上，因此这里**不需要**豁免；
// 将来若再加参数，请改签名而不是把 `too_many_arguments` 豁免加回模块级。
#[cfg(any(not(windows), test))]
fn serial_worker_loop_impl(
    mut port: impl SerialIo,
    command_rx: crossbeam_channel::Receiver<SerialCommand>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    bus: DataBus,
    source: String,
    waker: Option<Arc<dyn RepaintWaker>>,
) {
    // 线程退出（含 panic 展开）时清 alive；函数内不再有任何显式 store。
    let _alive_guard = AliveGuard(Arc::clone(&alive));
    let mut buffer = [0_u8; 4096];
    // 日志用干净端口名（去掉 "serial:" 前缀）。
    let port_name = extract_port(&source);
    let wake = || {
        if let Some(w) = &waker {
            w.wake();
        }
    };

    while !stop.load(Ordering::Acquire) {
        while let Ok(command) = command_rx.try_recv() {
            let keep_running =
                execute_serial_command(&mut port, command, &bus, &source, &port_name, &wake);
            if !keep_running {
                return;
            }
        }

        match port.read(&mut buffer) {
            Ok(0) => {}
            Ok(size) => {
                let mut data = buffer[..size].to_vec();
                // 内层 read loop 加预算：防止连续高速 RX 饿死写入/关闭
                let started = std::time::Instant::now();
                const MAX_EXTRA_READS: usize = 8;
                const MAX_EXTRA_READ_DURATION_MS: u64 = 5;
                let mut extra_reads = 0usize;
                loop {
                    if stop.load(Ordering::Relaxed)
                        || extra_reads >= MAX_EXTRA_READS
                        || started.elapsed() > Duration::from_millis(MAX_EXTRA_READ_DURATION_MS)
                    {
                        break;
                    }
                    match port.read(&mut buffer) {
                        Ok(more) if more > 0 => {
                            data.extend_from_slice(&buffer[..more]);
                            extra_reads += 1;
                        }
                        _ => break,
                    }
                }
                bus.publish(serial_rx_event(source.clone(), data));
                wake();
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => {
                bus.publish(Event::system_log(
                    LogLevel::Error,
                    "transport.serial",
                    format!("{port_name} 读取失败：{error}"),
                ));
                return;
            }
        }
    }
}

/// 把 worker 侧的执行结果回填给等待方：**错误按值搬进通道**，kind 与原始码都跟着走。
/// `io::Error` 不是 `Clone`，所以调用点此后不能再用它 —— 需要先记日志的（写失败、
/// 信号线设置失败）都在此之前记完。发送句柄在本行析构，与原内联写法同一个丢弃点。
#[cfg(any(not(windows), test))]
fn complete_command(completion: Option<Sender<std::io::Result<()>>>, result: std::io::Result<()>) {
    if let Some(completion) = completion {
        let _ = completion.send(result);
    }
}

/// 收尾一条 DTR/RTS 命令：失败只记日志，不让 worker 退出（与写入不同）。
/// 顺序是"先记日志、后交给通道"—— `io::Error` 不是 `Clone`，反过来就没有原件可发了。
#[cfg(any(not(windows), test))]
fn finish_signal_command(
    bus: &DataBus,
    port_name: &str,
    label: &str,
    result: std::io::Result<()>,
    completion: Option<Sender<std::io::Result<()>>>,
) {
    if let Err(error) = &result {
        bus.publish(Event::system_log(
            LogLevel::Error,
            "transport.serial",
            format!("{port_name} 设置 {label} 失败：{error}"),
        ));
    }
    complete_command(completion, result);
}

/// 执行一条命令，返回 **worker 是否继续运行**：只有写失败会终止 worker。
/// 这里不用 `io::Result<()>`：那条错误的两个消费者（系统日志、完成通道）都在本函数
/// 内用掉它了（`io::Error` 不是 `Clone`），循环要的只是"该不该退出"这一个事实。
#[cfg(any(not(windows), test))]
fn execute_serial_command(
    port: &mut impl SerialIo,
    command: SerialCommand,
    bus: &DataBus,
    source: &str,
    port_name: &str,
    wake: &impl Fn(),
) -> bool {
    match command {
        SerialCommand::Write { bytes, completion } => match port.write_all(&bytes) {
            Ok(()) => {
                complete_command(completion, Ok(()));
                bus.publish(serial_tx_event(source.to_owned(), bytes));
                wake();
                true
            }
            Err(error) => {
                // 与完成通道同理：日志先借用错误出，错误本体最后交给等待方。
                bus.publish(Event::system_log(
                    LogLevel::Error,
                    "transport.serial",
                    format!("{port_name} 写入失败：{error}"),
                ));
                complete_command(completion, Err(error));
                false
            }
        },
        // 与写入不同：信号线设置失败只记日志，不让 worker 退出。
        SerialCommand::SetDtr { value, completion } => {
            finish_signal_command(
                bus,
                port_name,
                "DTR",
                port.write_data_terminal_ready(value),
                completion,
            );
            true
        }
        SerialCommand::SetRts { value, completion } => {
            finish_signal_command(
                bus,
                port_name,
                "RTS",
                port.write_request_to_send(value),
                completion,
            );
            true
        }
    }
}

fn execute_virtual_command(bus: &DataBus, source: &str, command: SerialCommand) {
    match command {
        SerialCommand::Write { bytes, completion } => {
            bus.publish(serial_tx_event(source.to_owned(), bytes));
            if let Some(completion) = completion {
                let _ = completion.send(Ok(()));
            }
        }
        SerialCommand::SetDtr { completion, .. } | SerialCommand::SetRts { completion, .. } => {
            if let Some(completion) = completion {
                let _ = completion.send(Ok(()));
            }
        }
    }
}

// HEX 解析（含 `0x` 前缀、`_`/`-` 分隔符、补 0 与严格模式规则）已下沉到
// `tool_core::{parse_hex, parse_hex_strict}`：本 crate 只负责投递，不再持有判定规则。
// 调用点用 `.map_err(TransportError::InvalidHex)` 把裸文案重建成本 crate 的错误变体。

pub fn natural_sort_key(name: &str) -> (String, u64) {
    let prefix: String = name.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let number: u64 = name
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    (prefix, number)
}

impl From<DataBits> for sp::DataBits {
    fn from(v: DataBits) -> Self {
        match v {
            DataBits::Five => Self::Five,
            DataBits::Six => Self::Six,
            DataBits::Seven => Self::Seven,
            DataBits::Eight => Self::Eight,
        }
    }
}

impl From<StopBits> for sp::StopBits {
    fn from(v: StopBits) -> Self {
        match v {
            StopBits::One => Self::One,
            StopBits::Two => Self::Two,
        }
    }
}

impl From<Parity> for sp::Parity {
    fn from(v: Parity) -> Self {
        match v {
            Parity::None => Self::None,
            Parity::Odd => Self::Odd,
            Parity::Even => Self::Even,
        }
    }
}

// ── 发送辅助函数 ──
//
// 「输入框内容 → 字节」的判定（HEX 解码档位、换行后缀、事务性）不在本 crate：
// 唯一真相是 application 层的 `send_plan`（依赖方向仍是 application → transport），
// 本 crate 只接受已经计划好的字节，剩余的 HEX 入口只有插件侧的 `send_hex_to`。
// 这里曾有第二份实现（`send_impl_to`：按行拆分 HEX、空白输入静默 no-op、错误文案自创
// 「无效HEX：」），它是桌面端周期发送与点发送行为分叉的根源，随该路径收敛一并删除。

/// 将传输错误翻译为用户友好的中文提示。
///
/// 按 `TransportError` 变体类型化分发，而非字符串匹配（文案微调不会导致漏译）。
/// 调用方应在错误产生时立即调用本函数并保存返回的中文文案，而非保存原始
/// `Display` 字符串后再翻译。
pub fn translate_error(err: &TransportError) -> String {
    match err {
        TransportError::NoOpenPort => "未打开任何串口".into(),
        TransportError::PortNotOpen(port) => format!("串口 {port} 未打开（可能已断开，请重连）"),
        TransportError::WorkerClosed => "串口工作线程已关闭（可能已断开，请重连）".into(),
        TransportError::QueueFull => "发送队列已满：发送过快，请降低频率".into(),
        TransportError::InvalidHex(msg) => format!("无效HEX：{msg}"),
        TransportError::Serial(e) => {
            // 将常见的英文 serialport 错误转为中文，方便用户排查。
            let msg = e.to_string();
            let msg_lower = msg.to_ascii_lowercase();
            if is_permission_denied(&msg_lower) {
                serial_permission_message(&msg)
            } else if msg_lower.contains("device not found")
                || msg_lower.contains("not found")
                || msg_lower.contains("does not exist")
            {
                format!("串口设备不存在：{msg}")
            } else if msg_lower.contains("timeout") {
                format!("串口操作超时：{msg}")
            } else {
                format!("串口错误：{msg}")
            }
        }
        TransportError::Io(e) => {
            let message = e.to_string();
            match e.kind() {
                // 权限失败读 kind，不读 OS 文案（第 41 轮）：文案随系统语言本地化
                // （中文 Windows 实测「拒绝访问。 (os error 5)」，不含任何英文短语，
                // 且 Windows 报的码是 5 不是 13）。无需再兜 `raw_os_error()`：
                // `from_raw_os_error` 造的错必然已过 std 的码表（Windows
                // `ERROR_ACCESS_DENIED`/`WSAEACCES`、POSIX `EACCES`/`EPERM`
                // ⇒ `PermissionDenied`，std 1.92 pal 源码核过，Windows 两格另有
                // 本机用例实测），`Error::new`/`other` 造的错又没有原始码 ——
                // kind 之外再抄一份码表是永远多不出命中的死分支。
                // 第 42 轮起这条对**worker 侧的失败**也成立：端口 worker 的完成通道
                // 载荷已是 `io::Error` 本体（旧载荷是它的 `ToString`，等待方只能重包
                // 成 `ErrorKind::Other` —— 第 41 轮如实记下的那处收窄，根因在此）。
                std::io::ErrorKind::PermissionDenied => serial_permission_message(&message),
                // 「COM3 正在关闭中，请稍后重试」那句中文由 `open_serial` 直接 return
                // （本轮普查：它是全仓唯一造出 `TransportError::Io(WouldBlock)` 的出口 ——
                // `crates/recorder` 那句 WouldBlock 是 recorder 自己的 `io::Error`，进不来
                // 这个变体），从不经过完成通道 —— 所以"文案已含中文"这个前提没有被
                // 本轮换载荷牵连。
                std::io::ErrorKind::WouldBlock => message,
                std::io::ErrorKind::TimedOut => format!("操作超时：{message}"),
                // 原注释说的"HEX 严格模式奇偶校验文案"已不存在：HEX 判定下沉到
                // `tool_core::parse_hex`，失败走 `TransportError::InvalidHex`。本轮普查
                // （1.92 实测 Win32 码 0..4000）没有任何 os 码映射成 `InvalidData`，
                // 本 crate 也没有造它的出口 —— 这一臂当前无生产者，按原样透出的取舍
                // 与 `WouldBlock` 同族（正文已是给人看的句子），留着但不编造用例。
                std::io::ErrorKind::InvalidData => message,
                _ => format!("IO 错误：{message}"),
            }
        }
    }
}

/// 全仓最后一处「读英文文案猜权限」：第 41 轮把 `Io` 臂改读 `kind()` 后，
/// 只剩 `Serial` 臂还用它 —— `sp::Error::kind` 在 POSIX 后端能给出
/// `ErrorKind::Io(PermissionDenied)`，但 Windows 后端把 `ERROR_ACCESS_DENIED`
/// 折进 `ErrorKind::NoDevice`（与「设备不存在」共用），kind 在那半边不可判定；
/// 逐平台修法见 `docs/known-gaps.md`「串口权限错误的逐平台修法（Serial 臂）」。
fn is_permission_denied(message: &str) -> bool {
    message.contains("access is denied")
        || message.contains("access denied")
        || message.contains("permission denied")
        || message.contains("operation not permitted")
        || message.contains("os error 13")
}

fn serial_permission_message(detail: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        format!(
            "当前用户没有串口访问权限：{detail}\n\nUbuntu 用户通常需要加入 dialout 用户组：\n\nsudo usermod -aG dialout $USER\n\n完成后请注销并重新登录。不要使用 sudo 启动 Hardware Workbench。"
        )
    }

    #[cfg(not(target_os = "linux"))]
    {
        format!("串口被占用或无权限访问：{detail}，请检查是否已被其他程序打开")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use tool_databus::TopicFilter;

    // HEX 解析用例（原 `parses_spaced_hex` … `parse_hex_strict_rejects_odd_long_token`，共 10 条）
    // 随函数一并迁入 `crates/core/src/lib.rs` 的 `mod tests`，断言逐字未改。

    // ── #8: translate_error 按变体分发 ──
    /// 「翻译出来的文案是给人看的中文」这条判据的最小形式。
    /// 原来三处只写 `!is_empty()` —— 任何字符串都能过，包括把臂删成 `err.to_string()`
    /// 之后那口英文 `Description(...)`；`is_empty` 对那种回归完全无感。
    fn contains_chinese(text: &str) -> bool {
        text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
    }

    #[test]
    fn translate_error_covers_all_variants() {
        let no_port = translate_error(&TransportError::NoOpenPort);
        assert!(
            contains_chinese(&no_port),
            "NoOpenPort 的文案不是中文：{no_port:?}"
        );
        assert!(translate_error(&TransportError::PortNotOpen("COM3".into())).contains("COM3"));
        let worker_closed = translate_error(&TransportError::WorkerClosed);
        assert!(
            contains_chinese(&worker_closed),
            "WorkerClosed 的文案不是中文：{worker_closed:?}"
        );
        let queue_full = translate_error(&TransportError::QueueFull);
        assert!(
            contains_chinese(&queue_full),
            "QueueFull 的文案不是中文：{queue_full:?}"
        );
        assert!(translate_error(&TransportError::InvalidHex("bad".into())).contains("bad"));
        // 第 7 个变体 `Serial(sp::Error)` 这里**驱动不了**：全仓没有一处构造 `sp::Error`
        // （唯一的入口是 `#[from]`），本轮不为它编造构造方式。`Io` 变体已有自己的跨平台用例
        // （权限建议两格 + 超时/泛型框架），本轮另补的是它内部从未驱动过的两格透传，
        // 见 `translate_error_passes_the_already_chinese_io_arms_through`。
    }

    /// `Io` 臂五格里，`PermissionDenied` / `TimedOut` / `other` 三格**已有**跨平台用例
    /// （`translate_error_keeps_the_timeout_and_generic_frames_for_other_kinds` 等，Windows 跑得到 ——
    /// 本轮用变异核对覆盖时才发现，先前以为它们只在 linux）。剩下两格"原样透出"此前一处用例都没有：
    /// `WouldBlock`（那句「COM3 正在关闭中，请稍后重试」由 `open_serial` 直接造）与 `InvalidData`。
    /// 它们的契约恰恰是**不许**再包一层中文：包了就会出现「IO 错误：COM3 正在关闭中，请稍后重试」
    /// 这种两套说法叠在一起的用户可见文案。
    #[test]
    fn translate_error_passes_the_already_chinese_io_arms_through() {
        for (kind, detail) in [
            (
                std::io::ErrorKind::WouldBlock,
                "COM3 正在关闭中，请稍后重试",
            ),
            (
                std::io::ErrorKind::InvalidData,
                "文件不是本工具写出的录制格式",
            ),
        ] {
            let message = translate_error(&TransportError::Io(std::io::Error::new(kind, detail)));
            assert_eq!(
                message, detail,
                "kind {kind:?} 约定是原样透出（正文已是中文句子），再包一层会出现两套说法"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn translate_error_explains_linux_serial_group_permission() {
        let error = TransportError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Permission denied (os error 13)",
        ));
        let message = translate_error(&error);
        assert!(message.contains("dialout"));
        assert!(message.contains("不要使用 sudo 启动 Hardware Workbench"));
    }

    // ── 第 41 轮：`Io` 臂的权限分支按 kind 判定，不按 OS 文案 ──
    //
    // `serial_permission_message` 的正文按 cfg 分叉（Linux 讲 dialout 用户组，其余
    // 点名「串口被占用或无权限访问」），所以可移植的断言只有三件：OS 细节原样在场、
    // 没落到通用 `IO 错误：` 框架、本 cfg 下的区分句在场。与机器 UI 语言无关。
    fn assert_permission_advice(message: &str, detail: &str) {
        assert!(
            message.contains(detail),
            "OS 原话必须逐字留在建议里，实际: {message}"
        );
        assert!(
            !message.starts_with("IO 错误"),
            "权限失败不得落到通用 IO 框架，实际: {message}"
        );
        #[cfg(target_os = "linux")]
        assert!(
            message.contains("dialout"),
            "Linux 上的权限建议必须点名用户组，实际: {message}"
        );
        #[cfg(not(target_os = "linux"))]
        assert!(
            message.contains("串口被占用或无权限访问"),
            "非 Linux 上的权限建议必须点名占用/权限，实际: {message}"
        );
    }

    /// 本机（中文 Windows）权限失败的 Display 原话 —— 「拒绝访问。」不含
    /// `is_permission_denied` 的任何一个英文短语，Windows 报的码也是 5 不是 13，
    /// 所以旧实现从不进建议分支（红证用例）。kind 仍带着权限信息，按 kind 判定即通。
    #[test]
    fn translate_error_routes_a_localised_permission_failure_by_kind() {
        let message = translate_error(&TransportError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "拒绝访问。 (os error 5)",
        )));
        assert_permission_advice(&message, "拒绝访问。 (os error 5)");
    }

    /// 英文机器上旧文案谓词本来就命中 "access is denied"；改读 kind 后这一句必须
    /// 逐字不变 —— 这条钉的是「修本地化判定没有弄坏英文行为」。
    #[test]
    fn translate_error_keeps_english_permission_wording_on_the_same_advice() {
        let message = translate_error(&TransportError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Access is denied. (os error 5)",
        )));
        assert_permission_advice(&message, "Access is denied. (os error 5)");
    }

    /// 对照组：`TimedOut` 与未点名 kind 的框架不因权限判定的改动而变。
    #[test]
    fn translate_error_keeps_the_timeout_and_generic_frames_for_other_kinds() {
        assert_eq!(
            translate_error(&TransportError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "COM3 正在关闭中",
            ))),
            "操作超时：COM3 正在关闭中"
        );
        assert_eq!(
            translate_error(&TransportError::Io(std::io::Error::other("连接被拒绝"))),
            "IO 错误：连接被拒绝"
        );
    }

    /// 前提核实（Windows）：std 1.92 `pal/windows` 把 `ERROR_ACCESS_DENIED`(5)
    /// 映射成 `PermissionDenied`（`WSAEACCES` 同理），所以真 `from_raw_os_error`
    /// 造的错也直接进建议分支；`(os error 5)` 这半句是 Rust 自己追加的，
    /// 与系统语言无关，拿来当细节锚点。
    #[cfg(windows)]
    #[test]
    fn windows_raw_access_denied_reaches_the_permission_advice() {
        let raw = std::io::Error::from_raw_os_error(5);
        assert_eq!(
            raw.kind(),
            std::io::ErrorKind::PermissionDenied,
            "std 的映射前提变了（kind 不再是 PermissionDenied），这一族判定要重写"
        );
        let message = translate_error(&TransportError::Io(std::io::Error::from_raw_os_error(5)));
        assert_permission_advice(&message, "(os error 5)");
    }

    /// Windows 的 `os error 13` 是 `ERROR_INVALID_DATA`（数据无效），与 POSIX 的
    /// EACCES 无关 —— 旧实现的 `"os error 13"` 子串（照抄 POSIX 错误码）把它错包成
    /// 串口权限建议。按 kind 判定后它不得再进建议分支。
    #[cfg(windows)]
    #[test]
    fn windows_os_error_13_is_data_invalid_not_permission() {
        let raw = std::io::Error::from_raw_os_error(13);
        assert_ne!(
            raw.kind(),
            std::io::ErrorKind::PermissionDenied,
            "std 若把 13 映射成 PermissionDenied，本用例的前提（13≠EACCES）就变了"
        );
        let message = translate_error(&TransportError::Io(std::io::Error::from_raw_os_error(13)));
        assert!(
            !message.contains("串口被占用"),
            "非权限的 os error 13 又被文案谓词包成了串口权限建议，实际: {message}"
        );
        assert!(
            message.contains("(os error 13)"),
            "OS 细节必须原样在场，实际: {message}"
        );
    }

    /// 前提核实（POSIX）：std 1.92 `pal/unix` 有 `EACCES | EPERM => PermissionDenied`
    /// （`sys/pal/unix/mod.rs` 的 `decode_error_kind`），EACCES=13、EPERM=1，
    /// 旧谓词的 `"operation not permitted"`/`"os error 13"` 两句英文只有在英文
    /// locale 的机器上才命中（strerror 可被系统语言翻译）。
    #[cfg(unix)]
    #[test]
    fn posix_raw_eacces_and_eperm_reach_the_permission_advice() {
        for errno in [13, 1] {
            let raw = std::io::Error::from_raw_os_error(errno);
            assert_eq!(
                raw.kind(),
                std::io::ErrorKind::PermissionDenied,
                "errno {errno} 不再映射成 PermissionDenied，前提变了"
            );
            let message = translate_error(&TransportError::Io(std::io::Error::from_raw_os_error(
                errno,
            )));
            assert_permission_advice(&message, &format!("(os error {errno})"));
        }
    }

    #[test]
    fn parse_data_bits_default_and_valid() {
        assert_eq!(parse_data_bits("5"), DataBits::Five);
        assert_eq!(parse_data_bits("6"), DataBits::Six);
        assert_eq!(parse_data_bits("7"), DataBits::Seven);
        assert_eq!(parse_data_bits("8"), DataBits::Eight);
        assert_eq!(parse_data_bits("9"), DataBits::Eight); // 无效→默认 Eight
        assert_eq!(parse_data_bits(""), DataBits::Eight);
    }

    #[test]
    fn parse_stop_bits_default_and_valid() {
        assert_eq!(parse_stop_bits("1"), StopBits::One);
        assert_eq!(parse_stop_bits("2"), StopBits::Two);
        assert_eq!(parse_stop_bits("3"), StopBits::One); // 无效→默认 One
    }

    #[test]
    fn parse_parity_default_and_valid() {
        assert_eq!(parse_parity("none"), Parity::None);
        assert_eq!(parse_parity("odd"), Parity::Odd);
        assert_eq!(parse_parity("even"), Parity::Even);
        assert_eq!(parse_parity("mark"), Parity::None); // 无效→默认 None
    }

    // ── MockSerialPort + worker loop 测试 ──

    struct MockSerialPort {
        read_data: StdMutex<Vec<Vec<u8>>>,
        written: StdMutex<Vec<u8>>,
        dtr: StdMutex<bool>,
        rts: StdMutex<bool>,
        operations: Arc<StdMutex<Vec<String>>>,
    }

    impl MockSerialPort {
        fn new() -> Self {
            Self {
                read_data: StdMutex::new(Vec::new()),
                written: StdMutex::new(Vec::new()),
                dtr: StdMutex::new(false),
                rts: StdMutex::new(false),
                operations: Arc::new(StdMutex::new(Vec::new())),
            }
        }

        fn push_read(&self, data: Vec<u8>) {
            self.read_data.lock().unwrap().push(data);
        }
    }

    impl SerialIo for MockSerialPort {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut data = self.read_data.lock().unwrap();
            if data.is_empty() {
                Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout"))
            } else {
                let bytes = data.remove(0);
                let len = bytes.len().min(buf.len());
                buf[..len].copy_from_slice(&bytes[..len]);
                Ok(len)
            }
        }

        fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
            self.written.lock().unwrap().extend_from_slice(buf);
            self.operations
                .lock()
                .unwrap()
                .push(format!("write:{}", String::from_utf8_lossy(buf)));
            Ok(())
        }

        fn write_data_terminal_ready(&mut self, value: bool) -> std::io::Result<()> {
            *self.dtr.lock().unwrap() = value;
            self.operations.lock().unwrap().push(format!("dtr:{value}"));
            Ok(())
        }

        fn write_request_to_send(&mut self, value: bool) -> std::io::Result<()> {
            *self.rts.lock().unwrap() = value;
            self.operations.lock().unwrap().push(format!("rts:{value}"));
            Ok(())
        }
    }

    /// `MockSerialPort` 的 panic 版本：`read` 一被调用就把 worker 掀掉。
    struct PanickingPort;

    impl SerialIo for PanickingPort {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            panic!("模拟串口读路径 panic（预期：跳过所有显式 alive.store）");
        }
        fn write_all(&mut self, _buf: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        fn write_data_terminal_ready(&mut self, _value: bool) -> std::io::Result<()> {
            Ok(())
        }
        fn write_request_to_send(&mut self, _value: bool) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// `AliveGuard` 本身：**不需要任何端口**。
    ///
    /// 这条测的是 guard 这个类型的语义 —— 线程 panic 展开时 `Drop` 仍然执行。
    /// 至于"Windows 生产入口 `NativeWorker::run` 身上到底挂没挂这个 guard"，那是
    /// 另一条事实，由 `windows_native.rs` 里的 `native_worker_run_...` 测试覆盖
    /// （无需端口、无需硬件：`stop` 预先置位即可让 `run_impl` 第一句返回）。
    /// 两者合起来才是 Windows 侧的完整证据：类型对 + 站点对。
    #[test]
    fn alive_guard_clears_flag_when_thread_panics() {
        let alive = Arc::new(AtomicBool::new(true));
        let guard_alive = Arc::clone(&alive);
        let join = std::thread::spawn(move || {
            let _guard = AliveGuard(guard_alive);
            panic!("模拟 worker 线程 panic 展开");
        });
        assert!(
            join.join().is_err(),
            "mock 线程必须 panic，否则本测试什么都没测到"
        );
        // join() 返回即意味着展开已完成 ⇒ guard 已经 drop，无需任何等待/超时。
        assert!(
            !alive.load(Ordering::Acquire),
            "worker panic 后 alive 必须为 false，否则端口成为不可恢复的僵尸"
        );
    }

    #[test]
    fn alive_guard_clears_flag_on_normal_exit() {
        // panic 之外的一侧：guard 取代显式 store 后，正常返回同样要清零。
        let alive = Arc::new(AtomicBool::new(true));
        let guard_alive = Arc::clone(&alive);
        let join = std::thread::spawn(move || {
            let _guard = AliveGuard(guard_alive);
        });
        join.join().unwrap();
        assert!(
            !alive.load(Ordering::Acquire),
            "worker 正常退出后 alive 必须为 false"
        );
    }

    /// 现场复现 C4：worker panic 时 `serial_worker_loop_impl` 内所有显式
    /// `alive.store(false)` 都被展开跳过，端口既不被回收也不能重开。
    ///
    /// 注意适用范围：`serial_worker_loop_impl` 是 `#[cfg(any(not(windows), test))]`
    /// 的共享实现，Windows 生产路径走 `NativeWorker::run`；本测试证明的是"共享实现 +
    /// guard"协同生效。Windows 侧另有两条证据：`alive_guard_*`（guard 类型本身）与
    /// `windows_native.rs` 里直接调用 `NativeWorker::run` 的那条（站点本身）。
    #[test]
    fn panicked_worker_clears_alive_flag() {
        let alive = Arc::new(AtomicBool::new(true));
        let thread_alive = Arc::clone(&alive);
        let (writer, command_rx) = bounded::<SerialCommand>(4);
        let stop = Arc::new(AtomicBool::new(false));
        let join = std::thread::spawn(move || {
            serial_worker_loop_impl(
                PanickingPort,
                command_rx,
                stop,
                thread_alive,
                DataBus::new(),
                "serial:PANIC_TEST".to_owned(),
                None,
            )
        });
        assert!(
            join.join().is_err(),
            "mock 端口必须让 worker panic，否则本测试什么都没测到"
        );
        assert!(
            !alive.load(Ordering::Acquire),
            "worker panic 后 alive 必须为 false，否则端口成为不可恢复的僵尸"
        );
        drop(writer);
    }

    #[test]
    fn worker_loop_publishes_rx_and_tx() {
        let bus = DataBus::new();
        let rx_sub = bus.subscribe_lossless(TopicFilter::exact(tool_core::topics::SERIAL_RX));
        let tx_sub = bus.subscribe_lossless(TopicFilter::exact(tool_core::topics::SERIAL_TX));

        let mock = MockSerialPort::new();
        mock.push_read(b"hello".to_vec());

        let (command_tx, command_rx) = bounded::<SerialCommand>(16);
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));

        // 在另一个线程中运行 worker loop
        let thread_stop = stop.clone();
        let thread_alive = alive.clone();
        let thread_bus = bus.clone();
        let handle = std::thread::spawn(move || {
            serial_worker_loop_impl(
                mock,
                command_rx,
                thread_stop,
                thread_alive,
                thread_bus,
                "serial:COM1".to_owned(),
                None,
            );
        });

        // 等待 RX 事件
        let rx_event = rx_sub.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(rx_event.topic, tool_core::topics::SERIAL_RX);
        assert_eq!(rx_event.payload.text_lossy(), "hello");

        // 发送数据
        command_tx
            .send(SerialCommand::Write {
                bytes: b"world".to_vec(),
                completion: None,
            })
            .unwrap();
        let tx_event = tx_sub.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(tx_event.topic, tool_core::topics::SERIAL_TX);
        assert_eq!(tx_event.payload.text_lossy(), "world");

        // 停止 worker
        stop.store(true, Ordering::Relaxed);
        let _ = handle.join();
    }

    #[test]
    fn worker_loop_sets_dtr_rts() {
        let bus = DataBus::new();
        let mock = MockSerialPort::new();

        let (command_tx, command_rx) = bounded::<SerialCommand>(16);
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));

        // 发送 DTR 命令
        command_tx
            .send(SerialCommand::SetDtr {
                value: true,
                completion: None,
            })
            .unwrap();
        command_tx
            .send(SerialCommand::SetRts {
                value: true,
                completion: None,
            })
            .unwrap();

        let thread_stop = stop.clone();
        let thread_alive = alive.clone();
        let thread_bus = bus.clone();
        let handle = std::thread::spawn(move || {
            serial_worker_loop_impl(
                mock,
                command_rx,
                thread_stop,
                thread_alive,
                thread_bus,
                "serial:COM1".to_owned(),
                None,
            );
        });

        std::thread::sleep(Duration::from_millis(50));
        stop.store(true, Ordering::Relaxed);
        let _ = handle.join();

        // DTR/RTS 应在 worker loop 中被处理
        // 注意：mock 在 worker loop 中被 move 了，无法直接检查
        // 此测试主要验证 DTR/RTS 命令不会导致 panic
    }

    #[test]
    fn worker_loop_preserves_write_and_signal_fifo() {
        let bus = DataBus::new();
        let mock = MockSerialPort::new();
        let operations = Arc::clone(&mock.operations);
        let (command_tx, command_rx) = bounded::<SerialCommand>(8);
        let (a_tx, a_rx) = bounded(1);
        let (dtr_tx, dtr_rx) = bounded(1);
        let (b_tx, b_rx) = bounded(1);
        command_tx
            .send(SerialCommand::Write {
                bytes: b"A".to_vec(),
                completion: Some(a_tx),
            })
            .unwrap();
        command_tx
            .send(SerialCommand::SetDtr {
                value: false,
                completion: Some(dtr_tx),
            })
            .unwrap();
        command_tx
            .send(SerialCommand::Write {
                bytes: b"B".to_vec(),
                completion: Some(b_tx),
            })
            .unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let thread_stop = Arc::clone(&stop);
        let thread_alive = Arc::clone(&alive);
        let handle = std::thread::spawn(move || {
            serial_worker_loop_impl(
                mock,
                command_rx,
                thread_stop,
                thread_alive,
                bus,
                "serial:COM1".to_owned(),
                None,
            );
        });

        expect_completed(
            a_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "write:A",
        );
        expect_completed(
            dtr_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "dtr:false",
        );
        expect_completed(
            b_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "write:B",
        );
        stop.store(true, Ordering::Release);
        handle.join().unwrap();

        assert_eq!(
            *operations.lock().unwrap(),
            vec!["write:A", "dtr:false", "write:B"]
        );
    }

    /// 完成通知的"成功"侧断言。载荷换成 `io::Result<()>` 之后 `assert_eq!` 用不上了
    /// （`io::Error` 没有 `PartialEq`），失败时把 kind 与文案都打出来，一眼看出是哪条命令。
    fn expect_completed(received: std::io::Result<()>, what: &str) {
        if let Err(error) = received {
            panic!(
                "{what} 的完成通知应为 Ok，实际 kind={:?} 文案={error}",
                error.kind()
            );
        }
    }

    // ── 第 42 轮：worker 侧的 `io::Error` 走完成通道回来时，kind 必须还在 ──
    //
    // portable 路径上 `complete_command` 只有一个调用者 `execute_serial_command`
    // （Windows 的对应物是 `windows_native::drain_commands`，它另有用例），等待方是
    // `wait_for_completion` —— 两个都是本 crate 的私有项，所以这几条用例把
    // 「假端口造错 → 完成通道 → 等待方 → `translate_error`」整段跑完，
    // 不需要线程，用例里也没有任何等待或调度假设。
    // 改前的共同症状：通道发的是 `ToString` 文案，等待方再包成 `ErrorKind::Other`，
    // 于是第 41 轮那几臂按 kind 的分派对走过通道的失败全部不可达。

    /// 每条 I/O 都返回同一条错误的假端口。`io::Error` 不是 `Clone`，故存 kind + 文案，
    /// 每次调用现造一条同形状的 —— 等价于生产里 OS 每次都吐同一条错误。
    struct AlwaysFailingPort {
        kind: std::io::ErrorKind,
        detail: &'static str,
    }

    impl AlwaysFailingPort {
        fn error(&self) -> std::io::Error {
            std::io::Error::new(self.kind, self.detail)
        }
    }

    impl SerialIo for AlwaysFailingPort {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(self.error())
        }
        fn write_all(&mut self, _buf: &[u8]) -> std::io::Result<()> {
            Err(self.error())
        }
        fn write_data_terminal_ready(&mut self, _value: bool) -> std::io::Result<()> {
            Err(self.error())
        }
        fn write_request_to_send(&mut self, _value: bool) -> std::io::Result<()> {
            Err(self.error())
        }
    }

    /// 让 worker 执行一条必然失败的**写入**命令，站在等待方的位置取回错误 ——
    /// 与 `send_to_blocking` 走同一对生产者/消费者。
    fn write_failure_from_worker(kind: std::io::ErrorKind, detail: &'static str) -> TransportError {
        let (completion, result) = bounded(1);
        let mut port = AlwaysFailingPort { kind, detail };
        let keep_running = execute_serial_command(
            &mut port,
            SerialCommand::Write {
                bytes: b"abc".to_vec(),
                completion: Some(completion),
            },
            &DataBus::new(),
            "serial:FAIL42",
            "FAIL42",
            &|| {},
        );
        assert!(
            !keep_running,
            "装配前提：写失败的命令必须让 worker 退出，否则本用例没在测那条路径"
        );
        wait_for_completion(result, Duration::from_secs(5), "串口写入超时")
            .expect_err("worker 侧的写失败必须以错误回到等待方")
    }

    /// 同上，但命令是 **DTR** —— portable 路径上信号线那一格的生产者
    /// `finish_signal_command`（本轮改成"先记日志、后交给通道"）。信号线失败不终止 worker。
    fn signal_failure_from_worker(
        kind: std::io::ErrorKind,
        detail: &'static str,
    ) -> TransportError {
        let (completion, result) = bounded(1);
        let mut port = AlwaysFailingPort { kind, detail };
        let keep_running = execute_serial_command(
            &mut port,
            SerialCommand::SetDtr {
                value: true,
                completion: Some(completion),
            },
            &DataBus::new(),
            "serial:FAIL42",
            "FAIL42",
            &|| {},
        );
        assert!(
            keep_running,
            "装配前提：信号线失败只记日志，worker 必须继续运行"
        );
        wait_for_completion(result, Duration::from_secs(5), "串口控制信号超时")
            .expect_err("worker 侧的信号线失败必须以错误回到等待方")
    }

    /// 中文 Windows 上权限失败的 Display 原话，从端口写路径出来后仍要拿到
    /// 第 41 轮那句可执行提示，而不是通用的 `IO 错误：`。
    #[test]
    fn worker_side_permission_failure_reaches_the_advice_through_the_channel() {
        let error = write_failure_from_worker(
            std::io::ErrorKind::PermissionDenied,
            "拒绝访问。 (os error 5)",
        );
        assert!(
            matches!(&error, TransportError::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied),
            "完成通道又把 kind 抹掉了，实际: {error:?}"
        );
        assert_permission_advice(&translate_error(&error), "拒绝访问。 (os error 5)");
    }

    /// 第二种 kind 也要成立（不能只把一格调通）：`TimedOut` 必须拿到 `操作超时：`
    /// 框架。真实来源就在本 crate —— `windows_native::wait_overlapped_timeout` 的写超时
    /// 给的正是 `io::Error::new(TimedOut, "serial write timed out")`，旧通道到等待方
    /// 只剩 `IO 错误：serial write timed out`。
    #[test]
    fn worker_side_write_timeout_keeps_the_timeout_frame_through_the_channel() {
        let error =
            write_failure_from_worker(std::io::ErrorKind::TimedOut, "serial write timed out");
        assert_eq!(translate_error(&error), "操作超时：serial write timed out");
    }

    /// 第二个生产者也要成立（不是只把写臂调通）：DTR 走的是 `finish_signal_command` ——
    /// 本轮按"先记日志、后交给通道"重排过的那一处 portable 生产者。kind 选
    /// `PermissionDenied` 有真实出处：Windows 的 `EscapeCommFunction`、POSIX 的
    /// `TIOCMBIS` 都会给 EACCES。改前这一格同样只剩 `IO 错误：`。
    #[test]
    fn worker_side_signal_failure_keeps_its_kind_through_the_channel() {
        let error = signal_failure_from_worker(
            std::io::ErrorKind::PermissionDenied,
            "拒绝访问。 (os error 5)",
        );
        assert!(
            matches!(&error, TransportError::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied),
            "完成通道又把 kind 抹掉了，实际: {error:?}"
        );
        assert_permission_advice(&translate_error(&error), "拒绝访问。 (os error 5)");
    }

    /// 本轮只换载荷，没碰"发送方先消失"那一侧：worker 在回填前退出 ⇒ 发送句柄被
    /// drop ⇒ 等待方拿 `WorkerClosed`（既不是超时，也不是 `Io`）。**改前就是绿的**，
    /// 记在这里是钉住它改后仍绿。
    #[test]
    fn a_dropped_completion_sender_still_reports_worker_closed() {
        let (completion, result) = bounded(1);
        drop(completion);
        let error = wait_for_completion(result, Duration::from_secs(5), "串口写入超时")
            .expect_err("通道已断开，等待方不得当作成功");
        assert!(
            matches!(error, TransportError::WorkerClosed),
            "断连的定性变了，实际: {error:?}"
        );
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use tool_databus::TopicFilter;

    /// 构造测试用 PortHandle（无真实 worker 线程，join=None）。
    fn make_test_handle(alive: bool) -> PortHandle {
        PortHandle {
            config: SerialConfig::default(),
            connecting: Arc::new(AtomicBool::new(false)),
            writer: bounded::<SerialCommand>(1).0,
            #[cfg(windows)]
            wake: None,
            stop: Arc::new(AtomicBool::new(false)),
            alive: Arc::new(AtomicBool::new(alive)),
            join: None,
        }
    }

    #[test]
    fn resolve_open_port_name_exact_match() {
        let mut ports = HashMap::new();
        ports.insert("COM3".to_owned(), make_test_handle(true));
        assert_eq!(
            TransportManager::resolve_open_port_name_locked(&ports, "COM3"),
            Some("COM3".to_owned())
        );
    }

    #[test]
    fn resolve_open_port_name_case_insensitive() {
        let mut ports = HashMap::new();
        ports.insert("COM3".to_owned(), make_test_handle(true));
        assert_eq!(
            TransportManager::resolve_open_port_name_locked(&ports, "com3"),
            Some("COM3".to_owned())
        );
    }

    #[test]
    fn resolve_open_port_name_not_found() {
        let ports = HashMap::new();
        assert_eq!(
            TransportManager::resolve_open_port_name_locked(&ports, "COM3"),
            None
        );
    }

    #[test]
    fn transport_manager_new_has_no_open_ports() {
        let bus = DataBus::new();
        let tm = TransportManager::new(bus);
        assert!(tm.open_ports().is_empty());
    }

    // `parse_hex_rejects_empty` / `parse_hex_rejects_invalid_chars` 随函数一并迁入
    // `crates/core/src/lib.rs`（断言逐字未改）。这里改为钉住迁移后 transport 仍然负责的
    // 那一环：裸文案必须重建为 `TransportError::InvalidHex`，否则 `translate_error`
    // 的中文提示会退化成未知错误。
    #[test]
    fn send_hex_to_maps_core_parse_error() {
        let bus = DataBus::new();
        let tm = TransportManager::new(bus);
        let error = tm
            .send_hex_to("NO-SUCH-PORT", "gg")
            .expect_err("非 HEX 字符必须在投递前被拒绝");
        assert!(
            matches!(&error, TransportError::InvalidHex(message) if message == "'gg' is not hex"),
            "transport 必须把 tool_core 的裸文案包进 InvalidHex，实际: {error:?}"
        );
    }

    #[test]
    fn natural_sort_key_extracts_prefix_and_number() {
        assert_eq!(natural_sort_key("COM3"), ("COM".to_owned(), 3));
        assert_eq!(natural_sort_key("COM10"), ("COM".to_owned(), 10));
        assert_eq!(natural_sort_key("USB0"), ("USB".to_owned(), 0));
    }

    // ── #13: TransportManager 并发状态机测试 ──

    /// 自旋等待线程终止。断言对象是"线程已终止"这一事实本身，时限只是防止
    /// 测试永久卡死的兜底，**不是**通过条件（被测线程除了 panic 什么都不做）。
    fn wait_thread_finished(join: &JoinHandle<()>) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !join.is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "worker 线程应在合理时间内终止"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn port_is_dead_accepts_finished_join_without_alive_flag() {
        // 线程已终止 = 端口已死亡，与 worker 有没有记得置位无关。
        let mut handle = make_test_handle(true);
        handle.join = Some(thread::spawn(move || {
            panic!("模拟 worker panic 展开：一个 alive.store 都不会执行");
        }));
        wait_thread_finished(handle.join.as_ref().unwrap());
        assert!(
            handle.alive.load(Ordering::Acquire),
            "用例前提：没有任何代码把 alive 置为 false"
        );
        assert!(
            port_is_dead(&handle),
            "线程已退出即视为端口死亡，与 alive 标志无关"
        );
    }

    #[test]
    fn port_is_dead_keeps_healthy_handle_open() {
        // 反向：线程还在跑且 alive=true 时不得判死，否则双信号会把正常端口关掉。
        let (keep_alive_tx, keep_alive_rx) = bounded::<()>(1);
        let mut handle = make_test_handle(true);
        handle.join = Some(thread::spawn(move || {
            let _ = keep_alive_rx.recv();
        }));
        assert!(
            !port_is_dead(&handle),
            "worker 线程仍在运行时不得判定为死亡"
        );
        drop(keep_alive_tx);
        wait_thread_finished(handle.join.as_ref().unwrap());
        assert!(port_is_dead(&handle), "同一句柄在线程终止后必须立刻判死");
    }

    #[test]
    fn enqueue_command_rejects_handle_whose_worker_thread_finished() {
        // **谓词级**单测：锁定 `port_is_dead` 挂在 `enqueue_command` 这道门上，即判死
        // 只看句柄自身的两个信号、与 channel 状态无关。
        //
        // 这里"receiver 还握在测试手里"的通道状态在**生产里不可达**：worker 按值持有
        // Receiver（本 crate 无 clone / catch_unwind / forget），线程一结束 receiver 就
        // 被 drop，旧代码在 `try_send` 上同样得到 `Disconnected` → `WorkerClosed`。所以
        // 本测试不是"UI 一路显示已发"那个症状的复现，它也不声称修好了那个症状（那个洞是
        // 卡住未退出的 worker，见 `enqueue_command` 处的注释）。
        //
        // 留着它的价值有两条：(1) 它是"哪天有人从 worker 里 clone 出一个 Receiver、于是
        // 通道健康而线程已死"这一回归的绊线；(2) 它顺带覆盖了 `join: Some(finished)` 但
        // `alive` 仍为 true 的形状，正是 guard 缺席时的样子。
        let bus = DataBus::new();
        let tm = TransportManager::new(bus);
        let (writer, reader) = bounded::<SerialCommand>(4);
        let mut handle = make_test_handle(true);
        handle.writer = writer;
        handle.join = Some(thread::spawn(|| {}));
        wait_thread_finished(handle.join.as_ref().unwrap());
        tm.ports.lock().insert("COM3".to_owned(), handle);

        let err = tm.send_to("COM3", vec![0x01]).unwrap_err();
        assert!(matches!(err, TransportError::WorkerClosed), "got {err:?}");
        assert!(
            reader.try_recv().is_err(),
            "死亡句柄的命令通道不得收到任何字节"
        );
    }

    #[test]
    fn send_to_returns_port_not_open_for_unknown_port() {
        let bus = DataBus::new();
        let tm = TransportManager::new(bus);
        // 端口表为空，任意端口名应返回 PortNotOpen。
        let err = tm.send_to("COM99", vec![0x01]).unwrap_err();
        assert!(matches!(err, TransportError::PortNotOpen(_)));
    }

    #[test]
    fn send_to_returns_worker_closed_for_dead_handle() {
        let bus = DataBus::new();
        let tm = TransportManager::new(bus);
        // 手动塞入一个 dead handle（alive=false），模拟 worker 已退出但 handle 未清理。
        tm.ports
            .lock()
            .insert("COM3".to_owned(), make_test_handle(false));
        let err = tm.send_to("COM3", vec![0x01]).unwrap_err();
        assert!(matches!(err, TransportError::WorkerClosed), "got {err:?}");
    }

    #[test]
    fn send_to_returns_queue_full_when_channel_full() {
        let bus = DataBus::new();
        let tm = TransportManager::new(bus);
        // 用 capacity=1 的 writer，塞入一条后下一条应 QueueFull。
        let mut handle = make_test_handle(true);
        let (writer, _rx) = bounded::<SerialCommand>(1);
        handle.writer = writer;
        tm.ports.lock().insert("COM3".to_owned(), handle);
        // 先发一条填满 channel（capacity=1）。
        assert!(tm.send_to("COM3", vec![0x01]).is_ok());
        // 第二条应 QueueFull。
        let err = tm.send_to("COM3", vec![0x02]).unwrap_err();
        assert!(matches!(err, TransportError::QueueFull), "got {err:?}");
    }

    #[test]
    fn virtual_send_blocking_completes_after_tx_event() {
        let bus = DataBus::new();
        let tx = bus.subscribe_lossless(TopicFilter::exact(serial_topics::SERIAL_TX));
        let tm = TransportManager::new(bus);
        let virtual_port = tm.open_virtual_serial("VIRTUAL1").unwrap();

        tm.send_to_blocking(
            virtual_port.port_name(),
            b"hello".to_vec(),
            Duration::from_secs(1),
        )
        .unwrap();

        let event = tx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(event.payload.text_lossy(), "hello");
        tm.close_port_blocking(virtual_port.port_name(), Duration::from_secs(1))
            .unwrap();
    }

    #[test]
    fn virtual_close_then_immediate_reopen_waits_for_worker_exit() {
        let tm = TransportManager::new(DataBus::new());
        let first = tm.open_virtual_serial("VIRTUAL2").unwrap();
        tm.close_port(first.port_name());

        // Reopen is allowed to proceed immediately because close_port_blocking
        // waits for the old worker instead of merely setting its stop flag.
        let second = tm.open_virtual_serial("VIRTUAL2").unwrap();
        assert_eq!(second.port_name(), "VIRTUAL2");
        tm.close_port_blocking(second.port_name(), Duration::from_secs(1))
            .unwrap();
    }

    #[test]
    fn close_port_blocking_returns_ok_for_unknown_port() {
        let bus = DataBus::new();
        let tm = TransportManager::new(bus);
        // 不存在的端口：close_port_blocking 应返回 Ok（无操作），不 panic。
        let result = tm.close_port_blocking("COM99", Duration::from_millis(10));
        assert!(result.is_ok());
    }

    #[test]
    fn close_port_blocking_sets_stop_and_removes_handle() {
        let bus = DataBus::new();
        let tm = TransportManager::new(bus);
        let stop = Arc::new(AtomicBool::new(false));
        let mut handle = make_test_handle(true);
        handle.stop = Arc::clone(&stop);
        tm.ports.lock().insert("COM3".to_owned(), handle);
        // close_port_blocking 无 join（join=None）时直接返回 Ok。
        let result = tm.close_port_blocking("COM3", Duration::from_millis(10));
        assert!(result.is_ok());
        // stop 应被置 true。
        assert!(stop.load(Ordering::Acquire));
        // ports 表中应已移除。
        assert!(tm.ports.lock().get("COM3").is_none());
    }

    #[test]
    fn reap_dead_ports_removes_dead_handles() {
        let bus = DataBus::new();
        let tm = TransportManager::new(bus);
        // 塞入一个 alive、一个 dead。
        tm.ports
            .lock()
            .insert("COM3".to_owned(), make_test_handle(true));
        tm.ports
            .lock()
            .insert("COM4".to_owned(), make_test_handle(false));
        tm.reap_dead_ports();
        let ports = tm.ports.lock();
        assert!(ports.contains_key("COM3"), "alive handle 应保留");
        assert!(!ports.contains_key("COM4"), "dead handle 应被 reap 移除");
    }

    #[test]
    fn status_port_returns_closed_for_dead_handle() {
        let bus = DataBus::new();
        let tm = TransportManager::new(bus);
        tm.ports
            .lock()
            .insert("COM3".to_owned(), make_test_handle(false));
        let status = tm.status_port("COM3");
        assert!(!status.open, "dead handle 的 status 应为 closed");
    }

    #[test]
    fn status_port_returns_open_for_alive_handle() {
        let bus = DataBus::new();
        let tm = TransportManager::new(bus);
        tm.ports
            .lock()
            .insert("COM3".to_owned(), make_test_handle(true));
        let status = tm.status_port("COM3");
        assert!(status.open);
        assert_eq!(status.port_name.as_deref(), Some(""));
    }
}
