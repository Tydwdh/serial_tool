use crate::{
    MAX_INGEST_PER_FRAME, fmt_ts,
    table::{RowHighlight, RowSelection, edge_scroll_delta},
    theme,
};
use egui::text_selection::LabelSelectionState;
use egui::{Color32, RichText, ScrollArea, Sense, Stroke};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tool_core::{Direction, Event};
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
    auto_scroll: bool,
    /// 暂停接收：置位后 ingest 直接 drain subscription，不 push 新事件，
    /// 已显示内容冻结。用于高速数据流下停下来仔细看一段数据。
    pub paused: bool,
    /// 暂停期间被丢弃的事件计数（drain 掉的）。恢复后用于在画面顶部
    /// 显示一条 "已暂停 · 丢弃 N 条数据" 的提示，提醒用户此处有数据缺口。
    paused_dropped_count: u64,
    /// 暂停提示的剩余显示时间（秒）。恢复后置位，归零后不再绘制提示。
    paused_banner_remain: f64,

    search_text: String,
    /// 搜索是否大小写敏感（false=不敏感，默认；true=敏感）。
    search_case_sensitive: bool,
    port_filter: Option<String>,
    bookmarked_entry_ids: BTreeSet<u64>,

    pub max_entries: usize,

    pub height: f32,
    pub maximize_clicked: bool,

    /// 是否发生过截断（用于状态栏提示，显示后清除）
    pub truncated: bool,

    last_scroll_offsets: BTreeMap<String, f32>,
    pending_scroll_to_bottom_keys: BTreeSet<String>,
    /// 双击搜索匹配行时设置：下帧清除搜索并滚动到该行。
    pending_navigate_to_id: Option<u64>,

    next_entry_id: u64,
    selected_entry_id: Option<u64>,
    detail_entry_id: Option<u64>,

    /// 用户可调的字体大小（10-24px），默认 13.0
    pub font_size: f32,
    /// 合并阈值：同一端口、同一方向、间隔 ≤ 此毫秒且不含 \n 的连续事件合并显示
    pub merge_window_ms: u64,
    /// 框选状态
    pub selection: RowSelection,
    /// 缓存的上一帧实际 total_height，避免全量 layout 估算带来的累积误差。
    /// 只在 rows.len() 变化时按比例调整，resize 宽度不变时不重算。
    cached_total_height: f32,
    cached_total_height_rows: usize,
}

#[derive(Default)]
struct PortData {
    entries: VecDeque<TerminalEntry>,
    truncated_count: u64,
}

// impl Default for PortData {
//     fn default() -> Self {
//         Self {
//             entries: VecDeque::new(),
//             truncated_count: 0,
//         }
//     }
// }

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
    id: u64,
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
    /// 双击搜索匹配行时设置：该行的 entry id，供调用方清除搜索并跳转。
    pending_navigate_to_id: Option<u64>,
}

/// 将一行文本作为 entry push，或合并到上一条 entry（如果间隔 ≤ merge_window_ms
/// 且上一条不以 \n 结尾）。
fn push_entry(
    data: &mut PortData,
    text: &str,
    event: &Event,
    next_entry_id: &mut u64,
    max_entries: usize,
    on_remove: &mut dyn FnMut(u64),
    merge_window_ms: u64,
) {
    let bytes = text.as_bytes().to_vec();
    let display_text = format_terminal_text(text);
    let hex_text = format_hex(&bytes);
    let utf8_preview = format_utf8_preview(&bytes);
    let hex_preview = if hex_text.is_empty() {
        String::new()
    } else if utf8_preview.is_empty() {
        hex_text.clone()
    } else {
        format!("{hex_text} [{utf8_preview}]")
    };
    let preview_text = if hex_text.is_empty() {
        String::new()
    } else {
        utf8_preview
    };

    // ── 合并：同方向、不同 event、在阈值内、上一条不是完整行 → 直接拼接 ──
    if let Some(prev) = data.entries.back_mut()
        && prev.direction == event.direction
        && prev.event_id != event.id
        && event.timestamp_ms.saturating_sub(prev.timestamp_ms) <= merge_window_ms
        && !prev.raw_text.ends_with('\n')
    {
        prev.raw_text.push_str(text);
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

    // ── 不合并：独立 push ──
    let entry_id = *next_entry_id;
    *next_entry_id = next_entry_id.wrapping_add(1).max(1);

    data.entries.push_back(TerminalEntry {
        id: entry_id,
        event_id: event.id,
        timestamp_ms: event.timestamp_ms,
        timestamp_label: format!("[{}]", fmt_ts(event.timestamp_ms)),
        direction: event.direction,
        raw_text: text.to_owned(),
        display_text,
        hex_text,
        hex_preview,
        preview_text,
    });

    while data.entries.len() > max_entries {
        if let Some(removed) = data.entries.pop_front() {
            on_remove(removed.id);
        }
        data.truncated_count += 1;
    }
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
            auto_scroll: true,
            paused: false,
            paused_dropped_count: 0,
            paused_banner_remain: 0.0,

            search_text: String::new(),
            search_case_sensitive: false,
            port_filter: None,
            bookmarked_entry_ids: BTreeSet::new(),

            max_entries: 50_000,

            height: 350.0,
            maximize_clicked: false,
            truncated: false,

            last_scroll_offsets: BTreeMap::new(),
            pending_scroll_to_bottom_keys: BTreeSet::new(),
            pending_navigate_to_id: None,

            next_entry_id: 1,
            selected_entry_id: None,
            detail_entry_id: None,
            font_size: 13.0,
            merge_window_ms: 5,
            selection: RowSelection::new(0),
            cached_total_height: 0.0,
            cached_total_height_rows: 0,
        }
    }
    pub fn ingest_all_pending(&mut self) -> usize {
        // 每帧最多摄入 5000 条，防止大量数据突发时 UI 卡顿
        const MAX_INGEST_ALL: usize = 5000;
        let mut count = 0;

        // 暂停接收：drain subscription 避免积压，但不 push 新事件，视图冻结。
        if self.paused {
            while self.subscription.try_recv().is_some() {
                self.paused_dropped_count = self.paused_dropped_count.saturating_add(1);
            }
            return 0;
        }

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
        self.show_raw = false;
        self.auto_scroll = true;
        self.selection.clear();
        self.cached_total_height = 0.0;
        self.cached_total_height_rows = 0;
    }

    pub fn is_bookmarked(&self, entry_id: u64) -> bool {
        self.bookmarked_entry_ids.contains(&entry_id)
    }

    pub fn toggle_bookmark(&mut self, entry_id: u64) {
        if !self.bookmarked_entry_ids.insert(entry_id) {
            self.bookmarked_entry_ids.remove(&entry_id);
        }
    }

    /// 返回用于匹配的搜索词：大小写敏感时保留原样，否则转小写。
    fn search_query(&self) -> String {
        let trimmed = self.search_text.trim();
        if self.search_case_sensitive {
            trimmed.to_owned()
        } else {
            trimmed.to_ascii_lowercase()
        }
    }

    fn collect_visible_rows(&self) -> Vec<VisibleRow<'_>> {
        let search_key = self.search_query();
        let mut rows = Vec::new();
        for (port, data) in &self.ports {
            if let Some(ref filter) = self.port_filter
                && filter != port
            {
                continue;
            }

            let port_key = if self.search_case_sensitive {
                port.clone()
            } else {
                port.to_ascii_lowercase()
            };
            let mut port_rows = build_visible_rows_for_port(
                Some(port.as_str()),
                data.entries
                    .iter()
                    .filter(|entry| entry_visible(entry.direction, self.show_rx, self.show_tx)),
            );
            if !search_key.is_empty() {
                port_rows.retain(|row| {
                    row_matches_search(
                        row,
                        &search_key,
                        self.search_case_sensitive,
                        self.show_hex,
                        self.show_raw,
                    )
                });
            }
            rows.extend(port_rows);
        }

        rows.sort_by_key(|row| row.event_id);
        rows
    }

    pub fn export_visible_csv(&self) -> String {
        let show_hex = self.show_hex;
        let rows = self.collect_visible_rows();
        let show_metadata = true;

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
        let rows = self.collect_visible_rows();
        let show_metadata = true;

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
        let mut force_scroll_to_bottom = self.pending_scroll_to_bottom_keys.remove(&scroll_key);

        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.show_rx, "RX");
            ui.checkbox(&mut self.show_tx, "TX");
            ui.checkbox(&mut self.show_hex, "HEX");
            ui.checkbox(&mut self.show_raw, "原始");

            force_scroll_to_bottom |= crate::theme::auto_scroll_button(ui, &mut self.auto_scroll);

            // 暂停接收：用文字按钮而非 ⏸，避免与自动滚动按钮（⏸/↓）视觉撞图。
            // 语义不同：暂停接收会冻结已显示内容并丢弃新数据，自动滚动只控制滚到底。
            let (pause_label, pause_hint) = if self.paused {
                ("继续", "已暂停接收 · 新数据被丢弃，点击继续")
            } else {
                ("暂停", "暂停接收 · 冻结画面查看")
            };
            if ui
                .add(egui::Button::new(pause_label).selected(self.paused))
                .on_hover_text(pause_hint)
                .clicked()
            {
                self.paused = !self.paused;
                // 恢复接收时，若期间丢弃过数据，在画面顶部留一条提示 5 秒，
                // 让用户知道此处有数据缺口（高速流下恢复后容易看不出暂停过）。
                if !self.paused && self.paused_dropped_count > 0 {
                    self.paused_banner_remain = 5.0;
                }
            }

            // 清空：两步确认，避免误触丢失刚出现的故障数据。
            // 「清空」首次点击 → 变红「确认清空?」→ 再次点击才真正清空；
            // 3 秒内未点则自动解除武装。
            let clear_id = ui.id().with("clear_armed_ts");
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
            if armed {
                // 解除武装的可点击提示（点此取消）
                if ui.small_button("取消").clicked() {
                    ui.ctx().memory_mut(|m| m.data.remove_temp::<f64>(clear_id));
                }
            }

            if ui.button("⛶").on_hover_text("放大查看").clicked() {
                self.maximize_clicked = true;
            }
        });

        force_scroll_to_bottom |= self.auto_scroll && wheel_moves_towards_bottom;

        ui.horizontal(|ui| {
            ui.label("搜索");
            let search_resp = ui.add(
                egui::TextEdit::singleline(&mut self.search_text)
                    .desired_width(140.0)
                    .hint_text("文本 / HEX"),
            );
            // Escape 清空搜索（聚焦时），与 VSCode/Chrome 查找栏行为一致
            if search_resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.search_text.clear();
                search_resp.surrender_focus();
            }
            // 大小写敏感切换：选中时 "Aa" 高亮，匹配区分大小写。
            let case_btn = egui::Button::new("Aa")
                .selected(self.search_case_sensitive)
                .small();
            if ui
                .add(case_btn)
                .on_hover_text("区分大小写（HEX 为大写，默认不区分）")
                .clicked()
            {
                self.search_case_sensitive = !self.search_case_sensitive;
            }

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

        // 暂停提示：暂停中实时显示已丢弃条数；恢复后保留提示数秒再消失。
        if self.paused {
            // 暂停中：每帧都刷（数据仍在 drain），请求重绘以保持计数新鲜。
            ui.ctx().request_repaint();
            let dropped = self.paused_dropped_count;
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(8, 3))
                .fill(crate::theme::YELLOW_BG)
                .stroke(egui::Stroke::new(1.0, crate::theme::YELLOW))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "已暂停接收 · 丢弃 {dropped} 条数据（点击「继续」恢复）"
                        ))
                        .color(crate::theme::YELLOW),
                    );
                });
        } else if self.paused_banner_remain > 0.0 {
            // 恢复后：保留提示数秒，按 dt 递减。
            let dt = ui.input(|i| i.unstable_dt) as f64;
            self.paused_banner_remain = (self.paused_banner_remain - dt).max(0.0);
            let dropped = self.paused_dropped_count;
            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(8, 3))
                .fill(crate::theme::YELLOW_BG)
                .stroke(egui::Stroke::new(1.0, crate::theme::YELLOW))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "已恢复接收 · 暂停期间丢弃了 {dropped} 条数据"
                        ))
                        .color(crate::theme::YELLOW),
                    );
                });
        } else if self.paused_dropped_count > 0 {
            // 提示期结束且未再次暂停：清零计数，下一次暂停重新累计。
            self.paused_dropped_count = 0;
        }

        // 跳转到目标 entry 的 row 索引（用于"搜索时双击 → 跳转上下文"）
        let mut scroll_to_row: Option<usize> = None;

        // 双击搜索结果 → 下一帧清除搜索、关闭自动追踪、显示全部、跳转到对应行
        if self.pending_navigate_to_id.is_some() && !self.search_text.trim().is_empty() {
            self.search_text.clear();
            self.port_filter = None;
            self.auto_scroll = false;
        }

        let render_outcome = {
            // 预计算搜索查询：大小写敏感时保留原样，否则转小写（避免渲染循环中重复分配）。
            let search_key = self.search_query();

            let mut rows: Vec<VisibleRow<'_>> = Vec::new();

            for (port, data) in &self.ports {
                if let Some(filter_port) = &self.port_filter
                    && filter_port != port
                {
                    continue;
                }
                let port_key = if self.search_case_sensitive {
                    port.clone()
                } else {
                    port.to_ascii_lowercase()
                };

                let mut port_rows = build_visible_rows_for_port(
                    Some(port.as_str()),
                    data.entries
                        .iter()
                        .filter(|entry| entry_visible(entry.direction, self.show_rx, self.show_tx)),
                );
                if !search_key.is_empty() {
                    port_rows.retain(|row| {
                        row_matches_search(
                            row,
                            &search_key,
                            self.search_case_sensitive,
                            self.show_hex,
                            self.show_raw,
                        )
                    });
                }
                rows.extend(port_rows);
            }

            // 关键修复：
            // 全局视图按 DataBus 发布顺序显示，不按端口名分组，也不按毫秒时间排序。
            //
            // timestamp_ms 在高频串口下会大量相同；
            // BTreeMap 遍历又会按 COM 名排序；
            // 所以只按 timestamp_ms 或 (timestamp_ms, local_id) 都可能看起来像 COM 分组。
            rows.sort_by_key(|row| row.event_id);

            // 获取下帧跳转目标的 row 索引（现在是完整列表）
            if let Some(target_id) = self.pending_navigate_to_id.take() {
                scroll_to_row = rows.iter().position(|r| r.id == target_id);
            }

            let scroll_height = ui.available_height().max(40.0);
            let show_metadata = true;
            // 空状态引导：从未收到任何数据 vs 有数据但被筛选/搜索过滤光。
            let empty_hint = if self.ports.is_empty() {
                "暂无数据 · 选择并打开串口后开始接收"
            } else {
                "无匹配数据 · 试着清除筛选或搜索条件"
            };
            render_rows_view(
                ui,
                &scroll_key,
                scroll_height,
                &rows,
                scroll_to_row,
                self.show_hex,
                self.show_raw,
                show_metadata,
                show_metadata,
                show_metadata,
                self.auto_scroll,
                force_scroll_to_bottom,
                self.font_size,
                &mut self.selection,
                empty_hint,
                &mut self.cached_total_height,
                &mut self.cached_total_height_rows,
            )
        };

        self.apply_render_outcome(&scroll_key, render_outcome, ui);
        self.detail_popup(ui.ctx());
    }

    fn apply_render_outcome(&mut self, scroll_key: &str, outcome: RenderOutcome, ui: &egui::Ui) {
        if let Some(id) = outcome.pending_navigate_to_id {
            self.pending_navigate_to_id = Some(id);
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

        // 暂停接收：drain subscription 避免积压，但不 push 新事件，视图冻结。
        if self.paused {
            while self.subscription.try_recv().is_some() {
                self.paused_dropped_count = self.paused_dropped_count.saturating_add(1);
            }
            return 0;
        }

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

    /// 按最后一个 \n 拆分包数据：
    /// - 如果 text 以 \n 结尾 → 一整条 entry（所有 \n 保留在 raw_text）
    /// - 如果 text 不以 \n 结尾 → 最后一个 \n 之前的内容作为第一条 entry
    ///   （可被跨包合并），之后的内容作为第二条 entry（跨包合并的目标）
    fn push_event(&mut self, event: Event) {
        let port = event
            .metadata
            .get("port")
            .and_then(|value| value.as_str())
            .or_else(|| event.source.strip_prefix("serial:"))
            .unwrap_or("default")
            .to_owned();

        let data = self.ports.entry(port).or_default();
        let text = event.payload.text_lossy();
        if text.is_empty() {
            return;
        }

        // 按最后一个 \n 拆分
        if let Some(idx) = text.rfind('\n')
            && idx != text.len() - 1
        {
            // 最后一个 \n 之后有内容 → 拆成两条
            // 第一条（含 \n）：可以合并到上一条 entry
            let first = &text[..=idx]; // 包含 \n
            push_entry(
                data,
                first,
                &event,
                &mut self.next_entry_id,
                self.max_entries,
                &mut |id| {
                    if self.selected_entry_id == Some(id) {
                        self.selected_entry_id = None;
                    }
                    if self.detail_entry_id == Some(id) {
                        self.detail_entry_id = None;
                    }
                    self.bookmarked_entry_ids.remove(&id);
                    self.truncated = true;
                },
                self.merge_window_ms,
            );
            // 第二条（无 \n 结尾）：独立 push，不合并
            let second = &text[idx + 1..];
            if !second.is_empty() {
                push_entry(
                    data,
                    second,
                    &event,
                    &mut self.next_entry_id,
                    self.max_entries,
                    &mut |id| {
                        if self.selected_entry_id == Some(id) {
                            self.selected_entry_id = None;
                        }
                        if self.detail_entry_id == Some(id) {
                            self.detail_entry_id = None;
                        }
                        self.bookmarked_entry_ids.remove(&id);
                        self.truncated = true;
                    },
                    0, // 不合并：同包内拆出的未完成行
                );
            }
        } else {
            // 不含 \n 或以 \n 结尾 → 一整条 entry
            push_entry(
                data,
                &text,
                &event,
                &mut self.next_entry_id,
                self.max_entries,
                &mut |id| {
                    if self.selected_entry_id == Some(id) {
                        self.selected_entry_id = None;
                    }
                    if self.detail_entry_id == Some(id) {
                        self.detail_entry_id = None;
                    }
                    self.bookmarked_entry_ids.remove(&id);
                    self.truncated = true;
                },
                self.merge_window_ms,
            );
        }
    }

    /// 将一行文本作为 entry push，或合并到上一条 entry（如果间隔 ≤ merge_window_ms
    /// 且上一条不以 \n 结尾）。
    /// 尾巴已超时：作为独立 entry push，不强行合并到后续数据。
    fn entry_detail(&self, entry_id: u64) -> Option<EntryDetail> {
        for (port, data) in &self.ports {
            for entry in &data.entries {
                if entry.id == entry_id {
                    return Some(EntryDetail {
                        id: entry.id,
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
                    ui.label(
                        RichText::new(format!("#{} · {}B", detail.id, detail.raw_text.len()))
                            .color(theme::TEXT_DIMMED)
                            .small(),
                    );

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

        // Escape 关闭详情窗口（在 show() 闭包外处理，避免 open 的借用冲突）
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        if !open {
            self.detail_entry_id = None;
        }
    }
}

fn build_visible_rows_for_port<'a>(
    port: Option<&'a str>,
    entries: impl IntoIterator<Item = &'a TerminalEntry>,
) -> Vec<VisibleRow<'a>> {
    entries
        .into_iter()
        .map(|entry| VisibleRow::from_entry(port, entry))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn render_rows_view(
    ui: &mut egui::Ui,
    scroll_key: &str,
    height: f32,
    rows: &[VisibleRow<'_>],
    scroll_to_row: Option<usize>,
    show_hex: bool,
    show_raw: bool,
    show_timestamp: bool,
    show_port: bool,
    show_direction: bool,
    stick_to_bottom: bool,
    force_scroll_to_bottom: bool,
    font_size: f32,
    selection: &mut RowSelection,
    empty_hint: &str,
    cached_total_height: &mut f32,
    cached_total_height_rows: &mut usize,
) -> RenderOutcome {
    let height = height.max(40.0);
    let font_id = egui::FontId::new(font_size, egui::FontFamily::Monospace);
    let row_height = ui.fonts_mut(|f| f.row_height(&font_id));
    selection.sync_rows(rows.iter().map(|row| row.id));

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
                ui.label(RichText::new(empty_hint).color(theme::TEXT_SECONDARY));
            });

        return RenderOutcome {
            inner_rect: scroll_output.inner_rect,
            content_height: scroll_output.content_size.y,
            offset_y: scroll_output.state.offset.y,
            pending_navigate_to_id: None,
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

    const PREVIEW_COL_MIN_WIDTH: f32 = 80.0;

    let mut navigate_id: Option<u64> = None;

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

            // ═══════════════════════════════════════════════════════
            // total_height 用上一帧缓存的真实高度，避免全量 layout。
            // rows.len() 变化时按比例调整（新行用 row_height 估计）。
            // resize 宽度变化时 O(rows) → O(1)，让 50000+ 条目下拖动面板不卡。
            // 视口内行在绘制循环中按需懒 layout，总 layout 量 ≈ visible_rows。
            // ═══════════════════════════════════════════════════════
            let total_height =
                if rows.len() != *cached_total_height_rows && *cached_total_height > 0.0 {
                    let avg_h = *cached_total_height / *cached_total_height_rows as f32;
                    avg_h * rows.len() as f32
                } else if *cached_total_height > 0.0 {
                    *cached_total_height
                } else {
                    row_height * rows.len() as f32
                };

            let (full_rect, _alloc_response) =
                ui.allocate_exact_size(egui::vec2(full_width, total_height), Sense::hover());

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
            let data_rect = egui::Rect::from_min_max(hex_rect.left_top(), full_rect.right_bottom());
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
                    ui.make_persistent_id(("terminal-blank", scroll_key)),
                    Sense::click(),
                )
            });

            let label_painter = ui.painter_at(label_rect);

            // Draw rows with accumulated Y
            let mut hl = RowHighlight::new(ui, scroll_key);

            // 用 row_height 记录所有行的估计范围，供点击/拖拽命中（不影响实际绘制高度）。
            // 大部分行是单行，row_height 与真实高度一致；少数多行条目会有微小偏移但不影响交互。
            let mut recorded_y = label_rect.top();
            for _ in rows.iter() {
                hl.record_row(recorded_y, row_height);
                recorded_y += row_height;
            }

            // 整行选择只从元数据区起手；数据区完整保留给字符级文本选择。
            let mut ctx_response = ui.interact(
                label_rect,
                ui.make_persistent_id(("terminal-metadata", scroll_key)),
                Sense::click_and_drag(),
            );
            if let Some(response) = blank_response {
                ctx_response |= response;
            }
            let hovered_idx = ui
                .input(|input| input.pointer.hover_pos().map(|pos| pos.y))
                .and_then(|y| hl.row_index_at_y_clamped(y));
            let data_pressed = ui.input(|input| {
                input.pointer.button_pressed(egui::PointerButton::Primary)
                    && input
                        .pointer
                        .hover_pos()
                        .is_some_and(|pos| data_rect.contains(pos))
            }) && ui.rect_contains_pointer(data_rect);
            let blank_pressed = blank_rect.is_some_and(|rect| {
                ui.input(|input| {
                    input.pointer.button_pressed(egui::PointerButton::Primary)
                        && input
                            .pointer
                            .hover_pos()
                            .is_some_and(|pos| rect.contains(pos))
                }) && ui.rect_contains_pointer(rect)
            });
            if data_pressed || blank_pressed {
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
            // 视口剔除：视口外的行跳过 layout + paint + interact。
            // 只对视口内行做懒 layout（约 30-60 行），resize 宽度变化时不再 O(rows)。
            // 上下各留 1 行 buffer，避免边界行因浮点误差被误剔。
            let clip_top = viewport_rect.top() - row_height;
            let clip_bottom = viewport_rect.bottom() + row_height;
            for (row_idx, row) in rows.iter().enumerate() {
                // 先用 row_height 做视口判断；视口内才懒 layout 得到真实高度。
                let in_viewport = current_y + row_height >= clip_top && current_y <= clip_bottom;

                let (galley, preview_galley, entry_height) = if in_viewport {
                    let content = visible_row_content(row, show_hex, show_raw);
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
                    (Some(galley), preview_galley, height)
                } else {
                    (None, None, row_height)
                };
                let galley = galley;

                // 视口外只累加 Y，跳过所有 paint/interact/label 绘制
                if !in_viewport {
                    current_y += entry_height;
                    continue;
                }

                let label_y = current_y + row_height * 0.5;

                // 高亮悬停行（框选模式下跳过）
                let has_selection = selection.has_selection();
                if !has_selection {
                    hl.paint_background(ui, full_rect, current_y, entry_height);
                }

                // 框选高亮（使用 WIDGET_HOVER 颜色，与 hover 一致）
                if selection.is_selected(row_idx) {
                    selection.paint(ui, full_rect, current_y, entry_height);
                }

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
                if let Some(ref galley) = galley {
                    let galley_pos = egui::pos2(hex_rect.left() + text_padding, current_y);
                    let row_text_rect = egui::Rect::from_min_size(
                        egui::pos2(hex_rect.left(), current_y),
                        egui::vec2(hex_width, entry_height),
                    );
                    // Use a separate id salt for hex column to avoid id collision with preview
                    let row_id = ui.make_persistent_id(("hex", row.id));
                    let response = ui.interact(row_text_rect, row_id, Sense::click_and_drag());
                    text_drag_response = Some(match text_drag_response.take() {
                        Some(accumulated) => accumulated | response.clone(),
                        None => response.clone(),
                    });
                    ctx_response |= response.clone();

                    if selection.is_dragging() {
                        ui.painter().add(egui::epaint::TextShape::new(
                            galley_pos,
                            galley.clone(),
                            text_color,
                        ));
                    } else {
                        LabelSelectionState::label_text_selection(
                            ui,
                            &response,
                            galley_pos,
                            galley.clone(),
                            text_color,
                            Stroke::NONE,
                        );
                    }
                }

                // --- Draw preview text (HEX mode only) ---
                if let (Some(pr), Some(pg)) = (preview_rect, &preview_galley) {
                    let preview_pos = egui::pos2(pr.left() + text_padding, current_y);
                    let preview_painter = ui.painter_at(pr);
                    preview_painter.add(egui::epaint::TextShape::new(
                        preview_pos,
                        pg.clone(),
                        theme::TEXT_DIMMED,
                    ));
                }

                current_y += entry_height;
            }

            // 缓存本帧实际 total_height，供下帧使用（修复 row_height 估计累积误差）。
            // round() 消除浮点微扰——避免每帧 re-layout 导致的 sub-px 差异在
            // total_height 估计值/实际值之间来回摆动，引起 stick_to_bottom 画面抖动。
            *cached_total_height = (current_y - label_rect.top()).round();
            *cached_total_height_rows = rows.len();

            // 如果实际绘制高度超出 pre-allocated rect（例如一条超长数据跨越
            // 多行时 row_height 估计严重偏低），补充分配差额，让 ScrollArea 拿到
            // 正确的 content_size，避免内容被 clip 截断 / stick_to_bottom 错位。
            let actual_total = current_y - label_rect.top();
            if actual_total > total_height + 0.5 {
                ui.allocate_space(egui::vec2(0.0, actual_total - total_height));
            }

            if text_drag_response
                .as_ref()
                .is_some_and(|response| response.dragged_by(egui::PointerButton::Primary))
                && let Some(pointer_y) =
                    ui.input(|input| input.pointer.hover_pos().map(|pos| pos.y))
            {
                scroll_delta += edge_scroll_delta(pointer_y, viewport_rect.intersect(data_rect));
            }

            // 边缘滚动
            if scroll_delta != 0.0 {
                ui.scroll_with_delta(egui::vec2(0.0, scroll_delta));
                ui.ctx().request_repaint();
            }

            let frozen_row_idx = hl.resolve_click(
                ui,
                &ctx_response,
                ui.make_persistent_id(("term-frozen-row", scroll_key)),
            );
            // 双击搜索结果 → 离开搜索进入上下文：设置导航目标让下帧跳转
            let double_clicked = ctx_response.double_clicked();
            let mut pending_navigate: Option<u64> = None;
            if double_clicked {
                if let Some(idx) = frozen_row_idx.or_else(|| hl.hover_index(ui))
                    && let Some(row) = rows.get(idx)
                {
                    pending_navigate = Some(row.id);
                }
            }
            // 捕获到外层变量
            if pending_navigate.is_some() {
                navigate_id = pending_navigate;
            }

            // 跳转到目标行（搜索时双击 → 离开搜索进入上下文）
            if let Some(target_row) = scroll_to_row
                && let Some((y_top, _y_bottom)) = hl.row_y_range(target_row)
            {
                let target_rect =
                    egui::Rect::from_min_size(egui::pos2(0.0, y_top), egui::vec2(1.0, 1.0));
                ui.scroll_to_rect(target_rect, Some(egui::Align::Center));
            }
            let hovered_row: Option<(String, String)> = if ctx_response.context_menu_opened()
                || ctx_response.clicked_by(egui::PointerButton::Secondary)
            {
                frozen_row_idx
            } else {
                hl.hover_index(ui)
            }
            .and_then(|idx| {
                rows.get(idx).map(|row| {
                    let content_only = visible_row_content(row, show_hex, show_raw);
                    let port = row.port.as_deref().unwrap_or("");
                    let (dir_label, _) = direction_label(row.direction);
                    let full_line = format!(
                        "{} {} {} {}",
                        row.timestamp_label, port, dir_label, content_only
                    );
                    (full_line, content_only)
                })
            });

            // 框选范围文本（移入 context_menu 闭包内按需构造，避免菜单未打开时每帧构造）
            let selected_indices: Vec<usize> = selection.selected_indices().collect();

            // Ctrl+C 复制选中行：终端有选中、收到 Event::Copy、且无 TextEdit 聚焦时触发。
            // 复制 full（含时间戳/端口/方向前缀），与右键菜单"复制选中行"一致。
            // egui 0.35 把 Ctrl+C 转成 Event::Copy 事件（而非 Event::Key{C}），
            // 用 text_edit_focused 判断 TextEdit 聚焦（egui_wants_keyboard_input 过于宽泛，
            // 任何控件聚焦都返回 true）。
            let copy_requested =
                ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
            if !selected_indices.is_empty()
                && copy_requested
                && !ui.ctx().text_edit_focused()
                && let (Some(full), _) =
                    build_selected_text(rows, &selected_indices, show_hex, show_raw)
            {
                ui.ctx().copy_text(full);
            }

            ctx_response.context_menu(move |ctx_ui| {
                // 闭包仅在菜单打开时执行，此处构造选中文本开销可接受。
                let (selected_full, selected_data) =
                    build_selected_text(rows, &selected_indices, show_hex, show_raw);

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
                    && ctx_ui.button("复制选中行数据").clicked()
                {
                    ctx_ui.ctx().copy_text(text.clone());
                    ctx_ui.close();
                }
                if copy_full.is_some() || copy_data.is_some() {
                    ctx_ui.separator();
                }

                if ctx_ui.button("复制全部可见内容").clicked() {
                    // 按需构造，避免菜单未打开时每帧 join ~2000 行。
                    let combined_text: String = rows
                        .iter()
                        .map(|row| visible_row_content(row, show_hex, show_raw))
                        .collect::<Vec<_>>()
                        .join("\n");
                    ctx_ui.ctx().copy_text(combined_text);
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
        pending_navigate_to_id: navigate_id,
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

/// 接收区每个条目已经独占显示行，因此隐藏一个末尾行结束符；内部换行仍保留。
///
/// 原始模式（show_raw）例外：用户开启原始模式正是为了看到原始字节（含末尾换行），
/// 因此不剥末尾行结束符，并把所有 `\n` 转义为字面 `\n` 以便可见。
fn visible_row_content(row: &VisibleRow<'_>, show_hex: bool, show_raw: bool) -> String {
    let content = row_content_text(row, show_hex, show_raw);
    if show_hex {
        return content.to_owned();
    }

    if show_raw {
        // 原始模式：转义所有控制字符为可见字面（\n, \r, \t 等）
        return format_raw_visible(content);
    }

    // 普通显示模式：隐藏末尾一个行结束符，内部换行保留
    let content = content
        .strip_suffix("\r\n")
        .or_else(|| content.strip_suffix('\n'))
        .or_else(|| content.strip_suffix('\r'))
        .unwrap_or(content);
    content.to_owned()
}

fn entry_visible(direction: Direction, show_rx: bool, show_tx: bool) -> bool {
    match direction {
        Direction::Rx => show_rx,
        Direction::Tx => show_tx,
        Direction::Internal => false,
    }
}

/// 构造选中行的文本：full（含时间戳/端口/方向前缀）和 data（仅内容）。
/// 供右键菜单和 Ctrl+C 复用。
fn build_selected_text<'a>(
    rows: &[VisibleRow<'a>],
    selected_indices: &[usize],
    show_hex: bool,
    show_raw: bool,
) -> (Option<String>, Option<String>) {
    if selected_indices.is_empty() {
        return (None, None);
    }
    let full: String = selected_indices
        .iter()
        .map(|&index| &rows[index])
        .map(|row| {
            let content_only = visible_row_content(row, show_hex, show_raw);
            let port = row.port.as_deref().unwrap_or("");
            let (dir_label, _) = direction_label(row.direction);
            format!(
                "{} {} {} {}",
                row.timestamp_label, port, dir_label, content_only
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let data: String = selected_indices
        .iter()
        .map(|&index| &rows[index])
        .map(|row| visible_row_content(row, show_hex, show_raw))
        .collect::<Vec<_>>()
        .join("\n");
    (Some(full), Some(data))
}

fn row_matches_search(
    row: &VisibleRow<'_>,
    search_key: &str,
    case_sensitive: bool,
    show_hex: bool,
    show_raw: bool,
) -> bool {
    if search_key.is_empty() {
        return true;
    }
    let contains = |haystack: &str| -> bool {
        if case_sensitive {
            haystack.contains(search_key)
        } else {
            haystack.to_ascii_lowercase().contains(search_key)
        }
    };
    // 根据显示模式搜索对应字段：
    // - HEX 模式：搜索 hex_text
    // - 原始模式：搜索 raw_text
    // - 文本模式：搜索 raw_text 和 display_text
    if show_hex {
        contains(row.hex_text.as_ref())
    } else if show_raw {
        contains(row.raw_text.as_ref())
    } else {
        contains(row.raw_text.as_ref()) || contains(row.display_text.as_ref())
    }
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
    use tool_core::Payload;
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
    fn paused_ingest_drains_subscription_without_pushing() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        // 暂停前置一条已有数据，验证暂停期间不新增。
        bus.publish(
            Event::new(
                serial_topics::SERIAL_RX,
                "serial:COM1",
                Direction::Rx,
                Payload::Bytes(b"first".to_vec()),
            )
            .with_metadata(serde_json::json!({ "port": "COM1" })),
        );
        assert_eq!(panel.ingest_all_pending(), 1);

        panel.paused = true;

        // 暂停期间发布两条，ingest 应返回 0 且不 push。
        bus.publish(
            Event::new(
                serial_topics::SERIAL_RX,
                "serial:COM1",
                Direction::Rx,
                Payload::Bytes(b"dropped1".to_vec()),
            )
            .with_metadata(serde_json::json!({ "port": "COM1" })),
        );
        bus.publish(
            Event::new(
                serial_topics::SERIAL_RX,
                "serial:COM1",
                Direction::Rx,
                Payload::Bytes(b"dropped2".to_vec()),
            )
            .with_metadata(serde_json::json!({ "port": "COM1" })),
        );

        assert_eq!(panel.ingest_all_pending(), 0);
        let port = panel.ports.get("COM1").expect("COM1 still present");
        assert_eq!(port.entries.len(), 1, "paused should not push new entries");
        assert_eq!(port.entries.front().unwrap().raw_text, "first");

        // 恢复后，subscription 已被 drain，旧数据不会补放（丢弃语义）。
        panel.paused = false;
        assert_eq!(panel.ingest_all_pending(), 0);
    }

    /// 新逻辑：按 \n 拆分行，同包内不合并，跨包只合并未完成行。
    /// event1 "(2.0000" 不以 \n 结尾 → 未完成行
    /// event2 "0)...\n" 以 \n 结尾 → 完整行，合并到 event1 的未完成行
    /// event3 "next" 不以 \n 结尾 → 未完成行，上条已完整故不合并
    #[test]
    fn ingest_merges_newline_terminated_tail_into_unfinished_rx_entry() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        for event in [
            Event::with_timestamp(
                1_000,
                serial_topics::SERIAL_RX,
                "serial:COM6",
                Direction::Rx,
                Payload::Bytes(b"(2.0000".to_vec()),
            ),
            Event::with_timestamp(
                1_001,
                serial_topics::SERIAL_RX,
                "serial:COM6",
                Direction::Rx,
                Payload::Bytes(b"0)echo:busy: processing*26\n".to_vec()),
            ),
            Event::with_timestamp(
                1_002,
                serial_topics::SERIAL_RX,
                "serial:COM6",
                Direction::Rx,
                Payload::Bytes(b"next".to_vec()),
            ),
        ] {
            bus.publish(event);
        }

        assert_eq!(panel.ingest_all_pending(), 3);

        let entries = &panel.ports.get("COM6").expect("COM6 should exist").entries;
        // 三条数据间隔 ≤ 5ms，全部直接拼接；
        // 第二条以 \n 结尾（保留在 raw_text），第三条 "next" 无 \n → 合并成一整行。
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].raw_text, "(2.00000)echo:busy: processing*26\n");
        assert_eq!(entries[1].raw_text, "next");
    }

    /// event1 包含内部 \n，拆为两行：第一行完整，第二行未完成。
    /// event2 以 \n 结尾，合并到 event1 的第二行。
    #[test]
    fn ingest_merges_tail_when_previous_chunk_contains_an_earlier_newline() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        for event in [
            Event::with_timestamp(
                1_000,
                serial_topics::SERIAL_RX,
                "serial:COM6",
                Direction::Rx,
                Payload::Bytes(b"(2.00000)X first home. completed.*77\n(2.00000".to_vec()),
            ),
            Event::with_timestamp(
                1_001,
                serial_topics::SERIAL_RX,
                "serial:COM6",
                Direction::Rx,
                Payload::Bytes(b")X home. timeout = 20*16\n".to_vec()),
            ),
        ] {
            bus.publish(event);
        }

        assert_eq!(panel.ingest_all_pending(), 2);

        let entries = &panel.ports.get("COM6").expect("COM6 should exist").entries;
        // event1 拆为 2 行，event2 合并到 event1 的未完成尾行
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].raw_text,
            "(2.00000)X first home. completed.*77\n"
        );
        assert_eq!(entries[1].raw_text, "(2.00000)X home. timeout = 20*16\n");
    }

    /// event1 "abc\ndef" → 拆为 "abc"(完整) + "def"(未完成)
    /// event2 "ghi\n" → "ghi"(完整)，合并到 event1 的 "def"
    #[test]
    fn ingest_carries_trailing_data_after_newline_into_next_chunk() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        for event in [
            Event::with_timestamp(
                1_000,
                serial_topics::SERIAL_RX,
                "serial:COM7",
                Direction::Rx,
                Payload::Bytes(b"abc\ndef".to_vec()),
            ),
            Event::with_timestamp(
                1_001,
                serial_topics::SERIAL_RX,
                "serial:COM7",
                Direction::Rx,
                Payload::Bytes(b"ghi\n".to_vec()),
            ),
        ] {
            bus.publish(event);
        }

        assert_eq!(panel.ingest_all_pending(), 2);

        let entries = &panel.ports.get("COM7").expect("COM7 should exist").entries;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].raw_text, "abc\n");
        assert_eq!(entries[1].raw_text, "defghi\n");
    }

    /// event1 "abc\ndef" → "abc"(完整) + "def"(未完成)
    /// event2 "ghi" 不以 \n 结尾 → "ghi"(未完成)，合并到 event1 的 "def"
    #[test]
    fn ingest_holds_unterminated_tail_until_next_chunk_arrives() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        for event in [
            Event::with_timestamp(
                1_000,
                serial_topics::SERIAL_RX,
                "serial:COM8",
                Direction::Rx,
                Payload::Bytes(b"abc\ndef".to_vec()),
            ),
            Event::with_timestamp(
                1_001,
                serial_topics::SERIAL_RX,
                "serial:COM8",
                Direction::Rx,
                Payload::Bytes(b"ghi".to_vec()),
            ),
        ] {
            bus.publish(event);
        }

        assert_eq!(panel.ingest_all_pending(), 2);
        let entries = &panel.ports.get("COM8").expect("COM8 should exist").entries;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].raw_text, "abc\n");
        assert_eq!(entries[1].raw_text, "defghi");
    }

    #[test]
    fn visible_row_hides_only_the_final_line_ending() {
        let row = VisibleRow {
            id: 1,
            event_id: 1,
            port: Some(Cow::Borrowed("COM6")),
            timestamp_label: Cow::Borrowed("[10:00:39.580]"),
            direction: Direction::Rx,
            raw_text: Cow::Borrowed("first\r\nsecond\r\n"),
            display_text: Cow::Borrowed("first\nsecond\n"),
            hex_text: Cow::Borrowed("66 69 72 73 74 0D 0A"),
            preview_text: Cow::Borrowed("first\nsecond\n"),
        };

        assert_eq!(visible_row_content(&row, false, false), "first\nsecond");
        // 原始模式：转义所有控制字符（\r, \n 等）
        assert_eq!(
            visible_row_content(&row, false, true),
            "first\\r\\nsecond\\r\\n"
        );
        assert_eq!(
            visible_row_content(&row, true, false),
            "66 69 72 73 74 0D 0A"
        );
        assert_eq!(row.raw_text, "first\r\nsecond\r\n");
    }

    #[test]
    fn entries_map_one_to_one_visible_rows() {
        // push_event 已将间隔 ≤5ms 的未完成行合并，entry 已是完整行 → 1:1 映射
        let entry = TerminalEntry {
            id: 1,
            event_id: 1,
            timestamp_ms: 0,
            timestamp_label: "[12:00:00.000]".to_owned(),
            direction: Direction::Rx,
            raw_text: "(42.0000)ok*29\n".to_owned(),
            display_text: "(42.0000)ok*29\n".to_owned(),
            hex_text: format_hex(b"(42.0000)ok*29\n"),
            hex_preview: String::new(),
            preview_text: "(42.0000)ok*29\n".to_owned(),
        };

        let rows = build_visible_rows_for_port(Some("COM6"), [&entry]);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].port.as_deref(), Some("COM6"));
        assert_eq!(rows[0].raw_text, "(42.0000)ok*29\n");
    }

    /// 发送 "111\n111\n"（以 \n 结尾）→ 一整条 entry，同包内不拆分。
    #[test]
    fn multiline_text_splits_into_independent_entries() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        bus.publish(Event::with_timestamp(
            1_000,
            serial_topics::SERIAL_TX,
            "serial:COM2",
            Direction::Tx,
            Payload::Bytes(b"111\n111\n".to_vec()),
        ));

        assert_eq!(panel.ingest_all_pending(), 1);
        let entries = &panel.ports.get("COM2").expect("COM2 should exist").entries;
        assert_eq!(
            entries.len(),
            1,
            "111\\n111\\n should produce 1 entry (trailing \\n)"
        );
        assert_eq!(entries[0].raw_text, "111\n111\n");
    }

    /// 发送 "1\n" → raw_text="1\n"（保留 \n 供原始模式转义）
    #[test]
    fn single_line_preserves_newline_in_raw_text() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        bus.publish(Event::with_timestamp(
            1_000,
            serial_topics::SERIAL_TX,
            "serial:COM2",
            Direction::Tx,
            Payload::Bytes(b"1\n".to_vec()),
        ));

        assert_eq!(panel.ingest_all_pending(), 1);
        let entries = &panel.ports.get("COM2").expect("COM2 should exist").entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].raw_text, "1\n");
    }

    /// 两次快速发送（3ms ≤ 5ms）：第一次 "111\n111\n"（以 \n 结尾）→ 1 条
    /// 第二次 "111\n111\n"（3ms）→ 合并到上一条 → 1 条
    #[test]
    fn rapid_send_merges_unfinished_lines() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        bus.publish(Event::with_timestamp(
            1_000,
            serial_topics::SERIAL_TX,
            "serial:COM2",
            Direction::Tx,
            Payload::Bytes(b"111\n111\n".to_vec()),
        ));
        bus.publish(Event::with_timestamp(
            1_003,
            serial_topics::SERIAL_TX,
            "serial:COM2",
            Direction::Tx,
            Payload::Bytes(b"111\n111\n".to_vec()),
        ));

        assert_eq!(panel.ingest_all_pending(), 2);
        let entries = &panel.ports.get("COM2").expect("COM2 should exist").entries;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].raw_text, "111\n111\n");
        assert_eq!(entries[1].raw_text, "111\n111\n");
    }

    /// 周期发送（3ms × 3次）：每次 "111\n111\n"，全部合并
    #[test]
    fn periodic_send_merges_only_first_line_per_event() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        for ts in [1_000u64, 1_003, 1_006] {
            bus.publish(Event::with_timestamp(
                ts,
                serial_topics::SERIAL_TX,
                "serial:COM2",
                Direction::Tx,
                Payload::Bytes(b"111\n111\n".to_vec()),
            ));
        }

        assert_eq!(panel.ingest_all_pending(), 3);
        let entries = &panel.ports.get("COM2").expect("COM2 should exist").entries;
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].raw_text, "111\n111\n");
        assert_eq!(entries[1].raw_text, "111\n111\n");
        assert_eq!(entries[2].raw_text, "111\n111\n");
    }

    /// 发送三行 "111\n111\n111\n"（以 \n 结尾）：
    /// 最后一个 \n 在末尾 → 没有未完成行 → 一整条 entry。
    #[test]
    fn three_line_send_all_complete() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        bus.publish(Event::with_timestamp(
            1_000,
            serial_topics::SERIAL_TX,
            "serial:COM2",
            Direction::Tx,
            Payload::Bytes(b"111\n111\n111\n".to_vec()),
        ));

        assert_eq!(panel.ingest_all_pending(), 1);
        let entries = &panel.ports.get("COM2").expect("COM2 should exist").entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].raw_text, "111\n111\n111\n");
    }

    /// 发送三行 "111\n111\n111"（末尾没有 \n）：
    /// 最后一个 \n 不在末尾 → 拆成两条 entry。
    /// entry0 包含最后一个 \n 之前的所有内容（含 \n）："111\n111\n"
    /// entry1 是最后一个 \n 之后的内容（无 \n 结尾）："111"
    #[test]
    fn three_line_send_last_incomplete() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        bus.publish(Event::with_timestamp(
            1_000,
            serial_topics::SERIAL_TX,
            "serial:COM2",
            Direction::Tx,
            Payload::Bytes(b"111\n111\n111".to_vec()),
        ));

        assert_eq!(panel.ingest_all_pending(), 1);
        let entries = &panel.ports.get("COM2").expect("COM2 should exist").entries;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].raw_text, "111\n111\n");
        assert_eq!(entries[1].raw_text, "111");
    }

    /// 搜索应该能过滤出包含关键字的 entry。
    #[test]
    fn search_finds_matching_entry() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        for (ts, data) in [(1_000u64, "1"), (2_000, "2"), (3_000, "3")] {
            bus.publish(Event::with_timestamp(
                ts,
                serial_topics::SERIAL_TX,
                "serial:COM2",
                Direction::Tx,
                Payload::Bytes(data.as_bytes().to_vec()),
            ));
        }

        assert_eq!(panel.ingest_all_pending(), 3);

        panel.search_text = "2".to_owned();
        let rows = panel.collect_visible_rows();
        assert_eq!(rows.len(), 1, "search '2' should match exactly 1 entry");
        assert_eq!(rows[0].raw_text, "2");

        panel.search_text = "3".to_owned();
        let rows = panel.collect_visible_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].raw_text, "3");
    }
}
