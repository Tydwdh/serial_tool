use crate::state::LineEnding;
use std::path::PathBuf;
use tool_core::now_timestamp_ms;
use tool_recorder::RecordMode;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tool_panels::PanelManager;

/// 原子写入 JSON 文件：先写临时文件，再 rename 替换目标文件。
/// 崩溃时不会留下半写的目标文件。旧文件会被备份到 `.backup`。
fn atomic_write_json<T: Serialize>(path: &std::path::Path, value: &T) -> Result<(), String> {
    let temp_path = path.with_extension("tmp");
    let backup_path = path.with_extension("json.backup");

    // 1. 序列化到内存
    let data = serde_json::to_string_pretty(value).map_err(|e| format!("序列化失败：{e}"))?;

    // 2. 写入临时文件（同目录，保证 rename 是原子操作）
    std::fs::write(&temp_path, data).map_err(|e| format!("写入临时文件失败：{e}"))?;

    // 3. 备份旧文件（如果存在）
    if path.exists()
        && let Err(e) = std::fs::copy(path, &backup_path)
    {
        log::warn!("config: failed to backup to {}: {e}", backup_path.display());
    }

    // 4. 原子替换
    std::fs::rename(&temp_path, path).map_err(|e| format!("原子替换失败：{e}"))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PortProfile {
    pub(crate) baud_rate: String,
    pub(crate) data_bits: String,
    pub(crate) stop_bits: String,
    pub(crate) parity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedConfig {
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
    pub(crate) terminal_popup_always_on_top: bool,
    #[serde(default)]
    pub(crate) send_popup_always_on_top: bool,
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
    /// 终端合并阈值（ms），同端口同方向间隔 ≤ 此值的连续包合并显示。默认 5。
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
    Ok(PersistedConfig),
    /// 配置文件不存在（首次运行或配置被删除）
    NotFound,
    /// 配置文件存在但解析失败（配置损坏）
    ParseError { path: PathBuf, error: String },
}

pub(crate) fn load_config() -> ConfigLoadResult {
    let primary = config_path();

    // 尝试读主路径
    if let Ok(t) = std::fs::read_to_string(&primary) {
        match serde_json::from_str(&t) {
            Ok(cfg) => return ConfigLoadResult::Ok(cfg),
            Err(e) => {
                return ConfigLoadResult::ParseError {
                    path: primary,
                    error: e.to_string(),
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
        && let Ok(cfg) = serde_json::from_str::<PersistedConfig>(&t)
    {
        if let Some(parent) = primary.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::copy(legacy, &primary);
        return ConfigLoadResult::Ok(cfg);
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
pub(crate) fn record_mode_label(mode: RecordMode) -> &'static str {
    match mode {
        RecordMode::StandardReplay => "标准回放",
        RecordMode::RawSerial => "原始串口",
    }
}

pub(crate) fn default_recorder_path() -> String {
    format!("logs/session-{}.jsonl", now_timestamp_ms())
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
            panels: p,
            selected_port: self.serial.selected_port.clone(),
            baud_rate: self.serial.baud_rate.clone(),
            data_bits: self.serial.data_bits.clone(),
            stop_bits: self.serial.stop_bits.clone(),
            parity: self.serial.parity.clone(),
            recorder_path: self.recorder_path.clone(),
            enabled_plugins: self
                .plugin_manager
                .summaries()
                .into_iter()
                .filter(|s| {
                    matches!(
                        s.state,
                        tool_extension::PluginState::Enabled | tool_extension::PluginState::Running
                    )
                })
                .map(|s| s.id)
                .collect(),
            terminal_popup_always_on_top: self.popups.terminal_always_on_top,
            send_popup_always_on_top: self.popups.send_always_on_top,
            port_aliases: self.serial.port_aliases.clone(),
            port_groups: self.serial.port_groups.clone(),
            send_history: self.send.send_history.iter().cloned().collect(),
            line_ending: self.send.line_ending,
            port_profiles: self.serial.port_profiles.clone(),
            recent_workspaces: self.recent_workspaces.clone(),
            auto_reconnect: self.serial.auto_reconnect,
            keymap: self.keymap.clone(),
            monospace_font_size: self.monospace_font_size,
            terminal_merge_window_ms: self.terminal_panel.merge_window_ms,
            terminal_max_entries: self.terminal_panel.max_entries,
            log_max_entries: self.bottom_log_panel.max_entries,
            command_usage_order: self.command_palette.usage_order.clone(),
        }
    }

    pub(crate) fn save_config(&self) -> Result<(), String> {
        let cfg = self.build_config_snapshot();
        let path = config_path();
        atomic_write_json(&path, &cfg)
    }

    pub(crate) fn load_config_from_path(&mut self, path: &std::path::Path) -> Result<(), String> {
        let t = std::fs::read_to_string(path).map_err(|e| format!("读取失败：{e}"))?;
        let cfg: PersistedConfig =
            serde_json::from_str(&t).map_err(|e| format!("解析失败：{e}"))?;
        self.serial.selected_port = cfg.selected_port.clone();
        self.serial.baud_rate = cfg.baud_rate.clone();
        self.serial.data_bits = cfg.data_bits.clone();
        self.serial.stop_bits = cfg.stop_bits.clone();
        self.serial.parity = cfg.parity.clone();
        self.recorder_path = cfg.recorder_path.clone();
        self.popups.terminal_always_on_top = cfg.terminal_popup_always_on_top;
        self.popups.send_always_on_top = cfg.send_popup_always_on_top;
        self.serial.port_aliases = cfg.port_aliases.clone();
        self.serial.port_groups = cfg.port_groups.clone();
        self.serial.port_profiles = cfg.port_profiles.clone();
        self.serial.auto_reconnect = cfg.auto_reconnect;
        self.keymap = cfg.keymap.clone();
        self.monospace_font_size = cfg.monospace_font_size.clamp(10.0, 24.0);
        self.terminal_panel.merge_window_ms = cfg.terminal_merge_window_ms;
        self.terminal_panel.max_entries = cfg.terminal_max_entries.max(100);
        self.bottom_log_panel.max_entries = cfg.log_max_entries.max(100);
        self.command_palette.usage_order = cfg.command_usage_order;
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
    use tool_recorder::RecordMode;

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
        let modes = [RecordMode::StandardReplay, RecordMode::RawSerial];
        for mode in &modes {
            let label = record_mode_label(*mode);
            assert!(!label.is_empty(), "label for {mode:?} should not be empty");
        }
    }

    #[test]
    fn record_mode_label_standard_replay() {
        assert_eq!(record_mode_label(RecordMode::StandardReplay), "标准回放");
    }

    #[test]
    fn record_mode_label_raw_serial() {
        assert_eq!(record_mode_label(RecordMode::RawSerial), "原始串口");
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
