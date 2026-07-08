//! 表格行渲染公共工具：行高亮、行选择、右键菜单行匹配、冻结状态管理。
//!
//! `RowHighlight` 被日志面板和终端面板共用，消除重复代码。
//! `RowSelection` 提供框选、Shift/Ctrl 多选、边缘自动滚动。

use std::collections::BTreeSet;

use egui::{Id, Ui};

use crate::theme;

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
    row_y_ranges: Vec<(f32, f32)>,
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
                theme::WIDGET_HOVER,
            );
        }
        should_highlight
    }

    /// 在每行循环内调用：记录该行的 Y 范围，返回行索引。
    /// 参数 `current_y` 是该行顶部 Y，`entry_height` 是该行高度。
    pub(crate) fn record_row(&mut self, _current_y: f32, _entry_height: f32) -> usize {
        let index = self.row_y_ranges.len();
        self.row_y_ranges
            .push((_current_y, _current_y + _entry_height));
        index
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
                    .position(|(top, bottom)| pointer.y >= *top && pointer.y < *bottom)
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
                .position(|(top, bottom)| pointer.y >= *top && pointer.y < *bottom)
        })
    }

    /// 根据 Y 坐标查找行索引。
    pub(crate) fn row_index_at_y(&self, y: f32) -> Option<usize> {
        self.row_y_ranges
            .iter()
            .position(|(top, bottom)| y >= *top && y < *bottom)
    }

    /// 获取指定行索引的 Y 范围。
    pub(crate) fn row_y_range(&self, index: usize) -> Option<(f32, f32)> {
        self.row_y_ranges.get(index).copied()
    }

    /// 根据 Y 坐标查找行；拖拽越过首尾时钳制到第一/最后一行。
    pub(crate) fn row_index_at_y_clamped(&self, y: f32) -> Option<usize> {
        let first = self.row_y_ranges.first()?;
        if y < first.0 {
            return Some(0);
        }

        let last_index = self.row_y_ranges.len() - 1;
        let last = self.row_y_ranges[last_index];
        if y >= last.1 {
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

    fn begin_pointer(&mut self, index: usize, ctrl: bool, shift: bool) {
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
        ui.painter_at(full_rect)
            .rect_filled(sel_rect, 0.0, theme::WIDGET_HOVER);
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

#[cfg(test)]
mod tests {
    use super::{RowHighlight, RowSelection, edge_scroll_delta};

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
}
