use crate::config::{
    ConfigLoadResult, PersistedConfig, default_recorder_path, load_config, resolve_theme_path,
};
use crate::state::{MAX_SEND_HISTORY, NotificationQueue, SendUiState, SerialUiState, UpdateState};
use eframe::egui;
use std::collections::VecDeque;
use tool_application::api::core::{Event, LogLevel};
use tool_application::api::databus::DataBus;
use tool_application::{ApplicationConfig, Workbench};
use tool_panels::{
    DynamicPanels, LogPanel, PanelManager, PluginsPanel, ReplayPanel, TerminalPanel, theme,
};

use crate::bootstrap::{apply_theme, setup_fonts, user_plugins_dir, user_themes_dir};
use crate::ui::toast::ToastOverlay;

// ── 数据结构 ──

pub(crate) struct WorkbenchApp {
    pub(crate) workbench: Workbench,
    pub(crate) panels: PanelManager,
    pub(crate) terminal_panel: TerminalPanel,
    pub(crate) dynamic_panels: DynamicPanels,
    pub(crate) plugins_panel: PluginsPanel,
    pub(crate) replay_panel: ReplayPanel,
    pub(crate) bottom_log_panel: LogPanel,
    pub(crate) serial: SerialUiState,
    pub(crate) recorder_path: String,
    pub(crate) notifications: NotificationQueue,
    pub(crate) toast_overlay: ToastOverlay,
    pub(crate) recent_workspaces: Vec<String>,
    pub(crate) send: SendUiState,
    /// `egui_tiles` 的拖拽、选中和尺寸变更尚未写入配置。
    pub(crate) layout_dirty: bool,
    pub(crate) last_auto_save_time: f64,
    pub(crate) ui_events: tool_application::UiEventSubscriptions,
    pub(crate) replay_analyzer: crate::replay_task::ReplayAnalyzerState,
    /// 周期发送后台线程控制状态。
    pub(crate) periodic_send: crate::runtime::periodic_send::PeriodicSendState,
    /// 可配置快捷键映射
    pub(crate) keymap: crate::keymap::Keymap,
    /// 统一命令注册表（内置 + 插件命令）
    pub(crate) commands: crate::command_registry::CommandRegistry,
    /// 统一面板注册表（内置 + 插件动态面板）
    pub(crate) panel_registry: crate::panel_registry::PanelRegistry,
    /// 当前帧触发的快捷键命令 ID（handle_keys/命令面板设置，tick 执行）
    pub(crate) pending_command: Option<String>,
    /// 快捷键录制状态：点击"录制"后等待用户按键（命令 ID）
    pub(crate) key_recording: Option<String>,
    /// 命令面板状态（搜索、选中、使用顺序）
    pub(crate) command_palette: crate::ui::command_palette::CommandPaletteState,
    /// 自动更新状态
    pub(crate) update_state: UpdateState,
    /// UI contribution 运行时状态（toggle 值、progress 值等）
    pub(crate) contribution_states: std::collections::HashMap<String, serde_json::Value>,
    /// 插件 summaries 帧级缓存：每帧首次需要时计算一次，避免 ui_contribution_slot
    /// 在 top_bar/status_bar/bottom_panel 每帧共 5+ 次重复全量 clone manifest + 命令对账。
    /// 在 tick_pre_ui 开头 take() 重置。
    pub(crate) plugin_summaries_cache:
        std::cell::OnceCell<Vec<tool_application::api::extension::PluginSummary>>,
    /// 等宽字体大小（终端/日志区），默认 13.0
    pub(crate) monospace_font_size: f32,
    /// 当前主题的运行时风格（由已选 JSON 文件推导）。
    pub(crate) ui_theme: theme::AppTheme,
    /// 当前主题 JSON 的路径（内置和用户新增主题共用）。
    pub(crate) theme_path: Option<std::path::PathBuf>,
    /// 主题 JSON 文件目录。
    pub(crate) theme_dir: std::path::PathBuf,
    /// 网络请求的可选自定义代理；为空时交给系统/环境代理处理。
    pub(crate) network_proxy_url: String,
    /// 市场索引 URL（None 表示用默认）。
    pub(crate) marketplace: crate::runtime::marketplace::MarketplaceState,
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
        let waker: std::sync::Arc<dyn tool_application::api::transport::RepaintWaker> =
            std::sync::Arc::new(move || {
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
        let default_theme_path = theme::builtin_theme_path(default_theme, &theme_dir);
        let mut loaded_theme = default_theme;
        let mut loaded_theme_path = default_theme_path.clone();
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
        let mut rp = config
            .as_ref()
            .map(|c| c.panels.clone())
            .unwrap_or_else(PanelManager::default_workspace);
        rp.discard_dynamic_tabs();
        rp.dock.normalize_tool_layout();
        rp.ensure_tiles_layout();
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

        let workbench = {
            let mut w = Workbench::new(bus.clone());
            w.set_transport_repaint_waker(waker);
            let plugin_dir = user_plugins_dir();
            tool_application::api::marketplace::retire_old_plugin_dirs(&plugin_dir);
            if let Some(cfg) = config.as_ref() {
                let ac = ApplicationConfig {
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
                w = w.with_config(ac);
            }
            // 初始插件扫描也走统一后台任务，避免启动阶段在 UI 线程读 manifest。
            let _ = w.dispatch(tool_application::AppCommand::DiscoverPlugins {
                roots: vec![plugin_dir],
            });
            w
        };
        let ui_events = workbench.subscribe_ui_events();
        let mut app = Self {
            workbench,
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
                pending_open_notice: None,
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
                network_ports: config
                    .as_ref()
                    .map(|c| c.network_ports.clone())
                    .unwrap_or_default(),
                network_host: String::new(),
                network_port: "7125".to_owned(),
            },
            recorder_path: config
                .as_ref()
                .map(|c| c.recorder_path.clone())
                .unwrap_or_else(default_recorder_path),
            panels: rp.clone(),
            notifications: NotificationQueue::new(),
            toast_overlay: ToastOverlay::default(),
            recent_workspaces: config
                .as_ref()
                .map(|c| c.recent_workspaces.clone())
                .unwrap_or_default(),
            send,
            layout_dirty: false,
            last_auto_save_time: 0.0,
            ui_events,
            replay_analyzer: Default::default(),
            periodic_send: Default::default(),
            keymap: config
                .as_ref()
                .map(|c| c.keymap.clone())
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
                .map(|c| c.monospace_font_size.clamp(10.0, 24.0))
                .unwrap_or(13.0),
            ui_theme: loaded_theme,
            theme_path: loaded_theme_path,
            theme_dir,
            network_proxy_url: config
                .as_ref()
                .and_then(|c| c.network_proxy_url.clone())
                .unwrap_or_default(),
            marketplace: Default::default(),
        };
        // 从配置恢复等宽字体大小
        app.terminal_panel.font_size = app.monospace_font_size;
        app.bottom_log_panel.font_size = app.monospace_font_size;
        // 从配置恢复终端/日志的数据参数
        if let Some(c) = config.as_ref() {
            app.terminal_panel.merge_window_ms = c.terminal_merge_window_ms;
            app.terminal_panel.set_max_entries(c.terminal_max_entries);
            app.bottom_log_panel.set_max_entries(c.log_max_entries);
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
    pub(crate) fn plugin_summaries(&self) -> &[tool_application::api::extension::PluginSummary] {
        self.plugin_summaries_cache
            .get_or_init(|| self.workbench.query_plugins().summaries)
    }
}

impl Drop for WorkbenchApp {
    fn drop(&mut self) {
        // 退出前自动保存工作区
        if let Err(e) = self.save_config() {
            log::warn!("save_config failed: {e}")
        };
        self.workbench.stop_recording();
        self.workbench.shutdown_serial();
    }
}

// ── UI 组件 ──

impl eframe::App for WorkbenchApp {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        theme::bg_primary().to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
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

        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let poll_interval_ms = if focused { 80 } else { 250 };
        ctx.request_repaint_after(std::time::Duration::from_millis(poll_interval_ms));
    }
}
