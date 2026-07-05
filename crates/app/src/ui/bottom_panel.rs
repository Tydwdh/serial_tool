use crate::app::WorkbenchApp;
use crate::state::{LineEnding, MAX_SEND_HISTORY, StatusLevel};
use eframe::egui;
use egui::widgets::text_edit::TextEditState;
use tool_panels::theme;

/// 返回发送输入框的稳定 Id（用于读取光标状态）。
fn response_id_for_send_input(layout: SendLayout) -> egui::Id {
    let salt = match layout {
        SendLayout::Horizontal => "send-input-h",
        SendLayout::Vertical => "send-input-v",
        SendLayout::Popup => "send-input-popup",
    };
    egui::Id::new(salt)
}

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
                    .unwrap_or_else(|| "请选择串口".to_owned()),
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
                    let pin_label = if self.popups.send_always_on_top {
                        "\u{1f4cc} 置顶"
                    } else {
                        "置顶"
                    };
                    if ui
                        .selectable_label(self.popups.send_always_on_top, pin_label)
                        .on_hover_text("让该窗口保持在其他窗口上方")
                        .clicked()
                    {
                        self.popups.send_always_on_top = !self.popups.send_always_on_top;
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
        // 显式 id 稳定控件 id（前面有条件渲染的 error 标签、hex_preview、contribution 槽）。
        let width = ui.available_width();

        // 在 TextEdit 渲染之前读取光标位置，用于 ↑↓ 历史导航判断。
        // TextEdit 渲染后才处理 key_pressed，所以需要提前捕获「按键前」的光标行号。
        let edit_id = response_id_for_send_input(layout);
        let cursor_before = ui.ctx().data_mut(|d| {
            d.get_persisted::<TextEditState>(edit_id)
                .and_then(|s| s.cursor.char_range())
                .map(|r| r.primary.index)
        });

        let response = ui.add_sized(
            egui::vec2(width, input_height),
            egui::TextEdit::multiline(&mut self.send.input)
                .desired_width(f32::INFINITY)
                .id(edit_id)
                .hint_text(hint_text),
        );

        // ↑↓ 方向键导航发送历史。
        // 多行编辑时，方向键优先移动光标；只有当 egui 已无法跨行移动光标
        // （按键后光标 char index 未变化，说明顶到了首行/末行边界）时才切换历史。
        // 单行输入（无换行符）保持「按 ↑/↓ 直接切历史」的命令行习惯。
        if response.has_focus() && !self.send.send_history.is_empty() {
            let history_len = self.send.send_history.len();

            // TextEdit 已在本帧处理完方向键移动，读取「按键后」的光标位置。
            let cursor_after = ui.ctx().data_mut(|d| {
                d.get_persisted::<TextEditState>(edit_id)
                    .and_then(|s| s.cursor.char_range())
                    .map(|r| r.primary.index)
            });

            let multiline = self.send.input.contains('\n');
            let char_len = self.send.input.chars().count();
            let before: usize = cursor_before.map(|p| p.into()).unwrap_or(0);
            let after: usize = cursor_after.map(|p| p.into()).unwrap_or(0);

            // 单行：直接切历史。
            // 多行 ↑：光标在首段（前无 \n）且 egui 已顶到文本开头（after == 0）才切历史，
            //         这样首段任意位置按 ↑ 直接切到上一条，不经过「先移到行首」。
            // 多行 ↓：光标在末段（后无 \n）且 egui 已顶到文本末尾（after == char_len）才切历史。
            let before_in_first_para = !self.send.input.chars().take(before).any(|c| c == '\n');
            let before_in_last_para = !self.send.input.chars().skip(before).any(|c| c == '\n');
            let stuck_at_top = if multiline {
                before_in_first_para && after == 0
            } else {
                true
            };
            let stuck_at_bottom = if multiline {
                before_in_last_para && after == char_len
            } else {
                true
            };

            let up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
            let down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));

            if up && stuck_at_top {
                match self.send.history_index {
                    None => {
                        self.send.saved_input = self.send.input.clone();
                        self.send.history_index = Some(0);
                        self.send.input = self.send.send_history[0].clone();
                    }
                    Some(i) if i + 1 < history_len => {
                        self.send.history_index = Some(i + 1);
                        self.send.input = self.send.send_history[i + 1].clone();
                    }
                    _ => {}
                }
            } else if down && stuck_at_bottom {
                match self.send.history_index {
                    None => {}
                    Some(0) => {
                        self.send.history_index = None;
                        self.send.input = std::mem::take(&mut self.send.saved_input);
                    }
                    Some(i) => {
                        self.send.history_index = Some(i - 1);
                        self.send.input = self.send.send_history[i - 1].clone();
                    }
                }
            }
        } else if !response.has_focus() {
            // 失焦重置导航
            self.send.history_index = None;
            self.send.saved_input.clear();
        }

        response
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
        // HEX 模式下实时检查输入是否可解析（严格模式 vs 宽松模式）。
        let input_trim = self.send.input.trim();
        let hex_error = if self.send.hex_mode && !input_trim.is_empty() {
            match tool_transport::parse_hex(input_trim) {
                Ok(_) => None,
                Err(e) => Some(e.to_string()),
            }
        } else {
            None
        };
        let can_send = send_port_open
            && !input_trim.is_empty()
            && hex_error.is_none();

        let mut send_btn = ui.add_enabled(can_send, egui::Button::new("发送"));
        if let Some(ref err) = hex_error {
            send_btn = send_btn.on_disabled_hover_text(format!("HEX 解析失败: {err}"));
        }
        if send_btn.clicked() {
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
        // 间隔合法性：空串视为未设置（仅允许取消勾选，不允许新启用）；
        // 非正数或非数字视为非法。
        let trimmed: String = self.send.periodic_interval_ms.trim().to_owned();
        let interval_valid =
            !trimmed.is_empty() && trimmed.parse::<f64>().map(|v| v > 0.0).unwrap_or(false);
        // 已启用时即使输入变非法也允许取消勾选；未启用且非法时禁止勾选。
        let can_toggle = interval_valid || self.send.periodic_enabled;
        if ui
            .add_enabled(
                can_toggle,
                egui::Checkbox::new(&mut self.send.periodic_enabled, "周期发送"),
            )
            .on_disabled_hover_text("请设置有效的发送间隔（> 0 ms）")
            .changed()
        {
            self.send.periodic_send_count = 0;
            if !self.send.periodic_enabled
                && let Some(cancel) = self.periodic_send.cancel.take()
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
        } else if self.send.periodic_enabled {
            // 不应出现（空串已禁止启用），但防御性提示
            ui.colored_label(theme::YELLOW, "请设置间隔");
        }
    }

    // ── HEX 预览 ──

    fn render_hex_preview(&mut self, ui: &mut egui::Ui) {
        if self.send.hex_mode && !self.send.input.trim().is_empty() {
            let preview = hex_preview(&self.send.input);
            let is_err = preview.starts_with("解析失败");
            ui.label(
                egui::RichText::new(format!("HEX: {preview}"))
                    .color(if is_err {
                        theme::RED
                    } else {
                        theme::TEXT_SECONDARY
                    })
                    .monospace()
                    .small(),
            )
            .on_hover_text(if is_err {
                match tool_transport::parse_hex(self.send.input.trim()) {
                    Ok(_) => String::new(),
                    Err(e) => format!("HEX 解析失败: {e}"),
                }
            } else {
                String::new()
            });
        }
    }

    // ── 信号控制 ──

    fn send_signal_controls(&mut self, ui: &mut egui::Ui) {
        if self.send.target_port.is_none() {
            ui.add_enabled(false, egui::Checkbox::new(&mut self.send.dtr_high, "DTR"))
                .on_disabled_hover_text("请先选择发送目标串口");
            ui.add_enabled(false, egui::Checkbox::new(&mut self.send.rts_high, "RTS"))
                .on_disabled_hover_text("请先选择发送目标串口");
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
            let dtr_resp = ui.checkbox(&mut dtr, "DTR");
            if dtr_resp.changed() {
                match self.transport.set_dtr(&port, dtr) {
                    Ok(()) => self.send.dtr_high = dtr,
                    Err(e) => self.set_status_force(StatusLevel::Error, e.to_string()),
                }
            }
            dtr_resp.on_hover_text(
                "数据终端就绪 (DTR) 电平。点击会立即驱动该线路，部分设备会用它触发复位/进入 bootload，请谨慎切换。",
            );

            let mut rts = self.send.rts_high;
            let rts_resp = ui.checkbox(&mut rts, "RTS");
            if rts_resp.changed() {
                match self.transport.set_rts(&port, rts) {
                    Ok(()) => self.send.rts_high = rts,
                    Err(e) => self.set_status_force(StatusLevel::Error, e.to_string()),
                }
            }
            rts_resp.on_hover_text(
                "请求发送 (RTS) 电平。点击会立即驱动该线路，部分设备会用它触发复位/进入 bootload，请谨慎切换。",
            );
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
        self.send.history_index = None;
        self.send.saved_input.clear();
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
        let btn_resp = ui.button("历史");
        let popup_id = ui.id().with(id_salt);
        let popup = egui::Popup::from_response(&btn_resp)
            .open_memory(btn_resp.clicked().then_some(egui::SetOpenCommand::Toggle))
            .id(popup_id)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .layout(egui::Layout::top_down(egui::Align::LEFT));

        // pending 模式：popup 闭包内只收集要执行的动作，闭包返回后再
        // 调用 do_send / retain / clear。直接在 egui 回调里 &mut self 调
        // do_send 会触发深度借用崩溃（do_send → record_send_history 会修改
        // 正被遍历的 send_history）。
        enum PendingHistory {
            Send(String),
            Delete(String),
            Clear,
        }
        let mut pending: Option<PendingHistory> = None;

        popup.show(|ui| {
            ui.set_min_width(320.0);
            ui.set_max_width(420.0);
            ui.spacing_mut().item_spacing.y = 4.0;

            // 标题
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("发送历史")
                        .strong()
                        .color(theme::TEXT_WHITE),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{} 条", self.send.send_history.len()))
                            .small()
                            .color(theme::TEXT_DIMMED),
                    );
                });
            });

            // 搜索框：占满宽度
            ui.add_space(2.0);
            let search_resp = ui.add(
                egui::TextEdit::singleline(&mut self.send.history_search)
                    .desired_width(f32::INFINITY)
                    .hint_text("🔍 过滤历史…"),
            );
            if !search_resp.has_focus() {
                search_resp.request_focus();
            }

            ui.add_space(2.0);
            ui.separator();
            ui.add_space(2.0);

            // 过滤后的条目（克隆，避免遍历与删除/发送同时持有引用）
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
                ui.vertical_centered(|ui| {
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new(if self.send.send_history.is_empty() {
                            "暂无历史"
                        } else {
                            "无匹配项"
                        })
                        .color(theme::TEXT_DIMMED),
                    );
                    ui.add_space(12.0);
                });
            } else {
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        // item_spacing.y = 0 让行严格紧贴，分隔线贴在行底 = 下一行顶，
                        // 否则默认 item_spacing 会让分隔线悬在两行间隙里、视觉错位。
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let del_width = 28.0;
                        // 在循环外取一次列宽，避免每行 available_width() 随
                        // min_rect 增长而变大，导致删除按钮越往下越靠右。
                        let col_width = ui.available_width();
                        let text_width = (col_width - del_width).max(60.0);
                        let galley_width = (text_width - 16.0).max(20.0);
                        let count = entries.len();
                        // 历史条目用稍大字体，便于阅读。
                        let font_id = egui::FontId::proportional(
                            ui.style()
                                .text_styles
                                .get(&egui::TextStyle::Body)
                                .map(|f| f.size)
                                .map(|s| s.max(15.0))
                                .unwrap_or(15.0),
                        );
                        let min_row_height: f32 = 28.0;
                        let row_padding = 8.0; // 上下各 4px

                        for (idx, item) in entries.iter().enumerate() {
                            // 按 \n 拆成多段，每段独立 Truncate（超宽截断加 …），
                            // 然后垂直拼接。实现 \n 换行 + 每行截断两种效果。
                            let segments: Vec<std::sync::Arc<egui::Galley>> = item
                                .split('\n')
                                .map(|seg| {
                                    egui::WidgetText::from(seg)
                                        .color(theme::TEXT_PRIMARY)
                                        .into_galley(
                                            ui,
                                            Some(egui::TextWrapMode::Truncate),
                                            galley_width,
                                            font_id.clone(),
                                        )
                                })
                                .collect();
                            let total_text_h: f32 = segments.iter().map(|g| g.size().y).sum();
                            let row_height = min_row_height.max(total_text_h + row_padding);

                            let row_resp = ui.allocate_ui_with_layout(
                                egui::vec2(col_width, row_height),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    // 文字区：固定宽 + click 命中
                                    let text_resp = ui
                                        .allocate_exact_size(
                                            egui::vec2(text_width, row_height),
                                            egui::Sense::click(),
                                        )
                                        .1;
                                    let trect = text_resp.rect;
                                    if text_resp.hovered() {
                                        ui.painter().rect_filled(trect, 3.0, theme::BG_HOVER);
                                    }
                                    // 逐个绘制每段 galley（按 \n 拆分的），垂直排列
                                    let mut y = trect.center().y - total_text_h / 2.0;
                                    for g in &segments {
                                        ui.painter().galley(
                                            egui::pos2(trect.left() + 8.0, y),
                                            g.clone(),
                                            theme::TEXT_PRIMARY,
                                        );
                                        y += g.size().y;
                                    }
                                    if text_resp.clicked() {
                                        pending = Some(PendingHistory::Send(item.clone()));
                                    }
                                    text_resp
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .on_hover_text("点击填入发送框");

                                    // 删除区：固定宽 + click 命中，手绘 × 不用 Button
                                    let del_resp = ui
                                        .allocate_exact_size(
                                            egui::vec2(del_width, row_height),
                                            egui::Sense::click(),
                                        )
                                        .1;
                                    let drect = del_resp.rect;
                                    if del_resp.hovered() {
                                        ui.painter().rect_filled(drect, 3.0, theme::BG_HOVER);
                                    }
                                    ui.painter().text(
                                        drect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "×",
                                        font_id.clone(),
                                        if del_resp.hovered() {
                                            theme::TEXT_PRIMARY
                                        } else {
                                            theme::TEXT_SECONDARY
                                        },
                                    );
                                    if del_resp.clicked() {
                                        pending = Some(PendingHistory::Delete(item.clone()));
                                    }
                                    del_resp
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .on_hover_text("删除该条");
                                },
                            );

                            // 条目间分隔线：画在该行 rect 的底边，每行位置统一。
                            if idx + 1 < count {
                                let rect = row_resp.response.rect;
                                let y = rect.bottom();
                                ui.painter().line_segment(
                                    [
                                        egui::pos2(rect.left() + 4.0, y),
                                        egui::pos2(rect.right() - 4.0, y),
                                    ],
                                    egui::Stroke::new(1.0, theme::BORDER),
                                );
                            }
                        }
                    });
            }

            ui.add_space(2.0);
            ui.separator();
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                // 两步确认：避免误触一次性删除所有发送历史。
                let confirm_id = ui.id().with("clear_history_armed");
                let now = ui.input(|i| i.time);
                let armed_ts: Option<f64> = ui.ctx().memory(|m| m.data.get_temp(confirm_id));
                let armed = armed_ts.is_some_and(|t| now - t < 3.0);
                let label = if armed { "确认清空?" } else { "清空全部" };
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(label).color(theme::RED).small(),
                        )
                        .frame(true),
                    )
                    .on_hover_text(if armed { "再次点击确认清空" } else { "删除所有历史记录" })
                    .clicked()
                {
                    if armed {
                        pending = Some(PendingHistory::Clear);
                        ui.ctx().memory_mut(|m| m.data.remove_temp::<f64>(confirm_id));
                    } else {
                        ui.ctx().memory_mut(|m| m.data.insert_temp(confirm_id, now));
                    }
                }
                if armed
                    && ui.small_button("取消").clicked() {
                        ui.ctx().memory_mut(|m| m.data.remove_temp::<f64>(confirm_id));
                    }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("点击条目填入发送框")
                            .small()
                            .color(theme::TEXT_DIMMED),
                    );
                });
            });
        });

        // 闭包外执行实际状态变更
        match pending.take() {
            Some(PendingHistory::Send(text)) => {
                // 填入发送编辑器并关闭 popup
                self.send.input = text;
                egui::Popup::close_id(ui.ctx(), popup_id);
            }
            Some(PendingHistory::Delete(text)) => {
                self.send.send_history.retain(|h| h != &text);
                if let Err(e) = self.save_config() {
                    log::warn!("save_config failed: {e}")
                }
            }
            Some(PendingHistory::Clear) => {
                self.send.send_history.clear();
                self.send.history_search.clear();
                if let Err(e) = self.save_config() {
                    log::warn!("save_config failed: {e}")
                }
                egui::Popup::close_id(ui.ctx(), popup_id);
            }
            None => {}
        }
    }
}

use tool_transport::{hex_preview, send_impl_to, translate_error};

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

    /// 验证历史条目（短文本）行高=最小行高、间距=item_spacing=0、分隔线落在行底。
    /// 复现 popup 内 ScrollArea 的行布局：allocate_ui_with_layout + item_spacing.y=0。
    #[test]
    fn history_rows_uniform_height_and_separator_aligned() {
        egui::__run_test_ui(|ui| {
            ui.set_min_size(egui::vec2(360.0, 400.0));

            // 短条目：确保触发最小行高
            let entries: Vec<String> = (0..5).map(|i| format!("item-{i}")).collect();
            let row_height = 28.0; // 最小行高
            let del_width = 28.0;
            let col_width = ui.available_width();
            let text_width = (col_width - del_width).max(60.0);
            let count = entries.len();

            let mut row_rects: Vec<egui::Rect> = Vec::new();
            let mut sep_ys: Vec<f32> = Vec::new();

            egui::ScrollArea::vertical()
                .id_salt("test-hist-scroll")
                .max_height(320.0)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (idx, item) in entries.iter().enumerate() {
                        let row_resp = ui.allocate_ui_with_layout(
                            egui::vec2(col_width, row_height),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                let _ = ui.allocate_exact_size(
                                    egui::vec2(text_width, row_height),
                                    egui::Sense::click(),
                                );
                                let _ = ui.allocate_exact_size(
                                    egui::vec2(del_width, row_height),
                                    egui::Sense::click(),
                                );
                                let _ = item;
                            },
                        );
                        row_rects.push(row_resp.response.rect);
                        if idx + 1 < count {
                            let rect = row_resp.response.rect;
                            sep_ys.push(rect.bottom());
                        }
                    }
                });

            // 短条目每行高度应等于最小行高
            for (i, r) in row_rects.iter().enumerate() {
                let h = r.height();
                assert!(
                    (h - row_height).abs() < 0.5,
                    "第 {i} 行高度 {h} != 期望 {row_height}"
                );
            }

            // 分隔线 y 应等于对应行底（第 i 条分隔线在第 i 行底）
            for (i, &sy) in sep_ys.iter().enumerate() {
                let bottom = row_rects[i].bottom();
                assert!(
                    (sy - bottom).abs() < 0.5,
                    "分隔线 {i} y={sy} 与行底 {bottom} 不对齐"
                );
            }

            // 中间行间距应等于 row_height（ScrollArea 底部对最后一行有额外预留，
            // 所以只检查中间行的连续性，不检查最后一行后的间距）。
            assert!(row_rects.len() >= 3);
            let mid_gap = row_rects[2].top() - row_rects[1].top();
            assert!(
                (mid_gap - row_height).abs() < 0.5,
                "中间行间距 {mid_gap} != {row_height}，行高不一致"
            );
        });
    }
}
