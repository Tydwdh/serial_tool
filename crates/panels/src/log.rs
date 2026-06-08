use crate::theme;
use egui::{Color32, RichText, ScrollArea};
use std::collections::VecDeque;
use tool_core::{Event, LogLevel};
use tool_databus::{DataBus, Subscription, TopicFilter};

const MAX_INGEST_PER_FRAME: usize = 500;

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
    timestamp_ms: u64,
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
            subscription: bus.subscribe(TopicFilter::prefix("log.")),
            entries: VecDeque::new(),
            min_level: LogLevel::Info,
            auto_scroll: true,
            max_entries: 5_000,
            last_scroll_offset_y: 0.0,
        }
    }
    pub fn ingest_all_pending(&mut self) -> usize {
        let mut count = 0;

        while let Some(event) = self.subscription.try_recv() {
            self.push_event(event);
            count += 1;
        }

        count
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.last_scroll_offset_y = 0.0;
    }

    /// 让 main.rs 在日志面板不可见时也能消费日志事件。
    /// 这样回放、拖进度条、seek 时日志状态会和接收区一致。
    pub fn ingest_pending(&mut self) -> usize {
        self.ingest()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let _new_entries = self.ingest();

        let mut force_scroll_to_bottom = false;

        ui.horizontal_wrapped(|ui| {
            ui.label("级别");

            for level in [
                LogLevel::Trace,
                LogLevel::Debug,
                LogLevel::Info,
                LogLevel::Warn,
                LogLevel::Error,
            ] {
                ui.selectable_value(&mut self.min_level, level, level.as_str());
            }

            if self.auto_scroll {
                if ui.button("⏸").on_hover_text("暂停自动滚动").clicked() {
                    self.auto_scroll = false;
                }
            } else if ui.button("↓").on_hover_text("滚动到底部").clicked() {
                self.auto_scroll = true;
                force_scroll_to_bottom = true;
            }

            if ui.button("清空").clicked() {
                self.clear();
            }

            ui.label(
                RichText::new(format!("{} 条", self.entries.len())).color(theme::TEXT_SECONDARY),
            );
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

        self.entries.push_back(LogEntry {
            timestamp_ms: event.timestamp_ms,
            timestamp_label: format!("[{}]", fmt_ts(event.timestamp_ms)),
            level,
            source,
            message: event.payload.text_lossy(),
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
    let row_height = log_row_height(ui);

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

    let scroll_output = ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(stick_to_bottom)
        .id_salt("log-scroll")
        .show_rows(ui, row_height, rows.len(), |ui, row_range| {
            for row_index in row_range {
                let entry = rows[row_index];

                let response = show_log_entry(ui, entry, row_height);

                response.context_menu(|ui| {
                    if ui.button("复制消息").clicked() {
                        ui.ctx().copy_text(entry.message.clone());
                        ui.close();
                    }

                    if ui.button("复制整行").clicked() {
                        ui.ctx().copy_text(format!(
                            "{} {} {} {}",
                            entry.timestamp_label,
                            entry.level.as_str(),
                            entry.source,
                            entry.message
                        ));
                        ui.close();
                    }
                });
            }
        });

    LogRenderOutcome {
        inner_rect: scroll_output.inner_rect,
        content_height: scroll_output.content_size.y,
        offset_y: scroll_output.state.offset.y,
    }
}

fn show_log_entry(ui: &mut egui::Ui, entry: &LogEntry, row_height: f32) -> egui::Response {
    let row_width = ui.available_width();

    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(row_width, row_height), egui::Sense::click());

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
    let text_y = rect.center().y;

    let mut x = rect.left() + ROW_LEFT_PADDING;

    painter.text(
        egui::pos2(x, text_y),
        egui::Align2::LEFT_CENTER,
        &entry.timestamp_label,
        font_id.clone(),
        theme::TEXT_SECONDARY,
    );

    x += TIME_COL_WIDTH + COL_GAP;

    painter.text(
        egui::pos2(x, text_y),
        egui::Align2::LEFT_CENTER,
        entry.level.as_str(),
        font_id.clone(),
        level_color(entry.level),
    );

    x += LEVEL_COL_WIDTH + COL_GAP;

    let source_clip = egui::Rect::from_min_max(
        egui::pos2(x, rect.top()),
        egui::pos2((x + SOURCE_COL_WIDTH).min(rect.right()), rect.bottom()),
    );

    let source_painter = ui.painter().with_clip_rect(source_clip);
    let source_text = compact_middle(&entry.source, SOURCE_TEXT_MAX_CHARS);

    source_painter.text(
        egui::pos2(x, text_y),
        egui::Align2::LEFT_CENTER,
        source_text,
        font_id.clone(),
        theme::CYAN,
    );

    x += SOURCE_COL_WIDTH + COL_GAP;

    let message_clip = egui::Rect::from_min_max(
        egui::pos2(x, rect.top()),
        egui::pos2(rect.right(), rect.bottom()),
    );

    let message_painter = ui.painter().with_clip_rect(message_clip);

    message_painter.text(
        egui::pos2(x, text_y),
        egui::Align2::LEFT_CENTER,
        &entry.message,
        font_id,
        theme::TEXT_PRIMARY,
    );

    response
}

fn log_row_height(ui: &egui::Ui) -> f32 {
    (ui.text_style_height(&egui::TextStyle::Monospace).ceil() + 6.0).max(20.0)
}

fn fmt_ts(ms: u64) -> String {
    let Some(dt_utc) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms as i64) else {
        return "--:--:--.---".to_owned();
    };

    dt_utc
        .with_timezone(&chrono::Local)
        .format("%H:%M:%S%.3f")
        .to_string()
}

fn level_color(level: LogLevel) -> Color32 {
    match level {
        LogLevel::Trace => Color32::GRAY,
        LogLevel::Debug => theme::BLUE,
        LogLevel::Info => theme::GREEN,
        LogLevel::Warn => theme::YELLOW,
        LogLevel::Error => theme::RED,
    }
}

fn compact_middle(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();

    if char_count <= max_chars {
        return text.to_owned();
    }

    if max_chars <= 3 {
        return "...".to_owned();
    }

    let left_count = (max_chars - 3) / 2;
    let right_count = max_chars - 3 - left_count;

    let left = text.chars().take(left_count).collect::<String>();
    let right = text
        .chars()
        .rev()
        .take(right_count)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();

    format!("{left}...{right}")
}
