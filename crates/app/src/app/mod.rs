use crate::config::default_activity_order;
use crate::config::{ConfigLoadResult, PersistedConfig, default_recorder_path, load_config};
use crate::state::{MAX_SEND_HISTORY, SendUiState, SerialUiState, StatusState, UpdateState};
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
use tool_transport::TransportManager;

use crate::bootstrap::{app_dir, apply_theme, setup_fonts};

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
    pub(crate) serial: SerialUiState,
    pub(crate) recorder_path: String,
    pub(crate) status: StatusState,
    pub(crate) recent_workspaces: Vec<String>,
    pub(crate) send: SendUiState,
    pub(crate) terminal_popup_open: bool,
    pub(crate) terminal_popup_always_on_top: bool,
    pub(crate) send_popup_always_on_top: bool,
    pub(crate) detached_dynamic_panels: BTreeSet<String>,
    pub(crate) activity_order: Vec<Activity>,
    pub(crate) activity_drag_source: Option<usize>,
    pub(crate) activity_rects_cache: Vec<egui::Rect>,
    pub(crate) dock_dragging_panel: Option<tool_panels::PanelKind>,
    pub(crate) bottom_dock_rect: Option<egui::Rect>,
    pub(crate) right_dock_rect: Option<egui::Rect>,
    pub(crate) last_auto_save_time: f64,
    pub(crate) dynamic_drag_source: Option<usize>,
    pub(crate) file_broker: Arc<FileAccessBroker>,
    pub(crate) dialog_receiver: crossbeam_channel::Receiver<DialogRequest>,
    pub(crate) file_browse_subscription: tool_databus::Subscription,
    pub(crate) contribution_set_value_subscription: tool_databus::Subscription,
    pub(crate) replay_analyzer_job: Option<ReplayAnalyzerJob>,
    pub(crate) replay_analyzer_generation: u64,
    /// 周期发送后台线程的取消信号
    pub(crate) periodic_send_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// 周期发送后台线程的结束原因（失败/完成），主线程 tick 读取后回写状态栏。
    /// 后台线程无 &mut self，只能通过共享通道传递用户可见反馈。
    pub(crate) periodic_send_outcome:
        std::sync::Arc<std::sync::Mutex<Option<(crate::state::StatusLevel, String)>>>,
    /// 可配置快捷键映射
    pub(crate) keymap: crate::keymap::Keymap,
    /// 当前帧触发的快捷键动作（handle_keys 设置，tick 执行）
    pub(crate) pending_action: Option<crate::keymap::Action>,
    /// 快捷键录制状态：点击"录制"后等待用户按键
    pub(crate) key_recording: Option<crate::keymap::Action>,
    /// 自动更新状态
    pub(crate) update_state: UpdateState,
    /// UI contribution 运行时状态（toggle 值、progress 值等）
    pub(crate) contribution_states: std::collections::HashMap<String, serde_json::Value>,
    /// 插件 summaries 帧级缓存：每帧首次需要时计算一次，避免 ui_contribution_slot
    /// 在 top_bar/status_bar/bottom_panel 每帧共 5+ 次重复全量 clone manifest + 命令对账。
    /// 在 tick_pre_ui 开头 take() 重置。
    pub(crate) plugin_summaries_cache:
        std::cell::OnceCell<Vec<tool_extension::PluginSummary>>,
    /// 等宽字体大小（终端/日志区），默认 13.0
    pub(crate) monospace_font_size: f32,
}

pub(crate) struct ReplayAnalyzerJob {
    pub(crate) generation: u64,
    pub(crate) source_path: String,
    pub(crate) cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(crate) handle: Option<std::thread::JoinHandle<ReplayAnalyzerResult>>,
}

impl Drop for ReplayAnalyzerJob {
    fn drop(&mut self) {
        // 退出时取消 analyzer 线程并尝试 join（带超时，避免卡住 drop）。
        // analyzer 线程有 budget hook（30_000 指令）+ cancel 检查，最终会终止；
        // 此处 join 只为回收资源、避免 detach。
        self.cancel.store(true, std::sync::atomic::Ordering::Release);
        if let Some(handle) = self.handle.take() {
            // 短轮询等待最多 ~2s，超时则放弃 join（线程最终会自行退出）。
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !handle.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
            // 否则 detach：analyzer 线程会在 cancel 信号下自然退出，不泄漏。
        }
    }
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
        // 主题必须尽早设置，否则 eframe 在 new() 返回前可能已用默认主题渲染了首帧。
        apply_theme(&cc.egui_ctx);
        setup_fonts(cc);
        cc.egui_ctx.set_embed_viewports(false);
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());

        // 注入 UI 重绘唤醒器：串口 worker publish RX/TX 后立即 request_repaint，
        // 消除 80ms 轮询导致的显示延迟。has_repaint 短路防止重复唤醒风暴。
        // egui 0.35 的 Context 无 weak()，用强引用 clone（worker 退出前 Context 保持存活，
        // app 退出时 transport.close_serial() 先让 worker 退出，再 drop 闭包释放 Context）。
        {
            let ctx_strong = cc.egui_ctx.clone();
            transport.set_repaint_waker(std::sync::Arc::new(move || {
                if !ctx_strong.has_requested_repaint() {
                    ctx_strong.request_repaint();
                }
            }));
        }

        let (dialog_sender, dialog_receiver) = crossbeam_channel::unbounded::<DialogRequest>();
        let file_broker = Arc::new(FileAccessBroker::default());

        let mut pm = PluginManager::new(bus.clone(), transport.clone());
        pm.set_host_services(dialog_sender, file_broker.clone());

        let plugin_dir = app_dir().join("plugins");
        if let Err(e) = pm.discover_roots([plugin_dir, PathBuf::from("plugins")]) {
            bus.publish(Event::system_log(
                LogLevel::Error,
                "ext",
                format!("插件发现失败：{e}"),
            ));
        }
        let recorder = JsonlRecorder::new(bus.clone());
        let config_result = load_config();
        let config: Option<PersistedConfig> = match config_result {
            ConfigLoadResult::Ok(cfg) => Some(cfg),
            ConfigLoadResult::ParseError {
                ref path,
                ref error,
            } => {
                bus.publish(Event::system_log(
                    LogLevel::Error,
                    "app",
                    format!("配置文件损坏 {}: {error}，使用默认设置", path.display()),
                ));
                None
            }
            ConfigLoadResult::NotFound => {
                bus.publish(Event::system_log(
                    LogLevel::Warn,
                    "app",
                    "未找到配置文件，使用默认设置",
                ));
                None
            }
        };
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
            send.line_ending = cfg.line_ending;
        }

        let mut app = Self {
            terminal_panel: TerminalPanel::new(&bus),
            dynamic_panels: DynamicPanels::new(&bus),
            plugins_panel: PluginsPanel::new(),
            replay_panel: ReplayPanel::new(&bus),
            bottom_log_panel: LogPanel::new(&bus),
            serial: SerialUiState {
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
                last_port_refresh: 0.0,
                auto_reconnect: config.as_ref().map(|c| c.auto_reconnect).unwrap_or(true),
                pending_reconnect: None,
                port_aliases: config
                    .as_ref()
                    .map(|c| c.port_aliases.clone())
                    .unwrap_or_default(),
                port_groups: config
                    .as_ref()
                    .map(|c| c.port_groups.clone())
                    .unwrap_or_default(),
                port_profiles: config
                    .as_ref()
                    .map(|c| c.port_profiles.clone())
                    .unwrap_or_default(),
                top_bar_serial_collapsed: false,
            },
            recorder_path: config
                .as_ref()
                .map(|c| c.recorder_path.clone())
                .unwrap_or_else(default_recorder_path),
            panels: rp.clone(),
            status: StatusState::default(),
            recent_workspaces: config
                .as_ref()
                .map(|c| c.recent_workspaces.clone())
                .unwrap_or_default(),
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
            activity_order: config
                .as_ref()
                .map(|c| c.activity_order.clone())
                .unwrap_or_else(default_activity_order),
            activity_drag_source: None,
            activity_rects_cache: Vec::new(),
            last_auto_save_time: 0.0,
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
            contribution_set_value_subscription: bus.subscribe(tool_databus::TopicFilter::exact(
                tool_core::topics::UI_CONTRIBUTION_SET_VALUE,
            )),
            replay_analyzer_job: None,
            replay_analyzer_generation: 0,
            periodic_send_cancel: None,
            periodic_send_outcome: std::sync::Arc::new(std::sync::Mutex::new(None)),
            dock_dragging_panel: None,
            bottom_dock_rect: None,
            right_dock_rect: None,
            keymap: config
                .as_ref()
                .map(|c| c.keymap.clone())
                .unwrap_or_default(),
            pending_action: None,
            key_recording: None,
            update_state: UpdateState::default(),
            contribution_states: std::collections::HashMap::new(),
            plugin_summaries_cache: std::cell::OnceCell::new(),
            monospace_font_size: config
                .as_ref()
                .map(|c| c.monospace_font_size.clamp(10.0, 24.0))
                .unwrap_or(13.0),
        };
        // 从配置恢复等宽字体大小
        app.terminal_panel.font_size = app.monospace_font_size;
        app.bottom_log_panel.font_size = app.monospace_font_size;
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
        if let Err(e) = self.save_config() {
            log::warn!("save_config failed: {e}")
        };
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

        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let poll_interval_ms = if focused { 80 } else { 250 };
        ctx.request_repaint_after(std::time::Duration::from_millis(poll_interval_ms));
    }
}
