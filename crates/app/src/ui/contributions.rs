use crate::app::WorkbenchApp;
use eframe::egui;
use serde_json::json;
use std::path::PathBuf;
use tool_extension::PluginState;
use tool_panels::theme;

#[derive(Clone)]
struct ResolvedUiContribution {
    plugin_id: String,
    plugin_name: String,
    permissions: Vec<String>,
    id: String,
    slot: String,
    kind: String,
    title: String,
    command: Option<String>,
    tooltip: Option<String>,
    order: i32,
    enabled: bool,
    record_send_input: bool,
    default: serde_json::Value,
}

impl WorkbenchApp {
    pub(crate) fn ui_contribution_slot(&mut self, ui: &mut egui::Ui, slot: &str) {
        let items = self.resolved_ui_contributions(slot);
        if items.is_empty() {
            return;
        }

        for item in items {
            self.ui_contribution_item(ui, item);
        }
    }

    /// 由插件通过 ctx.ui.set_contribution_value 设置 contribution 的运行时状态。
    /// key 格式：`{plugin_id}:{contribution_id}`，防止跨插件 ID 冲突。
    pub(crate) fn set_contribution_value(
        &mut self,
        plugin_id: &str,
        contribution_id: &str,
        value: serde_json::Value,
    ) {
        self.contribution_states
            .insert(format!("{plugin_id}:{contribution_id}"), value);
    }

    fn resolved_ui_contributions(&mut self, slot: &str) -> Vec<ResolvedUiContribution> {
        let mut items = Vec::new();

        for summary in self.plugin_manager.summaries() {
            if !matches!(summary.state, PluginState::Enabled | PluginState::Running) {
                continue;
            }

            for contribution in &summary.contributes.ui {
                if contribution.slot != slot || !contribution.visible {
                    continue;
                }

                let command_title = contribution.command.as_ref().and_then(|command| {
                    summary
                        .contributes
                        .commands
                        .iter()
                        .find(|candidate| candidate.id == *command)
                        .map(|candidate| candidate.title.clone())
                });

                items.push(ResolvedUiContribution {
                    plugin_id: summary.id.clone(),
                    plugin_name: summary.name.clone(),
                    permissions: summary.permissions.clone(),
                    id: contribution.id.clone(),
                    slot: contribution.slot.clone(),
                    kind: contribution.kind.clone(),
                    title: contribution
                        .title
                        .clone()
                        .or(command_title)
                        .or_else(|| contribution.command.clone())
                        .unwrap_or_else(|| contribution.id.clone()),
                    command: contribution.command.clone(),
                    tooltip: contribution.tooltip.clone(),
                    order: contribution.order,
                    enabled: contribution.enabled,
                    record_send_input: contribution.record_send_input,
                    default: contribution.default.clone(),
                });
            }
        }

        items.sort_by(|a, b| {
            a.order
                .cmp(&b.order)
                .then_with(|| a.plugin_name.cmp(&b.plugin_name))
                .then_with(|| a.title.cmp(&b.title))
        });

        items
    }

    fn ui_contribution_item(&mut self, ui: &mut egui::Ui, item: ResolvedUiContribution) {
        let state_key = format!("{}:{}", item.plugin_id, item.id);

        match item.kind.to_ascii_lowercase().as_str() {
            "separator" => {
                ui.separator();
            }
            "label" | "status" => {
                // 先从 contribution_states 读运行时 text，否则用静态 title
                let display_text = self
                    .contribution_states
                    .get(&state_key)
                    .and_then(|v| v.as_str())
                    .unwrap_or(&item.title)
                    .to_owned();
                ui.label(egui::RichText::new(display_text).color(theme::TEXT_SECONDARY));
            }
            "progress" => {
                let state = self
                    .contribution_states
                    .get(&state_key)
                    .or_else(|| (!item.default.is_null()).then_some(&item.default));
                let Some(state) = state else {
                    return;
                };
                let value = state
                    .get("value")
                    .and_then(|v| v.as_f64())
                    .or_else(|| state.as_f64())
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let text = state.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if state.get("visible").and_then(|v| v.as_bool()) == Some(false)
                    || (value <= 0.0 && text.is_empty())
                {
                    return;
                }
                let response = ui
                    .horizontal(|ui| {
                        let response =
                            ui.add_sized([46.0, 8.0], egui::ProgressBar::new(value as f32));
                        if !text.is_empty() {
                            ui.label(egui::RichText::new(text).color(theme::TEXT_SECONDARY));
                        }
                        response
                    })
                    .inner;
                if let Some(tooltip) = item.tooltip.as_deref().filter(|t| !t.trim().is_empty()) {
                    response.on_hover_text(tooltip);
                }
            }
            "toggle" => {
                let current = self
                    .contribution_states
                    .get(&state_key)
                    .and_then(|v| v.as_bool())
                    .unwrap_or_else(|| item.default.as_bool().unwrap_or(false));

                let response = ui.selectable_label(current, &item.title);
                let response = match item.tooltip.as_deref() {
                    Some(tooltip) if !tooltip.trim().is_empty() => response.on_hover_text(tooltip),
                    _ => response,
                };

                if response.clicked() {
                    // 切换本地状态
                    self.contribution_states
                        .insert(state_key.clone(), json!(!current));

                    let payload = json!({
                        "plugin_id": item.plugin_id,
                        "contribution_id": item.id,
                        "slot": item.slot,
                        "kind": "toggle",
                        "command": item.command,
                        "state": !current,
                        "context": self.ui_contribution_context(&item.slot),
                    });

                    if let Some(command) = item.command.as_deref() {
                        self.publish_plugin_command_execute(&item.plugin_id, command, &payload);
                    }
                }
            }
            "button" | "small_button" | "" => {
                let response = ui.add_enabled(item.enabled, egui::Button::new(&item.title));
                let response = match item.tooltip.as_deref() {
                    Some(tooltip) if !tooltip.trim().is_empty() => response.on_hover_text(tooltip),
                    _ => response,
                };

                if response.clicked() {
                    self.publish_ui_contribution_action(&item);
                }
            }
            _ => {
                ui.add_enabled(false, egui::Button::new(item.title))
                    .on_hover_text(format!("不支持的插件控件类型: {}", item.kind));
            }
        }
    }

    fn publish_ui_contribution_action(&mut self, item: &ResolvedUiContribution) {
        self.authorize_send_input_file_if_needed(item);
        if item.slot.starts_with("send.") && item.record_send_input {
            self.record_send_history(self.send.input.clone());
        }

        let payload = json!({
            "plugin_id": item.plugin_id.clone(),
            "contribution_id": item.id.clone(),
            "slot": item.slot.clone(),
            "kind": item.kind.clone(),
            "command": item.command.clone(),
            "context": self.ui_contribution_context(&item.slot),
        });

        if let Some(command) = item.command.as_deref() {
            self.publish_plugin_command_execute(&item.plugin_id, command, &payload);
        }
    }

    fn authorize_send_input_file_if_needed(&mut self, item: &ResolvedUiContribution) {
        if !item.slot.starts_with("send.") {
            return;
        }
        if !item
            .permissions
            .iter()
            .any(|permission| permission == "fs.read.user_selected")
        {
            return;
        }

        let input = self.send.input.trim();
        if input.is_empty() || input.lines().count() != 1 {
            return;
        }

        let path = PathBuf::from(input.trim_matches('"'));
        if path.is_file() {
            self.file_broker.authorize(&item.plugin_id, path);
        }
    }

    fn ui_contribution_context(&self, slot: &str) -> serde_json::Value {
        let mut value = json!({
            "slot": slot,
        });

        if slot.starts_with("send.") {
            value = json!({
                "slot": slot,
                "send": {
                    "input": self.send.input.clone(),
                    "target_port": self.send.target_port.clone(),
                    "target_port_open": self.send_target_port_open(),
                    "hex_mode": self.send.hex_mode,
                    "line_ending": {
                        "label": self.send.line_ending.label(),
                        "suffix": self.send.line_ending.suffix(),
                    },
                    "periodic_enabled": self.send.periodic_enabled,
                    "periodic_interval_ms": self.send.periodic_interval_ms,
                },
                "serial": {
                    "selected_port": self.serial.selected_port.clone(),
                    "open_ports": self.transport.open_ports(),
                }
            });
        }

        value
    }
}
