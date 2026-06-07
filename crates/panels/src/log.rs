use crate::theme;
use egui::{Color32, RichText, ScrollArea};
use std::collections::VecDeque;
use tool_core::{Event, LogLevel};
use tool_databus::{DataBus, Subscription, TopicFilter};

pub struct LogPanel {
    subscription: Subscription,
    entries: VecDeque<LogEntry>,
    min_level: LogLevel,
    auto_scroll: bool,
    max_entries: usize,
}

struct LogEntry {
    timestamp_ms: u64,
    level: LogLevel,
    source: String,
    message: String,
}

impl LogPanel {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            subscription: bus.subscribe(TopicFilter::prefix("log.")),
            entries: VecDeque::new(),
            min_level: LogLevel::Info,
            auto_scroll: true,
            max_entries: 2_000,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let new_entries = self.ingest();

        ui.horizontal(|ui| {
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
            } else {
                if ui.button("↓").on_hover_text("滚动到底部").clicked() {
                    self.auto_scroll = true;
                }
            }
            if ui.button("清空").clicked() {
                self.entries.clear();
            }
        });

        let scroll_to_bottom = new_entries > 0 && self.auto_scroll;
        let scroll_output = ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("log-scroll")
            .show(ui, |ui| {
                for entry in self
                    .entries
                    .iter()
                    .filter(|entry| entry.level >= self.min_level)
                {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("[{}]", fmt_ts(entry.timestamp_ms))).monospace(),
                        );
                        ui.label(
                            RichText::new(entry.level.as_str())
                                .strong()
                                .color(level_color(entry.level)),
                        );
                        ui.label(RichText::new(&entry.source).monospace());
                        ui.label(&entry.message);
                    });
                }
                if scroll_to_bottom {
                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                }
            });

        // 用户手动向上滚 → 暂停自动滚动
        if ui.input(|i| i.smooth_scroll_delta.y > 0.0)
            && ui
                .input(|i| i.pointer.hover_pos())
                .is_some_and(|pos| scroll_output.inner_rect.contains(pos))
        {
            self.auto_scroll = false;
        }
        // 滚到底部 → 自动重新开启
        let at_bottom = scroll_output.state.offset.y
            >= scroll_output.content_size.y - scroll_output.inner_rect.height() - 4.0;
        if !self.auto_scroll && at_bottom {
            self.auto_scroll = true;
        }
    }

    fn ingest(&mut self) -> usize {
        let mut count = 0;
        for event in self.subscription.drain() {
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

        self.entries.push_back(LogEntry {
            timestamp_ms: event.timestamp_ms,
            level,
            source: event.source,
            message: event.payload.text_lossy(),
        });

        while self.entries.len() > self.max_entries {
            self.entries.pop_front();
        }
    }
}

fn fmt_ts(ms: u64) -> String {
    let secs = (ms / 1000) % 86400;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
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
