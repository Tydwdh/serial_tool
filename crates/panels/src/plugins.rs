use crate::theme;
use egui::{Color32, RichText, ScrollArea, TextEdit};
use std::path::PathBuf;
use tool_extension::{
    PluginDiagnostic, PluginDiagnosticSeverity, PluginManager, PluginState, PluginSummary,
};

pub struct PluginsPanel {
    root: String,
    recently_disabled: Vec<String>,
    /// disable 后待重新启用的插件 ID（用于 restart）
    pending_restart: Vec<String>,
}

impl PluginsPanel {
    pub fn new() -> Self {
        Self {
            root: "plugins".to_owned(),
            recently_disabled: Vec::new(),
            pending_restart: Vec::new(),
        }
    }

    pub fn take_recently_disabled(&mut self) -> Vec<String> {
        std::mem::take(&mut self.recently_disabled)
    }

    /// 渲染插件面板 UI，返回 (消息内容, 是否错误) 供调用方显示到状态栏。
    pub fn ui(&mut self, ui: &mut egui::Ui, manager: &mut PluginManager) -> Option<(String, bool)> {
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

        let toolbar_status = ui
            .horizontal(|ui| -> Option<(String, bool)> {
                ui.label("根目录");
                ui.add(TextEdit::singleline(&mut self.root).desired_width(240.0));
                if ui.button("刷新").clicked() {
                    match manager.discover_roots([PathBuf::from(self.root.trim())]) {
                        Ok(count) => return Some((format!("发现了 {count} 个插件"), false)),
                        Err(error) => return Some((error.to_string(), true)),
                    }
                }
                if ui.button("打开目录").clicked() {
                    let _ = open::that(&self.root);
                }
                None
            })
            .inner;

        let status = toolbar_status;

        ui.separator();
        let summaries = manager.summaries();
        let diagnostics = manager.diagnostics();

        if summaries.is_empty() && diagnostics.is_empty() {
            ui.label("未找到插件");
            return status;
        }

        if !diagnostics.is_empty() {
            ui.label(RichText::new("诊断").strong());
            for diagnostic in diagnostics {
                diagnostic_row(ui, diagnostic);
            }
            ui.separator();
        }

        if summaries.is_empty() {
            ui.label("没有可加载插件");
            return status;
        }

        let scroll_result = ScrollArea::vertical().auto_shrink([false, false]).show(
            ui,
            |ui| -> Option<(String, bool)> {
                let mut row_status: Option<(String, bool)> = None;
                for summary in summaries {
                    let s = self.plugin_row(ui, manager, summary);
                    if row_status.is_none() {
                        row_status = s;
                    }
                    ui.separator();
                }
                row_status
            },
        );

        status.or(scroll_result.inner)
    }

    fn plugin_row(
        &mut self,
        ui: &mut egui::Ui,
        manager: &mut PluginManager,
        summary: PluginSummary,
    ) -> Option<(String, bool)> {
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

        // 按钮行：通过闭包返回值传递操作结果

        ui.horizontal(|ui| -> Option<(String, bool)> {
            let can_enable = !matches!(summary.state, PluginState::Running | PluginState::Enabled);
            if ui
                .add_enabled(can_enable, egui::Button::new("启用"))
                .clicked()
            {
                match manager.enable(&summary.id) {
                    Ok(()) => {}
                    Err(error) => return Some((error.to_string(), true)),
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
                    Err(error) => return Some((error.to_string(), true)),
                }
            }
            let can_restart = can_disable;
            if ui
                .add_enabled(can_restart, egui::Button::new("重启"))
                .on_hover_text("禁用后重新启用")
                .clicked()
            {
                if let Err(e) = manager.disable(&summary.id) {
                    return Some((format!("禁用失败：{e}"), true));
                }
                // 不立即 enable：等待下一帧 reap_stopping_plugins 收割后再启用
                self.pending_restart.push(summary.id.clone());
            }
            None
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
        ui.monospace(&diagnostic.code);
        if let Some(plugin_id) = &diagnostic.plugin_id {
            ui.monospace(plugin_id);
        }
        ui.label(&diagnostic.message);
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("路径");
        ui.monospace(diagnostic.path.display().to_string());
    });
}
