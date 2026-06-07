use crossbeam_channel::{Sender, unbounded};
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
}

pub type TransportResult<T> = Result<T, TransportError>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataBits { Five, Six, Seven, Eight }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopBits { One, Two }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Parity { None, Odd, Even }

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
        Self { port_name: String::new(), baud_rate: 115_200, data_bits: DataBits::Eight,
               stop_bits: StopBits::One, parity: Parity::None, timeout_ms: 50 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SerialPortDescriptor { pub port_name: String, pub port_type: String }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportStatus {
    pub open: bool,
    pub port_name: Option<String>,
    pub baud_rate: Option<u32>,
}

impl TransportStatus {
    pub fn closed() -> Self { Self { open: false, port_name: None, baud_rate: None } }
}

#[derive(Clone)]
pub struct TransportManager {
    bus: DataBus,
    ports: Arc<Mutex<HashMap<String, PortHandle>>>,
}

struct PortHandle {
    config: SerialConfig,
    writer: Sender<Vec<u8>>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl TransportManager {
    pub fn new(bus: DataBus) -> Self {
        Self { bus, ports: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn list_serial_ports(&self) -> TransportResult<Vec<SerialPortDescriptor>> {
        let mut ports: Vec<SerialPortDescriptor> = sp::available_ports()?
            .into_iter().map(|info| SerialPortDescriptor { port_name: info.port_name, port_type: describe_port_type(info.port_type) }).collect();
        ports.sort_by_key(|port| natural_sort_key(&port.port_name));
        Ok(ports)
    }

    // ── 打开端口（不关闭已打开的端口）──
    pub fn open_serial(&self, config: SerialConfig) -> TransportResult<()> {
        // 如果同名端口已打开，先关掉旧的
        self.close_port(&config.port_name);

        let builder = sp::new(&config.port_name, config.baud_rate)
            .data_bits(config.data_bits.into())
            .stop_bits(config.stop_bits.into())
            .parity(config.parity.into())
            .timeout(Duration::from_millis(config.timeout_ms));

        let port = builder.open().map_err(|error| {
            self.bus.publish(Event::system_log(LogLevel::Error, "transport.serial", format!("failed to open {}: {error}", config.port_name)));
            TransportError::from(error)
        })?;

        let (writer, write_rx) = unbounded::<Vec<u8>>();
        let stop = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let thread_stop = Arc::clone(&stop);
        let thread_alive = Arc::clone(&alive);
        let thread_bus = self.bus.clone();
        let source = format!("serial:{}", config.port_name);
        let thread_source = source.clone();

        let join = thread::spawn(move || {
            serial_worker_loop(port, write_rx, thread_stop, thread_alive, thread_bus, thread_source);
        });

        self.ports.lock().insert(config.port_name.clone(), PortHandle {
            config: config.clone(), writer, stop, alive, join: Some(join),
        });

        self.bus.publish(Event::system_log(LogLevel::Info, "transport.serial", format!("opened {}", source)));
        Ok(())
    }

    // ── 关闭所有端口 ──
    pub fn close_serial(&self) {
        let names: Vec<String> = self.ports.lock().keys().cloned().collect();
        for name in names { self.close_port(&name); }
    }

    // ── 关闭指定端口 ──
    pub fn close_port(&self, port_name: &str) {
        if let Some(mut worker) = self.ports.lock().remove(port_name) {
            worker.stop.store(true, Ordering::Relaxed);
            if let Some(join) = worker.join.take() { let _ = join.join(); }
            self.bus.publish(Event::system_log(LogLevel::Info, "transport.serial", format!("closed {} @ {}", worker.config.port_name, worker.config.baud_rate)));
        }
    }

    // ── 发送到指定端口 ──
    pub fn send_to(&self, port_name: &str, bytes: Vec<u8>) -> TransportResult<()> {
        let guard = self.ports.lock();
        let worker = guard.get(port_name).ok_or_else(|| TransportError::PortNotOpen(port_name.to_owned()))?;
        if !worker.alive.load(Ordering::Relaxed) { return Err(TransportError::WorkerClosed); }
        worker.writer.send(bytes).map_err(|_| TransportError::WorkerClosed)
    }

    // ── 向后兼容：发送到第一个已打开端口 ──
    pub fn send(&self, bytes: Vec<u8>) -> TransportResult<()> {
        let guard = self.ports.lock();
        let name = guard.keys().next().cloned().ok_or(TransportError::NoOpenPort)?;
        drop(guard);
        self.send_to(&name, bytes)
    }

    pub fn send_text(&self, text: &str) -> TransportResult<()> {
        self.send(text.as_bytes().to_vec())
    }

    pub fn send_text_to(&self, port_name: &str, text: &str) -> TransportResult<()> {
        self.send_to(port_name, text.as_bytes().to_vec())
    }

    pub fn send_hex(&self, input: &str) -> TransportResult<()> {
        self.send(parse_hex(input)?)
    }

    pub fn send_hex_to(&self, port_name: &str, input: &str) -> TransportResult<()> {
        self.send_to(port_name, parse_hex(input)?)
    }

    // ── 状态 ──
    pub fn status(&self) -> TransportStatus {
        let guard = self.ports.lock();
        match guard.values().next() {
            Some(w) if w.alive.load(Ordering::Relaxed) => TransportStatus {
                open: true, port_name: Some(w.config.port_name.clone()), baud_rate: Some(w.config.baud_rate),
            },
            _ => TransportStatus::closed(),
        }
    }

    pub fn status_port(&self, port_name: &str) -> TransportStatus {
        let guard = self.ports.lock();
        match guard.get(port_name) {
            Some(w) if w.alive.load(Ordering::Relaxed) => TransportStatus {
                open: true, port_name: Some(w.config.port_name.clone()), baud_rate: Some(w.config.baud_rate),
            },
            _ => TransportStatus::closed(),
        }
    }

    pub fn status_all(&self) -> Vec<TransportStatus> {
        self.ports.lock().values().map(|w| {
            if w.alive.load(Ordering::Relaxed) {
                TransportStatus { open: true, port_name: Some(w.config.port_name.clone()), baud_rate: Some(w.config.baud_rate) }
            } else {
                TransportStatus::closed()
            }
        }).collect()
    }

    pub fn open_ports(&self) -> Vec<String> {
        self.ports.lock().keys().cloned().collect()
    }
}

impl Drop for TransportManager {
    fn drop(&mut self) { self.close_serial(); }
}

// ── 串口工作线程 ──

fn serial_worker_loop(
    mut port: Box<dyn sp::SerialPort>,
    write_rx: crossbeam_channel::Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    bus: DataBus,
    source: String,
) {
    let mut buffer = [0_u8; 4096];

    while !stop.load(Ordering::Relaxed) {
        while let Ok(bytes) = write_rx.try_recv() {
            match port.write_all(&bytes) {
                Ok(()) => {
                    bus.publish(Event::serial_tx(source.clone(), bytes));
                }
                Err(error) => {
                    bus.publish(Event::system_log(LogLevel::Error, "transport.serial", format!("write failed on {source}: {error}")));
                    alive.store(false, Ordering::Relaxed);
                    return;
                }
            }
        }

        match port.read(&mut buffer) {
            Ok(0) => {}
            Ok(size) => {
                let mut data = buffer[..size].to_vec();
                thread::sleep(Duration::from_millis(3));
                loop {
                    match port.read(&mut buffer) {
                        Ok(more) if more > 0 => data.extend_from_slice(&buffer[..more]),
                        _ => break,
                    }
                }
                bus.publish(Event::serial_rx(source.clone(), data));
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => {
                bus.publish(Event::system_log(LogLevel::Error, "transport.serial", format!("read failed on {source}: {error}")));
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
    if trimmed.is_empty() { return Err(TransportError::InvalidHex("empty input".to_owned())); }

    let tokens: Vec<&str> = trimmed
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',' || ch == ';')
        .filter(|token| !token.is_empty()).collect();

    if tokens.len() == 1 {
        let mut token = normalize_hex_token(tokens[0]);
        if token.len() > 2 && !token.len().is_multiple_of(2) { token.insert(0, '0'); }
        if token.len() > 2 {
            return token.as_bytes().chunks(2).map(|chunk| {
                parse_byte(std::str::from_utf8(chunk).unwrap_or_default())
            }).collect();
        }
    }

    tokens.into_iter().map(|token| {
        let token = normalize_hex_token(token);
        if token.is_empty() || token.len() > 2 {
            return Err(TransportError::InvalidHex(format!("invalid hex: '{token}'")));
        }
        parse_byte(&token)
    }).collect()
}

fn normalize_hex_token(token: &str) -> String {
    token.trim().trim_start_matches("0x").trim_start_matches("0X").replace(['_', '-'], "")
}

fn parse_byte(token: &str) -> TransportResult<u8> {
    u8::from_str_radix(token, 16).map_err(|_| TransportError::InvalidHex(format!("'{token}' is not hex")))
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
    let number: u64 = name.chars().skip_while(|c| !c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
    (prefix, number)
}

impl From<DataBits> for sp::DataBits {
    fn from(v: DataBits) -> Self {
        match v { DataBits::Five => Self::Five, DataBits::Six => Self::Six, DataBits::Seven => Self::Seven, DataBits::Eight => Self::Eight }
    }
}
impl From<StopBits> for sp::StopBits {
    fn from(v: StopBits) -> Self { match v { StopBits::One => Self::One, StopBits::Two => Self::Two } }
}
impl From<Parity> for sp::Parity {
    fn from(v: Parity) -> Self { match v { Parity::None => Self::None, Parity::Odd => Self::Odd, Parity::Even => Self::Even } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spaced_hex() { assert_eq!(parse_hex("01 0x02 ff").unwrap(), vec![1, 2, 255]); }
    #[test]
    fn parses_compact_hex() { assert_eq!(parse_hex("0102ff").unwrap(), vec![1, 2, 255]); }
    #[test]
    fn pads_odd_length_compact_hex() { assert_eq!(parse_hex("abc").unwrap(), vec![0x0a, 0xbc]); }
    #[test]
    fn parses_single_hex_token() { assert_eq!(parse_hex("FF").unwrap(), vec![255]); }
    #[test]
    fn parses_spaced_single_digits() { assert_eq!(parse_hex("1 2 3").unwrap(), vec![1, 2, 3]); }
}
