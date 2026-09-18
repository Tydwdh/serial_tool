use crate::app::WorkbenchApp;
use crate::state::{LineEnding, MAX_SEND_HISTORY, SendUiState, StatusLevel};
use eframe::egui;
use tool_panels::{
    SendAction, SendLayout, SendLineEnding, SendPortItem, SendToolbarButton, SendView,
    record_history as record_shared_send_history, sender_ui as shared_sender_ui,
};

impl WorkbenchApp {
    /// 确保 send.target_port 指向一个已打开的端口（自动回退逻辑）。
    pub(crate) fn ensure_send_target_port(&mut self) {
        let open_ports = self.workbench.open_port_names();

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
                .filter(|p| self.workbench.transport_status(p).open)
                .or_else(|| open_ports.first().cloned());
        }
    }

    pub(crate) fn send_target_port_open(&self) -> bool {
        self.send
            .target_port
            .as_deref()
            .is_some_and(|p| self.workbench.transport_status(p).open)
    }

    // ── 对外入口：两种工作区布局都走同一核心方法 ──

    pub(crate) fn send_panel_horizontal(&mut self, ui: &mut egui::Ui) {
        self.send_panel_body(ui, SendLayout::Horizontal);
    }

    pub(crate) fn send_panel_vertical(&mut self, ui: &mut egui::Ui) {
        self.send_panel_body(ui, SendLayout::Vertical);
    }

    // ── 统一核心渲染 ──

    fn send_panel_body(&mut self, ui: &mut egui::Ui, layout: SendLayout) {
        self.shared_send_panel_body(ui, layout);
    }

    fn shared_send_panel_body(&mut self, ui: &mut egui::Ui, layout: SendLayout) {
        self.ensure_send_target_port();
        let open_ports = self.workbench.open_port_names();
        let ports = open_ports
            .iter()
            .map(|port| SendPortItem {
                id: port.clone(),
                label: self.serial.port_label(port),
            })
            .collect::<Vec<_>>();
        let target_open = self.send_target_port_open();
        let mut line_ending = match self.send.line_ending {
            LineEnding::None => SendLineEnding::None,
            LineEnding::Lf => SendLineEnding::Lf,
            LineEnding::Cr => SendLineEnding::Cr,
            LineEnding::Crlf => SendLineEnding::Crlf,
        };
        let mut history = self.send.send_history.iter().cloned().collect::<Vec<_>>();
        let shared_layout = layout;
        let was_periodic = self.send.periodic_enabled;
        let toolbar_buttons: Vec<SendToolbarButton> = self.send_toolbar_buttons();
        let actions = {
            let SendUiState {
                input,
                hex_mode,
                error,
                target_port,
                history_search,
                history_index,
                saved_input,
                hex_strict,
                dtr_high,
                rts_high,
                periodic_enabled,
                periodic_interval_ms,
                periodic_send_count,
                ..
            } = &mut self.send;
            let mut view = SendView {
                ports: &ports,
                target_port,
                target_open,
                input,
                hex_mode,
                hex_strict,
                line_ending: &mut line_ending,
                error,
                history: &mut history,
                history_search,
                history_index,
                saved_input,
                periodic_enabled,
                periodic_interval_ms,
                periodic_send_count,
                dtr: dtr_high,
                rts: rts_high,
                toolbar_buttons: &toolbar_buttons,
                max_history: MAX_SEND_HISTORY,
                layout: shared_layout,
            };
            shared_sender_ui(ui, &mut view)
        };
        self.send.line_ending = match line_ending {
            SendLineEnding::None => LineEnding::None,
            SendLineEnding::Lf => LineEnding::Lf,
            SendLineEnding::Cr => LineEnding::Cr,
            SendLineEnding::Crlf => LineEnding::Crlf,
        };
        if was_periodic
            && !self.send.periodic_enabled
            && let Some(cancel) = self.periodic_send.cancel.take()
        {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        for action in actions {
            // SendText / SendHex 只差命令载荷，历史录制与错误处理共用同一段。
            let command = match action {
                SendAction::SendText { port, text } => tool_application::AppCommand::SendText {
                    port: tool_platform::PortId::new(port),
                    text,
                },
                SendAction::SendHex { port, hex, strict } => {
                    tool_application::AppCommand::SendHex {
                        port: tool_platform::PortId::new(port),
                        hex,
                        strict,
                    }
                }
                SendAction::SetDtr { port, value } => {
                    if let Err(error) =
                        self.workbench
                            .dispatch(tool_application::AppCommand::SetDtr {
                                port: tool_platform::PortId::new(port),
                                value,
                            })
                    {
                        self.set_status_force(StatusLevel::Error, error.to_string());
                    }
                    continue;
                }
                SendAction::SetRts { port, value } => {
                    if let Err(error) =
                        self.workbench
                            .dispatch(tool_application::AppCommand::SetRts {
                                port: tool_platform::PortId::new(port),
                                value,
                            })
                    {
                        self.set_status_force(StatusLevel::Error, error.to_string());
                    }
                    continue;
                }
                SendAction::ActivateToolbar {
                    plugin_id,
                    contribution_id,
                } => {
                    self.activate_send_toolbar_button(&plugin_id, &contribution_id);
                    continue;
                }
            };
            let history_text = self.send.input.clone();
            match self.workbench.dispatch(command) {
                Ok(_) => record_shared_send_history(&mut history, history_text, MAX_SEND_HISTORY),
                Err(error) => self.send.error = Some(error.to_string()),
            }
        }
        self.send.send_history = history.into_iter().collect();
        self.ui_contribution_non_button_slot(ui, "send.toolbar");
    }

    // ── 发送逻辑 ──

    pub(crate) fn do_send(&mut self) {
        let Some(port) = self.send.target_port.clone() else {
            self.send.error = Some("请选择发送目标串口".into());
            return;
        };
        let input = self.send.input.clone();
        if input.trim().is_empty() {
            self.send.error = Some("发送内容不能为空".to_owned());
        } else {
            let command = if self.send.hex_mode {
                tool_application::AppCommand::SendHex {
                    port: tool_platform::PortId::new(port),
                    hex: input.clone(),
                    strict: self.send.hex_strict,
                }
            } else {
                tool_application::AppCommand::SendText {
                    port: tool_platform::PortId::new(port),
                    text: format!("{}{}", input, self.send.line_ending.suffix()),
                }
            };
            self.send.error = self
                .workbench
                .dispatch(command)
                .err()
                .map(|e| e.to_string());
        }

        if self.send.error.is_none() && !self.send.input.trim().is_empty() {
            self.record_send_history(input);
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
}
