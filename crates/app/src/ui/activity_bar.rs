use crate::app::WorkbenchApp;
use eframe::egui;
use egui::Color32;
use tool_panels::{PanelKind, theme};

impl WorkbenchApp {
    pub(crate) fn activity_bar(&mut self, ui: &mut egui::Ui) {
        let pointer = ui.ctx().pointer_latest_pos();
        let mut activity_rects = Vec::with_capacity(self.activity_order.len());

        ui.vertical_centered(|ui| {
            for (idx, &act) in self.activity_order.iter().enumerate() {
                let selected =
                    self.panels.active_dynamic_id().is_none() && self.panels.activity == act;
                let label = format!("{} {}", aicon(act), act.label());
                let shortcut = ashortcut(act);

                let hover = if shortcut.is_empty() {
                    act.label().to_owned()
                } else {
                    format!("{} ({})", act.label(), shortcut)
                };

                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 28.0),
                    egui::Sense::click_and_drag(),
                );

                if response.drag_started() {
                    self.activity_drag_source = Some(idx);
                }

                if response.clicked() && self.activity_drag_source.is_none() {
                    self.panels.select_activity(act);
                    if let Some(kind) = act.panel_kind() {
                        self.panels
                            .dock
                            .move_panel(kind, tool_panels::DockArea::Center);
                        self.panels.sync_tabs_from_dock();
                    }
                }

                let is_source = self.activity_drag_source == Some(idx);

                let bg = if is_source {
                    theme::BG_TERTIARY
                } else if selected || response.hovered() {
                    if selected {
                        theme::BG_SELECTION
                    } else {
                        theme::WIDGET_HOVER
                    }
                } else {
                    theme::BG_SECONDARY
                };

                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 4.0, bg);

                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &label,
                    egui::FontId::proportional(12.0),
                    if is_source {
                        theme::TEXT_SECONDARY
                    } else {
                        theme::TEXT_PRIMARY
                    },
                );

                response.on_hover_text(hover);

                activity_rects.push(rect);
            }
        });

        let drag_insert_index = if self.activity_drag_source.is_some() {
            pointer.and_then(|pos| activity_insert_index_from_pointer(&activity_rects, pos))
        } else {
            None
        };

        if let Some(insert_index) = drag_insert_index {
            paint_activity_insert_line(ui, &activity_rects, insert_index);
        }

        if self.activity_drag_source.is_some()
            && ui.input(|i| i.pointer.any_released())
            && let Some(source_index) = self.activity_drag_source.take()
            && let Some(mut insert_index) = drag_insert_index
        {
            insert_index = insert_index.min(self.activity_order.len());

            if insert_index > source_index {
                insert_index -= 1;
            }

            if insert_index != source_index {
                let item = self.activity_order.remove(source_index);
                let insert_index = insert_index.min(self.activity_order.len());
                self.activity_order.insert(insert_index, item);
                if let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
            }
        }

        if self.activity_drag_source.is_some() && !ui.input(|i| i.pointer.primary_down()) {
            self.activity_drag_source = None;
        }

        self.activity_rects_cache = activity_rects;

        self.dynamic_panel_shortcuts(ui);

        ui.separator();

        if ui
            .selectable_label(self.bottom_panel_visible, "▽ 终端区")
            .on_hover_text("Ctrl+B")
            .clicked()
        {
            self.toggle_bottom_panel();
        }
    }
    pub(crate) fn dynamic_panel_shortcuts(&mut self, ui: &mut egui::Ui) {
        let items: Vec<(String, String)> = self
            .panels
            .tabs()
            .iter()
            .filter_map(|kind| kind.dynamic_id().map(|id| id.to_owned()))
            .filter(|id| self.dynamic_panels.contains(id))
            .map(|id| {
                let title = self.dynamic_panels.title(&id).unwrap_or(&id).to_owned();
                (id, title)
            })
            .collect();

        if items.is_empty() {
            return;
        }

        ui.separator();

        let pointer = ui.ctx().pointer_latest_pos();
        let mut rects = Vec::with_capacity(items.len());

        for (index, (id, title)) in items.iter().enumerate() {
            let active = self.panels.active_dynamic_id() == Some(id);
            let is_source = self.dynamic_drag_source == Some(index);

            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 24.0),
                egui::Sense::click_and_drag(),
            );

            if response.drag_started() {
                self.dynamic_drag_source = Some(index);
            }

            if response.clicked() && self.dynamic_drag_source.is_none() {
                self.panels.open_tab(PanelKind::Dynamic(id.to_owned()));
            }

            let bg = if is_source {
                theme::BG_TERTIARY
            } else if active || response.hovered() {
                if active {
                    theme::BG_SELECTION
                } else {
                    theme::WIDGET_HOVER
                }
            } else {
                Color32::TRANSPARENT
            };

            let painter = ui.painter_at(rect);

            if bg != Color32::TRANSPARENT {
                painter.rect_filled(rect, 4.0, bg);
            }

            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("  {title}"),
                egui::FontId::proportional(12.0),
                if is_source {
                    theme::TEXT_SECONDARY
                } else {
                    theme::TEXT_PRIMARY
                },
            );

            response.on_hover_text("拖动调整插件标签顺序");

            rects.push(rect);
        }

        let insert_index = if self.dynamic_drag_source.is_some() {
            pointer.and_then(|pos| vertical_insert_index_from_pointer(&rects, pos))
        } else {
            None
        };

        if let Some(insert_index) = insert_index {
            paint_vertical_insert_line(ui, &rects, insert_index);
        }

        if self.dynamic_drag_source.is_some()
            && ui.input(|input| input.pointer.any_released())
            && let Some(source_index) = self.dynamic_drag_source.take()
            && let Some(insert_index) = insert_index
        {
            self.reorder_dynamic_tabs(source_index, insert_index);
        }

        if self.dynamic_drag_source.is_some() && !ui.input(|input| input.pointer.primary_down()) {
            self.dynamic_drag_source = None;
        }
    }
    pub(crate) fn reorder_dynamic_tabs(&mut self, source_index: usize, mut insert_index: usize) {
        let mut dynamic_tabs: Vec<PanelKind> = self
            .panels
            .dock
            .center
            .tabs
            .iter()
            .filter(|kind| kind.dynamic_id().is_some())
            .cloned()
            .collect();

        if source_index >= dynamic_tabs.len() {
            return;
        }

        insert_index = insert_index.min(dynamic_tabs.len());

        if insert_index > source_index {
            insert_index -= 1;
        }

        if insert_index == source_index {
            return;
        }

        let item = dynamic_tabs.remove(source_index);
        let insert_index = insert_index.min(dynamic_tabs.len());
        dynamic_tabs.insert(insert_index, item);

        // 将重排后的动态面板顺序写回 dock.center.tabs
        let all_tabs = self.panels.dock.all_tabs();
        let mut dynamic_iter = dynamic_tabs.into_iter();
        let mut new_center: Vec<PanelKind> = Vec::with_capacity(self.panels.dock.center.tabs.len());
        for kind in &all_tabs {
            if kind.dynamic_id().is_some()
                && let Some(next) = dynamic_iter.next()
            {
                new_center.push(next);
            } else if !kind.dynamic_id().is_some() {
                new_center.push(kind.clone());
            }
        }
        self.panels.dock.center.tabs = new_center;

        if let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
    }
}

use tool_panels::Activity;

pub(crate) fn aicon(a: Activity) -> &'static str {
    match a {
        Activity::Devices => "📟",
        Activity::Replay => "⏪",
        Activity::Plugins => "🧩",
        Activity::Settings => "⚙",
        _ => "",
    }
}
pub(crate) fn ashortcut(a: Activity) -> &'static str {
    match a {
        Activity::Devices => "Ctrl+1",
        Activity::Replay => "Ctrl+2",
        Activity::Plugins => "Ctrl+3",
        Activity::Settings => "Ctrl+4",
        _ => "",
    }
}

/// 通用：根据指针位置计算插入索引。
fn insert_index_from_pointer(
    rects: &[egui::Rect],
    pointer: egui::Pos2,
    margin: f32,
) -> Option<usize> {
    if rects.is_empty() {
        return None;
    }

    let left = rects
        .iter()
        .map(|rect| rect.left())
        .fold(f32::INFINITY, f32::min);

    let right = rects
        .iter()
        .map(|rect| rect.right())
        .fold(f32::NEG_INFINITY, f32::max);

    let top = rects.first()?.top() - margin;
    let bottom = rects.last()?.bottom() + margin;

    if pointer.x < left - 16.0 || pointer.x > right + 16.0 || pointer.y < top || pointer.y > bottom
    {
        return None;
    }

    for (index, rect) in rects.iter().enumerate() {
        if pointer.y < rect.center().y {
            return Some(index);
        }
    }

    Some(rects.len())
}

/// 通用：绘制插入指示线。
fn paint_insert_line(ui: &egui::Ui, rects: &[egui::Rect], insert_index: usize) {
    if rects.is_empty() {
        return;
    }

    let left = rects
        .iter()
        .map(|rect| rect.left())
        .fold(f32::INFINITY, f32::min);

    let right = rects
        .iter()
        .map(|rect| rect.right())
        .fold(f32::NEG_INFINITY, f32::max);

    let y = if insert_index == 0 {
        rects[0].top() - 3.0
    } else if insert_index >= rects.len() {
        rects[rects.len() - 1].bottom() + 3.0
    } else {
        let above = rects[insert_index - 1];
        let below = rects[insert_index];
        (above.bottom() + below.top()) * 0.5
    };

    let painter = ui.painter();

    painter.line_segment(
        [egui::pos2(left + 6.0, y), egui::pos2(right - 6.0, y)],
        egui::Stroke::new(2.0, theme::BLUE),
    );

    painter.circle_filled(egui::pos2(left + 6.0, y), 3.0, theme::BLUE);
    painter.circle_filled(egui::pos2(right - 6.0, y), 3.0, theme::BLUE);
}

pub(crate) fn activity_insert_index_from_pointer(
    rects: &[egui::Rect],
    pointer: egui::Pos2,
) -> Option<usize> {
    insert_index_from_pointer(rects, pointer, 14.0)
}

pub(crate) fn paint_activity_insert_line(ui: &egui::Ui, rects: &[egui::Rect], insert_index: usize) {
    paint_insert_line(ui, rects, insert_index);
}

pub(crate) fn vertical_insert_index_from_pointer(
    rects: &[egui::Rect],
    pointer: egui::Pos2,
) -> Option<usize> {
    insert_index_from_pointer(rects, pointer, 10.0)
}

pub(crate) fn paint_vertical_insert_line(ui: &egui::Ui, rects: &[egui::Rect], insert_index: usize) {
    paint_insert_line(ui, rects, insert_index);
}
