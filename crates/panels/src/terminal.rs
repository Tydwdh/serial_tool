use crate::{
    MAX_INGEST_PER_FRAME, MESSAGE_EVENT_BUFFER_CAPACITY,
    design::{self, ButtonKind},
    fmt_ts,
    table::{
        AutoScrollState, MessageSearch, RowHighlight, RowSelection, TextSelectionRows,
        bulk_copy_button, claim_copy_focus, copy_text_with_feedback, edge_scroll_delta,
        owns_copy_focus, report_copy_feedback, wheel_scroll_during_selection,
    },
    theme,
    virtual_list::VirtualRowIndex,
};
use egui::text_selection::LabelSelectionState;
use egui::{Color32, RichText, ScrollArea, Sense, Stroke};
use egui_material_icons::icons::{
    ICON_CANCEL, ICON_DELETE_SWEEP, ICON_DOWNLOAD, ICON_FILTER_ALT_OFF, ICON_SEARCH,
};
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use tool_application::service::terminal_store::{
    MAX_TERMINAL_BLOCK_BYTES, TerminalAssembler, TerminalItem, TerminalStore, TerminalStoreUpdate,
};
use tool_core::topics as serial_topics;
use tool_core::{Direction, Event};
use tool_databus::{DataBus, RingSubscription, TopicFilter};

const TIME_COL_WIDTH: f32 = 118.0;
const PORT_COL_WIDTH: f32 = 52.0;
const DIR_COL_WIDTH: f32 = 28.0;
const ROW_LEFT_PADDING: f32 = 4.0;
const COL_GAP: f32 = 3.0;
const PREVIEW_COL_MIN_WIDTH: f32 = 80.0;
const COPY_OWNER: &str = "terminal";

/// 终端端口列的短显示名：IPv4 地址 + 端口（如 `192.168.1.100:7125`）
/// 只显示 IP 后两段（`1.100`），避免长端口名占据接收区视野；
/// 其余端口名（COM3、虚拟端口等）原样返回。
/// 仅用于界面显示，CSV/JSONL 导出与回放仍使用完整端口名。
fn short_port_display(port: &str) -> std::borrow::Cow<'_, str> {
    let host = port.rsplit_once(':').map(|(h, _)| h).unwrap_or(port);
    let segments: Vec<&str> = host.split('.').collect();
    if segments.len() == 4
        && segments
            .iter()
            .all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
    {
        std::borrow::Cow::Owned(format!(
            "{}.{}",
            segments[segments.len() - 2],
            segments[segments.len() - 1]
        ))
    } else {
        std::borrow::Cow::Borrowed(port)
    }
}

/// 端口列的显示名：配置了别名则显示别名，否则回退到短显示（IPv4 后两段）。
/// 仅用于界面显示，CSV/JSONL 导出与回放仍使用完整端口名。
fn port_display_name<'b>(
    port: &'b str,
    port_aliases: &'b std::collections::HashMap<String, String>,
) -> std::borrow::Cow<'b, str> {
    if let Some(alias) = port_aliases.get(port)
        && !alias.trim().is_empty()
    {
        std::borrow::Cow::Borrowed(alias.as_str())
    } else {
        short_port_display(port)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalExportFormat {
    Txt,
    Csv,
    Json,
}

/// 跳转目标行高亮总时长（秒）。
const NAV_HIGHLIGHT_DURATION: f64 = 1.5;
/// 跳转目标行高亮末段淡出时长（秒）。
const NAV_FADE: f64 = 0.3;

pub struct TerminalPanel {
    subscription: RingSubscription,
    store: TerminalStore,
    assembler: TerminalAssembler,
    view_index: TerminalViewIndex,
    /// 端口别名（app 层注入，渲染端口列时优先显示）。
    port_aliases: std::collections::HashMap<String, String>,

    show_rx: bool,
    show_tx: bool,
    show_hex: bool,
    show_raw: bool,
    auto_scroll: AutoScrollState,
    /// 工具栏产生的导出请求，由 app 层打开原生文件保存框并写入文件。
    export_request: Option<TerminalExportFormat>,

    search: MessageSearch,
    port_filter: Option<String>,
    bookmarked_entry_ids: BTreeSet<u64>,

    pub max_entries: usize,

    pub height: f32,
    /// 是否发生过截断（用于状态栏提示，显示后清除）
    pub truncated: bool,

    /// 双击搜索匹配行时设置：下帧清除搜索并滚动到该行。
    pending_navigate_to_id: Option<u64>,
    /// 跳转目标行高亮：(目标行 id, 起始时间秒)。渲染时若命中且未超时画强调色并淡出。
    navigate_highlight: Option<(u64, f64)>,

    selected_entry_id: Option<u64>,
    detail_entry_id: Option<u64>,

    /// 用户可调的字体大小（10-24px），默认 13.0
    pub font_size: f32,
    /// 展示块的空闲结束阈值。它只用于结束 LiveTail，不代表协议帧边界。
    pub merge_window_ms: u64,
    /// 框选状态
    pub selection: RowSelection,
    /// 字符级拖选覆盖的行；用于在自动滚动时保活视口外的选区端点。
    text_selection_rows: TextSelectionRows,
    /// 接收消息流的虚拟行索引：只布局视口和 overscan 范围内的行。
    virtual_rows: VirtualRowIndex,
}

struct VisibleRow<'a> {
    id: u64,
    port: Option<Cow<'a, str>>,
    timestamp_label: Cow<'a, str>,
    direction: Direction,
    raw_text: Cow<'a, str>,
    display_text: Cow<'a, str>,
    hex_text: Cow<'a, str>,
    preview_text: Cow<'a, str>,
    live: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct TerminalViewFilter {
    port_filter: Option<String>,
    show_rx: bool,
    show_tx: bool,
    search_text: String,
    search_case_sensitive: bool,
    show_hex: bool,
    show_raw: bool,
}

#[derive(Default)]
struct TerminalViewIndex {
    visible_ids: Vec<u64>,
    filter: Option<TerminalViewFilter>,
    changed_ids: BTreeSet<u64>,
    removed_ids: BTreeSet<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct VisibleIdsUpdate {
    changed: bool,
    append_only: bool,
}

impl TerminalViewIndex {
    fn clear(&mut self) {
        self.visible_ids.clear();
        self.filter = None;
        self.changed_ids.clear();
        self.removed_ids.clear();
    }

    fn mark_changed(&mut self, ids: impl IntoIterator<Item = u64>) {
        self.changed_ids.extend(ids);
    }

    fn mark_removed(&mut self, ids: impl IntoIterator<Item = u64>) {
        self.removed_ids.extend(ids);
    }

    fn sync(&mut self, store: &TerminalStore, filter: TerminalViewFilter) -> VisibleIdsUpdate {
        let query =
            crate::search::SearchQuery::new(&filter.search_text, filter.search_case_sensitive);

        if self.filter.as_ref() != Some(&filter) {
            let next_visible_ids: Vec<u64> = store
                .iter()
                .filter(|item| terminal_item_matches(item, &filter, &query))
                .map(TerminalItem::id)
                .collect();
            let visible_ids_changed = self.visible_ids != next_visible_ids;
            self.visible_ids = next_visible_ids;
            self.filter = Some(filter);
            self.changed_ids.clear();
            self.removed_ids.clear();
            return VisibleIdsUpdate {
                changed: visible_ids_changed,
                append_only: false,
            };
        }

        let mut update = VisibleIdsUpdate::default();
        for id in std::mem::take(&mut self.removed_ids) {
            if let Ok(index) = self.visible_ids.binary_search(&id) {
                self.visible_ids.remove(index);
                update.changed = true;
                update.append_only = false;
            }
        }

        for id in std::mem::take(&mut self.changed_ids) {
            let should_be_visible = store
                .get(id)
                .is_some_and(|item| terminal_item_matches(item, &filter, &query));
            match self.visible_ids.binary_search(&id) {
                Ok(index) if !should_be_visible => {
                    self.visible_ids.remove(index);
                    update.changed = true;
                    update.append_only = false;
                }
                Err(index) if should_be_visible => {
                    let append_candidate = !update.changed || update.append_only;
                    self.visible_ids.insert(index, id);
                    update.changed = true;
                    update.append_only = append_candidate && index == self.visible_ids.len() - 1;
                }
                _ => {}
            }
        }
        update
    }
}

impl VisibleRow<'static> {
    fn from_item(item: &TerminalItem) -> Self {
        let raw_text = String::from_utf8_lossy(item.bytes()).into_owned();
        Self {
            id: item.id(),
            port: Some(Cow::Owned(item.port().to_owned())),
            timestamp_label: Cow::Owned(format!("[{}]", fmt_ts(item.first_timestamp_ms()))),
            direction: item.direction(),
            display_text: Cow::Owned(format_terminal_text(&raw_text)),
            hex_text: Cow::Owned(format_hex(item.bytes())),
            preview_text: Cow::Owned(format_utf8_preview(item.bytes())),
            raw_text: Cow::Owned(raw_text),
            live: item.is_live(),
        }
    }
}

fn terminal_item_matches(
    item: &TerminalItem,
    filter: &TerminalViewFilter,
    query: &crate::search::SearchQuery,
) -> bool {
    if filter
        .port_filter
        .as_deref()
        .is_some_and(|port| port != item.port())
        || !entry_visible(item.direction(), filter.show_rx, filter.show_tx)
    {
        return false;
    }

    if query.is_empty() {
        return true;
    }

    let row = VisibleRow::from_item(item);
    row_matches_search(&row, query, filter.show_hex, filter.show_raw)
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

impl TerminalPanel {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            subscription: bus.subscribe_ring_bounded(
                TopicFilter::prefix("transport.serial."),
                MESSAGE_EVENT_BUFFER_CAPACITY,
            ),
            store: TerminalStore::new(50_000),
            assembler: TerminalAssembler {
                idle_finalize_ms: 5,
                max_block_bytes: MAX_TERMINAL_BLOCK_BYTES,
            },
            view_index: TerminalViewIndex::default(),
            port_aliases: std::collections::HashMap::new(),

            show_rx: true,
            show_tx: true,
            show_hex: false,
            show_raw: false,
            auto_scroll: AutoScrollState::default(),
            export_request: None,

            search: MessageSearch::default(),
            port_filter: None,
            bookmarked_entry_ids: BTreeSet::new(),

            max_entries: 50_000,

            height: 350.0,
            truncated: false,

            pending_navigate_to_id: None,
            navigate_highlight: None,

            selected_entry_id: None,
            detail_entry_id: None,
            font_size: 13.0,
            merge_window_ms: 5,
            selection: RowSelection::new(0),
            text_selection_rows: TextSelectionRows::default(),
            virtual_rows: VirtualRowIndex::default(),
        }
    }
    pub fn ingest_all_pending(&mut self) -> usize {
        // 每帧最多摄入 5000 条，防止大量数据突发时 UI 卡顿
        const MAX_INGEST_ALL: usize = 5000;
        let mut count = 0;

        for event in self.subscription.drain_limited(MAX_INGEST_ALL) {
            if !matches!(
                event.topic.as_str(),
                serial_topics::SERIAL_RX | serial_topics::SERIAL_TX
            ) {
                continue;
            }

            self.push_event(event);
            count += 1;
        }

        if count > 0 {
            self.enforce_max_entries();
        }

        count
    }
    pub fn ingest_pending(&mut self) -> usize {
        let mut count = 0;

        for event in self.subscription.drain_limited(MAX_INGEST_PER_FRAME) {
            if !matches!(
                event.topic.as_str(),
                serial_topics::SERIAL_RX | serial_topics::SERIAL_TX
            ) {
                continue;
            }

            self.push_event(event);
            count += 1;
        }

        if count > 0 {
            self.enforce_max_entries();
        }

        count
    }

    pub fn clear(&mut self) {
        self.subscription.clear();
        self.store.clear();
        self.view_index.clear();
        self.auto_scroll.reset();
        self.selected_entry_id = None;
        self.detail_entry_id = None;
        self.search.clear();
        self.port_filter = None;
        self.bookmarked_entry_ids.clear();
        self.virtual_rows.clear();
        // 清空后重置为自动滚动，与 LogPanel::clear() 保持一致
        self.show_raw = false;
        self.selection.clear();
        self.text_selection_rows.clear();
    }

    pub fn is_bookmarked(&self, entry_id: u64) -> bool {
        self.bookmarked_entry_ids.contains(&entry_id)
    }

    pub fn take_dropped_events(&self) -> u64 {
        self.subscription.take_dropped_count()
    }

    /// 设置接收区的全局保留上限，并立即清理所有端口中最旧的条目。
    pub fn set_max_entries(&mut self, max_entries: usize) {
        self.max_entries = max_entries.max(100);
        self.enforce_max_entries();
    }

    /// 同步端口别名（app 层在别名变更后调用）。
    pub fn set_port_aliases(&mut self, aliases: &std::collections::HashMap<String, String>) {
        self.port_aliases = aliases.clone();
    }

    fn enforce_max_entries(&mut self) {
        let removed = self.store.set_max_entries(self.max_entries);
        self.view_index.mark_removed(removed.iter().copied());
        if !removed.is_empty() {
            self.truncated = true;
        }
        for id in removed {
            if self.selected_entry_id == Some(id) {
                self.selected_entry_id = None;
            }
            if self.detail_entry_id == Some(id) {
                self.detail_entry_id = None;
            }
            self.bookmarked_entry_ids.remove(&id);
        }
    }

    pub fn toggle_bookmark(&mut self, entry_id: u64) {
        if !self.bookmarked_entry_ids.insert(entry_id) {
            self.bookmarked_entry_ids.remove(&entry_id);
        }
    }

    /// 编译当前搜索词（普通词字面量 / `re:` 前缀正则）。
    fn search_query(&self) -> crate::search::SearchQuery {
        self.search.query()
    }

    fn current_view_filter(&self) -> TerminalViewFilter {
        TerminalViewFilter {
            port_filter: self.port_filter.clone(),
            show_rx: self.show_rx,
            show_tx: self.show_tx,
            search_text: self.search.text.clone(),
            search_case_sensitive: self.search.case_sensitive,
            show_hex: self.show_hex,
            show_raw: self.show_raw,
        }
    }

    #[cfg(test)]
    fn collect_visible_rows(&mut self) -> Vec<VisibleRow<'static>> {
        let filter = self.current_view_filter();
        self.view_index.sync(&self.store, filter);
        self.view_index
            .visible_ids
            .iter()
            .filter_map(|id| self.store.get(*id))
            .map(VisibleRow::from_item)
            .collect()
    }

    /// 导出不是每帧渲染路径，允许一次性扫描当前 Store，避免为了保持
    /// `&self` 导出 API 而让 ViewIndex 使用内部可变性。
    fn collect_visible_rows_unindexed(&self) -> Vec<VisibleRow<'static>> {
        let filter = self.current_view_filter();
        let search_key = self.search_query();
        self.store
            .iter()
            .filter(|item| terminal_item_matches(item, &filter, &search_key))
            .map(VisibleRow::from_item)
            .collect()
    }

    pub fn take_export_request(&mut self) -> Option<TerminalExportFormat> {
        self.export_request.take()
    }

    pub fn export_visible_csv(&self) -> String {
        let show_hex = self.show_hex;
        let show_raw = self.show_raw;
        let rows = self.collect_visible_rows_unindexed();
        let show_metadata = true;

        let mut headers: Vec<&str> = Vec::new();
        if show_metadata {
            headers.push("time");
            headers.push("port");
            headers.push("direction");
        }
        headers.push(if show_hex {
            "hex"
        } else if show_raw {
            "raw"
        } else {
            "text"
        });

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
            cells.push(csv_cell(&visible_row_content(&row, show_hex, show_raw)));
            out.push_str(&cells.join(","));
            out.push('\n');
        }
        out
    }

    pub fn export_visible_text(&self) -> String {
        let mut out = self
            .collect_visible_rows_unindexed()
            .iter()
            .map(|row| visible_row_content(row, self.show_hex, self.show_raw))
            .collect::<Vec<_>>()
            .join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        out
    }

    pub fn export_visible_json(&self) -> String {
        let show_hex = self.show_hex;
        let show_raw = self.show_raw;
        let rows = self.collect_visible_rows_unindexed();
        let show_metadata = true;

        let mut values = Vec::with_capacity(rows.len());
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
            let content_key = if show_hex {
                "hex"
            } else if show_raw {
                "raw"
            } else {
                "text"
            };
            obj.insert(
                content_key.into(),
                serde_json::Value::String(visible_row_content(&row, show_hex, show_raw)),
            );
            values.push(serde_json::Value::Object(obj));
        }
        serde_json::to_string_pretty(&values).expect("serializable terminal export values")
    }

    pub fn port_names(&self) -> Vec<String> {
        self.store.port_names()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let scroll_key = "terminal-all".to_owned();
        // 仅当指针位于本面板内时，滚轮向下才触发强制滚到底；
        // 否则全局 smooth_scroll_delta 会误捕获其它区域的滚轮事件。
        let panel_rect = ui.max_rect();
        let panel_clicked = ui.input(|input| {
            let pointer = &input.pointer;
            (pointer.button_pressed(egui::PointerButton::Primary)
                || pointer.button_pressed(egui::PointerButton::Secondary))
                && pointer
                    .hover_pos()
                    .is_some_and(|position| panel_rect.contains(position))
        });
        if panel_clicked {
            claim_copy_focus(ui, COPY_OWNER);
        }
        let pointer_inside = ui
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|pos| panel_rect.contains(pos));
        // ScrollArea 会消费并清空 smooth_scroll_delta，必须在渲染前保存。
        let scroll_delta_y = if pointer_inside {
            ui.input(|input| input.smooth_scroll_delta.y)
        } else {
            0.0
        };
        let wheel_moves_towards_bottom =
            pointer_inside && crate::scroll_delta_moves_towards_bottom(scroll_delta_y);
        let mut force_scroll_to_bottom = self.auto_scroll.take_pending(&scroll_key);

        ui.horizontal_wrapped(|ui| {
            design::segmented_toggle(ui, &mut self.show_rx, "RX", "RX");
            design::segmented_toggle(ui, &mut self.show_tx, "TX", "TX");
            design::segmented_toggle(ui, &mut self.show_hex, "HEX", "HEX");
            design::segmented_toggle(ui, &mut self.show_raw, "原始", "原始");

            force_scroll_to_bottom |= self.auto_scroll.button(ui);

            ui.menu_button(design::icon_text(ICON_DOWNLOAD, "导出"), |ui| {
                if ui.button("导出 TXT 纯文本…").clicked() {
                    self.export_request = Some(TerminalExportFormat::Txt);
                    ui.close();
                }
                if ui.button("导出 CSV…").clicked() {
                    self.export_request = Some(TerminalExportFormat::Csv);
                    ui.close();
                }
                if ui.button("导出 JSON…").clicked() {
                    self.export_request = Some(TerminalExportFormat::Json);
                    ui.close();
                }
            });

            // 清空：两步确认，避免误触丢失刚出现的故障数据。
            // 「清空」首次点击 → 变红「确认清空?」→ 再次点击才真正清空；
            // 3 秒内未点则自动解除武装。
            let clear_id = ui.id().with("clear_armed_ts");
            let now = ui.input(|i| i.time);
            let armed_ts: Option<f64> = ui.ctx().memory(|m| m.data.get_temp(clear_id));
            let armed = armed_ts.is_some_and(|t| now - t < 3.0);
            let clear_label = if armed { "确认清空?" } else { "清空" };
            let clear_kind = if armed {
                ButtonKind::Danger
            } else {
                ButtonKind::Ghost
            };
            if design::button(ui, ICON_DELETE_SWEEP, clear_label, clear_kind).clicked() {
                if armed {
                    self.clear();
                    ui.ctx().memory_mut(|m| m.data.remove_temp::<f64>(clear_id));
                } else {
                    ui.ctx().memory_mut(|m| m.data.insert_temp(clear_id, now));
                }
            }
            if armed {
                // 解除武装的可点击提示（点此取消）
                if design::button(ui, ICON_CANCEL, "取消", ButtonKind::Ghost).clicked() {
                    ui.ctx().memory_mut(|m| m.data.remove_temp::<f64>(clear_id));
                }
            }
        });

        force_scroll_to_bottom |= self.auto_scroll.enabled && wheel_moves_towards_bottom;

        ui.horizontal_wrapped(|ui| {
            ui.label(design::icon_only(
                ICON_SEARCH,
                theme::text_secondary(),
                17.0,
            ));
            self.search.toolbar(
                ui,
                140.0,
                "文本 / HEX",
                "区分大小写（HEX 为大写，默认不区分）",
            );

            ui.label("端口");
            egui::ComboBox::from_id_salt("terminal-port-filter")
                .width(100.0)
                .selected_text(self.port_filter.as_deref().unwrap_or("全部"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.port_filter, None, "全部");
                    for port in self.store.port_names() {
                        ui.selectable_value(&mut self.port_filter, Some(port.clone()), port);
                    }
                });

            if design::button(ui, ICON_FILTER_ALT_OFF, "清除筛选", ButtonKind::Ghost).clicked()
            {
                self.search.clear();
                self.port_filter = None;
            }

            let selected_count = self.selection.selected_count();
            if selected_count > 0 {
                ui.separator();
                ui.label(
                    RichText::new(format!("已选 {selected_count} 行"))
                        .color(theme::cyan())
                        .strong(),
                );
            }
        });

        ui.separator();

        // 跳转到目标 entry 的 row 索引（用于"搜索时双击 → 跳转上下文"）
        let mut scroll_to_row: Option<usize> = None;

        // 双击搜索结果 → 下一帧清除搜索、关闭自动追踪、显示全部、跳转到对应行
        if self.pending_navigate_to_id.is_some() && self.search.is_active() {
            self.search.clear();
            self.port_filter = None;
            self.auto_scroll.enabled = false;
        }

        let render_outcome = {
            // Store 已经按全局稳定 ID 排序，这里只做当前视图的筛选。
            let filter = self.current_view_filter();
            let visible_ids_update = self.view_index.sync(&self.store, filter);
            let visible_ids = &self.view_index.visible_ids;

            // 获取下帧跳转目标的 row 索引；实际行内容由视口渲染阶段按需构造。
            if let Some(target_id) = self.pending_navigate_to_id.take() {
                scroll_to_row = visible_ids.iter().position(|id| *id == target_id);
                // 跳转生效：设置目标行高亮（起始时间用 egui 时钟）。
                self.navigate_highlight = Some((target_id, ui.ctx().input(|i| i.time)));
            }

            let scroll_height = ui.available_height().max(40.0);
            let show_metadata = true;
            // 空状态引导：从未收到任何数据 vs 有数据但被筛选/搜索过滤光。
            let empty_hint = if self.store.is_empty() {
                "暂无数据 · 选择并打开串口后开始接收"
            } else {
                "无匹配数据 · 试着清除筛选或搜索条件"
            };
            render_rows_view(
                ui,
                &scroll_key,
                scroll_height,
                &self.store,
                visible_ids,
                visible_ids_update,
                scroll_to_row,
                self.show_hex,
                self.show_raw,
                show_metadata,
                show_metadata,
                show_metadata,
                self.auto_scroll.enabled,
                force_scroll_to_bottom,
                scroll_delta_y,
                self.font_size,
                &self.port_aliases,
                &mut self.selection,
                &mut self.text_selection_rows,
                empty_hint,
                &mut self.virtual_rows,
                self.navigate_highlight,
            )
        };

        self.apply_render_outcome(&scroll_key, render_outcome, scroll_delta_y, ui);
        self.detail_popup(ui.ctx());

        // 高亮超时清理
        if let Some((_, start)) = self.navigate_highlight {
            let now = ui.ctx().input(|i| i.time);
            if now - start >= NAV_HIGHLIGHT_DURATION {
                self.navigate_highlight = None;
            }
        }
    }

    fn apply_render_outcome(
        &mut self,
        scroll_key: &str,
        outcome: RenderOutcome,
        scroll_delta_y: f32,
        ui: &egui::Ui,
    ) {
        if let Some(id) = outcome.pending_navigate_to_id {
            self.pending_navigate_to_id = Some(id);
        }
        self.auto_scroll.update(
            ui,
            scroll_key,
            outcome.inner_rect,
            outcome.content_height,
            outcome.offset_y,
            scroll_delta_y,
        );
    }

    fn push_event(&mut self, event: Event) {
        let port = event
            .metadata
            .get("port")
            .and_then(|value| value.as_str())
            .or_else(|| event.source.strip_prefix("serial:"))
            .unwrap_or("default")
            .to_owned();
        let bytes = payload_bytes(&event);
        if bytes.is_empty() {
            return;
        }

        self.assembler.idle_finalize_ms = self.merge_window_ms;
        let update: TerminalStoreUpdate =
            self.store
                .ingest(self.assembler, &event, port, bytes.as_ref());
        // LiveTail 预览始终固定为单行；离屏内容变化不应重置旧行高，避免用户查看历史
        // 时因为后台接收而发生滚动位置跳动。进入视口后仍会重新 layout 并收敛真实高度。
        self.view_index.mark_changed(update.changed_ids);
        self.view_index
            .mark_removed(update.removed_ids.iter().copied());
        let removed = update.removed_ids;
        let limit_removed = self.store.set_max_entries(self.max_entries);
        self.view_index.mark_removed(limit_removed.iter().copied());
        let mut removed = removed;
        removed.extend(limit_removed);
        if !removed.is_empty() {
            self.truncated = true;
        }
        for id in removed {
            if self.selected_entry_id == Some(id) {
                self.selected_entry_id = None;
            }
            if self.detail_entry_id == Some(id) {
                self.detail_entry_id = None;
            }
            self.bookmarked_entry_ids.remove(&id);
        }
    }

    fn entry_detail(&self, entry_id: u64) -> Option<EntryDetail> {
        let item = self.store.get(entry_id)?;
        let raw_text = String::from_utf8_lossy(item.bytes()).into_owned();
        Some(EntryDetail {
            id: item.id(),
            port: item.port().to_owned(),
            timestamp_label: format!("[{}]", fmt_ts(item.first_timestamp_ms())),
            direction: item.direction(),
            display_text: format_terminal_text(&raw_text),
            hex_text: format_hex(item.bytes()),
            raw_text,
        })
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
                    ui.label(
                        RichText::new(&detail.port)
                            .monospace()
                            .color(theme::yellow()),
                    );
                    ui.label(RichText::new(dir_label).strong().color(dir_color));
                    ui.label(
                        RichText::new(format!("#{} · {}B", detail.id, detail.raw_text.len()))
                            .color(theme::text_dimmed())
                            .small(),
                    );

                    if ui.button("复制内容").clicked() {
                        copy_text_with_feedback(ui, detail.raw_text.clone(), "已复制原始内容");
                    }

                    if ui.button("复制显示文本").clicked() {
                        copy_text_with_feedback(ui, detail.display_text.clone(), "已复制显示文本");
                    }

                    if ui.button("复制 HEX").clicked() {
                        copy_text_with_feedback(ui, detail.hex_text.clone(), "已复制 HEX");
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

fn terminal_layout_key(
    show_hex: bool,
    show_raw: bool,
    font_size: f32,
    content_width_px: i32,
    preview_width_px: i32,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    show_hex.hash(&mut hasher);
    show_raw.hash(&mut hasher);
    ((font_size * 1000.0).round() as i32).hash(&mut hasher);
    content_width_px.hash(&mut hasher);
    preview_width_px.hash(&mut hasher);
    hasher.finish()
}

fn terminal_live_line_count(row: &VisibleRow<'_>, _show_hex: bool, _show_raw: bool) -> usize {
    if !row.live {
        return 0;
    }

    1
}

fn live_tail_max_chars(width: f32, glyph_width: f32) -> usize {
    (width.max(glyph_width) / glyph_width.max(1.0))
        .floor()
        .max(1.0) as usize
}

/// LiveTail 是固定高度的 transient row，只展示当前最后一段；完整历史字节
/// 仍保存在 Store 中，封存后再恢复完整记录的正常高度。
fn compact_live_tail_preview(content: &str, max_chars: usize) -> String {
    let tail = content.rsplit_once('\n').map_or(content, |(_, tail)| tail);
    compact_live_tail_segment(tail, max_chars)
}

fn compact_live_tail_segment(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }

    let tail: String = text
        .chars()
        .rev()
        .take(max_chars.saturating_sub(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalTableWidths {
    label: f32,
    hex: f32,
    preview: f32,
}

fn terminal_table_widths(
    full_width: f32,
    desired_label_width: f32,
    show_hex: bool,
) -> TerminalTableWidths {
    let full_width = full_width.max(0.0);
    let label = desired_label_width.min(full_width);
    let content = (full_width - label).max(0.0);
    let preview = if show_hex {
        (content * 0.3).max(PREVIEW_COL_MIN_WIDTH).min(content)
    } else {
        0.0
    };
    let hex = (content - preview).max(0.0);

    TerminalTableWidths {
        label,
        hex,
        preview,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_rows_view(
    ui: &mut egui::Ui,
    scroll_key: &str,
    height: f32,
    store: &TerminalStore,
    row_ids: &[u64],
    visible_ids_update: VisibleIdsUpdate,
    scroll_to_row: Option<usize>,
    show_hex: bool,
    show_raw: bool,
    show_timestamp: bool,
    show_port: bool,
    show_direction: bool,
    stick_to_bottom: bool,
    force_scroll_to_bottom: bool,
    wheel_scroll_delta_y: f32,
    font_size: f32,
    port_aliases: &std::collections::HashMap<String, String>,
    selection: &mut RowSelection,
    text_selection_rows: &mut TextSelectionRows,
    empty_hint: &str,
    virtual_rows: &mut VirtualRowIndex,
    navigate_highlight: Option<(u64, f64)>,
) -> RenderOutcome {
    let height = height.max(40.0);
    let font_id = egui::FontId::new(font_size, egui::FontFamily::Monospace);
    let row_height = ui.fonts_mut(|f| f.row_height(&font_id));
    if visible_ids_update.changed {
        selection.sync_rows(row_ids.iter().copied());
    }

    // 列宽随字体大小缩放（基准 13px）
    let scale = font_size / 13.0;
    let time_col_width = TIME_COL_WIDTH * scale;
    let port_col_width = PORT_COL_WIDTH * scale;
    let dir_col_width = DIR_COL_WIDTH * scale;
    let col_gap = COL_GAP * scale;
    let row_left_padding = ROW_LEFT_PADDING * scale;

    if row_ids.is_empty() {
        virtual_rows.clear();
        let scroll_output = ScrollArea::vertical()
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .max_height(height)
            .auto_shrink([false, false])
            .id_salt((scroll_key, "v2"))
            .show(ui, |ui| {
                ui.label(RichText::new(empty_hint).color(theme::text_secondary()));
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

    let mut navigate_id: Option<u64> = None;
    let mut measured_content_height: Option<f32> = None;

    let scroll_output = ScrollArea::vertical()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .auto_shrink([false, false])
        .stick_to_bottom(stick_to_bottom)
        .id_salt((scroll_key, "v2"))
        .show(ui, |ui| {
            let full_width = ui.available_width().max(0.0);
            let widths = terminal_table_widths(full_width, label_width, show_hex);
            let label_width = widths.label;
            let hex_width = widths.hex;
            let preview_width = widths.preview;
            let text_padding = 4.0;
            let text_color = ui.style().visuals.text_color();
            let glyph_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font_id, '0'));

            let galley_width = (hex_width - text_padding).max(0.0).floor();
            let preview_galley_width = if show_hex {
                (preview_width - text_padding).max(0.0).floor()
            } else {
                0.0
            };
            let content_width_px = galley_width.max(0.0).round() as i32;
            let preview_width_px = preview_galley_width.max(0.0).round() as i32;

            let layout_key = terminal_layout_key(
                show_hex,
                show_raw,
                font_size,
                content_width_px,
                preview_width_px,
            );
            let virtual_rows_changed = if visible_ids_update.append_only
                && !virtual_rows.needs_sync(layout_key, row_height)
            {
                virtual_rows.append_ids(row_ids, layout_key, row_height)
            } else if visible_ids_update.changed || virtual_rows.needs_sync(layout_key, row_height)
            {
                virtual_rows.sync_ids(row_ids, layout_key, row_height)
            } else {
                false
            };
            if virtual_rows_changed {
                ui.ctx().request_repaint();
            }
            let total_height = virtual_rows.total_height().max(row_height);

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

            let scroll_offset = (viewport_rect.top() - full_rect.top()).max(0.0);
            let base_range =
                virtual_rows.visible_range(scroll_offset, viewport_rect.height(), row_height * 2.0);
            let text_selection_layout_range = if text_selection_rows.is_active() {
                text_selection_rows.layout_range(row_ids.iter().copied())
            } else {
                None
            };
            let render_start = text_selection_layout_range
                .as_ref()
                .map_or(base_range.start, |range| {
                    base_range.start.min(*range.start())
                });
            let render_end = text_selection_layout_range
                .as_ref()
                .map_or(base_range.end, |range| {
                    base_range.end.max(range.end().saturating_add(1))
                });
            let render_range = render_start..render_end.min(row_ids.len());
            for row_idx in render_range.clone() {
                hl.record_row_at(
                    row_idx,
                    label_rect.top() + virtual_rows.row_top(row_idx),
                    virtual_rows.height(row_idx),
                );
            }

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
            let primary_down =
                ui.input(|input| input.pointer.button_down(egui::PointerButton::Primary));
            let owns_text_selection = owns_copy_focus(ui, COPY_OWNER);
            let has_text_selection = owns_text_selection
                && ui
                    .ctx()
                    .plugin::<LabelSelectionState>()
                    .lock()
                    .has_selection();
            if !owns_text_selection || (!primary_down && !has_text_selection) {
                text_selection_rows.clear();
            }
            if data_pressed
                && let Some(index) = hovered_idx
                && let Some(row_id) = row_ids.get(index)
            {
                text_selection_rows.begin(*row_id);
            }
            if primary_down
                && text_selection_rows.is_active()
                && let Some(index) = hovered_idx
                && let Some(row_id) = row_ids.get(index)
            {
                text_selection_rows.update(*row_id);
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
                text_selection_rows.clear();
                ui.ctx()
                    .plugin::<LabelSelectionState>()
                    .lock()
                    .clear_selection();
            }

            let mut text_drag_response: Option<egui::Response> = None;
            let mut row_heights_changed = false;
            for row_idx in render_range {
                let Some(row) = row_ids
                    .get(row_idx)
                    .and_then(|id| store.get(*id))
                    .map(VisibleRow::from_item)
                else {
                    continue;
                };
                let current_y = label_rect.top() + virtual_rows.row_top(row_idx);
                let content = visible_row_content(&row, show_hex, show_raw);
                let content = if terminal_live_line_count(&row, show_hex, show_raw) > 0 {
                    compact_live_tail_preview(
                        &content,
                        live_tail_max_chars(galley_width, glyph_width),
                    )
                } else {
                    content
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
                let galley = Some(ui.fonts_mut(|f| f.layout_job(layout_job)));

                let preview_galley = if show_hex {
                    let preview_text = if row.preview_text.is_empty() {
                        " ".to_owned()
                    } else {
                        row.preview_text.to_string()
                    };
                    let preview_text = if terminal_live_line_count(&row, show_hex, show_raw) > 0 {
                        compact_live_tail_preview(
                            &preview_text,
                            live_tail_max_chars(preview_galley_width, glyph_width),
                        )
                    } else {
                        preview_text
                    };
                    let mut layout_job = egui::text::LayoutJob::simple(
                        preview_text,
                        font_id.clone(),
                        theme::text_dimmed(),
                        preview_galley_width,
                    );
                    layout_job.halign = egui::Align::LEFT;
                    Some(ui.fonts_mut(|f| f.layout_job(layout_job)))
                } else {
                    None
                };

                let entry_height = if let Some(ref pg) = preview_galley {
                    galley
                        .as_ref()
                        .expect("terminal galley exists")
                        .size()
                        .y
                        .max(pg.size().y)
                        .max(row_height)
                } else {
                    galley
                        .as_ref()
                        .expect("terminal galley exists")
                        .size()
                        .y
                        .max(row_height)
                };
                let entry_height = entry_height.round().max(row_height);
                row_heights_changed |= virtual_rows.set_height(row_idx, entry_height);

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

                // 跳转目标行高亮（叠在 selection/hover 之上，按剩余时间淡出）
                if let Some((target_id, start)) = navigate_highlight
                    && row.id == target_id
                {
                    let now = ui.ctx().input(|i| i.time);
                    let elapsed = now - start;
                    if elapsed < NAV_HIGHLIGHT_DURATION {
                        let alpha = if elapsed > NAV_HIGHLIGHT_DURATION - NAV_FADE {
                            ((NAV_HIGHLIGHT_DURATION - elapsed) / NAV_FADE).clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                        ui.painter_at(full_rect).rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(full_rect.left(), current_y),
                                egui::vec2(full_rect.width(), entry_height),
                            ),
                            0.0,
                            theme::nav_highlight().gamma_multiply(alpha as f32),
                        );
                        ui.ctx().request_repaint();
                    }
                }

                // --- Draw left labels ---
                let mut x = label_rect.left() + row_left_padding;

                if show_timestamp {
                    label_painter.text(
                        egui::pos2(x, label_y),
                        egui::Align2::LEFT_CENTER,
                        row.timestamp_label.as_ref(),
                        font_id.clone(),
                        theme::text_secondary(),
                    );
                    x += time_col_width + col_gap;
                }

                if show_port {
                    if let Some(port) = row.port.as_deref() {
                        label_painter.text(
                            egui::pos2(x, label_y),
                            egui::Align2::LEFT_CENTER,
                            port_display_name(port, port_aliases),
                            font_id.clone(),
                            theme::yellow(),
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
                if let Some(ref galley) = galley
                    && hex_width > 0.0
                {
                    let galley_pos = egui::pos2(hex_rect.left() + text_padding, current_y);
                    // row_text_rect 只覆盖 galley 实际文本区域。点击文本 → egui 字符级拖选；
                    // 点击文本外的空白（文本前 padding、行尾、文本上下）→ 整行选中。
                    let galley_size = galley.size();
                    let row_text_rect = egui::Rect::from_min_size(galley_pos, galley_size);
                    let hex_row_rect = egui::Rect::from_min_size(
                        egui::pos2(hex_rect.left(), current_y),
                        egui::vec2(hex_width, entry_height),
                    );
                    // 先构造 response：文本外空白分支（按下即选）与文本内 clicked 分支
                    // （松开判定）都要用到它。
                    // Use a separate id salt for hex column to avoid id collision with preview
                    let row_id = ui.make_persistent_id(("hex", row.id));
                    // 字符拖选已经开始后，将命中区扩展到整条内容行。这样从面板外
                    // 移回来时，即使当前行比起点短、指针落在文字右侧，egui 也能
                    // 把内部 cursor 从旧端点迁移到当前行。
                    let text_interact_rect = if has_text_selection && primary_down {
                        hex_row_rect
                    } else {
                        row_text_rect
                    };
                    let response = ui.interact(text_interact_rect, row_id, Sense::click_and_drag());

                    let (primary_pressed, ctrl, shift) = ui.input(|i| {
                        (
                            i.pointer.button_pressed(egui::PointerButton::Primary),
                            i.modifiers.ctrl || i.modifiers.command,
                            i.modifiers.shift,
                        )
                    });
                    // 文本外空白处按下 → 整行选中（即时反馈，与点元数据区等效）。
                    // drag/release 由 handle_input 接管（label_rect 入口），这里只触发 begin。
                    if primary_pressed
                        && ui.rect_contains_pointer(hex_row_rect)
                        && !ui.rect_contains_pointer(row_text_rect)
                    {
                        text_selection_rows.clear();
                        selection.begin_pointer(row_idx, ctrl, shift);
                        // 整行选中与字符级文本选区互斥：清掉 egui 的 label 文本选区。
                        ui.ctx()
                            .plugin::<LabelSelectionState>()
                            .lock()
                            .clear_selection();
                    }
                    // 文本内：松开且未拖动 → 整行选中。
                    // response.clicked() 在 egui 中只有"按下→原地松开、未拖动"才为 true
                    // （拖动超过阈值后松开走 drag，clicked 为 false，字符选区正常进行）。
                    // Ctrl/Shift/Ctrl+Shift 修饰键在松开时读取，复用 begin_pointer 语义。
                    if response.clicked() && ui.rect_contains_pointer(row_text_rect) {
                        text_selection_rows.clear();
                        selection.begin_pointer(row_idx, ctrl, shift);
                        ui.ctx()
                            .plugin::<LabelSelectionState>()
                            .lock()
                            .clear_selection();
                    }
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
                if let (Some(pr), Some(pg)) = (preview_rect, &preview_galley)
                    && preview_width > 0.0
                {
                    let preview_pos = egui::pos2(pr.left() + text_padding, current_y);
                    let preview_painter = ui.painter_at(pr);
                    preview_painter.add(egui::epaint::TextShape::new(
                        preview_pos,
                        pg.clone(),
                        theme::text_dimmed(),
                    ));
                }
            }

            if row_heights_changed {
                // 真实 galley 高度会在本帧布局后才可得；下一帧需要重新分配
                // ScrollArea 内容高度并重新计算视口范围。
                ui.ctx().request_repaint();
            }
            let actual_total = virtual_rows.total_height().round();
            measured_content_height = Some(actual_total);
            if actual_total > total_height + 0.5 {
                ui.allocate_space(egui::vec2(0.0, actual_total - total_height));
            }

            let text_selection_dragging = text_drag_response
                .as_ref()
                .is_some_and(|response| response.dragged_by(egui::PointerButton::Primary));
            if text_selection_dragging
                && let Some(pointer_y) =
                    ui.input(|input| input.pointer.hover_pos().map(|pos| pos.y))
            {
                scroll_delta += edge_scroll_delta(pointer_y, viewport_rect.intersect(data_rect));
            }

            // egui 在左键拖选期间会阻止 ScrollArea 读取滚轮；这里仅在选择拖拽
            // 确实生效时补回滚轮量，普通滚动仍由 ScrollArea 自己处理，避免重复。
            scroll_delta += wheel_scroll_during_selection(
                text_selection_dragging || selection.is_dragging(),
                wheel_scroll_delta_y,
            );

            // 边缘滚动 / 拖选时滚轮滚动
            if scroll_delta != 0.0 {
                ui.scroll_with_delta(egui::vec2(0.0, scroll_delta));
                ui.ctx().request_repaint();
            }

            let frozen_row_idx = hl.resolve_click(
                ui,
                &ctx_response,
                ui.make_persistent_id(("term-frozen-row", scroll_key)),
            );
            if ctx_response.clicked_by(egui::PointerButton::Secondary)
                && let Some(index) = frozen_row_idx
                && !selection.is_selected(index)
            {
                selection.select_only(index);
            }
            // 双击任意位置（文字或空白）→ 离开搜索进入上下文：设置导航目标让下帧跳转。
            // 用全局 button_double_clicked + 整行 rect 命中，不再依赖只覆盖文本列的 ctx_response。
            let double_clicked = ui.input(|i| {
                i.pointer
                    .button_double_clicked(egui::PointerButton::Primary)
            }) && ui.rect_contains_pointer(full_rect);
            let mut pending_navigate: Option<u64> = None;
            if double_clicked
                && let Some(idx) = frozen_row_idx.or_else(|| hl.hover_index(ui))
                && let Some(row_id) = row_ids.get(idx)
            {
                pending_navigate = Some(*row_id);
            }
            // 捕获到外层变量
            if pending_navigate.is_some() {
                navigate_id = pending_navigate;
            }

            // 跳转到目标行（搜索时双击 → 离开搜索进入上下文）
            if let Some(target_row) = scroll_to_row {
                let y_top = label_rect.top() + virtual_rows.row_top(target_row);
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
                row_ids
                    .get(idx)
                    .and_then(|id| store.get(*id))
                    .map(VisibleRow::from_item)
                    .map(|row| {
                        let content_only = visible_row_content(&row, show_hex, show_raw);
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

            // Ctrl+A 全选：无 TextEdit 聚焦时选中所有可见行。
            // 用 consume_key 消费事件，阻止 egui 的 LabelSelectionState 再对当前 galley
            // 做字符级 Ctrl+A 全选（会与整行多选冲突）。
            if owns_copy_focus(ui, COPY_OWNER)
                && !ui.ctx().text_edit_focused()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::A))
            {
                selection.select_all();
            }

            // Ctrl+C 复制选中行：终端有选中、收到 Event::Copy、且无 TextEdit 聚焦时触发。
            // 复制 full（含时间戳/端口/方向前缀），与右键菜单"复制选中行"一致。
            // egui 0.35 把 Ctrl+C 转成 Event::Copy 事件（而非 Event::Key{C}），
            // 用 text_edit_focused 判断 TextEdit 聚焦（egui_wants_keyboard_input 过于宽泛，
            // 任何控件聚焦都返回 true）。
            let copy_requested =
                ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
            if !selected_indices.is_empty()
                && copy_requested
                && owns_copy_focus(ui, COPY_OWNER)
                && !ui.ctx().text_edit_focused()
                && let Some(full) = {
                    let selected_rows = visible_rows_for_indices(store, row_ids, &selected_indices);
                    let selected_row_indices: Vec<usize> = (0..selected_rows.len()).collect();
                    build_selected_full_text(
                        &selected_rows,
                        &selected_row_indices,
                        show_hex,
                        show_raw,
                    )
                }
            {
                copy_text_with_feedback(
                    ui,
                    full,
                    format!("已复制 {} 行（含时间、端口和方向）", selected_indices.len()),
                );
            }
            if selected_indices.is_empty()
                && copy_requested
                && owns_copy_focus(ui, COPY_OWNER)
                && !ui.ctx().text_edit_focused()
                && ui
                    .ctx()
                    .plugin::<LabelSelectionState>()
                    .lock()
                    .has_selection()
            {
                report_copy_feedback(ui, "已复制所选文本");
            }

            ctx_response.context_menu(move |ctx_ui| {
                let selected_count = selected_indices.len();
                let target_count = if selected_count > 0 {
                    selected_count
                } else {
                    usize::from(hovered_row.is_some())
                };
                let full_label = if selected_count > 0 {
                    format!("复制选中 {selected_count} 行（含元数据）")
                } else {
                    "复制此行（含元数据）".to_owned()
                };
                if bulk_copy_button(ctx_ui, "terminal-selected-full", full_label, target_count) {
                    let text = if selected_count > 0 {
                        let selected_rows =
                            visible_rows_for_indices(store, row_ids, &selected_indices);
                        let selected_row_indices: Vec<usize> = (0..selected_rows.len()).collect();
                        build_selected_full_text(
                            &selected_rows,
                            &selected_row_indices,
                            show_hex,
                            show_raw,
                        )
                    } else {
                        hovered_row.as_ref().map(|(full, _)| full.clone())
                    };
                    if let Some(text) = text {
                        copy_text_with_feedback(
                            ctx_ui,
                            text,
                            format!("已复制 {target_count} 行（含时间、端口和方向）"),
                        );
                    }
                    ctx_ui.close();
                }

                let data_label = if selected_count > 0 {
                    format!("复制选中 {selected_count} 行数据")
                } else {
                    "复制此行数据".to_owned()
                };
                if bulk_copy_button(ctx_ui, "terminal-selected-data", data_label, target_count) {
                    let text = if selected_count > 0 {
                        let selected_rows =
                            visible_rows_for_indices(store, row_ids, &selected_indices);
                        let selected_row_indices: Vec<usize> = (0..selected_rows.len()).collect();
                        build_selected_data_text(
                            &selected_rows,
                            &selected_row_indices,
                            show_hex,
                            show_raw,
                        )
                    } else {
                        hovered_row.as_ref().map(|(_, data)| data.clone())
                    };
                    if let Some(text) = text {
                        copy_text_with_feedback(
                            ctx_ui,
                            text,
                            format!("已复制 {target_count} 行数据"),
                        );
                    }
                    ctx_ui.close();
                }
                if target_count > 0 {
                    ctx_ui.separator();
                }

                if bulk_copy_button(
                    ctx_ui,
                    "terminal-all-content",
                    format!("复制全部可见内容（{} 行）", row_ids.len()),
                    row_ids.len(),
                ) {
                    let all_indices: Vec<usize> = (0..row_ids.len()).collect();
                    let rows = visible_rows_for_indices(store, row_ids, &all_indices);
                    let combined_text: String = rows
                        .iter()
                        .map(|row| visible_row_content(row, show_hex, show_raw))
                        .collect::<Vec<_>>()
                        .join("\n");
                    copy_text_with_feedback(
                        ctx_ui,
                        combined_text,
                        format!("已复制全部可见内容（{} 行）", row_ids.len()),
                    );
                    ctx_ui.close();
                }

                if bulk_copy_button(
                    ctx_ui,
                    "terminal-all-csv",
                    "复制全部可见为 CSV",
                    row_ids.len(),
                ) {
                    let all_indices: Vec<usize> = (0..row_ids.len()).collect();
                    let rows = visible_rows_for_indices(store, row_ids, &all_indices);
                    let csv = build_csv(
                        &rows,
                        show_hex,
                        show_raw,
                        show_timestamp || show_port || show_direction,
                    );
                    copy_text_with_feedback(
                        ctx_ui,
                        csv,
                        format!("已复制 CSV（{} 行）", row_ids.len()),
                    );
                    ctx_ui.close();
                }

                if bulk_copy_button(
                    ctx_ui,
                    "terminal-all-jsonl",
                    "复制全部可见为 JSONL",
                    row_ids.len(),
                ) {
                    let all_indices: Vec<usize> = (0..row_ids.len()).collect();
                    let rows = visible_rows_for_indices(store, row_ids, &all_indices);
                    let jsonl = build_jsonl(
                        &rows,
                        show_hex,
                        show_raw,
                        show_timestamp,
                        show_port,
                        show_direction,
                    );
                    copy_text_with_feedback(
                        ctx_ui,
                        jsonl,
                        format!("已复制 JSONL（{} 行）", row_ids.len()),
                    );
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
        content_height: measured_content_height.unwrap_or(scroll_output.content_size.y),
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

fn strip_terminal_trailing_line_ending(content: &str) -> &str {
    content
        .strip_suffix("\r\n")
        .or_else(|| content.strip_suffix('\n'))
        .or_else(|| content.strip_suffix('\r'))
        .unwrap_or(content)
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
    strip_terminal_trailing_line_ending(content).to_owned()
}

fn entry_visible(direction: Direction, show_rx: bool, show_tx: bool) -> bool {
    match direction {
        Direction::Rx => show_rx,
        Direction::Tx => show_tx,
        Direction::Internal => false,
    }
}

/// 构造选中行的完整文本（含时间戳、端口和方向）。
fn visible_rows_for_indices(
    store: &TerminalStore,
    row_ids: &[u64],
    indices: &[usize],
) -> Vec<VisibleRow<'static>> {
    indices
        .iter()
        .filter_map(|&index| row_ids.get(index).and_then(|id| store.get(*id)))
        .map(VisibleRow::from_item)
        .collect()
}

fn build_selected_full_text<'a>(
    rows: &[VisibleRow<'a>],
    selected_indices: &[usize],
    show_hex: bool,
    show_raw: bool,
) -> Option<String> {
    if selected_indices.is_empty() {
        return None;
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
    Some(full)
}

/// 构造选中行的纯数据文本。
fn build_selected_data_text<'a>(
    rows: &[VisibleRow<'a>],
    selected_indices: &[usize],
    show_hex: bool,
    show_raw: bool,
) -> Option<String> {
    if selected_indices.is_empty() {
        return None;
    }
    Some(
        selected_indices
            .iter()
            .map(|&index| &rows[index])
            .map(|row| visible_row_content(row, show_hex, show_raw))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn row_matches_search(
    row: &VisibleRow<'_>,
    search: &crate::search::SearchQuery,
    show_hex: bool,
    show_raw: bool,
) -> bool {
    if search.is_empty() {
        return true;
    }
    let hit = |haystack: &str| search.matches(haystack);
    // 根据显示模式搜索对应字段：
    // - HEX 模式：搜索 hex_text
    // - 原始模式：搜索 raw_text
    // - 文本模式：搜索 raw_text 和 display_text
    if show_hex {
        hit(row.hex_text.as_ref())
    } else if show_raw {
        hit(row.raw_text.as_ref())
    } else {
        hit(row.raw_text.as_ref()) || hit(row.display_text.as_ref())
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

fn payload_bytes(event: &Event) -> Cow<'_, [u8]> {
    if let Some(bytes) = event.payload.as_bytes() {
        Cow::Borrowed(bytes)
    } else {
        Cow::Owned(event.payload.text_lossy().into_bytes())
    }
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
        Direction::Rx => ("RX", theme::green()),
        Direction::Tx => ("TX", theme::blue()),
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
    use tool_application::service::terminal_store::{Continuation, TerminalRecord};
    use tool_core::Payload;
    use tool_databus::DataBus;

    fn port_items<'a>(panel: &'a TerminalPanel, port: &str) -> Vec<&'a TerminalItem> {
        panel
            .store
            .iter()
            .filter(|item| item.port() == port)
            .collect()
    }

    fn item_text(item: &TerminalItem) -> String {
        String::from_utf8_lossy(item.bytes()).into_owned()
    }

    #[test]
    fn short_port_display_trims_ipv4_port_to_last_two_octets() {
        assert_eq!(short_port_display("192.168.1.100:7125"), "1.100");
        assert_eq!(short_port_display("10.0.0.5:7125"), "0.5");
        assert_eq!(short_port_display("192.168.100.250:4408"), "100.250");
    }

    #[test]
    fn short_port_display_keeps_other_port_names() {
        assert_eq!(short_port_display("COM3"), "COM3");
        assert_eq!(short_port_display("/dev/ttyUSB0"), "/dev/ttyUSB0");
        // 非 IPv4（含字母的主机名）不截断
        assert_eq!(
            short_port_display("klipper.local:7125"),
            "klipper.local:7125"
        );
    }

    #[test]
    fn port_display_name_prefers_alias() {
        let mut aliases = std::collections::HashMap::new();
        aliases.insert("192.168.1.100:7125".to_owned(), "主控板".to_owned());
        assert_eq!(port_display_name("192.168.1.100:7125", &aliases), "主控板");
    }

    #[test]
    fn port_display_name_falls_back_to_short() {
        let aliases = std::collections::HashMap::new();
        assert_eq!(port_display_name("192.168.1.100:7125", &aliases), "1.100");
        assert_eq!(port_display_name("COM3", &aliases), "COM3");
    }

    #[test]
    fn port_display_name_ignores_blank_alias() {
        let mut aliases = std::collections::HashMap::new();
        aliases.insert("COM3".to_owned(), "  ".to_owned());
        assert_eq!(port_display_name("COM3", &aliases), "COM3");
    }

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

        let items = port_items(&panel, "COM1");
        assert_eq!(items.len(), 1);

        let item = items[0];
        assert_eq!(item.direction(), Direction::Rx);
        assert_eq!(item_text(item), "hello");
        assert!(item.is_live());
        assert_eq!(format_hex(item.bytes()), "68 65 6C 6C 6F");
    }

    #[test]
    fn merge_does_not_create_unbounded_visual_entry() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);
        let first = "a".repeat(MAX_TERMINAL_BLOCK_BYTES);

        for (timestamp, text) in [(1_000, first), (1_001, "b".to_owned())] {
            bus.publish(
                Event::with_timestamp(
                    timestamp,
                    serial_topics::SERIAL_RX,
                    "serial:COM1",
                    Direction::Rx,
                    Payload::Text(text),
                )
                .with_metadata(serde_json::json!({ "port": "COM1" })),
            );
        }

        assert_eq!(panel.ingest_all_pending(), 2);
        let items = port_items(&panel, "COM1");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].bytes().len(), MAX_TERMINAL_BLOCK_BYTES);
        assert_eq!(item_text(items[1]), "b");
        assert!(!items[0].is_live());
        assert!(items[1].is_live());
    }

    #[test]
    fn live_tail_preview_keeps_latest_text_and_completed_lines() {
        assert_eq!(
            compact_live_tail_preview("completed\n0123456789", 5),
            "…6789"
        );
        assert_eq!(compact_live_tail_preview("short", 5), "short");
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
        assert!(panel.store.is_empty());
    }

    #[test]
    fn max_entries_is_global_across_ports_and_keeps_newest_events() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);
        panel.set_max_entries(100);

        for index in 0..120 {
            let port = if index % 2 == 0 { "COM1" } else { "COM2" };
            bus.publish(
                Event::with_timestamp(
                    1_000 + index,
                    serial_topics::SERIAL_RX,
                    format!("serial:{port}"),
                    Direction::Rx,
                    Payload::Text(format!("message-{index}\n")),
                )
                .with_metadata(serde_json::json!({ "port": port })),
            );
        }

        assert_eq!(panel.ingest_all_pending(), 120);
        let items: Vec<&TerminalItem> = panel.store.iter().collect();
        assert_eq!(items.len(), 100);
        assert!(items.iter().all(|item| item.first_event_id() > 20));
        let visible_ids: Vec<u64> = panel
            .collect_visible_rows()
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert!(visible_ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(panel.truncated);
    }

    #[test]
    fn lowering_max_entries_trims_immediately() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);
        for index in 0..120 {
            bus.publish(Event::with_timestamp(
                index,
                serial_topics::SERIAL_RX,
                "serial:COM1",
                Direction::Rx,
                Payload::Text(format!("message-{index}\n")),
            ));
        }
        panel.ingest_all_pending();

        panel.set_max_entries(100);

        let items = port_items(&panel, "COM1");
        assert_eq!(items.len(), 100);
        assert_eq!(items[0].first_event_id(), 21);
    }

    #[test]
    fn export_uses_standard_json_and_the_current_visible_content_mode() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);
        bus.publish(
            Event::new(
                serial_topics::SERIAL_RX,
                "serial:COM1",
                Direction::Rx,
                Payload::Bytes(b"hello,\"world\"".to_vec()),
            )
            .with_metadata(serde_json::json!({ "port": "COM1" })),
        );
        bus.publish(
            Event::new(
                serial_topics::SERIAL_TX,
                "serial:COM1",
                Direction::Tx,
                Payload::Bytes(b"hidden tx".to_vec()),
            )
            .with_metadata(serde_json::json!({ "port": "COM1" })),
        );
        assert_eq!(panel.ingest_all_pending(), 2);
        panel.show_tx = false;

        let csv = panel.export_visible_csv();
        assert!(csv.starts_with("time,port,direction,text\n"));
        assert!(csv.contains("\"hello,\"\"world\"\"\""));
        assert_eq!(panel.export_visible_text(), "hello,\"world\"\n");

        let json: serde_json::Value =
            serde_json::from_str(&panel.export_visible_json()).expect("valid JSON array");
        let rows = json.as_array().expect("top-level array");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["port"], "COM1");
        assert_eq!(rows[0]["direction"], "RX");
        assert_eq!(rows[0]["text"], "hello,\"world\"");

        panel.show_hex = true;
        let json: serde_json::Value =
            serde_json::from_str(&panel.export_visible_json()).expect("valid HEX JSON array");
        assert_eq!(json[0]["hex"], "68 65 6C 6C 6F 2C 22 77 6F 72 6C 64 22");
        assert!(json[0].get("text").is_none());
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

        let items = port_items(&panel, "COM6");
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(items[0]), "(2.00000)echo:busy: processing*26\n");
        assert_eq!(item_text(items[1]), "next");
        assert!(!items[0].is_live());
        assert!(items[1].is_live());
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

        let items = port_items(&panel, "COM6");
        // event1 产生一条 sealed 行和一个 LiveTail，event2 封存尾行。
        assert_eq!(items.len(), 2);
        assert_eq!(
            item_text(items[0]),
            "(2.00000)X first home. completed.*77\n"
        );
        assert_eq!(item_text(items[1]), "(2.00000)X home. timeout = 20*16\n");
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

        let items = port_items(&panel, "COM7");
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(items[0]), "abc\n");
        assert_eq!(item_text(items[1]), "defghi\n");
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
        let items = port_items(&panel, "COM8");
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(items[0]), "abc\n");
        assert_eq!(item_text(items[1]), "defghi");
    }

    #[test]
    fn visible_row_hides_only_the_final_line_ending() {
        let row = VisibleRow {
            id: 1,
            port: Some(Cow::Borrowed("COM6")),
            timestamp_label: Cow::Borrowed("[10:00:39.580]"),
            direction: Direction::Rx,
            raw_text: Cow::Borrowed("first\r\nsecond\r\n"),
            display_text: Cow::Borrowed("first\nsecond\n"),
            hex_text: Cow::Borrowed("66 69 72 73 74 0D 0A"),
            preview_text: Cow::Borrowed("first\nsecond\n"),
            live: false,
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
    fn terminal_layout_key_changes_when_layout_inputs_change() {
        let base = terminal_layout_key(false, false, 13.0, 240, 0);
        assert_ne!(base, terminal_layout_key(false, false, 13.0, 180, 0));
        assert_ne!(base, terminal_layout_key(true, false, 13.0, 240, 0));
        assert_ne!(base, terminal_layout_key(false, true, 13.0, 240, 0));
        assert_ne!(base, terminal_layout_key(false, false, 14.0, 240, 0));
    }

    #[test]
    fn table_widths_do_not_exceed_available_width_when_narrow() {
        let widths = terminal_table_widths(96.0, 220.0, true);

        assert_eq!(widths.label, 96.0);
        assert_eq!(widths.hex, 0.0);
        assert_eq!(widths.preview, 0.0);
        assert!(widths.label + widths.hex + widths.preview <= 96.0);

        let widths = terminal_table_widths(180.0, 120.0, true);
        assert!(widths.label + widths.hex + widths.preview <= 180.0);
        assert!(widths.preview <= 60.0);
    }

    #[test]
    fn entries_map_one_to_one_visible_rows() {
        let item = TerminalItem::Sealed(TerminalRecord {
            id: 1,
            first_event_id: 1,
            last_event_id: 1,
            first_timestamp_ms: 0,
            last_timestamp_ms: 0,
            port: "COM6".to_owned(),
            direction: Direction::Rx,
            bytes: b"(42.0000)ok*29\n".to_vec(),
            continuation: Continuation::Complete,
        });

        let row = VisibleRow::from_item(&item);

        assert_eq!(row.port.as_deref(), Some("COM6"));
        assert_eq!(row.raw_text, "(42.0000)ok*29\n");
        assert!(!row.live);
    }

    /// 换行是展示边界，同一个 packet 内的多行也分别封存。
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
        let items = port_items(&panel, "COM2");
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(items[0]), "111\n");
        assert_eq!(item_text(items[1]), "111\n");
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
        let items = port_items(&panel, "COM2");
        assert_eq!(items.len(), 1);
        assert_eq!(item_text(items[0]), "1\n");
    }

    /// 两次快速发送也保持每个换行分段，不再使用 5ms merge 定义记录。
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
        let items = port_items(&panel, "COM2");
        assert_eq!(items.len(), 4);
        assert!(items.iter().all(|item| item_text(item) == "111\n"));
    }

    /// 周期发送（3ms × 3次）：每个换行分段保持独立。
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
        let items = port_items(&panel, "COM2");
        assert_eq!(items.len(), 6);
        assert!(items.iter().all(|item| item_text(item) == "111\n"));
    }

    /// 发送三行完整文本：得到三条 sealed record。
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
        let items = port_items(&panel, "COM2");
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|item| item_text(item) == "111\n"));
    }

    /// 发送两条完整行和一个未完成的 LiveTail。
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
        let items = port_items(&panel, "COM2");
        assert_eq!(items.len(), 3);
        assert_eq!(item_text(items[0]), "111\n");
        assert_eq!(item_text(items[1]), "111\n");
        assert_eq!(item_text(items[2]), "111");
        assert!(items[2].is_live());
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

        panel.search.text = "2".to_owned();
        let rows = panel.collect_visible_rows();
        assert_eq!(rows.len(), 1, "search '2' should match exactly 1 entry");
        assert_eq!(rows[0].raw_text, "2");

        panel.search.text = "3".to_owned();
        let rows = panel.collect_visible_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].raw_text, "3");
    }

    #[test]
    fn view_index_updates_only_the_changed_live_tail_for_active_search() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        bus.publish(Event::with_timestamp(
            1_000,
            serial_topics::SERIAL_RX,
            "serial:COM9",
            Direction::Rx,
            Payload::Bytes(b"prefix".to_vec()),
        ));
        assert_eq!(panel.ingest_all_pending(), 1);

        panel.search.text = "target".to_owned();
        assert!(panel.collect_visible_rows().is_empty());

        bus.publish(Event::with_timestamp(
            1_001,
            serial_topics::SERIAL_RX,
            "serial:COM9",
            Direction::Rx,
            Payload::Bytes(b"target\n".to_vec()),
        ));
        assert_eq!(panel.ingest_all_pending(), 1);

        let rows = panel.collect_visible_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].raw_text, "prefixtarget\n");
        assert!(!rows[0].live);
    }

    #[test]
    fn view_index_marks_tail_growth_as_append_only() {
        let bus = DataBus::new();
        let mut panel = TerminalPanel::new(&bus);

        for (timestamp, text) in [(1_000, b"first".as_slice()), (2_000, b"second".as_slice())] {
            bus.publish(Event::with_timestamp(
                timestamp,
                serial_topics::SERIAL_RX,
                "serial:COM10",
                Direction::Rx,
                Payload::Bytes(text.to_vec()),
            ));
            assert_eq!(panel.ingest_all_pending(), 1);
            let _ = panel.collect_visible_rows();
        }

        let update = panel
            .view_index
            .sync(&panel.store, panel.current_view_filter());
        assert!(!update.changed);

        bus.publish(Event::with_timestamp(
            3_000,
            serial_topics::SERIAL_RX,
            "serial:COM10",
            Direction::Rx,
            Payload::Bytes(b"third".to_vec()),
        ));
        assert_eq!(panel.ingest_all_pending(), 1);
        let update = panel
            .view_index
            .sync(&panel.store, panel.current_view_filter());
        assert!(update.changed);
        assert!(update.append_only);
    }
}
