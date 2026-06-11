use crate::config::{PersistedConfig, default_activity_order};
use crate::config::{
    config_path, default_recorder_path, ensure_jsonl_extension, load_config, pick_recorder_path,
    record_mode_label, windows_open_dialog,
};
use crate::state::{BottomTab, DetachedPanelAction, SendUiState, StatusLevel, StatusState};
use crate::ui::activity_bar::{
    activity_insert_index_from_pointer, aicon, ashortcut, paint_activity_insert_line,
    paint_vertical_insert_line, vertical_insert_index_from_pointer,
};
use crate::ui::bottom_panel::{send_impl_to, translate_error};
use crate::ui::top_bar::{pdb, ppar, psb};
use eframe::egui;
use egui::Color32;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use tool_core::{Direction, Event, LogLevel, Payload, now_timestamp_ms, topic_matches, topics};
use tool_databus::DataBus;
use tool_extension::PluginManager;
use tool_lua_host::{DialogRequest, FileAccessBroker, LuaReplayConfig, run_replay_analyzer};
use tool_panels::{
    Activity, DynamicPanels, LogPanel, PanelKind, PanelManager, PluginsPanel, ReplayPanel,
    TerminalPanel, theme,
};
use tool_recorder::{JsonlRecorder, RecordMode};
use tool_transport::{
    DataBits, Parity, SerialConfig, SerialPortDescriptor, StopBits, TransportManager,
};

use crate::bootstrap::{
    ACTIVITY_BAR_WIDTH, BOTTOM_PANEL_HEIGHT, BOTTOM_PANEL_MIN, DEFAULT_WINDOW_HEIGHT,
    DEFAULT_WINDOW_WIDTH, INSPECTOR_WIDTH, REPAINT_INTERVAL_MS, app_dir, apply_theme, setup_fonts,
};
use crate::ui::top_bar::{
    baud_combo, serial_action_button, serial_action_button_enabled, serial_combo,
};

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
    pub(crate) bottom_panel_visible: bool,
    pub(crate) bottom_tab: BottomTab,
    pub(crate) send: SendUiState,
    pub(crate) terminal_popup_open: bool,
    pub(crate) detached_dynamic_panels: BTreeSet<String>,
    pub(crate) top_bar_serial_collapsed: bool,
    pub(crate) activity_order: Vec<Activity>,
    pub(crate) activity_drag_source: Option<usize>,
    pub(crate) activity_rects_cache: Vec<egui::Rect>,
    pub(crate) last_rate_check_time: f64,
    pub(crate) last_event_count: u64,
    pub(crate) event_rate: f64,
    pub(crate) dynamic_drag_source: Option<usize>,
    pub(crate) file_broker: Arc<FileAccessBroker>,
    pub(crate) dialog_receiver: crossbeam_channel::Receiver<DialogRequest>,
    pub(crate) file_browse_subscription: tool_databus::Subscription,
    pub(crate) replay_analyzer_job: Option<ReplayAnalyzerJob>,
    pub(crate) replay_analyzer_generation: u64,
}

pub(crate) struct ReplayAnalyzerJob {
    pub(crate) generation: u64,
    pub(crate) source_path: String,
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

// ══════════════════════════════════════════
//  WorkbenchApp impl
// ══════════════════════════════════════════

impl WorkbenchApp {
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
            bottom_panel_visible: rp.bottom_logs_visible,
            bottom_tab: BottomTab::Terminal,
            send: SendUiState::default(),
            terminal_popup_open: false,
            detached_dynamic_panels: BTreeSet::new(),
            top_bar_serial_collapsed: false,
            activity_order: config
                .as_ref()
                .map(|c| c.activity_order.clone())
                .unwrap_or_else(default_activity_order),
            activity_drag_source: None,
            activity_rects_cache: Vec::new(),
            last_rate_check_time: 0.0,
            last_event_count: 0,
            event_rate: 0.0,
            bus: bus.clone(),
            transport,
            plugin_manager: pm,
            recorder,
            dynamic_drag_source: None,
            file_broker,
            dialog_receiver,
            file_browse_subscription: bus.subscribe(tool_databus::TopicFilter::exact(
                tool_core::topics::UI_FORM_FILE_BROWSE,
            )),
            replay_analyzer_job: None,
            replay_analyzer_generation: 0,
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

// ── UI 组件 ──

impl eframe::App for WorkbenchApp {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        theme::BG_PRIMARY.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.clear_status_if_expired();
        match self.recorder.reap_stopping() {
            Some(Ok(path)) => {
                self.set_status_force(StatusLevel::Info, format!("录制已保存: {}", path.display()))
            }
            Some(Err(e)) => self.set_status_force(StatusLevel::Error, format!("录制失败: {e}")),
            None => {}
        }
        // 终端放大按钮
        if self.terminal_panel.maximize_clicked {
            self.terminal_panel.maximize_clicked = false;
            self.terminal_popup_open = true;
        }
        // 回放清理
        if self.replay_panel.want_clear_on_play {
            self.replay_panel.want_clear_on_play = false;
            self.terminal_panel.clear();
            self.bottom_log_panel.clear();
            self.dynamic_panels.clear_charts();
        }

        if let Some(steps) = self.replay_panel.want_step_backward.take() {
            self.terminal_panel.clear();
            self.bottom_log_panel.clear();
            self.dynamic_panels.clear_charts();

            self.bus.publish(Event::new(
                "ui.replay.reset",
                "ui.replay",
                Direction::Internal,
                Payload::Empty,
            ));

            let steps = steps.max(1);
            let pos = self.replay_panel.manager().backward_position_by(steps);

            if let Some(pos) = pos {
                // 阶段 1：先发布 ui.panel.create 并创建图表面板
                self.replay_panel.do_seek_panel_phase(pos);
                self.dynamic_panels.ingest(&mut self.panels);
                // 阶段 2：再发布数据事件
                self.replay_panel.do_seek_data_phase(pos);
            }

            let terminal_count = self.terminal_panel.ingest_all_pending();
            let log_count = self.bottom_log_panel.ingest_all_pending();
            let chart_count = self.dynamic_panels.ingest_all_pending();

            self.set_status(StatusLevel::Info, format!(
                "回放重建完成：接收 {terminal_count} 条，日志 {log_count} 条，图表 {chart_count} 条"
            ));
            ctx.request_repaint();
        }

        if let Some(p) = self.replay_panel.want_seek_replay.take() {
            self.terminal_panel.clear();
            self.bottom_log_panel.clear();
            self.dynamic_panels.clear_charts();

            self.bus.publish(Event::new(
                "ui.replay.reset",
                "ui.replay",
                Direction::Internal,
                Payload::Empty,
            ));

            // 阶段 1：先发布 ui.panel.create 并创建图表面板
            self.replay_panel.do_seek_panel_phase(p);
            self.dynamic_panels.ingest(&mut self.panels);
            // 阶段 2：再发布数据事件
            self.replay_panel.do_seek_data_phase(p);

            let terminal_count = self.terminal_panel.ingest_all_pending();
            let log_count = self.bottom_log_panel.ingest_all_pending();
            let chart_count = self.dynamic_panels.ingest_all_pending();

            self.set_status(StatusLevel::Info, format!(
                "回放重建完成：接收 {terminal_count} 条，日志 {log_count} 条，图表 {chart_count} 条"
            ));
            ctx.request_repaint();
        }
        if self.replay_panel.want_pick_file {
            self.replay_panel.want_pick_file = false;
            if let Some(p) = windows_open_dialog() {
                self.replay_panel.path = p.display().to_string();
                self.replay_panel.auto_load = true;
            }
        }
        // 运行 replay analyzer（后台线程，不卡 UI）
        if self.replay_panel.want_run_analyzers {
            self.launch_replay_analyzer_background();
        }
        // 检查后台 analyzer 是否完成
        self.poll_replay_analyzer_result();

        // 录制状态检测：worker 线程因错误退出时反馈给 UI
        if let Some(error) = self.recorder.reap_error() {
            self.set_status(StatusLevel::Error, format!("录制失败：{error}"));
        }

        // 处理 dialog 请求（Lua ctx.dialog.open_file）
        self.poll_dialog_requests();

        // 处理 file 字段浏览请求
        self.handle_file_browse_requests();

        // 处理插件禁用后的资源清理
        for plugin_id in self.plugins_panel.take_recently_disabled() {
            let removed = self.dynamic_panels.remove_by_plugin(&plugin_id);
            for id in &removed {
                self.detached_dynamic_panels.remove(id);
                self.panels
                    .close_tab(tool_panels::PanelKind::Dynamic(id.clone()));
            }
            self.file_broker.clear(&plugin_id);
        }

        self.dynamic_panels.ingest(&mut self.panels);
        let n = self.plugin_manager.process_pending();
        if n > 0 {
            self.set_status(StatusLevel::Info, format!("{n} 个插件事件"));
        }
        self.handle_keys(&ctx);

        // 速率统计
        let now = ctx.input(|i| i.time);
        if self.last_rate_check_time > 0.0 {
            let el = now - self.last_rate_check_time;
            if el >= 1.0 {
                let c = self.bus.published_count();
                self.event_rate = c.saturating_sub(self.last_event_count) as f64 / el;
                self.last_event_count = c;
                self.last_rate_check_time = now;
            }
        } else {
            self.last_rate_check_time = now;
            self.last_event_count = self.bus.published_count();
        }
        let refresh_interval = if ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
            0.5
        } else {
            2.0
        };
        if now - self.last_port_refresh > refresh_interval {
            self.last_port_refresh = now;
            self.refresh_ports_silent();
        }

        // 面板
        egui::Panel::top("top-bar").show_inside(ui, |ui| self.top_bar(ui));
        egui::Panel::left("activity-bar")
            .resizable(false)
            .default_size(ACTIVITY_BAR_WIDTH)
            .show_inside(ui, |ui| self.activity_bar(ui));

        egui::Panel::right("inspector")
            .resizable(false)
            .exact_size(if self.panels.inspector_visible {
                INSPECTOR_WIDTH
            } else {
                0.0
            })
            .show_separator_line(self.panels.inspector_visible)
            .show_inside(ui, |ui| {
                if self.panels.inspector_visible {
                    self.inspector(ui);
                }
            });

        if self.bottom_panel_visible {
            egui::Panel::bottom("bottom-bar")
                .resizable(true)
                .min_size(BOTTOM_PANEL_MIN)
                .default_size(BOTTOM_PANEL_HEIGHT)
                .show_separator_line(true)
                .show_inside(ui, |ui| self.show_bottom_panel_contents(ui));
        } else {
            egui::Panel::bottom("status-only")
                .resizable(false)
                .show_separator_line(false)
                .default_size(24.0)
                .show_inside(ui, |ui| self.status_bar(ui));
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.dynamic_tab_cleanup();
            if let Some(id) = self.panels.active_dynamic_id().map(str::to_owned) {
                self.dynamic_panel_ui(ui, &id);
            } else {
                match self.panels.activity {
                    Activity::Devices => self.device_panel(ui),
                    Activity::Replay => self.replay_panel.ui(ui),
                    Activity::Plugins => self.plugins_panel.ui(ui, &mut self.plugin_manager),
                    Activity::Settings => self.settings_panel(ui),
                    _ => self.device_panel(ui),
                }
            }
        });

        // 浮动拖拽副本
        if let Some(s) = self.activity_drag_source
            && s < self.activity_order.len()
            && let Some(p) = ctx.pointer_latest_pos()
        {
            let act = self.activity_order[s];
            let label = format!("{} {}", aicon(act), act.label());
            let gal = ctx.fonts_mut(|f| {
                f.layout(
                    label.clone(),
                    egui::FontId::proportional(12.0),
                    theme::TEXT_PRIMARY,
                    f32::INFINITY,
                )
            });
            let rect = egui::Rect::from_min_size(
                p + egui::vec2(8.0, -12.0),
                egui::vec2(gal.size().x + 16.0, 26.0),
            );
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("dghost"),
            ));
            painter.rect_filled(
                rect,
                5.0,
                egui::Color32::from_rgba_premultiplied(46, 80, 120, 210),
            );
            painter.galley(
                rect.center() - gal.size() * 0.5,
                gal,
                egui::Color32::from_rgba_premultiplied(255, 255, 255, 240),
            );
        }
        self.bottom_log_panel.ingest_pending();
        self.detached_dynamic_panel_viewports(&ctx);
        self.send_popup(&ctx);
        self.terminal_popup(&ctx);

        ctx.request_repaint_after(std::time::Duration::from_millis(REPAINT_INTERVAL_MS));
    }
}
