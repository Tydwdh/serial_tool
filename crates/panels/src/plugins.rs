use crate::{
    copy_text_with_feedback,
    design::{self, ButtonKind},
    theme,
};
use egui::{Color32, ScrollArea, TextEdit};
use egui_material_icons::icons::{
    ICON_APPS, ICON_CANCEL, ICON_CONTENT_COPY, ICON_DELETE, ICON_DIAGNOSIS, ICON_DOWNLOAD,
    ICON_FOLDER_OPEN, ICON_MANAGE_ACCOUNTS, ICON_OPEN_IN_NEW, ICON_REFRESH, ICON_RESTART_ALT,
    ICON_SEARCH, ICON_SHOPPING_CART, ICON_TOGGLE_OFF, ICON_TOGGLE_ON,
};
use std::collections::{BTreeSet, HashMap};
use tool_application::tool_extension::{
    PluginDiagnostic, PluginDiagnosticSeverity, PluginState, PluginSummary,
};
use tool_application::tool_marketplace::{Registry, RegistryPlugin};

const TWO_COLUMN_PLUGIN_WIDTH: f32 = 980.0;
const TWO_COLUMN_CARD_GAP: f32 = 10.0;
const PLUGIN_METRIC_WIDTH: f32 = 104.0;

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
    /// 用户请求启用插件。
    Enable(String),
    /// 用户请求禁用插件。
    Disable(String),
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
    /// 最近一次成功刷新实际使用的网络路径。
    pub network_diagnostics: Option<String>,
    /// 正在安装的插件 id → 进度 0.0..1.0。
    pub installing: HashMap<String, f32>,
    /// 已安装插件 id 集合（由 app 每帧从 PluginManager summaries 回填）。
    pub installed_ids: BTreeSet<String>,
}

pub struct PluginsPanel {
    root: String,
    /// disable 后待重新启用的插件 ID（用于 restart）
    pending_restart: Vec<String>,
    /// 待确认卸载的插件 ID（点过一次「卸载」后进入确认态，再点「确认」才执行）
    pending_uninstall: Option<String>,
    tab: PluginTab,
    market: MarketplaceState,
    market_search: String,
    market_category: Option<String>,
}

impl PluginsPanel {
    pub fn new() -> Self {
        Self {
            root: "plugins".to_owned(),
            pending_restart: Vec::new(),
            pending_uninstall: None,
            tab: PluginTab::Installed,
            market: MarketplaceState::default(),
            market_search: String::new(),
            market_category: None,
        }
    }

    // ── 市场 UI 状态 setter（供 app 回填） ──

    pub fn set_market_registry(&mut self, reg: Registry, network_diagnostics: String) {
        self.market.registry = Some(reg);
        self.market.refreshing = false;
        self.market.error = None;
        self.market.network_diagnostics = Some(network_diagnostics);
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

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        summaries: &[PluginSummary],
        diagnostics: &[PluginDiagnostic],
    ) -> Vec<PluginPanelEvent> {
        self.ui_with_view(ui, summaries, diagnostics)
    }

    /// DTO 驱动的渲染（不持有 PluginManager）。
    pub fn ui_with_view(
        &mut self,
        ui: &mut egui::Ui,
        summaries: &[PluginSummary],
        diagnostics: &[PluginDiagnostic],
    ) -> Vec<PluginPanelEvent> {
        let mut events = Vec::new();

        // ── tab 切换 ──
        design::elevated_card().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(
                        egui::Button::selectable(
                            self.tab == PluginTab::Installed,
                            design::icon_text(ICON_APPS, "已安装"),
                        )
                        .corner_radius(7.0)
                        .min_size(egui::vec2(112.0, 32.0)),
                    )
                    .clicked()
                {
                    self.tab = PluginTab::Installed;
                    self.pending_uninstall = None;
                }
                if ui
                    .add(
                        egui::Button::selectable(
                            self.tab == PluginTab::Marketplace,
                            design::icon_text(ICON_SHOPPING_CART, "市场"),
                        )
                        .corner_radius(7.0)
                        .min_size(egui::vec2(112.0, 32.0)),
                    )
                    .clicked()
                {
                    self.tab = PluginTab::Marketplace;
                    self.pending_uninstall = None;
                }
            });
        });
        ui.add_space(design::SECTION_GAP);

        match self.tab {
            PluginTab::Installed => {
                if let Some(ev) = self.installed_tab_inner(ui, summaries, diagnostics) {
                    events.push(ev);
                }
            }
            PluginTab::Marketplace => {
                events.extend(self.marketplace_tab(ui));
            }
        }

        events
    }

    pub fn take_pending_restart(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_restart)
    }

    pub fn push_pending_restart(&mut self, id: String) {
        self.pending_restart.push(id);
    }

    // ── 已安装 tab（DTO 版） ──

    fn installed_tab_inner(
        &mut self,
        ui: &mut egui::Ui,
        summaries: &[PluginSummary],
        diagnostics: &[PluginDiagnostic],
    ) -> Option<PluginPanelEvent> {
        // ── 管理 ──
        let toolbar_status = design::card()
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_MANAGE_ACCOUNTS, "插件管理");
                ui.separator();
                ui.horizontal_wrapped(|ui| -> Option<PluginPanelEvent> {
                    ui.label("根目录");
                    ui.add(TextEdit::singleline(&mut self.root).desired_width(240.0));
                    if design::button(ui, ICON_REFRESH, "刷新", ButtonKind::Primary).clicked() {
                        // 实际刷新由 Workbench 处理，Panel 只发状态提示
                        return Some(PluginPanelEvent::Status("刷新已请求".to_owned(), false));
                    }
                    if design::button(ui, ICON_FOLDER_OPEN, "打开目录", ButtonKind::Secondary)
                        .clicked()
                    {
                        let _ = open::that(&self.root);
                    }
                    None
                })
                .inner
            })
            .inner;

        let status = toolbar_status;

        if summaries.is_empty() && diagnostics.is_empty() {
            ui.add_space(8.0);
            design::empty_state(ui, ICON_APPS, "未找到插件");
            return status;
        }

        if !summaries.is_empty() {
            let running = summaries
                .iter()
                .filter(|summary| matches!(summary.state, PluginState::Running))
                .count();
            let failed = summaries
                .iter()
                .filter(|summary| matches!(summary.state, PluginState::Failed))
                .count();
            let inactive = summaries.len().saturating_sub(running + failed);
            ui.add_space(8.0);
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    design::status_pill_sized(
                        ui,
                        theme::cyan(),
                        format!("已发现 {}", summaries.len()),
                        PLUGIN_METRIC_WIDTH,
                    );
                    design::status_pill_sized(
                        ui,
                        theme::green(),
                        format!("运行 {running}"),
                        PLUGIN_METRIC_WIDTH,
                    );
                    design::status_pill_sized(
                        ui,
                        theme::text_secondary(),
                        format!("未运行 {inactive}"),
                        PLUGIN_METRIC_WIDTH,
                    );
                    if failed > 0 {
                        design::status_pill_sized(
                            ui,
                            theme::red(),
                            format!("异常 {failed}"),
                            PLUGIN_METRIC_WIDTH,
                        );
                    }
                    if !diagnostics.is_empty() {
                        design::status_pill_sized(
                            ui,
                            theme::yellow(),
                            format!("诊断 {}", diagnostics.len()),
                            PLUGIN_METRIC_WIDTH,
                        );
                    }
                });
            });
        }

        // ── 诊断 ──
        if !diagnostics.is_empty() {
            ui.add_space(8.0);
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_DIAGNOSIS, "诊断");
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
                if plugin_column_count(ui.available_width()) == 2 {
                    let card_width =
                        ((ui.available_width() - TWO_COLUMN_CARD_GAP) / 2.0).max(320.0);
                    let mut summaries = summaries.iter();
                    while let Some(left) = summaries.next() {
                        let right = summaries.next();
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = TWO_COLUMN_CARD_GAP;
                            ui.vertical(|ui| {
                                ui.set_width(card_width);
                                let event = self.plugin_row(ui, left);
                                if row_status.is_none() {
                                    row_status = event;
                                }
                            });
                            if let Some(right) = right {
                                ui.vertical(|ui| {
                                    ui.set_width(card_width);
                                    let event = self.plugin_row(ui, right);
                                    if row_status.is_none() {
                                        row_status = event;
                                    }
                                });
                            } else {
                                ui.allocate_space(egui::vec2(card_width, 0.0));
                            }
                        });
                        ui.add_space(TWO_COLUMN_CARD_GAP);
                    }
                } else {
                    for summary in summaries {
                        let event = self.plugin_row(ui, summary);
                        if row_status.is_none() {
                            row_status = event;
                        }
                        ui.add_space(8.0);
                    }
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
        design::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            design::section_header(ui, ICON_SHOPPING_CART, "插件市场");
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                let refreshing = self.market.refreshing;
                let refresh_label = if refreshing {
                    "刷新中…"
                } else {
                    "刷新市场"
                };
                if ui
                    .add_enabled_ui(!refreshing, |ui| {
                        design::button(ui, ICON_REFRESH, refresh_label, ButtonKind::Primary)
                    })
                    .inner
                    .clicked()
                {
                    events.push(PluginPanelEvent::RefreshMarket);
                }
                if let Some(err) = &self.market.error {
                    ui.colored_label(theme::red(), egui::RichText::new(err).small());
                } else if refreshing {
                    ui.label(egui::RichText::new("正在拉取市场索引…").small());
                }
            });

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(design::icon_only(
                    ICON_SEARCH,
                    theme::text_secondary(),
                    18.0,
                ));
                ui.add(
                    TextEdit::singleline(&mut self.market_search)
                        .desired_width(240.0)
                        .hint_text("搜索名称、ID、描述或作者"),
                );
                let categories: BTreeSet<String> = self
                    .market
                    .registry
                    .as_ref()
                    .into_iter()
                    .flat_map(|registry| registry.plugins.iter())
                    .filter_map(|plugin| plugin.category.clone())
                    .collect();
                egui::ComboBox::from_id_salt("plugin-market-category")
                    .selected_text(self.market_category.as_deref().unwrap_or("全部分类"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.market_category, None, "全部分类");
                        for category in categories {
                            ui.selectable_value(
                                &mut self.market_category,
                                Some(category.clone()),
                                category,
                            );
                        }
                    });
            });
        });

        ui.add_space(8.0);

        let Some(reg) = self.market.registry.clone() else {
            if !self.market.refreshing {
                design::empty_state(ui, ICON_SHOPPING_CART, "尚未加载市场索引");
            }
            return events;
        };

        if reg.plugins.is_empty() {
            design::empty_state(ui, ICON_SHOPPING_CART, "市场暂无插件");
            return events;
        }

        let query = crate::search::SearchQuery::new(&self.market_search, false);
        let visible_plugins: Vec<&RegistryPlugin> = reg
            .plugins
            .iter()
            .filter(|plugin| {
                let category_matches = self
                    .market_category
                    .as_ref()
                    .is_none_or(|category| plugin.category.as_ref() == Some(category));
                let query_matches = query.is_empty()
                    || query.matches(&plugin.name)
                    || query.matches(&plugin.id)
                    || plugin
                        .description
                        .as_deref()
                        .is_some_and(|text| query.matches(text))
                    || plugin
                        .author
                        .as_deref()
                        .is_some_and(|text| query.matches(text));
                category_matches && query_matches
            })
            .collect();

        if visible_plugins.is_empty() {
            design::empty_state(ui, ICON_SEARCH, "没有匹配插件");
            return events;
        }

        let scroll_result = ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if plugin_column_count(ui.available_width()) == 2 {
                    let card_width =
                        ((ui.available_width() - TWO_COLUMN_CARD_GAP) / 2.0).max(320.0);
                    let mut plugins = visible_plugins.into_iter();
                    while let Some(left) = plugins.next() {
                        let right = plugins.next();
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = TWO_COLUMN_CARD_GAP;
                            ui.vertical(|ui| {
                                ui.set_width(card_width);
                                self.market_plugin_row(ui, left, &mut events);
                            });
                            if let Some(right) = right {
                                ui.vertical(|ui| {
                                    ui.set_width(card_width);
                                    self.market_plugin_row(ui, right, &mut events);
                                });
                            } else {
                                ui.allocate_space(egui::vec2(card_width, 0.0));
                            }
                        });
                        ui.add_space(TWO_COLUMN_CARD_GAP);
                    }
                } else {
                    for plugin in visible_plugins {
                        self.market_plugin_row(ui, plugin, &mut events);
                        ui.add_space(8.0);
                    }
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

        design::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());

            // 标题行
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(&plugin.name)
                        .strong()
                        .color(theme::text_primary()),
                );
                ui.label(
                    egui::RichText::new(&plugin.id)
                        .monospace()
                        .color(theme::text_secondary()),
                );
                ui.label(format!("v{}", plugin.version));
                if is_installed {
                    design::status_pill(ui, theme::green(), "已安装");
                }
                if let Some(cat) = &plugin.category {
                    design::badge(ui, cat, theme::blue());
                }
            });

            ui.separator();

            // 描述
            if let Some(desc) = &plugin.description {
                ui.horizontal_wrapped(|ui| {
                    ui.label(desc);
                });
            }

            // 作者 / 大小
            ui.horizontal_wrapped(|ui| {
                if let Some(author) = &plugin.author {
                    ui.label("作者");
                    ui.label(
                        egui::RichText::new(author)
                            .monospace()
                            .color(theme::text_primary()),
                    );
                }
                ui.label(format!("{} 字节", plugin.size));
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("权限");
                if plugin.permissions.is_empty() {
                    design::badge(ui, "无额外权限", theme::green());
                } else {
                    for permission in &plugin.permissions {
                        design::badge(ui, permission, theme::orange());
                    }
                }
            });

            // 安装按钮 / 进度
            ui.horizontal_wrapped(|ui| {
                if let Some(pct) = progress {
                    // 安装中：显示进度条
                    let progress_bar =
                        egui::ProgressBar::new(pct).text(format!("安装中… {:.0}%", pct * 100.0));
                    ui.add(progress_bar);
                } else {
                    let label = if is_installed { "重装" } else { "安装" };
                    if design::button(ui, ICON_DOWNLOAD, label, ButtonKind::Primary).clicked() {
                        events.push(PluginPanelEvent::InstallPlugin(plugin.id.clone()));
                    }
                }
            });
        });
    }

    fn plugin_row(
        &mut self,
        ui: &mut egui::Ui,
        summary: &PluginSummary,
    ) -> Option<PluginPanelEvent> {
        design::card()
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());

                // 标题行
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(&summary.name)
                            .strong()
                            .color(theme::text_primary()),
                    );
                    ui.label(
                        egui::RichText::new(&summary.id)
                            .monospace()
                            .color(theme::text_secondary()),
                    );
                    ui.label(format!("v{}", summary.version));
                    design::status_pill(ui, state_color(summary.state), state_label(summary.state));
                });

                ui.separator();

                // 详情行
                ui.horizontal_wrapped(|ui| {
                    ui.label("运行时");
                    ui.label(
                        egui::RichText::new(&summary.runtime)
                            .monospace()
                            .color(theme::text_primary()),
                    );
                    ui.label("API");
                    ui.label(
                        egui::RichText::new(&summary.api_version)
                            .monospace()
                            .color(theme::text_primary()),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("权限");
                    if summary.permissions.is_empty() {
                        design::badge(ui, "无额外权限", theme::green());
                    } else {
                        for permission in &summary.permissions {
                            design::badge(ui, permission, theme::orange());
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("路径");
                    let full_path = summary.path.display().to_string();
                    let available_width = ui.available_width().max(40.0);
                    let display_path =
                        crate::compact_middle(&full_path, plugin_path_char_limit(available_width));
                    ui.add_sized(
                        egui::vec2(available_width, ui.spacing().interact_size.y),
                        egui::Label::new(
                            egui::RichText::new(display_path)
                                .monospace()
                                .color(theme::text_primary()),
                        )
                        .truncate(),
                    )
                    .on_hover_text(full_path);
                });

                if !summary.contributes.panels.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("面板");
                        for panel in &summary.contributes.panels {
                            ui.label(
                                egui::RichText::new(format!("{} ({})", panel.title, panel.kind))
                                    .monospace()
                                    .color(theme::text_primary()),
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
                                    ui.colored_label(theme::green(), format!("✓ {}", cmd.id));
                                } else if missing.iter().any(|m| m == &cmd.id) {
                                    design::badge(ui, &cmd.id, theme::yellow()).on_hover_text(
                                        "此命令已在 manifest 声明但尚未注册 handler",
                                    );
                                } else {
                                    ui.label(cmd.id.as_str());
                                }
                            }
                            for cmd in undeclared {
                                design::badge(ui, cmd, theme::text_secondary())
                                    .on_hover_text("此命令在运行时动态注册，未在 manifest 声明");
                            }
                        } else {
                            for cmd in declared {
                                ui.label(
                                    egui::RichText::new(&cmd.id)
                                        .monospace()
                                        .color(theme::text_primary()),
                                );
                            }
                        }
                    });
                }

                if let Some(error) = &summary.last_error {
                    ui.colored_label(theme::red(), error);
                }

                // 按钮行
                ui.horizontal_wrapped(|ui| -> Option<PluginPanelEvent> {
                    // 启用/禁用合并为单个按钮：当前正在运行/已启用则显示「禁用」并执行禁用，
                    // 否则显示「启用」并执行启用。
                    // 注意：Finished/Failed 视为未启用 —— disable() 对这两个状态是 no-op
                    // （运行时早已移除），因此把它们归入「启用」分支，保留直接重新启用的入口。
                    let is_active =
                        matches!(summary.state, PluginState::Running | PluginState::Enabled);
                    if is_active {
                        if design::button(ui, ICON_TOGGLE_OFF, "禁用", ButtonKind::Secondary)
                            .clicked()
                        {
                            return Some(PluginPanelEvent::Disable(summary.id.clone()));
                        }
                    } else if design::button(ui, ICON_TOGGLE_ON, "启用", ButtonKind::Primary)
                        .clicked()
                    {
                        return Some(PluginPanelEvent::Enable(summary.id.clone()));
                    }
                    // 重启语义：Running/Enabled 先 disable 再 enable（走 pending_restart 队列等线程退出）；
                    // Disabled/Finished/Failed 已无运行时，直接 enable 即可重新拉起。
                    // 因此「重启」对所有非 Discovered 状态都可点。
                    let can_restart = !matches!(summary.state, PluginState::Discovered);
                    if ui
                        .add_enabled_ui(can_restart, |ui| {
                            design::button(ui, ICON_RESTART_ALT, "重启", ButtonKind::Ghost)
                        })
                        .inner
                        .on_hover_text("禁用后重新启用")
                        .clicked()
                    {
                        if is_active {
                            self.pending_restart.push(summary.id.clone());
                            return Some(PluginPanelEvent::Disable(summary.id.clone()));
                        } else {
                            return Some(PluginPanelEvent::Enable(summary.id.clone()));
                        }
                    }

                    // 卸载按钮（两步确认，避免误删）：
                    // 第一次点「卸载」→ 进入确认态，按钮文案变为「确认卸载?」并伴随「取消」。
                    // 第二次点「确认卸载?」→ 发出 UninstallPlugin 事件。
                    let confirming = self.pending_uninstall.as_deref() == Some(&summary.id);
                    if confirming {
                        if design::button(ui, ICON_DELETE, "确认卸载?", ButtonKind::Danger)
                            .clicked()
                        {
                            self.pending_uninstall = None;
                            return Some(PluginPanelEvent::UninstallPlugin(summary.id.clone()));
                        }
                        if design::button(ui, ICON_CANCEL, "取消", ButtonKind::Ghost).clicked() {
                            self.pending_uninstall = None;
                        }
                    } else if design::button(ui, ICON_DELETE, "卸载", ButtonKind::Danger).clicked()
                    {
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
        PluginState::Discovered => theme::text_secondary(),
        PluginState::Enabled | PluginState::Finished => theme::green(),
        PluginState::Running => theme::blue(),
        PluginState::Failed => theme::red(),
        PluginState::Disabled => theme::yellow(),
    }
}

fn plugin_column_count(available_width: f32) -> usize {
    usize::from(available_width >= TWO_COLUMN_PLUGIN_WIDTH) + 1
}

fn plugin_path_char_limit(available_width: f32) -> usize {
    (available_width / 8.5).floor().max(12.0) as usize
}

fn state_label(state: PluginState) -> &'static str {
    match state {
        PluginState::Discovered => "已发现",
        PluginState::Enabled => "已启用",
        PluginState::Running => "运行中",
        PluginState::Finished => "已结束",
        PluginState::Failed => "运行失败",
        PluginState::Disabled => "已禁用",
    }
}

fn diagnostic_row(ui: &mut egui::Ui, diagnostic: &PluginDiagnostic) {
    let color = match diagnostic.severity {
        PluginDiagnosticSeverity::Warning => theme::yellow(),
        PluginDiagnosticSeverity::Error => theme::red(),
    };

    ui.horizontal_wrapped(|ui| {
        let severity = match diagnostic.severity {
            PluginDiagnosticSeverity::Warning => "警告",
            PluginDiagnosticSeverity::Error => "错误",
        };
        design::badge(ui, severity, color);
        ui.label(
            egui::RichText::new(&diagnostic.code)
                .monospace()
                .color(theme::text_primary()),
        );
        if let Some(plugin_id) = &diagnostic.plugin_id {
            ui.label(
                egui::RichText::new(plugin_id)
                    .monospace()
                    .color(theme::text_primary()),
            );
        }
        ui.label(&diagnostic.message);
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("路径");
        ui.label(
            egui::RichText::new(diagnostic.path.display().to_string())
                .monospace()
                .color(theme::text_primary()),
        );
        if design::button(ui, ICON_CONTENT_COPY, "复制", ButtonKind::Ghost).clicked() {
            copy_text_with_feedback(
                ui,
                diagnostic.path.display().to_string(),
                "已复制插件诊断路径",
            );
        }
        if design::button(ui, ICON_OPEN_IN_NEW, "打开位置", ButtonKind::Ghost).clicked() {
            let target = if diagnostic.path.is_file() {
                diagnostic.path.parent().unwrap_or(&diagnostic.path)
            } else {
                &diagnostic.path
            };
            let _ = open::that(target);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_plugin_state_has_a_localized_label() {
        for state in [
            PluginState::Discovered,
            PluginState::Enabled,
            PluginState::Running,
            PluginState::Finished,
            PluginState::Failed,
            PluginState::Disabled,
        ] {
            assert!(!state_label(state).is_empty());
            assert!(!state_label(state).is_ascii());
        }
    }

    #[test]
    fn plugin_cards_switch_to_two_columns_only_when_wide() {
        assert_eq!(plugin_column_count(TWO_COLUMN_PLUGIN_WIDTH - 1.0), 1);
        assert_eq!(plugin_column_count(TWO_COLUMN_PLUGIN_WIDTH), 2);
        assert_eq!(plugin_column_count(1440.0), 2);
    }

    #[test]
    fn plugin_path_limit_scales_with_card_width() {
        assert_eq!(plugin_path_char_limit(80.0), 12);
        assert!(plugin_path_char_limit(700.0) > plugin_path_char_limit(350.0));
    }
}
