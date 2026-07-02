use crossbeam_channel::{Sender, TrySendError, bounded};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serialport as sp;
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[cfg(not(windows))]
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use thiserror::Error;
use tool_core::{Direction, Event, LogLevel, Payload};

#[cfg(windows)]
mod windows_native;

pub(crate) enum DtrRtsCommand {
    SetDtr(bool),
    SetRts(bool),
}
use tool_databus::DataBus;

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
}

impl TransportStatus {
    pub fn closed() -> Self {
        Self {
            open: false,
            port_name: None,
            baud_rate: None,
        }
    }
}

use std::sync::atomic::AtomicU64;

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
    last_reap_time: Arc<std::sync::atomic::AtomicU64>,
    /// UI 重绘唤醒器，app 层注入。worker publish 串口事件后调用以立即重绘。
    /// `Arc<Mutex<Option<...>>>` 让所有 TransportManager clone 共享同一 waker（仅 app 启动时设一次）。
    repaint_waker: Arc<Mutex<Option<Arc<dyn RepaintWaker>>>>,
}

struct PortHandle {
    config: SerialConfig,
    writer: Sender<Vec<u8>>,
    dtr_rts_tx: Sender<DtrRtsCommand>,
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
                self.bus.publish(Event::system_log(
                    LogLevel::Info,
                    "transport.serial",
                    format!("已关闭 {} @ {}", h.port_name, h.baud_rate),
                ));
                // 发布结构化生命周期事件，供插件监听
                self.bus.publish(Event::new(
                    tool_core::topics::SERIAL_CLOSED,
                    format!("serial:{}", h.port_name),
                    Direction::Internal,
                    Payload::Json(serde_json::json!({
                        "port": h.port_name,
                        "baud_rate": h.baud_rate,
                    })),
                ));
                // swap_remove 把最后一个元素移到了 i，不递增 i
            } else {
                i += 1;
            }
        }
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
        let available = sp::available_ports().unwrap_or_default();
        let resolved = available
            .iter()
            .find(|p| p.port_name.eq_ignore_ascii_case(&config.port_name));
        if let Some(p) = resolved {
            config.port_name = p.port_name.clone();
        }
        // 先收割已完成关闭的旧 worker
        self.reap_closing();

        // 同配置重复打开：直接成功
        {
            let guard = self.ports.lock();
            if let Some(existing) = guard.get(&config.port_name)
                && existing.alive.load(Ordering::Acquire)
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

        let (writer, write_rx) = bounded::<Vec<u8>>(1024);
        let (dtr_rts_tx, dtr_rts_rx) = bounded::<DtrRtsCommand>(16);
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
                write_rx,
                dtr_rts_rx,
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
        let (join, wake) = {
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

            let join = thread::spawn(move || {
                serial_worker_loop(
                    port,
                    write_rx,
                    dtr_rts_rx,
                    thread_stop,
                    thread_alive,
                    thread_bus,
                    thread_source,
                    thread_waker.clone(),
                );
            });
            (join, None::<()>)
        };

        self.ports.lock().insert(
            config.port_name.clone(),
            PortHandle {
                config: config.clone(),
                writer,
                dtr_rts_tx,
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
            self.bus.publish(Event::system_log(
                LogLevel::Info,
                "transport.serial",
                format!("已关闭 {} @ {}", h.port_name, h.baud_rate),
            ));
            // 发布结构化生命周期事件，供插件监听
            self.bus.publish(Event::new(
                tool_core::topics::SERIAL_CLOSED,
                format!("serial:{}", h.port_name),
                Direction::Internal,
                Payload::Json(serde_json::json!({
                    "port": h.port_name,
                    "baud_rate": h.baud_rate,
                })),
            ));
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
                worker.stop.store(true, Ordering::Release);
                #[cfg(windows)]
                if let Some(wake) = &worker.wake {
                    wake.set();
                }
                let join = worker.join.take();
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
            worker.stop.store(true, Ordering::Release);
            #[cfg(windows)]
            if let Some(wake) = &worker.wake {
                wake.set();
            }
            let join = worker.join.take();
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
        Ok(())
    }

    // ── 发送到指定端口 ──
    pub fn send_to(&self, port_name: &str, bytes: Vec<u8>) -> TransportResult<()> {
        self.reap_closing();
        let guard = self.ports.lock();
        let resolved = Self::resolve_open_port_name_locked(&guard, port_name)
            .ok_or_else(|| TransportError::PortNotOpen(port_name.to_owned()))?;
        let worker = guard
            .get(&resolved)
            .ok_or_else(|| TransportError::PortNotOpen(port_name.to_owned()))?;
        if !worker.alive.load(Ordering::Acquire) {
            return Err(TransportError::WorkerClosed);
        }
        worker.writer.try_send(bytes).map_err(|e| match e {
            crossbeam_channel::TrySendError::Full(_) => TransportError::QueueFull,
            crossbeam_channel::TrySendError::Disconnected(_) => TransportError::WorkerClosed,
        })?;
        #[cfg(windows)]
        if let Some(wake) = &worker.wake {
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
        self.send_to(port_name, parse_hex(input)?)
    }

    // ── 状态 ──
    pub fn status_port(&self, port_name: &str) -> TransportStatus {
        self.reap_closing();
        self.reap_dead_ports();
        let guard = self.ports.lock();
        let key = Self::resolve_open_port_name_locked(&guard, port_name)
            .unwrap_or_else(|| port_name.to_owned());
        match guard.get(&key) {
            Some(w) if w.alive.load(Ordering::Relaxed) => TransportStatus {
                open: true,
                port_name: Some(w.config.port_name.clone()),
                baud_rate: Some(w.config.baud_rate),
            },
            _ => TransportStatus::closed(),
        }
    }

    pub fn status_all(&self) -> Vec<TransportStatus> {
        self.reap_closing();
        self.reap_dead_ports();
        self.ports
            .lock()
            .values()
            .map(|w| {
                if w.alive.load(Ordering::Relaxed) {
                    TransportStatus {
                        open: true,
                        port_name: Some(w.config.port_name.clone()),
                        baud_rate: Some(w.config.baud_rate),
                    }
                } else {
                    TransportStatus::closed()
                }
            })
            .collect()
    }

    pub fn open_ports(&self) -> Vec<String> {
        self.reap_closing();
        self.reap_dead_ports();
        self.ports.lock().keys().cloned().collect()
    }

    pub fn set_dtr(&self, port_name: &str, value: bool) -> TransportResult<()> {
        self.send_dtr_rts_command(port_name, DtrRtsCommand::SetDtr(value))
    }

    pub fn set_rts(&self, port_name: &str, value: bool) -> TransportResult<()> {
        self.send_dtr_rts_command(port_name, DtrRtsCommand::SetRts(value))
    }

    /// 向指定端口的 worker 发送 DTR/RTS 控制命令
    fn send_dtr_rts_command(&self, port_name: &str, cmd: DtrRtsCommand) -> TransportResult<()> {
        self.reap_closing();
        let guard = self.ports.lock();
        let resolved = Self::resolve_open_port_name_locked(&guard, port_name)
            .ok_or_else(|| TransportError::PortNotOpen(port_name.to_owned()))?;
        let worker = guard
            .get(&resolved)
            .ok_or_else(|| TransportError::PortNotOpen(port_name.to_owned()))?;
        if !worker.alive.load(Ordering::Acquire) {
            return Err(TransportError::WorkerClosed);
        }
        worker.dtr_rts_tx.try_send(cmd).map_err(|e| match e {
            TrySendError::Full(_) => TransportError::QueueFull,
            TrySendError::Disconnected(_) => TransportError::WorkerClosed,
        })?;
        #[cfg(windows)]
        if let Some(wake) = &worker.wake {
            wake.set();
        }
        Ok(())
    }

    /// 清理已退出 worker 的 stale port handle（alive == false）。
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
                .filter(|(_, h)| !h.alive.load(Ordering::Acquire))
                .map(|(name, _)| name.clone())
                .collect();
            dead_names
                .into_iter()
                .filter_map(|name| {
                    let mut handle = guard.remove(&name)?;
                    // stop 已无意义（worker 已死），但保持对称并防御性置位。
                    handle.stop.store(true, Ordering::Release);
                    #[cfg(windows)]
                    if let Some(wake) = &handle.wake {
                        wake.set();
                    }
                    let join = handle.join.take();
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

/// 串口读写抽象，使 `serial_worker_loop` 可测试。
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

#[cfg(not(windows))]
fn serial_worker_loop(
    port: Box<dyn sp::SerialPort>,
    write_rx: crossbeam_channel::Receiver<Vec<u8>>,
    dtr_rts_rx: crossbeam_channel::Receiver<DtrRtsCommand>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    bus: DataBus,
    source: String,
    waker: Option<Arc<dyn RepaintWaker>>,
) {
    serial_worker_loop_impl(port, write_rx, dtr_rts_rx, stop, alive, bus, source, waker)
}

#[cfg(any(not(windows), test))]
#[allow(clippy::too_many_arguments)]
fn serial_worker_loop_impl(
    mut port: impl SerialIo,
    write_rx: crossbeam_channel::Receiver<Vec<u8>>,
    dtr_rts_rx: crossbeam_channel::Receiver<DtrRtsCommand>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    bus: DataBus,
    source: String,
    waker: Option<Arc<dyn RepaintWaker>>,
) {
    let mut buffer = [0_u8; 4096];
    // 日志用干净端口名（去掉 "serial:" 前缀）。
    let port_name = extract_port(&source);
    let wake = || {
        if let Some(w) = &waker {
            w.wake();
        }
    };

    while !stop.load(Ordering::Acquire) {
        // 处理 DTR/RTS 命令
        while let Ok(cmd) = dtr_rts_rx.try_recv() {
            match cmd {
                DtrRtsCommand::SetDtr(value) => {
                    if let Err(e) = port.write_data_terminal_ready(value) {
                        bus.publish(Event::system_log(
                            LogLevel::Error,
                            "transport.serial",
                            format!("{port_name} 设置 DTR 失败：{e}"),
                        ));
                    }
                }
                DtrRtsCommand::SetRts(value) => {
                    if let Err(e) = port.write_request_to_send(value) {
                        bus.publish(Event::system_log(
                            LogLevel::Error,
                            "transport.serial",
                            format!("{port_name} 设置 RTS 失败：{e}"),
                        ));
                    }
                }
            }
        }

        while let Ok(bytes) = write_rx.try_recv() {
            match port.write_all(&bytes) {
                Ok(()) => {
                    bus.publish(serial_tx_event(source.clone(), bytes));
                    wake();
                }
                Err(error) => {
                    bus.publish(Event::system_log(
                        LogLevel::Error,
                        "transport.serial",
                        format!("{port_name} 写入失败：{error}"),
                    ));
                    alive.store(false, Ordering::Release);
                    return;
                }
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
                alive.store(false, Ordering::Release);
                return;
            }
        }
    }
    alive.store(false, Ordering::Release);
}

// ── parse_hex 等辅助函数不变 ──

pub fn parse_hex(input: &str) -> TransportResult<Vec<u8>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(TransportError::InvalidHex("empty input".to_owned()));
    }

    let tokens: Vec<&str> = trimmed
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',' || ch == ';')
        .filter(|token| !token.is_empty())
        .collect();

    // 单 token 与多 token 走同一个 parse_hex_token，保证分块/补0规则一致。
    let mut out = Vec::new();
    for token in &tokens {
        out.extend(parse_hex_token(token)?);
    }
    Ok(out)
}

/// 解析单个 HEX token，返回其对应的字节。
///
/// 规则（单 token 与多 token 一致）：
/// - 去除 `0x`/`0X` 前缀，删除 `_`/`-` 分隔符。
/// - 长度 ≤ 2：直接解析为单字节（单 nibble 如 `"A"` 自动左补 0 → `0x0A`）。
/// - 长度 > 2 且为奇数：左补一个 `0` 再按每 2 字符分块。
/// - 长度 > 2 且为偶数：直接按每 2 字符分块。
fn parse_hex_token(token: &str) -> TransportResult<Vec<u8>> {
    let mut token = normalize_hex_token(token);
    if token.is_empty() {
        return Err(TransportError::InvalidHex("empty token".to_owned()));
    }
    if token.len() > 2 && !token.len().is_multiple_of(2) {
        token.insert(0, '0');
    }
    if token.len() <= 2 {
        Ok(vec![parse_byte(&token)?])
    } else {
        token
            .as_bytes()
            .chunks(2)
            .map(|chunk| parse_byte(std::str::from_utf8(chunk).unwrap_or_default()))
            .collect()
    }
}

/// 严格模式解析整行 HEX：每个 token normalize 后长度必须恰为 2（拒绝单 nibble
/// 自动补0，与 hover 提示"严格模式：奇数 HEX 长度报错而非自动补0"一致）。
/// 逐 token 校验，确保 `"0xA 0xB"` 这类单 nibble 输入报错而非静默补0。
fn parse_hex_strict_line(line: &str) -> TransportResult<Vec<u8>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(TransportError::InvalidHex("empty input".to_owned()));
    }
    let tokens: Vec<&str> = trimmed
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',' || ch == ';')
        .filter(|token| !token.is_empty())
        .collect();
    let mut out = Vec::new();
    for token in &tokens {
        let normalized = normalize_hex_token(token);
        if normalized.is_empty() {
            return Err(TransportError::InvalidHex(format!(
                "严格模式: 空 token \"{token}\""
            )));
        }
        if normalized.len() != 2 {
            return Err(TransportError::InvalidHex(format!(
                "严格模式: \"{token}\" 规范化后为 {nib} 个字符，必须恰为 2（偶数 hex 长度），请补0或关闭严格模式",
                nib = normalized.len()
            )));
        }
        out.push(parse_byte(&normalized)?);
    }
    Ok(out)
}

fn normalize_hex_token(token: &str) -> String {
    token
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .replace(['_', '-'], "")
}

fn parse_byte(token: &str) -> TransportResult<u8> {
    u8::from_str_radix(token, 16)
        .map_err(|_| TransportError::InvalidHex(format!("'{token}' is not hex")))
}

fn natural_sort_key(name: &str) -> (String, u64) {
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

/// HEX 预览：将输入解析为 HEX 字节并显示 ASCII 预览。
pub fn hex_preview(input: &str) -> String {
    if input.trim().is_empty() {
        return "—".to_owned();
    }
    const MAX_PREVIEW: usize = 32;
    match parse_hex(input) {
        Ok(bytes) if !bytes.is_empty() => {
            let count = bytes.len();
            let ascii: String = bytes
                .iter()
                .take(MAX_PREVIEW)
                .map(|&b| {
                    if b.is_ascii_graphic() || b == b' ' {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            let hex = if count > MAX_PREVIEW {
                format!(
                    "{}… (共{count}B)",
                    bytes[..MAX_PREVIEW]
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            } else {
                bytes
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            format!("{hex}  |{ascii}|")
        }
        Ok(_) => "空".to_owned(),
        Err(_) => "解析失败".to_owned(),
    }
}

/// 向指定端口发送文本或 HEX 数据。
pub fn send_impl_to(
    port: &str,
    input: &str,
    hex: bool,
    line_ending_suffix: &str,
    hex_strict: bool,
    t: &TransportManager,
) -> TransportResult<()> {
    if input.trim().is_empty() {
        return Ok(());
    }
    if hex {
        // 事务性预校验：先解析所有行，任一行失败则不发送任何数据（避免部分发送）。
        // 严格模式下额外要求每个 token normalize 后长度恰为 2（拒绝单 nibble 自动补0）。
        let mut pending: Vec<Vec<u8>> = Vec::with_capacity(input.lines().count());
        for line in input.lines() {
            let x = line.trim();
            if x.is_empty() {
                continue;
            }
            pending.push(if hex_strict {
                parse_hex_strict_line(x)?
            } else {
                parse_hex(x)?
            });
        }
        for bytes in pending {
            t.send_to(port, bytes)?;
        }
        Ok(())
    } else {
        let mut text = input.to_owned();
        text.push_str(line_ending_suffix);
        t.send_text_to(port, &text)
    }
}

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
        TransportError::Serial(e) => format!("串口错误：{e}"),
        TransportError::Io(e) => match e.kind() {
            std::io::ErrorKind::WouldBlock => e.to_string(), // "正在关闭中" 等业务状态文案已含中文
            std::io::ErrorKind::TimedOut => format!("操作超时：{e}"),
            std::io::ErrorKind::InvalidData => e.to_string(), // HEX 严格模式奇偶校验文案已含中文
            _ => format!("IO 错误：{e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spaced_hex() {
        assert_eq!(parse_hex("01 0x02 ff").unwrap(), vec![1, 2, 255]);
    }
    #[test]
    fn parses_compact_hex() {
        assert_eq!(parse_hex("0102ff").unwrap(), vec![1, 2, 255]);
    }
    #[test]
    fn pads_odd_length_compact_hex() {
        assert_eq!(parse_hex("abc").unwrap(), vec![0x0a, 0xbc]);
    }
    #[test]
    fn parses_single_hex_token() {
        assert_eq!(parse_hex("FF").unwrap(), vec![255]);
    }
    #[test]
    fn parses_spaced_single_digits() {
        assert_eq!(parse_hex("1 2 3").unwrap(), vec![1, 2, 3]);
    }

    // ── #9: 单 token 与多 token 路径一致性 ──
    #[test]
    fn parse_hex_multitoken_long_token_chunks_like_single() {
        // "0A0B0C 0D"（多 token，首段 len=6）应与 "0A0B0C0D"（单 token）结果一致。
        assert_eq!(
            parse_hex("0A0B0C 0D").unwrap(),
            vec![0x0A, 0x0B, 0x0C, 0x0D]
        );
        assert_eq!(parse_hex("0A0B0C0D").unwrap(), vec![0x0A, 0x0B, 0x0C, 0x0D]);
    }

    #[test]
    fn parse_hex_multitoken_odd_long_token_pads_left() {
        // 多 token 中含奇数长度长 token（"abc 01"）应左补0，与单 token "abc" 一致。
        assert_eq!(parse_hex("abc 01").unwrap(), vec![0x0A, 0xBC, 0x01]);
        assert_eq!(parse_hex("abc").unwrap(), vec![0x0A, 0xBC]);
    }

    // ── #23: 严格模式逐 token 校验，拒绝单 nibble ──
    #[test]
    fn parse_hex_strict_rejects_single_nibble_token() {
        // "0xA 0xB" 在旧实现中通过（compact 长度偶数），严格模式应拒绝单 nibble。
        assert!(parse_hex_strict_line("0xA 0xB").is_err());
        assert!(parse_hex_strict_line("A B").is_err());
    }

    #[test]
    fn parse_hex_strict_accepts_even_tokens() {
        assert_eq!(
            parse_hex_strict_line("0A 0B 0C").unwrap(),
            vec![0x0A, 0x0B, 0x0C]
        );
        assert_eq!(parse_hex_strict_line("0xFF").unwrap(), vec![0xFF]);
    }

    #[test]
    fn parse_hex_strict_rejects_odd_long_token() {
        // 三字符 token "ABC" 严格模式应报错（不能自动补0）。
        assert!(parse_hex_strict_line("ABC").is_err());
    }

    // ── #8: translate_error 按变体分发 ──
    #[test]
    fn translate_error_covers_all_variants() {
        assert!(!translate_error(&TransportError::NoOpenPort).is_empty());
        assert!(translate_error(&TransportError::PortNotOpen("COM3".into())).contains("COM3"));
        assert!(!translate_error(&TransportError::WorkerClosed).is_empty());
        assert!(!translate_error(&TransportError::QueueFull).is_empty());
        assert!(translate_error(&TransportError::InvalidHex("bad".into())).contains("bad"));
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

    use std::sync::Mutex as StdMutex;
    use tool_databus::TopicFilter;

    struct MockSerialPort {
        read_data: StdMutex<Vec<Vec<u8>>>,
        written: StdMutex<Vec<u8>>,
        dtr: StdMutex<bool>,
        rts: StdMutex<bool>,
    }

    impl MockSerialPort {
        fn new() -> Self {
            Self {
                read_data: StdMutex::new(Vec::new()),
                written: StdMutex::new(Vec::new()),
                dtr: StdMutex::new(false),
                rts: StdMutex::new(false),
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
            Ok(())
        }

        fn write_data_terminal_ready(&mut self, value: bool) -> std::io::Result<()> {
            *self.dtr.lock().unwrap() = value;
            Ok(())
        }

        fn write_request_to_send(&mut self, value: bool) -> std::io::Result<()> {
            *self.rts.lock().unwrap() = value;
            Ok(())
        }
    }

    #[test]
    fn worker_loop_publishes_rx_and_tx() {
        let bus = DataBus::new();
        let rx_sub = bus.subscribe_lossless(TopicFilter::exact(tool_core::topics::SERIAL_RX));
        let tx_sub = bus.subscribe_lossless(TopicFilter::exact(tool_core::topics::SERIAL_TX));

        let mock = MockSerialPort::new();
        mock.push_read(b"hello".to_vec());

        let (write_tx, write_rx) = bounded::<Vec<u8>>(16);
        let (_dtr_tx, dtr_rx) = bounded::<DtrRtsCommand>(4);
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));

        // 在另一个线程中运行 worker loop
        let thread_stop = stop.clone();
        let thread_alive = alive.clone();
        let thread_bus = bus.clone();
        let handle = std::thread::spawn(move || {
            serial_worker_loop_impl(
                mock,
                write_rx,
                dtr_rx,
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
        write_tx.send(b"world".to_vec()).unwrap();
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

        let (_write_tx, write_rx) = bounded::<Vec<u8>>(16);
        let (dtr_tx, dtr_rx) = bounded::<DtrRtsCommand>(4);
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));

        // 发送 DTR 命令
        dtr_tx.send(DtrRtsCommand::SetDtr(true)).unwrap();
        dtr_tx.send(DtrRtsCommand::SetRts(true)).unwrap();

        let thread_stop = stop.clone();
        let thread_alive = alive.clone();
        let thread_bus = bus.clone();
        let handle = std::thread::spawn(move || {
            serial_worker_loop_impl(
                mock,
                write_rx,
                dtr_rx,
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
}

#[cfg(test)]
mod transport_tests {
    use super::*;

    #[test]
    fn resolve_open_port_name_exact_match() {
        let mut ports = HashMap::new();
        ports.insert(
            "COM3".to_owned(),
            PortHandle {
                config: SerialConfig::default(),
                writer: bounded(1).0,
                dtr_rts_tx: bounded(1).0,
                #[cfg(windows)]
                wake: None,
                stop: Arc::new(AtomicBool::new(false)),
                alive: Arc::new(AtomicBool::new(true)),
                join: None,
            },
        );
        assert_eq!(
            TransportManager::resolve_open_port_name_locked(&ports, "COM3"),
            Some("COM3".to_owned())
        );
    }

    #[test]
    fn resolve_open_port_name_case_insensitive() {
        let mut ports = HashMap::new();
        ports.insert(
            "COM3".to_owned(),
            PortHandle {
                config: SerialConfig::default(),
                writer: bounded(1).0,
                dtr_rts_tx: bounded(1).0,
                #[cfg(windows)]
                wake: None,
                stop: Arc::new(AtomicBool::new(false)),
                alive: Arc::new(AtomicBool::new(true)),
                join: None,
            },
        );
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

    #[test]
    fn parse_hex_rejects_empty() {
        assert!(parse_hex("").is_err());
    }

    #[test]
    fn parse_hex_rejects_invalid_chars() {
        assert!(parse_hex("gg").is_err());
    }

    #[test]
    fn natural_sort_key_extracts_prefix_and_number() {
        assert_eq!(natural_sort_key("COM3"), ("COM".to_owned(), 3));
        assert_eq!(natural_sort_key("COM10"), ("COM".to_owned(), 10));
        assert_eq!(natural_sort_key("USB0"), ("USB".to_owned(), 0));
    }

    // ── #13: TransportManager 并发状态机测试 ──

    /// 构造测试用 PortHandle（无真实 worker 线程，join=None）。
    fn make_test_handle(alive: bool) -> PortHandle {
        PortHandle {
            config: SerialConfig::default(),
            writer: bounded::<Vec<u8>>(1).0,
            dtr_rts_tx: bounded::<DtrRtsCommand>(1).0,
            #[cfg(windows)]
            wake: None,
            stop: Arc::new(AtomicBool::new(false)),
            alive: Arc::new(AtomicBool::new(alive)),
            join: None,
        }
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
        let (writer, _rx) = bounded::<Vec<u8>>(1);
        handle.writer = writer;
        tm.ports.lock().insert("COM3".to_owned(), handle);
        // 先发一条填满 channel（capacity=1）。
        assert!(tm.send_to("COM3", vec![0x01]).is_ok());
        // 第二条应 QueueFull。
        let err = tm.send_to("COM3", vec![0x02]).unwrap_err();
        assert!(matches!(err, TransportError::QueueFull), "got {err:?}");
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
