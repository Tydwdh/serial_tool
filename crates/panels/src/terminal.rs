use crate::{MAX_INGEST_PER_FRAME, fmt_ts, theme};
use egui::{Color32, RichText, ScrollArea};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tool_core::{Direction, Event, Payload};
use tool_databus::{DataBus, Subscription, TopicFilter};
use tool_transport::serial_topics;

const TIME_COL_WIDTH: f32 = 118.0;
const PORT_COL_WIDTH: f32 = 52.0;
const DIR_COL_WIDTH: f32 = 28.0;
const ROW_LEFT_PADDING: f32 = 4.0;
const COL_GAP: f32 = 3.0;

pub struct TerminalPanel {
    subscription: Subscription,
    ports: BTreeMap<String, PortData>,

    show_rx: bool,
    show_tx: bool,
    show_hex: bool,
    show_raw: bool,
    show_timestamp: bool,
    show_port: bool,
    auto_scroll: bool,

    search_text: String,
    port_filter: Option<String>,
    bookmarked_entry_ids: BTreeSet<u64>,

    max_entries: usize,

    pub height: f32,
    pub maximize_clicked: bool,

    last_scroll_offsets: BTreeMap<String, f32>,
    pending_scroll_to_bottom_keys: BTreeSet<String>,

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

    /// 预缓存的小写字段，用于搜索时避免每帧分配
    search_lower: String,
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
            show_raw: false,
            show_timestamp: true,
            show_port: true,
            auto_scroll: true,

            search_text: String::new(),
            port_filter: None,
            bookmarked_entry_ids: BTreeSet::new(),

            max_entries: 2_000,

            height: 350.0,
            maximize_clicked: false,

            last_scroll_offsets: BTreeMap::new(),
            pending_scroll_to_bottom_keys: BTreeSet::new(),

            next_entry_id: 1,
            selected_entry_id: None,
            detail_entry_id: None,
        }
    }
    pub fn ingest_all_pending(&mut self) -> usize {
        // 每帧最多摄入 5000 条，防止大量数据突发时 UI 卡顿
        const MAX_INGEST_ALL: usize = 5000;
        let mut count = 0;

        while let Some(event) = self.subscription.try_recv() {
            if !matches!(
                event.topic.as_str(),
                serial_topics::SERIAL_RX | serial_topics::SERIAL_TX
            ) {
                continue;
            }

            self.push_event(event);
            count += 1;
            if count >= MAX_INGEST_ALL {
                break;
            }
        }

        count
    }
    pub fn ingest_pending(&mut self) -> usize {
        self.ingest()
    }

    pub fn clear(&mut self) {
        self.ports.clear();
        self.last_scroll_offsets.clear();
        self.pending_scroll_to_bottom_keys.clear();
        self.selected_entry_id = None;
        self.detail_entry_id = None;
        self.search_text.clear();
        self.port_filter = None;
        self.bookmarked_entry_ids.clear();
        // 清空后重置为自动滚动，与 LogPanel::clear() 保持一致
        self.show_timestamp = true;
        self.show_port = true;
        self.show_raw = false;
        self.auto_scroll = true;
    }

    pub fn is_bookmarked(&self, entry_id: u64) -> bool {
        self.bookmarked_entry_ids.contains(&entry_id)
    }

    pub fn toggle_bookmark(&mut self, entry_id: u64) {
        if !self.bookmarked_entry_ids.insert(entry_id) {
            self.bookmarked_entry_ids.remove(&entry_id);
        }
    }

    /// 返回当前过滤条件下可见的条目迭代器 (port_name, &TerminalEntry)
    fn filtered_entries(&self) -> Vec<(String, &TerminalEntry)> {
        // 预计算搜索查询的小写版本，避免在循环中重复分配
        let search_lower = self.search_text.trim().to_ascii_lowercase();
        let mut result = Vec::new();
        for (port, data) in &self.ports {
            if let Some(ref filter) = self.port_filter
                && filter != port
            {
                continue;
            }
            let port_lower = port.to_ascii_lowercase();
            for entry in &data.entries {
                if !entry_visible(entry.direction, self.show_rx, self.show_tx) {
                    continue;
                }
                if !entry_matches_search(&port_lower, entry, &search_lower) {
                    continue;
                }
                result.push((port.clone(), entry));
            }
        }
        result
    }

    pub fn export_visible_csv(&self) -> String {
        let show_hex = self.show_hex;
        let show_timestamp = self.show_timestamp;
        let show_port = self.show_port;

        let mut headers: Vec<&str> = Vec::new();
        if show_timestamp { headers.push("time"); }
        if show_port { headers.push("port"); }
        headers.push("direction");
        if show_hex { headers.push("hex"); } else { headers.push("text"); }

        let mut out = headers.join(",");
        out.push('\n');

        for (port, entry) in self.filtered_entries() {
            let mut cells: Vec<String> = Vec::new();
            if show_timestamp {
                cells.push(csv_cell(&entry.timestamp_label));
            }
            if show_port {
                cells.push(csv_cell(&port));
            }
            cells.push(csv_cell(match entry.direction {
                Direction::Rx => "RX",
                Direction::Tx => "TX",
                Direction::Internal => "INTERNAL",
            }));
            if show_hex {
                cells.push(csv_cell(&entry.hex_text));
            } else {
                cells.push(csv_cell(&entry.raw_text));
            }
            out.push_str(&cells.join(","));
            out.push('\n');
        }
        out
    }

    pub fn export_visible_jsonl(&self) -> String {
        let show_hex = self.show_hex;
        let show_timestamp = self.show_timestamp;
        let show_port = self.show_port;

        let mut out = String::new();
        for (port, entry) in self.filtered_entries() {
            let mut obj = serde_json::Map::new();
            if show_timestamp {
                obj.insert("time".into(), serde_json::Value::String(entry.timestamp_label.clone()));
            }
            if show_port {
                obj.insert("port".into(), serde_json::Value::String(port.clone()));
            }
            obj.insert("direction".into(), serde_json::Value::String(match entry.direction {
                Direction::Rx => "RX".into(),
                Direction::Tx => "TX".into(),
                Direction::Internal => "INTERNAL".into(),
            }));
            if show_hex {
                obj.insert("hex".into(), serde_json::Value::String(entry.hex_text.clone()));
            } else {
                obj.insert("text".into(), serde_json::Value::String(entry.raw_text.clone()));
            }
            out.push_str(&serde_json::to_string(&serde_json::Value::Object(obj)).unwrap_or_else(|_| "{}".to_owned()));
            out.push('\n');
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
        let wheel_moves_towards_bottom =
            crate::scroll_delta_moves_towards_bottom(ui.input(|input| input.smooth_scroll_delta.y));

        let scroll_key = format!("terminal-port-{port_name}");
        let mut force_scroll_to_bottom = self.pending_scroll_to_bottom_keys.remove(&scroll_key);

        let render_outcome = {
            let data = self.ports.entry(port_name.to_owned()).or_default();

            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(port_name).monospace().strong());

                ui.checkbox(&mut data.show_rx, "RX");
                ui.checkbox(&mut data.show_tx, "TX");
                ui.checkbox(&mut show_hex, "HEX");
                ui.checkbox(&mut self.show_raw, "原始");
                ui.checkbox(&mut self.show_timestamp, "时间");

                force_scroll_to_bottom |= crate::theme::auto_scroll_button(ui, &mut auto_scroll);

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

            force_scroll_to_bottom |= auto_scroll && wheel_moves_towards_bottom;

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
                self.show_raw,
                self.show_timestamp,
                true, // 单端口视图始终显示端口名（工具栏已标明）
                auto_scroll,
                force_scroll_to_bottom,
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
        let wheel_moves_towards_bottom =
            crate::scroll_delta_moves_towards_bottom(ui.input(|input| input.smooth_scroll_delta.y));
        let mut force_scroll_to_bottom = self.pending_scroll_to_bottom_keys.remove(&scroll_key);

        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.show_rx, "RX");
            ui.checkbox(&mut self.show_tx, "TX");
            ui.checkbox(&mut self.show_hex, "HEX");
            ui.checkbox(&mut self.show_raw, "原始");

            force_scroll_to_bottom |= crate::theme::auto_scroll_button(ui, &mut self.auto_scroll);

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

        force_scroll_to_bottom |= self.auto_scroll && wheel_moves_towards_bottom;

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

            ui.checkbox(&mut self.show_timestamp, "时间");
            ui.checkbox(&mut self.show_port, "端口");

            if ui.button("清除筛选").clicked() {
                self.search_text.clear();
                self.port_filter = None;
            }
        });

        ui.separator();

        let render_outcome = {
            // 预计算搜索查询的小写版本，避免在渲染循环中重复分配
            let search_lower = self.search_text.trim().to_ascii_lowercase();
            let mut rows: Vec<VisibleRow<'_>> = Vec::new();

            for (port, data) in &self.ports {
                if let Some(filter_port) = &self.port_filter
                    && filter_port != port
                {
                    continue;
                }
                let port_lower = port.to_ascii_lowercase();

                for entry in data
                    .entries
                    .iter()
                    .filter(|entry| entry_visible(entry.direction, self.show_rx, self.show_tx))
                    .filter(|entry| entry_matches_search(&port_lower, entry, &search_lower))
                {
                    rows.push(VisibleRow {
                        port: Some(port.as_str()),
                        entry,
                    });
                }
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
                self.show_raw,
                self.show_timestamp,
                self.show_port,
                self.auto_scroll,
                force_scroll_to_bottom,
            )
        };

        self.apply_render_outcome(&scroll_key, render_outcome, ui);
        self.detail_popup(ui.ctx());
    }

    fn apply_render_outcome(&mut self, scroll_key: &str, outcome: RenderOutcome, ui: &egui::Ui) {
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

        let previous_offset_y = self
            .last_scroll_offsets
            .get(scroll_key)
            .copied()
            .unwrap_or(offset_y);
        let next_auto_scroll = crate::next_auto_scroll_state(
            self.auto_scroll,
            pointer_inside,
            smooth_scroll_y,
            previous_offset_y,
            offset_y,
            content_height,
            inner_rect.height(),
        );
        let should_repair_stick_to_bottom = next_auto_scroll
            && !crate::scroll_delta_moves_away_from_bottom(smooth_scroll_y)
            && !crate::scroll_is_at_bottom(offset_y, content_height, inner_rect.height());

        if self.auto_scroll != next_auto_scroll {
            if !self.auto_scroll && next_auto_scroll {
                self.pending_scroll_to_bottom_keys
                    .insert(scroll_key.to_owned());
            }

            self.auto_scroll = next_auto_scroll;
            ui.ctx().request_repaint();
        }

        if should_repair_stick_to_bottom {
            self.pending_scroll_to_bottom_keys
                .insert(scroll_key.to_owned());
            ui.ctx().request_repaint();
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

            if !matches!(
                event.topic.as_str(),
                serial_topics::SERIAL_RX | serial_topics::SERIAL_TX
            ) {
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

        // 预计算搜索用小写字符串，将所有可搜索字段合并
        let search_lower = {
            let mut s =
                String::with_capacity(raw_text.len() + display_text.len() + hex_text.len() + 1);
            s.push_str(&raw_text.to_ascii_lowercase());
            s.push('\0'); // 分隔符，防止跨字段匹配
            s.push_str(&display_text.to_ascii_lowercase());
            s.push('\0');
            s.push_str(&hex_text.to_ascii_lowercase());
            s
        };

        data.entries.push_back(TerminalEntry {
            id: entry_id,
            event_id: event.id,

            timestamp_label: format!("[{}]", fmt_ts(event.timestamp_ms)),
            direction: event.direction,

            raw_text,
            display_text,

            hex_text,
            hex_preview,
            search_lower,
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

                // 清理已截断条目的书签，避免内存泄漏
                self.bookmarked_entry_ids.remove(&removed.id);
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
                        let mut raw_text = format_raw_visible(&detail.raw_text);
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

#[allow(clippy::too_many_arguments)]
fn render_rows_view(
    ui: &mut egui::Ui,
    scroll_key: &str,
    height: f32,
    rows: &[VisibleRow<'_>],
    show_hex: bool,
    show_raw: bool,
    show_timestamp: bool,
    show_port: bool,
    stick_to_bottom: bool,
    force_scroll_to_bottom: bool,
) -> RenderOutcome {
    let height = height.max(40.0);
    let base_row_height = terminal_row_height(ui);

    if rows.is_empty() {
        let scroll_output = ScrollArea::vertical()
            .max_height(height)
            .auto_shrink([false, false])
            .id_salt((scroll_key, "v2"))
            .show(ui, |ui| {
                ui.label(RichText::new("暂无串口数据").color(theme::TEXT_SECONDARY));
            });

        return RenderOutcome {
            inner_rect: scroll_output.inner_rect,
            content_height: scroll_output.content_size.y,
            offset_y: scroll_output.state.offset.y,
        };
    }

    // Compute label column width based on visible flags
    let mut label_width = ROW_LEFT_PADDING;
    if show_timestamp {
        label_width += TIME_COL_WIDTH + COL_GAP;
    }
    if show_port {
        label_width += PORT_COL_WIDTH + COL_GAP;
    }
    label_width += DIR_COL_WIDTH + COL_GAP;

    // Build combined text: one line per entry
    let combined_text: String = rows
        .iter()
        .map(|row| {
            let content = entry_content_text(row.entry, show_hex, show_raw);
            // Replace newlines with spaces so each entry is exactly one line
            content.replace('\n', " ")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let num_rows = rows.len();
    let content_height = base_row_height * num_rows as f32;
    let total_height = content_height.max(1.0);

    let scroll_output = ScrollArea::vertical()
        .max_height(height)
        .auto_shrink([false, false])
        .stick_to_bottom(stick_to_bottom)
        .id_salt((scroll_key, "v2"))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // --- Left: painter-drawn labels ---
                let label_rect = ui
                    .allocate_exact_size(
                        egui::vec2(label_width, total_height),
                        egui::Sense::hover(),
                    )
                    .0;

                let painter = ui.painter_at(label_rect);
                let font_id = egui::TextStyle::Monospace.resolve(ui.style());

                for (i, row) in rows.iter().enumerate() {
                    let text_y = label_rect.top() + base_row_height * (i as f32 + 0.5);
                    let mut x = label_rect.left() + ROW_LEFT_PADDING;

                    if show_timestamp {
                        painter.text(
                            egui::pos2(x, text_y),
                            egui::Align2::LEFT_CENTER,
                            &row.entry.timestamp_label,
                            font_id.clone(),
                            theme::TEXT_SECONDARY,
                        );
                        x += TIME_COL_WIDTH + COL_GAP;
                    }

                    if show_port {
                        if let Some(port) = row.port {
                            painter.text(
                                egui::pos2(x, text_y),
                                egui::Align2::LEFT_CENTER,
                                port,
                                font_id.clone(),
                                theme::YELLOW,
                            );
                        }
                        x += PORT_COL_WIDTH + COL_GAP;
                    }

                    let (dir_label, dir_color) = direction_label(row.entry.direction);
                    painter.text(
                        egui::pos2(x, text_y),
                        egui::Align2::LEFT_CENTER,
                        dir_label,
                        font_id.clone(),
                        dir_color,
                    );
                }

                // --- Right: read-only TextEdit ---
                let mut text_copy = combined_text.clone();
                let text_edit_response = ui.add(
                    egui::TextEdit::multiline(&mut text_copy)
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false),
                );

                text_edit_response.context_menu(|ctx_ui| {
                    // Determine which row is under the cursor by checking
                    // the TextEdit's cursor position (line number)
                    // For now, provide generic copy-all actions
                    if ctx_ui.button("复制全部可见内容").clicked() {
                        ctx_ui.ctx().copy_text(combined_text.clone());
                        ctx_ui.close();
                    }

                    ctx_ui.separator();

                    // Build and provide per-entry copy submenus
                    // We can determine the row from cursor position
                    // But for simplicity, just provide a generic detail action
                    if ctx_ui.button("复制 CSV").clicked() {
                        // Build a simple CSV of visible rows
                        let csv: String = rows
                            .iter()
                            .map(|row| {
                                let content = entry_content_text(row.entry, show_hex, show_raw);
                                let port = row.port.unwrap_or("");
                                format!(
                                    "{},{},{},{}",
                                    row.entry.timestamp_label,
                                    port,
                                    match row.entry.direction {
                                        Direction::Rx => "RX",
                                        Direction::Tx => "TX",
                                        Direction::Internal => "IN",
                                    },
                                    content.replace('\n', " ")
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        ctx_ui.ctx().copy_text(csv);
                        ctx_ui.close();
                    }
                });
            });

            if force_scroll_to_bottom {
                // Add an invisible anchor at the bottom to scroll to.
                // scroll_to_cursor_animation on a non-interactive TextEdit is a no-op,
                // so we use a tiny spacer widget and scroll to it instead.
                let (rect, _sense) = ui.allocate_exact_size(
                    egui::vec2(0.0, 0.0),
                    egui::Sense::hover(),
                );
                ui.scroll_to_rect(rect, Some(egui::Align::BOTTOM));
            }
        });

    RenderOutcome {
        inner_rect: scroll_output.inner_rect,
        content_height: scroll_output.content_size.y,
        offset_y: scroll_output.state.offset.y,
    }
}

fn terminal_row_height(ui: &egui::Ui) -> f32 {
    crate::row_height(ui)
}

/// Returns the content text for an entry based on display priority: hex > raw > display.
fn entry_content_text<'a>(entry: &'a TerminalEntry, show_hex: bool, show_raw: bool) -> &'a str {
    if show_hex {
        &entry.hex_preview
    } else if show_raw {
        &entry.raw_text
    } else {
        &entry.display_text
    }
}

fn entry_visible(direction: Direction, show_rx: bool, show_tx: bool) -> bool {
    match direction {
        Direction::Rx => show_rx,
        Direction::Tx => show_tx,
        Direction::Internal => false,
    }
}

fn entry_matches_search(port_lower: &str, entry: &TerminalEntry, search_lower: &str) -> bool {
    if search_lower.is_empty() {
        return true;
    }

    // port_lower 和 search_lower 都由调用端预计算，零额外分配
    port_lower.contains(search_lower) || entry.search_lower.contains(search_lower)
}

fn csv_cell(s: &str) -> String {
    let escaped = s.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

fn format_hex(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let mut s = String::with_capacity(bytes.len() * 3 - 1);
    use std::fmt::Write;
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        write!(s, "{byte:02X}").unwrap();
    }
    s
}

fn format_utf8_preview(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    format_terminal_text(&text)
}

fn format_terminal_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());

    for ch in text.chars() {
        match ch {
            '\r' => {} // 跳过独立的 \r，\r\n 由 \n 处理
            '\t' => output.push('\t'),
            ch if ch.is_control() && ch != '\n' => output.push('\u{00B7}'), // 中间点
            ch => output.push(ch),
        }
    }

    output
}

fn format_raw_visible(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\\' => output.push_str("\\\\"),
            '\0' => output.push_str("\\0"),
            ch if ch.is_control() => output.push_str(&format!("\\x{:02x}", ch as u8)),
            ch => output.push(ch),
        }
    }
    output
}

// fmt_ts 已提取到 crate::fmt_ts

fn direction_label(direction: Direction) -> (&'static str, Color32) {
    match direction {
        Direction::Rx => ("RX", theme::GREEN),
        Direction::Tx => ("TX", theme::BLUE),
        Direction::Internal => ("IN", Color32::GRAY),
    }
}

fn detail_text_rows(text: &str, min_rows: usize, max_rows: usize) -> usize {
    let line_count = text.lines().count().max(1);
    line_count.clamp(min_rows, max_rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_databus::DataBus;

    #[test]
    fn ingest_serial_rx_keeps_received_entry() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        bus.publish(
            Event::new(
                serial_topics::SERIAL_RX,
                "serial:COM1",
                Direction::Rx,
                Payload::Bytes(b"hello".to_vec()),
            )
            .with_metadata(serde_json::json!({ "port": "COM1" })),
        );

        assert_eq!(panel.ingest_all_pending(), 1);

        let port = panel.ports.get("COM1").expect("COM1 should have entries");
        assert_eq!(port.entries.len(), 1);

        let entry = port.entries.front().expect("rx entry should be ingested");
        assert_eq!(entry.direction, Direction::Rx);
        assert_eq!(entry.raw_text, "hello");
        assert_eq!(entry.display_text, "hello");
        assert_eq!(entry.hex_text, "68 65 6C 6C 6F");
    }
}
