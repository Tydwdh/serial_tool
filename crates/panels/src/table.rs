//! 表格行渲染公共工具：行高亮、右键菜单行匹配、冻结状态管理。
//!
//! `RowHighlight` 被日志面板和终端面板共用，消除重复代码。

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
        let frozen_y: Option<(f32, f32)> =
            ui.data_mut(|d| d.get_persisted(frozen_y_id)).flatten();

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
    pub(crate) fn record_row(
        &mut self,
        _current_y: f32,
        _entry_height: f32,
    ) -> usize {
        let index = self.row_y_ranges.len();
        self.row_y_ranges.push((_current_y, _current_y + _entry_height));
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
            let frozen_y = ui
                .input(|i| i.pointer.hover_pos())
                .map(|p| (p.y, p.y));
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

    /// 创建右键菜单所需的 click response。
    /// 用 `ui.max_rect()` 覆盖整个可用区域（包括行数据之外的空白），
    /// 这样在空白区域右键也能弹出菜单。
    pub(crate) fn click_response(&self, ui: &mut Ui) -> egui::Response {
        let rect = ui.max_rect();
        ui.interact(rect, ui.next_auto_id(), Sense::click())
    }
}
