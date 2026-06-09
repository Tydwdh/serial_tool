use crate::theme;
use egui::{Color32, RichText, ScrollArea, TextEdit};
use std::path::PathBuf;
use tool_extension::{PluginManager, PluginState, PluginSummary};

pub struct PluginsPanel {
    root: String,
    last_error: Option<String>,
    recently_disabled: Vec<String>,
}

impl PluginsPanel {
    pub fn new() -> Self {
        Self {
            root: "plugins".to_owned(),
            last_error: None,
            recently_disabled: Vec::new(),
        }
    }

    pub fn take_recently_disabled(&mut self) -> Vec<String> {
        std::mem::take(&mut self.recently_disabled)
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, manager: &mut PluginManager) {
        ui.horizontal(|ui| {
            ui.label("根目录");
            ui.add(TextEdit::singleline(&mut self.root).desired_width(240.0));
            if ui.button("刷新").clicked() {
                self.refresh(manager);
            }
        });

        if let Some(error) = &self.last_error {
            ui.colored_label(theme::RED, error);
        }

        ui.separator();
        let summaries = manager.summaries();
        if summaries.is_empty() {
            ui.label("未找到插件");
            return;
        }

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for summary in summaries {
                    self.plugin_row(ui, manager, summary);
                    ui.separator();
                }
            });
    }

    fn refresh(&mut self, manager: &mut PluginManager) {
        match manager.discover_roots([PathBuf::from(self.root.trim())]) {
            Ok(count) => self.last_error = Some(format!("发现了 {count} 个插件")),
            Err(error) => self.last_error = Some(error.to_string()),
        }
    }

    fn plugin_row(
        &mut self,
        ui: &mut egui::Ui,
        manager: &mut PluginManager,
        summary: PluginSummary,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&summary.name).strong());
            ui.monospace(&summary.id);
            ui.label(format!("v{}", summary.version));
            ui.label(
                RichText::new(format!("{:?}", summary.state)).color(state_color(summary.state)),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("运行时");
            ui.monospace(&summary.runtime);
            ui.label("权限");
            ui.monospace(if summary.permissions.is_empty() {
                "none".to_owned()
            } else {
                summary.permissions.join(", ")
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("路径");
            ui.monospace(summary.path.display().to_string());
        });

        if !summary.contributes.panels.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label("面板");
                for panel in &summary.contributes.panels {
                    ui.monospace(format!("{} ({})", panel.title, panel.kind));
                }
            });
        }

        if !summary.contributes.subscriptions.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label("订阅");
                for subscription in &summary.contributes.subscriptions {
                    ui.monospace(&subscription.topic);
                }
            });
        }

        if let Some(error) = &summary.last_error {
            ui.colored_label(theme::RED, error);
        }

        ui.horizontal(|ui| {
            let can_enable = !matches!(summary.state, PluginState::Running | PluginState::Enabled);
            if ui
                .add_enabled(can_enable, egui::Button::new("启用"))
                .clicked()
            {
                match manager.enable(&summary.id) {
                    Ok(()) => self.last_error = None,
                    Err(error) => self.last_error = Some(error.to_string()),
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
                        self.last_error = None;
                        self.recently_disabled.push(summary.id.clone());
                    }
                    Err(error) => self.last_error = Some(error.to_string()),
                }
            }
        });
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
