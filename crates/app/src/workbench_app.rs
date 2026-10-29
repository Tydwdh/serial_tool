//! UI composition state for the native root.
//!
//! The runtime services and presentation state live in one type; panel modules
//! receive the pieces they render rather than owning runtime state.

use crate::panel_registry::PanelRegistry;
use crate::state::{NotificationQueue, SendUiState, SerialUiState, UpdateState};
use tool_application::Workbench;
use tool_panels::{DynamicPanels, LogPanel, PanelManager, PluginsPanel, TerminalPanel, theme};

pub(crate) struct WorkbenchApp {
    pub(crate) panels: PanelManager,
    pub(crate) terminal_panel: TerminalPanel,
    pub(crate) bottom_log_panel: LogPanel,
    pub(crate) panel_registry: PanelRegistry,
    pub(crate) layout_dirty: bool,
    pub(crate) ui_theme: theme::AppTheme,

    pub(crate) workbench: Workbench,
    pub(crate) dynamic_panels: DynamicPanels,
    pub(crate) plugins_panel: PluginsPanel,
    pub(crate) serial: SerialUiState,
    pub(crate) notifications: NotificationQueue,
    pub(crate) toast_overlay: crate::ui::toast::ToastOverlay,
    pub(crate) send: SendUiState,
    pub(crate) last_auto_save_time: f64,
    pub(crate) ui_events: tool_application::UiEventSubscriptions,
    pub(crate) periodic_send: crate::runtime::periodic_send::PeriodicSendState,
    pub(crate) update_state: UpdateState,
    pub(crate) contribution_states: std::collections::HashMap<String, serde_json::Value>,
    pub(crate) plugin_summaries_cache:
        std::cell::OnceCell<Vec<tool_application::query::PluginSummaryView>>,
    pub(crate) monospace_font_size: f32,
    pub(crate) theme_path: Option<std::path::PathBuf>,
    pub(crate) theme_dir: std::path::PathBuf,
    pub(crate) network_proxy_url: String,
    pub(crate) marketplace: crate::runtime::marketplace::MarketplaceState,
    pub(crate) perf: crate::perf::PerfDiagnostics,
    pub(crate) native_export: Option<crate::commands::NativeExportState>,
}
