//! Shared UI composition state for the Native and Web roots.
//!
//! The runtime services are target-specific, but the panel host, layout and
//! presentation state live in one type. This prevents the browser from
//! growing a second, subtly different application UI.

use crate::panel_registry::PanelRegistry;
use tool_panels::{
    ChartPanel, LogPanel, PanelManager, PluginsPanel, ReplayPanel, TerminalPanel, theme,
};

#[cfg(not(target_arch = "wasm32"))]
use crate::state::{NotificationQueue, SendUiState, SerialUiState, UpdateState};
#[cfg(not(target_arch = "wasm32"))]
use tool_application::Workbench;
#[cfg(not(target_arch = "wasm32"))]
use tool_panels::DynamicPanels;

#[cfg(target_arch = "wasm32")]
use crate::web::{WebSerialState, WebSettings};
#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;
#[cfg(target_arch = "wasm32")]
use tool_application::web::WebRuntime;
#[cfg(target_arch = "wasm32")]
use tool_platform::storage::web::WebSettingsStore;

pub(crate) struct WorkbenchApp {
    pub(crate) panels: PanelManager,
    pub(crate) terminal_panel: TerminalPanel,
    pub(crate) chart_panel: ChartPanel,
    pub(crate) bottom_log_panel: LogPanel,
    pub(crate) panel_registry: PanelRegistry,
    pub(crate) layout_dirty: bool,
    pub(crate) ui_theme: theme::AppTheme,

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) workbench: Workbench,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) dynamic_panels: DynamicPanels,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) plugins_panel: PluginsPanel,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) replay_panel: ReplayPanel,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) serial: SerialUiState,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) recorder_path: String,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) notifications: NotificationQueue,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) toast_overlay: crate::ui::toast::ToastOverlay,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) recent_workspaces: Vec<String>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) send: SendUiState,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) last_auto_save_time: f64,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) ui_events: tool_application::UiEventSubscriptions,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) replay_analyzer: crate::replay_task::ReplayAnalyzerState,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) periodic_send: crate::runtime::periodic_send::PeriodicSendState,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) keymap: crate::keymap::Keymap,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) commands: crate::command_registry::CommandRegistry,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) pending_command: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) key_recording: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) command_palette: crate::ui::command_palette::CommandPaletteState,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) update_state: UpdateState,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) contribution_states: std::collections::HashMap<String, serde_json::Value>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) plugin_summaries_cache:
        std::cell::OnceCell<Vec<tool_application::query::PluginSummaryView>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) monospace_font_size: f32,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) theme_path: Option<std::path::PathBuf>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) theme_dir: std::path::PathBuf,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) network_proxy_url: String,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) marketplace: crate::runtime::marketplace::MarketplaceState,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) perf: crate::perf::PerfDiagnostics,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) native_export: Option<crate::commands::NativeExportState>,

    #[cfg(target_arch = "wasm32")]
    pub(crate) theme_source: Option<String>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) runtime: Option<WebRuntime>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) dynamic_panels: tool_panels::DynamicPanels,
    #[cfg(target_arch = "wasm32")]
    pub(crate) serial: Rc<RefCell<WebSerialState>>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) settings_store: Option<WebSettingsStore>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) settings_load: Rc<RefCell<Option<WebSettings>>>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) recording_file_name: String,
    #[cfg(target_arch = "wasm32")]
    pub(crate) recording_mode: crate::web::WebRecordMode,
    #[cfg(target_arch = "wasm32")]
    pub(crate) replay_panel: ReplayPanel,
    #[cfg(target_arch = "wasm32")]
    pub(crate) replay_analyzer: crate::web::WebReplayAnalyzerState,
    #[cfg(target_arch = "wasm32")]
    pub(crate) plugins: crate::web::WebPluginState,
    #[cfg(target_arch = "wasm32")]
    pub(crate) plugin_data: crate::web_plugin_host::WebPluginDataStore,
    #[cfg(target_arch = "wasm32")]
    pub(crate) web_lua: tool_plugin_runtime::WebLuaEngine,
    #[cfg(target_arch = "wasm32")]
    pub(crate) plugins_panel: PluginsPanel,
    #[cfg(target_arch = "wasm32")]
    pub(crate) marketplace_url: String,
    #[cfg(target_arch = "wasm32")]
    pub(crate) keymap: crate::shared_keymap::Keymap,
    #[cfg(target_arch = "wasm32")]
    pub(crate) key_recording: Option<String>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) command_palette_open: bool,
    #[cfg(target_arch = "wasm32")]
    pub(crate) command_palette_query: String,
    #[cfg(target_arch = "wasm32")]
    pub(crate) command_palette_selected: Option<usize>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) command_usage_order: Vec<String>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) web_export: Option<crate::web::WebExportState>,
    #[cfg(target_arch = "wasm32")]
    pub(crate) perf: crate::web_perf::WebPerfDiagnostics,
    #[cfg(target_arch = "wasm32")]
    pub(crate) web_notifications: Vec<crate::web::WebNotification>,
}
