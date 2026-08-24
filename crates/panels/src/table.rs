//! 表格行渲染公共工具：行高亮、行选择、右键菜单行匹配、冻结状态管理。
//!
//! `RowHighlight` 被日志面板和终端面板共用，消除重复代码。
//! `RowSelection` 提供框选、Shift/Ctrl 多选、边缘自动滚动。

use std::collections::{BTreeMap, BTreeSet};

use egui::{Id, Ui};

use crate::{design, theme};
use egui_material_icons::icons::{ICON_PAUSE, ICON_VERTICAL_ALIGN_BOTTOM};

const COPY_FOCUS_ID: &str = "message-copy-focus";
const COPY_FEEDBACK_ID: &str = "copy-feedback";
const BULK_COPY_CONFIRM_ROWS: usize = 10_000;

#[derive(Clone, Default)]
struct CopyFeedback(String);

/// 把最近一次发生选择交互的面板设为键盘复制目标。
pub(crate) fn claim_copy_focus(ui: &Ui, owner: &'static str) {
    ui.ctx().data_mut(|data| {
        data.insert_temp(Id::new(COPY_FOCUS_ID), owner.to_owned());
    });
}

pub(crate) fn owns_copy_focus(ui: &Ui, owner: &'static str) -> bool {
    ui.ctx()
        .data(|data| data.get_temp::<String>(Id::new(COPY_FOCUS_ID)))
        .is_some_and(|current| current == owner)
}

/// 统一写剪贴板并排队一条应用级成功提示。
pub fn copy_text_with_feedback(ui: &Ui, text: impl Into<String>, message: impl Into<String>) {
    ui.ctx().copy_text(text.into());
    report_copy_feedback(ui, message);
}

/// 用于 egui 自己完成剪贴板写入的字符级文本选择，只补统一反馈。
pub(crate) fn report_copy_feedback(ui: &Ui, message: impl Into<String>) {
    ui.ctx().data_mut(|data| {
        data.insert_temp(Id::new(COPY_FEEDBACK_ID), CopyFeedback(message.into()));
    });
}

/// 由应用壳每帧提取一次，转入现有通知/Toast 系统。
pub fn take_copy_feedback(ctx: &egui::Context) -> Option<String> {
    ctx.data_mut(|data| {
        data.remove_temp::<CopyFeedback>(Id::new(COPY_FEEDBACK_ID))
            .map(|feedback| feedback.0)
    })
}

pub(crate) fn bulk_copy_requires_confirmation(row_count: usize) -> bool {
    row_count > BULK_COPY_CONFIRM_ROWS
}

/// 大批量同步导出前要求二次确认，避免一次误点在 UI 线程构造数万行文本。
pub(crate) fn bulk_copy_button(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    label: impl Into<egui::WidgetText>,
    row_count: usize,
) -> bool {
    let label = label.into();
    if row_count == 0 {
        ui.add_enabled(false, egui::Button::new(label));
        return false;
    }
    if !bulk_copy_requires_confirmation(row_count) {
        return ui.button(label).clicked();
    }

    let armed_id = ui.id().with(("bulk-copy-confirm", id_salt));
    let now = ui.input(|input| input.time);
    let armed_at = ui.ctx().data(|data| data.get_temp::<f64>(armed_id));
    let armed = armed_at.is_some_and(|time| now - time < 5.0);
    let button_label: egui::WidgetText = if armed {
        format!("确认复制 {row_count} 行？").into()
    } else {
        label
    };
    let response = ui.button(button_label).on_hover_text(format!(
        "数据量较大（{row_count} 行），首次点击后需要再次确认"
    ));
    if !response.clicked() {
        return false;
    }
    if armed {
        ui.ctx().data_mut(|data| data.remove_temp::<f64>(armed_id));
        true
    } else {
        ui.ctx().data_mut(|data| data.insert_temp(armed_id, now));
        false
    }
}

/// 日志与终端共用的搜索状态和工具栏。
///
/// 两种消息流只需要决定“搜索哪些字段”，搜索文本、大小写规则和交互保持一致。
#[derive(Default)]
pub(crate) struct MessageSearch {
    pub(crate) text: String,
    pub(crate) case_sensitive: bool,
}

impl MessageSearch {
    pub(crate) fn clear(&mut self) {
        self.text.clear();
    }

    pub(crate) fn is_active(&self) -> bool {
        !self.text.trim().is_empty()
    }

    /// 编译当前搜索词（普通词字面量 / `re:` 前缀正则）。
    pub(crate) fn query(&self) -> crate::search::SearchQuery {
        crate::search::SearchQuery::new(&self.text, self.case_sensitive)
    }

    pub(crate) fn matches(&self, haystack: &str, query: &crate::search::SearchQuery) -> bool {
        query.is_empty() || query.matches(haystack)
    }

    pub(crate) fn toolbar(&mut self, ui: &mut Ui, desired_width: f32, hint: &str, case_hint: &str) {
        // 搜索框后面通常还跟着大小写按钮、来源/端口筛选等控件。
        // 直接使用固定 desired_width 会在窄 Dock 中把后续控件推出可视区域，
        // 这里给后续按钮预留一点空间；外层使用 horizontal_wrapped 时也能自然换行。
        let input_width = desired_width.min((ui.available_width() - 40.0).max(48.0));
        let search_response = ui
            .add(
                egui::TextEdit::singleline(&mut self.text)
                    .desired_width(input_width)
                    .hint_text(hint),
            )
            .on_hover_text("支持正则：以 re: 开头（如 re:^ok\\d+）；否则按字面量搜索");
        if search_response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.clear();
            search_response.surrender_focus();
        }

        let mut case_button =
            egui::Button::new(egui::RichText::new("Aa").color(if self.case_sensitive {
                theme::toggle_selected_text()
            } else {
                theme::text_primary()
            }))
            .selected(self.case_sensitive)
            .small();
        if self.case_sensitive {
            case_button = case_button
                .fill(theme::toggle_selected_bg())
                .stroke(egui::Stroke::new(1.0, theme::toggle_selected_border()));
        }
        if ui.add(case_button).on_hover_text(case_hint).clicked() {
            self.case_sensitive = !self.case_sensitive;
        }
    }
}

/// 消息流共用的自动跟随状态。
///
/// 使用 key 保存各视图滚动位置，因此既支持日志的单视图，也支持终端按端口扩展为多视图。
pub(crate) struct AutoScrollState {
    pub(crate) enabled: bool,
    previous_offsets: BTreeMap<String, f32>,
    pending_scroll_to_bottom: BTreeSet<String>,
}

impl Default for AutoScrollState {
    fn default() -> Self {
        Self {
            enabled: true,
            previous_offsets: BTreeMap::new(),
            pending_scroll_to_bottom: BTreeSet::new(),
        }
    }
}

impl AutoScrollState {
    pub(crate) fn reset(&mut self) {
        self.enabled = true;
        self.previous_offsets.clear();
        self.pending_scroll_to_bottom.clear();
    }

    pub(crate) fn take_pending(&mut self, key: &str) -> bool {
        self.pending_scroll_to_bottom.remove(key)
    }

    pub(crate) fn button(&mut self, ui: &mut Ui) -> bool {
        let was_enabled = self.enabled;
        let (icon, label, tooltip) = if self.enabled {
            (ICON_PAUSE, "暂停跟随", "暂停自动跟随最新消息")
        } else {
            (
                ICON_VERTICAL_ALIGN_BOTTOM,
                "恢复跟随",
                "滚动到底部并继续跟随最新消息",
            )
        };
        if design::button(ui, icon, label, design::ButtonKind::Secondary)
            .on_hover_text(tooltip)
            .clicked()
        {
            self.enabled = !self.enabled;
        }
        !was_enabled && self.enabled
    }

    pub(crate) fn update(
        &mut self,
        ui: &Ui,
        key: &str,
        inner_rect: egui::Rect,
        content_height: f32,
        offset_y: f32,
        scroll_delta_y: f32,
    ) {
        let pointer_inside = ui
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|position| inner_rect.contains(position));
        let primary_down = ui.input(|input| input.pointer.primary_down());
        let previous_offset_y = self.previous_offsets.get(key).copied().unwrap_or(offset_y);
        let next_enabled = crate::next_auto_scroll_state(
            self.enabled,
            pointer_inside,
            primary_down,
            scroll_delta_y,
            previous_offset_y,
            offset_y,
            content_height,
            inner_rect.height(),
        );
        let should_repair_stick_to_bottom = next_enabled
            && !crate::scroll_delta_moves_away_from_bottom(scroll_delta_y)
            && !crate::scroll_is_at_bottom(offset_y, content_height, inner_rect.height());

        if self.enabled != next_enabled {
            if !self.enabled && next_enabled {
                self.pending_scroll_to_bottom.insert(key.to_owned());
            }
            self.enabled = next_enabled;
            ui.ctx().request_repaint();
        }

        if should_repair_stick_to_bottom {
            self.pending_scroll_to_bottom.insert(key.to_owned());
            ui.ctx().request_repaint();
        }
        self.previous_offsets.insert(key.to_owned(), offset_y);
    }
}

/// 根据列宽和等宽字体字符宽度，估算包含显式换行及自动折行后的行数。
pub(crate) fn estimated_wrapped_line_count(text: &str, width: f32, glyph_width: f32) -> usize {
    let width = width.max(glyph_width.max(1.0));
    let glyph_width = glyph_width.max(1.0);
    text.split('\n')
        .map(|line| {
            let chars = line.chars().count().max(1) as f32;
            ((chars * glyph_width) / width).ceil().max(1.0) as usize
        })
        .sum::<usize>()
        .max(1)
}

/// 动态消息流的共享行高缓存。
///
/// 日志和接收区都可能有换行、长文本折行和字体大小切换。缓存按稳定行 ID、内容签名
/// 与内容列宽度失效，只在进入视口时重新测量真实高度；离屏行使用保守估算值。
#[derive(Default)]
pub(crate) struct MessageList {
    row_heights: BTreeMap<u64, CachedMessageRowHeight>,
    last_total_height: f32,
    last_row_count: usize,
}

#[derive(Clone, Copy)]
struct CachedMessageRowHeight {
    signature: u64,
    width_key: u64,
    height: f32,
}

impl MessageList {
    pub(crate) fn clear(&mut self) {
        self.row_heights.clear();
        self.last_total_height = 0.0;
        self.last_row_count = 0;
    }

    pub(crate) fn remove(&mut self, id: u64) {
        self.row_heights.remove(&id);
    }

    pub(crate) fn estimated_height(
        &self,
        id: u64,
        signature: u64,
        width_key: u64,
        fallback: f32,
    ) -> f32 {
        self.row_heights
            .get(&id)
            .filter(|cached| cached.signature == signature && cached.width_key == width_key)
            .map_or(fallback, |cached| cached.height)
    }

    pub(crate) fn record_height(&mut self, id: u64, signature: u64, width_key: u64, height: f32) {
        self.row_heights.insert(
            id,
            CachedMessageRowHeight {
                signature,
                width_key,
                height,
            },
        );
    }

    /// 记录本帧内容总高；高度变化时要求下一帧重绘，使滚动条和底部跟随立即收敛。
    pub(crate) fn note_total_height(&mut self, ui: &Ui, total_height: f32, row_count: usize) {
        if (self.last_total_height - total_height).abs() > 0.5 || self.last_row_count != row_count {
            ui.ctx().request_repaint();
        }
        self.last_total_height = total_height;
        self.last_row_count = row_count;
    }
}

/// 管理表格行高亮和右键菜单的行匹配冻结状态。
///
/// 用法：
/// 1. 在渲染循环前创建 `RowHighlight::new(ui, scroll_id)`。
/// 2. 循环内每行调用 `paint_background()` 画高亮背景。
/// 3. 循环内每行调用 `record_row()` 记录 Y 范围。
/// 4. 循环结束后，用 `context_menu_data()` 获取冻结的行索引，构建右键菜单。
pub(crate) struct RowHighlight {
    frozen_y: Option<(f32, f32)>,
    frozen_y_id: Id,
    /// 只记录实际参与本帧交互的行；虚拟列表中索引可能不是从 0 开始。
    row_y_ranges: Vec<(usize, f32, f32)>,
}

/// 记录字符级拖选跨过的稳定行 ID。
///
/// `egui` 会在某一帧没有遇到文本选区的两个端点时主动清空选区。消息列表采用
/// 视口虚拟化后，滚动出视口的起点不会再调用 `label_text_selection`，因此需要让
/// 起点到当前端点之间的行继续参与文本布局；范围外的行仍可正常剔除。
#[derive(Debug, Default)]
pub(crate) struct TextSelectionRows {
    anchor: Option<u64>,
    extent: Option<u64>,
    /// 端点换行时保留一帧旧位置，让 egui 能在同一帧看到旧、新两个端点并完成迁移。
    previous_extent: Option<u64>,
}

impl TextSelectionRows {
    pub(crate) fn begin(&mut self, row_id: u64) {
        self.anchor = Some(row_id);
        self.extent = Some(row_id);
        self.previous_extent = None;
    }

    pub(crate) fn update(&mut self, row_id: u64) {
        if self.anchor.is_some() && self.extent != Some(row_id) {
            self.previous_extent = self.extent;
            self.extent = Some(row_id);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.anchor = None;
        self.extent = None;
        self.previous_extent = None;
    }

    pub(crate) fn is_active(&self) -> bool {
        self.anchor.is_some()
    }

    /// 返回拖选覆盖的行索引（含首尾）。过滤结果变化导致端点行消失时自动失效。
    pub(crate) fn layout_range(
        &mut self,
        row_ids: impl IntoIterator<Item = u64>,
    ) -> Option<std::ops::RangeInclusive<usize>> {
        let (Some(anchor), Some(extent)) = (self.anchor, self.extent) else {
            return None;
        };
        // `layout_range` 每帧调用一次。旧端点只需参与当前过渡帧，之后 egui 的
        // primary cursor 已迁移到新端点，可以恢复到较小的布局范围。
        let previous_extent = self.previous_extent.take();

        let mut anchor_index = None;
        let mut extent_index = None;
        let mut previous_extent_index = previous_extent.map(|_| None);
        for (index, row_id) in row_ids.into_iter().enumerate() {
            if row_id == anchor {
                anchor_index = Some(index);
            }
            if row_id == extent {
                extent_index = Some(index);
            }
            if previous_extent == Some(row_id) {
                previous_extent_index = Some(Some(index));
            }
            if anchor_index.is_some()
                && extent_index.is_some()
                && previous_extent_index.is_none_or(|index| index.is_some())
            {
                break;
            }
        }

        let (Some(anchor_index), Some(extent_index)) = (anchor_index, extent_index) else {
            self.clear();
            return None;
        };
        let Some(previous_extent_index) = previous_extent_index.flatten().or_else(|| {
            if previous_extent.is_none() {
                Some(extent_index)
            } else {
                None
            }
        }) else {
            self.clear();
            return None;
        };
        let start = anchor_index.min(extent_index).min(previous_extent_index);
        let end = anchor_index.max(extent_index).max(previous_extent_index);
        Some(start..=end)
    }
}

impl RowHighlight {
    /// 在渲染循环**之前**调用。
    pub(crate) fn new(ui: &Ui, scroll_id: impl std::hash::Hash + std::fmt::Debug) -> Self {
        let frozen_y_id = ui.make_persistent_id(("row-hl-y", scroll_id));
        let any_popup = ui.ctx().any_popup_open();

        // 菜单关闭后清除冻结的高亮位置
        if !any_popup {
            ui.data_mut(|d| d.remove::<Option<(f32, f32)>>(frozen_y_id));
        }
        let frozen_y: Option<(f32, f32)> = ui.data_mut(|d| d.get_persisted(frozen_y_id)).flatten();

        Self {
            frozen_y,
            frozen_y_id,
            row_y_ranges: Vec::new(),
        }
    }

    /// 在每行循环内调用：画高亮背景（在文字之前）。
    /// 返回 `true` 表示该行被高亮。
    pub(crate) fn paint_background(
        &self,
        ui: &Ui,
        full_rect: egui::Rect,
        current_y: f32,
        entry_height: f32,
    ) -> bool {
        let should_highlight = if let Some((top, bottom)) = self.frozen_y {
            current_y <= bottom && current_y + entry_height >= top
        } else {
            let hover_rect = egui::Rect::from_min_size(
                egui::pos2(full_rect.left(), current_y),
                egui::vec2(full_rect.width(), entry_height),
            );
            ui.rect_contains_pointer(hover_rect)
        };
        if should_highlight {
            ui.painter_at(full_rect).rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(full_rect.left(), current_y),
                    egui::vec2(full_rect.width(), entry_height),
                ),
                0.0,
                theme::widget_hover(),
            );
        }
        should_highlight
    }

    /// 在每行循环内调用：记录该行的 Y 范围，返回行索引。
    /// 参数 `current_y` 是该行顶部 Y，`entry_height` 是该行高度。
    pub(crate) fn record_row(&mut self, current_y: f32, entry_height: f32) -> usize {
        let index = self.row_y_ranges.len();
        self.record_row_at(index, current_y, entry_height);
        index
    }

    /// 记录带有绝对行索引的 Y 范围，供虚拟列表使用。
    pub(crate) fn record_row_at(&mut self, index: usize, current_y: f32, entry_height: f32) {
        self.row_y_ranges
            .push((index, current_y, current_y + entry_height));
    }

    /// 在渲染循环**之后**调用：根据右键点击位置计算悬停行索引。
    /// 返回 `(clicked_row_index, frozen_row_id)` 供 `context_menu_data()` 使用。
    pub(crate) fn resolve_click(
        &mut self,
        ui: &Ui,
        click_response: &egui::Response,
        frozen_data_id: Id,
    ) -> Option<usize> {
        let clicked = click_response.clicked_by(egui::PointerButton::Secondary);
        let menu_open = click_response.context_menu_opened();

        if clicked {
            // 冻结高亮位置
            let frozen_y = ui.input(|i| i.pointer.hover_pos()).map(|p| (p.y, p.y));
            ui.data_mut(|d| d.insert_persisted(self.frozen_y_id, frozen_y));

            // 冻结行索引
            let row_idx = ui.input(|i| i.pointer.hover_pos()).and_then(|pointer| {
                self.row_y_ranges
                    .iter()
                    .find(|(_, top, bottom)| pointer.y >= *top && pointer.y < *bottom)
                    .map(|(index, _, _)| *index)
            });
            ui.data_mut(|d| d.insert_persisted(frozen_data_id, row_idx));
            row_idx
        } else if menu_open {
            ui.data_mut(|d| d.get_persisted::<Option<usize>>(frozen_data_id))
                .flatten()
        } else {
            None
        }
    }

    /// 在未冻结时，根据鼠标实时位置计算悬停行索引。
    pub(crate) fn hover_index(&self, ui: &Ui) -> Option<usize> {
        ui.input(|i| i.pointer.hover_pos()).and_then(|pointer| {
            self.row_y_ranges
                .iter()
                .find(|(_, top, bottom)| pointer.y >= *top && pointer.y < *bottom)
                .map(|(index, _, _)| *index)
        })
    }

    /// 根据 Y 坐标查找行索引。
    pub(crate) fn row_index_at_y(&self, y: f32) -> Option<usize> {
        self.row_y_ranges
            .iter()
            .find(|(_, top, bottom)| y >= *top && y < *bottom)
            .map(|(index, _, _)| *index)
    }

    /// 获取指定行索引的 Y 范围。
    pub(crate) fn row_y_range(&self, index: usize) -> Option<(f32, f32)> {
        self.row_y_ranges
            .iter()
            .find(|(row_index, _, _)| *row_index == index)
            .map(|(_, top, bottom)| (*top, *bottom))
    }

    /// 根据 Y 坐标查找行；拖拽越过首尾时钳制到第一/最后一行。
    pub(crate) fn row_index_at_y_clamped(&self, y: f32) -> Option<usize> {
        let first = self.row_y_ranges.first()?;
        if y < first.1 {
            return Some(first.0);
        }

        let last_index = self.row_y_ranges.len() - 1;
        let last = self.row_y_ranges[last_index];
        if y >= last.2 {
            return Some(last.0);
        }

        self.row_index_at_y(y)
    }
}

// ── RowSelection：多行框选 ──

/// 行选择状态：支持左键拖拽框选、单击选中、Shift 扩展、Ctrl 追加。
pub struct RowSelection {
    /// 当前可见行的稳定 ID，索引与渲染行一致。
    row_keys: Vec<u64>,
    /// 已选中的稳定行 ID，支持 Ctrl 离散多选。
    selected: BTreeSet<u64>,
    /// Shift 扩展的稳定锚点。
    anchor: Option<u64>,
    /// 本次主键手势按下时所在的行。
    pointer_origin: Option<u64>,
    pointer_active: bool,
    dragging: bool,
    /// Ctrl+Shift 拖拽时设为 true，走 add_range 而非 select_range。
    ctrl_shift_drag: bool,
}

impl RowSelection {
    pub fn new(row_count: usize) -> Self {
        Self {
            row_keys: (0..row_count as u64).collect(),
            selected: BTreeSet::new(),
            anchor: None,
            pointer_origin: None,
            pointer_active: false,
            dragging: false,
            ctrl_shift_drag: false,
        }
    }

    /// 同步当前可见行。选区按稳定 ID 保留，不会因插入、删除或筛选而错位。
    pub fn sync_rows(&mut self, row_keys: impl IntoIterator<Item = u64>) {
        self.row_keys = row_keys.into_iter().collect();
        let visible: BTreeSet<u64> = self.row_keys.iter().copied().collect();
        self.selected.retain(|key| visible.contains(key));

        if self.anchor.is_some_and(|key| !visible.contains(&key)) {
            self.anchor = None;
        }
        if self
            .pointer_origin
            .is_some_and(|key| !visible.contains(&key))
        {
            self.pointer_origin = None;
            self.pointer_active = false;
            self.dragging = false;
        }
    }

    pub fn has_selection(&self) -> bool {
        !self.selected.is_empty()
    }

    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// 按当前显示顺序返回所有选中行，兼容 Ctrl 离散多选。
    pub fn selected_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.row_keys
            .iter()
            .enumerate()
            .filter_map(|(index, key)| self.selected.contains(key).then_some(index))
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.row_keys
            .get(index)
            .is_some_and(|key| self.selected.contains(key))
    }

    /// 清除所有选中。
    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.pointer_origin = None;
        self.pointer_active = false;
        self.dragging = false;
    }

    /// Ctrl+A 全选：选中所有可见行。
    pub fn select_all(&mut self) {
        self.selected = self.row_keys.iter().copied().collect();
        self.anchor = self.row_keys.last().copied();
    }

    /// 右键未选中的行时，让菜单明确作用于该行；右键已选中行时由调用方保留多选。
    pub fn select_only(&mut self, index: usize) {
        let Some(&key) = self.row_keys.get(index) else {
            return;
        };
        self.selected.clear();
        self.selected.insert(key);
        self.anchor = Some(key);
    }

    /// 处理整行选择手势。
    ///
    /// 不依赖覆盖正文的 `Response`，因此可与字符级文本选择共存：在同一逻辑行内拖动
    /// 仍由文本选择处理；指针跨行后切换为整行范围选择。
    pub fn handle_input(
        &mut self,
        ui: &Ui,
        interaction_rect: egui::Rect,
        viewport_rect: egui::Rect,
        hovered_row: Option<usize>,
        scroll_delta: &mut f32,
    ) -> bool {
        let (pointer_pos, primary_pressed, primary_down, primary_released, ctrl, shift) =
            ui.input(|input| {
                (
                    input.pointer.hover_pos(),
                    input.pointer.button_pressed(egui::PointerButton::Primary),
                    input.pointer.button_down(egui::PointerButton::Primary),
                    input.pointer.button_released(egui::PointerButton::Primary),
                    input.modifiers.ctrl || input.modifiers.command,
                    input.modifiers.shift,
                )
            });

        let pressed_inside = primary_pressed
            && pointer_pos.is_some_and(|pos| interaction_rect.contains(pos))
            && ui.rect_contains_pointer(interaction_rect);

        if pressed_inside {
            if let Some(index) = hovered_row {
                self.begin_pointer(index, ctrl, shift);
            } else {
                self.pointer_active = false;
                self.pointer_origin = None;
            }
        }

        if self.pointer_active && primary_down {
            if let Some(index) = hovered_row {
                self.drag_to(index);
            }

            if self.dragging
                && let Some(pointer_y) = pointer_pos.map(|pos| pos.y)
            {
                *scroll_delta += edge_scroll_delta(pointer_y, viewport_rect);
            }
        }

        if primary_released || (self.pointer_active && !primary_down) {
            self.pointer_active = false;
            self.pointer_origin = None;
            self.dragging = false;
        }

        pressed_inside
    }

    pub fn begin_pointer(&mut self, index: usize, ctrl: bool, shift: bool) {
        let Some(&key) = self.row_keys.get(index) else {
            return;
        };

        self.pointer_active = true;
        self.pointer_origin = Some(key);
        self.dragging = false;
        self.ctrl_shift_drag = ctrl && shift;

        if ctrl && shift {
            // Ctrl+Shift：从 anchor 扩展到当前行（不清空已有选中）
            let anchor_index = self
                .anchor
                .and_then(|anchor| self.index_of(anchor))
                .unwrap_or(index);
            if self.anchor.is_none() {
                self.anchor = Some(key);
            }
            self.add_range(anchor_index, index);
        } else if ctrl {
            if !self.selected.insert(key) {
                self.selected.remove(&key);
            }
            self.anchor = Some(key);
        } else if shift {
            let anchor_index = self
                .anchor
                .and_then(|anchor| self.index_of(anchor))
                .unwrap_or(index);
            if self.anchor.is_none() {
                self.anchor = Some(key);
            }
            self.select_range(anchor_index, index);
        } else {
            self.selected.clear();
            self.selected.insert(key);
            self.anchor = Some(key);
        }
    }

    fn drag_to(&mut self, index: usize) {
        let Some(origin) = self.pointer_origin else {
            return;
        };
        let Some(origin_index) = self.index_of(origin) else {
            return;
        };
        if index >= self.row_keys.len() {
            return;
        }

        if self.dragging || index != origin_index {
            self.dragging = true;
            if self.ctrl_shift_drag {
                // Ctrl+Shift 拖拽：追加扩展选区
                self.add_range(origin_index, index);
            } else {
                self.anchor = Some(origin);
                self.select_range(origin_index, index);
            }
        }
    }

    fn select_range(&mut self, first: usize, second: usize) {
        let (lo, hi) = if first <= second {
            (first, second)
        } else {
            (second, first)
        };
        self.selected = self.row_keys[lo..=hi].iter().copied().collect();
    }

    fn add_range(&mut self, first: usize, second: usize) {
        let (lo, hi) = if first <= second {
            (first, second)
        } else {
            (second, first)
        };
        for &key in &self.row_keys[lo..=hi] {
            self.selected.insert(key);
        }
    }

    fn index_of(&self, key: u64) -> Option<usize> {
        self.row_keys.iter().position(|candidate| *candidate == key)
    }

    /// 每行渲染时调用：画选中高亮。
    pub fn paint(&self, ui: &Ui, full_rect: egui::Rect, current_y: f32, entry_height: f32) {
        let sel_rect = egui::Rect::from_min_size(
            egui::pos2(full_rect.left(), current_y),
            egui::vec2(full_rect.width(), entry_height),
        );
        let painter = ui.painter_at(full_rect);
        painter.rect_filled(sel_rect, 0.0, theme::bg_selection());
        painter.line_segment(
            [sel_rect.left_top(), sel_rect.left_bottom()],
            egui::Stroke::new(2.0, theme::blue()),
        );
    }
}

/// `Ui::scroll_with_delta` 使用内容移动方向：顶部为正，底部为负。
pub(crate) fn edge_scroll_delta(pointer_y: f32, viewport_rect: egui::Rect) -> f32 {
    let edge = 24.0;
    if pointer_y < viewport_rect.top() + edge {
        (edge - (pointer_y - viewport_rect.top())).clamp(0.0, edge) * 0.3
    } else if pointer_y > viewport_rect.bottom() - edge {
        -(edge - (viewport_rect.bottom() - pointer_y)).clamp(0.0, edge) * 0.3
    } else {
        0.0
    }
}

/// `egui::ScrollArea` 在其它控件持有拖拽时不会处理滚轮。消息文本或整行选区正在
/// 左键拖拽时，由调用方把渲染前保存的滚轮量补到 `Ui::scroll_with_delta`。
pub(crate) fn wheel_scroll_during_selection(selection_dragging: bool, wheel_delta_y: f32) -> f32 {
    if selection_dragging {
        wheel_delta_y
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MessageList, RowHighlight, RowSelection, TextSelectionRows,
        bulk_copy_requires_confirmation, claim_copy_focus, copy_text_with_feedback,
        edge_scroll_delta, estimated_wrapped_line_count, owns_copy_focus, take_copy_feedback,
        wheel_scroll_during_selection,
    };

    #[test]
    fn message_list_height_cache_requires_matching_content_and_width() {
        let mut list = MessageList::default();
        list.record_height(7, 11, 120, 42.0);

        assert_eq!(list.estimated_height(7, 11, 120, 16.0), 42.0);
        assert_eq!(list.estimated_height(7, 11, 120, 64.0), 42.0);
        assert_eq!(list.estimated_height(7, 12, 120, 16.0), 16.0);
        assert_eq!(list.estimated_height(7, 11, 121, 16.0), 16.0);

        list.remove(7);
        assert_eq!(list.estimated_height(7, 11, 120, 16.0), 16.0);
    }

    #[test]
    fn message_search_is_unicode_case_insensitive() {
        let search = super::MessageSearch {
            text: "ä设备".to_owned(),
            ..Default::default()
        };
        let query = search.query();

        assert!(search.matches("Ä设备已连接", &query));
    }

    #[test]
    fn wrapped_line_estimate_accounts_for_width_and_explicit_lines() {
        assert_eq!(estimated_wrapped_line_count("12345678", 20.0, 5.0), 2);
        assert_eq!(estimated_wrapped_line_count("12\n345678", 20.0, 5.0), 3);
    }

    #[test]
    fn row_lookup_handles_variable_heights_and_dragging_past_edges() {
        let mut rows = RowHighlight {
            frozen_y: None,
            frozen_y_id: egui::Id::NULL,
            row_y_ranges: Vec::new(),
        };
        rows.record_row(10.0, 5.0);
        rows.record_row(15.0, 20.0);

        assert_eq!(rows.row_index_at_y(14.9), Some(0));
        assert_eq!(rows.row_index_at_y(15.0), Some(1));
        assert_eq!(rows.row_index_at_y_clamped(-100.0), Some(0));
        assert_eq!(rows.row_index_at_y_clamped(100.0), Some(1));
    }

    #[test]
    fn text_selection_rows_keep_the_full_range_in_both_directions() {
        let row_ids = [10, 20, 30, 40, 50];
        let mut selection = TextSelectionRows::default();

        selection.begin(20);
        selection.update(40);
        assert_eq!(selection.layout_range(row_ids), Some(1..=3));

        selection.begin(40);
        selection.update(20);
        assert_eq!(selection.layout_range(row_ids), Some(1..=3));
    }

    #[test]
    fn text_selection_rows_clear_when_a_filtered_endpoint_disappears() {
        let mut selection = TextSelectionRows::default();
        selection.begin(20);
        selection.update(40);

        assert_eq!(selection.layout_range([10, 20, 30]), None);
        assert!(!selection.is_active());
    }

    #[test]
    fn text_selection_rows_keep_the_old_endpoint_for_a_reverse_drag_frame() {
        let row_ids = [10, 20, 30, 40, 50];
        let mut selection = TextSelectionRows::default();

        selection.begin(20);
        selection.update(50);
        assert_eq!(selection.layout_range(row_ids), Some(1..=4));

        // 鼠标从第 5 行移回第 3 行时，这一帧仍需布局旧的第 5 行，否则 egui
        // 尚未来得及迁移的文本 cursor 会被判定为消失。
        selection.update(30);
        assert_eq!(selection.layout_range(row_ids), Some(1..=4));
        assert_eq!(selection.layout_range(row_ids), Some(1..=2));
    }

    #[test]
    fn ctrl_and_shift_selection_use_all_selected_rows() {
        let mut selection = RowSelection::new(0);
        selection.sync_rows([10, 20, 30, 40]);

        selection.begin_pointer(1, false, false);
        selection.begin_pointer(3, true, false);
        assert_eq!(selection.selected_indices().collect::<Vec<_>>(), [1, 3]);

        selection.begin_pointer(2, false, true);
        assert_eq!(selection.selected_indices().collect::<Vec<_>>(), [2, 3]);
    }

    #[test]
    fn selecting_only_one_row_replaces_an_old_discrete_selection() {
        let mut selection = RowSelection::new(0);
        selection.sync_rows([10, 20, 30]);
        selection.begin_pointer(0, false, false);
        selection.begin_pointer(2, true, false);

        selection.select_only(1);

        assert_eq!(selection.selected_count(), 1);
        assert_eq!(selection.selected_indices().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn large_bulk_copy_requires_confirmation() {
        assert!(!bulk_copy_requires_confirmation(10_000));
        assert!(bulk_copy_requires_confirmation(10_001));
    }

    #[test]
    fn latest_panel_interaction_owns_keyboard_copy() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            claim_copy_focus(ui, "terminal");
            assert!(owns_copy_focus(ui, "terminal"));
            assert!(!owns_copy_focus(ui, "log"));

            claim_copy_focus(ui, "log");
            assert!(owns_copy_focus(ui, "log"));
            assert!(!owns_copy_focus(ui, "terminal"));
        });
    }

    #[test]
    fn clipboard_helper_queues_one_feedback_message() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            copy_text_with_feedback(ui, "payload", "已复制测试内容");
        });

        assert_eq!(take_copy_feedback(&ctx).as_deref(), Some("已复制测试内容"));
        assert!(take_copy_feedback(&ctx).is_none());
    }

    #[test]
    fn selection_follows_stable_row_ids_when_rows_change() {
        let mut selection = RowSelection::new(0);
        selection.sync_rows([10, 20, 30]);
        selection.begin_pointer(1, false, false);

        selection.sync_rows([5, 10, 20, 30, 40]);
        assert_eq!(selection.selected_indices().collect::<Vec<_>>(), [2]);

        selection.sync_rows([5, 10, 30, 40]);
        assert!(!selection.has_selection());
    }

    #[test]
    fn crossing_rows_turns_pointer_gesture_into_a_range() {
        let mut selection = RowSelection::new(0);
        selection.sync_rows([10, 20, 30, 40]);
        selection.begin_pointer(2, false, false);
        assert!(!selection.is_dragging());

        selection.drag_to(0);
        assert!(selection.is_dragging());
        assert_eq!(selection.selected_indices().collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn edge_scroll_moves_content_in_the_expected_direction() {
        let viewport = egui::Rect::from_min_max(egui::pos2(0.0, 100.0), egui::pos2(200.0, 300.0));

        assert!(edge_scroll_delta(105.0, viewport) > 0.0);
        assert_eq!(edge_scroll_delta(200.0, viewport), 0.0);
        assert!(edge_scroll_delta(295.0, viewport) < 0.0);
    }

    #[test]
    fn wheel_scroll_is_forwarded_only_during_selection_drag() {
        assert_eq!(wheel_scroll_during_selection(true, 24.0), 24.0);
        assert_eq!(wheel_scroll_during_selection(true, -24.0), -24.0);
        assert_eq!(wheel_scroll_during_selection(false, 24.0), 0.0);
    }
}
