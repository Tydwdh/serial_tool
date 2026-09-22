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
        let hover =
            crate::search::SearchQuery::hover_hint(self.query().used_invalid_regex_fallback());
        let search_response = ui
            .add(
                egui::TextEdit::singleline(&mut self.text)
                    .desired_width(input_width)
                    .hint_text(hint),
            )
            .on_hover_text(hover);
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

    /// 取出并清除该视图排好的一次“滚到底”请求。
    pub(crate) fn take_pending(&mut self, key: &str) -> bool {
        self.pending_scroll_to_bottom.remove(key)
    }

    /// 绘制跟随开关；仅当本帧由“暂停”切回“跟随”时返回 `true`，提示调用方滚到底。
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
        // 可视区高度只从 `inner_rect` 取一次：状态机和底部修复判定读的是同一个量，
        // 各算一遍等于给两处留出漂移的机会。
        let viewport_height = inner_rect.height();
        let next_enabled = crate::next_auto_scroll_state(crate::AutoScrollInputs {
            auto_scroll: self.enabled,
            pointer_inside,
            primary_down,
            scroll_delta_y,
            previous_offset_y,
            offset_y,
            content_height,
            viewport_height,
        });
        let should_repair_stick_to_bottom = next_enabled
            && !crate::scroll_delta_moves_away_from_bottom(scroll_delta_y)
            && !crate::scroll_is_at_bottom(offset_y, content_height, viewport_height);

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

/// 管理表格行高亮和右键菜单的行匹配冻结状态。
///
/// 用法：
/// 1. 在渲染循环前创建 `RowHighlight::new(ui, scroll_id)`。
/// 2. 循环内每行调用 `paint_background()` 画高亮背景。
/// 3. 循环内每行调用 `record_row_at()` 记录 Y 范围。
/// 4. 循环结束后，用 `resolve_click()` 获取冻结的行索引，构建右键菜单。
pub(crate) struct RowHighlight {
    frozen_y: Option<(f32, f32)>,
    frozen_y_id: Id,
    /// 本帧参与交互的行：`(行索引, 行顶 y, 行底 y)`。
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
        let row_rect = egui::Rect::from_min_size(
            egui::pos2(full_rect.left(), current_y),
            egui::vec2(full_rect.width(), entry_height),
        );
        let should_highlight = if let Some((top, bottom)) = self.frozen_y {
            current_y <= bottom && current_y + entry_height >= top
        } else {
            ui.rect_contains_pointer(row_rect)
        };
        if should_highlight {
            let painter = ui.painter_at(full_rect);
            painter.rect_filled(row_rect, 0.0, theme::widget_hover());
        }
        should_highlight
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
        let (first_index, first_top, _) = *self.row_y_ranges.first()?;
        if y < first_top {
            return Some(first_index);
        }

        let (last_index, _, last_bottom) = *self.row_y_ranges.last()?;
        if y >= last_bottom {
            return Some(last_index);
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
    last_sync_generation: Option<u64>,
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
            last_sync_generation: None,
        }
    }

    /// 同步当前可见行。选区按稳定 ID 保留，不会因插入、删除或筛选而错位。
    pub fn sync_rows(&mut self, row_keys: impl IntoIterator<Item = u64>) {
        self.last_sync_generation = None;
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

    /// 按调用方维护的稳定 generation 同步行 ID，避免稳定帧重新遍历整张长列表。
    pub fn sync_rows_with_generation(
        &mut self,
        row_keys: impl IntoIterator<Item = u64>,
        generation: u64,
    ) {
        if self.last_sync_generation == Some(generation) {
            return;
        }
        self.sync_rows(row_keys);
        self.last_sync_generation = Some(generation);
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
    ///
    /// 返回本帧是否在 `interaction_rect` 内按下主键；拖拽靠近视口边缘时会往
    /// `scroll_delta` 累加滚动量，由调用方应用到外层滚动容器。
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

        if shift {
            // Shift：从锚点扩展到当前行。带 Ctrl 时追加进已有选区，
            // 不带 Ctrl 时替换选区（锚点缺失时以当前行为锚点）。
            let anchor_index = self
                .anchor
                .and_then(|anchor| self.index_of(anchor))
                .unwrap_or(index);
            if self.anchor.is_none() {
                self.anchor = Some(key);
            }
            if ctrl {
                self.add_range(anchor_index, index);
            } else {
                self.select_range(anchor_index, index);
            }
        } else if ctrl {
            // Ctrl：切换单行的选中状态，锚点跟到手势起点。
            if !self.selected.insert(key) {
                self.selected.remove(&key);
            }
            self.anchor = Some(key);
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
        let (lo, hi) = (first.min(second), first.max(second));
        self.selected = self.row_keys[lo..=hi].iter().copied().collect();
    }

    fn add_range(&mut self, first: usize, second: usize) {
        let (lo, hi) = (first.min(second), first.max(second));
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
        AutoScrollState, RowHighlight, RowSelection, TextSelectionRows,
        bulk_copy_requires_confirmation, claim_copy_focus, copy_text_with_feedback,
        edge_scroll_delta, estimated_wrapped_line_count, owns_copy_focus, take_copy_feedback,
        wheel_scroll_during_selection,
    };

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
        rows.record_row_at(0, 10.0, 5.0);
        rows.record_row_at(1, 15.0, 20.0);

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

    // ── 自动跟随：从唯一的生产调用点 `AutoScrollState::update` 驱动 ──────────
    //
    // 第 30 轮把 8 个位置参数收进 `AutoScrollInputs` 后普查过：那八个字段里六对相邻
    // 同型值任意对调，仓库里没有一条用例变红 —— 因为唯一能看见这次装配的调用点没有
    // 用例。下面这批不直接调 `next_auto_scroll_state`，只喂 `update` 真实帧：第 31 轮
    // 逐对复查过，这六对现在每一对都至少被一条用例抓到，曾经全盲的那一对也不例外。

    /// 基准视图：可视区高 100、内容高 1000，因此“贴底”对应 `offset_y = 900`。
    /// 内容高与可视区高必须是两个不同的数，否则这两个字段就换不出来了。
    const SCROLLABLE_VIEW: egui::Rect =
        egui::Rect::from_min_max(egui::pos2(100.0, 100.0), egui::pos2(300.0, 200.0));
    const POINTER_INSIDE: egui::Pos2 = egui::pos2(200.0, 150.0);
    const POINTER_OUTSIDE: egui::Pos2 = egui::pos2(200.0, 40.0);
    const CONTENT_HEIGHT: f32 = 1000.0;
    const BOTTOM_OFFSET: f32 = 900.0;
    const MID_DOCUMENT: f32 = 400.0;
    /// 滚轮符号照 egui：正 = 翻历史（离开底部），负 = 追最新。
    const WHEEL_UP: f32 = 24.0;
    const WHEEL_DOWN: f32 = -24.0;
    const VIEW_KEY: &str = "log";
    const OTHER_VIEW_KEY: &str = "terminal:COM3";

    /// 一帧喂给 `update` 的全部外部输入。
    ///
    /// 用命名字段而不是位置参数，理由和 `crate::AutoScrollInputs` 一样：这里三个相邻
    /// `f32` 加两个相邻 `bool`，按位置传会把第 30 轮刚拆掉的那面墙在测试里重新砌起来。
    #[derive(Clone, Copy)]
    struct Frame {
        key: &'static str,
        /// `ScrollArea` 的 inner rect：`update` 既从它取可视区高度，也用它判定悬停。
        inner_rect: egui::Rect,
        /// 指针悬停位置；`None` = 本帧 egui 报不出位置（鼠标移出视图或窗口）。
        hover: Option<egui::Pos2>,
        primary_down: bool,
        content_height: f32,
        offset_y: f32,
        scroll_delta_y: f32,
    }

    /// 基准帧：指针在视图内、左键松开、开着跟随、停在文档中部且本帧没有滚轮。
    /// 每个用例只改自己判别的那一两个字段，其余继承这里的值，读起来才知道谁在起作用。
    const BASE_FRAME: Frame = Frame {
        key: VIEW_KEY,
        inner_rect: SCROLLABLE_VIEW,
        hover: Some(POINTER_INSIDE),
        primary_down: false,
        content_height: CONTENT_HEIGHT,
        offset_y: MID_DOCUMENT,
        scroll_delta_y: 0.0,
    };

    /// 把 `hover` / `primary_down` 写成 egui 认的输入。
    ///
    /// egui 0.35 的 `RawInput` 已经没有 `pointer` 字段，`hover_pos()` 与
    /// `primary_down()` 只能由 `events` 里的 `PointerMoved` / `PointerButton` 推出来。
    /// 报不出位置时就一个事件都不发：`PointerButton` 无论按下还是抬起都会顺带写下位置，
    /// 补一个抬起事件会把“本帧没有悬停”污染成“悬停在别处”。按下必然带着位置，
    /// 因此按下状态只在有位置时注入。
    fn pointer_events(frame: &Frame) -> egui::RawInput {
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 400.0),
            )),
            ..Default::default()
        };
        let Some(position) = frame.hover else {
            return raw;
        };
        raw.events.push(egui::Event::PointerMoved(position));
        if frame.primary_down {
            raw.events.push(egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            });
        }
        raw
    }

    /// 跑一帧真实的 egui pass。
    ///
    /// 每帧一个新的 `Context`：`update` 只读指针、只写自己那份 map/set，帧间连续性全在
    /// `AutoScrollState` 里。这样 egui 的 `buttons_down` 不会漏进下一帧，断言也不依赖
    /// 它的重绘调度。
    fn drive(state: &mut AutoScrollState, frame: Frame) {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(pointer_events(&frame), |ui| {
            state.update(
                ui,
                frame.key,
                frame.inner_rect,
                frame.content_height,
                frame.offset_y,
                frame.scroll_delta_y,
            );
        });
    }

    /// 上面那套注入是否真的有效 —— 下面每条用例都建立在它之上，所以先让它自己能变红。
    ///
    /// `update` 从 egui 只读两个量：`pointer.hover_pos()` 和 `pointer.primary_down()`。
    /// 这里把同一份注入直接读回来，顺带确认基准几何确实“内/外”各就各位。注入一旦随
    /// egui 升级失效，本用例会红，而不是让“指针在视图外 → 状态不变”那类断言永远空过。
    #[test]
    fn injected_pointer_events_reach_the_input_reads() {
        let ctx = egui::Context::default();

        // 只悬停、不按键：位置只可能来自 `PointerMoved`。缺这一半，`PointerButton`
        // 顺带写下的位置会把失效的 `PointerMoved` 注入盖住 —— 第一版就是这么漏的。
        let _ = ctx.run_ui(pointer_events(&BASE_FRAME), |ui| {
            assert_eq!(
                ui.input(|input| input.pointer.hover_pos()),
                Some(POINTER_INSIDE)
            );
            assert!(!ui.input(|input| input.pointer.primary_down()));
        });

        // 按下之后两个读法仍要各自成立。
        let pressed = Frame {
            primary_down: true,
            ..BASE_FRAME
        };
        let _ = ctx.run_ui(pointer_events(&pressed), |ui| {
            assert_eq!(
                ui.input(|input| input.pointer.hover_pos()),
                Some(POINTER_INSIDE)
            );
            assert!(ui.input(|input| input.pointer.primary_down()));
        });
        assert!(SCROLLABLE_VIEW.contains(POINTER_INSIDE));
        assert!(!SCROLLABLE_VIEW.contains(POINTER_OUTSIDE));

        // 同一个读法在不注入时必须取不到值：判别量来自注入，不是默认值碰巧合适。
        // 全新 Context —— 指针位置与按下状态在 egui 里本来就跨帧保留。
        let untouched = egui::Context::default();
        let _ = untouched.run_ui(egui::RawInput::default(), |ui| {
            assert_eq!(ui.input(|input| input.pointer.hover_pos()), None);
            assert!(!ui.input(|input| input.pointer.primary_down()));
        });
    }

    /// 指针不在视图内时，这一帧什么都不该发生 —— 哪怕其余每个脱离条件都成立。
    ///
    /// 判别量是 `pointer_inside`：左键按着、滚轮翻历史、偏移还在往回走，唯独指针在
    /// 别处。两帧分别覆盖“报不出位置”的两种来源（悬停在视图外、本帧根本没有指针事件）。
    /// 与下一条用例只差一个悬停点，所以“`auto_scroll` 与 `pointer_inside` 对调”（把
    /// 开关当成位置、把位置当成开关）在这里会以“跟随被踢掉”的形式变红。
    #[test]
    fn pointer_outside_the_viewport_leaves_auto_scroll_untouched() {
        let mut state = AutoScrollState::default();
        let dragged_outside = Frame {
            hover: Some(POINTER_OUTSIDE),
            primary_down: true,
            offset_y: 300.0,
            scroll_delta_y: WHEEL_UP,
            ..BASE_FRAME
        };

        drive(&mut state, dragged_outside);
        // 第二个来源：这一帧 egui 一个指针事件都没收到，位置与按下都读不出来。
        drive(
            &mut state,
            Frame {
                hover: None,
                primary_down: false,
                offset_y: 100.0,
                ..dragged_outside
            },
        );

        assert!(state.enabled, "指针没在视图内，跟随不该掉");
        assert!(
            !state.take_pending(VIEW_KEY),
            "指针在视图外时也不该替这个视图补一次滚到底"
        );
    }

    /// 与上一条只差悬停点：指针进了视图，向上翻历史就是明确的“别看最新”。
    ///
    /// 这条是上一条的反证 —— 如果注入失效（两条用例其实都在“指针在外”下运行），
    /// 这里就会红。同时它也反证 `pointer_inside` 与 `primary_down` 的对调：本帧左键
    /// 是松开的，对调后状态机读到“没有悬停”，跟随就不会掉。
    #[test]
    fn wheel_up_inside_the_viewport_disengages_following() {
        let mut state = AutoScrollState::default();

        drive(
            &mut state,
            Frame {
                scroll_delta_y: WHEEL_UP,
                ..BASE_FRAME
            },
        );

        assert!(!state.enabled, "视图内向上滚轮应当立即暂停跟随");
        assert!(
            !state.take_pending(VIEW_KEY),
            "暂停的那一帧不该同时排一次滚到底"
        );

        // 停在文档中部、这帧没有滚轮：暂停状态必须自己维持下去，也不能偷偷补滚。
        drive(&mut state, BASE_FRAME);
        assert!(!state.enabled);
        assert!(!state.take_pending(VIEW_KEY));
    }

    /// 偏移回落本身不是用户意图，按下左键才算。
    ///
    /// 判别量是 `primary_down`：第 2、3 帧的偏移一路往文档顶部走（400→300→200），只有
    /// 第 3 帧左键按着。内容变短、虚拟列表回收行都会造成第 2 帧那种回落，跟随不该因此掉。
    /// 第 2 帧钉住 `scroll_delta_y`/`previous_offset_y`：对调后半路的 400 被读成正向
    /// 滚轮量，回落就成了脱离。第 3 帧钉住 `previous_offset_y`/`offset_y`（方向被读反）
    /// 与 `offset_y`/`content_height`（“已经到底”被读成事实），两种都把这次拖拽当成没发生。
    #[test]
    fn only_a_pressed_button_turns_an_offset_drop_into_a_drag() {
        let mut state = AutoScrollState::default();
        drive(&mut state, BASE_FRAME);

        drive(
            &mut state,
            Frame {
                offset_y: 300.0,
                ..BASE_FRAME
            },
        );
        assert!(state.enabled, "没有按下左键的偏移回落不该暂停跟随");

        drive(
            &mut state,
            Frame {
                offset_y: 200.0,
                primary_down: true,
                ..BASE_FRAME
            },
        );
        assert!(!state.enabled, "按着左键把视图拖离底部，才是主动放弃跟随");
    }

    /// 恢复跟随的门槛是“真的到底了”，不是“这帧在往下走”。
    ///
    /// 三帧：暂停中停在文档中部（400）→ 往下滚到 500 → 滚到 900（= 内容高 - 可视区高），
    /// 只有第三帧该恢复并排一次滚到底。第二帧钉的是第 30 轮普查里唯一无人可见的那对：
    /// `content_height` 与 `viewport_height` 对调后 `(可视区 - 内容).max(0.0)` 恒为 0，
    /// “是否到底”成了恒真，于是“还在半路就往里滚了一下”被当成恢复跟随。第一帧另挡两种
    /// 装配错：`auto_scroll` 与 `pointer_inside` 对调会把暂停中的视图凭空恢复，
    /// `offset_y` 与 `content_height` 对调会让第一帧就读到“已在底部”。
    #[test]
    fn following_resumes_only_once_the_view_reaches_the_bottom() {
        let mut state = AutoScrollState {
            enabled: false,
            ..Default::default()
        };

        drive(&mut state, BASE_FRAME);
        assert!(!state.enabled);
        assert!(!state.take_pending(VIEW_KEY));

        let halfway = Frame {
            offset_y: 500.0,
            scroll_delta_y: WHEEL_DOWN,
            ..BASE_FRAME
        };
        drive(&mut state, halfway);
        assert!(!state.enabled, "内容高与可视区高对调后这里会误判成已到底");
        assert!(!state.take_pending(VIEW_KEY));

        drive(
            &mut state,
            Frame {
                offset_y: BOTTOM_OFFSET,
                ..halfway
            },
        );
        assert!(state.enabled, "贴到底且继续向下，才该恢复跟随");
        assert!(state.take_pending(VIEW_KEY));
    }

    /// 开着跟随却没贴到底：每帧都要自己补一次“滚到底”，但只在真有需要时补。
    ///
    /// 这条管 `update` 里独立于状态机的那半个判定（`should_repair_stick_to_bottom`）。
    /// 第一帧是判别帧：内容比可视区长、偏移停在半路、本帧没有滚轮，必须补。
    /// `scroll_delta_y` 与 `previous_offset_y` 在这一帧对调，会把半路的 400 读成正向
    /// 滚轮量，跟随当场掉掉、补滚也就没了；`scroll_is_at_bottom(offset_y, content_height,
    /// viewport_height)` 里任意一对相邻实参对调都会把这帧读成“已经到底”，于是该补的不补。
    /// 后两帧管另两个边界：内容比可视区短（压根没得滚）与已经贴底 —— 都不许补。
    #[test]
    fn a_followed_view_that_is_not_at_the_bottom_queues_one_repair() {
        let mut state = AutoScrollState::default();

        drive(&mut state, BASE_FRAME);
        assert!(state.enabled);
        assert!(
            state.take_pending(VIEW_KEY),
            "开着跟随却停在半路，本帧就该把视图按回底部"
        );

        drive(
            &mut state,
            Frame {
                content_height: 50.0,
                offset_y: 0.0,
                ..BASE_FRAME
            },
        );
        assert!(
            !state.take_pending(VIEW_KEY),
            "内容比可视区短，没有可滚的余地，不该补滚"
        );

        drive(
            &mut state,
            Frame {
                offset_y: BOTTOM_OFFSET,
                scroll_delta_y: WHEEL_DOWN,
                ..BASE_FRAME
            },
        );
        assert!(state.enabled, "向下滚轮是追最新，不是放弃跟随");
        assert!(!state.take_pending(VIEW_KEY), "已经贴底了，不需要再补一次");
    }

    /// 每个视图各存自己那份上一帧偏移。
    ///
    /// 终端按端口展开成多个视图，共用一个 `AutoScrollState`：判别量是 `key`。前两帧给
    /// 两个视图各播一个偏移（400 与 900），第三帧才让第一个视图贴到底 —— 恢复跟随并只
    /// 为它自己排一次滚到底。第二帧那句“都不该恢复”拦的是 `previous_offsets` 退化成
    /// 一份全局偏移：那时第二个视图会拿第一个视图的 400 当自己的上一帧，自己的 900
    /// 就成了“朝底部移动”，跟随凭空恢复。
    #[test]
    fn each_view_keeps_its_own_previous_offset() {
        let mut state = AutoScrollState {
            enabled: false,
            ..Default::default()
        };

        drive(&mut state, BASE_FRAME);
        drive(
            &mut state,
            Frame {
                key: OTHER_VIEW_KEY,
                offset_y: BOTTOM_OFFSET,
                ..BASE_FRAME
            },
        );
        assert!(!state.enabled, "两个视图各自贴自己的上一帧，都不该恢复");
        assert!(!state.take_pending(OTHER_VIEW_KEY));

        drive(
            &mut state,
            Frame {
                offset_y: BOTTOM_OFFSET,
                ..BASE_FRAME
            },
        );
        assert!(state.enabled);
        assert!(state.take_pending(VIEW_KEY));
        assert!(
            !state.take_pending(OTHER_VIEW_KEY),
            "另一个视图的位置变化不该替它排队"
        );
    }
}
