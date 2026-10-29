use crate::config::{ConfigLoadResult, PersistedConfig, load_config, resolve_theme_path};
use crate::state::{MAX_SEND_HISTORY, NotificationQueue, SendUiState, SerialUiState, UpdateState};
use eframe::egui;
use std::collections::VecDeque;
use tool_application::{ApplicationConfig, Workbench};
use tool_core::{Event, LogLevel};
use tool_databus::DataBus;
use tool_marketplace::retire_old_plugin_dirs;
use tool_panels::{DynamicPanels, LogPanel, PanelManager, PluginsPanel, TerminalPanel, theme};
use tool_transport::RepaintWaker;

use crate::bootstrap::{apply_theme, setup_fonts, user_plugins_dir, user_themes_dir};
use crate::ui::toast::ToastOverlay;
pub(crate) use crate::workbench_app::WorkbenchApp;

// ══════════════════════════════════════════
//  WorkbenchApp impl
// ══════════════════════════════════════════

impl WorkbenchApp {
    /// composition root：启动期的副作用按固定顺序串起来，每段细节拆在本文件下方的私有函数里。
    ///
    /// 这里的调用顺序就是原 `new()` 的执行顺序，任何一步都不要重排：
    /// 主题先落地（否则首帧可能用默认主题）→ 注册字体 → 建 bus 与重绘 waker →
    /// 读配置（失败路径要往 bus 发日志）→ 建主题目录并解析主题 JSON（内部会再次 `apply_theme`）→
    /// 布局 / UI 状态 / Workbench → 组装 `Self` → 回填面板设置 → `refresh_ports` →
    /// 迁移回写配置 → 就绪日志。
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
        let (config, config_migrated, config_write_protected) = startup_config(&bus);
        let theme_dir = ensure_theme_dir();
        let (ui_theme, theme_path, theme_recovered) =
            activate_startup_theme(&cc.egui_ctx, config.as_ref(), &theme_dir);
        let panels = startup_panel_manager(config.as_ref());
        let (send, serial) = startup_ui_state(config.as_ref());
        let (workbench, ui_events) = startup_workbench(&bus, waker, config.as_ref());
        let plugins_panel = {
            let mut panel = PluginsPanel::new();
            panel.set_root(user_plugins_dir().display().to_string());
            panel
        };
        let mut app = Self {
            workbench,
            terminal_panel: TerminalPanel::new(&bus),
            dynamic_panels: DynamicPanels::new(&bus),
            plugins_panel,
            bottom_log_panel: LogPanel::new(&bus),
            serial,
            panels,
            notifications: NotificationQueue::new(),
            toast_overlay: ToastOverlay::default(),
            send,
            layout_dirty: false,
            last_auto_save_time: 0.0,
            ui_events,
            periodic_send: Default::default(),
            panel_registry: crate::panel_registry::PanelRegistry::builtin(),
            update_state: UpdateState::default(),
            contribution_states: std::collections::HashMap::new(),
            plugin_summaries_cache: std::cell::OnceCell::new(),
            monospace_font_size: config
                .as_ref()
                .map(|cfg| cfg.monospace_font_size.clamp(10.0, 24.0))
                .unwrap_or(13.0),
            ui_theme,
            theme_path,
            theme_dir,
            network_proxy_url: config
                .as_ref()
                .and_then(|cfg| cfg.network_proxy_url.clone())
                .unwrap_or_default(),
            marketplace: Default::default(),
            perf: crate::perf::PerfDiagnostics::default(),
            native_export: None,
        };
        apply_persisted_panel_settings(&mut app, config.as_ref());
        app.refresh_ports();
        rewrite_config_if_migrated(
            &app,
            config.as_ref(),
            config_migrated,
            theme_recovered,
            config_write_protected,
        );
        app.log(LogLevel::Info, "就绪");
        app
    }

    pub(crate) fn log(&self, lv: LogLevel, m: impl Into<String>) {
        self.workbench.log(lv, m);
    }

    /// 帧级缓存的插件 summaries。
    ///
    /// `summaries()` 会全量 clone 所有 manifest + 做命令对账；同帧内会被
    /// `ui_contribution_slot`（每 slot 一次）、插件面板、设置面板、
    /// 状态栏等多处调用。这里用一个 `OnceCell` 做帧内缓存：每帧首次调用
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

// ── new() 的分段实现 ──
//
// 下面的函数一一对应原 `new()` 里的连续片段，**自上而下就是启动顺序**。
// 拆出来只是为了可读，不要重排它们、也不要把某一步提前到另一段里：
// 中间几步之间有真实副作用（往 bus 发日志、写 ACTIVE_THEME、建目录、派发插件扫描命令）。

/// 读出持久化配置，并把三种失败路径（损坏 / 未来版本 / 缺失）作为系统日志发到总线上。
///
/// 返回 `(配置, 是否执行过迁移, 是否禁止写回)`：`NotFound` 与 `FutureVersion` 都退回默认配置，
/// 但只有后者要禁止本次写回，否则会覆盖更高版本程序写的文件。
/// 必须在 `DataBus` 建好之后调用——这几条是本进程最早的日志，晚于面板创建就会丢。
fn startup_config(bus: &DataBus) -> (Option<PersistedConfig>, bool, bool) {
    let config_result = load_config();
    match config_result {
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
    }
}

/// 准备用户主题目录（同时把内置主题 JSON 落盘）。建目录失败只告警，不阻断启动。
fn ensure_theme_dir() -> std::path::PathBuf {
    let theme_dir = user_themes_dir();
    if let Err(error) = theme::ensure_theme_directory(&theme_dir) {
        log::warn!("initialize theme directory failed: {error}");
    }
    theme_dir
}

/// 决定本次要激活的主题、加载对应 JSON，并在结论出来后立刻 `apply_theme`。
///
/// 返回 `(主题, 主题文件路径, 是否需要回写配置)`。末尾那次 `apply_theme` 必须留在本函数内、
/// 且晚于主题 JSON 的读取：`apply_theme` 会按当前 `ACTIVE_THEME` 取色重建全局 `Style`，
/// 提前调用会让首帧用错配色。
fn activate_startup_theme(
    ctx: &egui::Context,
    config: Option<&PersistedConfig>,
    theme_dir: &std::path::Path,
) -> (theme::AppTheme, Option<std::path::PathBuf>, bool) {
    let default_theme = theme::AppTheme::default();
    let mut loaded_theme = default_theme;
    let mut loaded_theme_path = theme::builtin_theme_path(default_theme, theme_dir);
    let mut theme_recovered = false;
    if let Some(cfg) = config {
        if let Some(path) = cfg
            .theme_path
            .as_deref()
            .map(|path| resolve_theme_path(theme_dir, path))
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
                    if let Err(fallback_error) = theme::load_builtin_theme(default_theme, theme_dir)
                    {
                        log::warn!("load fallback bundled theme failed: {fallback_error}");
                    }
                }
            }
        } else if let Err(error) = theme::load_builtin_theme(cfg.ui_theme, theme_dir) {
            log::warn!("load legacy bundled theme failed: {error}");
        } else {
            loaded_theme = cfg.ui_theme;
            loaded_theme_path = theme::builtin_theme_path(cfg.ui_theme, theme_dir);
        }
    } else if let Err(error) = theme::load_builtin_theme(default_theme, theme_dir) {
        log::warn!("load default bundled theme failed: {error}");
    }
    apply_theme(ctx, loaded_theme);
    (loaded_theme, loaded_theme_path, theme_recovered)
}

/// 恢复面板布局。三步顺序有依赖：先丢掉不跨会话的动态 tab，再归一化工具布局，
/// 最后由 `ensure_tiles_layout` 兜住归一化后可能变空的结构。
fn startup_panel_manager(config: Option<&PersistedConfig>) -> PanelManager {
    let mut panel_manager = config
        .map(|cfg| cfg.panels.clone())
        .unwrap_or_else(PanelManager::default_workspace);
    panel_manager.discard_dynamic_tabs();
    panel_manager.dock.normalize_tool_layout();
    panel_manager.ensure_tiles_layout();
    panel_manager
}

/// 串口/发送相关 UI 状态：没有配置文件时取 `default()`，
/// 有配置时只覆盖这些持久化字段，其余（端口列表、待重连等）仍是运行时初值。
fn startup_ui_state(config: Option<&PersistedConfig>) -> (SendUiState, SerialUiState) {
    let mut send = SendUiState::default();
    let mut serial = SerialUiState::default();
    if let Some(cfg) = config {
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
    (send, serial)
}

/// 构造领域侧 `Workbench`：注入 bus 与重绘 waker、套用配置、发起首次插件扫描并订阅 UI 事件。
///
/// waker 只能构造后注入（`Workbench::new` 不收这个参数），所以紧跟在 `new` 之后；
/// 插件扫描走命令派发，返回的 `ui_events` 必须在扫描派发之后订阅，否则会漏掉首帧的发现结果。
fn startup_workbench(
    bus: &DataBus,
    waker: std::sync::Arc<dyn RepaintWaker>,
    config: Option<&PersistedConfig>,
) -> (Workbench, tool_application::UiEventSubscriptions) {
    let mut workbench = Workbench::new(bus.clone());
    workbench.set_transport_repaint_waker(waker);
    let plugin_dir = user_plugins_dir();
    retire_old_plugin_dirs(&plugin_dir);
    if let Some(cfg) = config {
        let application_config = application_config_from(cfg);
        workbench = workbench.with_config(application_config);
    }
    // 初始插件扫描也走统一后台任务，避免启动阶段在 UI 线程读 manifest。
    let _ = workbench.dispatch(tool_application::AppCommand::DiscoverPlugins {
        roots: vec![plugin_dir],
    });
    let ui_events = workbench.subscribe_ui_events();
    (workbench, ui_events)
}

/// 持久化配置到 `tool-application` 启动配置的逐字段映射；缺省值全部由 serde 默认负责，
/// 这里不做任何兜底或转换，避免两端配置语义分叉。
fn application_config_from(cfg: &PersistedConfig) -> ApplicationConfig {
    ApplicationConfig {
        selected_port: cfg.selected_port.clone(),
        baud_rate: cfg.baud_rate.clone(),
        data_bits: cfg.data_bits.clone(),
        stop_bits: cfg.stop_bits.clone(),
        parity: cfg.parity.clone(),
        auto_reconnect: cfg.auto_reconnect,
        terminal_merge_window_ms: cfg.terminal_merge_window_ms,
        terminal_max_entries: cfg.terminal_max_entries,
        log_max_entries: cfg.log_max_entries,
        network_ports: cfg.network_ports.clone(),
        port_aliases: cfg.port_aliases.clone(),
        port_groups: cfg.port_groups.clone(),
        enabled_plugins: cfg.enabled_plugins.clone(),
        network_proxy_url: cfg.network_proxy_url.clone(),
    }
}

/// 等宽字号与终端/日志的数据参数不属于 UI 状态结构，只能等 `Self` 组装完再回填，
/// 因此必须在 `refresh_ports` 之前执行（面板的条目上限会影响后续入队行为）。
fn apply_persisted_panel_settings(app: &mut WorkbenchApp, config: Option<&PersistedConfig>) {
    // 从配置恢复等宽字体大小
    app.terminal_panel.font_size = app.monospace_font_size;
    app.bottom_log_panel.font_size = app.monospace_font_size;
    // 从配置恢复终端/日志的数据参数
    if let Some(cfg) = config {
        app.terminal_panel.merge_window_ms = cfg.terminal_merge_window_ms;
        app.terminal_panel.set_max_entries(cfg.terminal_max_entries);
        app.bottom_log_panel.set_max_entries(cfg.log_max_entries);
    }
}

/// 迁移过、主题 JSON 坏过、或老配置里还没有 `theme_path`，都要在启动末尾回写一次配置；
/// `write_protected`（来自更高版本的配置）时绝不覆盖用户的文件。
///
/// 判定顺序保持短路求值：先排除写保护，再看三个需要回写的理由。
/// 调用点必须在 `refresh_ports()` 之后，回写的内容才包含本次枚举到的端口状态。
fn rewrite_config_if_migrated(
    app: &WorkbenchApp,
    config: Option<&PersistedConfig>,
    config_migrated: bool,
    theme_recovered: bool,
    config_write_protected: bool,
) {
    let should_persist_config = !config_write_protected
        && (config_migrated
            || theme_recovered
            || config.is_none_or(|cfg| cfg.theme_path.is_none()));
    if should_persist_config && let Err(error) = app.save_config() {
        log::warn!("persist theme path migration failed: {error}");
    }
}

impl Drop for WorkbenchApp {
    fn drop(&mut self) {
        // 退出前自动保存工作区
        if let Err(error) = self.save_config() {
            log::warn!("save_config failed: {error}")
        }
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
