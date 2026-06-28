use crate::{MAX_INGEST_PER_FRAME, fmt_ts, table::RowHighlight, theme};
use egui::text_selection::LabelSelectionState;
use egui::{Color32, RichText, ScrollArea, Sense, Stroke};
use std::borrow::Cow;
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
    show_lines: bool,
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

    /// 用户可调的字体大小（10-24px），默认 13.0
    pub font_size: f32,
    /// 合并阈值：同一端口、同一方向、间隔 ≤ 此毫秒且不含 \n 的连续事件合并显示
    pub merge_window_ms: u64,
}

#[derive(Default)]
struct PortData {
    entries: VecDeque<TerminalEntry>,
    truncated_count: u64,
}

struct TerminalEntry {
    /// TerminalPanel 内部使用的稳定 UI id。
    id: u64,

    /// DataBus 分配的全局事件 id。
    /// 全局接收区按这个排序，避免 BTreeMap 端口顺序导致 COM 分组。
    event_id: u64,

    /// 原始时间戳（毫秒），用于合并判断。
    timestamp_ms: u64,
    timestamp_label: String,
    direction: Direction,

    raw_text: String,
    display_text: String,

    hex_text: String,
    #[allow(dead_code)]
    hex_preview: String,
    /// 纯 UTF8 预览文本（不含方括号），HEX 模式下独立显示
    preview_text: String,
}

struct VisibleRow<'a> {
    id: u64,
    event_id: u64,
    port: Option<Cow<'a, str>>,
    timestamp_label: Cow<'a, str>,
    direction: Direction,
    raw_text: Cow<'a, str>,
    display_text: Cow<'a, str>,
    hex_text: Cow<'a, str>,
    preview_text: Cow<'a, str>,
}

impl<'a> VisibleRow<'a> {
    fn from_entry(port: Option<&'a str>, entry: &'a TerminalEntry) -> Self {
        Self {
            id: entry.id,
            event_id: entry.event_id,
            port: port.map(Cow::Borrowed),
            timestamp_label: Cow::Borrowed(&entry.timestamp_label),
            direction: entry.direction,
            raw_text: Cow::Borrowed(&entry.raw_text),
            display_text: Cow::Borrowed(&entry.display_text),
            hex_text: Cow::Borrowed(&entry.hex_text),
            preview_text: Cow::Borrowed(&entry.preview_text),
        }
    }
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
            show_lines: false,
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
            font_size: 13.0,
            merge_window_ms: 10,
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
        while self.subscription.try_recv().is_some() {}
        self.ports.clear();
        self.last_scroll_offsets.clear();
        self.pending_scroll_to_bottom_keys.clear();
        self.selected_entry_id = None;
        self.detail_entry_id = None;
        self.search_text.clear();
        self.port_filter = None;
        self.bookmarked_entry_ids.clear();
        // 清空后重置为自动滚动，与 LogPanel::clear() 保持一致
        self.show_lines = false;
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

    fn collect_visible_rows(&self, line_mode: bool) -> Vec<VisibleRow<'_>> {
        let search_lower = self.search_text.trim().to_ascii_lowercase();
        let mut rows = Vec::new();

        for (port, data) in &self.ports {
            if let Some(ref filter) = self.port_filter
                && filter != port
            {
                continue;
            }

            let port_lower = port.to_ascii_lowercase();
            let mut port_rows = build_visible_rows_for_port(
                Some(port.as_str()),
                data.entries
                    .iter()
                    .filter(|entry| entry_visible(entry.direction, self.show_rx, self.show_tx)),
                line_mode,
            );
            if !search_lower.is_empty() {
                port_rows.retain(|row| row_matches_search(&port_lower, row, &search_lower));
            }
            rows.extend(port_rows);
        }

        rows.sort_by_key(|row| row.event_id);
        rows
    }

    pub fn export_visible_csv(&self) -> String {
        let show_hex = self.show_hex;
        let rows = self.collect_visible_rows(self.show_lines);
        let show_metadata = !self.show_lines;

        let mut headers: Vec<&str> = Vec::new();
        if show_metadata {
            headers.push("time");
            headers.push("port");
            headers.push("direction");
        }
        if show_hex {
            headers.push("hex");
        } else {
            headers.push("text");
        }

        let mut out = headers.join(",");
        out.push('\n');

        for row in rows {
            let mut cells: Vec<String> = Vec::new();
            if show_metadata {
                cells.push(csv_cell(&row.timestamp_label));
                cells.push(csv_cell(row.port.as_deref().unwrap_or("")));
                cells.push(csv_cell(match row.direction {
                    Direction::Rx => "RX",
                    Direction::Tx => "TX",
                    Direction::Internal => "INTERNAL",
                }));
            }
            if show_hex {
                cells.push(csv_cell(&row.hex_text));
            } else {
                cells.push(csv_cell(&row.raw_text));
            }
            out.push_str(&cells.join(","));
            out.push('\n');
        }
        out
    }

    pub fn export_visible_jsonl(&self) -> String {
        let show_hex = self.show_hex;
        let rows = self.collect_visible_rows(self.show_lines);
        let show_metadata = !self.show_lines;

        let mut out = String::new();
        for row in rows {
            let mut obj = serde_json::Map::new();
            if show_metadata {
                obj.insert(
                    "time".into(),
                    serde_json::Value::String(row.timestamp_label.to_string()),
                );
                if let Some(port) = row.port.as_deref() {
                    obj.insert("port".into(), serde_json::Value::String(port.to_owned()));
                }
                obj.insert(
                    "direction".into(),
                    serde_json::Value::String(match row.direction {
                        Direction::Rx => "RX".into(),
                        Direction::Tx => "TX".into(),
                        Direction::Internal => "INTERNAL".into(),
                    }),
                );
            }
            if show_hex {
                obj.insert(
                    "hex".into(),
                    serde_json::Value::String(row.hex_text.to_string()),
                );
            } else {
                obj.insert(
                    "text".into(),
                    serde_json::Value::String(row.raw_text.to_string()),
                );
            }
            out.push_str(
                &serde_json::to_string(&serde_json::Value::Object(obj))
                    .unwrap_or_else(|_| "{}".to_owned()),
            );
            out.push('\n');
        }
        out
    }

    pub fn port_names(&self) -> Vec<String> {
        self.ports.keys().cloned().collect()
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
            ui.checkbox(&mut self.show_lines, "按行显示");

            force_scroll_to_bottom |= crate::theme::auto_scroll_button(ui, &mut self.auto_scroll);

            if ui.button("清空").clicked() {
                self.clear();
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

                let mut port_rows = build_visible_rows_for_port(
                    Some(port.as_str()),
                    data.entries
                        .iter()
                        .filter(|entry| entry_visible(entry.direction, self.show_rx, self.show_tx)),
                    self.show_lines,
                );
                if !search_lower.is_empty() {
                    port_rows.retain(|row| row_matches_search(&port_lower, row, &search_lower));
                }
                rows.extend(port_rows);
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
            rows.sort_by_key(|row| row.event_id);

            let scroll_height = ui.available_height().max(40.0);
            let show_metadata = !self.show_lines;
            render_rows_view(
                ui,
                &scroll_key,
                scroll_height,
                &rows,
                self.show_hex,
                self.show_raw,
                show_metadata,
                show_metadata,
                show_metadata,
                self.auto_scroll,
                force_scroll_to_bottom,
                self.font_size,
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

        // 独立预览文本（用于 HEX 模式下的独立列显示）
        let preview_text = if hex_text.is_empty() {
            String::new()
        } else {
            utf8_preview
        };

        let data = self.ports.entry(port).or_default();

        // ── 合并逻辑：同端口、同方向、10ms 内、都不含 \n → 合并追加上一条 ──
        if let Some(prev) = data.entries.back_mut()
            && prev.direction == event.direction
            && event.timestamp_ms.saturating_sub(prev.timestamp_ms) <= self.merge_window_ms
            && !prev.raw_text.contains('\n')
            && !raw_text.contains('\n')
        {
            prev.raw_text.push_str(&raw_text);
            prev.display_text.push_str(&display_text);
            prev.hex_text.push(' ');
            prev.hex_text.push_str(&hex_text);
            if !preview_text.is_empty() {
                prev.preview_text.push(' ');
                prev.preview_text.push_str(&preview_text);
            }
            prev.hex_preview = if prev.hex_text.is_empty() {
                String::new()
            } else if prev.preview_text.is_empty() {
                prev.hex_text.clone()
            } else {
                format!("{} [{}]", prev.hex_text, prev.preview_text)
            };
            return;
        }

        let entry_id = self.next_entry_id;
        self.next_entry_id = self.next_entry_id.wrapping_add(1).max(1);

        data.entries.push_back(TerminalEntry {
            id: entry_id,
            event_id: event.id,
            timestamp_ms: event.timestamp_ms,

            timestamp_label: format!("[{}]", fmt_ts(event.timestamp_ms)),
            direction: event.direction,

            raw_text,
            display_text,

            hex_text,
            hex_preview,
            preview_text,
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

struct LineAccumulator {
    id: u64,
    event_id: u64,
    timestamp_label: String,
    direction: Direction,
    raw_text: String,
}

fn build_visible_rows_for_port<'a>(
    port: Option<&'a str>,
    entries: impl IntoIterator<Item = &'a TerminalEntry>,
    line_mode: bool,
) -> Vec<VisibleRow<'a>> {
    if !line_mode {
        return entries
            .into_iter()
            .map(|entry| VisibleRow::from_entry(port, entry))
            .collect();
    }

    let mut rows = Vec::new();
    let mut acc: Option<LineAccumulator> = None;
    let mut line_seq = 0_u64;
    let owned_port = port.map(str::to_owned);

    for entry in entries {
        if acc
            .as_ref()
            .is_some_and(|current| current.direction != entry.direction)
        {
            flush_line_accumulator(&mut rows, &mut acc, &owned_port, &mut line_seq);
        }

        if acc.is_none() {
            acc = Some(LineAccumulator {
                id: entry.id,
                event_id: entry.event_id,
                timestamp_label: entry.timestamp_label.clone(),
                direction: entry.direction,
                raw_text: String::new(),
            });
        }

        let Some(current_acc) = acc.as_mut() else {
            // acc was just set to Some above; this shouldn't happen.
            // Log and skip this entry rather than panicking.
            eprintln!("[tool-panels] WARNING: line accumulator unexpectedly None, skipping entry");
            continue;
        };
        append_entry_to_line_rows(
            &mut rows,
            current_acc,
            &entry.raw_text,
            &owned_port,
            &mut line_seq,
        );
    }

    flush_line_accumulator(&mut rows, &mut acc, &owned_port, &mut line_seq);
    rows
}

fn append_entry_to_line_rows<'a>(
    rows: &mut Vec<VisibleRow<'a>>,
    acc: &mut LineAccumulator,
    mut text: &str,
    port: &Option<String>,
    line_seq: &mut u64,
) {
    while let Some(pos) = text.find('\n') {
        acc.raw_text.push_str(&text[..pos]);
        emit_line_row(rows, acc, port, line_seq);
        acc.raw_text.clear();
        text = &text[pos + 1..];
    }
    acc.raw_text.push_str(text);
}

fn flush_line_accumulator<'a>(
    rows: &mut Vec<VisibleRow<'a>>,
    acc: &mut Option<LineAccumulator>,
    port: &Option<String>,
    line_seq: &mut u64,
) {
    let Some(current) = acc.take() else {
        return;
    };
    if current.raw_text.is_empty() {
        return;
    }
    emit_line_row(rows, &current, port, line_seq);
}

fn emit_line_row<'a>(
    rows: &mut Vec<VisibleRow<'a>>,
    acc: &LineAccumulator,
    port: &Option<String>,
    line_seq: &mut u64,
) {
    let raw_text = acc.raw_text.trim_end_matches('\r').to_owned();
    let bytes = raw_text.as_bytes();
    let display_text = format_terminal_text(&raw_text);
    let hex_text = format_hex(bytes);
    let preview_text = format_utf8_preview(bytes);
    let id = acc
        .id
        .wrapping_mul(1_000_003)
        .wrapping_add(*line_seq)
        .max(1);
    *line_seq = line_seq.wrapping_add(1);

    rows.push(VisibleRow {
        id,
        event_id: acc.event_id,
        port: port.clone().map(Cow::Owned),
        timestamp_label: Cow::Owned(acc.timestamp_label.clone()),
        direction: acc.direction,
        raw_text: Cow::Owned(raw_text),
        display_text: Cow::Owned(display_text),
        hex_text: Cow::Owned(hex_text),
        preview_text: Cow::Owned(preview_text),
    });
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
    show_direction: bool,
    stick_to_bottom: bool,
    force_scroll_to_bottom: bool,
    font_size: f32,
) -> RenderOutcome {
    let height = height.max(40.0);
    let font_id = egui::FontId::new(font_size, egui::FontFamily::Monospace);
    let row_height = ui.fonts_mut(|f| f.row_height(&font_id));

    // 列宽随字体大小缩放（基准 13px）
    let scale = font_size / 13.0;
    let time_col_width = TIME_COL_WIDTH * scale;
    let port_col_width = PORT_COL_WIDTH * scale;
    let dir_col_width = DIR_COL_WIDTH * scale;
    let col_gap = COL_GAP * scale;
    let row_left_padding = ROW_LEFT_PADDING * scale;

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
    let mut label_width = row_left_padding;
    if show_timestamp {
        label_width += time_col_width + col_gap;
    }
    if show_port {
        label_width += port_col_width + col_gap;
    }
    if show_direction {
        label_width += dir_col_width + col_gap;
    }

    // Pre-layout all rows to determine their galley heights
    struct RowLayout {
        galley: std::sync::Arc<egui::Galley>,
        /// Height needed for this entry's content (at least row_height)
        height: f32,
        /// Preview galley for HEX mode (UTF8 text)
        preview_galley: Option<std::sync::Arc<egui::Galley>>,
    }

    const PREVIEW_COL_MIN_WIDTH: f32 = 80.0;

    let scroll_output = ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(stick_to_bottom)
        .id_salt((scroll_key, "v2"))
        .show(ui, |ui| {
            let full_width = ui.available_width();
            let text_padding = 4.0;
            let text_color = ui.style().visuals.text_color();

            // In HEX mode, split the content area: hex text | preview text
            let hex_width: f32;
            let preview_width: f32;
            if show_hex {
                // Split remaining space between hex and preview
                let content_width = (full_width - label_width).max(0.0);
                // Preview gets roughly 30% or at least PREVIEW_COL_MIN_WIDTH
                preview_width = (content_width * 0.3).max(PREVIEW_COL_MIN_WIDTH);
                hex_width = (content_width - preview_width).max(0.0);
            } else {
                hex_width = (full_width - label_width).max(0.0);
                preview_width = 0.0;
            }

            let galley_width = (hex_width - text_padding).max(0.0);
            let preview_galley_width = if show_hex {
                (preview_width - text_padding).max(0.0)
            } else {
                0.0
            };

            // Layout all rows first to compute actual heights
            let row_layouts: Vec<RowLayout> = rows
                .iter()
                .map(|row| {
                    let content = row_content_text(row, show_hex, show_raw);
                    // Raw mode: escape newlines so content stays single-line
                    let content = if show_raw {
                        content.replace('\n', "\\n")
                    } else {
                        content.to_owned()
                    };
                    let content = if content.is_empty() {
                        " ".to_owned()
                    } else {
                        content
                    };

                    let mut layout_job = egui::text::LayoutJob::simple(
                        content,
                        font_id.clone(),
                        text_color,
                        galley_width,
                    );
                    layout_job.halign = egui::Align::LEFT;
                    let galley = ui.fonts_mut(|f| f.layout_job(layout_job));

                    // Layout preview text for HEX mode
                    let preview_galley = if show_hex {
                        let preview_text = if row.preview_text.is_empty() {
                            " ".to_owned()
                        } else {
                            row.preview_text.to_string()
                        };
                        let mut layout_job = egui::text::LayoutJob::simple(
                            preview_text,
                            font_id.clone(),
                            theme::TEXT_DIMMED,
                            preview_galley_width,
                        );
                        layout_job.halign = egui::Align::LEFT;
                        Some(ui.fonts_mut(|f| f.layout_job(layout_job)))
                    } else {
                        None
                    };

                    let height = if let Some(ref pg) = preview_galley {
                        galley.size().y.max(pg.size().y).max(row_height)
                    } else {
                        galley.size().y.max(row_height)
                    };
                    RowLayout {
                        galley,
                        height,
                        preview_galley,
                    }
                })
                .collect();

            let total_height: f32 = row_layouts.iter().map(|r| r.height).sum();

            let full_rect = ui
                .allocate_exact_size(
                    egui::vec2(full_width, total_height),
                    Sense::click_and_drag(),
                )
                .0;

            // Split into: labels | hex/raw/display | preview (HEX mode only)
            let label_rect = egui::Rect::from_min_size(
                full_rect.left_top(),
                egui::vec2(label_width, total_height),
            );
            let hex_rect = egui::Rect::from_min_size(
                egui::pos2(full_rect.left() + label_width, full_rect.top()),
                egui::vec2(hex_width, total_height),
            );
            let preview_rect = if show_hex {
                Some(egui::Rect::from_min_size(
                    egui::pos2(full_rect.left() + label_width + hex_width, full_rect.top()),
                    egui::vec2(preview_width, total_height),
                ))
            } else {
                None
            };

            let label_painter = ui.painter_at(label_rect);

            // Draw rows with accumulated Y
            let mut hl = RowHighlight::new(ui, scroll_key);
            let mut current_y = label_rect.top();
            for (row, layout) in rows.iter().zip(row_layouts.iter()) {
                let entry_height = layout.height;
                let label_y = current_y + row_height * 0.5;

                // 高亮悬停行（在文字之前绘制）
                hl.paint_background(ui, full_rect, current_y, entry_height);
                hl.record_row(current_y, entry_height);

                // --- Draw left labels ---
                let mut x = label_rect.left() + row_left_padding;

                if show_timestamp {
                    label_painter.text(
                        egui::pos2(x, label_y),
                        egui::Align2::LEFT_CENTER,
                        row.timestamp_label.as_ref(),
                        font_id.clone(),
                        theme::TEXT_SECONDARY,
                    );
                    x += time_col_width + col_gap;
                }

                if show_port {
                    if let Some(port) = row.port.as_deref() {
                        label_painter.text(
                            egui::pos2(x, label_y),
                            egui::Align2::LEFT_CENTER,
                            port,
                            font_id.clone(),
                            theme::YELLOW,
                        );
                    }
                    x += port_col_width + col_gap;
                }

                if show_direction {
                    let (dir_label, dir_color) = direction_label(row.direction);
                    label_painter.text(
                        egui::pos2(x, label_y),
                        egui::Align2::LEFT_CENTER,
                        dir_label,
                        font_id.clone(),
                        dir_color,
                    );
                }

                // --- Draw selectable content text (HEX / raw / display) ---
                {
                    let galley_pos = egui::pos2(hex_rect.left() + text_padding, current_y);
                    let row_text_rect = egui::Rect::from_min_size(
                        egui::pos2(hex_rect.left(), current_y),
                        egui::vec2(hex_width, entry_height),
                    );
                    // Use a separate id salt for hex column to avoid id collision with preview
                    let row_id = ui.make_persistent_id(("hex", row.id));
                    let response = ui.interact(row_text_rect, row_id, Sense::click_and_drag());

                    LabelSelectionState::label_text_selection(
                        ui,
                        &response,
                        galley_pos,
                        layout.galley.clone(),
                        text_color,
                        Stroke::NONE,
                    );
                }

                // --- Draw preview text (HEX mode only) ---
                if let (Some(pr), Some(pg)) = (&preview_rect, &layout.preview_galley) {
                    let preview_pos = egui::pos2(pr.left() + text_padding, current_y);
                    let preview_painter = ui.painter_at(*pr);
                    preview_painter.add(egui::epaint::TextShape::new(
                        preview_pos,
                        pg.clone(),
                        theme::TEXT_DIMMED,
                    ));
                }

                current_y += entry_height;
            }

            // Right-click context menu — global area
            let ctx_response = hl.click_response(ui);
            let frozen_row_idx = hl.resolve_click(
                ui,
                &ctx_response,
                ui.make_persistent_id(("term-frozen-row", scroll_key)),
            );
            let hovered_row: Option<(String, String)> = if ctx_response.context_menu_opened()
                || ctx_response.clicked_by(egui::PointerButton::Secondary)
            {
                frozen_row_idx
            } else {
                hl.hover_index(ui)
            }
            .and_then(|idx| {
                rows.get(idx).map(|row| {
                    let content = row_content_text(row, show_hex, show_raw);
                    let content_only = if show_raw {
                        content.replace('\n', "\\n")
                    } else {
                        content.to_owned()
                    };
                    let port = row.port.as_deref().unwrap_or("");
                    let (dir_label, _) = direction_label(row.direction);
                    let full_line = format!(
                        "{} {} {} {}",
                        row.timestamp_label, port, dir_label, content_only
                    );
                    (full_line, content_only)
                })
            });

            let combined_text: String = rows
                .iter()
                .map(|row| {
                    let content = row_content_text(row, show_hex, show_raw);
                    if show_raw {
                        content.replace('\n', "\\n")
                    } else {
                        content.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");

            ctx_response.context_menu(move |ctx_ui| {
                if let Some((ref row_full, ref row_content)) = hovered_row {
                    if ctx_ui.button("复制此行").clicked() {
                        ctx_ui.ctx().copy_text(row_full.clone());
                        ctx_ui.close();
                    }
                    if ctx_ui.button("复制此行数据").clicked() {
                        ctx_ui.ctx().copy_text(row_content.clone());
                        ctx_ui.close();
                    }
                    ctx_ui.separator();
                }

                if ctx_ui.button("复制全部可见内容").clicked() {
                    ctx_ui.ctx().copy_text(combined_text.clone());
                    ctx_ui.close();
                }

                if ctx_ui.button("复制 CSV").clicked() {
                    let csv = build_csv(
                        rows,
                        show_hex,
                        show_raw,
                        show_timestamp || show_port || show_direction,
                    );
                    ctx_ui.ctx().copy_text(csv);
                    ctx_ui.close();
                }

                if ctx_ui.button("复制 JSONL").clicked() {
                    let jsonl = build_jsonl(
                        rows,
                        show_hex,
                        show_raw,
                        show_timestamp,
                        show_port,
                        show_direction,
                    );
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

    RenderOutcome {
        inner_rect: scroll_output.inner_rect,
        content_height: scroll_output.content_size.y,
        offset_y: scroll_output.state.offset.y,
    }
}

/// Build CSV string from visible rows.
fn build_csv(
    rows: &[VisibleRow<'_>],
    show_hex: bool,
    show_raw: bool,
    show_metadata: bool,
) -> String {
    let mut out = if show_metadata {
        "time,port,direction,content\n".to_owned()
    } else {
        "content\n".to_owned()
    };
    for row in rows {
        let content = row_content_text(row, show_hex, show_raw);
        let port = row.port.as_deref().unwrap_or("");
        if show_metadata {
            out.push_str(&format!(
                "{},{},{},{}\n",
                csv_cell(&row.timestamp_label),
                csv_cell(port),
                match row.direction {
                    Direction::Rx => "RX",
                    Direction::Tx => "TX",
                    Direction::Internal => "IN",
                },
                csv_cell(&content.replace('\n', " ")),
            ));
        } else {
            out.push_str(&csv_cell(&content.replace('\n', " ")));
            out.push('\n');
        }
    }
    out
}

/// Build JSONL string from visible rows.
fn build_jsonl(
    rows: &[VisibleRow<'_>],
    show_hex: bool,
    _show_raw: bool,
    show_timestamp: bool,
    show_port: bool,
    show_direction: bool,
) -> String {
    let mut out = String::new();
    for row in rows {
        let mut obj = serde_json::Map::new();
        if show_timestamp {
            obj.insert(
                "time".into(),
                serde_json::Value::String(row.timestamp_label.to_string()),
            );
        }
        if show_port && let Some(port) = row.port.as_deref() {
            obj.insert("port".into(), serde_json::Value::String(port.to_owned()));
        }
        if show_direction {
            obj.insert(
                "direction".into(),
                serde_json::Value::String(match row.direction {
                    Direction::Rx => "RX".into(),
                    Direction::Tx => "TX".into(),
                    Direction::Internal => "INTERNAL".into(),
                }),
            );
        }
        if show_hex {
            obj.insert(
                "hex".into(),
                serde_json::Value::String(row.hex_text.to_string()),
            );
        } else {
            obj.insert(
                "text".into(),
                serde_json::Value::String(row.raw_text.to_string()),
            );
        }
        out.push_str(
            &serde_json::to_string(&serde_json::Value::Object(obj))
                .unwrap_or_else(|_| "{}".to_owned()),
        );
        out.push('\n');
    }
    out
}

/// Returns the content text for a row based on display priority: hex > raw > display.
fn row_content_text<'a>(row: &'a VisibleRow<'a>, show_hex: bool, show_raw: bool) -> &'a str {
    if show_hex {
        row.hex_text.as_ref()
    } else if show_raw {
        row.raw_text.as_ref()
    } else {
        row.display_text.as_ref()
    }
}

fn entry_visible(direction: Direction, show_rx: bool, show_tx: bool) -> bool {
    match direction {
        Direction::Rx => show_rx,
        Direction::Tx => show_tx,
        Direction::Internal => false,
    }
}

fn row_matches_search(port_lower: &str, row: &VisibleRow<'_>, search_lower: &str) -> bool {
    if search_lower.is_empty() {
        return true;
    }
    port_lower.contains(search_lower)
        || row.raw_text.to_ascii_lowercase().contains(search_lower)
        || row.display_text.to_ascii_lowercase().contains(search_lower)
        || row.hex_text.to_ascii_lowercase().contains(search_lower)
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
        // write! to String is infallible (fmt::Write for String never returns Err)
        write!(s, "{byte:02X}").expect("write to String should be infallible");
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

    #[test]
    fn clear_drains_pending_serial_events() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        bus.publish(
            Event::new(
                serial_topics::SERIAL_RX,
                "serial:COM1",
                Direction::Rx,
                Payload::Bytes(b"stale".to_vec()),
            )
            .with_metadata(serde_json::json!({ "port": "COM1" })),
        );

        panel.clear();

        assert_eq!(panel.ingest_all_pending(), 0);
        assert!(panel.ports.is_empty());
    }

    #[test]
    fn line_mode_merges_rx_chunks_until_newline() {
        let first = TerminalEntry {
            id: 1,
            event_id: 1,
            timestamp_ms: 0,
            timestamp_label: "[12:00:00.000]".to_owned(),
            direction: Direction::Rx,
            raw_text: "(42".to_owned(),
            display_text: "(42".to_owned(),
            hex_text: format_hex(b"(42"),
            hex_preview: String::new(),
            preview_text: "(42".to_owned(),
        };
        let second = TerminalEntry {
            id: 2,
            event_id: 2,
            timestamp_ms: 1,
            timestamp_label: "[12:00:00.001]".to_owned(),
            direction: Direction::Rx,
            raw_text: ".0000)ok*29\n".to_owned(),
            display_text: ".0000)ok*29\n".to_owned(),
            hex_text: format_hex(b".0000)ok*29\n"),
            hex_preview: String::new(),
            preview_text: ".0000)ok*29\n".to_owned(),
        };

        let rows = build_visible_rows_for_port(Some("COM6"), [&first, &second], true);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].port.as_deref(), Some("COM6"));
        assert_eq!(rows[0].raw_text, "(42.0000)ok*29");
        assert_eq!(rows[0].display_text, "(42.0000)ok*29");
    }
}
