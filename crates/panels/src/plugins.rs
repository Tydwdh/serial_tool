use crate::theme;
use egui::{Color32, RichText, ScrollArea, TextEdit};
use std::path::PathBuf;
use tool_extension::{PluginManager, PluginState, PluginSummary};

pub struct PluginsPanel {
    root: String,
    recently_disabled: Vec<String>,
}

impl PluginsPanel {
    pub fn new() -> Self {
        Self {
            root: "plugins".to_owned(),
            recently_disabled: Vec::new(),
        }
    }

    pub fn take_recently_disabled(&mut self) -> Vec<String> {
        std::mem::take(&mut self.recently_disabled)
    }

    /// 渲染插件面板 UI，返回 (消息内容, 是否错误) 供调用方显示到状态栏。
    pub fn ui(&mut self, ui: &mut egui::Ui, manager: &mut PluginManager) -> Option<(String, bool)> {
        let toolbar_status = ui.horizontal(|ui| -> Option<(String, bool)> {
            ui.label("根目录");
            ui.add(TextEdit::singleline(&mut self.root).desired_width(240.0));
            if ui.button("刷新").clicked() {
                match manager.discover_roots([PathBuf::from(self.root.trim())]) {
                    Ok(count) => return Some((format!("发现了 {count} 个插件"), false)),
                    Err(error) => return Some((error.to_string(), true)),
                }
            }
            if ui.button("创建插件...").clicked() {
                return self.scaffold_plugin();
            }
            if ui.button("打开目录").clicked() {
                let _ = open::that(&self.root);
            }
            None
        }).inner;

        let status = toolbar_status;

        ui.separator();
        let summaries = manager.summaries();
        if summaries.is_empty() {
            ui.label("未找到插件");
            return status;
        }

        let scroll_result = ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| -> Option<(String, bool)> {
                let mut row_status: Option<(String, bool)> = None;
                for summary in summaries {
                    let s = self.plugin_row(ui, manager, summary);
                    if row_status.is_none() {
                        row_status = s;
                    }
                    ui.separator();
                }
                row_status
            });

        status.or(scroll_result.inner)
    }

    fn scaffold_plugin(&mut self) -> Option<(String, bool)> {
        let name = "my-plugin";
        let dir = PathBuf::from(self.root.trim()).join(name);
        if dir.exists() {
            return Some((format!("{name} 已存在"), true));
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
                if let Err(e) = std::fs::write(
                    dir.join("plugin.json"),
                    serde_json::to_string_pretty(&plugin_json).unwrap_or_default(),
                ) {
                    return Some((format!("写入 plugin.json 失败：{e}"), true));
                }

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
                if let Err(e) = std::fs::write(dir.join("main.lua"), main_lua) {
                    return Some((format!("写入 main.lua 失败：{e}"), true));
                }

                Some((format!("已创建插件 {name}，请点击刷新后启用"), false))
            }
            Err(e) => Some((format!("创建失败：{e}"), true)),
        }
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
        let row_status = ui.horizontal(|ui| -> Option<(String, bool)> {
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
                } else if let Err(e) = manager.enable(&summary.id) {
                    return Some((format!("启用失败：{e}"), true));
                } else {
                    return Some((format!("{} 已重启", summary.id), false));
                }
            }
            None
        }).inner;

        row_status
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
