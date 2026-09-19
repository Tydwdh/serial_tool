use crate::config::{
    ConfigLoadResult, PersistedConfig, default_recorder_path, load_config, resolve_theme_path,
};
use crate::state::{MAX_SEND_HISTORY, NotificationQueue, SendUiState, SerialUiState, UpdateState};
use eframe::egui;
use std::collections::VecDeque;
use tool_application::{ApplicationConfig, Workbench};
use tool_core::{Event, LogLevel};
use tool_databus::DataBus;
use tool_marketplace::retire_old_plugin_dirs;
use tool_panels::{
    ChartPanel, DynamicPanels, LogPanel, PanelManager, PluginsPanel, ReplayPanel, TerminalPanel,
    theme,
};
use tool_transport::RepaintWaker;

use crate::bootstrap::{apply_theme, setup_fonts, user_plugins_dir, user_themes_dir};
use crate::ui::toast::ToastOverlay;
pub(crate) use crate::workbench_app::WorkbenchApp;

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
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
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
        apply_theme(&cc.egui_ctx, theme::AppTheme::default());
        setup_fonts(cc);
        let bus = DataBus::new();
        // waker 注入需在 Workbench 创建前准备好闭包，创建后立即注入
        let ctx_strong = cc.egui_ctx.clone();
        let waker: std::sync::Arc<dyn RepaintWaker> = std::sync::Arc::new(move || {
            if !ctx_strong.has_requested_repaint() {
                ctx_strong.request_repaint();
            }
        });
        let config_result = load_config();
        let (config, config_migrated, config_write_protected): (
            Option<PersistedConfig>,
            bool,
            bool,
        ) = match config_result {
            ConfigLoadResult::Ok { config, migrated } => (Some(config), migrated, false),
            ConfigLoadResult::ParseError {
                ref path,
                ref error,
                ref backup_path,
            } => {
                let backup_note = backup_path.as_ref().map_or_else(String::new, |backup| {
                    format!("，已备份为 {}", backup.display())
                });
                bus.publish(Event::system_log(
                    LogLevel::Error,
                    "app",
                    format!(
                        "配置文件损坏 {}: {error}{backup_note}，使用默认设置",
                        path.display()
                    ),
                ));
                (None, false, false)
            }
            ConfigLoadResult::FutureVersion { ref path, version } => {
                bus.publish(Event::system_log(
                    LogLevel::Error,
                    "app",
                    format!(
                        "配置 {} 使用未来版本 v{version}，当前程序不会覆盖它；请升级后再打开",
                        path.display()
                    ),
                ));
                (None, false, true)
            }
            ConfigLoadResult::NotFound => {
                bus.publish(Event::system_log(
                    LogLevel::Warn,
                    "app",
                    "未找到配置文件，使用默认设置",
                ));
                (None, false, false)
            }
        };
        let theme_dir = user_themes_dir();
        if let Err(error) = theme::ensure_theme_directory(&theme_dir) {
            log::warn!("initialize theme directory failed: {error}");
        }
        let default_theme = theme::AppTheme::default();
        let mut loaded_theme = default_theme;
        let mut loaded_theme_path = theme::builtin_theme_path(default_theme, &theme_dir);
        let mut theme_recovered = false;
        if let Some(cfg) = config.as_ref() {
            if let Some(path) = cfg
                .theme_path
                .as_deref()
                .map(|path| resolve_theme_path(&theme_dir, path))
            {
                match theme::load_theme_file(&path) {
                    Ok(_) => {
                        loaded_theme =
                            theme::builtin_theme_for_path(&path).unwrap_or(theme::AppTheme::Custom);
                        loaded_theme_path = Some(path);
                    }
                    Err(error) => {
                        log::warn!("load theme JSON failed: {error}");
                        theme_recovered = true;
                        if let Err(fallback_error) =
                            theme::load_builtin_theme(default_theme, &theme_dir)
                        {
                            log::warn!("load fallback bundled theme failed: {fallback_error}");
                        }
                    }
                }
            } else if let Err(error) = theme::load_builtin_theme(cfg.ui_theme, &theme_dir) {
                log::warn!("load legacy bundled theme failed: {error}");
            } else {
                loaded_theme = cfg.ui_theme;
                loaded_theme_path = theme::builtin_theme_path(cfg.ui_theme, &theme_dir);
            }
        } else if let Err(error) = theme::load_builtin_theme(default_theme, &theme_dir) {
            log::warn!("load default bundled theme failed: {error}");
        }
        apply_theme(&cc.egui_ctx, loaded_theme);
        let mut panel_manager = config
            .as_ref()
            .map(|cfg| cfg.panels.clone())
            .unwrap_or_else(PanelManager::default_workspace);
        panel_manager.discard_dynamic_tabs();
        panel_manager.dock.normalize_tool_layout();
        panel_manager.ensure_tiles_layout();
        // 串口/发送相关 UI 状态：没有配置文件时取 `default()`，
        // 有配置时只覆盖这些持久化字段，其余（端口列表、待重连等）仍是运行时初值。
        let mut send = SendUiState::default();
        let mut serial = SerialUiState::default();
        if let Some(cfg) = config.as_ref() {
            send.send_history = cfg
                .send_history
                .iter()
                .filter(|item| !item.trim().is_empty())
                .take(MAX_SEND_HISTORY)
                .cloned()
                .collect::<VecDeque<_>>();
            send.line_ending = cfg.line_ending;

            serial.selected_port = cfg.selected_port.clone();
            serial.baud_rate = cfg.baud_rate.clone();
            serial.data_bits = cfg.data_bits.clone();
            serial.stop_bits = cfg.stop_bits.clone();
            serial.parity = cfg.parity.clone();
            serial.auto_reconnect = cfg.auto_reconnect;
            serial.port_aliases = cfg.port_aliases.clone();
            serial.port_groups = cfg.port_groups.clone();
            serial.port_profiles = cfg.port_profiles.clone();
            serial.network_ports = cfg.network_ports.clone();
        }

        let mut workbench = Workbench::new(bus.clone());
        workbench.set_transport_repaint_waker(waker);
        let plugin_dir = user_plugins_dir();
        retire_old_plugin_dirs(&plugin_dir);
        if let Some(cfg) = config.as_ref() {
            let application_config = ApplicationConfig {
                selected_port: cfg.selected_port.clone(),
                baud_rate: cfg.baud_rate.clone(),
                data_bits: cfg.data_bits.clone(),
                stop_bits: cfg.stop_bits.clone(),
                parity: cfg.parity.clone(),
                auto_reconnect: cfg.auto_reconnect,
                terminal_merge_window_ms: cfg.terminal_merge_window_ms,
                terminal_max_entries: cfg.terminal_max_entries,
                log_max_entries: cfg.log_max_entries,
                recorder_path: cfg.recorder_path.clone(),
                network_ports: cfg.network_ports.clone(),
                port_aliases: cfg.port_aliases.clone(),
                port_groups: cfg.port_groups.clone(),
                enabled_plugins: cfg.enabled_plugins.clone(),
                network_proxy_url: cfg.network_proxy_url.clone(),
            };
            workbench = workbench.with_config(application_config);
        }
        // 初始插件扫描也走统一后台任务，避免启动阶段在 UI 线程读 manifest。
        let _ = workbench.dispatch(tool_application::AppCommand::DiscoverPlugins {
            roots: vec![plugin_dir],
        });
        let ui_events = workbench.subscribe_ui_events();
        let mut app = Self {
            workbench,
            terminal_panel: TerminalPanel::new(&bus),
            chart_panel: ChartPanel::new(&bus),
            dynamic_panels: DynamicPanels::new(&bus),
            plugins_panel: PluginsPanel::new(),
            replay_panel: ReplayPanel::new(),
            bottom_log_panel: LogPanel::new(&bus),
            serial,
            recorder_path: config
                .as_ref()
                .map(|cfg| cfg.recorder_path.clone())
                .unwrap_or_else(default_recorder_path),
            panels: panel_manager,
            notifications: NotificationQueue::new(),
            toast_overlay: ToastOverlay::default(),
            recent_workspaces: config
                .as_ref()
                .map(|cfg| cfg.recent_workspaces.clone())
                .unwrap_or_default(),
            send,
            layout_dirty: false,
            last_auto_save_time: 0.0,
            ui_events,
            replay_analyzer: Default::default(),
            periodic_send: Default::default(),
            keymap: config
                .as_ref()
                .map(|cfg| cfg.keymap.clone())
                .unwrap_or_default(),
            commands: crate::command_registry::CommandRegistry::builtin(),
            panel_registry: crate::panel_registry::PanelRegistry::builtin(),
            pending_command: None,
            key_recording: None,
            command_palette: Default::default(),
            update_state: UpdateState::default(),
            contribution_states: std::collections::HashMap::new(),
            plugin_summaries_cache: std::cell::OnceCell::new(),
            monospace_font_size: config
                .as_ref()
                .map(|cfg| cfg.monospace_font_size.clamp(10.0, 24.0))
                .unwrap_or(13.0),
            ui_theme: loaded_theme,
            theme_path: loaded_theme_path,
            theme_dir,
            network_proxy_url: config
                .as_ref()
                .and_then(|cfg| cfg.network_proxy_url.clone())
                .unwrap_or_default(),
            marketplace: Default::default(),
            perf: crate::perf::PerfDiagnostics::default(),
            native_export: None,
        };
        // 从配置恢复等宽字体大小
        app.terminal_panel.font_size = app.monospace_font_size;
        app.bottom_log_panel.font_size = app.monospace_font_size;
        // 从配置恢复终端/日志的数据参数
        if let Some(cfg) = config.as_ref() {
            app.terminal_panel.merge_window_ms = cfg.terminal_merge_window_ms;
            app.terminal_panel.set_max_entries(cfg.terminal_max_entries);
            app.bottom_log_panel.set_max_entries(cfg.log_max_entries);
        }
        app.refresh_ports();
        let should_persist_config = !config_write_protected
            && (config_migrated
                || theme_recovered
                || config.as_ref().is_none_or(|cfg| cfg.theme_path.is_none()));
        if should_persist_config && let Err(error) = app.save_config() {
            log::warn!("persist theme path migration failed: {error}");
        }
        app.log(LogLevel::Info, "就绪");
        app
    }

    pub(crate) fn log(&self, lv: LogLevel, m: impl Into<String>) {
        self.workbench.log(lv, m);
    }

    /// 帧级缓存的插件 summaries。
    ///
    /// `summaries()` 会全量 clone 所有 manifest + 做命令对账；同帧内会被
    /// `ui_contribution_slot`（每 slot 一次）、命令面板、插件面板、设置面板、
    /// 快捷键标签等多处调用。这里用一个 `OnceCell` 做帧内缓存：每帧首次调用
    /// 计算一次，同帧后续调用复用同一份 `Vec`。`tick_pre_ui` 开头会重置缓存。
    ///
    /// 注意：返回的是 `&[PluginSummary]` 借用，调用方不能在此引用存活期间
    /// 再 `&mut self`。需要 `&mut self` 的逻辑应先把需要的字段 clone 出来
    /// 或在循环外处理。
    pub(crate) fn plugin_summaries(&self) -> &[tool_application::query::PluginSummaryView] {
        self.plugin_summaries_cache
            .get_or_init(|| self.workbench.query_plugins().summaries)
    }
}

impl Drop for WorkbenchApp {
    fn drop(&mut self) {
        // 退出前自动保存工作区
        if let Err(error) = self.save_config() {
            log::warn!("save_config failed: {error}")
        }
        let _ = self
            .workbench
            .dispatch(tool_application::AppCommand::StopRecording);
        self.workbench.shutdown_serial();
    }
}

// ── UI 组件 ──

impl eframe::App for WorkbenchApp {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        theme::bg_primary().to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let frame_started = self.perf.begin_frame();
        let ctx = ui.ctx().clone();
        self.tick_pre_ui(&ctx);
        self.draw_shell(ui, &ctx);
        if let Some(message) = tool_panels::take_copy_feedback(&ctx) {
            self.notifications
                .push("clipboard", crate::state::StatusLevel::Info, message);
        }
        self.tick_post_ui(&ctx);
        if let Some(format) = self.terminal_panel.take_export_request() {
            self.export_terminal_data(format);
        }
        if let Some(format) = self.bottom_log_panel.take_export_request() {
            self.export_log_data(format);
        }
        self.toast_overlay.show(&ctx, &mut self.notifications);

        let perf_snapshot = self.workbench.perf_snapshot();
        self.perf.end_frame(frame_started, perf_snapshot);

        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let poll_interval_ms = if focused { 80 } else { 250 };
        ctx.request_repaint_after(std::time::Duration::from_millis(poll_interval_ms));
    }
}
