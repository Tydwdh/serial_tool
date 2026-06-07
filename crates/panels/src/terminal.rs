use crate::theme;
use egui::{Color32, RichText, ScrollArea};
use std::collections::{BTreeMap, VecDeque};
use tool_core::{Direction, Payload, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};

pub struct TerminalPanel {
    subscription: Subscription,
    ports: BTreeMap<String, PortData>,
    show_hex: bool,
    auto_scroll: bool,
    max_entries: usize,
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
            show_hex: false,
            auto_scroll: true,
            max_entries: 2_000,
            maximize_clicked: false,
        }
    }

    pub fn clear(&mut self) {
        self.ports.clear();
    }

    /// 所有有数据的端口名列表
    pub fn port_names(&self) -> Vec<String> {
        self.ports.keys().cloned().collect()
    }

    /// 渲染指定端口的数据视图
    pub fn port_ui(&mut self, ui: &mut egui::Ui, port_name: &str) {
        let new_entries = self.ingest();
        let data = self.ports.entry(port_name.to_owned()).or_insert_with(|| PortData {
            entries: VecDeque::new(), show_rx: true, show_tx: true,
        });

        ui.horizontal(|ui| {
            ui.label(port_name);
            ui.checkbox(&mut data.show_rx, "RX");
            ui.checkbox(&mut data.show_tx, "TX");
            ui.checkbox(&mut self.show_hex, "HEX");
            if self.auto_scroll {
                if ui.button("⏸").on_hover_text("暂停").clicked() { self.auto_scroll = false; }
            } else if ui.button("↓").on_hover_text("滚到底部").clicked() { self.auto_scroll = true; }
            if ui.button("清空").clicked() { data.entries.clear(); }
            if ui.button("⛶").on_hover_text("放大查看").clicked() { self.maximize_clicked = true; }
        });
        ui.separator();

        let scroll_to_bottom = new_entries > 0 && self.auto_scroll;
        let scroll_id = format!("terminal-{port_name}");
        let scroll_output = ScrollArea::vertical().auto_shrink([false, false]).id_salt(scroll_id).show(ui, |ui| {
            for entry in data.entries.iter().filter(|e| match e.direction {
                Direction::Rx => data.show_rx, Direction::Tx => data.show_tx, Direction::Internal => false,
            }) {
                ui.horizontal_wrapped(|ui| {
                    let (label, color) = direction_label(entry.direction);
                    ui.label(RichText::new(format!("[{}]", fmt_ts(entry.timestamp_ms))).monospace());
                    ui.label(RichText::new(label).strong().color(color));
                    if self.show_hex {
                        ui.label(RichText::new(format_hex(&entry.bytes)).monospace());
                        if !entry.bytes.is_empty() {
                            ui.label(RichText::new(format!("[{}]", format_ascii(&entry.bytes))).monospace().color(theme::TEXT_DIMMED));
                        }
                    } else {
                        ui.label(RichText::new(&entry.text).monospace());
                    }
                });
            }
            if scroll_to_bottom { ui.scroll_to_cursor(Some(egui::Align::BOTTOM)); }
        });

        if ui.input(|i| i.smooth_scroll_delta.y > 0.0)
            && ui.input(|i| i.pointer.hover_pos()).is_some_and(|pos| scroll_output.inner_rect.contains(pos))
        { self.auto_scroll = false; }
        let at_bottom = scroll_output.state.offset.y >= scroll_output.content_size.y - scroll_output.inner_rect.height() - 4.0;
        if !self.auto_scroll && at_bottom { self.auto_scroll = true; }
    }

    /// 兼容旧版单面板视图（显示所有端口混合数据）
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let new_entries = self.ingest();
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_hex, "HEX");
            if self.auto_scroll {
                if ui.button("⏸").on_hover_text("暂停").clicked() { self.auto_scroll = false; }
            } else if ui.button("↓").on_hover_text("滚到底部").clicked() { self.auto_scroll = true; }
            if ui.button("清空").clicked() { self.ports.clear(); }
            if ui.button("⛶").on_hover_text("放大查看").clicked() { self.maximize_clicked = true; }
        });
        ui.separator();

        let scroll_to_bottom = new_entries > 0 && self.auto_scroll;
        let scroll_output = ScrollArea::vertical().auto_shrink([false, false]).id_salt("terminal-all").show(ui, |ui| {
            for (port, data) in &self.ports {
                for entry in &data.entries {
                    ui.horizontal_wrapped(|ui| {
                        let (label, color) = direction_label(entry.direction);
                        ui.label(RichText::new(format!("[{}]", fmt_ts(entry.timestamp_ms))).monospace());
                        ui.label(RichText::new(port).monospace().color(theme::YELLOW));
                        ui.label(RichText::new(label).strong().color(color));
                        if self.show_hex {
                            ui.label(RichText::new(format_hex(&entry.bytes)).monospace());
                        } else {
                            ui.label(RichText::new(&entry.text).monospace());
                        }
                    });
                }
            }
            if scroll_to_bottom { ui.scroll_to_cursor(Some(egui::Align::BOTTOM)); }
        });

        if ui.input(|i| i.smooth_scroll_delta.y > 0.0)
            && ui.input(|i| i.pointer.hover_pos()).is_some_and(|pos| scroll_output.inner_rect.contains(pos))
        { self.auto_scroll = false; }
        let at_bottom = scroll_output.state.offset.y >= scroll_output.content_size.y - scroll_output.inner_rect.height() - 4.0;
        if !self.auto_scroll && at_bottom { self.auto_scroll = true; }
    }

    fn ingest(&mut self) -> usize {
        let mut count = 0;
        for event in self.subscription.drain() {
            if !matches!(event.topic.as_str(), topics::SERIAL_RX | topics::SERIAL_TX) { continue; }
            let port = event.metadata.get("port").and_then(|v| v.as_str()).unwrap_or("default").to_owned();
            let data = self.ports.entry(port).or_insert_with(|| PortData {
                entries: VecDeque::new(), show_rx: true, show_tx: true,
            });
            let bytes = match &event.payload {
                Payload::Bytes(b) => b.clone(),
                _ => event.payload.text_lossy().into_bytes(),
            };
            data.entries.push_back(TerminalEntry {
                timestamp_ms: event.timestamp_ms,
                direction: event.direction,
                text: event.payload.text_lossy(),
                bytes,
            });
            while data.entries.len() > self.max_entries { data.entries.pop_front(); }
            count += 1;
        }
        count
    }
}

fn format_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}
fn format_ascii(bytes: &[u8]) -> String {
    bytes.iter().map(|b| if b.is_ascii_graphic() || *b == b' ' { char::from(*b) } else { '.' }).collect()
}
fn fmt_ts(ms: u64) -> String {
    let secs = (ms / 1000) % 86400;
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}
fn direction_label(d: Direction) -> (&'static str, Color32) {
    match d { Direction::Rx => ("RX", theme::GREEN), Direction::Tx => ("TX", theme::BLUE), Direction::Internal => ("IN", Color32::GRAY) }
}
