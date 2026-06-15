use std::path::PathBuf;
use tool_core::now_timestamp_ms;
use tool_recorder::RecordMode;

use serde::{Deserialize, Serialize};
use tool_panels::{Activity, PanelManager};

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
    pub(crate) port_aliases: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub(crate) send_history: Vec<String>,
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
    match std::fs::read_to_string(&primary) {
        Ok(t) => match serde_json::from_str(&t) {
            Ok(cfg) => return Some(cfg),
            Err(e) => {
                eprintln!("配置解析失败 {}: {e}，尝试降级", primary.display());
            }
        },
        Err(_) => {}
    }

    // 从旧路径 (CWD/workspace.json) 迁移
    let legacy = std::env::current_dir().ok()?.join("workspace.json");
    if let Ok(t) = std::fs::read_to_string(&legacy) {
        if let Ok(cfg) = serde_json::from_str::<PersistedConfig>(&t) {
            if let Some(parent) = primary.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&legacy, &primary);
            return Some(cfg);
        }
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
