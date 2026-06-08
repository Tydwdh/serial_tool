use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub mod topics {
    pub const SERIAL_RX: &str = "transport.serial.default.rx";
    pub const SERIAL_TX: &str = "transport.serial.default.tx";
    pub const PROTOCOL_PID_SAMPLE: &str = "protocol.pid.sample";
    pub const PROTOCOL_IMU_ATTITUDE: &str = "protocol.imu.attitude";
    pub const PROTOCOL_WASM_DECODED: &str = "protocol.wasm.decoded";
    pub const LOG_SYSTEM: &str = "log.system";
    pub const UI_PANEL_CREATE: &str = "ui.panel.create";
    pub const UI_PANEL_REMOVE: &str = "ui.panel.remove";
    pub const UI_FORM_CHANGED: &str = "ui.form.changed";
    pub const TEST_RESULT: &str = "test.result";
}

// #[derive(Debug, Error)]
// pub enum CoreError {
//     #[error("invalid topic")]
//     InvalidTopic,
//     #[error("time moved backwards")]
//     TimeMovedBackwards,
// }

// pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Rx,
    Tx,
    Internal,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    pub fn parse_name(value: &str) -> Option<Self> {
        match value {
            "trace" => Some(Self::Trace),
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" | "warning" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

impl std::str::FromStr for LogLevel {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse_name(value).ok_or(())
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Payload {
    Empty,
    Bytes(Vec<u8>),
    Text(String),
    Json(Value),
}

impl Payload {
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(bytes) => Some(bytes.as_slice()),
            _ => None,
        }
    }

    pub fn text_lossy(&self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
            Self::Text(text) => text.clone(),
            Self::Json(value) => value.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: u64,
    pub timestamp_ms: u64,
    pub topic: String,
    pub source: String,
    pub direction: Direction,
    pub payload: Payload,
    pub metadata: Value,
}

impl Event {
    pub fn new(
        topic: impl Into<String>,
        source: impl Into<String>,
        direction: Direction,
        payload: Payload,
    ) -> Self {
        Self {
            id: 0,
            timestamp_ms: now_timestamp_ms(),
            topic: topic.into(),
            source: source.into(),
            direction,
            payload,
            metadata: json!({}),
        }
    }

    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn serial_rx(source: impl Into<String>, bytes: Vec<u8>) -> Self {
        let source = source.into();
        let port = source.strip_prefix("serial:").unwrap_or(&source).to_owned();
        Self::new(
            topics::SERIAL_RX,
            source,
            Direction::Rx,
            Payload::Bytes(bytes),
        )
        .with_metadata(json!({ "port": port }))
    }

    pub fn serial_tx(source: impl Into<String>, bytes: Vec<u8>) -> Self {
        let source = source.into();
        let port = source.strip_prefix("serial:").unwrap_or(&source).to_owned();
        Self::new(
            topics::SERIAL_TX,
            source,
            Direction::Tx,
            Payload::Bytes(bytes),
        )
        .with_metadata(json!({ "port": port }))
    }

    pub fn system_log(
        level: LogLevel,
        source: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(
            topics::LOG_SYSTEM,
            source,
            Direction::Internal,
            Payload::Text(message.into()),
        )
        .with_metadata(json!({ "level": level.as_str() }))
    }

    pub fn json(topic: impl Into<String>, source: impl Into<String>, payload: Value) -> Self {
        Self::new(topic, source, Direction::Internal, Payload::Json(payload))
    }

    pub fn payload_len(&self) -> usize {
        match &self.payload {
            Payload::Empty => 0,
            Payload::Bytes(bytes) => bytes.len(),
            Payload::Text(text) => text.len(),
            Payload::Json(value) => value.to_string().len(),
        }
    }

    // ── metadata 工具方法 ──

    /// 安全获取 metadata 字段的值。如果 metadata 不是 JSON object，返回 None。
    pub fn meta_get(&self, key: &str) -> Option<&Value> {
        self.metadata.as_object()?.get(key)
    }

    /// 获取 metadata bool 值，缺省 false。
    pub fn meta_bool(&self, key: &str) -> bool {
        self.meta_get(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// 获取 metadata 字符串值。
    pub fn meta_str(&self, key: &str) -> Option<&str> {
        self.meta_get(key).and_then(|v| v.as_str())
    }

    /// 安全写入 metadata 字段。如果 metadata 不是 JSON object，先替换为 `{}`。
    pub fn meta_set(&mut self, key: &str, value: Value) {
        if !self.metadata.is_object() {
            self.metadata = json!({});
        }
        if let Some(obj) = self.metadata.as_object_mut() {
            obj.insert(key.to_owned(), value);
        }
    }

    /// 检查是否为回放事件。
    pub fn is_replay(&self) -> bool {
        self.meta_bool("replay")
    }

    /// 检查事件类别（raw / derived / ephemeral）。
    pub fn category(&self) -> Option<&str> {
        self.meta_str("category")
    }

    /// 检查事件来源（live / replay / replay_derived）。
    pub fn origin(&self) -> Option<&str> {
        self.meta_str("origin")
    }
}

/// 给 analyzer 输出事件打 replay_derived 标记。
/// `derived_from` 可以是一个或多个输入事件 id。
pub fn mark_derived_event(
    event: &mut Event,
    plugin_id: &str,
    plugin_version: &str,
    derived_from: &[u64],
) {
    event.source = format!("replay-analyzer:{plugin_id}");
    event.meta_set("replay", Value::Bool(true));
    event.meta_set("origin", Value::String("replay_derived".to_owned()));
    event.meta_set("category", Value::String("derived".to_owned()));
    event.meta_set("derived", Value::Bool(true));
    event.meta_set("plugin_id", Value::String(plugin_id.to_owned()));
    event.meta_set("plugin_version", Value::String(plugin_version.to_owned()));
    event.meta_set(
        "derived_from",
        Value::Array(
            derived_from
                .iter()
                .map(|id| Value::Number((*id).into()))
                .collect(),
        ),
    );
    event.meta_set("recordable", Value::Bool(false));
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppCoreConfig {
    pub workspace_name: String,
    pub max_bus_history: usize,
}

impl Default for AppCoreConfig {
    fn default() -> Self {
        Self {
            workspace_name: "hardware-workbench".to_owned(),
            max_bus_history: 20_000,
        }
    }
}

pub fn now_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_payload_has_lossy_text() {
        let payload = Payload::Bytes(vec![0x48, 0x69]);
        assert_eq!(payload.text_lossy(), "Hi");
    }

    #[test]
    fn system_log_has_level_metadata() {
        let event = Event::system_log(LogLevel::Warn, "test", "careful");
        assert_eq!(event.topic, topics::LOG_SYSTEM);
        assert_eq!(event.metadata["level"], "warn");
    }
}
