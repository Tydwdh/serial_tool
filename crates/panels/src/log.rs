use crate::{
    MAX_INGEST_PER_FRAME, fmt_ts,
    table::{RowHighlight, RowSelection, edge_scroll_delta},
    theme,
};
use egui::text_selection::LabelSelectionState;
use egui::{RichText, ScrollArea, Sense, Stroke, TextEdit};
use std::collections::{BTreeSet, VecDeque};
use tool_core::{Event, LogLevel};
use tool_databus::{DataBus, Subscription, TopicFilter};

const TIME_COL_WIDTH: f32 = 118.0;
const LEVEL_COL_WIDTH: f32 = 52.0;
const SOURCE_COL_WIDTH: f32 = 140.0;
const SOURCE_TEXT_MAX_CHARS: usize = 18;
const ROW_LEFT_PADDING: f32 = 4.0;
const COL_GAP: f32 = 6.0;
/// 标签列到消息列之间的间距
const LABEL_TO_MSG_GAP: f32 = 3.0;
const LOG_SCROLL_ID: &str = "log-scroll-v2";
/// 日志面板最大保留条数（与终端面板一致）。
const MAX_LOG_ENTRIES: usize = 50_000;

pub struct LogPanel {
    subscription: Subscription,
    entries: VecDeque<LogEntry>,
    next_entry_id: u64,
    min_level: LogLevel,
    auto_scroll: bool,
    pub max_entries: usize,
    last_scroll_offset_y: f32,
    pending_scroll_to_bottom: bool,
    /// 搜索文本（默认大小写不敏感，同时匹配 source 和 message）。
    search_text: String,
    /// 搜索是否大小写敏感。
    search_case_sensitive: bool,
    /// 来源过滤：None 表示显示全部，Some 表示只显示指定 source。
    source_filter: Option<String>,
    /// 用户可调的字体大小（10-24px），默认 13.0
    pub font_size: f32,
    /// 行框选状态
    pub selection: RowSelection,
    /// 是否发生过截断（用于状态栏提示，显示后清除）
    pub truncated: bool,
    /// 待推送到状态栏的 warn/error 通知（每帧由 app 层 take 后推给 NotificationQueue）
    pub pending_notifications: VecDeque<(LogLevel, String)>,
}

struct LogEntry {
    id: u64,
    timestamp_label: String,
    level: LogLevel,
    source: String,
    message: String,
}

struct LogRenderOutcome {
    inner_rect: egui::Rect,
    content_height: f32,
    offset_y: f32,
}

impl LogPanel {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            subscription: bus.subscribe_lossy_bounded(TopicFilter::prefix("log."), 4096),
            entries: VecDeque::new(),
            next_entry_id: 1,
            min_level: LogLevel::Info,
            auto_scroll: true,
            max_entries: MAX_LOG_ENTRIES,
            last_scroll_offset_y: 0.0,
            pending_scroll_to_bottom: false,
            search_text: String::new(),
            search_case_sensitive: false,
            source_filter: None,
            font_size: 13.0,
            selection: RowSelection::new(0),
            truncated: false,
            pending_notifications: VecDeque::new(),
        }
    }
    pub fn ingest_all_pending(&mut self) -> usize {
        // 每帧最多摄入 2000 条，防止大量日志突发时 UI 卡顿
        const MAX_INGEST_ALL: usize = 2000;
        let mut count = 0;

        while let Some(event) = self.subscription.try_recv() {
            self.push_event(event);
            count += 1;
            if count >= MAX_INGEST_ALL {
                break;
            }
        }

        count
    }
    pub fn clear(&mut self) {
        while self.subscription.try_recv().is_some() {}
        self.entries.clear();
        self.last_scroll_offset_y = 0.0;
        self.auto_scroll = true;
        self.pending_scroll_to_bottom = false;
        self.search_text.clear();
        self.source_filter = None;
        self.selection.clear();
    }

    /// 收集所有已出现过的 source 名称，用于过滤下拉框。
    fn source_names(&self) -> Vec<String> {
        let mut names: BTreeSet<&str> = BTreeSet::new();
        for entry in &self.entries {
            names.insert(&entry.source);
        }
        names.into_iter().map(|s| s.to_owned()).collect()
    }

    /// 让 main.rs 在日志面板不可见时也能消费日志事件。
    pub fn ingest_pending(&mut self) -> usize {
        self.ingest()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let _new_entries = self.ingest();

        // 仅当指针位于本面板内时，滚轮向下才触发强制滚到底；
        // 否则全局 smooth_scroll_delta 会误捕获其它区域的滚轮事件。
        let panel_rect = ui.max_rect();
        let pointer_inside = ui
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|pos| panel_rect.contains(pos));
        let wheel_moves_towards_bottom = pointer_inside
            && crate::scroll_delta_moves_towards_bottom(
                ui.input(|input| input.smooth_scroll_delta.y),
            );
        let mut force_scroll_to_bottom = self.pending_scroll_to_bottom;
        self.pending_scroll_to_bottom = false;

        // ── 第一行：级别过滤 + 自动滚动 + 清空 ──
        ui.horizontal(|ui| {
            let padding = ui.spacing().button_padding.x * 2.0;
            let char_w = 10.0;
            let btn_w = padding + 5.0 * char_w + 4.0;

            for level in [
                LogLevel::Trace,
                LogLevel::Debug,
                LogLevel::Info,
                LogLevel::Warn,
                LogLevel::Error,
            ] {
                ui.allocate_ui_with_layout(
                    egui::vec2(btn_w, ui.available_height()),
                    egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| {
                        ui.set_min_size(egui::vec2(btn_w, ui.available_height()));
                        if ui
                            .selectable_label(self.min_level == level, level.as_str())
                            .clicked()
                        {
                            self.min_level = level;
                        }
                    },
                );
            }

            ui.separator();

            force_scroll_to_bottom |= crate::theme::auto_scroll_button(ui, &mut self.auto_scroll);

            // 清空：两步确认（与终端面板一致），避免误触丢失系统日志。
            let clear_id = ui.id().with("log_clear_armed_ts");
            let now = ui.input(|i| i.time);
            let armed_ts: Option<f64> = ui.ctx().memory(|m| m.data.get_temp(clear_id));
            let armed = armed_ts.is_some_and(|t| now - t < 3.0);
            let clear_label = if armed { "确认清空?" } else { "清空" };
            let clear_btn = egui::Button::new(egui::RichText::new(clear_label).color(if armed {
                crate::theme::RED
            } else {
                crate::theme::TEXT_PRIMARY
            }));
            if ui.add(clear_btn).clicked() {
                if armed {
                    self.clear();
                    ui.ctx().memory_mut(|m| m.data.remove_temp::<f64>(clear_id));
                } else {
                    ui.ctx().memory_mut(|m| m.data.insert_temp(clear_id, now));
                }
            }
            if armed && ui.small_button("取消").clicked() {
                ui.ctx().memory_mut(|m| m.data.remove_temp::<f64>(clear_id));
            }
        });

        // ── 第二行：搜索 + 来源过滤 ──
        ui.horizontal(|ui| {
            ui.label("搜索");
            let search_resp = ui.add(
                TextEdit::singleline(&mut self.search_text)
                    .desired_width(120.0)
                    .hint_text("关键词"),
            );
            if search_resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.search_text.clear();
                search_resp.surrender_focus();
            }
            let case_btn = egui::Button::new("Aa")
                .selected(self.search_case_sensitive)
                .small();
            if ui.add(case_btn).on_hover_text("区分大小写").clicked() {
                self.search_case_sensitive = !self.search_case_sensitive;
            }

            ui.label("来源");
            egui::ComboBox::from_id_salt("log-source-filter")
                .width(100.0)
                .selected_text(self.source_filter.as_deref().unwrap_or("全部"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.source_filter, None, "全部");
                    for name in self.source_names() {
                        ui.selectable_value(&mut self.source_filter, Some(name.clone()), &name);
                    }
                });

            if (!self.search_text.is_empty() || self.source_filter.is_some())
                && ui.small_button("清除筛选").clicked()
            {
                self.search_text.clear();
                self.source_filter = None;
            }
        });

        force_scroll_to_bottom |= self.auto_scroll && wheel_moves_towards_bottom;

        ui.separator();

        // ── 构建可见行列表 ──
        let search_key = if self.search_case_sensitive {
            self.search_text.trim().to_owned()
        } else {
            self.search_text.trim().to_ascii_lowercase()
        };
        let rows: Vec<&LogEntry> = self
            .entries
            .iter()
            .filter(|entry| entry.level >= self.min_level)
            .filter(|entry| {
                if let Some(ref filter) = self.source_filter {
                    entry.source == *filter
                } else {
                    true
                }
            })
            .filter(|entry| {
                if search_key.is_empty() {
                    return true;
                }
                let matches = |haystack: &str| -> bool {
                    if self.search_case_sensitive {
                        haystack.contains(&search_key)
                    } else {
                        haystack.to_ascii_lowercase().contains(&search_key)
                    }
                };
                matches(&entry.source) || matches(&entry.message)
            })
            .collect();

        let outcome = render_log_rows(
            ui,
            &rows,
            !self.entries.is_empty(),
            self.auto_scroll,
            force_scroll_to_bottom,
            self.font_size,
            &mut self.selection,
        );

        self.update_auto_scroll(
            ui,
            outcome.inner_rect,
            outcome.content_height,
            outcome.offset_y,
        );
    }

    fn ingest(&mut self) -> usize {
        let mut count = 0;

        for _ in 0..MAX_INGEST_PER_FRAME {
            let Some(event) = self.subscription.try_recv() else {
                break;
            };

            self.push_event(event);
            count += 1;
        }

        count
    }

    fn push_event(&mut self, event: Event) {
        let level = event
            .metadata
            .get("level")
            .and_then(|value| value.as_str())
            .and_then(|value| value.parse().ok())
            .unwrap_or(LogLevel::Info);

        let source = event
            .metadata
            .get("original_source")
            .and_then(|value| value.as_str())
            .unwrap_or(&event.source)
            .to_owned();

        let message = event.payload.text_lossy();

        let entry_id = self.next_entry_id;
        self.next_entry_id = self.next_entry_id.wrapping_add(1).max(1);

        self.entries.push_back(LogEntry {
            id: entry_id,
            timestamp_label: format!("[{}]", fmt_ts(event.timestamp_ms)),
            level,
            source,
            message,
        });

        while self.entries.len() > self.max_entries {
            self.entries.pop_front();
            self.truncated = true;
        }
    }

    fn update_auto_scroll(
        &mut self,
        ui: &egui::Ui,
        inner_rect: egui::Rect,
        content_height: f32,
        offset_y: f32,
    ) {
        let pointer_inside = ui
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|pos| inner_rect.contains(pos));

        let smooth_scroll_y = ui.input(|input| input.smooth_scroll_delta.y);
        let next_auto_scroll = crate::next_auto_scroll_state(
            self.auto_scroll,
            pointer_inside,
            smooth_scroll_y,
            self.last_scroll_offset_y,
            offset_y,
            content_height,
            inner_rect.height(),
        );
        let should_repair_stick_to_bottom = next_auto_scroll
            && !crate::scroll_delta_moves_away_from_bottom(smooth_scroll_y)
            && !crate::scroll_is_at_bottom(offset_y, content_height, inner_rect.height());

        if self.auto_scroll != next_auto_scroll {
            if !self.auto_scroll && next_auto_scroll {
                self.pending_scroll_to_bottom = true;
            }

            self.auto_scroll = next_auto_scroll;
            ui.ctx().request_repaint();
        }

        if should_repair_stick_to_bottom {
            self.pending_scroll_to_bottom = true;
            ui.ctx().request_repaint();
        }

        self.last_scroll_offset_y = offset_y;
    }
}

// ── 渲染 ──

/// 预计算的单行布局（复用终端面板的 LayoutJob 模式）。
struct RowLayout {
    /// 消息列的 galley（支持文本选择）。
    message_galley: std::sync::Arc<egui::Galley>,
    /// 该行高度（至少 base_row_height）。
    height: f32,
}

fn render_log_rows(
    ui: &mut egui::Ui,
    rows: &[&LogEntry],
    has_any_entries: bool,
    stick_to_bottom: bool,
    force_scroll_to_bottom: bool,
    font_size: f32,
    selection: &mut RowSelection,
) -> LogRenderOutcome {
    let font_id = egui::FontId::new(font_size, egui::FontFamily::Monospace);
    let base_row_height = ui.fonts_mut(|f| f.row_height(&font_id));
    selection.sync_rows(rows.iter().map(|entry| entry.id));

    // 列宽随字体大小缩放（基准 13px）
    let scale = font_size / 13.0;
    let time_col_width = TIME_COL_WIDTH * scale;
    let level_col_width = LEVEL_COL_WIDTH * scale;
    let source_col_width = SOURCE_COL_WIDTH * scale;
    let col_gap = COL_GAP * scale;
    let row_left_padding = ROW_LEFT_PADDING * scale;
    let label_to_msg_gap = LABEL_TO_MSG_GAP * scale;

    if rows.is_empty() {
        let scroll_output = ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt(LOG_SCROLL_ID)
            .show(ui, |ui| {
                let hint = if has_any_entries {
                    "无匹配日志 · 试着清除搜索或来源过滤"
                } else {
                    "应用日志会显示在这里"
                };
                ui.label(RichText::new(hint).color(theme::TEXT_SECONDARY));
            });

        return LogRenderOutcome {
            inner_rect: scroll_output.inner_rect,
            content_height: scroll_output.content_size.y,
            offset_y: scroll_output.state.offset.y,
        };
    }

    let scroll_output = ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(stick_to_bottom)
        .id_salt(LOG_SCROLL_ID)
        .show(ui, |ui| {
            let full_width = ui.available_width();
            let font_id = egui::FontId::new(font_size, egui::FontFamily::Monospace);
            let text_color = ui.style().visuals.text_color();

            // 标签列总宽度
            let label_width = row_left_padding
                + time_col_width
                + col_gap
                + level_col_width
                + col_gap
                + source_col_width
                + label_to_msg_gap;
            let message_width = (full_width - label_width).max(40.0);
            let text_padding = 4.0;
            let galley_width = (message_width - text_padding).max(0.0);

            // 预计算所有行的 LayoutJob
            let row_layouts: Vec<RowLayout> = rows
                .iter()
                .map(|entry| {
                    let mut layout_job = egui::text::LayoutJob::simple(
                        entry.message.clone(),
                        font_id.clone(),
                        text_color,
                        galley_width,
                    );
                    layout_job.halign = egui::Align::LEFT;
                    let galley = ui.fonts_mut(|f| f.layout_job(layout_job));
                    let height = galley.size().y.max(base_row_height);
                    RowLayout {
                        message_galley: galley,
                        height,
                    }
                })
                .collect();

            let total_height: f32 = row_layouts.iter().map(|r| r.height).sum();

            // 分配总区域
            let (full_rect, _alloc_response) =
                ui.allocate_exact_size(egui::vec2(full_width, total_height), Sense::hover());

            // 标签区域
            let label_rect = egui::Rect::from_min_size(
                full_rect.left_top(),
                egui::vec2(label_width, total_height),
            );
            let label_painter = ui.painter_at(label_rect);

            // 消息区域
            let message_rect = egui::Rect::from_min_size(
                egui::pos2(full_rect.left() + label_width, full_rect.top()),
                egui::vec2(message_width, total_height),
            );
            let viewport_rect = ui.clip_rect();
            let blank_rect = (full_rect.bottom() < viewport_rect.bottom()).then(|| {
                egui::Rect::from_min_max(
                    egui::pos2(full_rect.left(), full_rect.bottom()),
                    egui::pos2(full_rect.right(), viewport_rect.bottom()),
                )
            });
            let blank_response = blank_rect.map(|rect| {
                ui.interact(
                    rect,
                    ui.make_persistent_id(("log-blank", LOG_SCROLL_ID)),
                    Sense::click(),
                )
            });

            // 逐行绘制标签 + 可选择的文本
            let mut hl = RowHighlight::new(ui, LOG_SCROLL_ID);

            // 先记录所有行范围，当前帧的点击/拖拽才能立即命中正确行。
            let mut recorded_y = label_rect.top();
            for layout in &row_layouts {
                hl.record_row(recorded_y, layout.height);
                recorded_y += layout.height;
            }

            // 整行选择只从元数据区起手；消息区完整保留给字符级文本选择。
            let mut ctx_response = ui.interact(
                label_rect,
                ui.make_persistent_id(("log-metadata", LOG_SCROLL_ID)),
                Sense::click_and_drag(),
            );
            if let Some(response) = blank_response {
                ctx_response |= response;
            }
            let hovered_idx = ui
                .input(|input| input.pointer.hover_pos().map(|pos| pos.y))
                .and_then(|y| hl.row_index_at_y_clamped(y));
            let message_pressed = ui.input(|input| {
                input.pointer.button_pressed(egui::PointerButton::Primary)
                    && input
                        .pointer
                        .hover_pos()
                        .is_some_and(|pos| message_rect.contains(pos))
            }) && ui.rect_contains_pointer(message_rect);
            let blank_pressed = blank_rect.is_some_and(|rect| {
                ui.input(|input| {
                    input.pointer.button_pressed(egui::PointerButton::Primary)
                        && input
                            .pointer
                            .hover_pos()
                            .is_some_and(|pos| rect.contains(pos))
                }) && ui.rect_contains_pointer(rect)
            });
            if message_pressed || blank_pressed {
                selection.clear();
            }
            let mut scroll_delta: f32 = 0.0;
            let row_selection_started = selection.handle_input(
                ui,
                label_rect,
                ui.clip_rect().intersect(label_rect),
                hovered_idx,
                &mut scroll_delta,
            );
            if row_selection_started || blank_pressed || selection.is_dragging() {
                ui.ctx()
                    .plugin::<LabelSelectionState>()
                    .lock()
                    .clear_selection();
            }

            let mut current_y = label_rect.top();
            let mut text_drag_response: Option<egui::Response> = None;
            for (row_idx, (entry, layout)) in rows.iter().zip(row_layouts.iter()).enumerate() {
                let entry_height = layout.height;
                // 标签对齐第一行中心（和终端面板一致）
                let label_y = current_y + base_row_height * 0.5;

                // 高亮悬停行（框选模式下跳过）
                if !selection.has_selection() {
                    hl.paint_background(ui, full_rect, current_y, entry_height);
                }

                // 框选高亮
                if selection.is_selected(row_idx) {
                    selection.paint(ui, full_rect, current_y, entry_height);
                }

                // --- 标签列 ---
                let mut x = label_rect.left() + row_left_padding;

                // 时间戳
                label_painter.text(
                    egui::pos2(x, label_y),
                    egui::Align2::LEFT_CENTER,
                    &entry.timestamp_label,
                    font_id.clone(),
                    theme::TEXT_SECONDARY,
                );
                x += time_col_width + col_gap;

                // 级别
                label_painter.text(
                    egui::pos2(x, label_y),
                    egui::Align2::LEFT_CENTER,
                    entry.level.as_str(),
                    font_id.clone(),
                    crate::level_color(entry.level),
                );
                x += level_col_width + col_gap;

                // 来源（裁剪）
                let source_clip = egui::Rect::from_min_max(
                    egui::pos2(x, current_y),
                    egui::pos2(
                        (x + source_col_width).min(label_rect.right()),
                        current_y + entry_height,
                    ),
                );
                let source_painter = label_painter.with_clip_rect(source_clip);
                let source_text = crate::compact_middle(&entry.source, SOURCE_TEXT_MAX_CHARS);
                source_painter.text(
                    egui::pos2(x, label_y),
                    egui::Align2::LEFT_CENTER,
                    source_text,
                    font_id.clone(),
                    theme::CYAN,
                );

                // --- 可选择的消息文本 ---
                {
                    // galley 从行顶开始绘制（和终端面板一致）
                    let galley_pos = egui::pos2(message_rect.left() + text_padding, current_y);
                    let row_text_rect = egui::Rect::from_min_size(
                        egui::pos2(message_rect.left(), current_y),
                        egui::vec2(message_width, entry_height),
                    );
                    let row_id = ui.make_persistent_id(("log-msg", entry.id));
                    let response = ui.interact(row_text_rect, row_id, Sense::click_and_drag());
                    text_drag_response = Some(match text_drag_response.take() {
                        Some(accumulated) => accumulated | response.clone(),
                        None => response.clone(),
                    });
                    ctx_response |= response.clone();

                    if selection.is_dragging() {
                        ui.painter().add(egui::epaint::TextShape::new(
                            galley_pos,
                            layout.message_galley.clone(),
                            text_color,
                        ));
                    } else {
                        LabelSelectionState::label_text_selection(
                            ui,
                            &response,
                            galley_pos,
                            layout.message_galley.clone(),
                            text_color,
                            Stroke::NONE,
                        );
                    }
                }

                current_y += entry_height;
            }

            if text_drag_response
                .as_ref()
                .is_some_and(|response| response.dragged_by(egui::PointerButton::Primary))
                && let Some(pointer_y) =
                    ui.input(|input| input.pointer.hover_pos().map(|pos| pos.y))
            {
                scroll_delta += edge_scroll_delta(pointer_y, viewport_rect.intersect(message_rect));
            }

            // 边缘滚动
            if scroll_delta != 0.0 {
                ui.scroll_with_delta(egui::vec2(0.0, scroll_delta));
                ui.ctx().request_repaint();
            }

            let frozen_row_idx = hl.resolve_click(
                ui,
                &ctx_response,
                ui.make_persistent_id(("log-frozen-row", LOG_SCROLL_ID)),
            );
            let hovered_row = if ctx_response.context_menu_opened()
                || ctx_response.clicked_by(egui::PointerButton::Secondary)
            {
                frozen_row_idx
            } else {
                hl.hover_index(ui)
            }
            .and_then(|idx| {
                rows.get(idx).map(|entry| {
                    let line = format!(
                        "{} {} {} {}",
                        entry.timestamp_label,
                        entry.level.as_str(),
                        entry.source,
                        entry.message
                    );
                    (line, entry.message.clone())
                })
            });

            // 框选范围文本（移入 context_menu 闭包内按需构造，避免菜单未打开时每帧构造）
            let selected_indices: Vec<usize> = selection.selected_indices().collect();

            // Ctrl+C 复制选中行：有选中、收到 Event::Copy、且无 TextEdit 聚焦时触发。
            // 复制 full（含时间戳/级别/来源前缀），与右键菜单"复制选中行"一致。
            // egui 0.35 把 Ctrl+C 转成 Event::Copy 事件，用 text_edit_focused 判断 TextEdit 聚焦。
            let copy_requested =
                ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
            if !selected_indices.is_empty()
                && copy_requested
                && !ui.ctx().text_edit_focused()
                && let (Some(full), _) = build_selected_text_log(rows, &selected_indices)
            {
                ui.ctx().copy_text(full);
            }

            ctx_response.context_menu(move |ctx_ui| {
                let (selected_full, selected_data) =
                    build_selected_text_log(rows, &selected_indices);

                // 统一菜单：有框选用选中文本，否则用单行文本
                let copy_full = selected_full
                    .clone()
                    .or_else(|| hovered_row.as_ref().map(|(f, _)| f.clone()));
                let copy_data = selected_data
                    .clone()
                    .or_else(|| hovered_row.as_ref().map(|(_, d)| d.clone()));

                if let Some(ref text) = copy_full
                    && ctx_ui.button("复制选中行").clicked()
                {
                    ctx_ui.ctx().copy_text(text.clone());
                    ctx_ui.close();
                }
                if let Some(ref text) = copy_data
                    && ctx_ui.button("复制选中行消息").clicked()
                {
                    ctx_ui.ctx().copy_text(text.clone());
                    ctx_ui.close();
                }
                if copy_full.is_some() || copy_data.is_some() {
                    ctx_ui.separator();
                }

                if ctx_ui.button("复制全部可见内容").clicked() {
                    // 按需构造，避免菜单未打开时每帧 join 全部行。
                    let combined_text: String = rows
                        .iter()
                        .map(|entry| {
                            format!(
                                "{} {} {} {}",
                                entry.timestamp_label,
                                entry.level.as_str(),
                                entry.source,
                                entry.message
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    ctx_ui.ctx().copy_text(combined_text);
                    ctx_ui.close();
                }

                if ctx_ui.button("复制 CSV").clicked() {
                    let mut csv = String::from("time,level,source,message\n");
                    for entry in rows {
                        csv.push_str(&csv_cell(&entry.timestamp_label));
                        csv.push(',');
                        csv.push_str(&csv_cell(entry.level.as_str()));
                        csv.push(',');
                        csv.push_str(&csv_cell(&entry.source));
                        csv.push(',');
                        csv.push_str(&csv_cell(&entry.message.replace('\n', " ")));
                        csv.push('\n');
                    }
                    ctx_ui.ctx().copy_text(csv);
                    ctx_ui.close();
                }

                if ctx_ui.button("复制 JSONL").clicked() {
                    let mut jsonl = String::new();
                    for entry in rows {
                        let obj = serde_json::json!({
                            "time": entry.timestamp_label,
                            "level": entry.level.as_str(),
                            "source": entry.source,
                            "message": entry.message,
                        });
                        if let Ok(line) = serde_json::to_string(&obj) {
                            jsonl.push_str(&line);
                            jsonl.push('\n');
                        }
                    }
                    ctx_ui.ctx().copy_text(jsonl);
                    ctx_ui.close();
                }
            });

            if force_scroll_to_bottom {
                let (rect, _sense) =
                    ui.allocate_exact_size(egui::vec2(0.0, 0.0), egui::Sense::hover());
                ui.scroll_to_rect(rect, Some(egui::Align::BOTTOM));
            }
        });

    LogRenderOutcome {
        inner_rect: scroll_output.inner_rect,
        content_height: scroll_output.content_size.y,
        offset_y: scroll_output.state.offset.y,
    }
}

/// 构造选中日志行的文本：full（含时间戳/级别/来源前缀）和 data（仅 message）。
/// 供右键菜单和 Ctrl+C 复用。
fn build_selected_text_log(
    rows: &[&LogEntry],
    selected_indices: &[usize],
) -> (Option<String>, Option<String>) {
    if selected_indices.is_empty() {
        return (None, None);
    }
    let full: String = selected_indices
        .iter()
        .map(|&index| rows[index])
        .map(|entry| {
            format!(
                "{} {} {} {}",
                entry.timestamp_label,
                entry.level.as_str(),
                entry.source,
                entry.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let data: String = selected_indices
        .iter()
        .map(|&index| rows[index])
        .map(|entry| entry.message.clone())
        .collect::<Vec<_>>()
        .join("\n");
    (Some(full), Some(data))
}

fn csv_cell(s: &str) -> String {
    let escaped = s.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_databus::DataBus;

    #[test]
    fn ingest_system_log_keeps_app_ready_entry() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);

        bus.publish(Event::system_log(LogLevel::Info, "app", "就绪"));

        assert_eq!(panel.ingest_all_pending(), 1);
        assert_eq!(panel.entries.len(), 1);

        let entry = panel.entries.front().expect("log entry should be ingested");
        assert_eq!(entry.level, LogLevel::Info);
        assert_eq!(entry.source, "app");
        assert_eq!(entry.message, "就绪");
    }

    #[test]
    fn clear_drains_pending_log_events() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);

        bus.publish(Event::system_log(LogLevel::Info, "app", "stale"));
        panel.clear();

        assert_eq!(panel.ingest_all_pending(), 0);
        assert!(panel.entries.is_empty());
    }

    #[test]
    fn clear_resets_search_and_filter() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);

        panel.search_text = "error".into();
        panel.source_filter = Some("app".into());
        panel.clear();

        assert!(panel.search_text.is_empty());
        assert!(panel.source_filter.is_none());
        assert!(panel.entries.is_empty());
    }

    #[test]
    fn search_filters_by_source_and_message() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);

        panel.entries.push_back(LogEntry {
            id: 1,
            timestamp_label: "[12:00:00.000]".into(),
            level: LogLevel::Error,
            source: "transport.serial".into(),
            message: "read failed on COM3: timeout".into(),
        });
        panel.entries.push_back(LogEntry {
            id: 2,
            timestamp_label: "[12:00:01.000]".into(),
            level: LogLevel::Info,
            source: "app".into(),
            message: "就绪".into(),
        });

        panel.search_text = "com3".into();
        let rows: Vec<&LogEntry> = panel
            .entries
            .iter()
            .filter(|e| {
                e.source.to_ascii_lowercase().contains("com3")
                    || e.message.to_ascii_lowercase().contains("com3")
            })
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "transport.serial");

        panel.search_text = "app".into();
        let rows: Vec<&LogEntry> = panel
            .entries
            .iter()
            .filter(|e| {
                e.source.to_ascii_lowercase().contains("app")
                    || e.message.to_ascii_lowercase().contains("app")
            })
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message, "就绪");

        panel.search_text.clear();
        let rows: Vec<&LogEntry> = panel.entries.iter().filter(|_| true).collect();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn source_filter_excludes_others() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);

        panel.entries.push_back(LogEntry {
            id: 1,
            timestamp_label: "[12:00:00.000]".into(),
            level: LogLevel::Warn,
            source: "ext".into(),
            message: "plugin time out".into(),
        });
        panel.entries.push_back(LogEntry {
            id: 2,
            timestamp_label: "[12:00:01.000]".into(),
            level: LogLevel::Info,
            source: "app".into(),
            message: "就绪".into(),
        });

        panel.source_filter = Some("app".into());
        let rows: Vec<&LogEntry> = panel.entries.iter().filter(|e| e.source == "app").collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "app");

        panel.source_filter = None;
        let rows: Vec<&LogEntry> = panel.entries.iter().collect();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn max_entries_truncates_oldest() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);
        panel.max_entries = 3;

        for i in 0..5 {
            bus.publish(Event::system_log(
                LogLevel::Info,
                "test",
                format!("msg {i}"),
            ));
        }
        panel.ingest_all_pending();
        assert_eq!(panel.entries.len(), 3);
        assert_eq!(panel.entries[0].message, "msg 2");
        assert_eq!(panel.entries[2].message, "msg 4");
    }
}
