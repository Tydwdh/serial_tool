use crate::{
    MAX_INGEST_PER_FRAME, MESSAGE_EVENT_BUFFER_CAPACITY, TerminalExportFormat, fmt_ts,
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
use std::collections::{BTreeSet, HashMap, VecDeque};
use tool_core::{Event, LogLevel};
use tool_databus::{DataBus, RingSubscription, TopicFilter};

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
/// 日志面板最大保留条数（与终端面板一致）。
const MAX_LOG_ENTRIES: usize = 50_000;
/// 跳转目标行高亮总时长（秒）。
const NAV_HIGHLIGHT_DURATION: f64 = 1.5;
/// 跳转目标行高亮末段淡出时长（秒）。
const NAV_FADE: f64 = 0.3;

pub struct LogPanel {
    subscription: RingSubscription,
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
    pub max_entries: usize,
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
    /// 是否发生过截断（用于状态栏提示，显示后清除）
    pub truncated: bool,
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

pub struct LogExportJob {
    entries: Vec<LogEntry>,
}

impl LogExportJob {
    pub fn render(&self, format: TerminalExportFormat) -> String {
        match format {
            TerminalExportFormat::Txt => {
                let mut output = self
                    .entries
                    .iter()
                    .map(|entry| {
                        format!(
                            "{} {:<5} {:<18} {}",
                            entry.timestamp_label,
                            entry.level.as_str(),
                            entry.source,
                            entry.message
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !output.is_empty() {
                    output.push('\n');
                }
                output
            }
            TerminalExportFormat::Csv => {
                let mut output = "time,level,source,message\n".to_owned();
                for entry in &self.entries {
                    output.push_str(
                        &[
                            log_csv_cell(&entry.timestamp_label),
                            log_csv_cell(entry.level.as_str()),
                            log_csv_cell(&entry.source),
                            log_csv_cell(&entry.message),
                        ]
                        .join(","),
                    );
                    output.push('\n');
                }
                output
            }
            TerminalExportFormat::Json => {
                let values = self
                    .entries
                    .iter()
                    .map(|entry| {
                        serde_json::json!({
                            "time": entry.timestamp_label,
                            "level": entry.level.as_str(),
                            "source": entry.source,
                            "message": entry.message,
                        })
                    })
                    .collect::<Vec<_>>();
                serde_json::to_string_pretty(&values).unwrap_or_default()
            }
        }
    }
}

struct LogRenderOutcome {
    inner_rect: egui::Rect,
    content_height: f32,
    offset_y: f32,
}

impl LogPanel {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            subscription: bus
                .subscribe_ring_bounded(TopicFilter::prefix("log."), MESSAGE_EVENT_BUFFER_CAPACITY),
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
            max_entries: MAX_LOG_ENTRIES,
            pending_navigate_to_id: None,
            navigate_highlight: None,
            search: MessageSearch::default(),
            source_filter: None,
            font_size: 13.0,
            selection: RowSelection::new(0),
            text_selection_rows: TextSelectionRows::default(),
            virtual_rows: VirtualRowIndex::default(),
            truncated: false,
        }
    }
    pub fn ingest_all_pending(&mut self) -> usize {
        // 每帧最多摄入 2000 条，防止大量日志突发时 UI 卡顿
        const MAX_INGEST_ALL: usize = 2000;
        let mut count = 0;

        for event in self.subscription.drain_limited(MAX_INGEST_ALL) {
            self.push_event(event);
            count += 1;
        }

        count
    }
    pub fn clear(&mut self) {
        self.subscription.clear();
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

    pub fn take_dropped_events(&self) -> u64 {
        self.subscription.take_dropped_count()
    }

    pub fn take_export_request(&mut self) -> Option<TerminalExportFormat> {
        self.export_request.take()
    }

    pub fn export_job(&self) -> LogExportJob {
        LogExportJob {
            entries: self
                .collect_visible_entries()
                .into_iter()
                .cloned()
                .collect(),
        }
    }

    fn collect_visible_entries(&self) -> Vec<&LogEntry> {
        let search_key = self.search.query();
        self.entry_order
            .iter()
            .filter_map(|id| self.entries.get(id))
            .filter(|entry| entry.level >= self.min_level)
            .filter(|entry| {
                self.source_filter
                    .as_ref()
                    .is_none_or(|filter| entry.source == *filter)
            })
            .filter(|entry| {
                search_key.is_empty()
                    || self.search.matches(&entry.source, &search_key)
                    || self.search.matches(&entry.message, &search_key)
            })
            .collect()
    }

    pub fn export_visible_text(&self) -> String {
        let mut output = self
            .collect_visible_entries()
            .into_iter()
            .map(|entry| {
                format!(
                    "{} {:<5} {:<18} {}",
                    entry.timestamp_label,
                    entry.level.as_str(),
                    entry.source,
                    entry.message
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !output.is_empty() {
            output.push('\n');
        }
        output
    }

    pub fn export_visible_csv(&self) -> String {
        let mut output = "time,level,source,message\n".to_owned();
        for entry in self.collect_visible_entries() {
            output.push_str(
                &[
                    log_csv_cell(&entry.timestamp_label),
                    log_csv_cell(entry.level.as_str()),
                    log_csv_cell(&entry.source),
                    log_csv_cell(&entry.message),
                ]
                .join(","),
            );
            output.push('\n');
        }
        output
    }

    pub fn export_visible_json(&self) -> String {
        let values: Vec<serde_json::Value> = self
            .collect_visible_entries()
            .into_iter()
            .map(|entry| {
                serde_json::json!({
                    "time": entry.timestamp_label,
                    "level": entry.level.as_str(),
                    "source": entry.source,
                    "message": entry.message,
                })
            })
            .collect();
        serde_json::to_string_pretty(&values).expect("serializable log export values")
    }

    pub fn set_max_entries(&mut self, max_entries: usize) {
        self.max_entries = max_entries.max(100);
        self.enforce_max_entries();
    }

    fn enforce_max_entries(&mut self) {
        while self.entry_order.len() > self.max_entries {
            let Some(removed_id) = self.entry_order.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&removed_id)
                && let Some(count) = self.source_counts.get_mut(&removed.source)
            {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.source_counts.remove(&removed.source);
                    self.source_names_cache.remove(&removed.source);
                }
            }
            if !self.visible_rows_dirty
                && self
                    .visible_filter_key
                    .as_ref()
                    .is_some_and(|key| *key == self.current_filter_key())
                && self
                    .visible_rows_cache
                    .front()
                    .is_some_and(|entry| entry.id == removed_id)
            {
                self.visible_rows_cache.pop_front();
                self.visible_rows_generation = self.visible_rows_generation.wrapping_add(1);
            }
            self.truncated = true;
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let _new_entries = self.ingest();

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
        let mut force_scroll_to_bottom = self.auto_scroll.take_pending(LOG_SCROLL_ID);

        // ── 第一行：级别过滤 + 自动滚动 + 清空 ──
        ui.horizontal_wrapped(|ui| {
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
                    egui::vec2(btn_w, 30.0),
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

            ui.separator();

            force_scroll_to_bottom |= self.auto_scroll.button(ui);

            ui.menu_button("导出", |ui| {
                if ui.button("导出 TXT…").clicked() {
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

            // 清空：两步确认（与终端面板一致），避免误触丢失系统日志。
            let clear_id = ui.id().with("log_clear_armed_ts");
            let now = ui.input(|i| i.time);
            let armed_ts: Option<f64> = ui.ctx().memory(|m| m.data.get_temp(clear_id));
            let armed = armed_ts.is_some_and(|t| now - t < 3.0);
            let clear_label = if armed { "确认清空?" } else { "清空" };
            let clear_btn = egui::Button::new(egui::RichText::new(clear_label).color(if armed {
                crate::theme::red()
            } else {
                crate::theme::text_primary()
            }));
            if ui.add(clear_btn).clicked() {
                if armed {
                    self.clear();
                    ui.ctx().memory_mut(|m| m.data.remove_temp::<f64>(clear_id));
                } else {
                    ui.ctx().memory_mut(|m| m.data.insert_temp(clear_id, now));
                }
            }
            if armed && ui.small_button("取消").clicked() {
                ui.ctx().memory_mut(|m| m.data.remove_temp::<f64>(clear_id));
            }
        });

        // ── 第二行：搜索 + 来源过滤 ──
        ui.horizontal_wrapped(|ui| {
            ui.label("搜索");
            self.search.toolbar(ui, 120.0, "关键词", "区分大小写");

            ui.label("来源");
            egui::ComboBox::from_id_salt("log-source-filter")
                .width(100.0)
                .selected_text(self.source_filter.as_deref().unwrap_or("全部"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.source_filter, None, "全部");
                    for name in self.source_names() {
                        ui.selectable_value(&mut self.source_filter, Some(name.clone()), &name);
                    }
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

        force_scroll_to_bottom |= self.auto_scroll.enabled && wheel_moves_towards_bottom;

        ui.separator();

        // ── 构建可见行列表 ──
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
        let scroll_to_row: Option<usize> =
            taken_id.and_then(|target_id| rows.iter().position(|entry| entry.id == target_id));
        if let Some(target_id) = taken_id {
            // 跳转生效：设置目标行高亮（起始时间用 egui 时钟）。
            self.navigate_highlight = Some((target_id, ui.ctx().input(|i| i.time)));
        }

        let mut navigate_id: Option<u64> = None;

        let outcome = render_log_rows(
            ui,
            rows,
            !self.entry_order.is_empty(),
            scroll_to_row,
            &mut navigate_id,
            self.auto_scroll.enabled,
            force_scroll_to_bottom,
            scroll_delta_y,
            self.font_size,
            self.visible_rows_generation,
            &mut self.selection,
            &mut self.text_selection_rows,
            &mut self.virtual_rows,
            self.navigate_highlight,
        );

        if navigate_id.is_some() {
            self.pending_navigate_to_id = navigate_id;
        }

        // 高亮超时清理
        if let Some((_, start)) = self.navigate_highlight {
            let now = ui.ctx().input(|i| i.time);
            if now - start >= NAV_HIGHLIGHT_DURATION {
                self.navigate_highlight = None;
            }
        }
        self.auto_scroll.update(
            ui,
            LOG_SCROLL_ID,
            outcome.inner_rect,
            outcome.content_height,
            outcome.offset_y,
            scroll_delta_y,
        );
    }

    fn ingest(&mut self) -> usize {
        let mut count = 0;

        for event in self.subscription.drain_limited(MAX_INGEST_PER_FRAME) {
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
        let cache_is_current = !self.visible_rows_dirty
            && self
                .visible_filter_key
                .as_ref()
                .is_some_and(|key| *key == self.current_filter_key());
        if cache_is_current {
            let filter_key = self.current_filter_key();
            if self.matches_filter(&entry, &filter_key) {
                self.visible_rows_cache.push_back(entry.clone());
                self.visible_rows_generation = self.visible_rows_generation.wrapping_add(1);
            }
        } else {
            self.visible_rows_dirty = true;
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

    fn matches_filter(&self, entry: &LogEntry, key: &LogFilterKey) -> bool {
        if entry.level < key.min_level {
            return false;
        }
        if key
            .source_filter
            .as_ref()
            .is_some_and(|filter| entry.source != *filter)
        {
            return false;
        }
        let query = crate::search::SearchQuery::new(&key.search_text, key.case_sensitive);
        query.is_empty() || query.matches(&entry.source) || query.matches(&entry.message)
    }

    fn refresh_visible_rows(&mut self) {
        let key = self.current_filter_key();
        if self.visible_rows_dirty || self.visible_filter_key.as_ref() != Some(&key) {
            let query = crate::search::SearchQuery::new(&key.search_text, key.case_sensitive);
            self.visible_rows_cache = self
                .entry_order
                .iter()
                .filter_map(|id| {
                    self.entries
                        .get(id)
                        .filter(|entry| {
                            entry.level >= key.min_level
                                && key
                                    .source_filter
                                    .as_ref()
                                    .is_none_or(|filter| entry.source == *filter)
                                && (query.is_empty()
                                    || query.matches(&entry.source)
                                    || query.matches(&entry.message))
                        })
                        .cloned()
                })
                .collect();
            self.visible_filter_key = Some(key);
            self.visible_rows_dirty = false;
            self.visible_rows_generation = self.visible_rows_generation.wrapping_add(1);
        }
    }
}

fn log_csv_cell(value: &str) -> String {
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

#[allow(clippy::too_many_arguments)]
fn render_log_rows(
    ui: &mut egui::Ui,
    rows: &VecDeque<LogEntry>,
    has_any_entries: bool,
    scroll_to_row: Option<usize>,
    pending_navigate: &mut Option<u64>,
    stick_to_bottom: bool,
    force_scroll_to_bottom: bool,
    wheel_scroll_delta_y: f32,
    font_size: f32,
    visible_rows_generation: u64,
    selection: &mut RowSelection,
    text_selection_rows: &mut TextSelectionRows,
    virtual_rows: &mut VirtualRowIndex,
    navigate_highlight: Option<(u64, f64)>,
) -> LogRenderOutcome {
    let font_id = egui::FontId::new(font_size, egui::FontFamily::Monospace);
    let base_row_height = ui.fonts_mut(|f| f.row_height(&font_id));
    selection.sync_rows_with_generation(rows.iter().map(|entry| entry.id), visible_rows_generation);

    // 列宽随字体大小缩放（基准 13px）
    let scale = font_size / 13.0;
    let time_col_width = TIME_COL_WIDTH * scale;
    let level_col_width = LEVEL_COL_WIDTH * scale;
    let source_col_width = SOURCE_COL_WIDTH * scale;
    let col_gap = COL_GAP * scale;
    let row_left_padding = ROW_LEFT_PADDING * scale;
    let label_to_msg_gap = LABEL_TO_MSG_GAP * scale;

    if rows.is_empty() {
        let scroll_output = ScrollArea::vertical()
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .auto_shrink([false, false])
            .id_salt(LOG_SCROLL_ID)
            .show(ui, |ui| {
                let hint = if has_any_entries {
                    "无匹配日志 · 试着清除搜索或来源过滤"
                } else {
                    "应用日志会显示在这里"
                };
                ui.label(RichText::new(hint).color(theme::text_secondary()));
            });

        return LogRenderOutcome {
            inner_rect: scroll_output.inner_rect,
            content_height: scroll_output.content_size.y,
            offset_y: scroll_output.state.offset.y,
        };
    }

    let scroll_output = ScrollArea::vertical()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .auto_shrink([false, false])
        .stick_to_bottom(stick_to_bottom)
        .id_salt(LOG_SCROLL_ID)
        .show(ui, |ui| {
            let full_width = ui.available_width().max(0.0);
            let font_id = egui::FontId::new(font_size, egui::FontFamily::Monospace);
            let text_color = ui.style().visuals.text_color();
            let glyph_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font_id, '0'));

            // 标签列总宽度
            let desired_label_width = row_left_padding
                + time_col_width
                + col_gap
                + level_col_width
                + col_gap
                + source_col_width
                + label_to_msg_gap;
            let widths = log_table_widths(full_width, desired_label_width);
            let label_width = widths.label;
            let message_width = widths.message;
            let text_padding = 4.0;
            let galley_width = (message_width - text_padding).max(0.0).floor();

            let width_key = galley_width.max(0.0).round() as u64;
            let layout_key = width_key
                ^ ((font_size * 1000.0).round() as u64).rotate_left(17)
                ^ visible_rows_generation.rotate_left(31);
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
            let total_height = virtual_rows.total_height().max(base_row_height);

            // 分配总区域
            let (full_rect, _alloc_response) =
                ui.allocate_exact_size(egui::vec2(full_width, total_height), Sense::hover());

            // 标签区域
            let label_rect = egui::Rect::from_min_size(
                full_rect.left_top(),
                egui::vec2(label_width, total_height),
            );
            let label_painter = ui.painter_at(label_rect);

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

            // 逐行绘制标签 + 可选择的文本
            let mut hl = RowHighlight::new(ui, LOG_SCROLL_ID);

            let mut ctx_response = ui.interact(
                label_rect,
                ui.make_persistent_id(("log-metadata", LOG_SCROLL_ID)),
                Sense::click_and_drag(),
            );
            if let Some(response) = blank_response {
                ctx_response |= response;
            }
            let hovered_idx = ui
                .input(|input| input.pointer.hover_pos())
                .filter(|position| label_rect.contains(*position))
                .and_then(|position| virtual_rows.index_at_offset(position.y - label_rect.top()));
            let message_pressed = ui.input(|input| {
                input.pointer.button_pressed(egui::PointerButton::Primary)
                    && input
                        .pointer
                        .hover_pos()
                        .is_some_and(|pos| message_rect.contains(pos))
            }) && ui.rect_contains_pointer(message_rect);
            let blank_pressed = blank_rect.is_some_and(|rect| {
                ui.input(|input| {
                    input.pointer.button_pressed(egui::PointerButton::Primary)
                        && input
                            .pointer
                            .hover_pos()
                            .is_some_and(|pos| rect.contains(pos))
                }) && ui.rect_contains_pointer(rect)
            });
            if message_pressed || blank_pressed {
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

            let viewport_rect = ui.clip_rect();
            let scroll_offset = (viewport_rect.top() - label_rect.top()).max(0.0);
            let visible_range = virtual_rows.visible_range(
                scroll_offset,
                viewport_rect.height(),
                base_row_height * 2.0,
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

            let mut text_drag_response: Option<egui::Response> = None;
            let mut row_heights_changed = false;
            for row_idx in render_start..render_end.min(rows.len()) {
                let Some(entry) = rows.get(row_idx) else {
                    continue;
                };
                let current_y = label_rect.top() + virtual_rows.row_top(row_idx);
                let estimated_height = virtual_rows.height(row_idx).max(base_row_height);
                let in_text_selection = text_selection_layout_range
                    .as_ref()
                    .is_some_and(|range| range.contains(&row_idx));
                let in_viewport = in_text_selection
                    || (current_y + estimated_height >= viewport_rect.top() - estimated_height
                        && current_y <= viewport_rect.bottom() + estimated_height);
                let (message_galley, entry_height) = if in_viewport {
                    let mut layout_job = egui::text::LayoutJob::simple(
                        entry.message.clone(),
                        font_id.clone(),
                        text_color,
                        galley_width,
                    );
                    layout_job.halign = egui::Align::LEFT;
                    let galley = ui.fonts_mut(|f| f.layout_job(layout_job));
                    let height = galley.size().y.max(base_row_height).round();
                    row_heights_changed |= virtual_rows.set_height(row_idx, height);
                    (Some(galley), height)
                } else {
                    (None, estimated_height)
                };

                if !in_viewport {
                    continue;
                }
                hl.record_row_at(row_idx, current_y, entry_height);
                // 标签对齐第一行中心（和终端面板一致）
                let label_y = current_y + base_row_height * 0.5;

                // 高亮悬停行（框选模式下跳过）
                if !selection.has_selection() {
                    hl.paint_background(ui, full_rect, current_y, entry_height);
                }

                // 框选高亮
                if selection.is_selected(row_idx) {
                    selection.paint(ui, full_rect, current_y, entry_height);
                }

                // 跳转目标行高亮（叠在 selection/hover 之上，按剩余时间淡出）
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

                // --- 标签列 ---
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

                // --- 可选择的消息文本 ---
                if let Some(message_galley) = message_galley
                    && message_width > 0.0
                {
                    // galley 从行顶开始绘制（和终端面板一致）
                    let galley_pos = egui::pos2(message_rect.left() + text_padding, current_y);
                    // row_text_rect 只覆盖 galley 实际文本区域。点击文本 → egui 字符级拖选；
                    // 点击文本外的空白 → 整行选中。
                    let galley_size = message_galley.size();
                    let row_text_rect = egui::Rect::from_min_size(galley_pos, galley_size);
                    let msg_row_rect = egui::Rect::from_min_size(
                        egui::pos2(message_rect.left(), current_y),
                        egui::vec2(message_width, entry_height),
                    );
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
                        text_selection_rows.clear();
                        selection.begin_pointer(row_idx, ctrl, shift);
                        ui.ctx()
                            .plugin::<LabelSelectionState>()
                            .lock()
                            .clear_selection();
                    }
                    // 文本内：松开且未拖动 → 整行选中。
                    // response.clicked() 只有"按下→原地松开、未拖动"才为 true
                    // （拖动超过阈值后松开走 drag，clicked 为 false，字符选区正常）。
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
                            message_galley.clone(),
                            text_color,
                        ));
                    } else {
                        LabelSelectionState::label_text_selection(
                            ui,
                            &response,
                            galley_pos,
                            message_galley.clone(),
                            text_color,
                            Stroke::NONE,
                        );
                    }
                }
            }

            let actual_total = virtual_rows.total_height().round();
            if row_heights_changed {
                ui.ctx().request_repaint();
            }
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

            let frozen_row_idx = hl.resolve_click(
                ui,
                &ctx_response,
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
                *pending_navigate = Some(entry.id);
            }
            // 跳转到目标行
            if let Some(target_row) = scroll_to_row
                && let Some((y_top, _)) = hl.row_y_range(target_row)
            {
                let target_rect =
                    egui::Rect::from_min_size(egui::pos2(0.0, y_top), egui::vec2(1.0, 1.0));
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
                rows.get(idx).map(|entry| {
                    let line = format!(
                        "{} {} {} {}",
                        entry.timestamp_label,
                        entry.level.as_str(),
                        entry.source,
                        entry.message
                    );
                    (line, entry.message.clone())
                })
            });

            // 框选范围文本（移入 context_menu 闭包内按需构造，避免菜单未打开时每帧构造）
            let selected_indices: Vec<usize> = if selection.has_selection() {
                selection.selected_indices().collect()
            } else {
                Vec::new()
            };

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
            let copy_requested =
                ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
            if !selected_indices.is_empty()
                && copy_requested
                && owns_copy_focus(ui, COPY_OWNER)
                && !ui.ctx().text_edit_focused()
                && let Some(full) = build_selected_log_full_text(rows, &selected_indices)
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
                if bulk_copy_button(ctx_ui, "log-selected-full", full_label, target_count) {
                    let text = if selected_count > 0 {
                        build_selected_log_full_text(rows, &selected_indices)
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
                        build_selected_log_message_text(rows, &selected_indices)
                    } else {
                        hovered_row.as_ref().map(|(_, message)| message.clone())
                    };
                    if let Some(text) = text {
                        copy_text_with_feedback(
                            ctx_ui,
                            text,
                            format!("已复制 {target_count} 行日志消息"),
                        );
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
                    let combined_text: String = rows
                        .iter()
                        .map(|entry| {
                            format!(
                                "{} {} {} {}",
                                entry.timestamp_label,
                                entry.level.as_str(),
                                entry.source,
                                entry.message
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    copy_text_with_feedback(
                        ctx_ui,
                        combined_text,
                        format!("已复制全部可见日志（{} 行）", rows.len()),
                    );
                    ctx_ui.close();
                }

                if bulk_copy_button(ctx_ui, "log-all-csv", "复制全部可见为 CSV", rows.len())
                {
                    let mut csv = String::from("time,level,source,message\n");
                    for entry in rows {
                        csv.push_str(&csv_cell(&entry.timestamp_label));
                        csv.push(',');
                        csv.push_str(&csv_cell(entry.level.as_str()));
                        csv.push(',');
                        csv.push_str(&csv_cell(&entry.source));
                        csv.push(',');
                        csv.push_str(&csv_cell(&entry.message.replace('\n', " ")));
                        csv.push('\n');
                    }
                    copy_text_with_feedback(
                        ctx_ui,
                        csv,
                        format!("已复制日志 CSV（{} 行）", rows.len()),
                    );
                    ctx_ui.close();
                }

                if bulk_copy_button(ctx_ui, "log-all-jsonl", "复制全部可见为 JSONL", rows.len())
                {
                    let mut jsonl = String::new();
                    for entry in rows {
                        let obj = serde_json::json!({
                            "time": entry.timestamp_label,
                            "level": entry.level.as_str(),
                            "source": entry.source,
                            "message": entry.message,
                        });
                        if let Ok(line) = serde_json::to_string(&obj) {
                            jsonl.push_str(&line);
                            jsonl.push('\n');
                        }
                    }
                    copy_text_with_feedback(
                        ctx_ui,
                        jsonl,
                        format!("已复制日志 JSONL（{} 行）", rows.len()),
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

    LogRenderOutcome {
        inner_rect: scroll_output.inner_rect,
        content_height: scroll_output.content_size.y,
        offset_y: scroll_output.state.offset.y,
    }
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
        .map(|entry| {
            format!(
                "{} {} {} {}",
                entry.timestamp_label,
                entry.level.as_str(),
                entry.source,
                entry.message
            )
        })
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

fn csv_cell(s: &str) -> String {
    let escaped = s.replace('"', "\"\"");
    format!("\"{escaped}\"")
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

    #[test]
    fn unicode_case_insensitive_search_and_exports_use_visible_rows() {
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

        let visible = panel.collect_visible_entries();
        assert_eq!(visible.len(), 1);
        assert!(panel.export_visible_text().contains("设备就绪"));
        assert!(!panel.export_visible_text().contains("ignored"));
        assert!(panel.export_visible_csv().contains("\"Äpp\""));
        let json: serde_json::Value =
            serde_json::from_str(&panel.export_visible_json()).expect("valid JSON");
        assert_eq!(json.as_array().unwrap().len(), 1);
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
        panel.max_entries = 3;

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
}
