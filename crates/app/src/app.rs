use crate::config::default_activity_order;
use crate::config::{default_recorder_path, load_config};
use crate::state::{BottomTab, MAX_SEND_HISTORY, SendUiState, StatusState};
use eframe::egui;
use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use tool_core::{Event, LogLevel};
use tool_databus::DataBus;
use tool_extension::PluginManager;
use tool_lua_host::{DialogRequest, FileAccessBroker};
use tool_panels::{
    Activity, DynamicPanels, LogPanel, PanelManager, PluginsPanel, ReplayPanel, TerminalPanel,
    theme,
};
use tool_recorder::JsonlRecorder;
use tool_transport::{SerialPortDescriptor, TransportManager};

use crate::bootstrap::{REPAINT_INTERVAL_MS, app_dir, apply_theme, setup_fonts};

// ── 数据结构 ──

pub(crate) struct WorkbenchApp {
    pub(crate) bus: DataBus,
    pub(crate) transport: TransportManager,
    pub(crate) plugin_manager: PluginManager,
    pub(crate) recorder: JsonlRecorder,
    pub(crate) panels: PanelManager,
    pub(crate) terminal_panel: TerminalPanel,
    pub(crate) dynamic_panels: DynamicPanels,
    pub(crate) plugins_panel: PluginsPanel,
    pub(crate) replay_panel: ReplayPanel,
    pub(crate) bottom_log_panel: LogPanel,
    pub(crate) ports: Vec<SerialPortDescriptor>,
    pub(crate) selected_port: Option<String>,
    pub(crate) baud_rate: String,
    pub(crate) data_bits: String,
    pub(crate) stop_bits: String,
    pub(crate) parity: String,
    pub(crate) timeout_ms: String,
    pub(crate) recorder_path: String,
    pub(crate) status: StatusState,
    pub(crate) last_port_refresh: f64,
    pub(crate) auto_reconnect: bool,
    pub(crate) pending_reconnect: Option<PendingReconnect>,
    pub(crate) port_aliases: std::collections::HashMap<String, String>,
    pub(crate) port_profiles: std::collections::HashMap<String, crate::config::PortProfile>,
    pub(crate) recent_workspaces: Vec<String>,
    pub(crate) bottom_panel_visible: bool,
    pub(crate) bottom_tab: BottomTab,
    pub(crate) send: SendUiState,
    pub(crate) terminal_popup_open: bool,
    pub(crate) terminal_popup_always_on_top: bool,
    pub(crate) send_popup_always_on_top: bool,
    pub(crate) detached_dynamic_panels: BTreeSet<String>,
    pub(crate) top_bar_serial_collapsed: bool,
    pub(crate) activity_order: Vec<Activity>,
    pub(crate) activity_drag_source: Option<usize>,
    pub(crate) activity_rects_cache: Vec<egui::Rect>,
    pub(crate) dock_dragging_panel: Option<tool_panels::PanelKind>,
    pub(crate) last_auto_save_time: f64,
    pub(crate) last_rate_check_time: f64,
    pub(crate) last_event_count: u64,
    pub(crate) event_rate: f64,
    pub(crate) dynamic_drag_source: Option<usize>,
    pub(crate) file_broker: Arc<FileAccessBroker>,
    pub(crate) dialog_receiver: crossbeam_channel::Receiver<DialogRequest>,
    pub(crate) file_browse_subscription: tool_databus::Subscription,
    pub(crate) replay_analyzer_job: Option<ReplayAnalyzerJob>,
    pub(crate) replay_analyzer_generation: u64,
    pub(crate) bottom_dock_rect: Option<egui::Rect>,
    pub(crate) right_dock_rect: Option<egui::Rect>,
}

pub(crate) struct ReplayAnalyzerJob {
    pub(crate) generation: u64,
    pub(crate) source_path: String,
    pub(crate) cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) handle: std::thread::JoinHandle<ReplayAnalyzerResult>,
}

pub(crate) struct ReplayAnalyzerResult {
    pub(crate) total: usize,
    pub(crate) succeeded: usize,
    pub(crate) failed: usize,
    pub(crate) derived_events: Vec<Event>,
    pub(crate) errors: Vec<String>,
    pub(crate) logs: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct PendingReconnect {
    pub(crate) port_name: String,
    pub(crate) config: tool_transport::SerialConfig,
    pub(crate) attempts: u32,
    pub(crate) next_try_at: f64,
}

// ══════════════════════════════════════════
//  WorkbenchApp impl
// ══════════════════════════════════════════

impl WorkbenchApp {
    pub(crate) fn port_label(&self, port: &str) -> String {
        match self.port_aliases.get(port).filter(|s| !s.trim().is_empty()) {
            Some(alias) => format!("{alias} ({port})"),
            None => port.to_owned(),
        }
    }

    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(cc);
        cc.egui_ctx.set_embed_viewports(false);
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());

        let (dialog_sender, dialog_receiver) = crossbeam_channel::unbounded::<DialogRequest>();
        let file_broker = Arc::new(FileAccessBroker::default());

        let mut pm = PluginManager::new(bus.clone(), transport.clone());
        pm.set_host_services(dialog_sender, file_broker.clone());

        let plugin_dir = app_dir().join("plugins");
        if let Err(e) = pm.discover_roots([plugin_dir, PathBuf::from("plugins")]) {
            bus.publish(Event::system_log(
                LogLevel::Error,
                "ext",
                format!("plugin discover: {e}"),
            ));
        }
        let recorder = JsonlRecorder::new(bus.clone());
        let config = load_config();
        if config.is_none() {
            bus.publish(Event::system_log(
                LogLevel::Warn,
                "app",
                "未找到或无法加载配置，使用默认设置",
            ));
        }
        apply_theme(&cc.egui_ctx);
        let mut rp = config
            .as_ref()
            .map(|c| c.panels.clone())
            .unwrap_or_default();
        rp.discard_dynamic_tabs();
        rp.dock.normalize_tool_layout();
        let mut send = SendUiState::default();
        if let Some(cfg) = config.as_ref() {
            send.send_history = cfg
                .send_history
                .iter()
                .filter(|item| !item.trim().is_empty())
                .take(MAX_SEND_HISTORY)
                .cloned()
                .collect::<VecDeque<_>>();
        }

        let mut app = Self {
            terminal_panel: TerminalPanel::new(&bus),
            dynamic_panels: DynamicPanels::new(&bus),
            plugins_panel: PluginsPanel::new(),
            replay_panel: ReplayPanel::new(&bus),
            bottom_log_panel: LogPanel::new(&bus),
            ports: Vec::new(),
            selected_port: config.as_ref().and_then(|c| c.selected_port.clone()),
            baud_rate: config
                .as_ref()
                .map(|c| c.baud_rate.clone())
                .unwrap_or_else(|| "115200".into()),
            data_bits: config
                .as_ref()
                .map(|c| c.data_bits.clone())
                .unwrap_or_else(|| "8".into()),
            stop_bits: config
                .as_ref()
                .map(|c| c.stop_bits.clone())
                .unwrap_or_else(|| "1".into()),
            parity: config
                .as_ref()
                .map(|c| c.parity.clone())
                .unwrap_or_else(|| "none".into()),
            timeout_ms: config
                .as_ref()
                .map(|c| c.timeout_ms.clone())
                .unwrap_or_else(|| "50".into()),
            recorder_path: config
                .as_ref()
                .map(|c| c.recorder_path.clone())
                .unwrap_or_else(default_recorder_path),
            panels: rp.clone(),
            status: StatusState::default(),
            last_port_refresh: 0.0,
            auto_reconnect: config
                .as_ref()
                .map(|c| c.auto_reconnect)
                .unwrap_or(true),
            pending_reconnect: None,
            port_aliases: config
                .as_ref()
                .map(|c| c.port_aliases.clone())
                .unwrap_or_default(),
            port_profiles: config
                .as_ref()
                .map(|c| c.port_profiles.clone())
                .unwrap_or_default(),
            recent_workspaces: config
                .as_ref()
                .map(|c| c.recent_workspaces.clone())
                .unwrap_or_default(),
            bottom_panel_visible: rp.bottom_logs_visible,
            bottom_tab: BottomTab::Terminal,
            send,
            terminal_popup_open: false,
            terminal_popup_always_on_top: config
                .as_ref()
                .map(|c| c.terminal_popup_always_on_top)
                .unwrap_or(false),
            send_popup_always_on_top: config
                .as_ref()
                .map(|c| c.send_popup_always_on_top)
                .unwrap_or(false),
            detached_dynamic_panels: BTreeSet::new(),
            top_bar_serial_collapsed: false,
            activity_order: config
                .as_ref()
                .map(|c| c.activity_order.clone())
                .unwrap_or_else(default_activity_order),
            activity_drag_source: None,
            activity_rects_cache: Vec::new(),
            last_auto_save_time: 0.0,
            last_rate_check_time: 0.0,
            last_event_count: 0,
            event_rate: 0.0,
            bus: bus.clone(),
            transport,
            plugin_manager: pm,
            recorder,
            dynamic_drag_source: None,
            dock_dragging_panel: None,
            file_broker,
            dialog_receiver,
            file_browse_subscription: bus.subscribe(tool_databus::TopicFilter::exact(
                tool_core::topics::UI_FORM_FILE_BROWSE,
            )),
            replay_analyzer_job: None,
            replay_analyzer_generation: 0,
            bottom_dock_rect: None,
            right_dock_rect: None,
        };
        app.refresh_ports();
        let enabled: Vec<String> = config
            .as_ref()
            .map(|c| c.enabled_plugins.clone())
            .unwrap_or_default();
        for id in &enabled {
            if let Err(e) = app.plugin_manager.enable(id) {
                app.log(LogLevel::Warn, format!("restore plugin {id}: {e}"));
            }
        }
        app.log(LogLevel::Info, "就绪");
        app
    }

    pub(crate) fn log(&self, lv: LogLevel, m: impl Into<String>) {
        self.bus.publish(Event::system_log(lv, "app", m.into()));
    }
}

impl Drop for WorkbenchApp {
    fn drop(&mut self) {
        // 退出前自动保存工作区
        let _ = self.save_config();
        self.recorder.stop();
        self.transport.close_serial();
    }
}

// ── UI 组件 ──

impl eframe::App for WorkbenchApp {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        theme::BG_PRIMARY.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.tick_pre_ui(&ctx);
        self.draw_shell(ui, &ctx);
        self.tick_post_ui(&ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(REPAINT_INTERVAL_MS));
    }
}
