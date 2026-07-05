use crate::app::WorkbenchApp;
use crate::state::DetachedPanelAction;
use eframe::egui;
use serde_json::Value;
use tool_core::{Direction, Event, LogLevel, Payload};
use tool_panels::{PanelKind, theme};

/// 悬浮窗口（弹出窗口）运行时状态。
#[derive(Default)]
pub(crate) struct PopupsState {
    pub(crate) terminal_open: bool,
    pub(crate) terminal_always_on_top: bool,
    pub(crate) send_always_on_top: bool,
}

fn floating_viewport_builder(
    title: impl Into<String>,
    size: [f32; 2],
    min_size: [f32; 2],
    always_on_top: bool,
) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(size)
        .with_min_inner_size(min_size)
        .with_window_level(if always_on_top {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        })
}

impl WorkbenchApp {
    pub(crate) fn terminal_popup(&mut self, ctx: &egui::Context) {
        if !self.popups.terminal_open {
            return;
        }

        let vid = egui::ViewportId::from_hash_of("term-popup");
        let builder = floating_viewport_builder(
            "接收区 - 硬件调试工作台",
            [800.0, 600.0],
            [360.0, 240.0],
            self.popups.terminal_always_on_top,
        );

        let should_close = ctx.show_viewport_immediate(vid, builder, |ui, _| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                return true;
            }

            egui::CentralPanel::default()
                .show(ui, |ui| {
                    let mut close = false;

                    ui.horizontal(|ui| {
                        ui.heading("接收区");

                        let pin_label = if self.popups.terminal_always_on_top {
                            "\u{1f4cc} 置顶"
                        } else {
                            "置顶"
                        };

                        if ui
                            .selectable_label(self.popups.terminal_always_on_top, pin_label)
                            .on_hover_text("让该窗口保持在其他窗口上方")
                            .clicked()
                        {
                            self.popups.terminal_always_on_top =
                                !self.popups.terminal_always_on_top;
                            if let Err(e) = self.save_config() {
                                log::warn!("save_config failed: {e}")
                            };
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("清空").clicked() {
                                self.terminal_panel.clear();
                            }
                            if ui.button("关闭").clicked() {
                                close = true;
                            }
                        });
                    });
                    ui.separator();

                    self.terminal_panel.ui(ui);

                    close
                })
                .inner
        });

        if should_close {
            self.popups.terminal_open = false;
        }
    }
    pub(crate) fn send_popup(&mut self, ctx: &egui::Context) {
        if !self.send.popup_open {
            return;
        }
        let vid = egui::ViewportId::from_hash_of("send-popup");
        let builder = floating_viewport_builder(
            "发送 - 硬件调试工作台",
            [640.0, 480.0],
            [360.0, 260.0],
            self.popups.send_always_on_top,
        );
        let should_close = ctx.show_viewport_immediate(vid, builder, |ui, _| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                return true;
            }
            egui::CentralPanel::default()
                .show(ui, |ui| {
                    self.send_panel_popup(ui);
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
                        format!("file 字段 {panel_id}/{field_id} 没有 owner plugin，跳过授权"),
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
