use crate::scrollback::{ExportChunkState, Scrollback};
use crate::{
    MAX_INGEST_PER_FRAME, MESSAGE_EVENT_BUFFER_CAPACITY, TerminalExportFormat, design, fmt_ts,
    table::{
        AutoScrollState, MessageSearch, RowHighlight, RowSelection, TextSelectionRows,
        bulk_copy_button, claim_copy_focus, copy_text_with_feedback, edge_scroll_delta,
        estimated_wrapped_line_count, owns_copy_focus, report_copy_feedback,
        wheel_scroll_during_selection,
    },
    theme,
    virtual_list::VirtualRowIndex,
};
use egui::text_selection::LabelSelectionState;
use egui::{RichText, ScrollArea, Sense, Stroke};
use egui_material_icons::icons::ICON_ARTICLE;
use std::collections::{BTreeSet, HashMap, VecDeque};
use tool_core::{Event, LogLevel};
use tool_databus::{DataBus, TopicFilter};

const TIME_COL_WIDTH: f32 = 118.0;
const LEVEL_COL_WIDTH: f32 = 52.0;
const SOURCE_COL_WIDTH: f32 = 140.0;
const SOURCE_TEXT_MAX_CHARS: usize = 18;
const ROW_LEFT_PADDING: f32 = 4.0;
const COL_GAP: f32 = 6.0;
/// 标签列到消息列之间的间距
const LABEL_TO_MSG_GAP: f32 = 3.0;
const LOG_SCROLL_ID: &str = "log-scroll-v2";
const COPY_OWNER: &str = "log";
/// 清空按钮两步确认的状态 id。
const LOG_CLEAR_CONFIRM_ID: &str = "log-clear-confirm";
/// 日志面板最大保留条数（与终端面板一致）。
const MAX_LOG_ENTRIES: usize = 50_000;
/// 跳转目标行高亮总时长（秒）。
const NAV_HIGHLIGHT_DURATION: f64 = 1.5;
/// 跳转目标行高亮末段淡出时长（秒）。
const NAV_FADE: f64 = 0.3;

pub struct LogPanel {
    scrollback: Scrollback,
    entry_order: VecDeque<u64>,
    entries: HashMap<u64, LogEntry>,
    source_counts: HashMap<String, usize>,
    source_names_cache: BTreeSet<String>,
    visible_rows_cache: VecDeque<LogEntry>,
    visible_filter_key: Option<LogFilterKey>,
    visible_rows_dirty: bool,
    visible_rows_generation: u64,
    next_entry_id: u64,
    min_level: LogLevel,
    auto_scroll: AutoScrollState,
    export_request: Option<TerminalExportFormat>,
    /// 双击搜索匹配行时设置：下帧清除搜索并跳转到该行。
    pending_navigate_to_id: Option<u64>,
    /// 跳转目标行高亮：(目标行 id, 起始时间秒)。渲染时若命中且未超时画强调色并淡出。
    navigate_highlight: Option<(u64, f64)>,
    /// 搜索文本（默认大小写不敏感，同时匹配 source 和 message）。
    search: MessageSearch,
    /// 来源过滤：None 表示显示全部，Some 表示只显示指定 source。
    source_filter: Option<String>,
    /// 用户可调的字体大小（10-24px），默认 13.0
    pub font_size: f32,
    /// 行框选状态
    pub selection: RowSelection,
    /// 字符级拖选覆盖的行；用于在自动滚动时保活视口外的选区端点。
    text_selection_rows: TextSelectionRows,
    /// 日志消息流的共享虚拟渲染状态（与接收区共用实现）。
    virtual_rows: VirtualRowIndex,
}

#[derive(Clone)]
struct LogEntry {
    id: u64,
    timestamp_label: String,
    level: LogLevel,
    source: String,
    message: String,
}

#[derive(Clone, PartialEq, Eq)]
struct LogFilterKey {
    min_level: LogLevel,
    source_filter: Option<String>,
    search_text: String,
    case_sensitive: bool,
}

/// Incremental browser export cursor.  It captures the filter at the moment
/// export starts and walks the stable entry order in bounded batches.
pub struct LogExportCursor {
    filter: LogFilterKey,
    limit: usize,
    index: usize,
    progress: ExportChunkState,
}

/// 渲染一段导出内容：`first` 决定是否补表头/数组左括号，`has_previous` 决定是否补
/// 分段之间的分隔符，`done` 决定是否收尾。这是导出字节流的唯一渲染实现，
/// 两端导出都通过增量游标（[`LogExportCursor`] 的 `export_cursor_chunk`）走到这里。
fn render_log_export_chunk(
    format: TerminalExportFormat,
    entries: &[LogEntry],
    first: bool,
    has_previous: bool,
    done: bool,
) -> String {
    match format {
        TerminalExportFormat::Txt => {
            let mut output = String::new();
            if has_previous && !entries.is_empty() {
                output.push('\n');
            }
            output.push_str(
                &entries
                    .iter()
                    .map(log_export_line)
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            if done && !entries.is_empty() {
                output.push('\n');
            }
            output
        }
        TerminalExportFormat::Csv => {
            let mut output = String::new();
            if first {
                output.push_str("time,level,source,message\n");
            }
            for entry in entries {
                output.push_str(&log_csv_record(entry));
                output.push('\n');
            }
            output
        }
        TerminalExportFormat::Json => {
            let mut output = String::new();
            if first {
                output.push('[');
            } else if has_previous && !entries.is_empty() {
                output.push(',');
            }
            for (index, entry) in entries.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&log_json_value(entry).to_string());
            }
            if done {
                output.push(']');
            }
            output
        }
    }
}

/// 导出 TXT 的一行：定宽对齐时间、级别与来源，便于肉眼比对。
/// 只用于导出；复制到剪贴板的整行见 [`log_display_line`]。
fn log_export_line(entry: &LogEntry) -> String {
    format!(
        "{} {:<5} {:<18} {}",
        entry.timestamp_label,
        entry.level.as_str(),
        entry.source,
        entry.message
    )
}

/// 复制到剪贴板的整行文本：单个空格分隔，不做定宽补齐。
fn log_display_line(entry: &LogEntry) -> String {
    format!(
        "{} {} {} {}",
        entry.timestamp_label,
        entry.level.as_str(),
        entry.source,
        entry.message
    )
}

/// 导出 CSV 的一行（不含行尾换行），列顺序与表头 `time,level,source,message` 对应。
fn log_csv_record(entry: &LogEntry) -> String {
    [
        csv_cell(&entry.timestamp_label),
        csv_cell(entry.level.as_str()),
        csv_cell(&entry.source),
        csv_cell(&entry.message),
    ]
    .join(",")
}

/// 导出 JSON / JSONL 的一条记录。
fn log_json_value(entry: &LogEntry) -> serde_json::Value {
    serde_json::json!({
        "time": entry.timestamp_label,
        "level": entry.level.as_str(),
        "source": entry.source,
        "message": entry.message,
    })
}

struct LogRenderOutcome {
    inner_rect: egui::Rect,
    content_height: f32,
    offset_y: f32,
    /// 双击搜索匹配行时设置：该行的 entry id，供调用方清除搜索并跳转。
    pending_navigate_to_id: Option<u64>,
}

/// 主键或次键在本面板矩形内按下（用于抢占复制焦点）。
fn log_panel_pressed_in(ui: &egui::Ui, panel_rect: egui::Rect) -> bool {
    ui.input(|input| {
        let pointer = &input.pointer;
        (pointer.button_pressed(egui::PointerButton::Primary)
            || pointer.button_pressed(egui::PointerButton::Secondary))
            && pointer
                .hover_pos()
                .is_some_and(|position| panel_rect.contains(position))
    })
}

/// 指针位于本面板内时的滚轮增量；面板外记为 0，让后续的“滚轮向下”判定不受
/// 其它区域滚轮事件的影响。
fn log_wheel_scroll_delta_y(ui: &egui::Ui, panel_rect: egui::Rect) -> f32 {
    let pointer_inside = ui
        .input(|input| input.pointer.hover_pos())
        .is_some_and(|pos| panel_rect.contains(pos));
    if pointer_inside {
        ui.input(|input| input.smooth_scroll_delta.y)
    } else {
        0.0
    }
}

impl LogPanel {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            scrollback: Scrollback::new(
                bus,
                TopicFilter::prefix("log."),
                MESSAGE_EVENT_BUFFER_CAPACITY,
                MAX_LOG_ENTRIES,
            ),
            entry_order: VecDeque::new(),
            entries: HashMap::new(),
            source_counts: HashMap::new(),
            source_names_cache: BTreeSet::new(),
            visible_rows_cache: VecDeque::new(),
            visible_filter_key: None,
            visible_rows_dirty: true,
            visible_rows_generation: 1,
            next_entry_id: 1,
            min_level: LogLevel::Info,
            auto_scroll: AutoScrollState::default(),
            export_request: None,
            pending_navigate_to_id: None,
            navigate_highlight: None,
            search: MessageSearch::default(),
            source_filter: None,
            font_size: 13.0,
            selection: RowSelection::new(0),
            text_selection_rows: TextSelectionRows::default(),
            virtual_rows: VirtualRowIndex::default(),
        }
    }
    pub fn ingest_all_pending(&mut self) -> usize {
        // 每帧最多摄入 2000 条，防止大量日志突发时 UI 卡顿
        const MAX_INGEST_ALL: usize = 2000;
        let mut count = 0;
        let mut batch = self.scrollback.take_batch(MAX_INGEST_ALL);
        for event in batch.drain(..) {
            self.push_event(event);
            count += 1;
        }
        self.scrollback.recycle_batch(batch);
        count
    }
    pub fn clear(&mut self) {
        self.scrollback.clear();
        self.entry_order.clear();
        self.entries.clear();
        self.source_counts.clear();
        self.source_names_cache.clear();
        self.visible_rows_cache.clear();
        self.visible_filter_key = None;
        self.visible_rows_dirty = true;
        self.visible_rows_generation = self.visible_rows_generation.wrapping_add(1);
        self.auto_scroll.reset();
        self.pending_navigate_to_id = None;
        self.search.clear();
        self.source_filter = None;
        self.selection.clear();
        self.text_selection_rows.clear();
        self.virtual_rows.clear();
    }

    /// 收集所有已出现过的 source 名称，用于过滤下拉框。
    fn source_names(&self) -> Vec<String> {
        self.source_names_cache.iter().cloned().collect()
    }

    /// 让 main.rs 在日志面板不可见时也能消费日志事件。
    pub fn ingest_pending(&mut self) -> usize {
        self.ingest()
    }

    /// 与接收区同一契约：同一轮持续丢包只上报一次，静默 `DROP_EPISODE_QUIET_MS`
    /// 之后才重新武装。
    pub fn take_dropped_events(&mut self, now_ms: u64) -> u64 {
        self.scrollback.take_dropped_events(now_ms)
    }

    pub fn max_entries(&self) -> usize {
        self.scrollback.max_entries()
    }

    pub fn max_entries_mut(&mut self) -> &mut usize {
        self.scrollback.max_entries_mut()
    }

    pub fn take_truncated(&mut self) -> bool {
        self.scrollback.take_truncated()
    }

    pub fn truncated(&self) -> bool {
        self.scrollback.truncated()
    }

    pub fn clear_truncated(&mut self) {
        self.scrollback.clear_truncated();
    }

    pub fn take_export_request(&mut self) -> Option<TerminalExportFormat> {
        self.export_request.take()
    }

    pub fn begin_export_cursor(&self) -> LogExportCursor {
        LogExportCursor {
            filter: self.current_filter_key(),
            limit: self.entry_order.len(),
            index: 0,
            progress: ExportChunkState::new(),
        }
    }

    /// 每批最多扫描 `scan_budget` 条来源事件，返回
    /// `(text, finished, exported_rows)`。过滤是随游标推进做的，
    /// 因此大日志不必在浏览器首次导出帧前构造出完整的可见行向量。
    pub fn export_cursor_chunk(
        &self,
        cursor: &mut LogExportCursor,
        format: TerminalExportFormat,
        scan_budget: usize,
    ) -> (String, bool, usize) {
        if cursor.progress.done() {
            return (String::new(), true, 0);
        }
        let budget = scan_budget.max(1);
        let end = (cursor.index + budget).min(cursor.limit);
        let query = cursor.filter.query();
        let entries = self
            .entry_order
            .iter()
            .skip(cursor.index)
            .take(end - cursor.index)
            .filter_map(|id| self.entries.get(id))
            .filter(|entry| log_entry_matches(entry, &cursor.filter, &query))
            .cloned()
            .collect::<Vec<_>>();
        cursor.index = end;
        let done = end >= cursor.limit || end >= self.entry_order.len();
        let (first, has_previous) = cursor.progress.begin();
        let exported = entries.len();
        cursor.progress.finish(done, exported);
        (
            render_log_export_chunk(format, &entries, first, has_previous, done),
            done,
            exported,
        )
    }

    pub fn set_max_entries(&mut self, max_entries: usize) {
        self.scrollback.set_max_entries(max_entries);
        self.enforce_max_entries();
    }

    fn enforce_max_entries(&mut self) {
        // 丢弃最旧条目不会改变过滤条件，所以缓存是否可用只需判定一次。
        let cache_is_current = !self.visible_rows_dirty
            && self.visible_filter_key.as_ref() == Some(&self.current_filter_key());
        let mut removed = 0;
        while self.entry_order.len() > self.scrollback.max_entries() {
            let Some(removed_id) = self.entry_order.pop_front() else {
                break;
            };
            removed += 1;
            if let Some(removed) = self.entries.remove(&removed_id)
                && let Some(count) = self.source_counts.get_mut(&removed.source)
            {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.source_counts.remove(&removed.source);
                    self.source_names_cache.remove(&removed.source);
                }
            }
            if cache_is_current
                && self
                    .visible_rows_cache
                    .front()
                    .is_some_and(|entry| entry.id == removed_id)
            {
                self.visible_rows_cache.pop_front();
                self.visible_rows_generation = self.visible_rows_generation.wrapping_add(1);
            }
        }
        self.scrollback
            .note_trimmed(removed, self.entry_order.len());
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.ingest();

        // 仅当指针位于本面板内时，滚轮向下才触发强制滚到底；
        // 否则全局 smooth_scroll_delta 会误捕获其它区域的滚轮事件。
        let panel_rect = ui.max_rect();
        if log_panel_pressed_in(ui, panel_rect) {
            claim_copy_focus(ui, COPY_OWNER);
        }
        // ScrollArea 会消费并清空 smooth_scroll_delta，必须在渲染前保存。
        let scroll_delta_y = log_wheel_scroll_delta_y(ui, panel_rect);
        let mut force_scroll_to_bottom = self.auto_scroll.take_pending(LOG_SCROLL_ID);

        force_scroll_to_bottom |= self.level_filter_toolbar(ui);
        self.search_filter_toolbar(ui);

        force_scroll_to_bottom |=
            self.auto_scroll.enabled && crate::scroll_delta_moves_towards_bottom(scroll_delta_y);

        ui.separator();

        // ── 构建可见行列表 ──
        let scroll_to_row = self.begin_pending_navigation(ui);
        let rows = &self.visible_rows_cache;

        let outcome = render_log_rows(
            ui,
            rows,
            &LogRowsInputs {
                has_any_entries: !self.entry_order.is_empty(),
                scroll_to_row,
                stick_to_bottom: self.auto_scroll.enabled,
                force_scroll_to_bottom,
                wheel_scroll_delta_y: scroll_delta_y,
                font_size: self.font_size,
                visible_rows_generation: self.visible_rows_generation,
                navigate_highlight: self.navigate_highlight,
            },
            &mut self.selection,
            &mut self.text_selection_rows,
            &mut self.virtual_rows,
        );

        if let Some(id) = outcome.pending_navigate_to_id {
            self.pending_navigate_to_id = Some(id);
        }

        self.expire_navigate_highlight(ui);
        self.auto_scroll.update(
            ui,
            LOG_SCROLL_ID,
            outcome.inner_rect,
            outcome.content_height,
            outcome.offset_y,
            scroll_delta_y,
        );
    }

    /// ── 第一行：级别过滤 + 自动滚动 + 清空 ──
    /// 返回自动滚动按钮本帧是否要求强制滚到底。
    fn level_filter_toolbar(&mut self, ui: &mut egui::Ui) -> bool {
        let mut force_scroll_to_bottom = false;
        design::toolbar(ui, |ui| {
            self.level_filter_buttons(ui);

            ui.separator();

            force_scroll_to_bottom = self.auto_scroll.button(ui);

            self.export_menu_button(ui);

            self.clear_button(ui);
        });
        force_scroll_to_bottom
    }

    /// 级别过滤按钮：按固定字符宽度等宽占位，切换级别时不会把工具栏挤到换行。
    fn level_filter_buttons(&mut self, ui: &mut egui::Ui) {
        let padding = ui.spacing().button_padding.x * 2.0;
        let char_w = 10.0;
        let btn_w = padding + 5.0 * char_w + 4.0;

        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ] {
            ui.allocate_ui_with_layout(
                egui::vec2(btn_w, design::CONTROL_HEIGHT),
                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                |ui| {
                    if ui
                        .selectable_label(self.min_level == level, level.as_str())
                        .clicked()
                    {
                        self.min_level = level;
                    }
                },
            );
        }
    }

    /// 导出菜单：只记录本次请求的格式，真正的分块导出由调用方驱动。
    fn export_menu_button(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("导出", |ui| {
            for (label, format) in [
                ("导出 TXT…", TerminalExportFormat::Txt),
                ("导出 CSV…", TerminalExportFormat::Csv),
                ("导出 JSON…", TerminalExportFormat::Json),
            ] {
                if ui.button(label).clicked() {
                    self.export_request = Some(format);
                    ui.close();
                }
            }
        });
    }

    fn clear_button(&mut self, ui: &mut egui::Ui) {
        // 清空：两步确认（与终端面板一致），避免误触丢失系统日志。
        // 确认态只换等宽标签，不插入「取消」按钮，窄面板下工具栏不会重新换行；
        // 取消交给 3 秒超时 / Esc / 点击别处。
        let clear_armed = design::confirm_armed(ui, LOG_CLEAR_CONFIRM_ID);
        let clear_label = if clear_armed { "确认" } else { "清空" };
        let clear_color = if clear_armed {
            theme::red()
        } else {
            theme::text_primary()
        };
        let clear_response = ui.add(egui::Button::new(
            RichText::new(clear_label).color(clear_color),
        ));
        let clear_clicked = clear_response.clicked();
        if clear_armed {
            clear_response.on_hover_text("再次点击清空，3 秒内有效（Esc 或点击别处取消）");
        }
        if design::confirm_click(ui, LOG_CLEAR_CONFIRM_ID, clear_clicked) {
            self.clear();
        }
    }

    /// ── 第二行：搜索 + 来源过滤 ──
    fn search_filter_toolbar(&mut self, ui: &mut egui::Ui) {
        design::toolbar(ui, |ui| {
            ui.label("搜索");
            self.search.toolbar(ui, 120.0, "关键词", "区分大小写");

            ui.label("来源");
            // 该行会换行：ComboBox 必须按已知宽度占位（见 design::combo_slot）。
            design::combo_slot(ui, 100.0, |ui| {
                egui::ComboBox::from_id_salt("log-source-filter")
                    .width(100.0)
                    .selected_text(self.source_filter.as_deref().unwrap_or("全部"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.source_filter, None, "全部");
                        for name in self.source_names() {
                            ui.selectable_value(&mut self.source_filter, Some(name.clone()), &name);
                        }
                    });
            });

            if (self.search.is_active() || self.source_filter.is_some())
                && ui.small_button("清除筛选").clicked()
            {
                self.search.clear();
                self.source_filter = None;
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
    }

    /// 双击跳转的收尾：本帧清除搜索与过滤、关闭自动追踪，并算出要滚动到的行索引。
    fn begin_pending_navigation(&mut self, ui: &egui::Ui) -> Option<usize> {
        // 双击搜索结果 → 下一帧清除搜索、关闭自动追踪、显示全部、跳转到对应行
        if self.pending_navigate_to_id.is_some() && self.search.is_active() {
            self.search.clear();
            self.source_filter = None;
            self.auto_scroll.enabled = false;
        }

        self.refresh_visible_rows();
        let rows = &self.visible_rows_cache;

        // 获取跳转目标的 row 索引
        let taken_id = self.pending_navigate_to_id.take();
        let scroll_to_row =
            taken_id.and_then(|target_id| rows.iter().position(|entry| entry.id == target_id));
        if let Some(target_id) = taken_id {
            // 跳转生效：设置目标行高亮（起始时间用 egui 时钟）。
            self.navigate_highlight = Some((target_id, ui.ctx().input(|i| i.time)));
        }
        scroll_to_row
    }

    /// 高亮超时清理
    fn expire_navigate_highlight(&mut self, ui: &egui::Ui) {
        if let Some((_, start)) = self.navigate_highlight {
            let now = ui.ctx().input(|i| i.time);
            if now - start >= NAV_HIGHLIGHT_DURATION {
                self.navigate_highlight = None;
            }
        }
    }

    fn ingest(&mut self) -> usize {
        let mut count = 0;
        let mut batch = self.scrollback.take_batch(MAX_INGEST_PER_FRAME);
        for event in batch.drain(..) {
            self.push_event(event);
            count += 1;
        }
        self.scrollback.recycle_batch(batch);
        count
    }

    fn push_event(&mut self, event: Event) {
        let level = event
            .metadata
            .get("level")
            .and_then(|value| value.as_str())
            .and_then(|value| value.parse().ok())
            .unwrap_or(LogLevel::Info);

        let source = event
            .metadata
            .get("original_source")
            .and_then(|value| value.as_str())
            .unwrap_or(&event.source)
            .to_owned();

        let message = event.payload.text_lossy();

        let entry_id = self.next_entry_id;
        self.next_entry_id = self.next_entry_id.wrapping_add(1).max(1);

        let entry = LogEntry {
            id: entry_id,
            timestamp_label: format!("[{}]", fmt_ts(event.timestamp_ms)),
            level,
            source,
            message,
        };
        let filter_key = self.current_filter_key();
        let cache_is_current =
            !self.visible_rows_dirty && self.visible_filter_key.as_ref() == Some(&filter_key);
        if !cache_is_current {
            self.visible_rows_dirty = true;
        } else if log_entry_matches(&entry, &filter_key, &filter_key.query()) {
            self.visible_rows_cache.push_back(entry.clone());
            self.visible_rows_generation = self.visible_rows_generation.wrapping_add(1);
        }
        self.entry_order.push_back(entry_id);
        *self.source_counts.entry(entry.source.clone()).or_default() += 1;
        self.source_names_cache.insert(entry.source.clone());
        self.entries.insert(entry_id, entry);

        self.enforce_max_entries();
    }

    #[cfg(test)]
    fn push_test_entry(&mut self, entry: LogEntry) {
        let id = entry.id;
        self.entry_order.push_back(id);
        *self.source_counts.entry(entry.source.clone()).or_default() += 1;
        self.source_names_cache.insert(entry.source.clone());
        self.entries.insert(id, entry);
        self.visible_rows_dirty = true;
    }

    fn current_filter_key(&self) -> LogFilterKey {
        LogFilterKey {
            min_level: self.min_level,
            source_filter: self.source_filter.clone(),
            search_text: self.search.text.clone(),
            case_sensitive: self.search.case_sensitive,
        }
    }

    fn refresh_visible_rows(&mut self) {
        let key = self.current_filter_key();
        if self.visible_rows_dirty || self.visible_filter_key.as_ref() != Some(&key) {
            let query = key.query();
            self.visible_rows_cache = self
                .entry_order
                .iter()
                .filter_map(|id| {
                    self.entries
                        .get(id)
                        .filter(|entry| log_entry_matches(entry, &key, &query))
                        .cloned()
                })
                .collect();
            self.visible_filter_key = Some(key);
            self.visible_rows_dirty = false;
            self.visible_rows_generation = self.visible_rows_generation.wrapping_add(1);
        }
    }
}

impl LogFilterKey {
    /// 按这份过滤条件编译搜索词；逐行匹配前先编译一次，别每行都重编译。
    fn query(&self) -> crate::search::SearchQuery {
        crate::search::SearchQuery::new(&self.search_text, self.case_sensitive)
    }
}

/// 日志条目是否命中过滤条件。`query` 必须由调用方按同一份 `filter` 预先编译。
fn log_entry_matches(
    entry: &LogEntry,
    filter: &LogFilterKey,
    query: &crate::search::SearchQuery,
) -> bool {
    entry.level >= filter.min_level
        && filter
            .source_filter
            .as_ref()
            .is_none_or(|source| entry.source == *source)
        && (query.is_empty() || query.matches(&entry.source) || query.matches(&entry.message))
}

/// CSV 单元格：始终加双引号，内部的 `"` 双写转义。
fn csv_cell(value: &str) -> String {
    let escaped = value.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

// ── 渲染 ──

#[derive(Clone, Copy, Debug, PartialEq)]
struct LogTableWidths {
    label: f32,
    message: f32,
}

fn log_table_widths(full_width: f32, desired_label_width: f32) -> LogTableWidths {
    let full_width = full_width.max(0.0);
    let label = desired_label_width.min(full_width);
    let message = (full_width - label).max(0.0);

    LogTableWidths { label, message }
}

fn estimated_log_row_height(
    entry: &LogEntry,
    base_row_height: f32,
    message_width: f32,
    glyph_width: f32,
) -> f32 {
    (estimated_wrapped_line_count(&entry.message, message_width, glyph_width) as f32
        * base_row_height)
        .round()
        .max(base_row_height)
}

/// 一帧内日志行视图的只读输入：滚动意图、显示代次与跳转目标。
/// 这些值原先是 `render_log_rows` 的位置参数，相邻 bool
/// （`stick_to_bottom` / `force_scroll_to_bottom`）互换编译不报错、渲染却变；
/// 收进结构体后调用点按字段名书写，参数数量超限的 lint 豁免也随之删除。
/// 可变借用（`selection` / `text_selection_rows` / `virtual_rows`）不是这帧的输入，
/// 仍作为独立参数传入（模式与 terminal.rs 的 `RowsViewInputs` 一致）。
struct LogRowsInputs {
    has_any_entries: bool,
    scroll_to_row: Option<usize>,
    stick_to_bottom: bool,
    force_scroll_to_bottom: bool,
    wheel_scroll_delta_y: f32,
    font_size: f32,
    visible_rows_generation: u64,
    navigate_highlight: Option<(u64, f64)>,
}

/// 渲染日志消息流的行视图，返回滚动几何与双击跳转目标。
fn render_log_rows(
    ui: &mut egui::Ui,
    rows: &VecDeque<LogEntry>,
    inputs: &LogRowsInputs,
    selection: &mut RowSelection,
    text_selection_rows: &mut TextSelectionRows,
    virtual_rows: &mut VirtualRowIndex,
) -> LogRenderOutcome {
    let mut pending_navigate: Option<u64> = None;
    let font_id = egui::FontId::new(inputs.font_size, egui::FontFamily::Monospace);
    let base_row_height = ui.fonts_mut(|f| f.row_height(&font_id));
    selection.sync_rows_with_generation(
        rows.iter().map(|entry| entry.id),
        inputs.visible_rows_generation,
    );

    if rows.is_empty() {
        return render_empty_log_rows(ui, inputs.has_any_entries);
    }

    let scroll_output = ScrollArea::vertical()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .auto_shrink([false, false])
        .stick_to_bottom(inputs.stick_to_bottom)
        .id_salt(LOG_SCROLL_ID)
        .show(ui, |ui| {
            let frame =
                layout_log_row_frame(ui, rows, virtual_rows, &font_id, base_row_height, inputs);
            // 逐行绘制标签 + 可选择的文本
            let mut interaction = begin_log_rows_interaction(
                ui,
                rows,
                &frame,
                virtual_rows,
                selection,
                text_selection_rows,
            );

            let (render_range, text_selection_layout_range) =
                log_rows_render_range(rows, &frame, virtual_rows, text_selection_rows);
            let mut row_heights_changed = false;
            for row_idx in render_range.start..render_range.end.min(rows.len()) {
                let Some(entry) = rows.get(row_idx) else {
                    continue;
                };
                let rendered = layout_log_row(
                    ui,
                    entry,
                    row_idx,
                    &frame,
                    virtual_rows,
                    &text_selection_layout_range,
                );
                row_heights_changed |= rendered.heights_changed;
                paint_log_row(
                    ui,
                    entry,
                    &frame,
                    &mut interaction,
                    &rendered,
                    selection,
                    text_selection_rows,
                );
            }

            apply_log_row_heights_and_scroll(
                ui,
                virtual_rows,
                &frame,
                row_heights_changed,
                &interaction,
                selection,
                inputs.wheel_scroll_delta_y,
            );
            pending_navigate =
                resolve_log_row_interactions(ui, &mut interaction, &frame, rows, selection, inputs);
        });

    LogRenderOutcome {
        inner_rect: scroll_output.inner_rect,
        content_height: scroll_output.content_size.y,
        offset_y: scroll_output.state.offset.y,
        pending_navigate_to_id: pending_navigate,
    }
}

/// 一帧内所有日志行共用的排版输入与各列矩形。
struct LogRowFrame {
    font_id: egui::FontId,
    /// 面板字号（即 `font_id.size`）：列宽与 layout 哈希都由它推导。
    font_size: f32,
    base_row_height: f32,
    text_color: egui::Color32,
    galley_width: f32,
    text_padding: f32,
    message_width: f32,
    total_height: f32,
    full_rect: egui::Rect,
    label_rect: egui::Rect,
    message_rect: egui::Rect,
    viewport_rect: egui::Rect,
    blank_rect: Option<egui::Rect>,
    blank_response: Option<egui::Response>,
    /// 跳转目标行高亮：(目标行 id, 起始时间秒)，每行都要比对。
    navigate_highlight: Option<(u64, f64)>,
}

/// 列宽的基准字号：面板字号除以它得到缩放系数，两处列宽计算共用（不要再写字面的 13.0，
/// 否则改一处漏一处会让标签列宽度与行内列宽对不上）。
const LOG_FONT_BASE: f32 = 13.0;

fn log_col_scale(font_size: f32) -> f32 {
    font_size / LOG_FONT_BASE
}

/// 标签列总宽度：左内边距 + 时间/级别/来源三列（各带一个列间距）+ 标签到消息列的间距。
/// 逐项按同一 `scale` 累加，浮点求值顺序决定最终宽度，不能重排。
fn log_desired_label_width(font_size: f32) -> f32 {
    let scale = log_col_scale(font_size);
    ROW_LEFT_PADDING * scale
        + TIME_COL_WIDTH * scale
        + COL_GAP * scale
        + LEVEL_COL_WIDTH * scale
        + COL_GAP * scale
        + SOURCE_COL_WIDTH * scale
        + LABEL_TO_MSG_GAP * scale
}

/// 度量本帧的行区域：列宽、虚拟列表总高、各列矩形，以及下方空白区的 response。
fn layout_log_row_frame(
    ui: &mut egui::Ui,
    rows: &VecDeque<LogEntry>,
    virtual_rows: &mut VirtualRowIndex,
    font_id: &egui::FontId,
    base_row_height: f32,
    inputs: &LogRowsInputs,
) -> LogRowFrame {
    let (visible_rows_generation, navigate_highlight) =
        (inputs.visible_rows_generation, inputs.navigate_highlight);
    let font_size = font_id.size;
    let full_width = ui.available_width().max(0.0);
    let text_color = ui.style().visuals.text_color();
    let glyph_width = ui.fonts_mut(|fonts| fonts.glyph_width(font_id, '0'));

    let widths = log_table_widths(full_width, log_desired_label_width(font_size));
    let label_width = widths.label;
    let message_width = widths.message;
    let text_padding = 4.0;
    let galley_width = (message_width - text_padding).max(0.0).floor();

    let total_height = sync_log_virtual_rows(
        rows,
        virtual_rows,
        log_layout_key(galley_width, font_size, visible_rows_generation),
        base_row_height,
        galley_width,
        glyph_width,
    );

    // 分配总区域
    let (full_rect, _alloc_response) =
        ui.allocate_exact_size(egui::vec2(full_width, total_height), Sense::hover());

    // 标签区域
    let label_rect =
        egui::Rect::from_min_size(full_rect.left_top(), egui::vec2(label_width, total_height));

    // 消息区域
    let message_rect = egui::Rect::from_min_size(
        egui::pos2(full_rect.left() + label_width, full_rect.top()),
        egui::vec2(message_width, total_height),
    );
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
            ui.make_persistent_id(("log-blank", LOG_SCROLL_ID)),
            Sense::click(),
        )
    });

    LogRowFrame {
        font_id: font_id.clone(),
        font_size,
        base_row_height,
        text_color,
        galley_width,
        text_padding,
        message_width,
        total_height,
        full_rect,
        label_rect,
        message_rect,
        viewport_rect,
        blank_rect,
        blank_response,
        navigate_highlight,
    }
}

/// 本帧的交互状态：标签列与空白区合并出的 response、字符拖选期间累加的 response、
/// 主键是否按下、是否已有字符选区、框选产生的滚动量，以及逐行的几何记录。
struct LogRowsInteraction {
    hl: RowHighlight,
    ctx_response: egui::Response,
    text_drag_response: Option<egui::Response>,
    primary_down: bool,
    has_text_selection: bool,
    scroll_delta: f32,
}

/// 收集本帧的鼠标交互：悬停行、按下清选、字符级拖选端点、标签列的整行框选。
fn begin_log_rows_interaction(
    ui: &mut egui::Ui,
    rows: &VecDeque<LogEntry>,
    frame: &LogRowFrame,
    virtual_rows: &VirtualRowIndex,
    selection: &mut RowSelection,
    text_selection_rows: &mut TextSelectionRows,
) -> LogRowsInteraction {
    let label_rect = frame.label_rect;
    let hl = RowHighlight::new(ui, LOG_SCROLL_ID);
    let mut ctx_response = ui.interact(
        label_rect,
        ui.make_persistent_id(("log-metadata", LOG_SCROLL_ID)),
        Sense::click_and_drag(),
    );
    if let Some(response) = frame.blank_response.clone() {
        ctx_response |= response;
    }
    let hovered_idx = ui
        .input(|input| input.pointer.hover_pos())
        .filter(|position| label_rect.contains(*position))
        .and_then(|position| virtual_rows.index_at_offset(position.y - label_rect.top()));
    let message_pressed = log_primary_pressed_in_rect(ui, Some(frame.message_rect));
    let blank_pressed = log_primary_pressed_in_rect(ui, frame.blank_rect);
    if message_pressed || blank_pressed {
        selection.clear();
    }
    let primary_down = ui.input(|input| input.pointer.button_down(egui::PointerButton::Primary));
    let has_text_selection = refresh_log_text_selection(
        ui,
        rows,
        hovered_idx,
        message_pressed,
        primary_down,
        text_selection_rows,
    );
    let scroll_delta = handle_log_row_selection_input(
        ui,
        label_rect,
        hovered_idx,
        blank_pressed,
        selection,
        text_selection_rows,
    );

    LogRowsInteraction {
        hl,
        ctx_response,
        text_drag_response: None,
        primary_down,
        has_text_selection,
        scroll_delta,
    }
}

/// 本帧需要布局的行范围，以及字符选区端点所在行范围（后者要在布局期间保活）。
fn log_rows_render_range(
    rows: &VecDeque<LogEntry>,
    frame: &LogRowFrame,
    virtual_rows: &VirtualRowIndex,
    text_selection_rows: &mut TextSelectionRows,
) -> (
    std::ops::Range<usize>,
    Option<std::ops::RangeInclusive<usize>>,
) {
    let (viewport_rect, label_rect) = (frame.viewport_rect, frame.label_rect);
    let scroll_offset = (viewport_rect.top() - label_rect.top()).max(0.0);
    let visible_range = virtual_rows.visible_range(
        scroll_offset,
        viewport_rect.height(),
        frame.base_row_height * 2.0,
    );
    let text_selection_layout_range =
        text_selection_rows.layout_range(rows.iter().map(|entry| entry.id));
    let render_start = text_selection_layout_range
        .as_ref()
        .map_or(visible_range.start, |range| {
            visible_range.start.min(*range.start())
        });
    let render_end = text_selection_layout_range
        .as_ref()
        .map_or(visible_range.end, |range| {
            visible_range.end.max(range.end() + 1)
        });

    (render_start..render_end, text_selection_layout_range)
}

/// 单行的排版产物：行索引、纵向几何、消息列矩形与视口内排版的 galley。
struct LogRenderedRow {
    row_idx: usize,
    current_y: f32,
    entry_height: f32,
    label_y: f32,
    msg_row_rect: egui::Rect,
    /// `None` 表示该行落在视口外且不是选区端点行：只保留估算占位高度，本帧不绘制。
    message_galley: Option<std::sync::Arc<egui::Galley>>,
    /// 实测行高与估算值不同，需要请求下一帧重排。
    heights_changed: bool,
}

/// 排版一行：视口内实测 galley 并写回行高，视口外沿用虚拟索引里的估算高度。
fn layout_log_row(
    ui: &mut egui::Ui,
    entry: &LogEntry,
    row_idx: usize,
    frame: &LogRowFrame,
    virtual_rows: &mut VirtualRowIndex,
    text_selection_layout_range: &Option<std::ops::RangeInclusive<usize>>,
) -> LogRenderedRow {
    let current_y = frame.label_rect.top() + virtual_rows.row_top(row_idx);
    let estimated_height = virtual_rows.height(row_idx).max(frame.base_row_height);
    let in_text_selection = text_selection_layout_range
        .as_ref()
        .is_some_and(|range| range.contains(&row_idx));
    let viewport_rect = frame.viewport_rect;
    let in_viewport = in_text_selection
        || (current_y + estimated_height >= viewport_rect.top() - estimated_height
            && current_y <= viewport_rect.bottom() + estimated_height);
    // 标签对齐第一行中心（和终端面板一致）
    let label_y = current_y + frame.base_row_height * 0.5;

    let (message_galley, entry_height, heights_changed) = if in_viewport {
        let (galley, height) = layout_log_message_galley(
            ui,
            entry,
            &frame.font_id,
            frame.text_color,
            frame.galley_width,
            frame.base_row_height,
        );
        let changed = virtual_rows.set_height(row_idx, height);
        (Some(galley), height, changed)
    } else {
        (None, estimated_height, false)
    };
    let msg_row_rect = egui::Rect::from_min_size(
        egui::pos2(frame.message_rect.left(), current_y),
        egui::vec2(frame.message_width, entry_height),
    );

    LogRenderedRow {
        row_idx,
        current_y,
        entry_height,
        label_y,
        msg_row_rect,
        message_galley,
        heights_changed,
    }
}

/// 绘制一行：悬停/框选/跳转高亮、标签列与可选择的消息文本。
fn paint_log_row(
    ui: &mut egui::Ui,
    entry: &LogEntry,
    frame: &LogRowFrame,
    interaction: &mut LogRowsInteraction,
    rendered: &LogRenderedRow,
    selection: &mut RowSelection,
    text_selection_rows: &mut TextSelectionRows,
) {
    let Some(message_galley) = &rendered.message_galley else {
        return;
    };
    let (current_y, entry_height) = (rendered.current_y, rendered.entry_height);
    interaction
        .hl
        .record_row_at(rendered.row_idx, current_y, entry_height);

    // 高亮悬停行（框选模式下跳过）
    if !selection.has_selection() {
        interaction
            .hl
            .paint_background(ui, frame.full_rect, current_y, entry_height);
    }

    // 框选高亮
    if selection.is_selected(rendered.row_idx) {
        selection.paint(ui, frame.full_rect, current_y, entry_height);
    }

    // 跳转目标行高亮（叠在 selection/hover 之上，按剩余时间淡出）
    paint_log_navigate_highlight(
        ui,
        frame.full_rect,
        entry,
        frame.navigate_highlight,
        current_y,
        entry_height,
    );

    paint_log_row_labels(ui, entry, frame, rendered);

    // --- 可选择的消息文本 ---
    if frame.message_width > 0.0 {
        let (response, galley_pos, row_text_rect) = interact_log_message_row(
            ui,
            entry,
            message_galley,
            rendered.msg_row_rect,
            frame.text_padding,
            interaction.has_text_selection,
            interaction.primary_down,
        );
        handle_log_message_row_click(
            ui,
            &response,
            row_text_rect,
            rendered.msg_row_rect,
            rendered.row_idx,
            selection,
            text_selection_rows,
        );
        interaction.text_drag_response = Some(match interaction.text_drag_response.take() {
            Some(accumulated) => accumulated | response.clone(),
            None => response.clone(),
        });
        interaction.ctx_response |= response.clone();
        paint_log_message_text(
            ui,
            &response,
            galley_pos,
            message_galley,
            frame.text_color,
            selection.is_dragging(),
        );
    }
}

/// --- 标签列 ---：时间戳 / 级别 / 来源（来源按列宽与标签列右边界裁剪）。
fn paint_log_row_labels(
    ui: &egui::Ui,
    entry: &LogEntry,
    frame: &LogRowFrame,
    rendered: &LogRenderedRow,
) {
    let scale = log_col_scale(frame.font_size);
    let time_col_width = TIME_COL_WIDTH * scale;
    let level_col_width = LEVEL_COL_WIDTH * scale;
    let source_col_width = SOURCE_COL_WIDTH * scale;
    let col_gap = COL_GAP * scale;
    let row_left_padding = ROW_LEFT_PADDING * scale;

    let label_rect = frame.label_rect;
    let label_painter = ui.painter_at(label_rect);
    let font_id = &frame.font_id;
    let (current_y, entry_height, label_y) =
        (rendered.current_y, rendered.entry_height, rendered.label_y);

    let mut x = label_rect.left() + row_left_padding;

    // 时间戳
    label_painter.text(
        egui::pos2(x, label_y),
        egui::Align2::LEFT_CENTER,
        &entry.timestamp_label,
        font_id.clone(),
        theme::text_secondary(),
    );
    x += time_col_width + col_gap;

    // 级别
    label_painter.text(
        egui::pos2(x, label_y),
        egui::Align2::LEFT_CENTER,
        entry.level.as_str(),
        font_id.clone(),
        crate::level_color(entry.level),
    );
    x += level_col_width + col_gap;

    // 来源（裁剪）
    if x < label_rect.right() {
        let source_clip = egui::Rect::from_min_max(
            egui::pos2(x, current_y),
            egui::pos2(
                (x + source_col_width).min(label_rect.right()),
                current_y + entry_height,
            ),
        );
        let source_painter = label_painter.with_clip_rect(source_clip);
        let source_text = crate::compact_middle(&entry.source, SOURCE_TEXT_MAX_CHARS);
        source_painter.text(
            egui::pos2(x, label_y),
            egui::Align2::LEFT_CENTER,
            source_text,
            font_id.clone(),
            theme::cyan(),
        );
    }
}

/// 循环结束后：补齐实测行高，并应用拖选边缘滚动与滚轮滚动。
fn apply_log_row_heights_and_scroll(
    ui: &mut egui::Ui,
    virtual_rows: &mut VirtualRowIndex,
    frame: &LogRowFrame,
    row_heights_changed: bool,
    interaction: &LogRowsInteraction,
    selection: &RowSelection,
    wheel_scroll_delta_y: f32,
) {
    finish_log_row_heights(ui, virtual_rows, frame.total_height, row_heights_changed);

    apply_log_selection_scroll(
        ui,
        &interaction.text_drag_response,
        selection,
        frame.message_rect,
        frame.viewport_rect,
        wheel_scroll_delta_y,
        interaction.scroll_delta,
    );
}

/// 循环结束后统一解析本帧的行交互：冻结高亮行、右键单选、双击跳转、复制快捷键、
/// 右键菜单与滚到底。返回双击跳转的目标条目 id。
fn resolve_log_row_interactions(
    ui: &mut egui::Ui,
    interaction: &mut LogRowsInteraction,
    frame: &LogRowFrame,
    rows: &VecDeque<LogEntry>,
    selection: &mut RowSelection,
    inputs: &LogRowsInputs,
) -> Option<u64> {
    let (scroll_to_row, force_scroll_to_bottom) =
        (inputs.scroll_to_row, inputs.force_scroll_to_bottom);
    let (navigate_target, hovered_row) = resolve_log_row_clicks(
        ui,
        &mut interaction.hl,
        &interaction.ctx_response,
        rows,
        selection,
        frame.full_rect,
        scroll_to_row,
    );

    // 框选范围文本（移入 context_menu 闭包内按需构造，避免菜单未打开时每帧构造）
    let selected_indices = log_selected_row_indices(selection);

    handle_log_copy_shortcuts(ui, rows, selection, &selected_indices);

    interaction.ctx_response.context_menu(move |ctx_ui| {
        show_log_context_menu(ctx_ui, rows, &selected_indices, &hovered_row);
    });

    design::scroll_to_bottom_if_requested(ui, force_scroll_to_bottom);

    navigate_target
}

/// 框选中的行索引；无框选时为空，避免每帧为菜单构造选区文本。
fn log_selected_row_indices(selection: &RowSelection) -> Vec<usize> {
    if selection.has_selection() {
        selection.selected_indices().collect()
    } else {
        Vec::new()
    }
}

/// 空列表视图：无日志与无匹配共用同一条滚动路径，返回的几何与其它分支一致。
fn render_empty_log_rows(ui: &mut egui::Ui, has_any_entries: bool) -> LogRenderOutcome {
    let height = ui.available_height().max(40.0);
    let scroll_output = ScrollArea::vertical()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .auto_shrink([false, false])
        .id_salt(LOG_SCROLL_ID)
        .show(ui, |ui| {
            let (title, hint) = if has_any_entries {
                ("无匹配日志", "试着清除搜索或来源过滤")
            } else {
                ("暂无日志", "应用日志会显示在这里")
            };
            // 与终端接收区一致：内容铺满视口并居中，避免自动跟随持续重绘。
            ui.set_min_height(height);
            ui.add_space(((height - 96.0) * 0.5).clamp(8.0, 96.0));
            design::empty_state_compact_with_hint(ui, ICON_ARTICLE, title, hint);
        });

    LogRenderOutcome {
        inner_rect: scroll_output.inner_rect,
        content_height: scroll_output.content_size.y,
        offset_y: scroll_output.state.offset.y,
        pending_navigate_to_id: None,
    }
}

/// 虚拟列表的布局键：换行宽度、字号、可见行代次任一变化都要重建估算高度。
fn log_layout_key(galley_width: f32, font_size: f32, visible_rows_generation: u64) -> u64 {
    let width_key = galley_width.max(0.0).round() as u64;
    width_key
        ^ ((font_size * 1000.0).round() as u64).rotate_left(17)
        ^ visible_rows_generation.rotate_left(31)
}

/// 估算高度只在布局键或行数变化时重建，返回本帧占位用的总高度。
fn sync_log_virtual_rows(
    rows: &VecDeque<LogEntry>,
    virtual_rows: &mut VirtualRowIndex,
    layout_key: u64,
    base_row_height: f32,
    galley_width: f32,
    glyph_width: f32,
) -> f32 {
    if !virtual_rows.matches_layout(layout_key, rows.len()) {
        let row_ids: Vec<u64> = rows.iter().map(|entry| entry.id).collect();
        let estimated_heights: Vec<f32> = rows
            .iter()
            .map(|entry| {
                estimated_log_row_height(entry, base_row_height, galley_width, glyph_width)
            })
            .collect();
        virtual_rows.sync_rows(&row_ids, &estimated_heights, layout_key);
    }
    virtual_rows.total_height().max(base_row_height)
}

/// 主键落在 `rect` 内按下（`None` 表示下方没有空白区，直接判定为未按下）。
fn log_primary_pressed_in_rect(ui: &mut egui::Ui, rect: Option<egui::Rect>) -> bool {
    rect.is_some_and(|rect| {
        ui.input(|input| {
            input.pointer.button_pressed(egui::PointerButton::Primary)
                && input
                    .pointer
                    .hover_pos()
                    .is_some_and(|pos| rect.contains(pos))
        }) && ui.rect_contains_pointer(rect)
    })
}

/// 维护字符级拖选的行端点，返回本帧是否已有字符选区（决定消息行命中区是否扩到整行）。
fn refresh_log_text_selection(
    ui: &mut egui::Ui,
    rows: &VecDeque<LogEntry>,
    hovered_idx: Option<usize>,
    message_pressed: bool,
    primary_down: bool,
    text_selection_rows: &mut TextSelectionRows,
) -> bool {
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
    if message_pressed
        && let Some(index) = hovered_idx
        && let Some(entry) = rows.get(index)
    {
        text_selection_rows.begin(entry.id);
    }
    if primary_down
        && text_selection_rows.is_active()
        && let Some(index) = hovered_idx
        && let Some(entry) = rows.get(index)
    {
        text_selection_rows.update(entry.id);
    }
    has_text_selection
}

/// 标签列的整行框选输入：返回本帧累加到的滚动量，框选开始时清掉字符选区。
fn handle_log_row_selection_input(
    ui: &mut egui::Ui,
    label_rect: egui::Rect,
    hovered_idx: Option<usize>,
    blank_pressed: bool,
    selection: &mut RowSelection,
    text_selection_rows: &mut TextSelectionRows,
) -> f32 {
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
    scroll_delta
}

/// 排版一条消息的 galley，返回 galley 与实测行高（不小于基准行高）。
fn layout_log_message_galley(
    ui: &mut egui::Ui,
    entry: &LogEntry,
    font_id: &egui::FontId,
    text_color: egui::Color32,
    galley_width: f32,
    base_row_height: f32,
) -> (std::sync::Arc<egui::Galley>, f32) {
    let mut layout_job = egui::text::LayoutJob::simple(
        entry.message.clone(),
        font_id.clone(),
        text_color,
        galley_width,
    );
    layout_job.halign = egui::Align::LEFT;
    let galley = ui.fonts_mut(|f| f.layout_job(layout_job));
    let height = galley.size().y.max(base_row_height).round();
    (galley, height)
}

/// 跳转目标行高亮：剩余时长末段淡出，计时未结束时请求下一帧继续画。
fn paint_log_navigate_highlight(
    ui: &mut egui::Ui,
    full_rect: egui::Rect,
    entry: &LogEntry,
    navigate_highlight: Option<(u64, f64)>,
    current_y: f32,
    entry_height: f32,
) {
    if let Some((target_id, start)) = navigate_highlight
        && entry.id == target_id
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
}

/// 构造消息行的命中区与 `Response`，同时返回 galley 绘制位置与实际文本区域。
fn interact_log_message_row(
    ui: &mut egui::Ui,
    entry: &LogEntry,
    galley: &std::sync::Arc<egui::Galley>,
    msg_row_rect: egui::Rect,
    text_padding: f32,
    has_text_selection: bool,
    primary_down: bool,
) -> (egui::Response, egui::Pos2, egui::Rect) {
    // galley 从行顶开始绘制（和终端面板一致）
    let galley_pos = egui::pos2(msg_row_rect.left() + text_padding, msg_row_rect.top());
    // row_text_rect 只覆盖 galley 实际文本区域。点击文本 → egui 字符级拖选；
    // 点击文本外的空白 → 整行选中。
    let galley_size = galley.size();
    let row_text_rect = egui::Rect::from_min_size(galley_pos, galley_size);
    // 先构造 response：文本外空白分支（按下即选）与文本内 clicked 分支
    // （松开判定）都要用到它。
    let row_id = ui.make_persistent_id(("log-msg", entry.id));
    // 字符拖选已经开始后，将命中区扩展到整条消息行。这样从面板外
    // 移回来时，即使当前行较短、指针落在文字右侧，egui 也能把
    // 内部 cursor 从旧端点迁移到当前行。
    let text_interact_rect = if has_text_selection && primary_down {
        msg_row_rect
    } else {
        row_text_rect
    };
    let response = ui.interact(text_interact_rect, row_id, Sense::click_and_drag());
    (response, galley_pos, row_text_rect)
}

/// 整行选中（含 ctrl/shift 扩选），并清掉字符级选区，避免两套选区叠加。
fn select_log_whole_row(
    ui: &mut egui::Ui,
    row_idx: usize,
    ctrl: bool,
    shift: bool,
    selection: &mut RowSelection,
    text_selection_rows: &mut TextSelectionRows,
) {
    text_selection_rows.clear();
    selection.begin_pointer(row_idx, ctrl, shift);
    ui.ctx()
        .plugin::<LabelSelectionState>()
        .lock()
        .clear_selection();
}

/// 消息列上的按下/单击判定：文本外空白即时整行选中，文本内单击（未拖动）同样整行选中。
fn handle_log_message_row_click(
    ui: &mut egui::Ui,
    response: &egui::Response,
    row_text_rect: egui::Rect,
    msg_row_rect: egui::Rect,
    row_idx: usize,
    selection: &mut RowSelection,
    text_selection_rows: &mut TextSelectionRows,
) {
    let (primary_pressed, ctrl, shift) = ui.input(|i| {
        (
            i.pointer.button_pressed(egui::PointerButton::Primary),
            i.modifiers.ctrl || i.modifiers.command,
            i.modifiers.shift,
        )
    });
    // 文本外空白处按下 → 整行选中（即时反馈）。
    if primary_pressed
        && ui.rect_contains_pointer(msg_row_rect)
        && !ui.rect_contains_pointer(row_text_rect)
    {
        select_log_whole_row(ui, row_idx, ctrl, shift, selection, text_selection_rows);
    }
    // 文本内：松开且未拖动 → 整行选中。
    // response.clicked() 只有"按下→原地松开、未拖动"才为 true
    // （拖动超过阈值后松开走 drag，clicked 为 false，字符选区正常）。
    if response.clicked() && ui.rect_contains_pointer(row_text_rect) {
        select_log_whole_row(ui, row_idx, ctrl, shift, selection, text_selection_rows);
    }
}

/// 绘制消息文本：整行框选拖拽中直接画 galley，否则交给 egui 做字符级选区绘制。
fn paint_log_message_text(
    ui: &mut egui::Ui,
    response: &egui::Response,
    galley_pos: egui::Pos2,
    galley: &std::sync::Arc<egui::Galley>,
    text_color: egui::Color32,
    row_dragging: bool,
) {
    if row_dragging {
        ui.painter().add(egui::epaint::TextShape::new(
            galley_pos,
            galley.clone(),
            text_color,
        ));
    } else {
        LabelSelectionState::label_text_selection(
            ui,
            response,
            galley_pos,
            galley.clone(),
            text_color,
            Stroke::NONE,
        );
    }
}

/// 实测行高超过估算值时补齐占位高度，并在行高变化时请求下一帧。
fn finish_log_row_heights(
    ui: &mut egui::Ui,
    virtual_rows: &mut VirtualRowIndex,
    total_height: f32,
    row_heights_changed: bool,
) {
    let actual_total = virtual_rows.total_height().round();
    if row_heights_changed {
        ui.ctx().request_repaint();
    }
    if actual_total > total_height + 0.5 {
        ui.allocate_space(egui::vec2(0.0, actual_total - total_height));
    }
}

/// 拖选期间的边缘滚动与滚轮滚动，与普通滚动互不重复。
fn apply_log_selection_scroll(
    ui: &mut egui::Ui,
    text_drag_response: &Option<egui::Response>,
    selection: &RowSelection,
    message_rect: egui::Rect,
    viewport_rect: egui::Rect,
    wheel_scroll_delta_y: f32,
    mut scroll_delta: f32,
) {
    let text_selection_dragging = text_drag_response
        .as_ref()
        .is_some_and(|response| response.dragged_by(egui::PointerButton::Primary));
    if text_selection_dragging
        && let Some(pointer_y) = ui.input(|input| input.pointer.hover_pos().map(|pos| pos.y))
    {
        scroll_delta += edge_scroll_delta(pointer_y, viewport_rect.intersect(message_rect));
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
}

/// 循环结束后统一解析本帧的行交互：冻结高亮行、右键单选、双击跳转与悬停行文本。
/// 返回 `(双击跳转目标, 悬停行的整行文本与纯消息文本)`。
fn resolve_log_row_clicks(
    ui: &mut egui::Ui,
    hl: &mut RowHighlight,
    ctx_response: &egui::Response,
    rows: &VecDeque<LogEntry>,
    selection: &mut RowSelection,
    full_rect: egui::Rect,
    scroll_to_row: Option<usize>,
) -> (Option<u64>, Option<(String, String)>) {
    let mut navigate_target: Option<u64> = None;
    let frozen_row_idx = hl.resolve_click(
        ui,
        ctx_response,
        ui.make_persistent_id(("log-frozen-row", LOG_SCROLL_ID)),
    );
    if ctx_response.clicked_by(egui::PointerButton::Secondary)
        && let Some(index) = frozen_row_idx
        && !selection.is_selected(index)
    {
        selection.select_only(index);
    }
    // 双击任意位置（文字或空白）→ 离开搜索进入上下文。
    // 用全局 button_double_clicked + 整行 rect 命中，不再依赖只覆盖文本列的 ctx_response。
    let double_clicked = ui.input(|i| {
        i.pointer
            .button_double_clicked(egui::PointerButton::Primary)
    }) && ui.rect_contains_pointer(full_rect);
    if double_clicked
        && let Some(idx) = frozen_row_idx.or_else(|| hl.hover_index(ui))
        && let Some(entry) = rows.get(idx)
    {
        navigate_target = Some(entry.id);
    }
    // 跳转到目标行
    if let Some(target_row) = scroll_to_row
        && let Some((y_top, _)) = hl.row_y_range(target_row)
    {
        let target_rect = egui::Rect::from_min_size(egui::pos2(0.0, y_top), egui::vec2(1.0, 1.0));
        ui.scroll_to_rect(target_rect, Some(egui::Align::Center));
    }
    let hovered_row = if ctx_response.context_menu_opened()
        || ctx_response.clicked_by(egui::PointerButton::Secondary)
    {
        frozen_row_idx
    } else {
        hl.hover_index(ui)
    }
    .and_then(|idx| {
        rows.get(idx)
            .map(|entry| (log_display_line(entry), entry.message.clone()))
    });
    (navigate_target, hovered_row)
}

/// Ctrl+A 整行全选与 Ctrl+C 复制：两者都只在面板持有复制焦点且无 TextEdit 聚焦时生效。
fn handle_log_copy_shortcuts(
    ui: &mut egui::Ui,
    rows: &VecDeque<LogEntry>,
    selection: &mut RowSelection,
    selected_indices: &[usize],
) {
    // Ctrl+A 全选：无 TextEdit 聚焦时选中所有可见行。
    // 用 consume_key 消费事件，阻止 egui 的 LabelSelectionState 再对当前 galley
    // 做字符级 Ctrl+A 全选（会与整行多选冲突）。
    if owns_copy_focus(ui, COPY_OWNER)
        && !ui.ctx().text_edit_focused()
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::A))
    {
        selection.select_all();
    }

    // Ctrl+C 复制选中行：有选中、收到 Event::Copy、且无 TextEdit 聚焦时触发。
    // 复制 full（含时间戳/级别/来源前缀），与右键菜单"复制选中行"一致。
    // egui 0.35 把 Ctrl+C 转成 Event::Copy 事件，用 text_edit_focused 判断 TextEdit 聚焦。
    let copy_requested = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
    if !selected_indices.is_empty()
        && copy_requested
        && owns_copy_focus(ui, COPY_OWNER)
        && !ui.ctx().text_edit_focused()
        && let Some(full) = build_selected_log_full_text(rows, selected_indices)
    {
        copy_text_with_feedback(
            ui,
            full,
            format!(
                "已复制 {} 行日志（含时间、级别和来源）",
                selected_indices.len()
            ),
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
}

/// 右键菜单：选中行/悬停行/全部可见内容的复制项，文本按需构造。
fn show_log_context_menu(
    ctx_ui: &mut egui::Ui,
    rows: &VecDeque<LogEntry>,
    selected_indices: &[usize],
    hovered_row: &Option<(String, String)>,
) {
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
    if bulk_copy_button(ctx_ui, "log-selected-full", full_label, target_count) {
        let text = if selected_count > 0 {
            build_selected_log_full_text(rows, selected_indices)
        } else {
            hovered_row.as_ref().map(|(full, _)| full.clone())
        };
        if let Some(text) = text {
            copy_text_with_feedback(
                ctx_ui,
                text,
                format!("已复制 {target_count} 行日志（含时间、级别和来源）"),
            );
        }
        ctx_ui.close();
    }

    let message_label = if selected_count > 0 {
        format!("复制选中 {selected_count} 行消息")
    } else {
        "复制此行消息".to_owned()
    };
    if bulk_copy_button(ctx_ui, "log-selected-message", message_label, target_count) {
        let text = if selected_count > 0 {
            build_selected_log_message_text(rows, selected_indices)
        } else {
            hovered_row.as_ref().map(|(_, message)| message.clone())
        };
        if let Some(text) = text {
            copy_text_with_feedback(ctx_ui, text, format!("已复制 {target_count} 行日志消息"));
        }
        ctx_ui.close();
    }
    if target_count > 0 {
        ctx_ui.separator();
    }

    if bulk_copy_button(
        ctx_ui,
        "log-all-content",
        format!("复制全部可见内容（{} 行）", rows.len()),
        rows.len(),
    ) {
        let combined_text = build_log_all_visible_text(rows);
        copy_text_with_feedback(
            ctx_ui,
            combined_text,
            format!("已复制全部可见日志（{} 行）", rows.len()),
        );
        ctx_ui.close();
    }

    if bulk_copy_button(ctx_ui, "log-all-csv", "复制全部可见为 CSV", rows.len()) {
        let csv = build_log_all_visible_csv(rows);
        copy_text_with_feedback(ctx_ui, csv, format!("已复制日志 CSV（{} 行）", rows.len()));
        ctx_ui.close();
    }

    if bulk_copy_button(ctx_ui, "log-all-jsonl", "复制全部可见为 JSONL", rows.len()) {
        let jsonl = build_log_all_visible_jsonl(rows);
        copy_text_with_feedback(
            ctx_ui,
            jsonl,
            format!("已复制日志 JSONL（{} 行）", rows.len()),
        );
        ctx_ui.close();
    }
}

/// 全部可见行的整行文本（含时间、级别和来源），用于「复制全部可见内容」。
fn build_log_all_visible_text(rows: &VecDeque<LogEntry>) -> String {
    rows.iter()
        .map(log_display_line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// 全部可见行的剪贴板 CSV，用于「复制全部可见为 CSV」。
fn build_log_all_visible_csv(rows: &VecDeque<LogEntry>) -> String {
    let mut csv = String::from("time,level,source,message\n");
    for entry in rows {
        // 与导出文件的 CSV 唯一差别：把消息里的换行压成空格，保证一行一条记录。
        csv.push_str(
            &[
                csv_cell(&entry.timestamp_label),
                csv_cell(entry.level.as_str()),
                csv_cell(&entry.source),
                csv_cell(&entry.message.replace('\n', " ")),
            ]
            .join(","),
        );
        csv.push('\n');
    }
    csv
}

/// 全部可见行的剪贴板 JSONL（每行一条紧凑记录），用于「复制全部可见为 JSONL」。
fn build_log_all_visible_jsonl(rows: &VecDeque<LogEntry>) -> String {
    let mut jsonl = String::new();
    for entry in rows {
        if let Ok(line) = serde_json::to_string(&log_json_value(entry)) {
            jsonl.push_str(&line);
            jsonl.push('\n');
        }
    }
    jsonl
}

/// 构造选中日志行的完整文本（含时间、级别和来源）。
fn build_selected_log_full_text(
    rows: &VecDeque<LogEntry>,
    selected_indices: &[usize],
) -> Option<String> {
    if selected_indices.is_empty() {
        return None;
    }
    let full: String = selected_indices
        .iter()
        .map(|&index| &rows[index])
        .map(log_display_line)
        .collect::<Vec<_>>()
        .join("\n");
    Some(full)
}

/// 构造选中日志行的纯消息文本。
fn build_selected_log_message_text(
    rows: &VecDeque<LogEntry>,
    selected_indices: &[usize],
) -> Option<String> {
    if selected_indices.is_empty() {
        return None;
    }
    Some(
        selected_indices
            .iter()
            .map(|&index| &rows[index])
            .map(|entry| entry.message.clone())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_databus::DataBus;

    fn ordered_entries(panel: &LogPanel) -> Vec<&LogEntry> {
        panel
            .entry_order
            .iter()
            .filter_map(|id| panel.entries.get(id))
            .collect()
    }

    #[test]
    fn ingest_system_log_keeps_app_ready_entry() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);

        bus.publish(Event::system_log(LogLevel::Info, "app", "就绪"));

        assert_eq!(panel.ingest_all_pending(), 1);
        assert_eq!(panel.entry_order.len(), 1);

        let entry = ordered_entries(&panel)
            .first()
            .copied()
            .expect("log entry should be ingested");
        assert_eq!(entry.level, LogLevel::Info);
        assert_eq!(entry.source, "app");
        assert_eq!(entry.message, "就绪");
    }

    #[test]
    fn clear_drains_pending_log_events() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);

        bus.publish(Event::system_log(LogLevel::Info, "app", "stale"));
        panel.clear();

        assert_eq!(panel.ingest_all_pending(), 0);
        assert!(panel.entry_order.is_empty());
    }

    #[test]
    fn clear_resets_search_and_filter() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);

        panel.search.text = "error".into();
        panel.source_filter = Some("app".into());
        panel.clear();

        assert!(panel.search.text.is_empty());
        assert!(panel.source_filter.is_none());
        assert!(panel.entry_order.is_empty());
    }

    #[test]
    fn search_filters_by_source_and_message() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);

        panel.push_test_entry(LogEntry {
            id: 1,
            timestamp_label: "[12:00:00.000]".into(),
            level: LogLevel::Error,
            source: "transport.serial".into(),
            message: "read failed on COM3: timeout".into(),
        });
        panel.push_test_entry(LogEntry {
            id: 2,
            timestamp_label: "[12:00:01.000]".into(),
            level: LogLevel::Info,
            source: "app".into(),
            message: "就绪".into(),
        });

        panel.search.text = "com3".into();
        let rows: Vec<&LogEntry> = ordered_entries(&panel)
            .into_iter()
            .filter(|e| {
                e.source.to_ascii_lowercase().contains("com3")
                    || e.message.to_ascii_lowercase().contains("com3")
            })
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "transport.serial");

        panel.search.text = "app".into();
        let rows: Vec<&LogEntry> = ordered_entries(&panel)
            .into_iter()
            .filter(|e| {
                e.source.to_ascii_lowercase().contains("app")
                    || e.message.to_ascii_lowercase().contains("app")
            })
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message, "就绪");

        panel.search.clear();
        let rows = ordered_entries(&panel);
        assert_eq!(rows.len(), 2);
    }

    /// 搜索词按 Unicode 大小写不敏感命中 `Äpp`；导出的字节就是按该条件过滤后的
    /// 可见行（走活的游标分块路径）。
    #[test]
    fn unicode_case_insensitive_search_filters_exported_rows() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);
        panel.push_test_entry(LogEntry {
            id: 1,
            timestamp_label: "[12:00:00.000]".into(),
            level: LogLevel::Info,
            source: "Äpp".into(),
            message: "设备就绪".into(),
        });
        panel.push_test_entry(LogEntry {
            id: 2,
            timestamp_label: "[12:00:01.000]".into(),
            level: LogLevel::Info,
            source: "other".into(),
            message: "ignored".into(),
        });
        panel.search.text = "äpp".into();

        let expected_txt = "[12:00:00.000] info  Äpp                设备就绪";
        let expected_csv = concat!(
            "time,level,source,message\n",
            "\"[12:00:00.000]\",\"info\",\"Äpp\",\"设备就绪\"\n",
        );
        let expected_json = "[{\"level\":\"info\",\"message\":\"设备就绪\",\"source\":\"Äpp\",\"time\":\"[12:00:00.000]\"}]";

        for (format, expected) in [
            (TerminalExportFormat::Txt, expected_txt),
            (TerminalExportFormat::Csv, expected_csv),
            (TerminalExportFormat::Json, expected_json),
        ] {
            let mut cursor = panel.begin_export_cursor();
            let mut actual = String::new();
            loop {
                let (chunk, done, _) = panel.export_cursor_chunk(&mut cursor, format, 1);
                actual.push_str(&chunk);
                if done {
                    break;
                }
            }
            assert_eq!(
                actual, expected,
                "{format:?} 过滤后的导出字节与字面量不一致"
            );
        }
    }

    /// 分块导出逐字节钉死三种格式（小批次，批间边界最密）。
    /// 消息带逗号+引号：CSV 双写转义和 JSON 字符串转义也是钉死内容的一部分。
    #[test]
    fn cursor_export_pins_chunk_bytes_in_small_batches() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);
        for index in 0..3 {
            panel.push_test_entry(LogEntry {
                id: index + 1,
                timestamp_label: format!("[12:00:0{index}.000]"),
                level: LogLevel::Info,
                source: "cursor-test".into(),
                message: format!("message-{index}, \"quoted\""),
            });
        }

        // 字面量表：期望字节是在未改动树上、旧快照 oracle 等价用例仍绿时，
        // 从活的分块游标路径捕获的（第 35 轮）。时间戳在夹具里就是字面量，
        // 所以整段期望值不需要任何插值。
        let expected_txt = concat!(
            "[12:00:00.000] info  cursor-test        message-0, \"quoted\"\n",
            "[12:00:01.000] info  cursor-test        message-1, \"quoted\"\n",
            "[12:00:02.000] info  cursor-test        message-2, \"quoted\"\n",
        );
        let expected_csv = concat!(
            "time,level,source,message\n",
            "\"[12:00:00.000]\",\"info\",\"cursor-test\",\"message-0, \"\"quoted\"\"\"\n",
            "\"[12:00:01.000]\",\"info\",\"cursor-test\",\"message-1, \"\"quoted\"\"\"\n",
            "\"[12:00:02.000]\",\"info\",\"cursor-test\",\"message-2, \"\"quoted\"\"\"\n",
        );
        let expected_json = concat!(
            "[{\"level\":\"info\",\"message\":\"message-0, \\\"quoted\\\"\",\"source\":\"cursor-test\",\"time\":\"[12:00:00.000]\"},",
            "{\"level\":\"info\",\"message\":\"message-1, \\\"quoted\\\"\",\"source\":\"cursor-test\",\"time\":\"[12:00:01.000]\"},",
            "{\"level\":\"info\",\"message\":\"message-2, \\\"quoted\\\"\",\"source\":\"cursor-test\",\"time\":\"[12:00:02.000]\"}]",
        );

        for (format, expected) in [
            (TerminalExportFormat::Txt, expected_txt),
            (TerminalExportFormat::Csv, expected_csv),
            (TerminalExportFormat::Json, expected_json),
        ] {
            let mut cursor = panel.begin_export_cursor();
            let mut actual = String::new();
            loop {
                let (chunk, done, _) = panel.export_cursor_chunk(&mut cursor, format, 1);
                actual.push_str(&chunk);
                if done {
                    break;
                }
            }
            assert_eq!(actual, expected, "{format:?} 分块导出字节与字面量不一致");
        }

        let mut cursor = panel.begin_export_cursor();
        panel.push_test_entry(LogEntry {
            id: 4,
            timestamp_label: "[12:00:04.000]".into(),
            level: LogLevel::Info,
            source: "cursor-test".into(),
            message: "future".into(),
        });
        let mut rows = 0;
        loop {
            let (_, done, exported) =
                panel.export_cursor_chunk(&mut cursor, TerminalExportFormat::Txt, 1);
            rows += exported;
            if done {
                break;
            }
        }
        assert_eq!(rows, 3);
    }

    #[test]
    fn lowering_log_limit_trims_immediately() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);
        for index in 0..120 {
            panel.push_test_entry(LogEntry {
                id: index + 1,
                timestamp_label: "[12:00:00.000]".into(),
                level: LogLevel::Info,
                source: "test".into(),
                message: format!("message-{index}"),
            });
        }

        panel.set_max_entries(100);

        assert_eq!(panel.entry_order.len(), 100);
        assert_eq!(
            ordered_entries(&panel).first().unwrap().message,
            "message-20"
        );
    }

    #[test]
    fn source_filter_excludes_others() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);

        panel.push_test_entry(LogEntry {
            id: 1,
            timestamp_label: "[12:00:00.000]".into(),
            level: LogLevel::Warn,
            source: "ext".into(),
            message: "plugin time out".into(),
        });
        panel.push_test_entry(LogEntry {
            id: 2,
            timestamp_label: "[12:00:01.000]".into(),
            level: LogLevel::Info,
            source: "app".into(),
            message: "就绪".into(),
        });

        panel.source_filter = Some("app".into());
        let rows: Vec<&LogEntry> = ordered_entries(&panel)
            .into_iter()
            .filter(|e| e.source == "app")
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "app");

        panel.source_filter = None;
        let rows = ordered_entries(&panel);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn table_widths_do_not_exceed_available_width_when_narrow() {
        let widths = log_table_widths(96.0, 320.0);

        assert_eq!(widths.label, 96.0);
        assert_eq!(widths.message, 0.0);
        assert!(widths.label + widths.message <= 96.0);

        let widths = log_table_widths(360.0, 320.0);
        assert_eq!(widths.message, 40.0);
        assert!(widths.label + widths.message <= 360.0);
    }

    #[test]
    fn max_entries_truncates_oldest() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);
        *panel.max_entries_mut() = 3;

        for i in 0..5 {
            bus.publish(Event::system_log(
                LogLevel::Info,
                "test",
                format!("msg {i}"),
            ));
        }
        panel.ingest_all_pending();
        let rows = ordered_entries(&panel);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].message, "msg 2");
        assert_eq!(rows[2].message, "msg 4");
    }

    /// 与接收区同一契约：每轮越界只提醒一次，`clear()` 结束当前一轮。
    ///
    /// 满载后每条日志都裁掉一条最旧条目；若每条都重新置位 `truncated`，状态栏
    /// 每帧都会刷新「日志已截断」通知，清空后也停不下来。
    #[test]
    fn truncation_notice_fires_once_per_over_capacity_episode() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);
        panel.set_max_entries(100);
        let publish = |index: usize| {
            bus.publish(Event::system_log(
                LogLevel::Info,
                "test",
                format!("msg {index}"),
            ));
        };

        for index in 0..100 {
            publish(index);
        }
        panel.ingest_all_pending();
        assert!(!panel.truncated(), "装满但未越界不应提醒截断");

        let mut notices = 0;
        for index in 100..105 {
            publish(index);
            panel.ingest_all_pending();
            if panel.truncated() {
                notices += 1;
                panel.clear_truncated();
            }
        }
        assert_eq!(notices, 1, "同一轮满载只应提醒一次，实际 {notices} 次");

        panel.clear();
        assert!(!panel.truncated(), "清空应复位待发提醒");
        publish(200);
        panel.ingest_all_pending();
        assert!(!panel.truncated(), "清空后未越界不应提醒截断");

        for index in 201..302 {
            publish(index);
        }
        panel.ingest_all_pending();
        assert!(panel.truncated(), "清空后再次越界应重新提醒截断");
    }

    /// 与接收区同一契约：同一轮持续丢包只上报一次。
    #[test]
    fn data_loss_fires_once_per_drop_episode() {
        let bus = DataBus::new();
        let mut panel = LogPanel::new(&bus);
        let publish = |index: usize| {
            bus.publish(Event::system_log(
                LogLevel::Info,
                "test",
                format!("msg {index}"),
            ));
        };

        for index in 0..(crate::MESSAGE_EVENT_BUFFER_CAPACITY + 1_000) {
            publish(index);
        }
        assert_eq!(
            panel.take_dropped_events(1_000),
            1_000,
            "本轮第一次丢包要上报计数"
        );

        for index in 0..10 {
            publish(1_000_000 + index);
        }
        assert_eq!(
            panel.take_dropped_events(2_000),
            0,
            "持续丢包不应逐帧重复上报"
        );

        assert_eq!(
            panel.take_dropped_events(2_000 + crate::DROP_EPISODE_QUIET_MS),
            0,
            "静默窗口内没有新丢包时不应上报"
        );
        for index in 0..5 {
            publish(2_000_000 + index);
        }
        assert_eq!(
            panel.take_dropped_events(2_001 + crate::DROP_EPISODE_QUIET_MS),
            5,
            "新一轮丢包应重新上报"
        );
    }
}
