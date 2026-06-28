//! 表格行渲染公共工具：行高亮、行选择、右键菜单行匹配、冻结状态管理。
//!
//! `RowHighlight` 被日志面板和终端面板共用，消除重复代码。
//! `RowSelection` 提供框选、Shift/Ctrl 多选、边缘自动滚动。

use egui::{Id, Sense, Ui};

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

    /// 创建右键菜单所需的 click response。
    /// 用 `ui.max_rect()` 覆盖整个可用区域（包括行数据之外的空白），
    /// 这样在空白区域右键也能弹出菜单。
    pub(crate) fn click_response(&self, ui: &mut Ui) -> egui::Response {
        let rect = ui.max_rect();
        ui.interact(rect, ui.next_auto_id(), Sense::click())
    }
}

// ── RowSelection：多行框选 ──

/// 行选择状态：支持左键拖拽框选、单击选中、Shift 扩展、Ctrl 追加。
pub struct RowSelection {
    /// 锚点行（Shift 扩展的起点，也是最后一次单击的行）
    anchor: Option<usize>,
    /// 当前拖拽/扩展到的行
    cursor: Option<usize>,
    /// 各行的独立选中状态（用于 Ctrl+点击追加/取消单行）
    pinned: Vec<bool>,
}

impl RowSelection {
    pub fn new(row_count: usize) -> Self {
        Self {
            anchor: None,
            cursor: None,
            pinned: vec![false; row_count],
        }
    }

    /// 更新行数（数据变化后调用）。
    pub fn resize(&mut self, row_count: usize) {
        self.pinned.resize(row_count, false);
        if let Some(a) = self.anchor
            && a >= row_count
        {
            self.anchor = None;
            self.cursor = None;
            self.pinned.fill(false);
        }
    }

    pub fn has_selection(&self) -> bool {
        self.anchor.is_some()
    }

    /// 返回选中的行索引范围（lo, hi 含两端）。
    pub fn selected_range(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        let c = self.cursor?;
        Some(if a <= c { (a, c) } else { (c, a) })
    }

    /// 该行是否在选中范围内。
    pub fn is_selected(&self, index: usize) -> bool {
        self.selected_range()
            .is_some_and(|(lo, hi)| index >= lo && index <= hi)
    }

    /// 清除所有选中。
    pub fn clear(&mut self) {
        self.anchor = None;
        self.cursor = None;
        self.pinned.fill(false);
    }

    /// 事件处理入口。调用方在每帧传入当前交互状态。
    pub fn handle_input(
        &mut self,
        ui: &Ui,
        response: &egui::Response,
        hovered_row: Option<usize>,
        scroll_delta: &mut f32,
    ) {
        let ctrl = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
        let shift = ui.input(|i| i.modifiers.shift);

        // Ctrl+点击：切换单行
        if response.clicked_by(egui::PointerButton::Primary) && ctrl {
            if let Some(idx) = hovered_row {
                let was_selected = self.is_selected(idx);
                // 清除范围选择，转为 pinned 模式
                self.anchor = None;
                self.cursor = None;
                // 确保 pinned 够长
                if idx >= self.pinned.len() {
                    self.pinned.resize(idx + 1, false);
                }
                self.pinned[idx] = !was_selected;
            }
            return;
        }

        // Shift+点击：扩展范围
        if response.clicked_by(egui::PointerButton::Primary) && shift {
            if let Some(idx) = hovered_row {
                self.pinned.fill(false);
                self.cursor = Some(idx);
                if self.anchor.is_none() {
                    self.anchor = Some(idx);
                }
            }
            return;
        }

        // 普通单击：选中单行
        if response.clicked_by(egui::PointerButton::Primary) {
            self.pinned.fill(false);
            if let Some(idx) = hovered_row {
                self.anchor = Some(idx);
                self.cursor = Some(idx);
            }
            return;
        }

        // 拖拽开始
        if response.drag_started_by(egui::PointerButton::Primary) {
            self.pinned.fill(false);
            if let Some(idx) = hovered_row {
                self.anchor = Some(idx);
                self.cursor = Some(idx);
            }
            return;
        }

        // 拖拽中：更新范围 + 边缘滚动
        if response.dragged_by(egui::PointerButton::Primary) {
            if let Some(idx) = hovered_row {
                self.cursor = Some(idx);
            }
            // 边缘滚动：指针在 scroll area 顶部/底部时持续滚动
            if let Some(pointer_y) = response.interact_pointer_pos().map(|p| p.y) {
                let rect = response.rect;
                let edge = 20.0;
                if pointer_y < rect.top() + edge {
                    *scroll_delta -= (edge - (pointer_y - rect.top())) * 0.3;
                } else if pointer_y > rect.bottom() - edge {
                    *scroll_delta += (edge - (rect.bottom() - pointer_y)) * 0.3;
                }
            }
        }
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
