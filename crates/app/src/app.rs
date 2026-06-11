use crate::config::{
    config_path, default_recorder_path, ensure_jsonl_extension, load_config, pick_recorder_path,
    record_mode_label, windows_open_dialog,
};
use crate::state::SendUiState;
use crate::state::StatusState;
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
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StatusLevel {
    Info,
    Warn,
    Error,
}

impl StatusLevel {
    pub(crate) fn ttl_ms(self) -> u64 {
        match self {
            Self::Info => 5_000,
            Self::Warn => 8_000,
            Self::Error => 15_000,
        }
    }
}

impl WorkbenchApp {
    /// 统一状态入口。低级别不能覆盖未过期的高级消息。
    pub(crate) fn set_status(&mut self, level: StatusLevel, text: impl Into<String>) {
        let now = now_timestamp_ms();
        if level as u8 >= self.status.level as u8 || now > self.status.deadline_ms {
            self.status.level = level;
            self.status.message = text.into();
            self.status.deadline_ms = now + level.ttl_ms();
        }
    }

    /// 用户主动操作：总是更新状态（不被旧错误阻塞）。
    pub(crate) fn set_status_force(&mut self, level: StatusLevel, text: impl Into<String>) {
        let now = now_timestamp_ms();
        self.status.level = level;
        self.status.message = text.into();
        self.status.deadline_ms = now + level.ttl_ms();
    }

    /// 过期后重置为就绪。每帧调用。
    pub(crate) fn clear_status_if_expired(&mut self) {
        if now_timestamp_ms() > self.status.deadline_ms {
            self.status.level = StatusLevel::Info;
            self.status.message = "就绪".into();
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetachedPanelAction {
    None,
    Attach,
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BottomTab {
    Terminal,
    Logs,
}

impl BottomTab {
    const ALL: [Self; 2] = [Self::Terminal, Self::Logs];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Terminal => "接收",
            Self::Logs => "日志",
        }
    }

    pub(crate) fn is_available(self, terminal_popup_open: bool) -> bool {
        !matches!(self, Self::Terminal) || !terminal_popup_open
    }
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedConfig {
    panels: PanelManager,
    selected_port: Option<String>,
    baud_rate: String,
    data_bits: String,
    stop_bits: String,
    parity: String,
    timeout_ms: String,
    recorder_path: String,
    #[serde(default = "default_activity_order")]
    activity_order: Vec<Activity>,
    #[serde(default)]
    enabled_plugins: Vec<String>,
}

pub(crate) fn default_activity_order() -> Vec<Activity> {
    vec![
        Activity::Devices,
        Activity::Replay,
        Activity::Plugins,
        Activity::Settings,
    ]
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
    pub(crate) fn refresh_ports(&mut self) {
        self.refresh_ports_impl(true);
    }

    pub(crate) fn refresh_ports_silent(&mut self) {
        self.refresh_ports_impl(false);
    }

    pub(crate) fn refresh_ports_impl(&mut self, show_status: bool) {
        let old_names: BTreeSet<String> = self
            .ports
            .iter()
            .map(|port| port.port_name.clone())
            .collect();

        let old_selected = self.selected_port.clone();

        match self.transport.list_serial_ports() {
            Ok(new_ports) => {
                let new_names: BTreeSet<String> = new_ports
                    .iter()
                    .map(|port| port.port_name.clone())
                    .collect();

                let added_ports: Vec<String> = new_names.difference(&old_names).cloned().collect();

                let removed_ports: Vec<String> =
                    old_names.difference(&new_names).cloned().collect();

                self.ports = new_ports;

                let selected_still_exists = self
                    .selected_port
                    .as_ref()
                    .is_some_and(|selected| new_names.contains(selected));

                // 关键：只在当前选中端口消失时清空选择，不自动切到新端口。
                if !selected_still_exists {
                    let stale_port = self.selected_port.take();
                    if let Some(ref p) = stale_port {
                        self.set_status(StatusLevel::Warn, format!("{p} 已拔出或不可用"));
                    }
                }

                if show_status {
                    self.set_status(StatusLevel::Info, format!("{} 个串口", self.ports.len()));
                    return;
                }

                if !added_ports.is_empty() {
                    self.set_status(
                        StatusLevel::Info,
                        format!("发现串口 {}", added_ports.join(", ")),
                    );
                } else if !removed_ports.is_empty() {
                    self.set_status(
                        StatusLevel::Info,
                        format!("移除串口 {}", removed_ports.join(", ")),
                    );
                } else if self.selected_port != old_selected {
                    self.set_status(StatusLevel::Info, "请选择串口");
                }
            }
            Err(error) => {
                self.set_status(StatusLevel::Error, error.to_string());
            }
        }
    }

    pub(crate) fn open_selected_port(&mut self) {
        self.refresh_ports_silent();

        let Some(p) = self.selected_port.clone() else {
            self.log(LogLevel::Warn, "请选择串口");
            self.set_status(StatusLevel::Warn, "请选择串口");
            return;
        };

        let selected_exists = self.ports.iter().any(|port| port.port_name == p);

        if !selected_exists {
            self.set_status(StatusLevel::Error, format!("串口 {p} 不存在，请重新选择"));
            return;
        }

        let baud_rate = match self.baud_rate.trim().parse::<u32>() {
            Ok(v) if v > 0 => v,
            _ => {
                self.set_status_force(StatusLevel::Warn, "波特率格式错误");
                return;
            }
        };

        let timeout_ms = match self.timeout_ms.trim().parse::<u64>() {
            Ok(v) if (1..=1000).contains(&v) => v,
            _ => {
                self.set_status_force(StatusLevel::Warn, "超时时间必须为 1..=1000 ms");
                return;
            }
        };

        let cfg = SerialConfig {
            port_name: p.clone(),
            baud_rate,
            data_bits: pdb(&self.data_bits),
            stop_bits: psb(&self.stop_bits),
            parity: ppar(&self.parity),
            timeout_ms,
        };

        match self.transport.open_serial(cfg) {
            Ok(()) => {
                self.set_status_force(StatusLevel::Info, format!("{p} 已连接"));
                self.open_bottom_panel();
            }
            Err(e) => {
                self.set_status_force(StatusLevel::Error, e.to_string());
            }
        }
    }
    pub(crate) fn start_or_stop_recording(&mut self) {
        if self.recorder.is_running() || self.recorder.is_stopping() {
            self.recorder.stop();
            self.set_status_force(StatusLevel::Info, "正在停止录制...");
        } else {
            match self.recorder.start(PathBuf::from(&self.recorder_path)) {
                Ok(()) => {
                    self.set_status_force(StatusLevel::Info, "录制中");
                }
                Err(e) => {
                    self.set_status_force(StatusLevel::Error, e.to_string());
                }
            }
        }
    }
    pub(crate) fn save_config(&mut self) -> Result<(), String> {
        self.panels.bottom_logs_visible = self.bottom_panel_visible;
        let mut p = self.panels.clone();
        p.discard_dynamic_tabs();
        p.bottom_logs_visible = self.bottom_panel_visible;
        let cfg = PersistedConfig {
            panels: p,
            selected_port: self.selected_port.clone(),
            baud_rate: self.baud_rate.clone(),
            data_bits: self.data_bits.clone(),
            stop_bits: self.stop_bits.clone(),
            parity: self.parity.clone(),
            timeout_ms: self.timeout_ms.clone(),
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
        };
        let t = serde_json::to_string_pretty(&cfg).map_err(|e| format!("序列化失败：{e}"))?;
        std::fs::write(config_path(), t).map_err(|e| format!("写入失败：{e}"))
    }
    pub(crate) fn available_bottom_tabs(&self) -> Vec<BottomTab> {
        BottomTab::ALL
            .into_iter()
            .filter(|tab| tab.is_available(self.terminal_popup_open))
            .collect()
    }

    pub(crate) fn ensure_bottom_tab_available(&mut self) {
        if self.bottom_tab.is_available(self.terminal_popup_open) {
            return;
        }
        if let Some(tab) = self.available_bottom_tabs().into_iter().next() {
            self.bottom_tab = tab;
        }
    }

    pub(crate) fn open_bottom_panel(&mut self) {
        self.bottom_panel_visible = true;
        if BottomTab::Terminal.is_available(self.terminal_popup_open) {
            self.bottom_tab = BottomTab::Terminal;
        } else {
            self.ensure_bottom_tab_available();
        }
    }

    pub(crate) fn toggle_bottom_panel(&mut self) {
        if self.bottom_panel_visible {
            self.bottom_panel_visible = false;
        } else {
            self.open_bottom_panel();
            self.set_status(StatusLevel::Info, "底部面板已打开");
        }
    }

    // ── UI 组件 ──

    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let so = self
                .selected_port
                .as_deref()
                .is_some_and(|p| self.transport.status_port(p).open);
            let sl = if so {
                format!("串口 ▸ {}", self.selected_port.as_deref().unwrap_or("?"))
            } else {
                "串口 ▸ 未连接".into()
            };
            if ui
                .selectable_label(
                    !self.top_bar_serial_collapsed,
                    egui::RichText::new(format!("{} {sl}", if so { "●" } else { "○" }))
                        .color(if so { theme::GREEN } else { theme::RED }),
                )
                .clicked()
            {
                self.top_bar_serial_collapsed = !self.top_bar_serial_collapsed;
            }
            if !self.top_bar_serial_collapsed {
                self.serial_connect_controls(ui, "top-port", "top-baud", 130.0, 80.0, true);
            }
            ui.separator();
            let rec = self.recorder.is_running();
            if ui
                .button(if rec {
                    egui::RichText::new("⏹ 停止").color(theme::RED)
                } else {
                    egui::RichText::new("⏺ 录制").color(theme::TEXT_SECONDARY)
                })
                .clicked()
            {
                self.start_or_stop_recording();
            }
            if ui.small_button("保存布局").clicked() {
                match self.save_config() {
                    Ok(()) => self.set_status(StatusLevel::Info, "布局已保存"),
                    Err(e) => self.set_status(StatusLevel::Error, format!("保存布局失败：{e}")),
                }
            }
        });
    }

    pub(crate) fn serial_connect_controls(
        &mut self,
        ui: &mut egui::Ui,
        port_combo_id: &'static str,
        baud_combo_id: &'static str,
        port_width: f32,
        baud_width: f32,
        compact: bool,
    ) {
        if !compact {
            ui.label("端口");
        }

        serial_combo(
            ui,
            port_combo_id,
            port_width,
            &self.ports,
            &mut self.selected_port,
        );

        if compact {
            // 顶栏只显示连接状态，详细参数统一在设备页
        } else {
            ui.label("波特率");
            baud_combo(ui, baud_combo_id, baud_width, &mut self.baud_rate);
        }

        let selected_open = self
            .selected_port
            .as_deref()
            .is_some_and(|port| self.transport.status_port(port).open);

        if selected_open {
            if serial_action_button(ui, "重连").clicked() {
                self.open_selected_port();
            }
        } else if serial_action_button(ui, "打开").clicked() {
            self.open_selected_port();
        }

        if serial_action_button_enabled(ui, selected_open, "关闭").clicked() {
            if let Some(ref port) = self.selected_port {
                self.transport.close_port(port);
                self.set_status(StatusLevel::Info, format!("{port} 已关闭"));
            }
        }

        if !compact {
            match self.selected_port.as_deref() {
                Some(port) => {
                    let st = self.transport.status_port(port);

                    if st.open {
                        ui.label(
                            egui::RichText::new(format!(
                                "● {} @ {} {}N{}",
                                port,
                                st.baud_rate.unwrap_or(0),
                                &self.data_bits,
                                &self.stop_bits
                            ))
                            .color(theme::GREEN),
                        );
                    } else {
                        ui.label(egui::RichText::new("○ 未连接").color(theme::TEXT_SECONDARY));
                    }
                }
                None => {
                    ui.label(egui::RichText::new("○ 未选择串口").color(theme::TEXT_SECONDARY));
                }
            }
        }
    }
}

// ══════════════════════════════════════════
//  eframe::App
// ══════════════════════════════════════════

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

// ── 发送放大窗口 ──
impl WorkbenchApp {
    pub(crate) fn terminal_popup(&mut self, ctx: &egui::Context) {
        if !self.terminal_popup_open {
            return;
        }

        let vid = egui::ViewportId::from_hash_of("term-popup");
        let builder = egui::ViewportBuilder::default()
            .with_title("接收区 - 硬件调试工作台")
            .with_inner_size([800.0, 600.0]);

        let should_close = ctx.show_viewport_immediate(vid, builder, |ui, _| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                return true;
            }

            egui::CentralPanel::default()
                .show_inside(ui, |ui| {
                    let mut close = false;

                    ui.horizontal(|ui| {
                        ui.heading("接收区");
                        if ui.button("关闭").clicked() {
                            close = true;
                        }
                    });
                    ui.separator();

                    self.terminal_panel.height = (ui.available_height() - 42.0).max(120.0);
                    self.terminal_panel.ui(ui);

                    close
                })
                .inner
        });

        if should_close {
            self.terminal_popup_open = false;
        }
    }
    pub(crate) fn send_popup(&mut self, ctx: &egui::Context) {
        if !self.send.popup_open {
            return;
        }
        let vid = egui::ViewportId::from_hash_of("send-popup");
        let builder = egui::ViewportBuilder::default()
            .with_title("发送 - 硬件调试工作台")
            .with_inner_size([640.0, 480.0])
            .with_min_inner_size([360.0, 260.0]);
        let should_close = ctx.show_viewport_immediate(vid, builder, |ui, _| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                return true;
            }
            egui::CentralPanel::default()
                .show_inside(ui, |ui| {
                    let so = self
                        .selected_port
                        .as_deref()
                        .is_some_and(|p| self.transport.status_port(p).open);
                    let ctrl_enter = ui
                        .ctx()
                        .input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.send.hex_mode, false, "文本");
                        ui.radio_value(&mut self.send.hex_mode, true, "HEX");
                        ui.add_enabled_ui(!self.send.hex_mode, |ui| {
                            ui.checkbox(&mut self.send.append_lf, "LF")
                                .on_disabled_hover_text("HEX 模式请手动添加 0A");
                        });
                        if ui
                            .add_enabled(
                                so && !self.send.input.is_empty(),
                                egui::Button::new("发送 (Ctrl+Enter)"),
                            )
                            .clicked()
                            || (ctrl_enter && so && !self.send.input.is_empty())
                        {
                            self.do_send();
                        }
                        if ui.button("清空").clicked() {
                            self.send.input.clear();
                            self.send.error = None;
                        }
                    });
                    ui.separator();
                    ui.add(
                        egui::TextEdit::multiline(&mut self.send.input)
                            .desired_width(f32::INFINITY)
                            .desired_rows(24)
                            .hint_text("Ctrl+Enter 发送"),
                    );
                    if let Some(ref e) = self.send.error {
                        ui.colored_label(theme::RED, translate_error(e));
                    }
                    false
                })
                .inner
        });
        if should_close {
            self.send.popup_open = false;
        }
    }

    /// 处理 Lua ctx.dialog.open_file 请求。每帧最多处理一个。
    pub(crate) fn poll_dialog_requests(&mut self) {
        if let Ok(request) = self.dialog_receiver.try_recv() {
            let mut dialog = rfd::FileDialog::new().set_title(&request.title);
            for filter in &request.filters {
                if !filter.extensions.is_empty() && filter.extensions[0] != "*" {
                    dialog = dialog.add_filter(
                        &filter.name,
                        &filter
                            .extensions
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>(),
                    );
                }
            }
            let result = dialog.pick_file();
            // 授权路径
            if let Some(ref path) = result {
                self.file_broker.authorize(&request.plugin_id, path.clone());
            }
            // 发送结果回 Lua
            let _ = request.response_sender.send(result);
        }
    }

    /// 处理 ui.form.file_browse 请求。每帧最多处理一个，避免连续弹多个模态对话框。
    pub(crate) fn handle_file_browse_requests(&mut self) {
        let Some(event) = self.file_browse_subscription.try_recv() else {
            return;
        };
        if let Payload::Json(value) = event.payload {
            let panel_id = value.get("panel_id").and_then(Value::as_str).unwrap_or("");
            let field_id = value.get("field_id").and_then(Value::as_str).unwrap_or("");
            let filters: Vec<tool_lua_host::FileFilter> = value
                .get("filters")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .map(|f| tool_lua_host::FileFilter {
                            name: f
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                            extensions: f
                                .get("extensions")
                                .and_then(Value::as_array)
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| v.as_str().map(String::from))
                                        .collect()
                                })
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            let mut dialog = rfd::FileDialog::new().set_title("选择文件");
            for filter in &filters {
                if !filter.extensions.is_empty() && filter.extensions[0] != "*" {
                    dialog = dialog.add_filter(
                        &filter.name,
                        &filter
                            .extensions
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>(),
                    );
                }
            }
            let result = dialog.pick_file();

            // 用户取消时不发布事件，避免清空表单原路径
            if let Some(ref selected_path) = result {
                if let Some(owner) = self.dynamic_panels.panel_owner(panel_id) {
                    self.file_broker.authorize(owner, selected_path.clone());
                } else {
                    self.log(
                        LogLevel::Warn,
                        &format!("file 字段 {panel_id}/{field_id} 没有 owner plugin，跳过授权"),
                    );
                }

                self.bus.publish(Event::new(
                    tool_core::topics::UI_FORM_FILE_SELECTED,
                    "ui",
                    Direction::Internal,
                    Payload::Json(serde_json::json!({
                        "panel_id": panel_id,
                        "field_id": field_id,
                        "path": selected_path.display().to_string(),
                    })),
                ));
            }
        }
    }
}

impl Drop for WorkbenchApp {
    fn drop(&mut self) {
        let _ = self.save_config();
        self.recorder.stop();
        self.transport.close_serial();
    }
}

// ══════════════════════════════════════════
//  辅助函数
// ══════════════════════════════════════════

pub(crate) fn pdb(v: &str) -> DataBits {
    match v {
        "5" => DataBits::Five,
        "6" => DataBits::Six,
        "7" => DataBits::Seven,
        _ => DataBits::Eight,
    }
}
pub(crate) fn psb(v: &str) -> StopBits {
    match v {
        "2" => StopBits::Two,
        _ => StopBits::One,
    }
}
pub(crate) fn ppar(v: &str) -> Parity {
    match v {
        "odd" => Parity::Odd,
        "even" => Parity::Even,
        _ => Parity::None,
    }
}
pub(crate) fn send_impl_to(
    port: &str,
    input: &str,
    hex: bool,
    lf: bool,
    t: &TransportManager,
) -> Result<(), tool_transport::TransportError> {
    if input.trim().is_empty() {
        return Ok(());
    }
    if hex {
        for line in input.lines() {
            let x = line.trim();
            if x.is_empty() {
                continue;
            }
            t.send_hex_to(port, x)?;
        }
        Ok(())
    } else {
        let mut text = input.to_owned();
        if lf {
            text.push('\n');
        }
        t.send_text_to(port, &text)
    }
}
pub(crate) fn translate_error(m: &str) -> String {
    if m.contains("no serial") {
        "串口未打开".into()
    } else if m.contains("invalid hex") {
        format!("无效HEX: {}", m.trim_start_matches("invalid hex input: "))
    } else {
        m.to_owned()
    }
}

pub(crate) fn aicon(a: Activity) -> &'static str {
    match a {
        Activity::Devices => "📟",
        Activity::Replay => "⏪",
        Activity::Plugins => "🧩",
        Activity::Settings => "⚙",
        _ => "",
    }
}
pub(crate) fn ashortcut(a: Activity) -> &'static str {
    match a {
        Activity::Devices => "Ctrl+1",
        Activity::Replay => "Ctrl+2",
        Activity::Plugins => "Ctrl+3",
        Activity::Settings => "Ctrl+4",
        _ => "",
    }
}

pub(crate) fn activity_insert_index_from_pointer(
    rects: &[egui::Rect],
    pointer: egui::Pos2,
) -> Option<usize> {
    if rects.is_empty() {
        return None;
    }

    let left = rects
        .iter()
        .map(|rect| rect.left())
        .fold(f32::INFINITY, f32::min);

    let right = rects
        .iter()
        .map(|rect| rect.right())
        .fold(f32::NEG_INFINITY, f32::max);

    let top = rects.first()?.top() - 14.0;
    let bottom = rects.last()?.bottom() + 14.0;

    if pointer.x < left - 16.0 || pointer.x > right + 16.0 || pointer.y < top || pointer.y > bottom
    {
        return None;
    }

    for (index, rect) in rects.iter().enumerate() {
        if pointer.y < rect.center().y {
            return Some(index);
        }
    }

    Some(rects.len())
}

pub(crate) fn paint_activity_insert_line(ui: &egui::Ui, rects: &[egui::Rect], insert_index: usize) {
    if rects.is_empty() {
        return;
    }

    let left = rects
        .iter()
        .map(|rect| rect.left())
        .fold(f32::INFINITY, f32::min);

    let right = rects
        .iter()
        .map(|rect| rect.right())
        .fold(f32::NEG_INFINITY, f32::max);

    let y = if insert_index == 0 {
        rects[0].top() - 3.0
    } else if insert_index >= rects.len() {
        rects[rects.len() - 1].bottom() + 3.0
    } else {
        let above = rects[insert_index - 1];
        let below = rects[insert_index];
        (above.bottom() + below.top()) * 0.5
    };

    let painter = ui.painter();

    painter.line_segment(
        [egui::pos2(left + 6.0, y), egui::pos2(right - 6.0, y)],
        egui::Stroke::new(2.0, theme::BLUE),
    );

    painter.circle_filled(egui::pos2(left + 6.0, y), 3.0, theme::BLUE);
    painter.circle_filled(egui::pos2(right - 6.0, y), 3.0, theme::BLUE);
}
pub(crate) fn vertical_insert_index_from_pointer(
    rects: &[egui::Rect],
    pointer: egui::Pos2,
) -> Option<usize> {
    if rects.is_empty() {
        return None;
    }

    let left = rects
        .iter()
        .map(|rect| rect.left())
        .fold(f32::INFINITY, f32::min);

    let right = rects
        .iter()
        .map(|rect| rect.right())
        .fold(f32::NEG_INFINITY, f32::max);

    let top = rects.first()?.top() - 10.0;
    let bottom = rects.last()?.bottom() + 10.0;

    if pointer.x < left - 16.0 || pointer.x > right + 16.0 || pointer.y < top || pointer.y > bottom
    {
        return None;
    }

    for (index, rect) in rects.iter().enumerate() {
        if pointer.y < rect.center().y {
            return Some(index);
        }
    }

    Some(rects.len())
}

pub(crate) fn paint_vertical_insert_line(ui: &egui::Ui, rects: &[egui::Rect], insert_index: usize) {
    if rects.is_empty() {
        return;
    }

    let left = rects
        .iter()
        .map(|rect| rect.left())
        .fold(f32::INFINITY, f32::min);

    let right = rects
        .iter()
        .map(|rect| rect.right())
        .fold(f32::NEG_INFINITY, f32::max);

    let y = if insert_index == 0 {
        rects[0].top() - 3.0
    } else if insert_index >= rects.len() {
        rects[rects.len() - 1].bottom() + 3.0
    } else {
        let above = rects[insert_index - 1];
        let below = rects[insert_index];
        (above.bottom() + below.top()) * 0.5
    };

    let painter = ui.painter();

    painter.line_segment(
        [egui::pos2(left + 6.0, y), egui::pos2(right - 6.0, y)],
        egui::Stroke::new(2.0, theme::BLUE),
    );

    painter.circle_filled(egui::pos2(left + 6.0, y), 3.0, theme::BLUE);
    painter.circle_filled(egui::pos2(right - 6.0, y), 3.0, theme::BLUE);
}
