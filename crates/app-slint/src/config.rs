use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tool_core::config::{CURRENT_SCHEMA_VERSION, atomic_write_json, quarantine_corrupt_file};
use tool_panels::{PanelManager, theme::AppTheme};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PortProfile {
    pub baud_rate: String,
    pub data_bits: String,
    pub stop_bits: String,
    pub parity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedConfig {
    #[serde(default, alias = "version")]
    pub schema_version: u32,
    pub panels: PanelManager,
    pub selected_port: Option<String>,
    pub baud_rate: String,
    pub data_bits: String,
    pub stop_bits: String,
    pub parity: String,
    pub recorder_path: String,
    #[serde(default)]
    pub enabled_plugins: Vec<String>,
    #[serde(default)]
    pub port_aliases: HashMap<String, String>,
    #[serde(default)]
    pub port_groups: HashMap<String, String>,
    #[serde(default)]
    pub send_history: Vec<String>,
    #[serde(default = "default_line_ending")]
    pub line_ending: LineEnding,
    #[serde(default)]
    pub port_profiles: HashMap<String, PortProfile>,
    #[serde(default)]
    pub recent_workspaces: Vec<String>,
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,
    #[serde(default)]
    pub keymap: KeymapStub,
    #[serde(default = "default_monospace_font_size")]
    pub monospace_font_size: f32,
    #[serde(default, skip_serializing)]
    pub ui_theme: AppTheme,
    #[serde(default, alias = "custom_theme_path")]
    pub theme_path: Option<String>,
    #[serde(default = "default_terminal_merge_window_ms")]
    pub terminal_merge_window_ms: u64,
    #[serde(default = "default_terminal_max_entries")]
    pub terminal_max_entries: usize,
    #[serde(default = "default_log_max_entries")]
    pub log_max_entries: usize,
    #[serde(default)]
    pub command_usage_order: Vec<String>,
    #[serde(default)]
    pub network_proxy_url: Option<String>,
    #[serde(default)]
    pub network_ports: Vec<tool_transport::NetworkSerialConfig>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineEnding {
    None,
    Lf,
    Cr,
    Crlf,
}
impl Default for LineEnding {
    fn default() -> Self {
        Self::None
    }
}
impl LineEnding {
    pub const ALL: [Self; 4] = [Self::None, Self::Lf, Self::Cr, Self::Crlf];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::Lf => "LF",
            Self::Cr => "CR",
            Self::Crlf => "CRLF",
        }
    }
    pub fn suffix(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Lf => "\n",
            Self::Cr => "\r",
            Self::Crlf => "\r\n",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeymapStub {
    #[serde(default)]
    pub bindings: HashMap<String, Vec<KeyBindingStub>>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyBindingStub {
    pub key: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Debug)]
pub enum ConfigLoadResult {
    Ok { config: PersistedConfig, migrated: bool },
    NotFound,
    ParseError {
        path: PathBuf,
        error: String,
        backup_path: Option<PathBuf>,
    },
    FutureVersion { path: PathBuf, version: u32 },
}

fn declared_schema_version(text: &str) -> Option<u32> {
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    value
        .as_object()?
        .get("schema_version")
        .or_else(|| value.as_object()?.get("version"))
        .and_then(serde_json::Value::as_u64)
        .map(|v| v as u32)
}
fn parse_persisted_config(text: &str) -> Result<(PersistedConfig, bool), String> {
    let mut value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("JSON 解析失败：{e}"))?;
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
        if !object.contains_key("theme_path") {
            if let Some(v) = object.get("custom_theme_path").cloned() {
                object.insert("theme_path".to_owned(), v);
            }
        }
        object.remove("custom_theme_path");
    }
    object.insert(
        "schema_version".to_owned(),
        serde_json::Value::from(CURRENT_SCHEMA_VERSION),
    );
    let mut config = serde_json::from_value::<PersistedConfig>(value)
        .map_err(|e| format!("配置字段无效：{e}"))?;
    config.schema_version = CURRENT_SCHEMA_VERSION;
    Ok((config, migrated))
}

pub fn load_config() -> ConfigLoadResult {
    let primary = config_path();
    if let Ok(t) = std::fs::read_to_string(&primary) {
        if let Some(v) = declared_schema_version(&t)
            && v > CURRENT_SCHEMA_VERSION
        {
            return ConfigLoadResult::FutureVersion {
                path: primary,
                version: v,
            };
        }
        match parse_persisted_config(&t) {
            Ok((cfg, migrated)) => {
                return ConfigLoadResult::Ok {
                    config: cfg,
                    migrated,
                }
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
    // 兼容旧 CWD 路径
    let legacy = std::env::current_dir().ok().map(|d| d.join("workspace.json"));
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

pub fn config_path() -> PathBuf {
    if let Some(dir) = dirs_next::config_dir() {
        let app_dir = dir.join("HardwareWorkbench");
        let _ = std::fs::create_dir_all(&app_dir);
        return app_dir.join("workspace.json");
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("workspace.json")
}

pub fn resolve_theme_path(theme_dir: &Path, stored: &str) -> PathBuf {
    let p = PathBuf::from(stored);
    if p.is_absolute() {
        p
    } else {
        theme_dir.join(p)
    }
}

pub fn theme_dir() -> PathBuf {
    if let Some(dir) = dirs_next::config_dir() {
        dir.join("HardwareWorkbench").join("themes")
    } else {
        PathBuf::from("themes")
    }
}

pub fn save_config_snapshot(cfg: &PersistedConfig) -> Result<(), String> {
    atomic_write_json(&config_path(), cfg)
}

pub fn ensure_jsonl_extension(mut path: PathBuf) -> PathBuf {
    let is_jsonl = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("jsonl"));
    if !is_jsonl {
        path.set_extension("jsonl");
    }
    path
}

pub fn default_recorder_path() -> String {
    format!("logs/session-{}.jsonl", tool_core::now_timestamp_ms())
}
