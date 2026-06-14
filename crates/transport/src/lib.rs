use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serialport as sp;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thiserror::Error;
use tool_core::{Event, LogLevel};

enum DtrRtsCommand {
    SetDtr(bool),
    SetRts(bool),
}
use tool_databus::DataBus;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SerialConfig {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: DataBits,
    pub stop_bits: StopBits,
    pub parity: Parity,
    pub timeout_ms: u64,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            port_name: String::new(),
            baud_rate: 115_200,
            data_bits: DataBits::Eight,
            stop_bits: StopBits::One,
            parity: Parity::None,
            timeout_ms: 50,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SerialPortDescriptor {
    pub port_name: String,
    pub port_type: String,
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

#[derive(Clone)]
pub struct TransportManager {
    bus: DataBus,
    ports: Arc<Mutex<HashMap<String, PortHandle>>>,
    closing: Arc<Mutex<Vec<ClosingHandle>>>,
}

struct PortHandle {
    config: SerialConfig,
    writer: Sender<Vec<u8>>,
    dtr_rts_tx: Sender<DtrRtsCommand>,
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
        }
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
    fn reap_closing(&self) {
        let mut closing = self.closing.lock();
        let mut i = 0;
        while i < closing.len() {
            if closing[i].join.is_finished() {
                let h = closing.swap_remove(i);
                let _ = h.join.join();
                self.bus.publish(Event::system_log(
                    LogLevel::Info,
                    "transport.serial",
                    format!("closed {} @ {}", h.port_name, h.baud_rate),
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
                port_type: describe_port_type(info.port_type),
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
            if let Some(existing) = guard.get(&config.port_name) {
                if existing.alive.load(Ordering::Relaxed) && existing.config == config {
                    return Ok(());
                }
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
        self.close_port_blocking(
            &config.port_name,
            Duration::from_millis(config.timeout_ms + 100),
        )?;

        let builder = sp::new(&config.port_name, config.baud_rate)
            .data_bits(config.data_bits.into())
            .stop_bits(config.stop_bits.into())
            .parity(config.parity.into())
            .timeout(Duration::from_millis(config.timeout_ms));

        let port = builder.open().map_err(|error| {
            self.bus.publish(Event::system_log(
                LogLevel::Error,
                "transport.serial",
                format!("failed to open {}: {error}", config.port_name),
            ));
            TransportError::from(error)
        })?;

        let (writer, write_rx) = bounded::<Vec<u8>>(1024);
        let (dtr_rts_tx, dtr_rts_rx) = bounded::<DtrRtsCommand>(16);
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let thread_stop = Arc::clone(&stop);
        let thread_alive = Arc::clone(&alive);
        let thread_bus = self.bus.clone();
        let source = format!("serial:{}", config.port_name);
        let thread_source = source.clone();

        let join = thread::spawn(move || {
            serial_worker_loop(
                port,
                write_rx,
                dtr_rts_rx,
                thread_stop,
                thread_alive,
                thread_bus,
                thread_source,
            );
        });

        self.ports.lock().insert(
            config.port_name.clone(),
            PortHandle {
                config: config.clone(),
                writer,
                dtr_rts_tx,
                stop,
                alive,
                join: Some(join),
            },
        );

        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "transport.serial",
            format!("opened {}", source),
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
                format!("closed {} @ {}", h.port_name, h.baud_rate),
            ));
        }
    }

    // ── 关闭指定端口（异步：设 stop 并移入 closing，不 join）──
    pub fn close_port(&self, port_name: &str) {
        let mut guard = self.ports.lock();
        let key = Self::resolve_open_port_name_locked(&guard, port_name)
            .unwrap_or_else(|| port_name.to_owned());
        if let Some(mut worker) = guard.remove(&key) {
            worker.stop.store(true, Ordering::Relaxed);
            if let Some(join) = worker.join.take() {
                self.closing.lock().push(ClosingHandle {
                    port_name: worker.config.port_name.clone(),
                    baud_rate: worker.config.baud_rate,
                    join,
                });
            }
            self.bus.publish(Event::system_log(
                LogLevel::Info,
                "transport.serial",
                format!(
                    "closing {} @ {}",
                    worker.config.port_name, worker.config.baud_rate
                ),
            ));
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
            worker.stop.store(true, Ordering::Relaxed);
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
        if !worker.alive.load(Ordering::Relaxed) {
            return Err(TransportError::WorkerClosed);
        }
        worker.writer.try_send(bytes).map_err(|e| match e {
            crossbeam_channel::TrySendError::Full(_) => TransportError::QueueFull,
            crossbeam_channel::TrySendError::Disconnected(_) => TransportError::WorkerClosed,
        })
    }

    // ── 向后兼容：发送到第一个已打开端口 ──
    pub fn send(&self, bytes: Vec<u8>) -> TransportResult<()> {
        let guard = self.ports.lock();
        let name = guard
            .keys()
            .next()
            .cloned()
            .ok_or(TransportError::NoOpenPort)?;
        drop(guard);
        self.send_to(&name, bytes)
    }

    #[deprecated = "多串口场景不安全：请使用 send_text_to(port_name, text)"]
    pub fn send_text(&self, text: &str) -> TransportResult<()> {
        self.send(text.as_bytes().to_vec())
    }

    pub fn send_text_to(&self, port_name: &str, text: &str) -> TransportResult<()> {
        self.send_to(port_name, text.as_bytes().to_vec())
    }

    #[deprecated = "多串口场景不安全：请使用 send_hex_to(port_name, hex)"]
    pub fn send_hex(&self, input: &str) -> TransportResult<()> {
        self.send(parse_hex(input)?)
    }

    pub fn send_hex_to(&self, port_name: &str, input: &str) -> TransportResult<()> {
        self.send_to(port_name, parse_hex(input)?)
    }

    // ── 状态 ──
    #[deprecated = "多串口场景语义不稳定：请使用 status_port(port_name)"]
    pub fn status(&self) -> TransportStatus {
        let guard = self.ports.lock();
        match guard.values().next() {
            Some(w) if w.alive.load(Ordering::Relaxed) => TransportStatus {
                open: true,
                port_name: Some(w.config.port_name.clone()),
                baud_rate: Some(w.config.baud_rate),
            },
            _ => TransportStatus::closed(),
        }
    }

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
        self.reap_closing();
        let guard = self.ports.lock();
        let resolved = Self::resolve_open_port_name_locked(&guard, port_name)
            .ok_or_else(|| TransportError::PortNotOpen(port_name.to_owned()))?;
        let worker = guard
            .get(&resolved)
            .ok_or_else(|| TransportError::PortNotOpen(port_name.to_owned()))?;
        if !worker.alive.load(Ordering::Relaxed) {
            return Err(TransportError::WorkerClosed);
        }
        worker
            .dtr_rts_tx
            .try_send(DtrRtsCommand::SetDtr(value))
            .map_err(|e| match e {
                TrySendError::Full(_) => TransportError::QueueFull,
                TrySendError::Disconnected(_) => TransportError::WorkerClosed,
            })
    }

    pub fn set_rts(&self, port_name: &str, value: bool) -> TransportResult<()> {
        self.reap_closing();
        let guard = self.ports.lock();
        let resolved = Self::resolve_open_port_name_locked(&guard, port_name)
            .ok_or_else(|| TransportError::PortNotOpen(port_name.to_owned()))?;
        let worker = guard
            .get(&resolved)
            .ok_or_else(|| TransportError::PortNotOpen(port_name.to_owned()))?;
        if !worker.alive.load(Ordering::Relaxed) {
            return Err(TransportError::WorkerClosed);
        }
        worker
            .dtr_rts_tx
            .try_send(DtrRtsCommand::SetRts(value))
            .map_err(|e| match e {
                TrySendError::Full(_) => TransportError::QueueFull,
                TrySendError::Disconnected(_) => TransportError::WorkerClosed,
            })
    }

    /// 清理已退出 worker 的 stale port handle（alive == false）。
    /// 可在 status 查询、list/open/close 或定时刷新时调用。
    pub fn reap_dead_ports(&self) {
        let dead_names: Vec<String> = {
            let guard = self.ports.lock();
            guard
                .iter()
                .filter(|(_, h)| !h.alive.load(Ordering::Relaxed))
                .map(|(name, _)| name.clone())
                .collect()
        };
        for name in dead_names {
            self.close_port(&name);
        }
    }
}

// TransportManager 不再实现 Drop：
// clone 被 Lua plugin/PluginManager 多处持有，任意 clone drop 会误关所有串口。
// 关闭串口的唯一安全调用点是 WorkbenchApp::drop()。

// ── 串口工作线程 ──

fn serial_worker_loop(
    mut port: Box<dyn sp::SerialPort>,
    write_rx: Receiver<Vec<u8>>,
    dtr_rts_rx: Receiver<DtrRtsCommand>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    bus: DataBus,
    source: String,
) {
    let mut buffer = [0_u8; 4096];

    while !stop.load(Ordering::Relaxed) {
        // 处理 DTR/RTS 命令
        while let Ok(cmd) = dtr_rts_rx.try_recv() {
            match cmd {
                DtrRtsCommand::SetDtr(value) => {
                    if let Err(e) = port.write_data_terminal_ready(value) {
                        bus.publish(Event::system_log(
                            LogLevel::Error,
                            "transport.serial",
                            format!("set DTR failed on {source}: {e}"),
                        ));
                    }
                }
                DtrRtsCommand::SetRts(value) => {
                    if let Err(e) = port.write_request_to_send(value) {
                        bus.publish(Event::system_log(
                            LogLevel::Error,
                            "transport.serial",
                            format!("set RTS failed on {source}: {e}"),
                        ));
                    }
                }
            }
        }

        while let Ok(bytes) = write_rx.try_recv() {
            match port.write_all(&bytes) {
                Ok(()) => {
                    bus.publish(Event::serial_tx(source.clone(), bytes));
                }
                Err(error) => {
                    bus.publish(Event::system_log(
                        LogLevel::Error,
                        "transport.serial",
                        format!("write failed on {source}: {error}"),
                    ));
                    alive.store(false, Ordering::Relaxed);
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
                bus.publish(Event::serial_rx(source.clone(), data));
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => {
                bus.publish(Event::system_log(
                    LogLevel::Error,
                    "transport.serial",
                    format!("read failed on {source}: {error}"),
                ));
                alive.store(false, Ordering::Relaxed);
                return;
            }
        }
    }
    alive.store(false, Ordering::Relaxed);
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

    if tokens.len() == 1 {
        let mut token = normalize_hex_token(tokens[0]);
        if token.len() > 2 && !token.len().is_multiple_of(2) {
            token.insert(0, '0');
        }
        if token.len() > 2 {
            return token
                .as_bytes()
                .chunks(2)
                .map(|chunk| parse_byte(std::str::from_utf8(chunk).unwrap_or_default()))
                .collect();
        }
    }

    tokens
        .into_iter()
        .map(|token| {
            let token = normalize_hex_token(token);
            if token.is_empty() || token.len() > 2 {
                return Err(TransportError::InvalidHex(format!(
                    "invalid hex: '{token}'"
                )));
            }
            parse_byte(&token)
        })
        .collect()
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

fn describe_port_type(port_type: sp::SerialPortType) -> String {
    match port_type {
        sp::SerialPortType::UsbPort(usb) => usb.product.unwrap_or_else(|| "USB".to_owned()),
        sp::SerialPortType::BluetoothPort => "Bluetooth".to_owned(),
        sp::SerialPortType::PciPort => "PCI".to_owned(),
        sp::SerialPortType::Unknown => String::new(),
    }
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
}
