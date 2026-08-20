use std::{path::PathBuf, sync::Arc};

use parking_lot::Mutex;
use crate::{
    config::{ConfigLoadResult, PersistedConfig, load_config, resolve_theme_path, save_config_snapshot},
    state::{NotificationQueue, StatusLevel},
};

pub struct AppState {
    pub config: PersistedConfig,
    pub config_migrated: bool,
    pub theme_dir: PathBuf,
    pub notifications: Arc<Mutex<NotificationQueue>>,
    pub bus: tool_databus::DataBus,
    pub transport: tool_transport::TransportManager,
}

impl AppState {
    pub fn load() -> Self {
        let theme_dir = crate::config::theme_dir();
        let _ = tool_panels::theme::ensure_theme_directory(&theme_dir);
        let (config, migrated) = match load_config() {
            ConfigLoadResult::Ok { config, migrated } => (config, migrated),
            ConfigLoadResult::NotFound => (
                default_config(&theme_dir),
                false,
            ),
            ConfigLoadResult::ParseError { error, backup_path, .. } => {
                log::warn!("config parse error: {error} backup={backup_path:?}");
                (default_config(&theme_dir), false)
            }
            ConfigLoadResult::FutureVersion { version, .. } => {
                log::warn!("future config version {version}, using defaults");
                (default_config(&theme_dir), false)
            }
        };
        let bus = tool_databus::DataBus::new();
        let transport = tool_transport::TransportManager::new(bus.clone());
        Self {
            config,
            config_migrated: migrated,
            theme_dir,
            notifications: Arc::new(Mutex::new(NotificationQueue::new())),
            bus,
            transport,
        }
    }

    pub fn theme_path(&self) -> Option<PathBuf> {
        self.config
            .theme_path
            .as_deref()
            .map(|s| resolve_theme_path(&self.theme_dir, s))
    }

    pub fn status_text(&self) -> String {
        let mut q = self.notifications.lock();
        let cur = q.current();
        if cur.is_empty() {
            "就绪".to_owned()
        } else {
            cur.last().map(|n| n.text.clone()).unwrap_or_default()
        }
    }

    pub fn status_level(&self) -> StatusLevel {
        let mut q = self.notifications.lock();
        let cur = q.current();
        cur.last().map(|n| n.level).unwrap_or(StatusLevel::Info)
    }

    pub fn push_status(&self, level: StatusLevel, text: impl Into<String>) {
        self.notifications.lock().push("general", level, text);
    }

    pub fn build_snapshot(&self) -> PersistedConfig {
        // Slint 不承载复杂 panels 布局，沿用现有 PersistedConfig 并剔除 egui 动态 tab
        let mut panels = self.config.panels.clone();
        panels.discard_dynamic_tabs();
        let mut cfg = self.config.clone();
        cfg.panels = panels;
        cfg.schema_version = tool_core::config::CURRENT_SCHEMA_VERSION;
        cfg
    }

    pub fn save(&self) -> Result<(), String> {
        let snap = self.build_snapshot();
        save_config_snapshot(&snap)
    }
}

fn default_config(theme_dir: &std::path::Path) -> PersistedConfig {
    let panels = tool_panels::PanelManager::default();
    let theme_path = tool_panels::theme::builtin_theme_path(
        tool_panels::theme::AppTheme::OneDarkPro,
        theme_dir,
    )
    .map(|p| {
        p.strip_prefix(theme_dir)
            .unwrap_or(&p)
            .display()
            .to_string()
    });
    PersistedConfig {
        schema_version: tool_core::config::CURRENT_SCHEMA_VERSION,
        panels,
        selected_port: None,
        baud_rate: "115200".to_owned(),
        data_bits: "8".to_owned(),
        stop_bits: "1".to_owned(),
        parity: "none".to_owned(),
        recorder_path: crate::config::default_recorder_path(),
        enabled_plugins: Vec::new(),
        port_aliases: Default::default(),
        port_groups: Default::default(),
        send_history: Vec::new(),
        line_ending: crate::config::LineEnding::None,
        port_profiles: Default::default(),
        recent_workspaces: Vec::new(),
        auto_reconnect: true,
        keymap: Default::default(),
        monospace_font_size: 13.0,
        ui_theme: tool_panels::theme::AppTheme::OneDarkPro,
        theme_path,
        terminal_merge_window_ms: 5,
        terminal_max_entries: 50_000,
        log_max_entries: 50_000,
        command_usage_order: Vec::new(),
        network_proxy_url: None,
        network_ports: Vec::new(),
    }
}

/// 供 Slint status 轮询：返回 (text, level_str)
pub fn poll_status_text(app_state: &AppState) -> (String, String) {
    let mut q = app_state.notifications.lock();
    let cur = q.current();
    if let Some(n) = cur.last() {
        (n.text.clone(), n.level.label().to_owned())
    } else {
        ("就绪".to_owned(), "info".to_owned())
    }
}

pub fn available_themes(theme_dir: &std::path::Path) -> Vec<(PathBuf, String)> {
    tool_panels::theme::discover_theme_files(theme_dir)
}
