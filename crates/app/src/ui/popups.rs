use crate::app::WorkbenchApp;
use crate::bootstrap::REPAINT_INTERVAL_MS;
use crate::state::{DetachedPanelAction, StatusLevel};
use crate::ui::bottom_panel::translate_error;
use eframe::egui;
use serde_json::Value;
use tool_core::{Direction, Event, LogLevel, Payload};
use tool_panels::{PanelKind, theme};

impl WorkbenchApp {
    pub(crate) fn terminal_popup(&mut self, ctx: &egui::Context) {
        if !self.terminal_popup_open {
            return;
        }

        let vid = egui::ViewportId::from_hash_of("term-popup");
        let builder = egui::ViewportBuilder::default()
            .with_title("接收区 - 硬件调试工作台")
            .with_inner_size([800.0, 600.0]);

        let should_close = ctx.show_viewport_immediate(vid, builder, |ui, _| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                return true;
            }

            egui::CentralPanel::default()
                .show_inside(ui, |ui| {
                    let mut close = false;

                    ui.horizontal(|ui| {
                        ui.heading("接收区");
                        if ui.button("关闭").clicked() {
                            close = true;
                        }
                    });
                    ui.separator();

                    self.terminal_panel.height = (ui.available_height() - 42.0).max(120.0);
                    self.terminal_panel.ui(ui);

                    close
                })
                .inner
        });

        if should_close {
            self.terminal_popup_open = false;
        }
    }
    pub(crate) fn send_popup(&mut self, ctx: &egui::Context) {
        if !self.send.popup_open {
            return;
        }
        let vid = egui::ViewportId::from_hash_of("send-popup");
        let builder = egui::ViewportBuilder::default()
            .with_title("发送 - 硬件调试工作台")
            .with_inner_size([640.0, 480.0])
            .with_min_inner_size([360.0, 260.0]);
        let should_close = ctx.show_viewport_immediate(vid, builder, |ui, _| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                return true;
            }
            egui::CentralPanel::default()
                .show_inside(ui, |ui| {
                    let so = self
                        .selected_port
                        .as_deref()
                        .is_some_and(|p| self.transport.status_port(p).open);
                    let ctrl_enter = ui
                        .ctx()
                        .input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.send.hex_mode, false, "文本");
                        ui.radio_value(&mut self.send.hex_mode, true, "HEX");
                        ui.add_enabled_ui(!self.send.hex_mode, |ui| {
                            ui.checkbox(&mut self.send.append_lf, "LF")
                                .on_disabled_hover_text("HEX 模式请手动添加 0A");
                        });
                        if ui
                            .add_enabled(
                                so && !self.send.input.is_empty(),
                                egui::Button::new("发送 (Ctrl+Enter)"),
                            )
                            .clicked()
                            || (ctrl_enter && so && !self.send.input.is_empty())
                        {
                            self.do_send();
                        }
                        if ui.button("清空").clicked() {
                            self.send.input.clear();
                            self.send.error = None;
                        }
                    });
                    ui.separator();
                    ui.add(
                        egui::TextEdit::multiline(&mut self.send.input)
                            .desired_width(f32::INFINITY)
                            .desired_rows(24)
                            .hint_text("Ctrl+Enter 发送"),
                    );
                    if let Some(ref e) = self.send.error {
                        ui.colored_label(theme::RED, translate_error(e));
                    }
                    false
                })
                .inner
        });
        if should_close {
            self.send.popup_open = false;
        }
    }

    /// 处理 Lua ctx.dialog.open_file 请求。每帧最多处理一个。
    pub(crate) fn poll_dialog_requests(&mut self) {
        if let Ok(request) = self.dialog_receiver.try_recv() {
            let mut dialog = rfd::FileDialog::new().set_title(&request.title);
            for filter in &request.filters {
                if !filter.extensions.is_empty() && filter.extensions[0] != "*" {
                    dialog = dialog.add_filter(
                        &filter.name,
                        &filter
                            .extensions
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>(),
                    );
                }
            }
            let result = dialog.pick_file();
            // 授权路径
            if let Some(ref path) = result {
                self.file_broker.authorize(&request.plugin_id, path.clone());
            }
            // 发送结果回 Lua
            let _ = request.response_sender.send(result);
        }
    }

    /// 处理 ui.form.file_browse 请求。每帧最多处理一个，避免连续弹多个模态对话框。
    pub(crate) fn handle_file_browse_requests(&mut self) {
        let Some(event) = self.file_browse_subscription.try_recv() else {
            return;
        };
        if let Payload::Json(value) = event.payload {
            let panel_id = value.get("panel_id").and_then(Value::as_str).unwrap_or("");
            let field_id = value.get("field_id").and_then(Value::as_str).unwrap_or("");
            let filters: Vec<tool_lua_host::FileFilter> = value
                .get("filters")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .map(|f| tool_lua_host::FileFilter {
                            name: f
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                            extensions: f
                                .get("extensions")
                                .and_then(Value::as_array)
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| v.as_str().map(String::from))
                                        .collect()
                                })
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            let mut dialog = rfd::FileDialog::new().set_title("选择文件");
            for filter in &filters {
                if !filter.extensions.is_empty() && filter.extensions[0] != "*" {
                    dialog = dialog.add_filter(
                        &filter.name,
                        &filter
                            .extensions
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>(),
                    );
                }
            }
            let result = dialog.pick_file();

            // 用户取消时不发布事件，避免清空表单原路径
            if let Some(ref selected_path) = result {
                if let Some(owner) = self.dynamic_panels.panel_owner(panel_id) {
                    self.file_broker.authorize(owner, selected_path.clone());
                } else {
                    self.log(
                        LogLevel::Warn,
                        &format!("file 字段 {panel_id}/{field_id} 没有 owner plugin，跳过授权"),
                    );
                }

                self.bus.publish(Event::new(
                    tool_core::topics::UI_FORM_FILE_SELECTED,
                    "ui",
                    Direction::Internal,
                    Payload::Json(serde_json::json!({
                        "panel_id": panel_id,
                        "field_id": field_id,
                        "path": selected_path.display().to_string(),
                    })),
                ));
            }
        }
    }
    pub(crate) fn dynamic_tab_cleanup(&mut self) {
        let stale: Vec<String> = self
            .panels
            .tabs
            .iter()
            .filter_map(|k| k.dynamic_id().map(str::to_owned))
            .filter(|id| !self.dynamic_panels.contains(id))
            .collect();
        for id in stale {
            self.detached_dynamic_panels.remove(&id);
            self.panels.close_tab(PanelKind::Dynamic(id));
        }
    }

    pub(crate) fn dynamic_panel_ui(&mut self, ui: &mut egui::Ui, id: &str) {
        let title = self.dynamic_panels.title(id).unwrap_or(id).to_owned();
        ui.horizontal(|ui| {
            ui.heading(&title);
            if self.detached_dynamic_panels.contains(id) {
                if ui.button("↙").clicked() {
                    self.detached_dynamic_panels.remove(id);
                }
            } else if ui.button("↗").clicked() {
                self.detached_dynamic_panels.insert(id.to_owned());
            }
        });
        ui.separator();
        if self.detached_dynamic_panels.contains(id) {
            ui.label("已弹出到独立窗口");
            return;
        }
        self.dynamic_panels.ui_body(ui, id);
    }

    pub(crate) fn detached_dynamic_panel_viewports(&mut self, ctx: &egui::Context) {
        let ids: Vec<String> = self.detached_dynamic_panels.iter().cloned().collect();

        for id in ids {
            if !self.dynamic_panels.contains(&id) {
                self.detached_dynamic_panels.remove(&id);
                continue;
            }

            let title = self.dynamic_panels.title(&id).unwrap_or(&id).to_owned();
            let viewport_id = egui::ViewportId::from_hash_of(("dynamic-panel", id.as_str()));

            let builder = egui::ViewportBuilder::default()
                .with_title(format!("{title} - 硬件调试工作台"))
                .with_inner_size([900.0, 640.0])
                .with_min_inner_size([520.0, 360.0]);

            let action = ctx.show_viewport_immediate(viewport_id, builder, |ctx, _class| {
                let mut action = DetachedPanelAction::None;

                if ctx.input(|input| input.viewport().close_requested()) {
                    action = DetachedPanelAction::Attach;
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::default().fill(theme::BG_PRIMARY))
                    .show(ctx, |ui| {
                        // 再手动铺一层，避免某些平台 / resize 时出现未清屏黑边。
                        let rect = ui.max_rect();
                        ui.painter().rect_filled(rect, 0.0, theme::BG_PRIMARY);

                        ui.horizontal(|ui| {
                            ui.heading(&title);

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("关闭面板").clicked() {
                                        action = DetachedPanelAction::Close;
                                    }

                                    if ui.button("↙ 回到标签栏").clicked() {
                                        action = DetachedPanelAction::Attach;
                                    }
                                },
                            );
                        });

                        ui.separator();

                        egui::Frame::default()
                            .fill(theme::BG_PRIMARY)
                            .show(ui, |ui| {
                                self.dynamic_panels.ui_body(ui, &id);
                            });
                    });

                action
            });

            match action {
                DetachedPanelAction::Attach => {
                    self.detached_dynamic_panels.remove(&id);
                    self.panels.open_tab(PanelKind::Dynamic(id));
                }
                DetachedPanelAction::Close => {
                    self.detached_dynamic_panels.remove(&id);
                    self.dynamic_panels.remove(&id);
                    self.panels.close_tab(PanelKind::Dynamic(id));
                }
                DetachedPanelAction::None => {}
            }
        }
    }
}
