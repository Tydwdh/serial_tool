//! Native panel host for the shared egui_tiles shell.

use crate::app::WorkbenchApp;
use crate::panel_registry::BuiltinPanel;
use crate::shared_shell::DockHost;
use crate::state::StatusLevel;
use eframe::egui;
use tool_panels::{PanelId, PluginPanelEvent, send_layout_for_width, theme};

impl WorkbenchApp {
    fn tile_panel_body(&mut self, ui: &mut egui::Ui, id: &PanelId) {
        self.panels.active_tab = id.clone();
        if !self.panel_registry.is_available(id) {
            ui.vertical_centered(|ui| {
                ui.add_space(32.0);
                ui.heading(self.panel_registry.title(id));
                ui.label("当前运行环境暂不支持此功能");
            });
            return;
        }
        match self.panel_registry.kind_for(id) {
            Some(crate::panel_registry::PanelKind::Builtin(kind)) => match kind {
                BuiltinPanel::Devices => {
                    egui::ScrollArea::vertical()
                        .id_salt("scroll-devices")
                        .show(ui, |ui| self.device_panel(ui));
                }
                BuiltinPanel::Replay => {
                    let replay_status = self.workbench.query_replay();
                    egui::ScrollArea::vertical()
                        .id_salt("scroll-replay")
                        .show(ui, |ui| self.replay_panel.ui(ui, &replay_status));
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
                    for id in self.plugins_panel.take_pending_restart() {
                        if let Err(tool_application::AppError::Plugin(message)) = self
                            .workbench
                            .dispatch(tool_application::AppCommand::EnablePlugin {
                                plugin_id: id.clone(),
                            })
                            && message.contains("shutting down")
                        {
                            self.plugins_panel.push_pending_restart(id);
                        }
                    }
                    self.handle_plugin_panel_events(events);
                }
                BuiltinPanel::Settings => {
                    egui::ScrollArea::vertical()
                        .id_salt("scroll-settings")
                        .show(ui, |ui| self.settings_panel(ui));
                }
                BuiltinPanel::Terminal => {
                    let started = self.perf.begin_frame();
                    self.terminal_panel
                        .set_port_aliases(&self.serial.port_aliases);
                    self.terminal_panel.ui(ui);
                    self.perf.record_terminal_render(started);
                }
                BuiltinPanel::Sender => match send_layout_for_width(ui.available_width()) {
                    tool_panels::SendLayout::Horizontal => self.send_panel_horizontal(ui),
                    tool_panels::SendLayout::Vertical => self.send_panel_vertical(ui),
                },
                BuiltinPanel::Logs => {
                    let started = self.perf.begin_frame();
                    self.bottom_log_panel.ui(ui);
                    self.perf.record_log_render(started);
                }
                BuiltinPanel::Chart => {
                    let started = self.perf.begin_frame();
                    self.chart_panel.ui(ui);
                    self.perf.record_chart_render(started);
                }
            },
            Some(crate::panel_registry::PanelKind::Dynamic { suffix }) => {
                if self.dynamic_panels.contains(&suffix) {
                    let started = self.perf.begin_frame();
                    let _ = egui::ScrollArea::vertical()
                        .show(ui, |ui| self.dynamic_panels.ui_body(ui, &suffix));
                    self.perf.record_chart_render(started);
                } else {
                    ui.colored_label(theme::red(), format!("动态面板不存在：{suffix}"));
                }
            }
            None => {
                ui.colored_label(theme::red(), format!("面板不存在：{id}"));
            }
        }
    }

    pub(crate) fn handle_plugin_panel_events(&mut self, events: Vec<PluginPanelEvent>) {
        for event in events {
            match event {
                PluginPanelEvent::Status(message, is_error) => {
                    self.set_status_force(
                        if is_error {
                            StatusLevel::Error
                        } else {
                            StatusLevel::Info
                        },
                        message,
                    );
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
                PluginPanelEvent::RefreshMarket => self.start_marketplace_refresh(),
                PluginPanelEvent::ImportPlugin => self.set_status(
                    StatusLevel::Info,
                    "桌面端插件通过插件目录发现，无需导入文件",
                ),
                PluginPanelEvent::MarketplaceUrlChanged(_) => {}
                PluginPanelEvent::InstallPlugin(id) => {
                    match self.plugins_panel.find_market_plugin(&id) {
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
    fn panels(&mut self) -> &mut tool_panels::PanelManager {
        &mut self.panels
    }

    fn panels_ref(&self) -> &tool_panels::PanelManager {
        &self.panels
    }

    fn panel_registry(&self) -> &crate::panel_registry::PanelRegistry {
        &self.panel_registry
    }

    fn render_panel(&mut self, ui: &mut egui::Ui, id: &PanelId) {
        self.tile_panel_body(ui, id);
    }

    fn mark_layout_dirty(&mut self) {
        self.layout_dirty = true;
    }
}
