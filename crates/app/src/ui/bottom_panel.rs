use crate::app::{WorkbenchApp, send_impl_to, translate_error};
use crate::state::{BottomTab, StatusLevel};
use crate::ui::top_bar::{serial_action_button, serial_action_button_enabled};
use eframe::egui;
use tool_panels::theme;

impl WorkbenchApp {
    pub(crate) fn send_bar(&mut self, ui: &mut egui::Ui) {
        let so = self
            .selected_port
            .as_deref()
            .is_some_and(|p| self.transport.status_port(p).open);
        ui.horizontal(|ui| {
            ui.label("发送");
            ui.radio_value(&mut self.send.hex_mode, false, "文本");
            ui.radio_value(&mut self.send.hex_mode, true, "HEX");
            ui.add_enabled_ui(!self.send.hex_mode, |ui| {
                ui.checkbox(&mut self.send.append_lf, "LF")
                    .on_disabled_hover_text("HEX 模式请手动添加 0A");
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⛶").on_hover_text("放大编辑").clicked() {
                    self.send.popup_open = true;
                }
            });
        });
        ui.add(
            egui::TextEdit::multiline(&mut self.send.input)
                .desired_width(f32::INFINITY)
                .desired_rows(5)
                .hint_text(if so {
                    "Ctrl+Enter 发送 | ⛶ 放大编辑"
                } else {
                    "可先编辑内容，打开串口后发送"
                }),
        );
        let ctrl_enter = ui
            .ctx()
            .input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(so && !self.send.input.is_empty(), egui::Button::new("发送"))
                .clicked()
                || (ctrl_enter && so && !self.send.input.is_empty())
            {
                self.do_send();
            }
            if ui.button("清空").clicked() {
                self.send.input.clear();
                self.send.error = None;
            }
            if !so {
                ui.colored_label(theme::YELLOW, "⚠ 请先打开串口");
            }
            if let Some(ref e) = self.send.error {
                ui.colored_label(theme::RED, translate_error(e));
            }
        });
    }

    pub(crate) fn do_send(&mut self) {
        let Some(port) = self.selected_port.as_deref() else {
            self.send.error = Some("请选择串口".into());
            return;
        };
        self.send.error = send_impl_to(
            port,
            &self.send.input,
            self.send.hex_mode,
            self.send.append_lf,
            &self.transport,
        )
        .err()
        .map(|e| e.to_string());
    }
    pub(crate) fn show_bottom_panel_contents(&mut self, ui: &mut egui::Ui) {
        self.ensure_bottom_tab_available();
        let visible_tabs = self.available_bottom_tabs();

        // 顶部标签栏：固定在底部面板顶部
        ui.horizontal_wrapped(|ui| {
            for tab in &visible_tabs {
                if ui
                    .selectable_label(self.bottom_tab == *tab, tab.label())
                    .clicked()
                {
                    self.bottom_tab = *tab;
                }
            }
        });
        ui.separator();

        let body_height = ui.available_height();

        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), body_height),
            egui::Layout::bottom_up(egui::Align::Min),
            |ui| {
                // 1. 状态栏固定在最底部
                self.status_bar(ui);

                // 2. 发送区固定在状态栏上方（仅 Terminal 显示）
                if !self.send.popup_open && self.bottom_tab == BottomTab::Terminal {
                    ui.separator();
                    self.send_bar(ui);
                }

                ui.separator();

                // 3. 剩余空间全部给接收区 / 日志区
                let receive_area_total_height = ui.available_height().max(80.0);

                match self.bottom_tab {
                    BottomTab::Terminal => {
                        // TerminalPanel 内部自己还有 RX/TX/HEX 工具栏 + separator
                        let terminal_header_height = 42.0;

                        self.terminal_panel.height =
                            (receive_area_total_height - terminal_header_height).max(40.0);

                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), receive_area_total_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                self.terminal_panel.ui(ui);
                            },
                        );
                    }

                    BottomTab::Logs => {
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), receive_area_total_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                self.bottom_log_panel.ui(ui);
                            },
                        );
                    }
                }
            },
        );
    }
}
