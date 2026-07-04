use crate::app::WorkbenchApp;
use eframe::egui;
use egui::Color32;
use tool_panels::theme;

impl WorkbenchApp {
    /// 左侧栏 = Center 主工作区的标签栏。
    /// 与底部/右侧停靠区共用同一套 PanelKind + dock_dragging_panel 拖拽体系。
    pub(super) fn activity_bar(&mut self, ui: &mut egui::Ui) {
        let pointer = ui.ctx().pointer_latest_pos();
        let tabs = self.panels.dock.center.tabs.clone();

        let mut tab_rects: Vec<egui::Rect> = Vec::with_capacity(tabs.len());

        ui.vertical_centered(|ui| {
            for kind in &tabs {
                let active = self.panels.dock.center.active.as_ref() == Some(kind);
                let dragging = self.dock_dragging_panel.as_ref() == Some(kind);
                let title = self.panel_title(kind);
                let label = format!("{} {}", kind.icon(), title);

                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 28.0),
                    egui::Sense::click_and_drag(),
                );

                if response.clicked() && self.dock_dragging_panel.is_none() {
                    self.panels.select_center_panel(kind.clone());
                }

                if response.drag_started() {
                    self.dock_dragging_panel = Some(kind.clone());
                }

                if response.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    ui.ctx().request_repaint();
                }

                tab_rects.push(rect);

                let bg = if dragging {
                    theme::BG_TERTIARY
                } else if active {
                    theme::BG_SELECTION
                } else if response.hovered() {
                    theme::BG_HOVER
                } else {
                    Color32::TRANSPARENT
                };
                response.on_hover_text(&title);

                let fg = if active || dragging {
                    theme::TEXT_WHITE
                } else {
                    theme::TEXT_PRIMARY
                };

                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 4.0, bg);
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &label,
                    egui::FontId::proportional(12.0),
                    fg,
                );
            }
        });

        // 拖拽排序：在左侧栏内拖动 center tab 重排顺序
        let drag_insert_index = if self.dock_dragging_panel.is_some() {
            pointer.and_then(|pos| vertical_insert_index_from_pointer(&tab_rects, pos))
        } else {
            None
        };

        if let Some(insert_index) = drag_insert_index {
            paint_vertical_insert_line(ui, &tab_rects, insert_index);
        }

        // 拖拽释放在左侧栏区域内：重排 center.tabs（若来源也是 center）
        // 跨区拖入（从 bottom/right 拖回左侧）由 paint_dock_drop_overlay 处理。
        if self.dock_dragging_panel.is_some()
            && ui.input(|i| i.pointer.any_released())
            && let Some(kind) = self.dock_dragging_panel.clone()
            && let Some(insert_index) = drag_insert_index
        {
            // 仅当来源是 center 时做原地重排
            if self.panels.dock.center.contains(&kind) {
                let source_index = self
                    .panels
                    .dock
                    .center
                    .tabs
                    .iter()
                    .position(|k| k == &kind);
                if let Some(src) = source_index {
                    let mut ins = insert_index.min(self.panels.dock.center.tabs.len());
                    if ins > src {
                        ins -= 1;
                    }
                    if ins != src {
                        let item = self.panels.dock.center.tabs.remove(src);
                        let ins = ins.min(self.panels.dock.center.tabs.len());
                        self.panels.dock.center.tabs.insert(ins, item);
                        if let Err(e) = self.save_config() {
                            log::warn!("save_config failed: {e}")
                        };
                    }
                }
            }
            self.dock_dragging_panel = None;
        }

        // 释放但未命中任何区域：清除拖拽（跨区移动由 overlay 处理）
        if self.dock_dragging_panel.is_some() && ui.input(|i| i.pointer.any_released()) {
            // 若释放在左侧栏外且不在 bottom/right，paint_dock_drop_overlay 会处理跨区；
            // 这里仅兜底：若释放时仍在拖拽状态，清除之。
            // 注意：paint_dock_drop_overlay 在本函数之后运行，会先处理跨区命中。
        }
        if self.dock_dragging_panel.is_some() && !ui.input(|i| i.pointer.primary_down()) {
            // 鼠标已松开但 dragging 仍在 → 留给 overlay 处理；overlay 未处理则清除
            // overlay 在 paint_dock_drop_overlay 末尾会清除，这里不重复清除以免抢夺
        }

        ui.separator();

        if ui
            .selectable_label(self.panels.dock.bottom_visible, "▽ 终端区")
            .on_hover_text("Ctrl+B")
            .clicked()
        {
            self.toggle_bottom_panel();
        }
    }
}

/// 根据指针 y 计算竖排 tab 的插入索引。
fn vertical_insert_index_from_pointer(
    rects: &[egui::Rect],
    pointer: egui::Pos2,
) -> Option<usize> {
    if rects.is_empty() {
        return None;
    }
    let left = rects.iter().map(|r| r.left()).fold(f32::INFINITY, f32::min);
    let right = rects.iter().map(|r| r.right()).fold(f32::NEG_INFINITY, f32::max);
    if pointer.x < left - 16.0 || pointer.x > right + 16.0 {
        return None;
    }
    let top = rects.first()?.top() - 8.0;
    let bottom = rects.last()?.bottom() + 8.0;
    if pointer.y < top || pointer.y > bottom {
        return None;
    }
    for (index, rect) in rects.iter().enumerate() {
        if pointer.y < rect.center().y {
            return Some(index);
        }
    }
    Some(rects.len())
}

fn paint_vertical_insert_line(ui: &egui::Ui, rects: &[egui::Rect], index: usize) {
    if rects.is_empty() {
        return;
    }
    let y = if index >= rects.len() {
        rects.last().expect("non-empty").bottom() + 3.0
    } else {
        rects[index].top() - 3.0
    };
    let left = rects.iter().map(|r| r.left()).fold(f32::INFINITY, f32::min);
    let right = rects.iter().map(|r| r.right()).fold(f32::NEG_INFINITY, f32::max);
    ui.painter().line_segment(
        [egui::pos2(left, y), egui::pos2(right, y)],
        egui::Stroke::new(2.0, theme::BLUE),
    );
}