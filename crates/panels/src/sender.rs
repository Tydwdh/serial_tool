//! Sender 面板：串口发送 UI 组件。
//!
//! 从 `app/src/ui/bottom_panel.rs` 移入，消除 app crate 中的面板实现寄生。

use egui::{RichText, TextEdit};
use std::collections::VecDeque;
use tool_transport::{TransportManager, hex_preview, send_impl_to};

use crate::theme;

const MAX_SEND_HISTORY: usize = 200;

/// 发送面板状态。
pub struct SenderPanel {
    pub input: String,
    pub hex_mode: bool,
    pub hex_strict: bool,
    pub line_ending_suffix: &'static str,
    pub error: Option<String>,
    pub target_port: Option<String>,
    pub send_history: VecDeque<String>,
}

impl Default for SenderPanel {
    fn default() -> Self {
        Self {
            input: String::new(),
            hex_mode: false,
            hex_strict: true,
            line_ending_suffix: "",
            error: None,
            target_port: None,
            send_history: VecDeque::new(),
        }
    }
}

impl SenderPanel {
    /// 执行发送操作。返回发送的文本（用于历史记录）。
    pub fn do_send(&mut self, transport: &TransportManager) -> Option<String> {
        let port = match self.target_port.as_deref() {
            Some(p) => p,
            None => {
                self.error = Some("请选择发送目标串口".into());
                return None;
            }
        };

        self.error = send_impl_to(
            port,
            &self.input,
            self.hex_mode,
            self.line_ending_suffix,
            self.hex_strict,
            transport,
        )
        .err()
        .map(|e| e.to_string());

        if self.error.is_none() && !self.input.trim().is_empty() {
            Some(self.input.clone())
        } else {
            None
        }
    }

    /// 记录发送历史（去重，限制长度）。
    pub fn record_send_history(&mut self, text: impl Into<String>) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }
        if self.send_history.front() == Some(&text) {
            return;
        }
        if let Some(index) = self.send_history.iter().position(|c| c == &text) {
            self.send_history.remove(index);
        }
        self.send_history.push_front(text);
        while self.send_history.len() > MAX_SEND_HISTORY {
            self.send_history.pop_back();
        }
    }

    /// 渲染 HEX 预览。
    pub fn render_hex_preview(&self, ui: &mut egui::Ui) {
        let preview = hex_preview(&self.input);
        if !preview.is_empty() {
            ui.colored_label(theme::TEXT_DIMMED, format!("HEX: {preview}"));
        }
    }

    /// 渲染发送输入区域。
    pub fn send_input_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.hex_mode, "HEX");
            if self.hex_mode {
                ui.checkbox(&mut self.hex_strict, "严格");
            }
        });

        let hint = if self.hex_mode {
            "HEX 发送 (如: 01 02 FF)"
        } else {
            "输入发送内容，Enter 发送"
        };
        let text_edit = TextEdit::multiline(&mut self.input)
            .hint_text(hint)
            .desired_rows(3)
            .desired_width(f32::INFINITY);
        ui.add(text_edit);

        if self.hex_mode {
            self.render_hex_preview(ui);
        }

        if let Some(ref err) = self.error {
            ui.colored_label(theme::RED, err);
        }
    }

    /// 渲染发送历史下拉框。
    pub fn send_history_combo(&mut self, ui: &mut egui::Ui, id_salt: &'static str) {
        if self.send_history.is_empty() {
            return;
        }
        let entries: Vec<String> = self
            .send_history
            .iter()
            .take(MAX_SEND_HISTORY)
            .cloned()
            .collect();
        ui.separator();
        egui::ComboBox::from_id_salt(id_salt)
            .width(140.0)
            .selected_text("发送历史")
            .show_ui(ui, |ui| {
                for entry in &entries {
                    let label = if entry.len() > 40 {
                        format!("{}...", &entry[..37])
                    } else {
                        entry.clone()
                    };
                    if ui
                        .selectable_label(false, RichText::new(&label).monospace())
                        .clicked()
                    {
                        self.input = entry.clone();
                    }
                }
            });
    }
}
