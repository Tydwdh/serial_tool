use crate::theme;
use egui::{Color32, ScrollArea, TextEdit};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use tool_extension::{
    PluginDiagnostic, PluginDiagnosticSeverity, PluginManager, PluginState, PluginSummary,
};
use tool_marketplace::{Registry, RegistryPlugin};

/// 插件面板顶部 tab。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginTab {
    Installed,
    Marketplace,
}

/// 插件面板向上返回的事件。一帧内可能产生多个。
#[derive(Debug, Clone)]
pub enum PluginPanelEvent {
    /// 状态栏反馈（消息内容，是否错误）——兼容原 ui() 返回值语义。
    Status(String, bool),
    /// 用户请求刷新市场索引。
    RefreshMarket,
    /// 用户请求安装/重装某个市场插件。
    InstallPlugin(String),
    /// 用户请求卸载某个已安装插件。
    UninstallPlugin(String),
}

/// 市场 UI 状态：registry 缓存、刷新/安装进度、错误信息、已安装 id 集合。
/// 由 app 在后台任务完成后通过 setter 回填，panels 自身只负责渲染。
#[derive(Default)]
pub struct MarketplaceState {
    pub registry: Option<Registry>,
    pub refreshing: bool,
    pub error: Option<String>,
    /// 正在安装的插件 id → 进度 0.0..1.0。
    pub installing: HashMap<String, f32>,
    /// 已安装插件 id 集合（由 app 每帧从 PluginManager summaries 回填）。
    pub installed_ids: BTreeSet<String>,
}

pub struct PluginsPanel {
    root: String,
    recently_disabled: Vec<String>,
    /// disable 后待重新启用的插件 ID（用于 restart）
    pending_restart: Vec<String>,
    /// 待确认卸载的插件 ID（点过一次「卸载」后进入确认态，再点「确认」才执行）
    pending_uninstall: Option<String>,
    tab: PluginTab,
    market: MarketplaceState,
}

impl PluginsPanel {
    pub fn new() -> Self {
        Self {
            root: "plugins".to_owned(),
            recently_disabled: Vec::new(),
            pending_restart: Vec::new(),
            pending_uninstall: None,
            tab: PluginTab::Installed,
            market: MarketplaceState::default(),
        }
    }

    pub fn take_recently_disabled(&mut self) -> Vec<String> {
        std::mem::take(&mut self.recently_disabled)
    }

    /// 由 app 在卸载/重装前 disable 插件后调用，把 id 入队，
    /// 以便 tick_plugin_lifecycle 经 take_recently_disabled() 清理动态面板/文件授权。
    pub fn recently_disabled_push(&mut self, id: String) {
        if !self.recently_disabled.contains(&id) {
            self.recently_disabled.push(id);
        }
    }

    // ── 市场 UI 状态 setter（供 app 回填） ──

    pub fn set_market_registry(&mut self, reg: Registry) {
        self.market.registry = Some(reg);
        self.market.refreshing = false;
        self.market.error = None;
    }

    pub fn set_market_error(&mut self, msg: String) {
        self.market.refreshing = false;
        self.market.error = Some(msg);
    }

    pub fn set_market_refreshing(&mut self, refreshing: bool) {
        self.market.refreshing = refreshing;
        if refreshing {
            self.market.error = None;
        }
    }

    pub fn mark_installing(&mut self, id: &str) {
        self.market.installing.insert(id.to_owned(), 0.0);
    }

    pub fn set_install_progress(&mut self, id: &str, pct: f32) {
        self.market
            .installing
            .insert(id.to_owned(), pct.clamp(0.0, 1.0));
    }

    pub fn clear_installing(&mut self, id: &str) {
        self.market.installing.remove(id);
    }

    /// 由 app 每帧调用，传入当前已发现的插件 id 集合。
    pub fn set_installed_ids(&mut self, ids: BTreeSet<String>) {
        self.market.installed_ids = ids;
    }

    /// 查找市场 registry 中某 id 的插件条目（clone 返回），供 app 安装时使用。
    pub fn find_market_plugin(&self, id: &str) -> Option<RegistryPlugin> {
        self.market
            .registry
            .as_ref()
            .and_then(|r| r.plugins.iter().find(|p| p.id == id).cloned())
    }

    /// 是否需要首次拉取市场索引（无缓存、无错误、未在刷新中）。
    pub fn market_needs_initial_fetch(&self) -> bool {
        self.market.registry.is_none() && self.market.error.is_none() && !self.market.refreshing
    }

    /// 渲染插件面板 UI，返回本帧产生的事件列表。
    pub fn ui(&mut self, ui: &mut egui::Ui, manager: &mut PluginManager) -> Vec<PluginPanelEvent> {
        let mut events = Vec::new();

        // 重试 pending restart：上一帧 disable 后等待线程退出，现在尝试 enable
        let pending: Vec<String> = std::mem::take(&mut self.pending_restart);
        for id in pending {
            if let Err(e) = manager.enable(&id) {
                // 如果还在关闭中，放回队列下帧重试
                if matches!(e, tool_extension::ExtensionError::Stopping(_)) {
                    self.pending_restart.push(id);
                }
            }
        }

        // ── tab 切换 ──
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.tab == PluginTab::Installed, "已安装")
                .clicked()
            {
                self.tab = PluginTab::Installed;
                self.pending_uninstall = None;
            }
            ui.separator();
            if ui
                .selectable_label(self.tab == PluginTab::Marketplace, "市场")
                .clicked()
            {
                self.tab = PluginTab::Marketplace;
                self.pending_uninstall = None;
            }
        });
        ui.separator();

        match self.tab {
            PluginTab::Installed => {
                if let Some(ev) = self.installed_tab(ui, manager) {
                    events.push(ev);
                }
            }
            PluginTab::Marketplace => {
                events.extend(self.marketplace_tab(ui));
            }
        }

        events
    }

    // ── 已安装 tab ──

    fn installed_tab(
        &mut self,
        ui: &mut egui::Ui,
        manager: &mut PluginManager,
    ) -> Option<PluginPanelEvent> {
        // ── 管理 ──
        let toolbar_status = egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::card_accent_bar(ui, theme::CARD_ACCENT_PLUGIN);
                    ui.label(egui::RichText::new("🔧 管理").heading());
                });
                ui.separator();
                ui.horizontal(|ui| -> Option<PluginPanelEvent> {
                    ui.label("根目录");
                    ui.add(TextEdit::singleline(&mut self.root).desired_width(240.0));
                    if ui.button("刷新").clicked() {
                        match manager.discover_roots([PathBuf::from(self.root.trim())]) {
                            Ok(count) => {
                                return Some(PluginPanelEvent::Status(
                                    format!("发现了 {count} 个插件"),
                                    false,
                                ));
                            }
                            Err(error) => {
                                return Some(PluginPanelEvent::Status(error.to_string(), true));
                            }
                        }
                    }
                    if ui.button("打开目录").clicked() {
                        let _ = open::that(&self.root);
                    }
                    None
                })
                .inner
            })
            .inner;

        let status = toolbar_status;

        let summaries = manager.summaries();
        let diagnostics = manager.diagnostics();

        if summaries.is_empty() && diagnostics.is_empty() {
            ui.add_space(8.0);
            ui.label("未找到插件");
            return status;
        }

        // ── 诊断 ──
        if !diagnostics.is_empty() {
            ui.add_space(8.0);
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        theme::card_accent_bar(ui, theme::YELLOW);
                        ui.label(egui::RichText::new("⚠ 诊断").heading());
                    });
                    ui.separator();
                    for diagnostic in diagnostics {
                        diagnostic_row(ui, diagnostic);
                    }
                });
        }

        if summaries.is_empty() {
            ui.add_space(8.0);
            ui.label("没有可加载插件");
            return status;
        }

        ui.add_space(8.0);

        let scroll_result = ScrollArea::vertical().auto_shrink([false, false]).show(
            ui,
            |ui| -> Option<PluginPanelEvent> {
                let mut row_status: Option<PluginPanelEvent> = None;
                for summary in summaries {
                    let s = self.plugin_row(ui, manager, summary);
                    if row_status.is_none() {
                        row_status = s;
                    }
                    ui.add_space(8.0);
                }
                row_status
            },
        );

        status.or(scroll_result.inner)
    }

    // ── 市场 tab ──

    fn marketplace_tab(&mut self, ui: &mut egui::Ui) -> Vec<PluginPanelEvent> {
        let mut events = Vec::new();

        // 首次进入市场 tab：自动触发一次索引拉取（条件保证只触发一次）。
        if self.market_needs_initial_fetch() {
            events.push(PluginPanelEvent::RefreshMarket);
        }

        // ── 工具栏：刷新按钮 + 状态 ──
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::card_accent_bar(ui, theme::CARD_ACCENT_PLUGIN);
                    ui.label(egui::RichText::new("🛒 市场").heading());
                });
                ui.separator();
                ui.horizontal(|ui| {
                    let refreshing = self.market.refreshing;
                    let btn = egui::Button::new(if refreshing {
                        "刷新中…"
                    } else {
                        "刷新市场"
                    });
                    if ui.add_enabled(!refreshing, btn).clicked() {
                        events.push(PluginPanelEvent::RefreshMarket);
                    }
                    if let Some(err) = &self.market.error {
                        ui.colored_label(theme::RED, egui::RichText::new(err).small());
                    } else if refreshing {
                        ui.label(egui::RichText::new("正在拉取市场索引…").small());
                    } else if let Some(reg) = &self.market.registry {
                        ui.label(
                            egui::RichText::new(format!(
                                "共 {} 个插件（更新于 {}）",
                                reg.plugins.len(),
                                reg.updated
                            ))
                            .small(),
                        );
                    }
                });
            });

        ui.add_space(8.0);

        let Some(reg) = self.market.registry.clone() else {
            if !self.market.refreshing {
                ui.label("尚未加载市场索引，点击「刷新市场」");
            }
            return events;
        };

        if reg.plugins.is_empty() {
            ui.label("市场暂无可用插件");
            return events;
        }

        let scroll_result = ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for plugin in &reg.plugins {
                    self.market_plugin_row(ui, plugin, &mut events);
                    ui.add_space(8.0);
                }
            });
        let _ = scroll_result;

        events
    }

    fn market_plugin_row(
        &mut self,
        ui: &mut egui::Ui,
        plugin: &RegistryPlugin,
        events: &mut Vec<PluginPanelEvent>,
    ) {
        let is_installed = self.market.installed_ids.contains(&plugin.id);
        let progress = self.market.installing.get(&plugin.id).copied();

        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                // 标题行
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&plugin.name)
                            .strong()
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.label(
                        egui::RichText::new(&plugin.id)
                            .monospace()
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.label(format!("v{}", plugin.version));
                    if is_installed {
                        ui.colored_label(theme::GREEN, "✓ 已安装");
                    }
                    if let Some(cat) = &plugin.category {
                        ui.label(
                            egui::RichText::new(cat)
                                .small()
                                .color(theme::TEXT_SECONDARY),
                        );
                    }
                });

                ui.separator();

                // 描述
                if let Some(desc) = &plugin.description {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(desc);
                    });
                }

                // 作者 / 大小 / 权限
                ui.horizontal_wrapped(|ui| {
                    if let Some(author) = &plugin.author {
                        ui.label("作者");
                        ui.label(
                            egui::RichText::new(author)
                                .monospace()
                                .color(theme::TEXT_PRIMARY),
                        );
                    }
                    ui.label(format!("{} 字节", plugin.size));
                    ui.label("权限");
                    ui.label(
                        egui::RichText::new(if plugin.permissions.is_empty() {
                            "none".to_owned()
                        } else {
                            plugin.permissions.join(", ")
                        })
                        .monospace()
                        .color(theme::TEXT_PRIMARY),
                    );
                });

                // 安装按钮 / 进度
                ui.horizontal(|ui| {
                    if let Some(pct) = progress {
                        // 安装中：显示进度条
                        let progress_bar = egui::ProgressBar::new(pct)
                            .text(format!("安装中… {:.0}%", pct * 100.0));
                        ui.add(progress_bar);
                    } else {
                        let label = if is_installed { "重装" } else { "安装" };
                        if ui.button(label).clicked() {
                            events.push(PluginPanelEvent::InstallPlugin(plugin.id.clone()));
                        }
                    }
                });
            });
    }

    fn plugin_row(
        &mut self,
        ui: &mut egui::Ui,
        manager: &mut PluginManager,
        summary: PluginSummary,
    ) -> Option<PluginPanelEvent> {
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                // 标题行
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&summary.name)
                            .strong()
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.label(
                        egui::RichText::new(&summary.id)
                            .monospace()
                            .color(theme::TEXT_SECONDARY),
                    );
                    ui.label(format!("v{}", summary.version));
                    ui.label(
                        egui::RichText::new(format!("{:?}", summary.state))
                            .color(state_color(summary.state)),
                    );
                });

                ui.separator();

                // 详情行
                ui.horizontal_wrapped(|ui| {
                    ui.label("运行时");
                    ui.label(
                        egui::RichText::new(&summary.runtime)
                            .monospace()
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.label("权限");
                    ui.label(
                        egui::RichText::new(if summary.permissions.is_empty() {
                            "none".to_owned()
                        } else {
                            summary.permissions.join(", ")
                        })
                        .monospace()
                        .color(theme::TEXT_PRIMARY),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("路径");
                    ui.label(
                        egui::RichText::new(summary.path.display().to_string())
                            .monospace()
                            .color(theme::TEXT_PRIMARY),
                    );
                });

                if !summary.contributes.panels.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("面板");
                        for panel in &summary.contributes.panels {
                            ui.label(
                                egui::RichText::new(format!("{} ({})", panel.title, panel.kind))
                                    .monospace()
                                    .color(theme::TEXT_PRIMARY),
                            );
                        }
                    });
                }

                // ── 命令状态（仅 Running 时展示对账结果） ──
                let declared = &summary.contributes.commands;
                let registered = &summary.registered_commands;
                let missing = &summary.missing_commands;
                let undeclared = &summary.undeclared_commands;
                let is_running = matches!(summary.state, PluginState::Running);

                if !declared.is_empty() || !registered.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("命令");
                        if is_running {
                            for cmd in declared {
                                if registered.iter().any(|r| r == &cmd.id) {
                                    ui.colored_label(theme::GREEN, format!("✓ {}", cmd.id));
                                } else if missing.iter().any(|m| m == &cmd.id) {
                                    ui.colored_label(theme::YELLOW, format!("⚠ {}", cmd.id))
                                        .on_hover_text(
                                            "此命令已在 manifest 声明但尚未注册 handler",
                                        );
                                } else {
                                    ui.label(cmd.id.as_str());
                                }
                            }
                            for cmd in undeclared {
                                ui.colored_label(theme::TEXT_SECONDARY, format!("ℹ {}", cmd))
                                    .on_hover_text("此命令在运行时动态注册，未在 manifest 声明");
                            }
                        } else {
                            for cmd in declared {
                                ui.label(
                                    egui::RichText::new(&cmd.id)
                                        .monospace()
                                        .color(theme::TEXT_PRIMARY),
                                );
                            }
                        }
                    });
                }

                if let Some(error) = &summary.last_error {
                    ui.colored_label(theme::RED, error);
                }

                // 按钮行
                ui.horizontal(|ui| -> Option<PluginPanelEvent> {
                    let can_enable =
                        !matches!(summary.state, PluginState::Running | PluginState::Enabled);
                    if ui
                        .add_enabled(can_enable, egui::Button::new("启用"))
                        .clicked()
                    {
                        match manager.enable(&summary.id) {
                            Ok(()) => {}
                            Err(error) => {
                                return Some(PluginPanelEvent::Status(error.to_string(), true));
                            }
                        }
                    }
                    let can_disable = matches!(
                        summary.state,
                        PluginState::Running
                            | PluginState::Enabled
                            | PluginState::Finished
                            | PluginState::Failed
                    );
                    if ui
                        .add_enabled(can_disable, egui::Button::new("禁用"))
                        .clicked()
                    {
                        match manager.disable(&summary.id) {
                            Ok(()) => {
                                self.recently_disabled.push(summary.id.clone());
                            }
                            Err(error) => {
                                return Some(PluginPanelEvent::Status(error.to_string(), true));
                            }
                        }
                    }
                    let can_restart = can_disable;
                    if ui
                        .add_enabled(can_restart, egui::Button::new("重启"))
                        .on_hover_text("禁用后重新启用")
                        .clicked()
                    {
                        if let Err(e) = manager.disable(&summary.id) {
                            return Some(PluginPanelEvent::Status(format!("禁用失败：{e}"), true));
                        }
                        self.pending_restart.push(summary.id.clone());
                    }

                    // 卸载按钮（两步确认，避免误删）：
                    // 第一次点「卸载」→ 进入确认态，按钮文案变为「确认卸载?」并伴随「取消」。
                    // 第二次点「确认卸载?」→ 发出 UninstallPlugin 事件。
                    let confirming = self.pending_uninstall.as_deref() == Some(&summary.id);
                    if confirming {
                        if ui
                            .add(
                                egui::Button::new("确认卸载?").fill(theme::RED.gamma_multiply(0.3)),
                            )
                            .clicked()
                        {
                            self.pending_uninstall = None;
                            return Some(PluginPanelEvent::UninstallPlugin(summary.id.clone()));
                        }
                        if ui.button("取消").clicked() {
                            self.pending_uninstall = None;
                        }
                    } else if ui.button("卸载").clicked() {
                        self.pending_uninstall = Some(summary.id.clone());
                    }
                    None
                })
                .inner
            })
            .inner
    }
}

impl Default for PluginsPanel {
    fn default() -> Self {
        Self::new()
    }
}

fn state_color(state: PluginState) -> Color32 {
    match state {
        PluginState::Discovered => theme::TEXT_SECONDARY,
        PluginState::Enabled | PluginState::Finished => theme::GREEN,
        PluginState::Running => theme::BLUE,
        PluginState::Failed => theme::RED,
        PluginState::Disabled => theme::YELLOW,
    }
}

fn diagnostic_row(ui: &mut egui::Ui, diagnostic: &PluginDiagnostic) {
    let color = match diagnostic.severity {
        PluginDiagnosticSeverity::Warning => theme::YELLOW,
        PluginDiagnosticSeverity::Error => theme::RED,
    };

    ui.horizontal_wrapped(|ui| {
        ui.colored_label(color, format!("{:?}", diagnostic.severity));
        ui.label(
            egui::RichText::new(&diagnostic.code)
                .monospace()
                .color(theme::TEXT_PRIMARY),
        );
        if let Some(plugin_id) = &diagnostic.plugin_id {
            ui.label(
                egui::RichText::new(plugin_id)
                    .monospace()
                    .color(theme::TEXT_PRIMARY),
            );
        }
        ui.label(&diagnostic.message);
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("路径");
        ui.label(
            egui::RichText::new(diagnostic.path.display().to_string())
                .monospace()
                .color(theme::TEXT_PRIMARY),
        );
    });
}
