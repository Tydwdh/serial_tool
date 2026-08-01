use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

/// JSON 配置文件的通用读写与恢复工具。
pub mod config {
    use serde::Serialize;
    use std::fs;
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};

    /// 所有由工作台管理的 JSON 配置文档当前使用的 schema 版本。
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;

    /// 原子写入文本：先落盘到同目录临时文件，再备份并替换目标文件。
    pub fn atomic_write_text(path: &Path, text: &str) -> Result<(), String> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|error| format!("创建配置目录失败：{error}"))?;

        let temp_path = path.with_extension("tmp");
        let backup_path = path.with_extension("json.backup");
        let write_result = (|| -> io::Result<()> {
            let mut file = fs::File::create(&temp_path)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temp_path);
            return Err(format!("写入临时配置失败：{error}"));
        }

        if path.exists()
            && let Err(error) = fs::copy(path, &backup_path)
        {
            log::warn!(
                "config: failed to backup {} to {}: {error}",
                path.display(),
                backup_path.display()
            );
        }

        fs::rename(&temp_path, path).map_err(|error| {
            let _ = fs::remove_file(&temp_path);
            format!("原子替换配置失败：{error}")
        })
    }

    /// 序列化并原子写入 JSON 文档。
    pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
        let data =
            serde_json::to_string_pretty(value).map_err(|error| format!("序列化失败：{error}"))?;
        atomic_write_text(path, &data)
    }

    /// 将无法解析的配置移动到同目录的带时间戳备份，避免下次启动继续读取坏文件。
    pub fn quarantine_corrupt_file(path: &Path) -> Result<Option<PathBuf>, String> {
        if !path.exists() {
            return Ok(None);
        }

        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("config.json");
        let timestamp = crate::now_timestamp_ms();
        let mut backup = path.with_file_name(format!("{name}.corrupt-{timestamp}.backup"));
        let mut suffix = 1_u32;
        while backup.exists() {
            backup = path.with_file_name(format!("{name}.corrupt-{timestamp}-{suffix}.backup"));
            suffix += 1;
        }

        fs::rename(path, &backup).map_err(|error| {
            format!(
                "备份损坏配置 {} 到 {} 失败：{error}",
                path.display(),
                backup.display()
            )
        })?;
        Ok(Some(backup))
    }
}

pub mod topics {
    pub const SERIAL_RX: &str = "transport.serial.default.rx";
    pub const SERIAL_TX: &str = "transport.serial.default.tx";
    pub const SERIAL_OPENED: &str = "transport.serial.opened";
    pub const SERIAL_CLOSED: &str = "transport.serial.closed";
    pub const PROTOCOL_PID_SAMPLE: &str = "protocol.pid.sample";
    pub const PROTOCOL_IMU_ATTITUDE: &str = "protocol.imu.attitude";
    pub const LOG_SYSTEM: &str = "log.system";
    pub const UI_PANEL_CREATE: &str = "ui.panel.create";
    pub const UI_PANEL_REMOVE: &str = "ui.panel.remove";
    pub const UI_FORM_CHANGED: &str = "ui.form.changed";
    pub const UI_FORM_ACTION: &str = "ui.form.action";
    pub const UI_FORM_SET_VALUE: &str = "ui.form.set_value";
    pub const UI_FORM_SET_ENABLED: &str = "ui.form.set_enabled";
    pub const UI_FORM_SET_VISIBLE: &str = "ui.form.set_visible";
    pub const UI_FORM_FILE_BROWSE: &str = "ui.form.file_browse";
    pub const UI_FORM_FILE_SELECTED: &str = "ui.form.file_selected";
    pub const UI_CONTRIBUTION_SET_VALUE: &str = "ui.contribution.set_value";
    pub const UI_SET_STATUS: &str = "ui.set.status";
    pub const PLUGIN_COMMAND_EXECUTE: &str = "plugin.command.execute";
    pub const PLUGIN_COMMAND_REGISTERED: &str = "plugin.command.registered";
    pub const PLUGIN_COMMAND_UNREGISTERED: &str = "plugin.command.unregistered";
    pub const TEST_RESULT: &str = "test.result";
}

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
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse_name(value).ok_or_else(|| format!("invalid log level: '{value}'"))
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

    /// 使用指定时间戳创建事件（用于测试中冻结时间）。
    pub fn with_timestamp(
        timestamp_ms: u64,
        topic: impl Into<String>,
        source: impl Into<String>,
        direction: Direction,
        payload: Payload,
    ) -> Self {
        Self {
            id: 0,
            timestamp_ms,
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
        // 安全获取 as_object_mut：因前面已确保 metadata 是 object，
        // 但为防御性编程，仍使用 expect 给出明确错误信息。
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

pub fn now_timestamp_ms() -> u64 {
    SystemClock.now_ms()
}

/// 时间源抽象，使依赖时间戳的代码可测试。
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// 生产实现：使用系统时钟。
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_else(|e| {
                log::warn!("system clock before UNIX_EPOCH: {e}");
                1
            })
    }
}

/// 测试用冻结时钟。
#[cfg(test)]
#[derive(Clone)]
pub struct FrozenClock {
    now_ms: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

#[cfg(test)]
impl FrozenClock {
    pub fn new(initial_ms: u64) -> Self {
        Self {
            now_ms: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(initial_ms)),
        }
    }

    pub fn advance(&self, ms: u64) {
        self.now_ms
            .fetch_add(ms, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set(&self, ms: u64) {
        self.now_ms.store(ms, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
impl Clock for FrozenClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Topic 匹配：`*` 后缀按前缀匹配，不带 `*` 精确匹配。
/// 供实时事件路由、replay analyzer、Lua callback 统一使用。
pub fn topic_matches(pattern: &str, topic: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        topic.starts_with(prefix)
    } else {
        topic == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_matches_exact_and_prefix() {
        // 精确匹配
        assert!(topic_matches("a.b.c", "a.b.c"));
        assert!(!topic_matches("a.b.c", "a.b.d"));
        // `*` 后缀：前缀匹配
        assert!(topic_matches(
            "transport.serial.*",
            "transport.serial.default.rx"
        ));
        assert!(topic_matches("transport.serial.*", "transport.serial."));
        assert!(!topic_matches("transport.serial.*", "transport.usb.x"));
        // 空 pattern 只匹配空 topic
        assert!(topic_matches("", ""));
        assert!(!topic_matches("", "x"));
        // pattern 等于 topic 且无 `*`：精确匹配
        assert!(topic_matches("log.system", "log.system"));
    }

    #[test]
    fn log_level_roundtrip() {
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ] {
            // as_str ↔ parse_name 往返
            assert_eq!(LogLevel::parse_name(level.as_str()), Some(level));
            // Display 与 as_str 一致
            assert_eq!(level.to_string(), level.as_str());
            // FromStr
            assert_eq!(level.as_str().parse::<LogLevel>().unwrap(), level);
        }
        // "warning" 是 "warn" 的别名
        assert_eq!(LogLevel::parse_name("warning"), Some(LogLevel::Warn));
        // 无效输入返回 None / Err
        assert_eq!(LogLevel::parse_name("fatal"), None);
        assert!("fatal".parse::<LogLevel>().is_err());
    }

    #[test]
    fn now_timestamp_ms_is_plausible() {
        // 2026 年的时间戳远大于 1（UNIX_EPOCH 异常时返回 1）
        let ts = now_timestamp_ms();
        assert!(
            ts > 1_700_000_000_000,
            "timestamp should be after 2023: {ts}"
        );
    }

    #[test]
    fn meta_set_replaces_non_object_metadata_without_panic() {
        // 锁定行为：metadata 为非 object（如数组）时，meta_set 先替换为 {} 再写入，不 panic。
        let mut event = Event::new("t", "s", Direction::Internal, Payload::Empty);
        event.metadata = json!([1, 2, 3]); // 畸形 metadata
        event.meta_set("k", json!(42));
        assert_eq!(event.meta_get("k"), Some(&json!(42)));
        // 原数组已被替换为空 object，k 是唯一字段
        assert!(event.metadata.is_object());
    }

    #[test]
    fn meta_accessors() {
        let mut event = Event::new("t", "s", Direction::Internal, Payload::Empty);
        assert_eq!(event.meta_get("missing"), None);
        assert!(!event.meta_bool("missing")); // 缺省 false
        assert_eq!(event.meta_str("missing"), None);
        event.meta_set("flag", json!(true));
        assert!(event.meta_bool("flag"));
        event.meta_set("name", json!("hello"));
        assert_eq!(event.meta_str("name"), Some("hello"));
    }

    #[test]
    fn mark_derived_event_sets_full_metadata() {
        let mut event = Event::new("protocol.x", "src", Direction::Internal, Payload::Empty);
        mark_derived_event(&mut event, "myplugin", "1.2.3", &[10, 20]);
        assert!(event.is_replay());
        assert_eq!(event.origin(), Some("replay_derived"));
        assert_eq!(event.category(), Some("derived"));
        assert!(event.meta_bool("derived"));
        assert_eq!(event.meta_str("plugin_id"), Some("myplugin"));
        assert_eq!(event.meta_str("plugin_version"), Some("1.2.3"));
        assert!(!event.meta_bool("recordable"));
        // derived_from 为数组 [10, 20]
        let derived_from = event.meta_get("derived_from").unwrap();
        assert_eq!(derived_from, &json!([10, 20]));
        assert_eq!(event.source, "replay-analyzer:myplugin");
    }

    #[test]
    fn payload_text_lossy_and_event_len() {
        let bytes = Payload::Bytes(vec![0x48, 0x69]); // "Hi"
        assert_eq!(bytes.text_lossy(), "Hi");
        assert_eq!(bytes.as_bytes(), Some(&[0x48, 0x69][..]));

        let text = Payload::Text("hello".to_owned());
        assert_eq!(text.text_lossy(), "hello");
        assert_eq!(text.as_bytes(), None); // Text 不是 Bytes

        let empty = Payload::Empty;
        assert_eq!(empty.text_lossy(), "");

        let json_payload = Payload::Json(json!({"a": 1}));
        assert_eq!(json_payload.text_lossy(), json!({"a": 1}).to_string());

        // payload_len 是 Event 的方法
        let ev_bytes = Event::new("t", "s", Direction::Rx, Payload::Bytes(vec![1, 2, 3]));
        assert_eq!(ev_bytes.payload_len(), 3);
        let ev_text = Event::new("t", "s", Direction::Rx, Payload::Text("hello".to_owned()));
        assert_eq!(ev_text.payload_len(), 5);
        let ev_empty = Event::new("t", "s", Direction::Rx, Payload::Empty);
        assert_eq!(ev_empty.payload_len(), 0);
    }

    #[test]
    fn event_with_metadata_replaces() {
        let event = Event::new("t", "s", Direction::Internal, Payload::Empty)
            .with_metadata(json!({"preset": 1}));
        assert_eq!(event.meta_get("preset"), Some(&json!(1)));
    }
}
