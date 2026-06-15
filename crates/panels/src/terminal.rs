use crate::theme;
use egui::{Color32, RichText, ScrollArea};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tool_core::{Direction, Event, Payload, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};

const MAX_INGEST_PER_FRAME: usize = 500;

const TIME_COL_WIDTH: f32 = 118.0;
const PORT_COL_WIDTH: f32 = 64.0;
const DIR_COL_WIDTH: f32 = 28.0;
const ROW_LEFT_PADDING: f32 = 4.0;
const COL_GAP: f32 = 4.0;

pub struct TerminalPanel {
    subscription: Subscription,
    ports: BTreeMap<String, PortData>,

    show_rx: bool,
    show_tx: bool,
    show_hex: bool,
    auto_scroll: bool,

    search_text: String,
    port_filter: Option<String>,
    bookmarked_entry_ids: BTreeSet<u64>,

    max_entries: usize,

    pub height: f32,
    pub maximize_clicked: bool,

    last_scroll_offsets: BTreeMap<String, f32>,

    next_entry_id: u64,
    selected_entry_id: Option<u64>,
    detail_entry_id: Option<u64>,
}

struct PortData {
    entries: VecDeque<TerminalEntry>,
    show_rx: bool,
    show_tx: bool,
    truncated_count: u64,
}

struct TerminalEntry {
    /// TerminalPanel 内部使用的稳定 UI id。
    id: u64,

    /// DataBus 分配的全局事件 id。
    /// 全局接收区按这个排序，避免 BTreeMap 端口顺序导致 COM 分组。
    event_id: u64,

    timestamp_label: String,
    direction: Direction,

    raw_text: String,
    display_text: String,

    hex_text: String,
    hex_preview: String,
}

struct VisibleRow<'a> {
    port: Option<&'a str>,
    entry: &'a TerminalEntry,
}

#[derive(Clone)]
struct EntryDetail {
    port: String,
    timestamp_label: String,
    direction: Direction,

    raw_text: String,
    display_text: String,
    hex_text: String,
}

struct RenderOutcome {
    inner_rect: egui::Rect,
    content_height: f32,
    offset_y: f32,
    clicked_entry_id: Option<u64>,
    open_detail_entry_id: Option<u64>,
}

impl TerminalPanel {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            subscription: bus
                .subscribe_lossy_bounded(TopicFilter::prefix("transport.serial."), 4096),
            ports: BTreeMap::new(),

            show_rx: true,
            show_tx: true,
            show_hex: false,
            auto_scroll: true,

            search_text: String::new(),
            port_filter: None,
            bookmarked_entry_ids: BTreeSet::new(),

            max_entries: 2_000,

            height: 350.0,
            maximize_clicked: false,

            last_scroll_offsets: BTreeMap::new(),

            next_entry_id: 1,
            selected_entry_id: None,
            detail_entry_id: None,
        }
    }
    pub fn ingest_all_pending(&mut self) -> usize {
        let mut count = 0;

        while let Some(event) = self.subscription.try_recv() {
            if !matches!(event.topic.as_str(), topics::SERIAL_RX | topics::SERIAL_TX) {
                continue;
            }

            self.push_event(event);
            count += 1;
        }

        count
    }
    pub fn ingest_pending(&mut self) -> usize {
        self.ingest()
    }

    pub fn clear(&mut self) {
        self.ports.clear();
        self.last_scroll_offsets.clear();
        self.selected_entry_id = None;
        self.detail_entry_id = None;
        self.search_text.clear();
        self.port_filter = None;
        self.bookmarked_entry_ids.clear();
    }

    pub fn is_bookmarked(&self, entry_id: u64) -> bool {
        self.bookmarked_entry_ids.contains(&entry_id)
    }

    pub fn toggle_bookmark(&mut self, entry_id: u64) {
        if !self.bookmarked_entry_ids.insert(entry_id) {
            self.bookmarked_entry_ids.remove(&entry_id);
        }
    }

    pub fn export_visible_csv(&self) -> String {
        let mut out = String::from("time,port,direction,text,hex\n");
        for (port, data) in &self.ports {
            if let Some(ref filter) = self.port_filter {
                if filter != port {
                    continue;
                }
            }
            for entry in &data.entries {
                if !entry_visible(entry.direction, self.show_rx, self.show_tx) {
                    continue;
                }
                if !entry_matches_search(port, entry, &self.search_text) {
                    continue;
                }
                out.push_str(&csv_cell(&entry.timestamp_label));
                out.push(',');
                out.push_str(&csv_cell(port));
                out.push(',');
                out.push_str(&csv_cell(match entry.direction {
                    Direction::Rx => "RX",
                    Direction::Tx => "TX",
                    Direction::Internal => "INTERNAL",
                }));
                out.push(',');
                out.push_str(&csv_cell(&entry.raw_text));
                out.push(',');
                out.push_str(&csv_cell(&entry.hex_text));
                out.push('\n');
            }
        }
        out
    }

    pub fn export_visible_jsonl(&self) -> String {
        let mut out = String::new();
        for (port, data) in &self.ports {
            if let Some(ref filter) = self.port_filter {
                if filter != port {
                    continue;
                }
            }
            for entry in &data.entries {
                if !entry_visible(entry.direction, self.show_rx, self.show_tx) {
                    continue;
                }
                if !entry_matches_search(port, entry, &self.search_text) {
                    continue;
                }
                let line = serde_json::json!({
                    "time": entry.timestamp_label,
                    "port": port,
                    "direction": match entry.direction {
                        Direction::Rx => "RX",
                        Direction::Tx => "TX",
                        Direction::Internal => "INTERNAL",
                    },
                    "text": entry.raw_text,
                    "hex": entry.hex_text,
                });
                out.push_str(&serde_json::to_string(&line).unwrap_or_default());
                out.push('\n');
            }
        }
        out
    }

    pub fn port_names(&self) -> Vec<String> {
        self.ports.keys().cloned().collect()
    }

    pub fn port_ui(&mut self, ui: &mut egui::Ui, port_name: &str) {
        let _new_entries = self.ingest();

        let mut show_hex = self.show_hex;
        let mut auto_scroll = self.auto_scroll;
        let mut maximize_clicked = false;
        let mut clear_selection = false;

        let scroll_key = format!("terminal-port-{port_name}");

        let render_outcome = {
            let data = self.ports.entry(port_name.to_owned()).or_default();
            let mut force_scroll_to_bottom = false;

            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(port_name).monospace().strong());

                ui.checkbox(&mut data.show_rx, "RX");
                ui.checkbox(&mut data.show_tx, "TX");
                ui.checkbox(&mut show_hex, "HEX");

                force_scroll_to_bottom |= auto_scroll_button(ui, &mut auto_scroll);

                if ui.button("清空").clicked() {
                    data.entries.clear();
                    clear_selection = true;
                }

                if ui.button("⛶").on_hover_text("放大查看").clicked() {
                    maximize_clicked = true;
                }

                let dropped = self.subscription.dropped_count();
                if dropped > 0 {
                    ui.colored_label(theme::YELLOW, format!("已丢弃 {dropped} 条，数据不完整"));
                }
            });

            ui.separator();

            let rows: Vec<VisibleRow<'_>> = data
                .entries
                .iter()
                .filter(|entry| entry_visible(entry.direction, data.show_rx, data.show_tx))
                .map(|entry| VisibleRow { port: None, entry })
                .collect();

            let scroll_height = ui.available_height().max(40.0);
            render_rows_view(
                ui,
                &scroll_key,
                scroll_height,
                &rows,
                show_hex,
                auto_scroll,
                force_scroll_to_bottom,
                self.selected_entry_id,
            )
        };

        self.show_hex = show_hex;
        self.auto_scroll = auto_scroll;
        self.maximize_clicked |= maximize_clicked;

        if clear_selection {
            self.selected_entry_id = None;
            self.detail_entry_id = None;
        }

        self.apply_render_outcome(&scroll_key, render_outcome, ui);
        self.detail_popup(ui.ctx());
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let scroll_key = "terminal-all".to_owned();
        let mut force_scroll_to_bottom = false;

        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.show_rx, "RX");
            ui.checkbox(&mut self.show_tx, "TX");
            ui.checkbox(&mut self.show_hex, "HEX");

            force_scroll_to_bottom |= auto_scroll_button(ui, &mut self.auto_scroll);

            if ui.button("清空").clicked() {
                self.clear();
            }

            if ui
                .button("复制 CSV")
                .on_hover_text("复制过滤后的视图为 CSV")
                .clicked()
            {
                ui.ctx().copy_text(self.export_visible_csv());
            }

            if ui
                .button("复制 JSONL")
                .on_hover_text("复制过滤后的视图为 JSONL")
                .clicked()
            {
                ui.ctx().copy_text(self.export_visible_jsonl());
            }

            if ui.button("⛶").on_hover_text("放大查看").clicked() {
                self.maximize_clicked = true;
            }
        });

        ui.horizontal(|ui| {
            ui.label("搜索");
            ui.add(
                egui::TextEdit::singleline(&mut self.search_text)
                    .desired_width(140.0)
                    .hint_text("文本 / HEX"),
            );

            ui.label("端口");
            egui::ComboBox::from_id_salt("terminal-port-filter")
                .width(100.0)
                .selected_text(self.port_filter.as_deref().unwrap_or("全部"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.port_filter, None, "全部");
                    for port in self.ports.keys() {
                        ui.selectable_value(&mut self.port_filter, Some(port.clone()), port);
                    }
                });

            if ui.button("清除筛选").clicked() {
                self.search_text.clear();
                self.port_filter = None;
            }
        });

        ui.separator();

        let total_all: usize = self.ports.values().map(|d| d.entries.len()).sum();

        let render_outcome = {
            let mut rows: Vec<VisibleRow<'_>> = Vec::new();

            for (port, data) in &self.ports {
                if let Some(filter_port) = &self.port_filter {
                    if filter_port != port {
                        continue;
                    }
                }

                for entry in data
                    .entries
                    .iter()
                    .filter(|entry| entry_visible(entry.direction, self.show_rx, self.show_tx))
                    .filter(|entry| entry_matches_search(port, entry, &self.search_text))
                {
                    rows.push(VisibleRow {
                        port: Some(port.as_str()),
                        entry,
                    });
                }
            }

            let visible_count = rows.len();
            if visible_count != total_all {
                ui.label(
                    RichText::new(format!("{visible_count} / {total_all} 条"))
                        .color(theme::TEXT_SECONDARY),
                );
            }

            // 截断提示
            let total_truncated: u64 = self.ports.values().map(|d| d.truncated_count).sum();
            if total_truncated > 0 {
                ui.label(
                    RichText::new(format!(
                        "已截断 {total_truncated} 条，当前仅保留最近 {} 条",
                        self.max_entries
                    ))
                    .color(theme::YELLOW),
                );
            }

            // 关键修复：
            // 全局视图按 DataBus 发布顺序显示，不按端口名分组，也不按毫秒时间排序。
            //
            // timestamp_ms 在高频串口下会大量相同；
            // BTreeMap 遍历又会按 COM 名排序；
            // 所以只按 timestamp_ms 或 (timestamp_ms, local_id) 都可能看起来像 COM 分组。
            rows.sort_by_key(|row| row.entry.event_id);

            let scroll_height = ui.available_height().max(40.0);
            render_rows_view(
                ui,
                &scroll_key,
                scroll_height,
                &rows,
                self.show_hex,
                self.auto_scroll,
                force_scroll_to_bottom,
                self.selected_entry_id,
            )
        };

        self.apply_render_outcome(&scroll_key, render_outcome, ui);
        self.detail_popup(ui.ctx());
    }

    fn apply_render_outcome(&mut self, scroll_key: &str, outcome: RenderOutcome, ui: &egui::Ui) {
        if let Some(entry_id) = outcome.clicked_entry_id {
            self.selected_entry_id = Some(entry_id);
        }

        if let Some(entry_id) = outcome.open_detail_entry_id {
            self.selected_entry_id = Some(entry_id);
            self.detail_entry_id = Some(entry_id);
        }

        self.update_auto_scroll(
            ui,
            scroll_key,
            outcome.inner_rect,
            outcome.content_height,
            outcome.offset_y,
        );
    }

    fn update_auto_scroll(
        &mut self,
        ui: &egui::Ui,
        scroll_key: &str,
        inner_rect: egui::Rect,
        content_height: f32,
        offset_y: f32,
    ) {
        let pointer_inside = ui
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|pos| inner_rect.contains(pos));

        let smooth_scroll_y = ui.input(|input| input.smooth_scroll_delta.y);
        let scrolling_away_from_bottom = pointer_inside && smooth_scroll_y > 0.0;

        let previous_offset_y = self
            .last_scroll_offsets
            .get(scroll_key)
            .copied()
            .unwrap_or(offset_y);

        let moving_towards_bottom = offset_y > previous_offset_y + 0.5;

        let bottom_offset = (content_height - inner_rect.height()).max(0.0);
        let at_bottom = offset_y >= bottom_offset - 4.0;

        if scrolling_away_from_bottom {
            self.auto_scroll = false;
        }

        if !self.auto_scroll && at_bottom && pointer_inside && moving_towards_bottom {
            self.auto_scroll = true;
        }

        self.last_scroll_offsets
            .insert(scroll_key.to_owned(), offset_y);
    }

    fn ingest(&mut self) -> usize {
        let mut count = 0;

        for _ in 0..MAX_INGEST_PER_FRAME {
            let Some(event) = self.subscription.try_recv() else {
                break;
            };

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

        let raw_text = event.payload.text_lossy();
        let display_text = format_terminal_text(&raw_text);

        let hex_text = format_hex(&bytes);
        let utf8_preview = format_utf8_preview(&bytes);

        let hex_preview = if hex_text.is_empty() {
            String::new()
        } else if utf8_preview.is_empty() {
            hex_text.clone()
        } else {
            format!("{hex_text} [{utf8_preview}]")
        };

        let entry_id = self.next_entry_id;
        self.next_entry_id = self.next_entry_id.wrapping_add(1).max(1);

        let data = self.ports.entry(port).or_default();

        data.entries.push_back(TerminalEntry {
            id: entry_id,
            event_id: event.id,

            timestamp_label: format!("[{}]", fmt_ts(event.timestamp_ms)),
            direction: event.direction,

            raw_text,
            display_text,

            hex_text,
            hex_preview,
        });

        while data.entries.len() > self.max_entries {
            let removed = data.entries.pop_front();

            if let Some(removed) = removed {
                if self.selected_entry_id == Some(removed.id) {
                    self.selected_entry_id = None;
                }

                if self.detail_entry_id == Some(removed.id) {
                    self.detail_entry_id = None;
                }
            }
            data.truncated_count += 1;
        }
    }

    fn entry_detail(&self, entry_id: u64) -> Option<EntryDetail> {
        for (port, data) in &self.ports {
            for entry in &data.entries {
                if entry.id == entry_id {
                    return Some(EntryDetail {
                        port: port.clone(),
                        timestamp_label: entry.timestamp_label.clone(),
                        direction: entry.direction,

                        raw_text: entry.raw_text.clone(),
                        display_text: entry.display_text.clone(),
                        hex_text: entry.hex_text.clone(),
                    });
                }
            }
        }

        None
    }

    fn detail_popup(&mut self, ctx: &egui::Context) {
        let Some(entry_id) = self.detail_entry_id else {
            return;
        };

        let Some(detail) = self.entry_detail(entry_id) else {
            self.detail_entry_id = None;
            return;
        };

        let mut open = true;

        egui::Window::new("接收详情")
            .open(&mut open)
            .resizable(true)
            .default_size([760.0, 520.0])
            .min_size([520.0, 320.0])
            .show(ctx, |ui| {
                let (dir_label, dir_color) = direction_label(detail.direction);

                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&detail.timestamp_label).monospace());
                    ui.label(RichText::new(&detail.port).monospace().color(theme::YELLOW));
                    ui.label(RichText::new(dir_label).strong().color(dir_color));

                    if ui.button("复制内容").clicked() {
                        ui.ctx().copy_text(detail.raw_text.clone());
                    }

                    if ui.button("复制显示文本").clicked() {
                        ui.ctx().copy_text(detail.display_text.clone());
                    }

                    if ui.button("复制 HEX").clicked() {
                        ui.ctx().copy_text(detail.hex_text.clone());
                    }
                });

                ui.separator();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(ui.available_height())
                    .show(ui, |ui| {
                        ui.label(RichText::new("原始内容").strong());
                        let mut raw_text = detail.raw_text.clone();
                        ui.add(
                            egui::TextEdit::multiline(&mut raw_text)
                                .desired_width(f32::INFINITY)
                                .desired_rows(detail_text_rows(&detail.raw_text, 6, 14))
                                .font(egui::TextStyle::Monospace),
                        );

                        ui.separator();

                        ui.label(RichText::new("显示文本").strong());
                        let mut display_text = detail.display_text.clone();
                        ui.add(
                            egui::TextEdit::multiline(&mut display_text)
                                .desired_width(f32::INFINITY)
                                .desired_rows(detail_text_rows(&detail.display_text, 4, 10))
                                .font(egui::TextStyle::Monospace),
                        );

                        ui.separator();

                        ui.label(RichText::new("HEX").strong());
                        let mut hex_text = detail.hex_text.clone();
                        ui.add(
                            egui::TextEdit::multiline(&mut hex_text)
                                .desired_width(f32::INFINITY)
                                .desired_rows(detail_text_rows(&detail.hex_text, 4, 12))
                                .font(egui::TextStyle::Monospace),
                        );
                    });
            });

        if !open {
            self.detail_entry_id = None;
        }
    }
}

impl Default for PortData {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            show_rx: true,
            show_tx: true,
            truncated_count: 0,
        }
    }
}

fn render_rows_view(
    ui: &mut egui::Ui,
    scroll_key: &str,
    height: f32,
    rows: &[VisibleRow<'_>],
    show_hex: bool,
    stick_to_bottom: bool,
    force_scroll_to_bottom: bool,
    selected_entry_id: Option<u64>,
) -> RenderOutcome {
    let height = height.max(40.0);
    let row_height = terminal_row_height(ui);

    if rows.is_empty() {
        let scroll_output = ScrollArea::vertical()
            .max_height(height)
            .auto_shrink([false, false])
            .id_salt(scroll_key)
            .show(ui, |ui| {
                ui.label(RichText::new("暂无串口数据").color(theme::TEXT_SECONDARY));
            });

        return RenderOutcome {
            inner_rect: scroll_output.inner_rect,
            content_height: scroll_output.content_size.y,
            offset_y: scroll_output.state.offset.y,
            clicked_entry_id: None,
            open_detail_entry_id: None,
        };
    }

    let mut clicked_entry_id = None;
    let mut open_detail_entry_id = None;

    let mut scroll_area = ScrollArea::vertical()
        .max_height(height)
        .auto_shrink([false, false])
        .stick_to_bottom(stick_to_bottom)
        .id_salt(scroll_key);

    if force_scroll_to_bottom {
        scroll_area = scroll_area.vertical_scroll_offset(1e9);
    }

    let scroll_output = scroll_area.show_rows(ui, row_height, rows.len(), |ui, row_range| {
        for row_index in row_range {
            let row = &rows[row_index];
            let selected = selected_entry_id == Some(row.entry.id);

            let response = show_entry_fast(ui, row.port, row.entry, show_hex, row_height, selected);

            if response.clicked() {
                clicked_entry_id = Some(row.entry.id);
            }

            if response.double_clicked() {
                clicked_entry_id = Some(row.entry.id);
                open_detail_entry_id = Some(row.entry.id);
            }

            response.context_menu(|ui| {
                if ui.button("复制内容").clicked() {
                    ui.ctx().copy_text(row.entry.raw_text.clone());
                    ui.close();
                }

                if ui.button("复制显示文本").clicked() {
                    ui.ctx().copy_text(row.entry.display_text.clone());
                    ui.close();
                }

                if ui.button("复制 HEX").clicked() {
                    ui.ctx().copy_text(row.entry.hex_text.clone());
                    ui.close();
                }

                ui.separator();

                if ui.button("查看详情").clicked() {
                    open_detail_entry_id = Some(row.entry.id);
                    ui.close();
                }
            });
        }
    });

    RenderOutcome {
        inner_rect: scroll_output.inner_rect,
        content_height: scroll_output.content_size.y,
        offset_y: scroll_output.state.offset.y,
        clicked_entry_id,
        open_detail_entry_id,
    }
}

fn terminal_row_height(ui: &egui::Ui) -> f32 {
    (ui.text_style_height(&egui::TextStyle::Monospace).ceil() + 6.0).max(20.0)
}

fn entry_visible(direction: Direction, show_rx: bool, show_tx: bool) -> bool {
    match direction {
        Direction::Rx => show_rx,
        Direction::Tx => show_tx,
        Direction::Internal => false,
    }
}

fn entry_matches_search(port: &str, entry: &TerminalEntry, search: &str) -> bool {
    let q = search.trim();
    if q.is_empty() {
        return true;
    }

    let q = q.to_ascii_lowercase();

    port.to_ascii_lowercase().contains(&q)
        || entry.raw_text.to_ascii_lowercase().contains(&q)
        || entry.display_text.to_ascii_lowercase().contains(&q)
        || entry.hex_text.to_ascii_lowercase().contains(&q)
}

fn csv_cell(s: &str) -> String {
    let escaped = s.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

fn auto_scroll_button(ui: &mut egui::Ui, auto_scroll: &mut bool) -> bool {
    if *auto_scroll {
        if ui.button("⏸").on_hover_text("暂停自动滚动").clicked() {
            *auto_scroll = false;
        }

        false
    } else if ui.button("↓").on_hover_text("滚动到底部").clicked() {
        *auto_scroll = true;
        true
    } else {
        false
    }
}

fn format_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_utf8_preview(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    format_terminal_text(&text)
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
    let Some(dt_utc) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms as i64) else {
        return "--:--:--.---".to_owned();
    };

    dt_utc
        .with_timezone(&chrono::Local)
        .format("%H:%M:%S%.3f")
        .to_string()
}

fn direction_label(direction: Direction) -> (&'static str, Color32) {
    match direction {
        Direction::Rx => ("RX", theme::GREEN),
        Direction::Tx => ("TX", theme::BLUE),
        Direction::Internal => ("IN", Color32::GRAY),
    }
}

fn show_entry_fast(
    ui: &mut egui::Ui,
    port: Option<&str>,
    entry: &TerminalEntry,
    show_hex: bool,
    row_height: f32,
    selected: bool,
) -> egui::Response {
    let row_width = ui.available_width();

    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(row_width, row_height), egui::Sense::click());

    let bg = if selected {
        theme::BG_SELECTION
    } else if response.hovered() {
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

    if let Some(port) = port {
        painter.text(
            egui::pos2(x, text_y),
            egui::Align2::LEFT_CENTER,
            port,
            font_id.clone(),
            theme::YELLOW,
        );
        x += PORT_COL_WIDTH + COL_GAP;
    }

    let (dir_label, dir_color) = direction_label(entry.direction);

    painter.text(
        egui::pos2(x, text_y),
        egui::Align2::LEFT_CENTER,
        dir_label,
        font_id.clone(),
        dir_color,
    );
    x += DIR_COL_WIDTH + COL_GAP;

    let payload = if show_hex {
        &entry.hex_preview
    } else {
        &entry.display_text
    };

    let payload_clip = egui::Rect::from_min_max(
        egui::pos2(x, rect.top()),
        egui::pos2(rect.right(), rect.bottom()),
    );

    let payload_painter = ui.painter().with_clip_rect(payload_clip);

    payload_painter.text(
        egui::pos2(x, text_y),
        egui::Align2::LEFT_CENTER,
        payload,
        font_id,
        theme::TEXT_PRIMARY,
    );

    response
}
fn detail_text_rows(text: &str, min_rows: usize, max_rows: usize) -> usize {
    let line_count = text.lines().count().max(1);
    line_count.clamp(min_rows, max_rows)
}
