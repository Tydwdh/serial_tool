//! Shared sender panel used by the Native and Web composition roots.
//!
//! The transport is deliberately represented by actions.  This keeps all
//! interaction and layout decisions in one place while allowing Native and
//! Web to submit the same application commands through their own runtimes.

use egui::widgets::text_edit::TextEditState;
use egui::{Id, Ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SendLineEnding {
    #[default]
    None,
    Lf,
    Cr,
    Crlf,
}

impl SendLineEnding {
    pub const ALL: [Self; 4] = [Self::None, Self::Lf, Self::Cr, Self::Crlf];

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::Lf => "LF",
            Self::Cr => "CR",
            Self::Crlf => "CRLF",
        }
    }

    pub const fn suffix(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Lf => "\n",
            Self::Cr => "\r",
            Self::Crlf => "\r\n",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendLayout {
    Horizontal,
    Vertical,
}

/// Width at which the sender can keep its option/action rows on one line.
/// Both composition roots use this same breakpoint so a Dock resize produces
/// the same sender layout on Native and Web.
pub const SEND_LAYOUT_BREAKPOINT: f32 = 420.0;

pub const fn send_layout_for_width(width: f32) -> SendLayout {
    if width < SEND_LAYOUT_BREAKPOINT {
        SendLayout::Vertical
    } else {
        SendLayout::Horizontal
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendPortItem {
    pub id: String,
    pub label: String,
}

/// A plugin button contributed to the shared sender toolbar.
///
/// The panel only renders the button and returns its identity as an action;
/// the Native/Web composition roots remain responsible for dispatching the
/// plugin command and building the platform-specific context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendToolbarButton {
    pub plugin_id: String,
    pub contribution_id: String,
    pub title: String,
    pub tooltip: Option<String>,
    pub order: i32,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendAction {
    SendText {
        port: String,
        text: String,
    },
    SendHex {
        port: String,
        hex: String,
        strict: bool,
    },
    SetDtr {
        port: String,
        value: bool,
    },
    SetRts {
        port: String,
        value: bool,
    },
    ActivateToolbar {
        plugin_id: String,
        contribution_id: String,
    },
}

/// Mutable sender state plus the current transport snapshot.
pub struct SendView<'a> {
    pub ports: &'a [SendPortItem],
    pub target_port: &'a mut Option<String>,
    pub target_open: bool,
    pub input: &'a mut String,
    pub hex_mode: &'a mut bool,
    pub hex_strict: &'a mut bool,
    pub line_ending: &'a mut SendLineEnding,
    pub error: &'a mut Option<String>,
    pub history: &'a mut Vec<String>,
    pub history_search: &'a mut String,
    pub history_index: &'a mut Option<usize>,
    pub saved_input: &'a mut String,
    pub periodic_enabled: &'a mut bool,
    pub periodic_interval_ms: &'a mut String,
    pub periodic_send_count: &'a mut u64,
    pub dtr: &'a mut bool,
    pub rts: &'a mut bool,
    pub toolbar_buttons: &'a [SendToolbarButton],
    pub max_history: usize,
    pub layout: SendLayout,
}

pub fn sender_ui(ui: &mut Ui, view: &mut SendView<'_>) -> Vec<SendAction> {
    let mut actions = Vec::new();
    let target_open = view.target_open;
    let layout = view.layout;

    render_options(ui, view);

    let available = ui.available_size();
    let reserved = match layout {
        SendLayout::Horizontal => 92.0,
        SendLayout::Vertical => 150.0,
    };
    let min_input = 40.0_f32.min(available.y.max(0.0));
    let input_height = (available.y - reserved).max(min_input);
    let response = render_input(ui, view, input_height, layout);

    render_actions(ui, view, target_open, &mut actions);
    if let Some(error) = view.error.as_deref() {
        ui.colored_label(crate::theme::red(), error);
    }
    ui.take_available_space();

    if response.changed() {
        *view.periodic_send_count = 0;
        *view.error = None;
    }
    actions
}

fn render_options(ui: &mut Ui, view: &mut SendView<'_>) {
    let row = |ui: &mut Ui, view: &mut SendView<'_>| {
        ui.label("发送到");
        let selected = view
            .target_port
            .as_deref()
            .and_then(|id| view.ports.iter().find(|port| port.id == id))
            .map(|port| port.label.clone())
            .unwrap_or_else(|| "请选择串口".to_owned());
        egui::ComboBox::from_id_salt("shared-send-target-port")
            .width(130.0)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if view.ports.is_empty() {
                    ui.add_enabled(false, egui::Label::new("无已打开串口"));
                } else {
                    for port in view.ports {
                        ui.selectable_value(view.target_port, Some(port.id.clone()), &port.label);
                    }
                }
            });
        ui.separator();
        ui.selectable_value(view.hex_mode, false, "文本");
        ui.selectable_value(view.hex_mode, true, "HEX");
        if *view.hex_mode {
            ui.checkbox(view.hex_strict, "严格")
                .on_hover_text("严格模式：每个 HEX token 必须是完整的两位字节");
        }
        ui.add_enabled_ui(!*view.hex_mode, |ui| {
            egui::ComboBox::from_id_salt("shared-send-line-ending")
                .width(72.0)
                .selected_text(view.line_ending.label())
                .show_ui(ui, |ui| {
                    for ending in SendLineEnding::ALL {
                        ui.selectable_value(view.line_ending, ending, ending.label());
                    }
                });
        });
    };

    if matches!(view.layout, SendLayout::Horizontal) {
        egui::ScrollArea::horizontal()
            .id_salt("shared-send-options-scroll")
            .max_height(34.0)
            .auto_shrink([false, true])
            .show(ui, |ui| ui.horizontal(|ui| row(ui, view)));
    } else {
        ui.horizontal_wrapped(|ui| row(ui, view));
    }
}

fn render_input(
    ui: &mut Ui,
    view: &mut SendView<'_>,
    input_height: f32,
    layout: SendLayout,
) -> egui::Response {
    let id = Id::new(match layout {
        SendLayout::Horizontal => "shared-send-input-horizontal",
        SendLayout::Vertical => "shared-send-input-vertical",
    });
    let cursor_before = ui.ctx().data_mut(|data| {
        data.get_persisted::<TextEditState>(id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index.into())
    });
    let hint = if view.target_open {
        "输入要发送的文本或 HEX，Ctrl+Enter 发送"
    } else {
        "请选择已打开的串口"
    };
    let response = egui::ScrollArea::vertical()
        .id_salt(match layout {
            SendLayout::Horizontal => "shared-send-input-scroll-horizontal",
            SendLayout::Vertical => "shared-send-input-scroll-vertical",
        })
        .max_height(input_height)
        .show(ui, |ui| {
            ui.add_sized(
                egui::vec2(ui.available_width(), input_height),
                egui::TextEdit::multiline(view.input)
                    .id(id)
                    .desired_width(f32::INFINITY)
                    .hint_text(hint),
            )
        })
        .inner;

    if response.has_focus() && !view.history.is_empty() {
        let cursor_after = ui.ctx().data_mut(|data| {
            data.get_persisted::<TextEditState>(id)
                .and_then(|state| state.cursor.char_range())
                .map(|range| range.primary.index.into())
        });
        let before = cursor_before.unwrap_or(0);
        let after = cursor_after.unwrap_or(before);
        let char_len = view.input.chars().count();
        let multiline = view.input.contains('\n');
        let first = !view.input.chars().take(before).any(|c| c == '\n');
        let last = !view.input.chars().skip(before).any(|c| c == '\n');
        let up = ui.input(|input| input.key_pressed(egui::Key::ArrowUp));
        let down = ui.input(|input| input.key_pressed(egui::Key::ArrowDown));
        let at_top = !multiline || (first && after == 0);
        let at_bottom = !multiline || (last && after == char_len);
        if up && at_top {
            match *view.history_index {
                None => {
                    *view.saved_input = view.input.clone();
                    *view.history_index = Some(0);
                    *view.input = view.history[0].clone();
                }
                Some(index) if index + 1 < view.history.len() => {
                    *view.history_index = Some(index + 1);
                    *view.input = view.history[index + 1].clone();
                }
                _ => {}
            }
        } else if down && at_bottom {
            match *view.history_index {
                Some(0) => {
                    *view.history_index = None;
                    *view.input = std::mem::take(view.saved_input);
                }
                Some(index) => {
                    *view.history_index = Some(index - 1);
                    *view.input = view.history[index - 1].clone();
                }
                None => {}
            }
        }
    } else if !response.has_focus() {
        *view.history_index = None;
        view.saved_input.clear();
    }

    response
}

fn render_actions(
    ui: &mut Ui,
    view: &mut SendView<'_>,
    target_open: bool,
    actions: &mut Vec<SendAction>,
) {
    let input = view.input.trim().to_owned();
    let hex_error = if *view.hex_mode && !input.is_empty() {
        hex_error(&input, *view.hex_strict)
    } else {
        None
    };
    let can_send = target_open && !input.is_empty() && hex_error.is_none();
    let target = view.target_port.clone();

    let render_main_row = |ui: &mut Ui, view: &mut SendView<'_>, actions: &mut Vec<SendAction>| {
        if ui
            .add_enabled(can_send, egui::Button::new("发送"))
            .on_disabled_hover_text(
                hex_error
                    .as_deref()
                    .unwrap_or("请先连接串口并输入要发送的内容"),
            )
            .clicked()
            && let Some(port) = target.clone()
        {
            if *view.hex_mode {
                actions.push(SendAction::SendHex {
                    port,
                    hex: view.input.clone(),
                    strict: *view.hex_strict,
                });
            } else {
                actions.push(SendAction::SendText {
                    port,
                    text: format!("{}{}", view.input, view.line_ending.suffix()),
                });
            }
            *view.periodic_send_count = 0;
            *view.history_index = None;
            view.saved_input.clear();
        }
        if ui.button("清空").clicked() {
            view.input.clear();
            *view.error = None;
            *view.periodic_send_count = 0;
        }
        render_history(ui, view);
        for button in view.toolbar_buttons {
            let response = ui.add_enabled(button.enabled, egui::Button::new(&button.title));
            let response = if let Some(tooltip) = button.tooltip.as_deref() {
                response.on_hover_text(tooltip)
            } else {
                response
            };
            if response.clicked() {
                actions.push(SendAction::ActivateToolbar {
                    plugin_id: button.plugin_id.clone(),
                    contribution_id: button.contribution_id.clone(),
                });
            }
        }
    };

    let render_secondary_row =
        |ui: &mut Ui, view: &mut SendView<'_>, actions: &mut Vec<SendAction>| {
            let interval = view.periodic_interval_ms.trim().to_owned();
            let valid = interval
                .parse::<f64>()
                .map(|value| value > 0.0)
                .unwrap_or(false);
            let can_toggle = valid || *view.periodic_enabled;
            if ui
                .add_enabled(
                    can_toggle,
                    egui::Checkbox::new(view.periodic_enabled, "周期发送"),
                )
                .on_disabled_hover_text("请输入大于 0 的发送间隔")
                .changed()
            {
                *view.periodic_send_count = 0;
            }
            if ui
                .add(egui::TextEdit::singleline(view.periodic_interval_ms).desired_width(72.0))
                .changed()
            {
                *view.periodic_send_count = 0;
            }
            ui.label("ms");
            if !valid && *view.periodic_enabled {
                ui.colored_label(crate::theme::yellow(), "间隔必须 > 0ms");
            }
            ui.separator();

            let enabled = target_open && target.is_some();
            ui.add_enabled_ui(enabled, |ui| {
                if ui.checkbox(view.dtr, "DTR").changed()
                    && let Some(port) = target.clone()
                {
                    actions.push(SendAction::SetDtr {
                        port,
                        value: *view.dtr,
                    });
                }
                if ui.checkbox(view.rts, "RTS").changed()
                    && let Some(port) = target.clone()
                {
                    actions.push(SendAction::SetRts {
                        port,
                        value: *view.rts,
                    });
                }
            });

            if *view.hex_mode && !input.is_empty() {
                ui.monospace(format!("HEX: {}", hex_preview(&input)));
            }
            if let Some(error) = hex_error.as_deref() {
                ui.colored_label(crate::theme::red(), error);
            }
        };

    if matches!(view.layout, SendLayout::Horizontal) {
        egui::ScrollArea::horizontal()
            .id_salt("shared-send-actions-main-scroll")
            .max_height(44.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| render_main_row(ui, view, actions));
            });
        egui::ScrollArea::horizontal()
            .id_salt("shared-send-actions-secondary-scroll")
            .max_height(44.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| render_secondary_row(ui, view, actions));
            });
    } else {
        ui.horizontal_wrapped(|ui| render_main_row(ui, view, actions));
        ui.horizontal_wrapped(|ui| render_secondary_row(ui, view, actions));
    }
}

fn render_history(ui: &mut Ui, view: &mut SendView<'_>) {
    if view.history.is_empty() {
        return;
    }
    ui.menu_button("历史", |ui| {
        ui.set_min_width(320.0);
        ui.add(egui::TextEdit::singleline(view.history_search).hint_text("过滤历史"));
        let query = view.history_search.trim().to_lowercase();
        let entries: Vec<String> = view
            .history
            .iter()
            .filter(|item| query.is_empty() || item.to_lowercase().contains(&query))
            .take(view.max_history)
            .cloned()
            .collect();
        if entries.is_empty() {
            ui.label("暂无匹配项");
        } else {
            let mut selected = None;
            let mut deleted = None;
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .show(ui, |ui| {
                    for item in entries {
                        ui.horizontal(|ui| {
                            if ui.button(&item).clicked() {
                                selected = Some(item.clone());
                                ui.close();
                            }
                            if ui.small_button("×").clicked() {
                                deleted = Some(item);
                            }
                        });
                    }
                });
            if let Some(item) = selected {
                *view.input = item;
                *view.history_index = None;
            }
            if let Some(item) = deleted {
                view.history.retain(|candidate| candidate != &item);
            }
        }
        if ui.button("清空历史").clicked() {
            view.history.clear();
            view.history_search.clear();
            ui.close();
        }
    });
}

fn hex_error(input: &str, strict: bool) -> Option<String> {
    let tokens = input
        .trim()
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',' || ch == ';')
        .filter(|token| !token.is_empty());
    let mut found = false;
    for token in tokens {
        found = true;
        let normalized = token
            .strip_prefix("0x")
            .or_else(|| token.strip_prefix("0X"))
            .unwrap_or(token)
            .replace(['_', '-'], "");
        if normalized.is_empty() {
            return Some("HEX 中包含空 token".to_owned());
        }
        if strict && normalized.len() != 2 {
            return Some(format!("严格 HEX 模式要求每个 token 是两位：{token}"));
        }
        if normalized.chars().any(|c| !c.is_ascii_hexdigit()) {
            return Some(format!("HEX 中包含无效字符：{token}"));
        }
    }
    (!found).then_some("HEX 输入为空".to_owned())
}

fn hex_preview(input: &str) -> String {
    if let Some(error) = hex_error(input, false) {
        if error == "HEX 输入为空" {
            return String::new();
        }
        return format!("解析失败：{error}");
    }
    let mut bytes = Vec::new();
    for token in input
        .trim()
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',' || ch == ';')
        .filter(|token| !token.is_empty())
    {
        let mut normalized = token
            .strip_prefix("0x")
            .or_else(|| token.strip_prefix("0X"))
            .unwrap_or(token)
            .replace(['_', '-'], "");
        if normalized.len() > 2 && !normalized.len().is_multiple_of(2) {
            normalized.insert(0, '0');
        }
        if normalized.len() == 1 {
            normalized.insert(0, '0');
        }
        bytes.extend(normalized.as_bytes().chunks(2).map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).unwrap_or_default(), 16).unwrap_or(0)
        }));
    }
    if bytes.is_empty() {
        return String::new();
    }
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn record_history(history: &mut Vec<String>, text: impl Into<String>, max_history: usize) {
    let text = text.into();
    if text.trim().is_empty() {
        return;
    }
    history.retain(|item| item != &text);
    history.insert(0, text);
    history.truncate(max_history);
}
