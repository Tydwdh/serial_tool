use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// JSON 配置文件的通用读写与恢复工具。
pub mod config {
    use serde::Serialize;
    use std::fs;
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};

    /// 所有由工作台管理的 JSON 配置文档当前使用的 schema 版本。
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;

    /// 读出配置对象声明的 schema 版本，只回答"这个文件声明了什么"。
    ///
    /// 键名规则：`schema_version` 优先，历史键名 `version` 兜底；两个键都不存在，
    /// 或先命中的那个键不是整数时返回 `None`。
    ///
    /// `version` 是 `schema_version` 之前的历史键名，所有读取持久化配置的地方
    /// 都必须同时接受两个键，否则这类配置会被读成 v0：既可能重复执行 v0 迁移，
    /// 也可能漏掉"版本高于当前程序"的判定。
    ///
    /// 覆盖在它之上的策略——拒绝加载、隔离坏文件、按 v0 迁移——**刻意留在各调用方**，
    /// 每类配置的取舍不同；本函数不改变任何一方的判定。
    pub fn declared_schema_version(
        object: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<u32> {
        object
            .get("schema_version")
            .or_else(|| object.get("version"))
            .and_then(serde_json::Value::as_u64)
            .map(|version| version as u32)
    }

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
        // 是刻意防御：宁可静默跳过这一次写入，也不让打日志/导出的路径 panic。
        if let Some(obj) = self.metadata.as_object_mut() {
            obj.insert(key.to_owned(), value);
        }
    }
}

/// 跨平台挂钟时间，单位为毫秒。
///
/// 读系统时钟并把毫秒数钳制到 `u64`。系统时钟落在 `UNIX_EPOCH` 之前时
/// 记一条 warning 并返回 `1`——刻意不返回 `0`，调用方可以假定时间戳非零。
///
/// 这里曾有 `Clock`/`SystemClock`/`FrozenClock`，但没有任何函数接收过时钟，它不是
/// 可注入的接缝，故整组删除；真要冻结时间，给那几个必须确定性的函数加一个时钟入参。
pub fn now_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_else(|e| {
            log::warn!("system clock before UNIX_EPOCH: {e}");
            1
        })
}

/// 返回用于计算耗时的跨平台时钟，单位为纳秒。
///
/// 使用进程内单调时钟，单位为纳秒。
pub fn monotonic_now_nanos() -> u64 {
    static START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
    START.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
}

/// Topic 匹配：`*` 后缀按前缀匹配，不带 `*` 精确匹配。
/// 供实时事件路由、Lua callback 统一使用。
/// Configuration for the WebSocket-backed network serial endpoint.
///
/// One definition lives here because both the shared platform layer and the
/// native transport crate need it; each used to carry an identical copy.
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
}

pub fn topic_matches(pattern: &str, topic: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        topic.starts_with(prefix)
    } else {
        topic == pattern
    }
}

// ── 换行符表：native 与 wasm 共用的唯一一份 ──
//
// 原先 `tool-panels` 的 `SendLineEnding`、桌面 `state::LineEnding`、浏览器设置的
// `WebLineEnding` 各抄了一份完全相同的四行表，改一处就会漏改另两处。
// `tool-core` 是这三处都无条件依赖的 crate，因此表收在这里。
//
// 变体名与顺序是**持久化格式的一部分**：桌面 workspace.json 与浏览器设置存的都是
// 裸变体名字符串（`"None"`/`"Lf"`/`"Cr"`/`"Crlf"`），重命名或调序会让老配置读不回来。
// 本文件末尾的用例按字面量钉住这个形状。

/// 文本发送追加的行尾符。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineEnding {
    #[default]
    None,
    Lf,
    Cr,
    Crlf,
}

impl LineEnding {
    /// 下拉框的候选顺序，与持久化无关，只决定 UI 展示次序。
    pub const ALL: [Self; 4] = [Self::None, Self::Lf, Self::Cr, Self::Crlf];

    /// 下拉框与插件上下文里展示的名字。
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::Lf => "LF",
            Self::Cr => "CR",
            Self::Crlf => "CRLF",
        }
    }

    /// 追加到待发文本末尾的字节串；HEX 模式不使用它。
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Lf => "\n",
            Self::Cr => "\r",
            Self::Crlf => "\r\n",
        }
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

/// 从 JSON-RPC 文本中提取 gcode 响应内容。
/// 只处理 `notify_gcode_response` 推送，其余 notify（状态/进程统计等）一律忽略。
///
/// 收在 `tool-core`：`tool-platform` 与 `tool-transport` 都无条件依赖它，
/// 解析规则放在这里，两侧共用同一份实现。
pub fn parse_gcode_response(text: &str) -> Option<String> {
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

    /// 系统时钟落在 `UNIX_EPOCH` 之前那一条兜底臂：文档承诺"时间戳非零"，
    /// 而这条分支**跑不到** —— 第 27 轮删掉 `Clock` 之后（此前也没有任何函数接收过时钟）
    /// 没有任何地方能把系统时钟拨到 epoch 之前，所以钉形状是这里唯一可用的机器检查。
    ///
    /// 做法：只取 `now_timestamp_ms` 的函数体、丢掉注释行、压掉空白，再看那条
    /// `log::warn!(…); 1 }` 的字面值。压空白让它对 rustfmt 的重新缩进不敏感。
    ///
    /// **这条断言的限度要如实说**：它钉的是文本，不是运行时行为 —— 把 `1` 换成
    /// `nonzero_const` 这类语义等价的写法它也会红（那时应连同文档一起重新审），
    /// 反过来它也证明不了那一支真能返回非零。
    #[test]
    fn clock_before_epoch_fallback_is_pinned_to_a_nonzero_literal() {
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
            .expect("读得到自己的源码，这条用例与实现同文件")
            // 本仓的源文件里有 CRLF：不归一化 `}` 那行的收尾就对不上，
            // 切片会一路跑到文件末尾，把后面的代码当成"函数体"。
            .replace("\r\n", "\n");
        let head = "pub fn now_timestamp_ms() -> u64 {";
        let start = src
            .find(head)
            .unwrap_or_else(|| panic!("找不到 `{head}`，实现被改名或搬走时这条用例要一起挪"));
        let body = &src[start + head.len()..];
        let body = body
            .split('\n')
            .take_while(|line| *line != "}")
            // 注释行不算实现：函数自己的文档里同时写着 `1` 和 `0`。
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>()
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>();

        assert!(
            body.contains(r#");1})"#),
            "`now_timestamp_ms` 的 epoch 之前兜底臂不再是「记 warning 后返回 1」，\
             而函数文档仍然向调用方承诺时间戳非零：{body}"
        );
        assert!(
            !body.contains(r#");0})"#),
            "实现里出现了返回 0 的兜底臂？那就是把非零承诺改成了返回 0"
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

    // ── 换行符表：持久化形状取证 ──
    //
    // 期望值一律写字面量：三份副本合并成一个类型之后，「拿两个名字比一下」的断言会
    // 恒真，只有字面量还能在改名、调序或加 `rename_all` 时变红。

    #[test]
    fn line_ending_persisted_names_are_bare_variant_strings() {
        // 桌面 workspace.json 与浏览器设置里都是这个裸名字符串。
        for (value, name) in [
            (LineEnding::None, "\"None\""),
            (LineEnding::Lf, "\"Lf\""),
            (LineEnding::Cr, "\"Cr\""),
            (LineEnding::Crlf, "\"Crlf\""),
        ] {
            assert_eq!(serde_json::to_string(&value).unwrap(), name);
            assert_eq!(serde_json::from_str::<LineEnding>(name).unwrap(), value);
        }

        // 老配置里的未知值仍应报错，而不是被静默当成某个变体。
        assert!(serde_json::from_str::<LineEnding>("\"crlf\"").is_err());
        assert!(serde_json::from_str::<LineEnding>("3").is_err());
    }

    #[test]
    fn line_ending_label_and_suffix_tables() {
        // label 会随插件上下文（`send.line_ending.label`）外泄给 Lua，文案同样是契约。
        assert_eq!(
            LineEnding::ALL.map(|it| (it.label(), it.suffix())),
            [("无", ""), ("LF", "\n"), ("CR", "\r"), ("CRLF", "\r\n")]
        );
        // CRLF 的字节序不能被写成 "\n\r"。
        assert_eq!(LineEnding::Crlf.suffix(), "\r\n");
    }

    #[test]
    fn line_ending_default_is_none() {
        // 缺字段的配置（`#[serde(default)]`）与新建状态都落到 None，不能追加任何字节。
        assert_eq!(LineEnding::default(), LineEnding::None);
        assert_eq!(LineEnding::default().suffix(), "");
    }

    // ── gcode 响应解析：用例自 `tool-transport::network` 迁入，原有断言逐字未改 ──

    #[test]
    fn extracts_gcode_response() {
        let msg = r#"{"jsonrpc":"2.0","method":"notify_gcode_response","params":["ok BED_SIZE"]}"#;
        assert_eq!(parse_gcode_response(msg).as_deref(), Some("ok BED_SIZE"));
        // 第二条正例（原来只断言 `"ok"`）。
        let another = r#"{"jsonrpc":"2.0","method":"notify_gcode_response","params":["ok"]}"#;
        assert_eq!(parse_gcode_response(another).as_deref(), Some("ok"));
    }

    #[test]
    fn ignores_status_update() {
        let msg = r#"{"jsonrpc":"2.0","method":"notify_status_update","params":[{"print_stats":{"state":"printing"}}]}"#;
        assert_eq!(parse_gcode_response(msg), None);
        // `notify_status_update` + 空 `params`。它和上面那条一样在
        // method 守卫处就返回 None，并没有走到 `.first()?`——真正钉住那一行的是
        // `ignores_gcode_notification_with_no_text_params`。
        let empty_params = r#"{"jsonrpc":"2.0","method":"notify_status_update","params":[]}"#;
        assert_eq!(parse_gcode_response(empty_params), None);
        // 非 gcode 推送、但首元素是字符串。
        // 上面两条反例即使删掉 method 守卫也仍然返回 None（对象走 `.as_str()`、
        // 空数组走 `.first()?`），只有这条能钉住守卫本身。
        let text_payload =
            r#"{"jsonrpc":"2.0","method":"notify_status_update","params":["printing"]}"#;
        assert_eq!(parse_gcode_response(text_payload), None);
    }

    /// 守卫放行之后 params 形状不对的两条反例：`.first()?`（空数组）与
    /// `.as_str()`（首元素不是字符串）。两份旧副本的用例都只在 method 守卫处返回
    /// None，摸不到这两行；没有本用例，把 `.first()?` 改坏不会有任何测试变红。
    #[test]
    fn ignores_gcode_notification_with_no_text_params() {
        let empty = r#"{"jsonrpc":"2.0","method":"notify_gcode_response","params":[]}"#;
        assert_eq!(parse_gcode_response(empty), None);
        let not_text =
            r#"{"jsonrpc":"2.0","method":"notify_gcode_response","params":[{"state":"printing"}]}"#;
        assert_eq!(parse_gcode_response(not_text), None);
    }

    #[test]
    fn ignores_plain_responses_and_non_json() {
        let msg = r#"{"jsonrpc":"2.0","result":"ok","id":1}"#;
        assert_eq!(parse_gcode_response(msg), None);
        assert_eq!(parse_gcode_response("not json"), None);
    }

    // ── config::declared_schema_version：版本键名规则 ──
    //
    // 这条规则曾在三个 crate 里各写一遍、只靠注释保持一致，以下用例是它的第一个
    // 机器检查：删掉任一键名、颠倒优先级、或把非整数读成 Some(0) 都会在这里变红。

    fn object_of(source: &str) -> serde_json::Map<String, serde_json::Value> {
        serde_json::from_str::<Value>(source)
            .expect("用例本身必须是合法 JSON")
            .as_object()
            .expect("用例根节点必须是 JSON 对象")
            .clone()
    }

    #[test]
    fn declared_schema_version_accepts_both_key_names() {
        let versioned = object_of(r#"{"schema_version": 2}"#);
        assert_eq!(config::declared_schema_version(&versioned), Some(2));

        // `version` 是 `schema_version` 之前的历史键名：单独出现也算版本声明，
        // 不能被读成 v0。
        let legacy = object_of(r#"{"version": 3}"#);
        assert_eq!(config::declared_schema_version(&legacy), Some(3));
    }

    #[test]
    fn declared_schema_version_prefers_the_current_key_name() {
        let both = object_of(r#"{"schema_version": 2, "version": 7}"#);
        assert_eq!(config::declared_schema_version(&both), Some(2));
    }

    #[test]
    fn declared_schema_version_needs_an_integer_value() {
        // 两个键都没有：文件没有声明版本。
        let unversioned = object_of(r#"{"values": {}}"#);
        assert_eq!(config::declared_schema_version(&unversioned), None);

        // 值不是整数（字符串 / null / 浮点）同样不算声明版本。
        for source in [
            r#"{"version": "2"}"#,
            r#"{"schema_version": null}"#,
            r#"{"schema_version": 1.5}"#,
        ] {
            assert_eq!(
                config::declared_schema_version(&object_of(source)),
                None,
                "非整数值应读成 None：{source}"
            );
        }

        // 先命中的键决定结果：`schema_version` 是非整数时不会回落到 `version`。
        let broken_current = object_of(r#"{"schema_version": "x", "version": 4}"#);
        assert_eq!(config::declared_schema_version(&broken_current), None);
    }
}
