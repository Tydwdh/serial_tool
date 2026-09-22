//! Platform capabilities shared by the Native and Web composition roots.
//!
//! The application layer should depend on these stable identifiers and
//! asynchronous operations instead of assuming that every platform has a
//! string such as `COM3` or a synchronous filesystem.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod storage;

#[cfg(target_arch = "wasm32")]
pub mod web_serial;

#[cfg(target_arch = "wasm32")]
pub mod web_network;

#[cfg(target_arch = "wasm32")]
pub mod web_fetch;

#[cfg(not(target_arch = "wasm32"))]
pub mod native;

/// An opaque, platform-owned port identity.
///
/// Native values currently contain a canonical OS port name for compatibility
/// with the existing serial worker. Web values are generated from the
/// browser's authorized `SerialPort` object and must not be interpreted as a
/// filesystem or device path by application code.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct PortId(String);

impl PortId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PortId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A platform-neutral description of a port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortDescriptor {
    pub id: PortId,
    pub label: String,
    pub kind: PortKind,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    /// Whether the browser has already granted access to this port.
    pub authorized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortKind {
    Serial,
    Network,
    Unknown,
}

/// Configuration for the optional WebSocket-backed network serial endpoint.
///
/// The native transport crate has the same wire protocol, but the
/// configuration belongs here so both composition roots can use the same
/// platform-neutral value. A network port is represented as a normal
/// [`PortDescriptor`] with [`PortKind::Network`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkSerialConfig {
    pub host: String,
    pub port: u16,
    #[serde(default)]
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
    pub fn display_name(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Stable browser identity. The prefix prevents a network endpoint from
    /// colliding with a browser-owned SerialPort whose opaque id happens to
    /// have the same display text.
    pub fn port_id(&self) -> PortId {
        PortId::new(format!("network://{}:{}", self.host, self.port))
    }

    pub fn descriptor(&self) -> PortDescriptor {
        PortDescriptor {
            id: self.port_id(),
            label: self.display_name(),
            kind: PortKind::Network,
            vendor_id: None,
            product_id: None,
            authorized: true,
        }
    }
}

/// Serial line parameters accepted by both Native serialport and Web Serial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialSettings {
    pub baud_rate: u32,
    pub data_bits: u8,
    pub stop_bits: u8,
    pub parity: SerialParity,
}

impl Default for SerialSettings {
    fn default() -> Self {
        Self {
            baud_rate: 115_200,
            data_bits: 8,
            stop_bits: 1,
            parity: SerialParity::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SerialParity {
    None,
    Odd,
    Even,
}

/// Capabilities are data, not UI assumptions. The UI can use this to decide
/// whether to show “添加设备”, DTR/RTS controls, or a disabled explanation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportCapabilities {
    pub list_known_ports: bool,
    pub request_port: bool,
    pub connect: bool,
    pub disconnect: bool,
    pub send: bool,
    pub set_dtr: bool,
    pub set_rts: bool,
}

impl TransportCapabilities {
    pub const NATIVE_SERIAL: Self = Self {
        list_known_ports: true,
        request_port: true,
        connect: true,
        disconnect: true,
        send: true,
        set_dtr: true,
        set_rts: true,
    };

    pub const WEB_SERIAL: Self = Self {
        list_known_ports: true,
        request_port: true,
        connect: true,
        disconnect: true,
        send: true,
        set_dtr: true,
        set_rts: true,
    };

    pub const WEB_NETWORK: Self = Self {
        list_known_ports: false,
        request_port: false,
        connect: true,
        disconnect: true,
        send: true,
        set_dtr: false,
        set_rts: false,
    };
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TransportError {
    #[error("transport capability is not supported on this platform")]
    Unsupported,
    #[error("no authorized serial port is available")]
    NoPortAvailable,
    #[error("port is not known: {0}")]
    PortNotKnown(PortId),
    #[error("port is not connected: {0}")]
    PortNotConnected(PortId),
    #[error("browser denied the serial port request")]
    PermissionDenied,
    #[error("transport operation failed: {0}")]
    Operation(String),
}

pub type TransportResult<T> = Result<T, TransportError>;
pub type TransportFuture<T> = Pin<Box<dyn Future<Output = TransportResult<T>> + 'static>>;

/// 把平台传输错误翻译成用户可见的中文提示。
///
/// **与 `tool_transport::translate_error` 是同一个用户概念的两个枚举**：`tool-transport`
/// 只在 native 编译（wasm 依赖不到它），所以本函数与它必须各留一份；同一件事在两端说
/// 同一句话这件事由 `tests/transport_error_wording.rs` 机器钉住，改任何一侧的文案都会
/// 让那条断言变红 —— 请两边一起改，别只动一处。逐字抄来的那几条（`PortNotConnected`
/// 对应 `TransportError::PortNotOpen`）刻意不做改写。
///
/// `Operation(detail)` 原样返回：native 侧的 `operation_error` 放进来的已经是
/// `tool_transport::translate_error` 的结果（`QueueFull` / `WorkerClosed` / `InvalidHex`
/// 这些只有 transport 侧才有的变体都走这条路），再套一层中文框架就成了
/// 「操作失败：串口错误：…」这类同义反复。
///
/// 按变体类型化分发，不做字符串匹配（与 transport 侧同一份规则）。
pub fn translate_error(err: &TransportError) -> String {
    match err {
        TransportError::Unsupported => "此平台不支持该传输能力".into(),
        TransportError::NoPortAvailable => "没有可用的串口（请先连接设备或申请端口）".into(),
        TransportError::PortNotKnown(port) => {
            format!("未知串口 {port}（授权列表里没有它，请重新申请端口）")
        }
        // 与 `tool_transport::translate_error` 的 `PortNotOpen` 逐字相同。
        TransportError::PortNotConnected(port) => {
            format!("串口 {port} 未打开（可能已断开，请重连）")
        }
        TransportError::PermissionDenied => "浏览器拒绝了串口授权请求".into(),
        TransportError::Operation(detail) => detail.clone(),
    }
}

fn serial_event(
    topic: &str,
    direction: tool_core::Direction,
    port: &PortId,
    bytes: Vec<u8>,
) -> tool_core::Event {
    tool_core::Event::new(
        topic,
        format!("serial:{port}"),
        direction,
        tool_core::Payload::Bytes(bytes),
    )
    .with_metadata(serde_json::json!({ "port": port.as_str() }))
}

/// Build the canonical RX event without depending on the Native serial crate.
pub fn serial_rx_event(port: &PortId, bytes: Vec<u8>) -> tool_core::Event {
    serial_event(
        tool_core::topics::SERIAL_RX,
        tool_core::Direction::Rx,
        port,
        bytes,
    )
}

/// Build the canonical TX event without depending on the Native serial crate.
pub fn serial_tx_event(port: &PortId, bytes: Vec<u8>) -> tool_core::Event {
    serial_event(
        tool_core::topics::SERIAL_TX,
        tool_core::Direction::Tx,
        port,
        bytes,
    )
}

/// Events that a backend can surface to Application/DataBus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportEvent {
    Connected {
        port: PortId,
    },
    Disconnected {
        port: PortId,
    },
    Rx {
        port: PortId,
        bytes: Vec<u8>,
    },
    Tx {
        port: PortId,
        bytes: Vec<u8>,
    },
    Error {
        port: Option<PortId>,
        message: String,
    },
}

/// Common asynchronous transport capability.
///
/// No method blocks the caller. Native implementations move blocking serial
/// calls to a worker thread; Web implementations await JavaScript Promises.
pub trait TransportBackend: Clone + 'static {
    fn capabilities(&self) -> TransportCapabilities;
    fn list_known_ports(&self) -> TransportFuture<Vec<PortDescriptor>>;
    fn request_port(&self) -> TransportFuture<PortDescriptor>;
    fn connect(&self, port: PortId, settings: SerialSettings) -> TransportFuture<()>;
    fn disconnect(&self, port: PortId) -> TransportFuture<()>;
    fn send(&self, port: PortId, bytes: Vec<u8>) -> TransportFuture<()>;
    fn set_dtr(&self, port: PortId, value: bool) -> TransportFuture<()>;
    fn set_rts(&self, port: PortId, value: bool) -> TransportFuture<()>;
}
