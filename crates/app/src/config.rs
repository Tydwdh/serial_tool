use std::path::PathBuf;
use tool_core::now_timestamp_ms;
use tool_recorder::RecordMode;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tool_panels::{Activity, PanelManager};

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
    if path.exists() {
        if let Err(e) = std::fs::copy(path, &backup_path) {
            log::warn!("config: failed to backup to {}: {e}", backup_path.display());
        }
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
    pub(crate) timeout_ms: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedConfig {
    pub(crate) panels: PanelManager,
    pub(crate) selected_port: Option<String>,
    pub(crate) baud_rate: String,
    pub(crate) data_bits: String,
    pub(crate) stop_bits: String,
    pub(crate) parity: String,
    pub(crate) timeout_ms: String,
    pub(crate) recorder_path: String,
    #[serde(default = "default_activity_order")]
    pub(crate) activity_order: Vec<Activity>,
    #[serde(default)]
    pub(crate) enabled_plugins: Vec<String>,
    #[serde(default)]
    pub(crate) terminal_popup_always_on_top: bool,
    #[serde(default)]
    pub(crate) send_popup_always_on_top: bool,
    #[serde(default)]
    pub(crate) port_aliases: HashMap<String, String>,
    #[serde(default)]
    pub(crate) send_history: Vec<String>,
    #[serde(default)]
    pub(crate) port_profiles: HashMap<String, PortProfile>,
    #[serde(default)]
    pub(crate) recent_workspaces: Vec<String>,
    #[serde(default = "default_true")]
    pub(crate) auto_reconnect: bool,
}

fn default_true() -> bool {
    true
}

pub(crate) fn default_activity_order() -> Vec<Activity> {
    vec![
        Activity::Devices,
        Activity::Replay,
        Activity::Plugins,
        Activity::Settings,
    ]
}

pub(crate) fn load_config() -> Option<PersistedConfig> {
    let primary = config_path();

    // 尝试读主路径
    if let Ok(t) = std::fs::read_to_string(&primary) {
        match serde_json::from_str(&t) {
            Ok(cfg) => return Some(cfg),
            Err(e) => {
                eprintln!("配置解析失败 {}: {e}，尝试降级", primary.display());
            }
        }
    }

    // 从旧路径 (CWD/workspace.json) 迁移
    let legacy = std::env::current_dir().ok()?.join("workspace.json");
    if let Ok(t) = std::fs::read_to_string(&legacy)
        && let Ok(cfg) = serde_json::from_str::<PersistedConfig>(&t)
    {
        if let Some(parent) = primary.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::copy(&legacy, &primary);
        return Some(cfg);
    }
    None
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
pub(crate) fn pick_workspace_open_path() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Workspace", &["json"])
        .pick_file()
}

pub(crate) fn pick_workspace_save_path() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("Workspace", &["json"])
        .set_file_name("workspace.json")
        .save_file()
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
        RecordMode::FullDebug => "完整调试",
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
            timeout_ms: self.serial.timeout_ms.clone(),
            recorder_path: self.recorder_path.clone(),
            activity_order: self.activity_order.clone(),
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
            terminal_popup_always_on_top: self.terminal_popup_always_on_top,
            send_popup_always_on_top: self.send_popup_always_on_top,
            port_aliases: self.serial.port_aliases.clone(),
            send_history: self.send.send_history.iter().cloned().collect(),
            port_profiles: self.serial.port_profiles.clone(),
            recent_workspaces: self.recent_workspaces.clone(),
            auto_reconnect: self.serial.auto_reconnect,
        }
    }

    pub(crate) fn save_config(&self) -> Result<(), String> {
        let cfg = self.build_config_snapshot();
        let path = config_path();
        atomic_write_json(&path, &cfg)
    }

    pub(crate) fn save_config_to_path(&self, path: &std::path::Path) -> Result<(), String> {
        let cfg = self.build_config_snapshot();
        atomic_write_json(path, &cfg)
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
        self.serial.timeout_ms = cfg.timeout_ms.clone();
        self.recorder_path = cfg.recorder_path.clone();
        self.activity_order = cfg.activity_order.clone();
        self.terminal_popup_always_on_top = cfg.terminal_popup_always_on_top;
        self.send_popup_always_on_top = cfg.send_popup_always_on_top;
        self.serial.port_aliases = cfg.port_aliases.clone();
        self.serial.port_profiles = cfg.port_profiles.clone();
        self.serial.auto_reconnect = cfg.auto_reconnect;
        self.send.send_history = cfg
            .send_history
            .iter()
            .filter(|item| !item.trim().is_empty())
            .take(MAX_SEND_HISTORY)
            .cloned()
            .collect();
        self.panels = cfg.panels.clone();
        self.apply_loaded_workspace_postprocess();
        Ok(())
    }
}
