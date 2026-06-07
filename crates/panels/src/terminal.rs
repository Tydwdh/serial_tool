use crate::theme;
use egui::{Color32, RichText, ScrollArea};
use std::collections::{BTreeMap, VecDeque};
use tool_core::{Direction, Event, Payload, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};

pub struct TerminalPanel {
    subscription: Subscription,
    ports: BTreeMap<String, PortData>,
    show_rx: bool,
    show_tx: bool,
    show_hex: bool,
    auto_scroll: bool,
    max_entries: usize,
    pub max_height: f32,
    pub maximize_clicked: bool,
}

struct PortData {
    entries: VecDeque<TerminalEntry>,
    show_rx: bool,
    show_tx: bool,
}

struct TerminalEntry {
    timestamp_ms: u64,
    direction: Direction,
    text: String,
    bytes: Vec<u8>,
}

impl TerminalPanel {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            subscription: bus.subscribe(TopicFilter::prefix("transport.serial.")),
            ports: BTreeMap::new(),
            show_rx: true,
            show_tx: true,
            show_hex: false,
            auto_scroll: true,
            max_entries: 2_000,
            max_height: f32::INFINITY,
            maximize_clicked: false,
        }
    }

    pub fn clear(&mut self) {
        self.ports.clear();
    }

    pub fn port_names(&self) -> Vec<String> {
        self.ports.keys().cloned().collect()
    }

    pub fn port_ui(&mut self, ui: &mut egui::Ui, port_name: &str) {
        let new_entries = self.ingest();
        let mut show_hex = self.show_hex;
        let mut auto_scroll = self.auto_scroll;
        let mut maximize_clicked = false;
        let scroll_to_bottom = new_entries > 0 && auto_scroll;

        let (inner_rect, content_height, offset_y) = {
            let data = self.ports.entry(port_name.to_owned()).or_default();

            ui.horizontal(|ui| {
                ui.label(RichText::new(port_name).monospace().strong());
                ui.checkbox(&mut data.show_rx, "RX");
                ui.checkbox(&mut data.show_tx, "TX");
                ui.checkbox(&mut show_hex, "HEX");
                auto_scroll_button(ui, &mut auto_scroll);
                if ui.button("清空").clicked() {
                    data.entries.clear();
                }
                if ui.button("⛶").on_hover_text("放大查看").clicked() {
                    maximize_clicked = true;
                }
            });
            ui.separator();

            let scroll_output =
                ScrollArea::vertical()
                    .max_height(self.max_height)
                    .stick_to_bottom(auto_scroll)
                    .id_salt(format!("terminal-port-{port_name}"))
                    .show(ui, |ui| {
                        let mut visible_count = 0;
                        for entry in data.entries.iter().filter(|entry| {
                            entry_visible(entry.direction, data.show_rx, data.show_tx)
                        }) {
                            visible_count += 1;
                            show_entry(ui, Some(port_name), entry, show_hex);
                        }

                        if visible_count == 0 {
                            ui.label(RichText::new("暂无串口数据").color(theme::TEXT_SECONDARY));
                        }
                        if scroll_to_bottom {
                            ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                        }
                    });

            (
                scroll_output.inner_rect,
                scroll_output.content_size.y,
                scroll_output.state.offset.y,
            )
        };

        self.show_hex = show_hex;
        self.auto_scroll = auto_scroll;
        self.maximize_clicked |= maximize_clicked;
        self.update_auto_scroll(ui, inner_rect, content_height, offset_y);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let new_entries = self.ingest();

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_rx, "RX");
            ui.checkbox(&mut self.show_tx, "TX");
            ui.checkbox(&mut self.show_hex, "HEX");
            auto_scroll_button(ui, &mut self.auto_scroll);
            if ui.button("清空").clicked() {
                self.ports.clear();
            }
            if ui.button("⛶").on_hover_text("放大查看").clicked() {
                self.maximize_clicked = true;
            }
            let total = self
                .ports
                .values()
                .map(|port| port.entries.len())
                .sum::<usize>();
            ui.label(RichText::new(format!("{total} 条")).color(theme::TEXT_SECONDARY));
        });
        ui.separator();

        let scroll_to_bottom = new_entries > 0 && self.auto_scroll;
        let scroll_output =
            ScrollArea::vertical()
                .max_height(self.max_height)
                .stick_to_bottom(self.auto_scroll)
                .id_salt("terminal-all")
                .show(ui, |ui| {
                    let mut visible_count = 0;
                    for (port, data) in &self.ports {
                        for entry in data.entries.iter().filter(|entry| {
                            entry_visible(entry.direction, self.show_rx, self.show_tx)
                        }) {
                            visible_count += 1;
                            show_entry(ui, Some(port), entry, self.show_hex);
                        }
                    }

                    if visible_count == 0 {
                        ui.label(RichText::new("暂无串口数据").color(theme::TEXT_SECONDARY));
                    }
                    if scroll_to_bottom {
                        ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    }
                });

        self.update_auto_scroll(
            ui,
            scroll_output.inner_rect,
            scroll_output.content_size.y,
            scroll_output.state.offset.y,
        );
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

        if pointer_inside && ui.input(|input| input.smooth_scroll_delta.y > 0.0) {
            self.auto_scroll = false;
        }

        let at_bottom = offset_y >= content_height - inner_rect.height() - 4.0;
        if !self.auto_scroll && at_bottom {
            self.auto_scroll = true;
        }
    }

    fn ingest(&mut self) -> usize {
        let mut count = 0;
        for event in self.subscription.drain() {
            if !matches!(event.topic.as_str(), topics::SERIAL_RX | topics::SERIAL_TX) {
                continue;
            }
            self.push_event(event);
            count += 1;
        }
        count
    }

    fn push_event(&mut self, event: Event) {
        let port = event
            .metadata
            .get("port")
            .and_then(|value| value.as_str())
            .or_else(|| event.source.strip_prefix("serial:"))
            .unwrap_or("default")
            .to_owned();

        let bytes = match &event.payload {
            Payload::Bytes(bytes) => bytes.clone(),
            _ => event.payload.text_lossy().into_bytes(),
        };
        let text = event.payload.text_lossy();
        let data = self.ports.entry(port).or_default();
        data.entries.push_back(TerminalEntry {
            timestamp_ms: event.timestamp_ms,
            direction: event.direction,
            text,
            bytes,
        });

        while data.entries.len() > self.max_entries {
            data.entries.pop_front();
        }
    }
}

impl Default for PortData {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            show_rx: true,
            show_tx: true,
        }
    }
}

fn show_entry(ui: &mut egui::Ui, port: Option<&str>, entry: &TerminalEntry, show_hex: bool) {
    ui.horizontal_wrapped(|ui| {
        let (label, color) = direction_label(entry.direction);
        ui.label(RichText::new(format!("[{}]", fmt_ts(entry.timestamp_ms))).monospace());
        if let Some(port) = port {
            ui.label(RichText::new(port).monospace().color(theme::YELLOW));
        }
        ui.label(RichText::new(label).strong().color(color));
        if show_hex {
            ui.label(RichText::new(format_hex(&entry.bytes)).monospace());
            if !entry.bytes.is_empty() {
                ui.label(
                    RichText::new(format!("[{}]", format_ascii(&entry.bytes)))
                        .monospace()
                        .color(theme::TEXT_DIMMED),
                );
            }
        } else {
            ui.label(RichText::new(format_terminal_text(&entry.text)).monospace());
        }
    });
}

fn entry_visible(direction: Direction, show_rx: bool, show_tx: bool) -> bool {
    match direction {
        Direction::Rx => show_rx,
        Direction::Tx => show_tx,
        Direction::Internal => false,
    }
}

fn auto_scroll_button(ui: &mut egui::Ui, auto_scroll: &mut bool) {
    if *auto_scroll {
        if ui.button("⏸").on_hover_text("暂停自动滚动").clicked() {
            *auto_scroll = false;
        }
    } else if ui.button("↓").on_hover_text("滚动到底部").clicked() {
        *auto_scroll = true;
    }
}

fn format_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| {
            if byte.is_ascii_graphic() || *byte == b' ' {
                char::from(*byte)
            } else {
                '.'
            }
        })
        .collect()
}

fn format_terminal_text(text: &str) -> String {
    let mut output = String::new();
    for ch in text.chars() {
        match ch {
            '\r' => output.push_str("\\r"),
            '\n' => output.push_str("\\n"),
            '\t' => output.push_str("\\t"),
            ch if ch.is_control() => output.push('·'),
            ch => output.push(ch),
        }
    }
    output
}

fn fmt_ts(ms: u64) -> String {
    let secs = (ms / 1000) % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3_600,
        (secs % 3_600) / 60,
        secs % 60
    )
}

fn direction_label(direction: Direction) -> (&'static str, Color32) {
    match direction {
        Direction::Rx => ("RX", theme::GREEN),
        Direction::Tx => ("TX", theme::BLUE),
        Direction::Internal => ("IN", Color32::GRAY),
    }
}
