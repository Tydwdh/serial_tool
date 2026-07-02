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

    pub(super) fn send_target_port_combo(&mut self, ui: &mut egui::Ui, id_salt: &'static str) {
        let open_ports: Vec<String> = self.transport.open_ports();

        egui::ComboBox::from_id_salt(id_salt)
            .width(130.0)
            .selected_text(
                self.send
                    .target_port
                    .as_deref()
                    .map(|p| self.serial.port_label(p))
                    .unwrap_or_else(|| "无端口".to_owned()),
            )
            .show_ui(ui, |ui| {
                if open_ports.is_empty() {
                    ui.add_enabled(false, egui::Label::new("无已打开串口"));
                } else {
                    for port in &open_ports {
                        let label = self.serial.port_label(port);
                        ui.selectable_value(&mut self.send.target_port, Some(port.clone()), label);
                    }
                }
            });
    }

    // ── 对外入口：三个布局都走这一个核心方法 ──

    pub(super) fn send_panel_horizontal(&mut self, ui: &mut egui::Ui) {
        self.send_panel_body(ui, SendLayout::Horizontal);
    }

    pub(super) fn send_panel_vertical(&mut self, ui: &mut egui::Ui) {
        self.send_panel_body(ui, SendLayout::Vertical);
    }

    pub(super) fn send_panel_popup(&mut self, ui: &mut egui::Ui) {
        self.send_panel_body(ui, SendLayout::Popup);
    }

    // ── 统一核心渲染 ──

    fn send_panel_body(&mut self, ui: &mut egui::Ui, layout: SendLayout) {
        self.ensure_send_target_port();
        let send_port_open = self.send_target_port_open();

        // ── 1. 选项栏（顶部，自然高度）──
        self.render_send_options(ui, layout);

        // ── 2. 输入区（吃满中间）+ 操作栏（沉底）+ 错误提示 ──
        // 视觉顺序（自顶向底）：输入区 → 操作栏 → 错误提示。
        //
        // egui resizable 面板的高度跟随 content 的 min_rect（Frame outer_rect 写回 PanelState）：
        //   · content < panel → 面板缩回 content min_rect（拖大后回弹到 min_size）；
        //   · content > panel → 面板被撑大（正反馈涨到顶满）。
        // 要让面板稳定在用户拖动高度，content min_rect 必须恰好 = panel 内容区。
        // 末尾 `ui.take_available_space()` 把 content min_rect 撑到 max_rect（= panel 内容区），
        // 配合 dock 面板 `Frame::NONE`（margin=0），Frame outer = content = panel，PanelState
        // 写回 = panel，面板稳定。注意不能用 `set_min_height(max_rect.height())`——它在 cursor
        // 位置加 max_rect 高度（cursor+max_rect）会撑爆；take_available_space 用 available_size
        // （max_rect - cursor）正确撑到 max_rect。
        //
        // 输入区用 add_sized(available - 预留) 视觉吃满，操作栏在其下方；reserved 是操作栏估算
        // 高度，偏大也无妨（take_available_space 保证 content = panel，留白在操作栏下方）。
        let avail = ui.available_size();
        let reserved_for_bottom = match layout {
            SendLayout::Vertical => 150.0,
            SendLayout::Horizontal | SendLayout::Popup => 84.0,
        };
        let input_height = (avail.y - reserved_for_bottom).max(80.0);

        let resp = self.render_send_input(ui, layout, send_port_open, input_height);

        // 操作栏 + 错误提示（在输入区下方，沉底）
        self.render_send_actions(ui, layout, send_port_open);
        if let Some(err) = &self.send.error {
            ui.colored_label(theme::RED, err);
        }

        // 撑满 panel 内容区，打破 resizable 面板的正反馈（面板稳定在用户拖动高度）。
        ui.take_available_space();

        if resp.changed() {
            self.send.periodic_send_count = 0;
            self.send.error = None;
        }

        // Ctrl+Enter 发送统一由 keymap(Action::Send → handle_keys)处理,
        // 不在此处重复检测,避免同帧双重触发 do_send 导致重复发送。
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
        input_height: f32,
    ) -> egui::Response {
        let hint_text = match layout {
            SendLayout::Horizontal => {
                if send_port_open {
                    "Ctrl+Enter 发送 | ⛶ 放大编辑"
                } else {
                    "请选择已打开的串口"
                }
            }
            SendLayout::Vertical => "Ctrl+Enter 发送",
            SendLayout::Popup => "Ctrl+Enter 发送",
        };

        // 输入区用 add_sized 撑满传入高度（视觉吃满剩余空间）。
        // 配合 send_panel_body 末尾的 take_available_space() + dock 面板 Frame::NONE，
        // content min_rect = panel，打破 egui resizable 面板的正反馈（参见 send_panel_body 注释）。
        // 显式 id_salt 稳定控件 id（前面有条件渲染的 error 标签、hex_preview、contribution 槽）。
        let id_salt = match layout {
            SendLayout::Horizontal => "send-input-h",
            SendLayout::Vertical => "send-input-v",
            SendLayout::Popup => "send-input-popup",
        };
        let width = ui.available_width();
        ui.add_sized(
            egui::vec2(width, input_height),
            egui::TextEdit::multiline(&mut self.send.input)
                .desired_width(f32::INFINITY)
                .id_salt(id_salt)
                .hint_text(hint_text),
        )
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
        // 间隔合法性：空串视为未设置（可取消勾选但不能新启用）；非正数或非数字视为非法。
        let trimmed: String = self.send.periodic_interval_ms.trim().to_owned();
        let interval_valid = trimmed.is_empty()
            || trimmed.parse::<f64>().map(|v| v > 0.0).unwrap_or(false);
        // 已启用时即使输入变非法也允许取消勾选；未启用且非法时禁止勾选。
        let can_toggle = interval_valid || self.send.periodic_enabled;
        if ui
            .add_enabled(can_toggle, egui::Checkbox::new(&mut self.send.periodic_enabled, "周期发送"))
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

        // SAFETY: we returned early above if target_port is None
        let port = self
            .send
            .target_port
            .clone()
            .expect("target_port was checked non-None above");
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
        .map(|e| translate_error(&e));

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

    pub(super) fn send_history_combo(&mut self, ui: &mut egui::Ui, id_salt: &'static str) {
        if self.send.send_history.is_empty() {
            return;
        }

        ui.separator();
        // 历史 popup：搜索 + 单条删除 + 清空全部。点条目直接发送。
        let btn_resp = ui.button("历史");
        let popup_id = ui.id().with(id_salt);
        let popup = egui::Popup::from_response(&btn_resp)
            .open_memory(btn_resp.clicked().then_some(egui::SetOpenCommand::Toggle))
            .id(popup_id)
            .layout(egui::Layout::top_down(egui::Align::LEFT));

        popup.show(|ui| {
            ui.set_min_width(280.0);
            ui.set_max_width(380.0);

            // 搜索框
            ui.horizontal(|ui| {
                ui.label("搜索");
                ui.add(
                    egui::TextEdit::singleline(&mut self.send.history_search)
                        .desired_width(220.0)
                        .hint_text("过滤历史"),
                );
            });
            ui.separator();

            // 过滤后的条目（克隆避免在删除时同时遍历）
            let query = self.send.history_search.to_lowercase();
            let entries: Vec<String> = self
                .send
                .send_history
                .iter()
                .filter(|item| query.is_empty() || item.to_lowercase().contains(&query))
                .take(MAX_SEND_HISTORY)
                .cloned()
                .collect();

            if entries.is_empty() {
                ui.label(
                    egui::RichText::new(if self.send.send_history.is_empty() {
                        "无历史"
                    } else {
                        "无匹配"
                    })
                    .color(theme::TEXT_SECONDARY),
                );
            }

            egui::ScrollArea::vertical()
                .max_height(240.0)
                .show(ui, |ui| {
                    for item in &entries {
                        ui.horizontal(|ui| {
                            // 点击条目：填入 input 并直接发送
                            if ui
                                .add(
                                    egui::Button::new(shorten_for_ui(item, 40))
                                        .frame(false)
                                        .wrap_mode(egui::TextWrapMode::Truncate),
                                )
                                .clicked()
                            {
                                self.send.input = item.clone();
                                self.do_send();
                                egui::Popup::close_id(ui.ctx(), popup_id);
                            }
                            // 单条删除
                            if ui.small_button("×").clicked() {
                                self.send.send_history.retain(|h| h != item);
                                if let Err(e) = self.save_config() {
                                    log::warn!("save_config failed: {e}")
                                }
                            }
                        });
                    }
                });

            ui.separator();
            if ui.button("清空全部历史").clicked() {
                self.send.send_history.clear();
                if let Err(e) = self.save_config() {
                    log::warn!("save_config failed: {e}")
                }
                egui::Popup::close_id(ui.ctx(), popup_id);
            }
        });
    }
}

use tool_transport::{hex_preview, send_impl_to, translate_error};

fn shorten_for_ui(s: &str, max_chars: usize) -> String {
    let mut out = s.chars().take(max_chars).collect::<String>();
    if s.chars().count() > max_chars {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use eframe::egui;

    /// 验证发送区 top_down 布局：输入区（固定 desired_rows 的 ScrollArea）与操作栏不重叠。
    /// 输入区在上方，操作栏在下方，两者垂直方向不重叠。
    #[test]
    fn send_layout_input_does_not_overlap_actions() {
        egui::__run_test_ui(|ui| {
            ui.set_max_size(egui::vec2(600.0, 400.0));

            // 选项栏
            ui.horizontal(|ui| {
                ui.label("发送到");
                ui.label("COM1");
                ui.separator();
            });

            let input_resp = egui::ScrollArea::vertical()
                .id_salt("test-send-input")
                .max_height(200.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut String::new())
                            .desired_width(f32::INFINITY)
                            .desired_rows(8),
                    )
                })
                .inner;

            let actions_top = ui.cursor().min.y;

            ui.horizontal_wrapped(|ui| {
                let _ = ui.button("发送");
                let _ = ui.button("清空");
            });

            // 输入区底部应 ≤ 操作栏顶部（不重叠）
            let input_bottom = input_resp.rect.bottom();
            assert!(
                input_bottom <= actions_top + 0.5,
                "输入区底部 {} 超过操作栏顶部 {}，发生重叠",
                input_bottom,
                actions_top
            );
        });
    }
}

