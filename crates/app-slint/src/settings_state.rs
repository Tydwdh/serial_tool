use std::path::PathBuf;

pub struct SettingsState {
    pub tab: usize,
    pub theme_name: String,
    pub theme_options: Vec<String>,
    pub theme_dir: PathBuf,
    pub recorder_path: String,
    pub proxy_url: String,
    pub font_size: f32,
    pub term_merge_ms: u64,
    pub term_max: usize,
    pub log_max: usize,
    pub status: String,
}

impl SettingsState {
    pub fn from_config(cfg: &crate::config::PersistedConfig, theme_dir: PathBuf) -> Self {
        let theme_name = cfg
            .theme_path
            .as_deref()
            .and_then(|p| {
                let path = crate::config::resolve_theme_path(&theme_dir, p);
                std::fs::read_to_string(&path).ok().and_then(|s| {
                    serde_json::from_str::<serde_json::Value>(&s)
                        .ok()
                        .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(|n| n.to_owned()))
                })
            })
            .unwrap_or_else(|| "One Dark Pro".to_owned());
        let theme_options = tool_panels::theme::discover_theme_files(&theme_dir)
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        Self {
            tab: 0,
            theme_name,
            theme_options,
            theme_dir,
            recorder_path: cfg.recorder_path.clone(),
            proxy_url: cfg.network_proxy_url.clone().unwrap_or_default(),
            font_size: cfg.monospace_font_size,
            term_merge_ms: cfg.terminal_merge_window_ms,
            term_max: cfg.terminal_max_entries,
            log_max: cfg.log_max_entries,
            status: String::new(),
        }
    }

    pub fn apply_to_config(&self, cfg: &mut crate::config::PersistedConfig) {
        cfg.recorder_path = self.recorder_path.clone();
        cfg.network_proxy_url = if self.proxy_url.trim().is_empty() {
            None
        } else {
            Some(self.proxy_url.trim().to_owned())
        };
        cfg.monospace_font_size = self.font_size.clamp(10.0, 24.0);
        cfg.terminal_merge_window_ms = self.term_merge_ms;
        cfg.terminal_max_entries = self.term_max.clamp(500, 50000);
        cfg.log_max_entries = self.log_max.clamp(500, 50000);
    }
}
