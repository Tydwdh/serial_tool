use crate::theme;
use egui::{Color32, RichText, ScrollArea, TextEdit};
use std::path::PathBuf;
use tool_extension::{PluginManager, PluginState, PluginSummary};

pub struct PluginsPanel {
    root: String,
    last_error: Option<String>,
    recently_disabled: Vec<String>,
    pending_enable: Option<PluginSummary>,
}

impl PluginsPanel {
    pub fn new() -> Self {
        Self {
            root: "plugins".to_owned(),
            last_error: None,
            recently_disabled: Vec::new(),
            pending_enable: None,
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
            if ui.button("创建插件...").clicked() {
                self.scaffold_plugin();
            }
            if ui.button("打开目录").clicked() {
                self.last_error = Some(format!("插件目录: {}", self.root));
            }
        });

        if let Some(error) = &self.last_error {
            ui.colored_label(theme::RED, error);
        }

        // 权限确认对话框
        if let Some(ref summary) = self.pending_enable.clone() {
            egui::Window::new("确认启用插件")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(format!("「{}」({}) 请求以下权限：", summary.name, summary.id));
                    ui.separator();
                    for perm in &summary.permissions {
                        ui.label(format!("  • {perm}"));
                    }
                    if !summary.contributes.subscriptions.is_empty() {
                        ui.label("订阅主题：");
                        for sub in &summary.contributes.subscriptions {
                            ui.label(format!("  • {}", sub.topic));
                        }
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("确认启用").clicked() {
                            let id = summary.id.clone();
                            match manager.enable(&id) {
                                Ok(()) => self.last_error = None,
                                Err(error) => self.last_error = Some(error.to_string()),
                            }
                            self.pending_enable = None;
                        }
                        if ui.button("取消").clicked() {
                            self.pending_enable = None;
                        }
                    });
                });
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

    fn scaffold_plugin(&mut self) {
        let name = "my-plugin";
        let dir = PathBuf::from(self.root.trim()).join(name);
        if dir.exists() {
            self.last_error = Some(format!("{name} 已存在"));
            return;
        }
        match std::fs::create_dir_all(&dir) {
            Ok(()) => {
                let plugin_json = serde_json::json!({
                    "id": name,
                    "name": "我的插件",
                    "version": "0.1.0",
                    "runtime": "lua",
                    "main": "main.lua",
                    "permissions": ["bus", "log", "ui", "storage"],
                });
                let _ = std::fs::write(
                    dir.join("plugin.json"),
                    serde_json::to_string_pretty(&plugin_json).unwrap_or_default(),
                );

                let main_lua = r#"local PANEL_ID = "my-panel"

function on_init(ctx)
    ctx.log("info", "插件已加载")

    ctx.ui.create_form({
        id = PANEL_ID,
        title = "我的面板",
        fields = {
            { id = "msg", kind = "TextArea", title = "消息", value = "Hello!", rows = 1 },
            { id = "btn", kind = "Button", title = "发送日志", action = "my.send" },
        },
    })
end

function on_form_changed(ctx, panel_id, values)
    -- 处理表单变更
end

function on_form_action(ctx, panel_id, field_id, action, values)
    if action == "my.send" then
        ctx.log("info", "发送: " .. tostring(values.msg or ""))
    end
end
"#;
                let _ = std::fs::write(dir.join("main.lua"), main_lua);

                self.last_error = Some(format!("已创建插件 {name}，请点击刷新后启用"));
            }
            Err(e) => {
                self.last_error = Some(format!("创建失败：{e}"));
            }
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
                self.pending_enable = Some(summary.clone());
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
            let can_restart = can_disable;
            if ui
                .add_enabled(can_restart, egui::Button::new("重启"))
                .on_hover_text("禁用后重新启用")
                .clicked()
            {
                if let Err(e) = manager.disable(&summary.id) {
                    self.last_error = Some(format!("禁用失败：{e}"));
                } else if let Err(e) = manager.enable(&summary.id) {
                    self.last_error = Some(format!("启用失败：{e}"));
                } else {
                    self.last_error = Some(format!("{} 已重启", summary.id));
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
