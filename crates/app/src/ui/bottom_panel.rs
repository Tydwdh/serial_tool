use crate::app::WorkbenchApp;
use crate::state::{LineEnding, MAX_SEND_HISTORY, StatusLevel};
use eframe::egui;
use tool_panels::theme;

/// 发送面板布局模式
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendLayout {
    /// 底部面板水平布局
    Horizontal,
    /// 右侧面板垂直布局
    Vertical,
    /// 悬浮窗口
    Popup,
}

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
                .serial
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

    // ── 对外入口：三个布局都走这一个核心方法 ──

    pub(crate) fn send_panel_horizontal(&mut self, ui: &mut egui::Ui) {
        self.send_panel_body(ui, SendLayout::Horizontal);
    }

    pub(crate) fn send_panel_vertical(&mut self, ui: &mut egui::Ui) {
        self.send_panel_body(ui, SendLayout::Vertical);
    }

    pub(crate) fn send_panel_popup(&mut self, ui: &mut egui::Ui) {
        self.send_panel_body(ui, SendLayout::Popup);
    }

    // ── 统一核心渲染 ──

    fn send_panel_body(&mut self, ui: &mut egui::Ui, layout: SendLayout) {
        self.ensure_send_target_port();
        let send_port_open = self.send_target_port_open();

        // ── 1. 选项栏 ──
        self.render_send_options(ui, layout);

        // ── 2. 输入区（固定高度 + 滚动条）──
        let resp = self.render_send_input(ui, layout, send_port_open);

        if resp.changed() {
            self.send.periodic_send_count = 0;
            self.send.error = None;
        }

        let ctrl_enter = resp.has_focus()
            && ui
                .ctx()
                .input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));

        if ctrl_enter && send_port_open && !self.send.input.trim().is_empty() {
            self.do_send();
        }

        // ── 3. 操作栏 ──
        self.render_send_actions(ui, layout, send_port_open);

        // ── 4. 错误提示 ──
        if let Some(err) = &self.send.error {
            ui.colored_label(theme::RED, translate_error(err));
        }
    }

    // ── 选项栏 ──

    fn render_send_options(&mut self, ui: &mut egui::Ui, layout: SendLayout) {
        let is_popup = matches!(layout, SendLayout::Popup);

        // ── 标题（Popup）──
        if is_popup {
            ui.horizontal(|ui| {
                ui.heading("发送");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pin_label = if self.send_popup_always_on_top {
                        "\u{1f4cc} 置顶"
                    } else {
                        "置顶"
                    };
                    if ui
                        .selectable_label(self.send_popup_always_on_top, pin_label)
                        .on_hover_text("让该窗口保持在其他窗口上方")
                        .clicked()
                    {
                        self.send_popup_always_on_top = !self.send_popup_always_on_top;
                        if let Err(e) = self.save_config() {
                            log::warn!("save_config failed: {e}")
                        };
                    }
                });
            });
        }

        // ── 目标 / 发送模式 ──
        self.render_send_target_options(ui, layout);
    }

    fn render_send_target_options(&mut self, ui: &mut egui::Ui, layout: SendLayout) {
        let (target_id, line_ending_id, line_ending_width, show_popup_button) = match layout {
            SendLayout::Horizontal => ("send-target-port-bottom", "line-ending-bottom", 64.0, true),
            SendLayout::Vertical => ("send-target-port-right", "line-ending-right", 80.0, true),
            SendLayout::Popup => (
                "send-popup-target-port",
                "send-popup-line-ending",
                64.0,
                false,
            ),
        };

        ui.horizontal_wrapped(|ui| {
            ui.label("发送到");
            self.send_target_port_combo(ui, target_id);
            ui.separator();
            self.render_hex_toggle(ui);
            self.render_line_ending_combo(ui, line_ending_id, line_ending_width);

            if show_popup_button && ui.small_button("⛶").on_hover_text("放大编辑").clicked() {
                self.send.popup_open = true;
            }
        });
    }

    /// HEX/文本切换 + 严格模式
    fn render_hex_toggle(&mut self, ui: &mut egui::Ui) {
        ui.radio_value(&mut self.send.hex_mode, false, "文本");
        ui.radio_value(&mut self.send.hex_mode, true, "HEX");
        if self.send.hex_mode {
            ui.checkbox(&mut self.send.hex_strict, "严格")
                .on_hover_text("严格模式：奇数 HEX 长度报错而非自动补0");
        }
    }

    /// 换行符下拉框
    fn render_line_ending_combo(&mut self, ui: &mut egui::Ui, id_salt: &'static str, width: f32) {
        ui.add_enabled_ui(!self.send.hex_mode, |ui| {
            egui::ComboBox::from_id_salt(id_salt)
                .width(width)
                .selected_text(self.send.line_ending.label())
                .show_ui(ui, |ui| {
                    for &le in LineEnding::ALL.iter() {
                        ui.selectable_value(&mut self.send.line_ending, le, le.label());
                    }
                });
        });
    }

    // ── 输入区 ──

    fn render_send_input(
        &mut self,
        ui: &mut egui::Ui,
        layout: SendLayout,
        send_port_open: bool,
    ) -> egui::Response {
        let (max_height, desired_rows, id_salt, hint_text) = match layout {
            SendLayout::Horizontal => {
                let h = ui.text_style_height(&egui::TextStyle::Monospace) * 6.0 + 8.0;
                (
                    h,
                    5,
                    "send-input-scroll-h",
                    if send_port_open {
                        "Ctrl+Enter 发送 | ⛶ 放大编辑"
                    } else {
                        "请选择已打开的串口"
                    },
                )
            }
            SendLayout::Vertical => {
                let h = ui.text_style_height(&egui::TextStyle::Monospace) * 10.0 + 8.0;
                (h, 8, "send-input-scroll-v", "输入要发送的数据")
            }
            SendLayout::Popup => (
                popup_send_input_height(ui),
                24,
                "send-input-scroll-popup",
                "Ctrl+Enter 发送",
            ),
        };

        let mut scroll = egui::ScrollArea::vertical()
            .id_salt(id_salt)
            .max_height(max_height);

        if layout == SendLayout::Popup {
            scroll = scroll.auto_shrink([false, false]);
        }

        scroll
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.send.input)
                        .desired_width(f32::INFINITY)
                        .desired_rows(desired_rows)
                        .hint_text(hint_text),
                )
            })
            .inner
    }

    // ── 操作栏 ──

    fn render_send_actions(&mut self, ui: &mut egui::Ui, layout: SendLayout, send_port_open: bool) {
        let (history_id, interval_width) = match layout {
            SendLayout::Horizontal => ("send-history-bottom", 54.0),
            SendLayout::Vertical => ("send-history-right", 72.0),
            SendLayout::Popup => ("send-popup-history", 54.0),
        };

        ui.horizontal_wrapped(|ui| {
            self.render_send_and_clear_buttons(ui, send_port_open);
            self.send_history_combo(ui, history_id);
            self.ui_contribution_slot(ui, "send.toolbar");
        });

        ui.horizontal_wrapped(|ui| {
            self.render_periodic_controls(ui, interval_width);
            ui.separator();
            self.send_signal_controls(ui);
            self.render_hex_preview(ui);
        });
    }

    /// 发送 + 清空 按钮
    fn render_send_and_clear_buttons(&mut self, ui: &mut egui::Ui, send_port_open: bool) {
        if ui
            .add_enabled(
                send_port_open && !self.send.input.trim().is_empty(),
                egui::Button::new("发送"),
            )
            .clicked()
        {
            self.do_send();
        }

        if ui.button("清空").clicked() {
            self.send.input.clear();
            self.send.error = None;
            self.send.periodic_send_count = 0;
        }
    }

    /// 周期发送控件
    fn render_periodic_controls(&mut self, ui: &mut egui::Ui, width: f32) {
        if ui
            .checkbox(&mut self.send.periodic_enabled, "周期发送")
            .changed()
        {
            self.send.periodic_send_count = 0;
            if !self.send.periodic_enabled
                && let Some(cancel) = self.periodic_send_cancel.take()
            {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.send.periodic_interval_ms).desired_width(width),
        );
        ui.label("ms");
        // 实时验证：非空且非正数时给出提示
        let trimmed = self.send.periodic_interval_ms.trim();
        if !trimmed.is_empty() {
            match trimmed.parse::<f64>() {
                Ok(v) if v <= 0.0 => {
                    ui.colored_label(theme::YELLOW, "间隔必须 > 0ms");
                }
                Err(_) => {
                    ui.colored_label(theme::YELLOW, "请输入有效数字");
                }
                _ => {}
            }
        }
    }

    // ── HEX 预览 ──

    fn render_hex_preview(&mut self, ui: &mut egui::Ui) {
        if self.send.hex_mode && !self.send.input.trim().is_empty() {
            let preview = hex_preview(&self.send.input);
            ui.label(
                egui::RichText::new(format!("HEX: {preview}"))
                    .color(theme::TEXT_SECONDARY)
                    .monospace()
                    .small(),
            );
        }
    }

    // ── 信号控制 ──

    fn send_signal_controls(&mut self, ui: &mut egui::Ui) {
        if self.send.target_port.is_none() {
            ui.add_enabled(false, egui::Checkbox::new(&mut self.send.dtr_high, "DTR"));
            ui.add_enabled(false, egui::Checkbox::new(&mut self.send.rts_high, "RTS"));
            return;
        }

        let port = self.send.target_port.clone().unwrap();
        let open = self.transport.status_port(&port).open;

        ui.add_enabled_ui(open, |ui| {
            let mut dtr = self.send.dtr_high;
            if ui.checkbox(&mut dtr, "DTR").changed() {
                match self.transport.set_dtr(&port, dtr) {
                    Ok(()) => self.send.dtr_high = dtr,
                    Err(e) => self.set_status_force(StatusLevel::Error, e.to_string()),
                }
            }

            let mut rts = self.send.rts_high;
            if ui.checkbox(&mut rts, "RTS").changed() {
                match self.transport.set_rts(&port, rts) {
                    Ok(()) => self.send.rts_high = rts,
                    Err(e) => self.set_status_force(StatusLevel::Error, e.to_string()),
                }
            }
        });
    }

    // ── 发送逻辑 ──

    pub(crate) fn do_send(&mut self) {
        let Some(port) = self.send.target_port.as_deref() else {
            self.send.error = Some("请选择发送目标串口".into());
            return;
        };
        self.send.error = send_impl_to(
            port,
            &self.send.input,
            self.send.hex_mode,
            self.send.line_ending.suffix(),
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

        if self.send.send_history.front() == Some(&text) {
            return;
        }
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
        // 不再每次发送都同步落盘，依赖 tick_auto_save 每 60 秒自动保存
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
}

use tool_transport::{hex_preview, send_impl_to, translate_error};

fn popup_send_input_height(ui: &egui::Ui) -> f32 {
    let action_rows = ui.text_style_height(&egui::TextStyle::Button) * 2.0;
    let spacing = ui.spacing().item_spacing.y * 5.0;
    let reserved_for_actions = action_rows + spacing + 18.0;
    let fallback = ui.text_style_height(&egui::TextStyle::Monospace) * 14.0 + 8.0;
    let available = ui.available_height();

    if available.is_finite() && available > reserved_for_actions {
        (available - reserved_for_actions).max(120.0)
    } else {
        fallback
    }
}

fn shorten_for_ui(s: &str, max_chars: usize) -> String {
    let mut out = s.chars().take(max_chars).collect::<String>();
    if s.chars().count() > max_chars {
        out.push('…');
    }
    out
}
