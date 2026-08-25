use crate::bootstrap::{user_data_dir, user_logs_dir};
use crate::state::LineEnding;
use std::path::{Path, PathBuf};
use tool_application::api::core::{
    config::{CURRENT_SCHEMA_VERSION, atomic_write_json, quarantine_corrupt_file},
    now_timestamp_ms,
};
use tool_application::query::NetworkPortConfig;
use tool_application::query::RecordModeView;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tool_panels::PanelManager;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PortProfile {
    pub(crate) baud_rate: String,
    pub(crate) data_bits: String,
    pub(crate) stop_bits: String,
    pub(crate) parity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedConfig {
    /// 配置格式版本。缺失时按 v0 读取并迁移到当前版本。
    #[serde(default, alias = "version")]
    pub(crate) schema_version: u32,
    pub(crate) panels: PanelManager,
    pub(crate) selected_port: Option<String>,
    pub(crate) baud_rate: String,
    pub(crate) data_bits: String,
    pub(crate) stop_bits: String,
    pub(crate) parity: String,
    pub(crate) recorder_path: String,

    #[serde(default)]
    pub(crate) enabled_plugins: Vec<String>,
    #[serde(default)]
    pub(crate) port_aliases: HashMap<String, String>,
    #[serde(default)]
    pub(crate) port_groups: HashMap<String, String>,
    #[serde(default)]
    pub(crate) send_history: Vec<String>,
    #[serde(default = "default_line_ending")]
    pub(crate) line_ending: LineEnding,
    #[serde(default)]
    pub(crate) port_profiles: HashMap<String, PortProfile>,
    #[serde(default)]
    pub(crate) recent_workspaces: Vec<String>,
    #[serde(default = "default_true")]
    pub(crate) auto_reconnect: bool,
    /// 可配置快捷键映射（默认 VSCode 风格）。
    #[serde(default)]
    pub(crate) keymap: crate::keymap::Keymap,
    /// 等宽字体大小（终端/日志区），默认 13.0。
    #[serde(default = "default_monospace_font_size")]
    pub(crate) monospace_font_size: f32,
    /// 旧版主题标识，仅用于迁移没有 `theme_path` 的配置。
    #[serde(default, skip_serializing)]
    pub(crate) ui_theme: tool_panels::theme::AppTheme,
    /// 当前主题 JSON 路径。内置和用户新增主题都走同一字段。
    #[serde(default, alias = "custom_theme_path")]
    pub(crate) theme_path: Option<String>,
    /// 终端展示块空闲结束阈值（ms）。仅用于展示块封存，不代表协议帧。默认 5。
    #[serde(default = "default_terminal_merge_window_ms")]
    pub(crate) terminal_merge_window_ms: u64,
    /// 终端保留条数上限。默认 2000。
    #[serde(default = "default_terminal_max_entries")]
    pub(crate) terminal_max_entries: usize,
    /// 日志保留条数上限。默认 2000。
    #[serde(default = "default_log_max_entries")]
    pub(crate) log_max_entries: usize,
    /// 命令面板使用顺序（label key 列表，最近使用的在前）。
    #[serde(default)]
    pub(crate) command_usage_order: Vec<String>,
    /// 市场与更新使用的可选 HTTP/SOCKS 代理地址；为空时自动使用系统/环境代理。
    #[serde(default)]
    pub(crate) network_proxy_url: Option<String>,
    /// 网络模拟串口列表（WebSocket + JSON-RPC gcode 桥，Nexus Prime 等 Klipper 服务器）。
    #[serde(default)]
    pub(crate) network_ports: Vec<NetworkPortConfig>,
}

fn default_terminal_merge_window_ms() -> u64 {
    5
}

fn default_terminal_max_entries() -> usize {
    50_000
}

fn default_log_max_entries() -> usize {
    50_000
}

fn default_true() -> bool {
    true
}

fn default_monospace_font_size() -> f32 {
    13.0
}

fn default_line_ending() -> LineEnding {
    LineEnding::None
}

/// 配置加载结果：区分"无配置文件"和"配置损坏"两种情况。
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum ConfigLoadResult {
    /// 成功加载
    Ok {
        config: PersistedConfig,
        migrated: bool,
    },
    /// 配置文件不存在（首次运行或配置被删除）
    NotFound,
    /// 配置文件存在但解析失败（配置损坏）
    ParseError {
        path: PathBuf,
        error: String,
        backup_path: Option<PathBuf>,
    },
    /// 配置来自更高版本的程序；为保护数据仅允许本次使用默认值，不写回原文件。
    FutureVersion { path: PathBuf, version: u32 },
}

fn declared_schema_version(text: &str) -> Option<u32> {
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    value
        .as_object()?
        .get("schema_version")
        .or_else(|| value.as_object()?.get("version"))
        .and_then(serde_json::Value::as_u64)
        .map(|version| version as u32)
}

fn parse_persisted_config(text: &str) -> Result<(PersistedConfig, bool), String> {
    let mut value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("JSON 解析失败：{error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "配置根节点必须是 JSON 对象".to_owned())?;
    let version = object
        .get("schema_version")
        .or_else(|| object.get("version"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    if version > CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "配置版本 {version} 高于当前程序支持的 {CURRENT_SCHEMA_VERSION}"
        ));
    }

    let migrated = version != CURRENT_SCHEMA_VERSION;
    if version == 0 {
        if !object.contains_key("theme_path")
            && let Some(legacy_theme_path) = object.get("custom_theme_path").cloned()
        {
            object.insert("theme_path".to_owned(), legacy_theme_path);
        }
        // `theme_path` 已经由迁移写入（或新旧字段同时存在）后，移除旧字段，避免
        // serde 的 alias 将两个键识别为同一字段。
        object.remove("custom_theme_path");
    }
    object.insert(
        "schema_version".to_owned(),
        serde_json::Value::from(CURRENT_SCHEMA_VERSION),
    );

    let mut config = serde_json::from_value::<PersistedConfig>(value)
        .map_err(|error| format!("配置字段无效：{error}"))?;
    config.schema_version = CURRENT_SCHEMA_VERSION;
    Ok((config, migrated))
}

pub(crate) fn load_config() -> ConfigLoadResult {
    let primary = config_path();

    // 尝试读主路径
    if let Ok(t) = std::fs::read_to_string(&primary) {
        if let Some(version) = declared_schema_version(&t)
            && version > CURRENT_SCHEMA_VERSION
        {
            return ConfigLoadResult::FutureVersion {
                path: primary,
                version,
            };
        }
        match parse_persisted_config(&t) {
            Ok((cfg, migrated)) => {
                return ConfigLoadResult::Ok {
                    config: cfg,
                    migrated,
                };
            }
            Err(error) => {
                let backup_path = quarantine_corrupt_file(&primary).ok().flatten();
                return ConfigLoadResult::ParseError {
                    path: primary,
                    error,
                    backup_path,
                };
            }
        }
    }

    // 从旧路径 (CWD/workspace.json) 迁移
    let legacy = std::env::current_dir()
        .ok()
        .map(|d| d.join("workspace.json"));
    if let Some(ref legacy) = legacy
        && let Ok(t) = std::fs::read_to_string(legacy)
        && let Ok((cfg, _)) = parse_persisted_config(&t)
    {
        let _ = atomic_write_json(&primary, &cfg);
        return ConfigLoadResult::Ok {
            config: cfg,
            migrated: true,
        };
    }
    ConfigLoadResult::NotFound
}
pub(crate) fn config_path() -> PathBuf {
    // 优先使用平台配置目录，避免 CWD 变化导致配置"丢失"
    if let Some(dir) = dirs_next::config_dir() {
        let app_dir = dir.join("HardwareWorkbench");
        let _ = std::fs::create_dir_all(&app_dir);
        return app_dir.join("workspace.json");
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("workspace.json")
}

/// 主题目录内的文件以相对路径保存，便携包或安装目录改变后仍能恢复主题。
pub(crate) fn resolve_theme_path(theme_dir: &Path, stored_path: &str) -> PathBuf {
    let path = PathBuf::from(stored_path);
    if path.is_absolute() {
        path
    } else {
        theme_dir.join(path)
    }
}

pub(crate) fn windows_open_dialog() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("JSONL", &["jsonl"])
        .set_directory("logs")
        .pick_file()
}
pub(crate) fn pick_recorder_path(current: &str) -> Option<PathBuf> {
    let current_path = PathBuf::from(current);

    let mut dialog = rfd::FileDialog::new().add_filter("JSONL", &["jsonl"]);

    if let Some(parent) = current_path.parent()
        && !parent.as_os_str().is_empty()
    {
        dialog = dialog.set_directory(parent);
    } else {
        dialog = dialog.set_directory("logs");
    }

    if let Some(file_name) = current_path.file_name().and_then(|name| name.to_str()) {
        dialog = dialog.set_file_name(file_name);
    } else {
        dialog = dialog.set_file_name(format!("session-{}.jsonl", now_timestamp_ms()));
    }

    dialog.save_file().map(ensure_jsonl_extension)
}

pub(crate) fn ensure_jsonl_extension(mut path: PathBuf) -> PathBuf {
    let is_jsonl = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"));

    if !is_jsonl {
        path.set_extension("jsonl");
    }

    path
}
pub(crate) fn record_mode_label(mode: RecordModeView) -> &'static str {
    match mode {
        RecordModeView::StandardReplay => "标准回放",
        RecordModeView::RawSerial => "原始串口",
    }
}

pub(crate) fn default_recorder_path() -> String {
    user_logs_dir()
        .join(format!("session-{}.jsonl", now_timestamp_ms()))
        .display()
        .to_string()
}

/// 将旧配置中的相对录制路径解析到用户可写目录。
///
/// 旧版本保存的是 `logs/session-*.jsonl`。在 Linux `.deb` 安装中，进程
/// 可能从只读的系统目录启动，因此不能再把相对路径交给录制器。
pub(crate) fn resolve_recorder_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        user_data_dir().join(path)
    }
}

// ── WorkbenchApp 配置持久化方法 ──
// 从 commands.rs 迁入，集中配置快照/保存/加载职责。

use crate::app::WorkbenchApp;
use crate::state::MAX_SEND_HISTORY;

impl WorkbenchApp {
    /// 构建当前配置的快照
    pub(crate) fn build_config_snapshot(&self) -> PersistedConfig {
        let mut p = self.panels.clone();
        p.discard_dynamic_tabs();
        PersistedConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            panels: p,
            selected_port: self.serial.selected_port.clone(),
            baud_rate: self.serial.baud_rate.clone(),
            data_bits: self.serial.data_bits.clone(),
            stop_bits: self.serial.stop_bits.clone(),
            parity: self.serial.parity.clone(),
            recorder_path: self.recorder_path.clone(),
            enabled_plugins: self
                .workbench
                .query_plugins()
                .summaries
                .into_iter()
                .filter(|s| {
                    matches!(
                        s.state,
                        tool_application::api::extension::PluginState::Enabled
                            | tool_application::api::extension::PluginState::Running
                    )
                })
                .map(|s| s.id)
                .collect(),
            port_aliases: self.serial.port_aliases.clone(),
            port_groups: self.serial.port_groups.clone(),
            send_history: self.send.send_history.iter().cloned().collect(),
            line_ending: self.send.line_ending,
            port_profiles: self.serial.port_profiles.clone(),
            recent_workspaces: self.recent_workspaces.clone(),
            auto_reconnect: self.serial.auto_reconnect,
            keymap: self.keymap.clone(),
            monospace_font_size: self.monospace_font_size,
            ui_theme: self.ui_theme,
            theme_path: self.theme_path.as_ref().map(|path| {
                path.strip_prefix(&self.theme_dir)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            }),
            terminal_merge_window_ms: self.terminal_panel.merge_window_ms,
            terminal_max_entries: self.terminal_panel.max_entries,
            log_max_entries: self.bottom_log_panel.max_entries,
            command_usage_order: self.command_palette.usage_order.clone(),
            network_proxy_url: (!self.network_proxy_url.trim().is_empty())
                .then(|| self.network_proxy_url.trim().to_owned()),
            network_ports: self.serial.network_ports.clone(),
        }
    }

    pub(crate) fn save_config(&self) -> Result<(), String> {
        let cfg = self.build_config_snapshot();
        let path = config_path();
        atomic_write_json(&path, &cfg)
    }

    pub(crate) fn load_config_from_path(&mut self, path: &std::path::Path) -> Result<(), String> {
        let t = std::fs::read_to_string(path).map_err(|e| format!("读取失败：{e}"))?;
        let (cfg, _) = parse_persisted_config(&t)?;
        self.serial.selected_port = cfg.selected_port.clone();
        self.serial.baud_rate = cfg.baud_rate.clone();
        self.serial.data_bits = cfg.data_bits.clone();
        self.serial.stop_bits = cfg.stop_bits.clone();
        self.serial.parity = cfg.parity.clone();
        self.recorder_path = cfg.recorder_path.clone();
        self.serial.port_aliases = cfg.port_aliases.clone();
        self.serial.port_groups = cfg.port_groups.clone();
        self.serial.port_profiles = cfg.port_profiles.clone();
        self.serial.auto_reconnect = cfg.auto_reconnect;
        self.keymap = cfg.keymap.clone();
        self.monospace_font_size = cfg.monospace_font_size.clamp(10.0, 24.0);
        self.theme_path = cfg
            .theme_path
            .as_deref()
            .map(|path| resolve_theme_path(&self.theme_dir, path));
        if let Some(path) = self.theme_path.as_deref() {
            tool_panels::theme::load_theme_file(path)?;
            self.ui_theme = tool_panels::theme::builtin_theme_for_path(path)
                .unwrap_or(tool_panels::theme::AppTheme::Custom);
        } else {
            self.ui_theme = cfg.ui_theme;
        }
        self.terminal_panel.merge_window_ms = cfg.terminal_merge_window_ms;
        self.terminal_panel
            .set_max_entries(cfg.terminal_max_entries);
        self.bottom_log_panel.set_max_entries(cfg.log_max_entries);
        self.command_palette.usage_order = cfg.command_usage_order;
        self.network_proxy_url = cfg.network_proxy_url.unwrap_or_default();
        self.serial.network_ports = cfg.network_ports.clone();
        self.send.send_history = cfg
            .send_history
            .iter()
            .filter(|item| !item.trim().is_empty())
            .take(MAX_SEND_HISTORY)
            .cloned()
            .collect();
        self.send.line_ending = cfg.line_ending;
        self.panels = cfg.panels.clone();
        self.apply_loaded_workspace_postprocess();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tool_application::query::RecordModeView;

    fn legacy_workspace_json() -> String {
        serde_json::json!({
            "panels": PanelManager::default(),
            "selected_port": null,
            "baud_rate": "115200",
            "data_bits": "8",
            "stop_bits": "1",
            "parity": "none",
            "recorder_path": "logs/session.jsonl",
            "custom_theme_path": "one-dark-pro.json"
        })
        .to_string()
    }

    #[test]
    fn legacy_workspace_migrates_to_versioned_theme_path() {
        let (config, migrated) = parse_persisted_config(&legacy_workspace_json())
            .expect("legacy workspace should migrate");

        assert!(migrated);
        assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(config.theme_path.as_deref(), Some("one-dark-pro.json"));
    }

    #[test]
    fn future_workspace_schema_is_rejected() {
        let mut value: serde_json::Value =
            serde_json::from_str(&legacy_workspace_json()).expect("valid test JSON");
        value["schema_version"] = serde_json::json!(CURRENT_SCHEMA_VERSION + 1);

        assert_eq!(
            declared_schema_version(&value.to_string()),
            Some(CURRENT_SCHEMA_VERSION + 1)
        );
        assert!(parse_persisted_config(&value.to_string()).is_err());
    }

    // ── ensure_jsonl_extension ──

    #[test]
    fn ensure_jsonl_extension_no_extension_adds_jsonl() {
        let path = PathBuf::from("/tmp/session-12345");
        let result = ensure_jsonl_extension(path);
        assert_eq!(result.extension().unwrap(), "jsonl");
        assert!(result.to_string_lossy().ends_with(".jsonl"));
    }

    #[test]
    fn ensure_jsonl_extension_already_jsonl_unchanged() {
        let path = PathBuf::from("/tmp/session-12345.jsonl");
        let result = ensure_jsonl_extension(path.clone());
        assert_eq!(result, path);
    }

    #[test]
    fn ensure_jsonl_extension_case_insensitive_jsonl_unchanged() {
        let path = PathBuf::from("/tmp/session-12345.JSONL");
        let result = ensure_jsonl_extension(path.clone());
        assert_eq!(result, path);
    }

    #[test]
    fn ensure_jsonl_extension_other_extension_replaced() {
        let path = PathBuf::from("/tmp/session-12345.txt");
        let result = ensure_jsonl_extension(path);
        assert_eq!(result.extension().unwrap(), "jsonl");
    }

    // ── record_mode_label ──

    #[test]
    fn record_mode_label_all_variants_non_empty() {
        let modes = [RecordModeView::StandardReplay, RecordModeView::RawSerial];
        for mode in &modes {
            let label = record_mode_label(*mode);
            assert!(!label.is_empty(), "label for {mode:?} should not be empty");
        }
    }

    #[test]
    fn record_mode_label_standard_replay() {
        assert_eq!(
            record_mode_label(RecordModeView::StandardReplay),
            "标准回放"
        );
    }

    #[test]
    fn record_mode_label_raw_serial() {
        assert_eq!(record_mode_label(RecordModeView::RawSerial), "原始串口");
    }

    // ── default_recorder_path ──

    #[test]
    fn default_recorder_path_contains_session_and_jsonl() {
        let path = default_recorder_path();
        assert!(
            path.contains("session-"),
            "path should contain 'session-': {path}"
        );
        assert!(
            path.ends_with(".jsonl"),
            "path should end with .jsonl: {path}"
        );
    }

    // ── default_true ──

    #[test]
    fn default_true_returns_true() {
        assert!(default_true());
    }

    // ── atomic_write_json ──

    #[test]
    fn atomic_write_json_basic_write_and_read_back() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("test.json");

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Data {
            key: String,
            value: i32,
        }

        let original = Data {
            key: "hello".into(),
            value: 42,
        };

        atomic_write_json(&path, &original).expect("write should succeed");
        assert!(path.exists(), "file should exist after write");

        let read_back: Data =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read_back, original);
    }

    #[test]
    fn atomic_write_json_overwrite() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("test.json");

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Data {
            value: i32,
        }

        let first = Data { value: 1 };
        let second = Data { value: 2 };

        atomic_write_json(&path, &first).expect("first write");
        atomic_write_json(&path, &second).expect("second write");

        let read_back: Data =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read_back, second);
    }

    #[test]
    fn atomic_write_json_creates_backup() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("test.json");
        let backup_path = dir.path().join("test.json.backup");

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Data {
            value: i32,
        }

        let first = Data { value: 1 };
        let second = Data { value: 2 };

        // 第一次写入不会创建备份（因为目标文件还不存在）
        atomic_write_json(&path, &first).expect("first write");
        assert!(!backup_path.exists(), "no backup on first write");

        // 第二次写入应该创建备份
        atomic_write_json(&path, &second).expect("second write");
        assert!(backup_path.exists(), "backup should exist after overwrite");

        // 备份文件应该包含第一次写入的内容
        let backup_data: Data =
            serde_json::from_str(&std::fs::read_to_string(&backup_path).unwrap()).unwrap();
        assert_eq!(backup_data, first);
    }

    #[test]
    fn atomic_write_json_no_temp_file_left_behind() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("test.json");
        let temp_path = dir.path().join("test.tmp");

        #[derive(Serialize)]
        struct Data {
            value: i32,
        }

        atomic_write_json(&path, &Data { value: 1 }).expect("write should succeed");
        assert!(
            !temp_path.exists(),
            "temp file should not remain after write"
        );
    }
}
