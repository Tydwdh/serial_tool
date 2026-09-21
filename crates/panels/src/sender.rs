//! Shared sender panel used by the Native and Web composition roots.
//!
//! The transport is deliberately represented by actions.  This keeps all
//! interaction and layout decisions in one place while allowing Native and
//! Web to submit the same application commands through their own runtimes.

use egui::widgets::text_edit::TextEditState;
use egui::{Id, Ui};
use egui_material_icons::icons::{ICON_DELETE, ICON_DELETE_SWEEP};

use crate::design::combo_slot;

/// 「清空全部」两步确认按钮在 egui 临时记忆里的 salt。
const CLEAR_SEND_HISTORY_CONFIRM_ID: &str = "clear_send_history_confirm";

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

    render_options(ui, view);

    let available = ui.available_size();
    // 操作行一律换行（见 render_actions），插件按钮多时会多占一行。预留高度取
    // 「上一帧量到的操作区高度」与常数中的较大值，这样宽度变小、按钮换行后不会
    // 把最后一行挤出面板；首帧用常数，最多差一帧。
    let height_id = actions_height_id(view.layout);
    let measured = ui.ctx().data_mut(|data| data.get_temp::<f32>(height_id));
    let reserved = match view.layout {
        SendLayout::Horizontal => 92.0_f32,
        SendLayout::Vertical => 150.0_f32,
    }
    .max(measured.unwrap_or(0.0));
    let min_input = 40.0_f32.min(available.y.max(0.0));
    let input_height = (available.y - reserved).max(min_input);
    let response = render_input(ui, view, input_height);

    let actions_top = ui.cursor().min.y;
    render_actions(ui, view, &mut actions);
    if let Some(error) = view.error.as_deref() {
        ui.colored_label(crate::theme::red(), error);
    }
    let actions_height = (ui.cursor().min.y - actions_top).max(0.0);
    ui.ctx()
        .data_mut(|data| data.insert_temp(height_id, actions_height));
    ui.take_available_space();

    if response.changed() {
        *view.periodic_send_count = 0;
        *view.error = None;
    }
    actions
}

/// 记忆操作区实测高度的键（两种布局各自独立，避免拖拽 Dock 时互相污染）。
fn actions_height_id(layout: SendLayout) -> Id {
    Id::new(match layout {
        SendLayout::Horizontal => "shared-send-actions-height-horizontal",
        SendLayout::Vertical => "shared-send-actions-height-vertical",
    })
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
        combo_slot(ui, 130.0, |ui| {
            egui::ComboBox::from_id_salt("shared-send-target-port")
                .width(130.0)
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    if view.ports.is_empty() {
                        ui.add_enabled(false, egui::Label::new("无已打开串口"));
                    } else {
                        for port in view.ports {
                            ui.selectable_value(
                                view.target_port,
                                Some(port.id.clone()),
                                &port.label,
                            );
                        }
                    }
                });
        });
        ui.separator();
        ui.selectable_value(view.hex_mode, false, "文本");
        ui.selectable_value(view.hex_mode, true, "HEX");
        if *view.hex_mode {
            ui.checkbox(view.hex_strict, "严格")
                .on_hover_text("严格模式：每个 HEX token 必须是完整的两位字节");
        }
        combo_slot(ui, 72.0, |ui| {
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
        });
    };

    // 选项行一律换行（原先 Horizontal 档走横向 ScrollArea）：控件总宽可以超过面板
    // —— Vertical 档的窄面板，或 HEX 档多出一个「严格」勾选框 —— 横向滚动会把溢出
    // 的控件藏到视野外，换行则始终可见，也不多出滚动条。
    ui.horizontal_wrapped(|ui| row(ui, view));
}

/// 读取文本框当前光标所在的字符下标；该文本框尚未布局过时返回 `None`。
fn cursor_index(ui: &Ui, id: Id) -> Option<usize> {
    ui.ctx().data_mut(|data| {
        data.get_persisted::<TextEditState>(id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index.into())
    })
}

fn render_input(ui: &mut Ui, view: &mut SendView<'_>, input_height: f32) -> egui::Response {
    let id = Id::new(match view.layout {
        SendLayout::Horizontal => "shared-send-input-horizontal",
        SendLayout::Vertical => "shared-send-input-vertical",
    });
    // 布局 TextEdit 之前取到的光标属于上一帧：本帧的上下键要按"按键前"的行位置判断。
    let before = cursor_index(ui, id).unwrap_or(0);
    let hint = if view.target_open {
        "输入要发送的文本或 HEX，Ctrl+Enter 发送"
    } else {
        "请选择已打开的串口"
    };
    let response = egui::ScrollArea::vertical()
        .id_salt(match view.layout {
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
        let after = cursor_index(ui, id).unwrap_or(before);
        let char_len = view.input.chars().count();
        let multiline = view.input.contains('\n');
        let first = !view.input.chars().take(before).any(|c| c == '\n');
        let last = !view.input.chars().skip(before).any(|c| c == '\n');
        let up = ui.input(|input| input.key_pressed(egui::Key::ArrowUp));
        let down = ui.input(|input| input.key_pressed(egui::Key::ArrowDown));
        // 光标停在首/末行时上下键才归历史所有，否则是行内移动。
        let at_top = !multiline || (first && after == 0);
        let at_bottom = !multiline || (last && after == char_len);
        if up && at_top {
            recall_older(view);
        } else if down && at_bottom {
            recall_newer(view);
        }
    } else if !response.has_focus() {
        *view.history_index = None;
        view.saved_input.clear();
    }

    response
}

/// 向上翻到更早的一条历史；首次翻历史时先把当前输入存进 `saved_input`。
fn recall_older(view: &mut SendView<'_>) {
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
}

/// 向下翻回更新的历史，翻过头则还原成进入历史前的那条输入。
fn recall_newer(view: &mut SendView<'_>) {
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

fn render_actions(ui: &mut Ui, view: &mut SendView<'_>, actions: &mut Vec<SendAction>) {
    let input = view.input.trim().to_owned();
    let hex_error = if *view.hex_mode && !input.is_empty() {
        hex_error(&input, *view.hex_strict)
    } else {
        None
    };
    let can_send = view.target_open && !input.is_empty() && hex_error.is_none();
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

            let enabled = view.target_open && target.is_some();
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

    // 主操作行与次操作行一律换行：插件贡献的按钮（G文件/G单条/暂停恢复/取消等）
    // 会把行撑过可用宽度，用横向 ScrollArea 会把它们裁到视野外，只能靠拖动滚动条
    // 才能点到；换行则始终可见。
    ui.horizontal_wrapped(|ui| render_main_row(ui, view, actions));
    ui.horizontal_wrapped(|ui| render_secondary_row(ui, view, actions));
}

/// 历史弹层：标题 + 条数、过滤框、定高行列表、两步「清空全部」。
///
/// 行是**手绘**的（`allocate_exact_size` + 截断 galley），不用 `ui.button`：按钮宽度按内容
/// 撑开、行高随字体浮动，长中文条目会把弹层拉成一格一格的空白，删除按钮还会越往下越靠右。
fn render_history(ui: &mut Ui, view: &mut SendView<'_>) {
    if view.history.is_empty() {
        return;
    }
    ui.menu_button("历史", |ui| history_popup(ui, view));
}

/// [`render_history`] 的弹层内容。单独成函数是为了让测试能直接渲染它：
/// `menu_button` 的展开态需要一个点击路径，而这段的回归风险恰恰在弹层里的排版。
fn history_popup(ui: &mut Ui, view: &mut SendView<'_>) {
    ui.set_min_width(320.0);
    ui.set_max_width(420.0);
    ui.spacing_mut().item_spacing.y = 4.0;

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("发送历史")
                .strong()
                .color(crate::theme::text_white()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{} 条", view.history.len()))
                    .small()
                    .color(crate::theme::text_dimmed()),
            );
        });
    });

    ui.add_space(2.0);
    let search_response = ui
        .add(
            egui::TextEdit::singleline(view.history_search)
                .hint_text("过滤历史")
                .desired_width(f32::INFINITY),
        )
        .on_hover_text(crate::search::SearchQuery::hover_hint(
            crate::search::SearchQuery::new(view.history_search, false)
                .used_invalid_regex_fallback(),
        ));
    if !search_response.has_focus() {
        search_response.request_focus();
    }
    ui.add_space(2.0);
    ui.separator();
    ui.add_space(2.0);

    // 过滤走共用 SearchQuery：与其它搜索入口一样认 `re:`。
    let query = crate::search::SearchQuery::new(view.history_search, false);
    let entries: Vec<String> = view
        .history
        .iter()
        .filter(|item| query.is_empty() || query.matches(item))
        .take(view.max_history)
        .cloned()
        .collect();

    if entries.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(12.0);
            ui.label(egui::RichText::new("无匹配项").color(crate::theme::text_dimmed()));
            ui.add_space(12.0);
        });
    } else {
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                // item_spacing.y = 0：分隔线要贴在下一行顶边，否则它会悬在两行的
                // 缝隙里，视觉上错位。列宽也在循环外取一次 —— 循环内 available_width
                // 会随 min_rect 增长而变大，删除按钮就会越往下越靠右。
                ui.spacing_mut().item_spacing.y = 0.0;
                let delete_width = 28.0;
                let column_width = ui.available_width();
                let text_width = (column_width - delete_width).max(60.0);
                let galley_width = (text_width - 16.0).max(20.0);
                let count = entries.len();
                // 条目用不小于 15px 的正文，历史里多为命令原文，太小难读。
                let body_size = ui
                    .style()
                    .text_styles
                    .get(&egui::TextStyle::Body)
                    .map_or(15.0, |body| body.size.max(15.0));
                let font_id = egui::FontId::proportional(body_size);

                for (index, item) in entries.iter().enumerate() {
                    // 按 \n 拆段，每段独立按列宽排版（Truncate 只丢弃溢出的字形，egui 不补
                    // 省略号），再垂直拼接。
                    let segments: Vec<std::sync::Arc<egui::Galley>> = item
                        .split('\n')
                        .map(|segment| {
                            egui::WidgetText::from(segment)
                                .color(crate::theme::text_primary())
                                .into_galley(
                                    ui,
                                    Some(egui::TextWrapMode::Truncate),
                                    galley_width,
                                    font_id.clone(),
                                )
                        })
                        .collect();
                    let text_height: f32 = segments.iter().map(|galley| galley.size().y).sum();
                    let row_height = 28.0_f32.max(text_height + 8.0);

                    let row = ui.allocate_ui_with_layout(
                        egui::vec2(column_width, row_height),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let (_, text_response) = ui.allocate_exact_size(
                                egui::vec2(text_width, row_height),
                                egui::Sense::click(),
                            );
                            let text_rect = text_response.rect;
                            if text_response.hovered() {
                                ui.painter()
                                    .rect_filled(text_rect, 3.0, crate::theme::bg_hover());
                            }
                            let mut y = text_rect.center().y - text_height / 2.0;
                            for galley in &segments {
                                ui.painter().galley(
                                    egui::pos2(text_rect.left() + 8.0, y),
                                    galley.clone(),
                                    crate::theme::text_primary(),
                                );
                                y += galley.size().y;
                            }
                            if text_response.clicked() {
                                *view.input = item.clone();
                                *view.history_index = None;
                                ui.close();
                            }
                            text_response
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text("点击填入发送框");

                            let (_, delete_response) = ui.allocate_exact_size(
                                egui::vec2(delete_width, row_height),
                                egui::Sense::click(),
                            );
                            let delete_rect = delete_response.rect;
                            if delete_response.hovered() {
                                ui.painter().rect_filled(
                                    delete_rect,
                                    3.0,
                                    crate::theme::bg_hover(),
                                );
                            }
                            ui.painter().text(
                                delete_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                ICON_DELETE.codepoint,
                                egui::FontId::new(17.0, ICON_DELETE.font_family()),
                                if delete_response.hovered() {
                                    crate::theme::text_primary()
                                } else {
                                    crate::theme::text_secondary()
                                },
                            );
                            if delete_response.clicked() {
                                let removed = item.clone();
                                view.history.retain(|item| *item != removed);
                            }
                            delete_response
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text("删除该条");
                        },
                    );

                    if index + 1 < count {
                        let rect = row.response.rect;
                        ui.painter().line_segment(
                            [
                                egui::pos2(rect.left() + 4.0, rect.bottom()),
                                egui::pos2(rect.right() - 4.0, rect.bottom()),
                            ],
                            egui::Stroke::new(1.0, crate::theme::border()),
                        );
                    }
                }
            });
    }

    ui.add_space(2.0);
    ui.separator();
    ui.add_space(2.0);
    // 两步确认：清空全部历史不可撤销，与终端/日志的「清空」同一份规则。
    let armed = crate::design::confirm_armed(ui, CLEAR_SEND_HISTORY_CONFIRM_ID);
    let label = if armed {
        "确认清空"
    } else {
        "清空全部"
    };
    let response = crate::design::button(
        ui,
        ICON_DELETE_SWEEP,
        label,
        crate::design::ButtonKind::Danger,
    );
    let clicked = response.clicked();
    if armed {
        response.on_hover_text("再次点击清空全部历史，3 秒内有效（Esc 或点击别处取消）");
    } else {
        response.on_hover_text("删除所有历史记录");
    }
    if crate::design::confirm_click(ui, CLEAR_SEND_HISTORY_CONFIRM_ID, clicked) {
        view.history.clear();
        view.history_search.clear();
        ui.close();
    }
}

/// HEX 输入的分词：按空白、`,`、`;` 切开，丢掉空 token。
fn hex_tokens(input: &str) -> impl Iterator<Item = &str> {
    input
        .trim()
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',' || ch == ';')
        .filter(|token| !token.is_empty())
}

/// 规范化单个 token：剥掉**一层** `0x`/`0X` 前缀，再去掉 `_`、`-` 分隔符。
///
/// 只剥一层是这里的实际规则，与 `tool_core::normalize_hex_token`（反复剥）不同；
/// 该差异由 `gate_and_decoder_disagree_on_repeated_0x_prefix` 钉住，别顺手改成多遍剥。
fn normalize_hex_token(token: &str) -> String {
    token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
        .unwrap_or(token)
        .replace(['_', '-'], "")
}

fn hex_error(input: &str, strict: bool) -> Option<String> {
    let mut tokens = hex_tokens(input).peekable();
    if tokens.peek().is_none() {
        return Some("HEX 输入为空".to_owned());
    }
    for token in tokens {
        let normalized = normalize_hex_token(token);
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
    None
}

fn hex_preview(input: &str) -> String {
    if let Some(error) = hex_error(input, false) {
        if error == "HEX 输入为空" {
            return String::new();
        }
        return format!("解析失败：{error}");
    }
    let mut bytes = Vec::new();
    for token in hex_tokens(input) {
        let mut normalized = normalize_hex_token(token);
        // 宽松档允许奇数个 hex 字符：左补一个 '0' 再两两成字节（"C" → 0x0C，不是 0xC0）。
        if !normalized.len().is_multiple_of(2) {
            normalized.insert(0, '0');
        }
        bytes.extend(normalized.as_bytes().chunks(2).map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).unwrap_or_default(), 16).unwrap_or(0)
        }));
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};
    use std::{cell::RefCell, rc::Rc};

    // ── 活的 HEX 门禁真值表 ───────────────────────────────────────────────
    //
    // `hex_error` 是**两平台发送按钮实际读到的那道门**：`render_actions` 里
    // `can_send = view.target_open && !input.is_empty() && hex_error.is_none()`，
    // native 经 `bottom_panel.rs` 的 `tool_panels::sender_ui` 走到它，web 经
    // `crates/app/src/web.rs` 的同一个组件走到它（见 docs/ARCHITECTURE.md 的
    // 「预检的规则来源」行）。本文件此前一条 `#[test]` 都没有：把严格档的
    // `normalized.len() != 2` 放松成 `> 2`，全工作区 585 条用例仍然全绿 ——
    // 也就是按钮可以在输入根本发不出去的情况下亮着，而 CI 看不见。
    //
    // 形状照抄 `crates/application/src/send_plan.rs` 的 `mod tests`：真值表就是
    // 判定本身，每格同时钉严格/宽松两档。

    /// 一行判定：`None` = 放行（按钮可点），`Some(片段)` = 拒绝且消息含该片段。
    /// 起名字是为了把 `clippy::type_complexity` 消除在定义处 —— 用 `type` 别名
    /// 而不是 `#[allow]`：放宽注解会把这条 lint 从整个测试模块上关掉。
    type Verdict = Option<&'static str>;

    const STRICT_LEN_RULE: &str = "严格 HEX 模式要求每个 token 是两位";
    const BAD_CHAR_RULE: &str = "HEX 中包含无效字符";
    const EMPTY_TOKEN_RULE: &str = "HEX 中包含空 token";
    const EMPTY_INPUT_RULE: &str = "HEX 输入为空";

    /// 每格 = (输入, 严格档判定, 宽松档判定)。
    const TABLE: &[(&str, Verdict, Verdict)] = &[
        // 规范写法：两档都收。
        ("AB CD", None, None),
        // 小写同样合法：门禁只看字符集与长度，不要求大写。
        ("ab cd", None, None),
        // 紧凑奇数长度：严格档按 normalize 后的长度判 3 → 拒；宽松档放行。
        ("abc", Some(STRICT_LEN_RULE), None),
        // 紧凑偶数长度：严格档仍按"每 token 恰 2 字符"判 → 拒；宽松档放行。
        ("abcd", Some(STRICT_LEN_RULE), None),
        // 单 nibble：严格档拒、宽松档放行（补齐成 0x0C 是发送侧的事）。
        ("AB C", Some(STRICT_LEN_RULE), None),
        // 4 个十六进制字符挤在一个 token 里：严格档拒。
        ("0xAABB", Some(STRICT_LEN_RULE), None),
        // 非 HEX 字符：两档都拒（长度先过，卡在字符集那一行）。
        ("ZZ", Some(BAD_CHAR_RULE), Some(BAD_CHAR_RULE)),
        // 分隔符 `,` / `;` 与空白等价：两档都收。
        ("0A,BB", None, None),
        ("0A;BB", None, None),
        // 单层 `0x` 前缀：剥掉后恰是两位 → 两档都收。
        ("0xAB", None, None),
        // 大写 `0X` 前缀同样只剥一层。
        ("0XAB", None, None),
        // `_` / `-` 被 replace 掉后是 "AABBCC"（长度 6）→ 严格拒、宽松放行。
        ("AA_BB-CC", Some(STRICT_LEN_RULE), None),
        // 空 token：`0x` 剥完什么都不剩，两档都拒，且不是"输入为空"那条消息。
        ("0x", Some(EMPTY_TOKEN_RULE), Some(EMPTY_TOKEN_RULE)),
        // 全空白 / 空串：没有任何 token 也算非法，否则按钮会亮而发送侧报"empty"。
        ("   ", Some(EMPTY_INPUT_RULE), Some(EMPTY_INPUT_RULE)),
        ("", Some(EMPTY_INPUT_RULE), Some(EMPTY_INPUT_RULE)),
    ];

    fn check_cell(input: &str, expect: Verdict, strict: bool) {
        let mode = if strict { "严格" } else { "宽松" };
        let actual = hex_error(input, strict);
        match expect {
            None => assert!(
                actual.is_none(),
                "{mode}档应放行 {input:?}，实际拒绝：{actual:?}",
            ),
            Some(fragment) => {
                let message =
                    actual.unwrap_or_else(|| panic!("{mode}档应拒绝 {input:?}，实际放行"));
                assert!(
                    message.contains(fragment),
                    "{mode}档拒绝 {input:?} 的消息应含 {fragment:?}，实际 {message:?}",
                );
            }
        }
    }

    #[test]
    fn gate_has_exactly_one_verdict_per_input_and_mode() {
        for (input, strict_expect, lenient_expect) in TABLE {
            check_cell(input, *strict_expect, true);
            check_cell(input, *lenient_expect, false);
        }
    }

    /// 严格档的**形状**规则：长度不等于 2 就拒 —— 包括比 2 长的情况。
    ///
    /// 单列一条而不是只靠表里的格子：把判定放松成 `len() > 2`（本波之前的变异探针
    /// 就是这么打穿 585 条用例的）时，"3 字符被拒"与"6 字符被拒"两格都会红，
    /// 这条给出第三个独立观测点，且直接点名被改坏的那个方向。
    #[test]
    fn strict_mode_rejects_every_length_except_two() {
        for len in [0usize, 1, 3, 4, 5, 6, 8] {
            if len == 2 {
                continue;
            }
            let token = "a".repeat(len);
            let actual = hex_error(&token, true);
            assert!(
                actual.is_some(),
                "严格档必须拒绝长度 {len} 的 token {token:?}，实际放行 —— \
                 `len() != 2` 被写成 `len() > 2` 就是这个形状",
            );
        }
        assert!(
            hex_error("ab", true).is_none(),
            "长度恰为 2 时必须放行，否则严格档把合法输入也堵死了",
        );
    }

    /// 本门禁与 `tool_core`（真正解码的那一份）**已知不一致**的那一格。
    ///
    /// 这是刻意的特征测试（characterization test），不是在为这个分歧背书：
    /// `hex_error` 只剥**一层** `0x`，`tool_core::normalize_hex_token` 用
    /// `trim_start_matches` 反复剥，于是 `"0x0xAB"` 在发送侧解得出 `AB`、
    /// 在门禁侧两档都被拒 —— 结果是**按钮灰着而 `dispatch` 其实接受这串**
    /// （`docs/ARCHITECTURE.md`「预检的规则来源」行记的就是它，且把它判为
    /// 后续项而非本轮改动：改判定会移动用户可见的按钮状态）。
    ///
    /// 把它钉在两边各自的实际行为上，是为了让"某天有人统一了它"以红测试的
    /// 形式出现，而不是以一行未经核实的文档结论出现。
    #[test]
    fn gate_and_decoder_disagree_on_repeated_0x_prefix() {
        let decoded = tool_core::parse_hex_strict("0x0xAB");
        assert_eq!(
            decoded.as_deref(),
            Ok([0xAB].as_slice()),
            "前提：发送侧（`tool_core`）接受 \"0x0xAB\"，反复剥 `0x` 后得到 AB，got {decoded:?}",
        );
        assert!(
            hex_error("0x0xAB", true).is_some(),
            "前提：门禁（本文件）在严格档拒绝 \"0x0xAB\" —— 只剥一层 `0x` 后长度是 4",
        );
        assert!(
            hex_error("0x0xAB", false).is_some(),
            "前提：门禁在宽松档同样拒绝 \"0x0xAB\"（残留的 `x` 不是 HEX 字符）",
        );
        // 对照格：单层前缀两边一致放行，说明分歧只在"重复前缀"这一类输入上。
        assert!(hex_error("0xAB", true).is_none(), "单层 `0x` 两档都应放行");
        assert_eq!(
            tool_core::parse_hex_strict("0xAB").as_deref(),
            Ok([0xAB].as_slice()),
            "对照格：`tool_core` 对单层前缀同样放行",
        );
    }

    // ── 操作区的渲染与布局（egui_kittest）─────────────────────────────────
    //
    // 下面的用例都跑真 `sender_ui`：操作行一律换行之后，按钮是否还在面板里、
    // 输入区是否压住操作行，只能靠实际布局量出来。

    /// 发送器的全部可变状态，测试里持有它并在每帧构造 [`SendView`]。
    struct SenderFixture {
        ports: Vec<SendPortItem>,
        target_port: Option<String>,
        input: String,
        hex_mode: bool,
        hex_strict: bool,
        line_ending: SendLineEnding,
        error: Option<String>,
        history: Vec<String>,
        history_search: String,
        history_index: Option<usize>,
        saved_input: String,
        periodic_enabled: bool,
        periodic_interval_ms: String,
        periodic_send_count: u64,
        dtr: bool,
        rts: bool,
        toolbar_buttons: Vec<SendToolbarButton>,
        layout: SendLayout,
    }

    impl SenderFixture {
        fn new(layout: SendLayout) -> Self {
            Self {
                ports: vec![SendPortItem {
                    id: "COM1".to_owned(),
                    label: "COM1".to_owned(),
                }],
                target_port: Some("COM1".to_owned()),
                input: "payload".to_owned(),
                hex_mode: false,
                hex_strict: false,
                line_ending: SendLineEnding::Lf,
                error: None,
                history: vec!["上一帧发送的内容".to_owned()],
                history_search: String::new(),
                history_index: None,
                saved_input: String::new(),
                periodic_enabled: false,
                periodic_interval_ms: "1000".to_owned(),
                periodic_send_count: 0,
                dtr: false,
                rts: false,
                toolbar_buttons: Vec::new(),
                layout,
            }
        }

        /// gcode-sender 这类插件会在操作行里追加按钮。
        fn with_plugin_buttons(mut self, titles: &[&str]) -> Self {
            self.toolbar_buttons = titles
                .iter()
                .map(|title| SendToolbarButton {
                    plugin_id: "test.plugin".to_owned(),
                    contribution_id: (*title).to_owned(),
                    title: (*title).to_owned(),
                    tooltip: None,
                    order: 0,
                    enabled: true,
                })
                .collect();
            self
        }

        fn view(&mut self) -> SendView<'_> {
            SendView {
                ports: &self.ports,
                target_port: &mut self.target_port,
                target_open: true,
                input: &mut self.input,
                hex_mode: &mut self.hex_mode,
                hex_strict: &mut self.hex_strict,
                line_ending: &mut self.line_ending,
                error: &mut self.error,
                history: &mut self.history,
                history_search: &mut self.history_search,
                history_index: &mut self.history_index,
                saved_input: &mut self.saved_input,
                periodic_enabled: &mut self.periodic_enabled,
                periodic_interval_ms: &mut self.periodic_interval_ms,
                periodic_send_count: &mut self.periodic_send_count,
                dtr: &mut self.dtr,
                rts: &mut self.rts,
                toolbar_buttons: &self.toolbar_buttons,
                max_history: 200,
                layout: self.layout,
            }
        }
    }

    /// 渲染一帧发送器：面板矩形写进 `panel_rect`，返回跑过这一帧的 harness。
    fn sender_harness<'a>(
        fixture: &'a Rc<RefCell<SenderFixture>>,
        panel_rect: &'a Rc<RefCell<Option<egui::Rect>>>,
        size: egui::Vec2,
    ) -> Harness<'a> {
        let mut harness = Harness::builder().with_size(size).build_ui(move |ui| {
            *panel_rect.borrow_mut() = Some(ui.max_rect());
            let mut state = fixture.borrow_mut();
            let _ = sender_ui(ui, &mut state.view());
        });
        harness.run();
        harness
    }

    /// 操作行换行后的共同底线：每个 label 对应的按钮都必须完整落在面板矩形里。
    fn assert_buttons_inside_panel(harness: &Harness<'_>, panel: egui::Rect, labels: &[&str]) {
        for label in labels {
            let rect = harness.get_by_label(label).rect();
            assert!(
                panel.contains_rect(rect),
                "「{label}」被裁出面板：按钮 {rect:?} 不在 {panel:?} 内"
            );
        }
    }

    #[test]
    fn wrapped_action_row_keeps_every_plugin_button_visible() {
        let fixture = Rc::new(RefCell::new(
            SenderFixture::new(SendLayout::Horizontal).with_plugin_buttons(&[
                "G文件",
                "G单条",
                "暂停/恢复",
                "取消",
                "回零",
                "预热",
                "校准",
                "导出 G",
            ]),
        ));
        let panel_rect = Rc::new(RefCell::new(None));
        // 430px 宽（仍在 ≥420px 的 Horizontal 区间）：内置按钮 + 8 个插件按钮
        // 一行放不下，必须换行而不是横向裁切。
        let harness = sender_harness(&fixture, &panel_rect, egui::vec2(430.0, 360.0));
        let panel = panel_rect.borrow().expect("panel rect");

        assert_buttons_inside_panel(
            &harness,
            panel,
            &[
                "发送",
                "清空",
                "历史",
                "G文件",
                "G单条",
                "暂停/恢复",
                "取消",
                "回零",
                "预热",
                "校准",
                "导出 G",
            ],
        );
    }

    /// 窄面板（Vertical 布局）同样不能裁掉任何按钮。
    #[test]
    fn narrow_panel_keeps_every_plugin_button_visible() {
        let fixture = Rc::new(RefCell::new(
            SenderFixture::new(SendLayout::Vertical).with_plugin_buttons(&[
                "G文件",
                "G单条",
                "暂停/恢复",
                "取消",
            ]),
        ));
        let panel_rect = Rc::new(RefCell::new(None));
        let harness = sender_harness(&fixture, &panel_rect, egui::vec2(300.0, 360.0));
        let panel = panel_rect.borrow().expect("panel rect");

        assert_buttons_inside_panel(
            &harness,
            panel,
            &[
                "发送",
                "清空",
                "历史",
                "G文件",
                "G单条",
                "暂停/恢复",
                "取消",
            ],
        );
    }

    /// 操作行换行后可能多占一行；预留高度必须跟着实测高度走，否则最后一行会被
    /// 输入区挤出面板底部。
    #[test]
    fn wrapped_action_rows_stay_above_the_panel_bottom() {
        let fixture = Rc::new(RefCell::new(
            SenderFixture::new(SendLayout::Horizontal).with_plugin_buttons(&[
                "G文件",
                "G单条",
                "暂停/恢复",
                "取消",
            ]),
        ));
        let panel_rect = Rc::new(RefCell::new(None));
        // 高度收紧到刚好够：换行前 92px 的旧预留会不够用。
        let harness = sender_harness(&fixture, &panel_rect, egui::vec2(470.0, 250.0));
        let panel = panel_rect.borrow().expect("panel rect");

        assert_buttons_inside_panel(
            &harness,
            panel,
            &[
                "发送",
                "G文件",
                "暂停/恢复",
                "取消",
                "周期发送",
                "DTR",
                "RTS",
            ],
        );
    }

    /// 操作行不再使用横向滚动：同一行在宽面板下也不会出现滚动条容器。
    #[test]
    fn action_row_has_no_horizontal_scroll_container() {
        let fixture = Rc::new(RefCell::new(SenderFixture::new(SendLayout::Horizontal)));
        let panel_rect = Rc::new(RefCell::new(None));
        let harness = sender_harness(&fixture, &panel_rect, egui::vec2(640.0, 320.0));

        // 发送按钮左边与面板左边对齐：若外层是横向 ScrollArea，会有内容内边距。
        let panel = panel_rect.borrow().expect("panel rect");
        let send = harness.get_by_label("发送").rect();
        assert!(
            (send.min.x - panel.min.x).abs() < 12.0,
            "操作行左边缘 {} 与面板左边缘 {} 偏差过大，可能仍在滚动容器内",
            send.min.x,
            panel.min.x
        );
    }

    /// 输入区不能压到操作行之上：单帧渲染后，输入框底边必须仍在「发送」那一行上面
    /// （沿用原 `send_layout_input_does_not_overlap_actions` 的语义，改为针对真实实现断言）。
    #[test]
    fn send_input_stays_above_the_action_row() {
        let fixture = Rc::new(RefCell::new(SenderFixture::new(SendLayout::Horizontal)));
        let panel_rect = Rc::new(RefCell::new(None));
        let harness = sender_harness(&fixture, &panel_rect, egui::vec2(640.0, 360.0));

        let send = harness.get_by_label("发送").rect();
        let input = harness
            .query_by_role(egui::accesskit::Role::MultilineTextInput)
            .expect("发送输入框应当存在");
        assert!(
            input.rect().max.y <= send.min.y + 0.5,
            "输入区底部 {} 超过操作行顶部 {}，发生重叠",
            input.rect().max.y,
            send.min.y
        );
    }

    /// 渲染一次历史弹层，并把它的占用矩形写回 `popup_rect`。
    fn history_popup_harness<'a>(
        fixture: &'a Rc<RefCell<SenderFixture>>,
        popup_rect: &'a Rc<RefCell<egui::Rect>>,
    ) -> Harness<'a> {
        let popup_rect = popup_rect.clone();
        // 与 `terminal_harness` 同一手法：kittest 在构建时就先跑一帧，而删除图标用的
        // 具名族 "material-icons" 必须在那之前注册，所以第一帧只注册字体、不渲染。
        let registered = std::cell::Cell::new(false);
        Harness::builder()
            .with_size(egui::vec2(900.0, 600.0))
            .build_ui(move |ui| {
                if !registered.get() {
                    registered.set(true);
                    egui_material_icons::initialize(ui.ctx());
                    return;
                }
                let mut state = fixture.borrow_mut();
                let mut view = state.view();
                history_popup(ui, &mut view);
                *popup_rect.borrow_mut() = ui.min_rect();
            })
    }

    fn history_popup_rect(history: Vec<String>) -> egui::Rect {
        let fixture = Rc::new(RefCell::new(SenderFixture::new(SendLayout::Horizontal)));
        fixture.borrow_mut().history = history;
        let popup_rect = Rc::new(RefCell::new(egui::Rect::NOTHING));
        let mut harness = history_popup_harness(&fixture, &popup_rect);
        harness.run();
        let rect = *popup_rect.borrow();
        assert!(
            rect.is_finite() && !rect.is_negative(),
            "弹层没有布局：{rect:?}"
        );
        rect
    }

    /// 历史弹层的排版契约：**宽度是一个上限**（不随条目长度或窗口宽度增长），且有标题与条数。
    ///
    /// 上一版给每条历史画一个 `ui.button`，弹层只有 `set_min_width(320)`、没有上限，
    /// 一条 200 字的中文历史就能把它拉到窗口那么宽、行与行之间全是空白格（用户截图那样）。
    /// 变异核对：删掉 `set_max_width(420)` 后本用例变红（弹层从 420 涨到 harness 的 900）。
    ///
    /// **本用例证明不了逐段截断**：行是手绘的，galley 宽度只决定"画出多少字"，不参与
    /// min_rect —— 把 `galley_width` 换成 `f32::INFINITY` 实测仍然绿。那一条要靠肉眼或
    /// 快照验，别把它记成"有测试守着"。
    #[test]
    fn history_popup_keeps_a_fixed_width_and_a_header() {
        let long = history_popup_rect(vec!["实".repeat(200), "AT".to_owned(), "12321".to_owned()]);
        let short = history_popup_rect(vec!["AT".to_owned(), "12321".to_owned(), "x".to_owned()]);
        assert!(
            (long.width() - short.width()).abs() <= 0.5,
            "弹层宽度随条目长度变了：长 {long:?} vs 短 {short:?} —— 宽度应当由上限决定"
        );
        assert!(
            long.width() <= 440.0,
            "弹层没有宽度上限，被条目/窗口撑开了：{long:?}"
        );

        let fixture = Rc::new(RefCell::new(SenderFixture::new(SendLayout::Horizontal)));
        fixture.borrow_mut().history = vec!["AT".to_owned(), "12321".to_owned()];
        let popup_rect = Rc::new(RefCell::new(egui::Rect::NOTHING));
        let mut harness = history_popup_harness(&fixture, &popup_rect);
        harness.run();
        // 旧实现只有一列按钮，没有任何标题；标题与条数是这版排版的组成部分。
        let header = harness.get_by_label("发送历史");
        assert!(header.rect().height() > 0.0, "「发送历史」标题未渲染");
        let count = harness.get_by_label("2 条");
        assert!(count.rect().height() > 0.0, "「N 条」计数未渲染");
    }
}
