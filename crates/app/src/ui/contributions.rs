use crate::app::WorkbenchApp;
use eframe::egui;
use serde_json::json;
use std::path::PathBuf;
use tool_application::api::extension::PluginState;
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
    pub(super) fn ui_contribution_slot(&mut self, ui: &mut egui::Ui, slot: &str) {
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

        // 帧级缓存：summaries() 全量 clone manifest + 命令对账，每帧被 5+ slot 调用，
        // 缓存到 OnceCell，同帧只算一次。tick_pre_ui 开头已重置。
        let summaries: &[tool_application::api::extension::PluginSummary] = self.plugin_summaries();

        for summary in summaries {
            // 注意：遍历缓存（不可变借用 summaries），后续需要 &mut self 的调用必须延后到循环外。
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
                ui.label(egui::RichText::new(display_text).color(theme::text_secondary()));
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
                // ProgressBar 内部 height = desired_height.unwrap_or(interact_size.y)
                // （progress_bar.rs:115）。本项目把 interact_size.y 设为 28（bootstrap.rs:106），
                // 而状态栏面板 exact_size(26) 可用内容高仅 22（inner_margin 上下各 2）。
                // 于是 ProgressBar 主动占 28px > 22 可用，超出 6px 被 clip_rect 裁掉底部，
                // 导致状态栏 label 只显示一半。add_sized 的 8.0 对 ProgressBar 无效，必须显式
                // desired_height。再用 left_to_right(Align::Min) 包裹：cross_align=Min 时
                // vertical_align()=Min，不触发 next_frame_ignore_wrap 里 “Center 时 frame 填到
                // available” 的填充，避免父 Center 把 frame 抬到 22 之外。
                let bar_height = 8.0;
                let bar_width = 46.0;
                let response = ui
                    .allocate_ui_with_layout(
                        egui::vec2(bar_width, bar_height),
                        egui::Layout::left_to_right(egui::Align::Min),
                        |ui| {
                            ui.add(
                                egui::ProgressBar::new(value as f32)
                                    .desired_width(bar_width)
                                    .desired_height(bar_height),
                            )
                        },
                    )
                    .inner;
                if !text.is_empty() {
                    ui.label(egui::RichText::new(text).color(theme::text_secondary()));
                }
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
            self.workbench.authorize_plugin_file(&item.plugin_id, path);
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
                    "open_ports": self.workbench.open_port_names(),
                }
            });
        }

        value
    }
}
