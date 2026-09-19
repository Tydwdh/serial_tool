use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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
    pub const UI_PANEL_SET_VALUES: &str = "ui.panel.set_values";
    pub const UI_FORM_SET_ENABLED: &str = "ui.form.set_enabled";
    pub const UI_FORM_SET_VISIBLE: &str = "ui.form.set_visible";
    pub const UI_FORM_FILE_BROWSE: &str = "ui.form.file_browse";
    pub const UI_FORM_FILE_SELECTED: &str = "ui.form.file_selected";
    pub const UI_TABLE_SET_ROWS: &str = "ui.table.set_rows";
    pub const UI_TABLE_APPEND_ROWS: &str = "ui.table.append_rows";
    pub const UI_TABLE_REMOVE_ROWS: &str = "ui.table.remove_rows";
    pub const UI_TABLE_CLEAR: &str = "ui.table.clear";
    pub const UI_TABLE_SELECTION_CHANGED: &str = "ui.table.selection_changed";
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
        Self::with_timestamp(now_timestamp_ms(), topic, source, direction, payload)
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
        // 上一步已保证 metadata 是 object；这里仍按 Option 处理而不是 expect，
        // 是刻意防御：宁可静默跳过这一次写入，也不让打日志/回放的路径 panic。
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

/// 返回用于计算耗时的跨平台时钟，单位为纳秒。
///
/// `wasm32-unknown-unknown` 没有 `std::time::Instant` 的实现，因此 Web
/// 构建必须使用浏览器提供的时间源。Native 使用进程内单调时钟；Web 使用
/// `Date.now()`，它足以支持短耗时统计，并且不会触发标准库的 unsupported
/// time panic。
pub fn monotonic_now_nanos() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now().max(0.0) * 1_000_000.0) as u64
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START
            .get_or_init(Instant::now)
            .elapsed()
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64
    }
}

/// 时间源抽象，使依赖时间戳的代码可测试。
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// 生产实现：使用系统时钟。
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        #[cfg(target_arch = "wasm32")]
        {
            // `wasm32-unknown-unknown` has no OS-backed implementation of
            // `std::time::SystemTime`.  Use the browser wall clock instead.
            js_sys::Date::now().max(0.0) as u64
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
                .unwrap_or_else(|e| {
                    log::warn!("system clock before UNIX_EPOCH: {e}");
                    1
                })
        }
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

// ── HEX 解析与预览：native 与 wasm 共用的唯一判定 ──
//
// 原先 `tool-transport` 与 `tool-application` 的 wasm 侧各有一份手抄实现，判定实际已经
// 分叉到「同一串 HEX 在 native 合法、在 web 非法」。`tool-core` 是两端都无条件依赖的
// crate，规则收到这里之后：transport 只管投递、application 只管路由、presentation 只管
// 校验，四方共用同一个判定函数。
//
// 错误值刻意只带裸文案（`"empty input"`、`"'ZZ' is not hex"`、`"严格模式: …"`）：
// `tool-transport` 用它重建 `TransportError::InvalidHex`，`translate_error` 的中文文案不变。

/// 宽松模式解析 HEX：接受 `0x`/`0X` 前缀、`_`/`-` 分隔符、空白/`,`/`;` 分词，
/// 并按兼容规则给奇数长度的长 token 左补 `0`。
pub fn parse_hex(input: &str) -> Result<Vec<u8>, String> {
    let tokens = split_hex_tokens(input)?;

    // 单 token 与多 token 走同一个 parse_hex_token，保证分块/补0规则一致。
    let mut out = Vec::new();
    for token in &tokens {
        out.extend(parse_hex_token(token)?);
    }
    Ok(out)
}

/// HEX 输入的统一分词：空输入报 `"empty input"`，否则按空白 / `,` / `;` 切开并丢掉空串。
fn split_hex_tokens(input: &str) -> Result<Vec<&str>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("empty input".to_owned());
    }
    Ok(trimmed
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',' || ch == ';')
        .filter(|token| !token.is_empty())
        .collect())
}

/// Parse HEX without the compatibility padding rules used by [`parse_hex`].
///
/// The strictness choice belongs to the Application command, so both Native
/// and Web can validate the same input before the backend sends it.
pub fn parse_hex_strict(input: &str) -> Result<Vec<u8>, String> {
    parse_hex_strict_line(input)
}

/// 解析单个 HEX token，返回其对应的字节。
///
/// 规则（单 token 与多 token 一致）：
/// - 去除 `0x`/`0X` 前缀，删除 `_`/`-` 分隔符。
/// - 长度 ≤ 2：直接解析为单字节（单 nibble 如 `"A"` 自动左补 0 → `0x0A`）。
/// - 长度 > 2 且为奇数：左补一个 `0` 再按每 2 字符分块。
/// - 长度 > 2 且为偶数：直接按每 2 字符分块。
fn parse_hex_token(token: &str) -> Result<Vec<u8>, String> {
    let mut token = normalize_hex_token(token);
    if token.is_empty() {
        return Err("empty token".to_owned());
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
fn parse_hex_strict_line(line: &str) -> Result<Vec<u8>, String> {
    let tokens = split_hex_tokens(line)?;
    let mut out = Vec::new();
    for token in &tokens {
        let normalized = normalize_hex_token(token);
        if normalized.is_empty() {
            return Err(format!("严格模式: 空 token \"{token}\""));
        }
        if normalized.len() != 2 {
            return Err(format!(
                "严格模式: \"{token}\" 规范化后为 {nib} 个字符，必须恰为 2（偶数 hex 长度），请补0或关闭严格模式",
                nib = normalized.len()
            ));
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

fn parse_byte(token: &str) -> Result<u8, String> {
    u8::from_str_radix(token, 16).map_err(|_| format!("'{token}' is not hex"))
}

/// HEX 预览：将输入解析为 HEX 字节并显示 ASCII 预览。
pub fn hex_preview(input: &str) -> String {
    if input.trim().is_empty() {
        return "—".to_owned();
    }
    const MAX_PREVIEW: usize = 32;
    match parse_hex(input) {
        Ok(bytes) if !bytes.is_empty() => {
            let count = bytes.len();
            let shown = &bytes[..count.min(MAX_PREVIEW)];
            let hex = shown
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            let ascii: String = shown
                .iter()
                .map(|&b| {
                    if b.is_ascii_graphic() || b == b' ' {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            let truncated = if count > MAX_PREVIEW {
                format!("… (共{count}B)")
            } else {
                String::new()
            };
            format!("{hex}{truncated}  |{ascii}|")
        }
        Ok(_) => "空".to_owned(),
        Err(_) => "解析失败".to_owned(),
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

    // ── HEX 解析：用例自 `tool-transport` 原样迁入（断言逐字未改）──
    //
    // 它们是「同一串 HEX 两端判定必须一致」的唯一钉住点，计数等式：
    // tool-core +12 条 = tool-transport -12 条。

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

    #[test]
    fn parse_hex_rejects_empty() {
        assert!(parse_hex("").is_err());
    }

    #[test]
    fn parse_hex_rejects_invalid_chars() {
        assert!(parse_hex("gg").is_err());
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
        assert_eq!(parse_hex_strict("0A 0B").unwrap(), vec![0x0A, 0x0B]);
    }

    #[test]
    fn parse_hex_strict_rejects_odd_long_token() {
        // 三字符 token "ABC" 严格模式应报错（不能自动补0）。
        assert!(parse_hex_strict_line("ABC").is_err());
    }

    // ── 预览渲染：presentation 侧 HEX 预览只此一份实现 ──
    #[test]
    fn hex_preview_renders_bytes_and_ascii() {
        assert_eq!(hex_preview("48 69"), "48 69  |Hi|");
        assert_eq!(hex_preview("AB CD"), "AB CD  |..|");
    }

    #[test]
    fn hex_preview_distinguishes_empty_and_invalid() {
        assert_eq!(hex_preview("   "), "—");
        assert_eq!(hex_preview("gg"), "解析失败");
    }

    #[test]
    fn hex_preview_truncates_beyond_32_bytes() {
        let input = vec!["41"; 40].join(" ");
        let preview = hex_preview(&input);
        assert!(
            preview.contains("… (共40B)"),
            "超过 32 字节必须截断并标注总长，实际: {preview}"
        );
        assert_eq!(
            preview.matches("41").count(),
            32,
            "十六进制列必须恰好渲染 32 字节，实际: {preview}"
        );
        assert_eq!(
            preview.split('|').nth(1).unwrap().chars().count(),
            32,
            "ASCII 列同样必须截到 32 字节，实际: {preview}"
        );
    }
}
