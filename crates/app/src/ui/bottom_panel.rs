use crate::app::WorkbenchApp;
use crate::state::{BottomTab, LineEnding, MAX_SEND_HISTORY, StatusLevel};
use eframe::egui;
use tool_panels::theme;

impl WorkbenchApp {
    /// 确保 send.target_port 指向一个已打开的端口（自动回退逻辑）。
    pub(crate) fn ensure_send_target_port(&mut self) {
        let open_ports = self.transport.open_ports();

        if self
            .send
            .target_port
            .as_ref()
            .is_none_or(|p| !open_ports.contains(p))
        {
            self.send.target_port = self
                .selected_port
                .clone()
                .filter(|p| self.transport.status_port(p).open)
                .or_else(|| open_ports.first().cloned());
        }
    }

    pub(crate) fn send_target_port_open(&self) -> bool {
        self.send
            .target_port
            .as_deref()
            .is_some_and(|p| self.transport.status_port(p).open)
    }

    pub(crate) fn send_target_port_combo(&mut self, ui: &mut egui::Ui, id_salt: &'static str) {
        let open_ports: Vec<String> = self.transport.open_ports();

        egui::ComboBox::from_id_salt(id_salt)
            .width(130.0)
            .selected_text(
                self.send
                    .target_port
                    .as_deref()
                    .map(|p| self.port_label(p))
                    .unwrap_or_else(|| "无端口".to_owned()),
            )
            .show_ui(ui, |ui| {
                if open_ports.is_empty() {
                    ui.add_enabled(false, egui::Label::new("无已打开串口"));
                } else {
                    for port in &open_ports {
                        let label = self.port_label(port);
                        ui.selectable_value(&mut self.send.target_port, Some(port.clone()), label);
                    }
                }
            });
    }

    pub(crate) fn send_bar(&mut self, ui: &mut egui::Ui) {
        self.ensure_send_target_port();
        let send_port_open = self.send_target_port_open();

        ui.horizontal(|ui| {
            ui.label("发送到");
            self.send_target_port_combo(ui, "send-target-port");

            ui.radio_value(&mut self.send.hex_mode, false, "文本");
            ui.radio_value(&mut self.send.hex_mode, true, "HEX");
            if self.send.hex_mode {
                ui.checkbox(&mut self.send.hex_strict, "严格")
                    .on_hover_text("严格模式：奇数 HEX 长度报错而非自动补0");
            }

            ui.add_enabled_ui(!self.send.hex_mode, |ui| {
                egui::ComboBox::from_id_salt("line-ending")
                    .width(60.0)
                    .selected_text(self.send.line_ending.label())
                    .show_ui(ui, |ui| {
                        for &le in LineEnding::ALL.iter() {
                            ui.selectable_value(&mut self.send.line_ending, le, le.label());
                        }
                    });
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⛶").on_hover_text("放大编辑").clicked() {
                    self.send.popup_open = true;
                }
            });
        });

        let text_edit_resp = ui.add(
            egui::TextEdit::multiline(&mut self.send.input)
                .desired_width(f32::INFINITY)
                .desired_rows(5)
                .hint_text(if send_port_open {
                    "Ctrl+Enter 发送 | ⛶ 放大编辑"
                } else {
                    "请选择已打开的串口"
                }),
        );
        if text_edit_resp.changed() {
            self.send.periodic_send_count = 0;
        }

        let ctrl_enter = text_edit_resp.has_focus()
            && ui
                .ctx()
                .input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));

        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    send_port_open && !self.send.input.is_empty(),
                    egui::Button::new("发送"),
                )
                .clicked()
                || (ctrl_enter && send_port_open && !self.send.input.is_empty())
            {
                self.do_send();
            }

            if ui.button("清空").clicked() {
                self.send.input.clear();
                self.send.error = None;
                self.send.periodic_send_count = 0;
            }

            self.ui_contribution_slot(ui, "send.toolbar");

            if !send_port_open {
                ui.colored_label(theme::YELLOW, "\u{26a0} 请先选择并打开串口");
            }

            if let Some(e) = &self.send.error {
                ui.colored_label(theme::RED, translate_error(e));
            }
            if self.send.hex_mode && !self.send.input.trim().is_empty() {
                let preview = hex_preview(&self.send.input);
                ui.label(
                    egui::RichText::new(format!("HEX 预览: {preview}"))
                        .color(theme::TEXT_SECONDARY)
                        .monospace(),
                );
            }
        });

        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut self.send.periodic_enabled, "周期发送")
                .changed()
            {
                if self.send.periodic_enabled {
                    // 开启前预检
                    let mut disable = false;
                    if !self.send_target_port_open() {
                        self.send.error = Some("请先选择并打开目标串口".into());
                        disable = true;
                    } else if self.send.input.trim().is_empty() {
                        self.send.error = Some("请先输入发送内容".into());
                        disable = true;
                    } else if self.send.hex_mode {
                        let interval = self.send.periodic_interval_ms.trim().parse::<u64>().unwrap_or(0);
                        if interval < 10 {
                            self.send.error = Some("周期发送间隔必须 >= 10ms".into());
                            disable = true;
                        }
                    }
                    if disable {
                        self.send.periodic_enabled = false;
                    } else {
                        self.send.periodic_send_count = 0;
                        self.send.error = None;
                        let now = ui.ctx().input(|i| i.time);
                        let interval_ms = self
                            .send
                            .periodic_interval_ms
                            .trim()
                            .parse::<u64>()
                            .unwrap_or(1000);
                        self.send.next_periodic_send_time = now + interval_ms as f64 / 1000.0;
                    }
                }
            }
            ui.add_enabled(
                self.send.periodic_enabled,
                egui::TextEdit::singleline(&mut self.send.periodic_interval_ms)
                    .desired_width(60.0)
                    .hint_text("ms"),
            );
            ui.label("ms");

            if self.send.periodic_enabled {
                let now = ui.ctx().input(|i| i.time);
                let remaining = (self.send.next_periodic_send_time - now).max(0.0);
                ui.label(format!("{:.1}s 后", remaining));
                ui.label(format!("已发送 {} 次", self.send.periodic_send_count));
            }

            self.send_history_combo(ui, "send-history");

            if ui.button("重置周期").clicked() {
                self.send.next_periodic_send_time = 0.0;
                self.send.periodic_send_count = 0;
            }
        });

        if let Some(port) = self.send.target_port.clone() {
            if self.transport.status_port(&port).open {
                ui.horizontal(|ui| {
                    ui.label("信号");
                    let dtr_label = if self.send.dtr_high { "DTR ⬆" } else { "DTR ⬇" };
                    let rts_label = if self.send.rts_high { "RTS ⬆" } else { "RTS ⬇" };
                    if ui.small_button(dtr_label).on_hover_text("切换 DTR").clicked() {
                        let new_val = !self.send.dtr_high;
                        if let Err(e) = self.transport.set_dtr(&port, new_val) {
                            self.set_status_force(StatusLevel::Error, e.to_string());
                        } else {
                            self.send.dtr_high = new_val;
                        }
                    }
                    if ui.small_button("DTR ⏱").on_hover_text("DTR 脉冲(LOW 100ms→HIGH)").clicked() {
                        let _ = self.transport.set_dtr(&port, false);
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        let _ = self.transport.set_dtr(&port, true);
                        self.send.dtr_high = true;
                    }
                    if ui.small_button(rts_label).on_hover_text("切换 RTS").clicked() {
                        let new_val = !self.send.rts_high;
                        if let Err(e) = self.transport.set_rts(&port, new_val) {
                            self.set_status_force(StatusLevel::Error, e.to_string());
                        } else {
                            self.send.rts_high = new_val;
                        }
                    }
                    if ui.small_button("RTS ⏱").on_hover_text("RTS 脉冲(LOW 100ms→HIGH)").clicked() {
                        let _ = self.transport.set_rts(&port, false);
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        let _ = self.transport.set_rts(&port, true);
                        self.send.rts_high = true;
                    }
                });
            }
        }
    }

    pub(crate) fn do_send(&mut self) {
        let Some(port) = self.send.target_port.as_deref() else {
            self.send.error = Some("请选择发送目标串口".into());
            return;
        };
        self.send.error = send_impl_to(
            port,
            &self.send.input,
            self.send.hex_mode,
            self.send.line_ending,
            self.send.hex_strict,
            &self.transport,
        )
        .err()
        .map(|e| e.to_string());

        if self.send.error.is_none() && !self.send.input.trim().is_empty() {
            let text = self.send.input.clone();
            self.record_send_history(text);
        }
    }

    pub(crate) fn record_send_history(&mut self, text: impl Into<String>) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }

        let changed = if self.send.send_history.front() == Some(&text) {
            false
        } else {
            if let Some(index) = self
                .send
                .send_history
                .iter()
                .position(|candidate| candidate == &text)
            {
                self.send.send_history.remove(index);
            }
            self.send.send_history.push_front(text);
            while self.send.send_history.len() > MAX_SEND_HISTORY {
                self.send.send_history.pop_back();
            }
            true
        };

        if changed {
            let _ = self.save_config();
        }
    }

    pub(crate) fn send_history_combo(&mut self, ui: &mut egui::Ui, id_salt: &'static str) {
        if self.send.send_history.is_empty() {
            return;
        }

        let entries: Vec<String> = self.send.send_history.iter().take(20).cloned().collect();
        ui.separator();
        egui::ComboBox::from_id_salt(id_salt)
            .width(140.0)
            .selected_text("发送历史")
            .show_ui(ui, |ui| {
                for item in entries {
                    if ui.button(shorten_for_ui(&item, 48)).clicked() {
                        self.send.input = item;
                    }
                }
            });
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

use tool_transport::TransportManager;

pub(crate) fn hex_preview(input: &str) -> String {
    if input.trim().is_empty() {
        return "—".to_owned();
    }
    const MAX_PREVIEW: usize = 32;
    match tool_transport::parse_hex(input) {
        Ok(bytes) if !bytes.is_empty() => {
            let count = bytes.len();
            let display = if count > MAX_PREVIEW {
                format!(
                    "{}… (共{count}字节)",
                    bytes[..MAX_PREVIEW]
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            } else {
                bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
            };
            display
        }
        Ok(_) => "空".to_owned(),
        Err(_) => "解析失败".to_owned(),
    }
}

pub(crate) fn send_impl_to(
    port: &str,
    input: &str,
    hex: bool,
    line_ending: LineEnding,
    hex_strict: bool,
    t: &TransportManager,
) -> Result<(), tool_transport::TransportError> {
    if input.trim().is_empty() {
        return Ok(());
    }
    if hex {
        for line in input.lines() {
            let x = line.trim();
            if x.is_empty() {
                continue;
            }
            if hex_strict {
                let compact: String = x.chars().filter(|c| !c.is_whitespace()).collect();
                if compact.len() % 2 != 0 {
                    return Err(tool_transport::TransportError::Io(
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("HEX 严格模式: 奇数字节数 \"{x}\", 请补0或关闭严格模式"),
                        ),
                    ));
                }
            }
            t.send_hex_to(port, x)?;
        }
        Ok(())
    } else {
        let mut text = input.to_owned();
        text.push_str(line_ending.suffix());
        t.send_text_to(port, &text)
    }
}
pub(crate) fn translate_error(m: &str) -> String {
    if m.contains("no serial") {
        "串口未打开".into()
    } else if m.contains("invalid hex") {
        format!("无效HEX: {}", m.trim_start_matches("invalid hex input: "))
    } else {
        m.to_owned()
    }
}

fn shorten_for_ui(s: &str, max_chars: usize) -> String {
    let mut out = s.chars().take(max_chars).collect::<String>();
    if s.chars().count() > max_chars {
        out.push('…');
    }
    out
}
