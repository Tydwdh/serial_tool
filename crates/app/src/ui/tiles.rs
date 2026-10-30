//! Native panel host for the shared egui_tiles shell.

use crate::app::WorkbenchApp;
use crate::panel_registry::BuiltinPanel;
use crate::shared_shell::DockHost;
use crate::state::StatusLevel;
use eframe::egui;
use tool_panels::{PluginPanelEvent, send_layout_for_width};

impl WorkbenchApp {
    fn render_platform_panel_body(&mut self, ui: &mut egui::Ui, kind: BuiltinPanel) {
        match kind {
            BuiltinPanel::Devices => {
                egui::ScrollArea::vertical()
                    .id_salt("scroll-devices")
                    .show(ui, |ui| self.device_panel(ui));
            }
            BuiltinPanel::Plugins => {
                let plugin_view = self.workbench.query_plugins();
                let summaries = plugin_view.summaries;
                let diagnostics = plugin_view.diagnostics;
                let events = egui::ScrollArea::vertical()
                    .id_salt("scroll-plugins")
                    .show(ui, |ui| {
                        self.plugins_panel
                            .ui_with_view(ui, &summaries, &diagnostics)
                    })
                    .inner;
                for plugin_id in self.plugins_panel.take_pending_restart() {
                    // 旧运行时还在退出就把重启请求留给下一帧。判定取自 PluginManager
                    // 的 stopping_plugins，而不是错误文案。
                    if self.workbench.plugin_is_stopping(&plugin_id) {
                        self.plugins_panel.push_pending_restart(plugin_id);
                    } else {
                        let _ = self
                            .workbench
                            .dispatch(tool_application::AppCommand::EnablePlugin { plugin_id });
                    }
                }
                self.handle_plugin_panel_events(events);
            }
            BuiltinPanel::Settings => {
                egui::ScrollArea::vertical()
                    .id_salt("scroll-settings")
                    .show(ui, |ui| self.settings_panel(ui));
            }
            BuiltinPanel::Sender => match send_layout_for_width(ui.available_width()) {
                tool_panels::SendLayout::Horizontal => self.send_panel_horizontal(ui),
                tool_panels::SendLayout::Vertical => self.send_panel_vertical(ui),
            },
            BuiltinPanel::Terminal | BuiltinPanel::Logs => {}
        }
    }

    pub(super) fn render_config_overlay(&mut self, ctx: &egui::Context) {
        let Some(page) = self.config_overlay else {
            return;
        };
        let viewport = ctx.content_rect();
        let default_size = egui::vec2(
            (viewport.width() * 0.82).clamp(680.0, 1160.0),
            (viewport.height() * 0.84).clamp(520.0, 900.0),
        );
        let default_pos = viewport.center() - default_size * 0.5;
        let mut open = true;
        egui::Window::new(page.title())
            .id(egui::Id::new("config-overlay"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(default_pos)
            .default_size(default_size)
            .min_size(egui::vec2(560.0, 360.0))
            .show(ctx, |ui| {
                self.render_platform_panel_body(ui, page.panel());
            });
        if !open {
            self.config_overlay = None;
        }
    }

    pub(crate) fn handle_plugin_panel_events(&mut self, events: Vec<PluginPanelEvent>) {
        for event in events {
            match event {
                PluginPanelEvent::Status(message, is_error) => {
                    let level = if is_error {
                        StatusLevel::Error
                    } else {
                        StatusLevel::Info
                    };
                    self.set_status_force(level, message);
                }
                PluginPanelEvent::Enable(id) => {
                    match self
                        .workbench
                        .dispatch(tool_application::AppCommand::EnablePlugin {
                            plugin_id: id.clone(),
                        }) {
                        Ok(_) => {
                            if let Err(error) = self.save_config() {
                                log::warn!("save_config after enabling plugin failed: {error}");
                            }
                            self.set_status_force(StatusLevel::Info, format!("插件 {id} 已启用"));
                        }
                        Err(error) => self.set_status_force(
                            StatusLevel::Error,
                            format!("启用插件 {id} 失败：{error}"),
                        ),
                    }
                }
                PluginPanelEvent::Disable(id) => {
                    match self
                        .workbench
                        .dispatch(tool_application::AppCommand::DisablePlugin {
                            plugin_id: id.clone(),
                        }) {
                        Ok(_) => {
                            if let Err(error) = self.save_config() {
                                log::warn!("save_config after disabling plugin failed: {error}");
                            }
                            self.set_status_force(StatusLevel::Info, format!("插件 {id} 正在禁用"));
                        }
                        Err(error) => self.set_status_force(
                            StatusLevel::Error,
                            format!("禁用插件 {id} 失败：{error}"),
                        ),
                    }
                }
                PluginPanelEvent::RefreshPlugins { root } => {
                    match self
                        .workbench
                        .dispatch(tool_application::AppCommand::DiscoverPlugins {
                            roots: vec![std::path::PathBuf::from(root)],
                        }) {
                        Ok(_) => self.set_status_force(StatusLevel::Info, "正在刷新插件..."),
                        Err(error) => self
                            .set_status_force(StatusLevel::Error, format!("刷新插件失败：{error}")),
                    }
                }
                PluginPanelEvent::RefreshMarket => self.start_marketplace_refresh(),
                PluginPanelEvent::ImportPlugin => self.set_status(
                    StatusLevel::Info,
                    "桌面端插件通过插件目录发现，无需导入文件",
                ),
                PluginPanelEvent::MarketplaceUrlChanged(_) => {}
                PluginPanelEvent::InstallPlugin(id) => {
                    // registry 的持有权在 app 运行时，面板只提供条目 id。
                    let entry = self.market_entry(&id);
                    match entry {
                        Some(entry) => self.start_marketplace_install(entry),
                        None => self.set_status(
                            StatusLevel::Warn,
                            format!("找不到插件 {id} 的市场条目，请先刷新市场"),
                        ),
                    }
                }
                PluginPanelEvent::UninstallPlugin(id) => self.uninstall_plugin(&id),
            }
        }
    }
}

impl DockHost for WorkbenchApp {
    type RenderStart = std::time::Instant;

    fn panels(&mut self) -> &mut tool_panels::PanelManager {
        &mut self.panels
    }

    fn panels_ref(&self) -> &tool_panels::PanelManager {
        &self.panels
    }

    fn panel_registry(&self) -> &crate::panel_registry::PanelRegistry {
        &self.panel_registry
    }

    fn shared_panels(&mut self) -> crate::shared_shell::SharedPanels<'_> {
        crate::shared_shell::SharedPanels {
            terminal: &mut self.terminal_panel,
            log: &mut self.bottom_log_panel,
            dynamic: &mut self.dynamic_panels,
        }
    }

    fn render_platform_panel(&mut self, ui: &mut egui::Ui, kind: BuiltinPanel) {
        self.render_platform_panel_body(ui, kind);
    }

    fn prepare_terminal_panel(&mut self) {
        self.terminal_panel
            .set_port_aliases(&self.serial.port_aliases);
    }

    fn panel_render_start(&self) -> Self::RenderStart {
        self.perf.begin_frame()
    }

    fn note_panel_render(&mut self, kind: BuiltinPanel, started: Self::RenderStart) {
        match kind {
            BuiltinPanel::Terminal => self.perf.record_terminal_render(started),
            BuiltinPanel::Logs => self.perf.record_log_render(started),
            _ => {}
        }
    }

    fn note_dynamic_panel_render(&mut self, started: Self::RenderStart) {
        self.perf.record_chart_render(started);
    }

    fn mark_layout_dirty(&mut self) {
        self.layout_dirty = true;
    }
}
