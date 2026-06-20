use crate::{fmt_ts, theme, MAX_INGEST_PER_FRAME};
use egui::{Color32, RichText, ScrollArea, UiBuilder};
use std::collections::VecDeque;
use tool_core::{Event, LogLevel};
use tool_databus::{DataBus, Subscription, TopicFilter};

const TIME_COL_WIDTH: f32 = 118.0;
const LEVEL_COL_WIDTH: f32 = 52.0;
const SOURCE_COL_WIDTH: f32 = 190.0;
const SOURCE_TEXT_MAX_CHARS: usize = 26;
const ROW_LEFT_PADDING: f32 = 4.0;
const COL_GAP: f32 = 6.0;

pub struct LogPanel {
    subscription: Subscription,
    entries: VecDeque<LogEntry>,
    min_level: LogLevel,
    auto_scroll: bool,
    max_entries: usize,
    last_scroll_offset_y: f32,
}

struct LogEntry {
    timestamp_label: String,
    level: LogLevel,
    source: String,
    message: String,
    /// 消息的行数（预计算，避免渲染时重复 count）
    line_count: usize,
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
            min_level: LogLevel::Info,
            auto_scroll: true,
            max_entries: 5_000,
            last_scroll_offset_y: 0.0,
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
        self.entries.clear();
        self.last_scroll_offset_y = 0.0;
        // 清空后重置为自动滚动，确保新日志可见
        self.auto_scroll = true;
    }

    /// 让 main.rs 在日志面板不可见时也能消费日志事件。
    /// 这样回放、拖进度条、seek 时日志状态会和接收区一致。
    pub fn ingest_pending(&mut self) -> usize {
        self.ingest()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let _new_entries = self.ingest();

        let mut force_scroll_to_bottom = false;

        ui.horizontal(|ui| {
            // 预计算标签所需宽度：按钮 padding + 最宽文字，避免 hover 框撑大时抖动
            let padding = ui.spacing().button_padding.x * 2.0;
            let char_w = 10.0; // 近似等宽字符宽度
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

            force_scroll_to_bottom |= crate::theme::auto_scroll_button(ui, &mut self.auto_scroll);

            let dropped = self.subscription.dropped_count();
            if dropped > 0 {
                ui.colored_label(theme::YELLOW, format!("已丢弃 {dropped} 条"));
            }

            if ui.button("清空").clicked() {
                self.clear();
            }
        });

        ui.separator();

        let rows: Vec<&LogEntry> = self
            .entries
            .iter()
            .filter(|entry| entry.level >= self.min_level)
            .collect();

        let outcome = render_log_rows(ui, &rows, self.auto_scroll || force_scroll_to_bottom);

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

        // 回放事件会被 mark_replay_event() 改成 replay:<source>。
        // 这里优先显示 original_source，让回放日志看起来和原始日志一致。
        let source = event
            .metadata
            .get("original_source")
            .and_then(|value| value.as_str())
            .unwrap_or(&event.source)
            .to_owned();

        let message = event.payload.text_lossy();

        self.entries.push_back(LogEntry {
            timestamp_label: format!("[{}]", fmt_ts(event.timestamp_ms)),
            level,
            source,
            line_count: message.lines().count().max(1),
            message,
        });

        while self.entries.len() > self.max_entries {
            self.entries.pop_front();
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

        let scrolling_away_from_bottom = pointer_inside && smooth_scroll_y > 0.0;

        let moving_towards_bottom = offset_y > self.last_scroll_offset_y + 0.5;

        let bottom_offset = (content_height - inner_rect.height()).max(0.0);
        let at_bottom = offset_y >= bottom_offset - 4.0;

        if scrolling_away_from_bottom {
            self.auto_scroll = false;
        }

        if !self.auto_scroll && at_bottom && pointer_inside && moving_towards_bottom {
            self.auto_scroll = true;
        }

        self.last_scroll_offset_y = offset_y;
    }
}

fn render_log_rows(
    ui: &mut egui::Ui,
    rows: &[&LogEntry],
    stick_to_bottom: bool,
) -> LogRenderOutcome {
    let base_row_height = log_row_height(ui);

    if rows.is_empty() {
        let scroll_output = ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("log-scroll")
            .show(ui, |ui| {
                ui.label(RichText::new("暂无日志").color(theme::TEXT_SECONDARY));
            });

        return LogRenderOutcome {
            inner_rect: scroll_output.inner_rect,
            content_height: scroll_output.content_size.y,
            offset_y: scroll_output.state.offset.y,
        };
    }

    // 计算总内容高度
    let total_content_height: f32 = rows
        .iter()
        .map(|entry| base_row_height * entry.line_count.max(1) as f32)
        .sum();

    let scroll_output = ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(stick_to_bottom)
        .id_salt("log-scroll")
        .show(ui, |ui| {
            let clip_rect = ui.clip_rect();
            let mut y_cursor = 0.0_f32;

            for entry in rows {
                let entry_height = base_row_height * entry.line_count.max(1) as f32;
                let entry_rect = egui::Rect::from_min_size(
                    egui::pos2(ui.min_rect().left(), ui.min_rect().top() + y_cursor),
                    egui::vec2(ui.available_width(), entry_height),
                );

                // 视口裁剪
                if entry_rect.bottom() >= clip_rect.top() - entry_height
                    && entry_rect.top() <= clip_rect.bottom() + entry_height
                {
                    let mut child_ui = ui.new_child(
                        UiBuilder::new()
                            .max_rect(entry_rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );

                    let response = show_log_entry(&mut child_ui, entry, base_row_height);

                    response.context_menu(|ctx_ui| {
                        if ctx_ui.button("复制消息").clicked() {
                            ctx_ui.ctx().copy_text(entry.message.clone());
                            ctx_ui.close();
                        }

                        if ctx_ui.button("复制整行").clicked() {
                            ctx_ui.ctx().copy_text(format!(
                                "{} {} {} {}",
                                entry.timestamp_label,
                                entry.level.as_str(),
                                entry.source,
                                entry.message
                            ));
                            ctx_ui.close();
                        }
                    });
                }

                y_cursor += entry_height;
            }

            ui.allocate_space(egui::vec2(ui.available_width(), total_content_height));
        });

    LogRenderOutcome {
        inner_rect: scroll_output.inner_rect,
        content_height: scroll_output.content_size.y,
        offset_y: scroll_output.state.offset.y,
    }
}

fn show_log_entry(ui: &mut egui::Ui, entry: &LogEntry, base_row_height: f32) -> egui::Response {
    let row_width = ui.available_width();
    let entry_height = base_row_height * entry.line_count.max(1) as f32;

    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(row_width, entry_height), egui::Sense::click());

    let bg = if response.hovered() {
        theme::WIDGET_HOVER
    } else {
        Color32::TRANSPARENT
    };

    let painter = ui.painter_at(rect);

    if bg != Color32::TRANSPARENT {
        painter.rect_filled(rect, 2.0, bg);
    }

    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let first_line_y = rect.top() + base_row_height * 0.5;

    let mut x = rect.left() + ROW_LEFT_PADDING;

    // 时间戳 — 第一行居中
    painter.text(
        egui::pos2(x, first_line_y),
        egui::Align2::LEFT_CENTER,
        &entry.timestamp_label,
        font_id.clone(),
        theme::TEXT_SECONDARY,
    );

    x += TIME_COL_WIDTH + COL_GAP;

    // 级别 — 第一行居中
    painter.text(
        egui::pos2(x, first_line_y),
        egui::Align2::LEFT_CENTER,
        entry.level.as_str(),
        font_id.clone(),
        crate::level_color(entry.level),
    );

    x += LEVEL_COL_WIDTH + COL_GAP;

    // 来源 — 第一行居中
    let source_clip = egui::Rect::from_min_max(
        egui::pos2(x, rect.top()),
        egui::pos2((x + SOURCE_COL_WIDTH).min(rect.right()), rect.bottom()),
    );

    let source_painter = ui.painter().with_clip_rect(source_clip);
    let source_text = crate::compact_middle(&entry.source, SOURCE_TEXT_MAX_CHARS);

    source_painter.text(
        egui::pos2(x, first_line_y),
        egui::Align2::LEFT_CENTER,
        source_text,
        font_id.clone(),
        theme::CYAN,
    );

    x += SOURCE_COL_WIDTH + COL_GAP;

    // 消息 — 逐行渲染
    let message_clip = egui::Rect::from_min_max(
        egui::pos2(x, rect.top()),
        egui::pos2(rect.right(), rect.bottom()),
    );

    let message_painter = ui.painter().with_clip_rect(message_clip);

    for (line_idx, line) in entry.message.lines().enumerate() {
        let line_y = rect.top() + base_row_height * (line_idx as f32 + 0.5);
        message_painter.text(
            egui::pos2(x, line_y),
            egui::Align2::LEFT_CENTER,
            line,
            font_id.clone(),
            theme::TEXT_PRIMARY,
        );
    }

    response
}

fn log_row_height(ui: &egui::Ui) -> f32 {
    crate::row_height(ui)
}

// fmt_ts 已提取到 crate::fmt_ts
// level_color 和 compact_middle 已提取到 crate 根模块
