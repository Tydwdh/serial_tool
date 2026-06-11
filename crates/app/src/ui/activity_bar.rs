use crate::app::{
    WorkbenchApp, activity_insert_index_from_pointer, aicon, ashortcut, paint_activity_insert_line,
    paint_vertical_insert_line, vertical_insert_index_from_pointer,
};
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

        if self.activity_drag_source.is_some() && ui.input(|i| i.pointer.any_released()) {
            if let Some(source_index) = self.activity_drag_source.take() {
                if let Some(mut insert_index) = drag_insert_index {
                    insert_index = insert_index.min(self.activity_order.len());

                    if insert_index > source_index {
                        insert_index -= 1;
                    }

                    if insert_index != source_index {
                        let item = self.activity_order.remove(source_index);
                        let insert_index = insert_index.min(self.activity_order.len());
                        self.activity_order.insert(insert_index, item);
                        let _ = self.save_config();
                    }
                }
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
            .tabs
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
                self.panels.open_tab(PanelKind::Dynamic(id.clone()));
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

        if self.dynamic_drag_source.is_some() && ui.input(|input| input.pointer.any_released()) {
            if let Some(source_index) = self.dynamic_drag_source.take() {
                if let Some(insert_index) = insert_index {
                    self.reorder_dynamic_tabs(source_index, insert_index);
                }
            }
        }

        if self.dynamic_drag_source.is_some() && !ui.input(|input| input.pointer.primary_down()) {
            self.dynamic_drag_source = None;
        }
    }
    pub(crate) fn reorder_dynamic_tabs(&mut self, source_index: usize, mut insert_index: usize) {
        let mut dynamic_tabs: Vec<PanelKind> = self
            .panels
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

        let mut dynamic_iter = dynamic_tabs.into_iter();

        for kind in &mut self.panels.tabs {
            if kind.dynamic_id().is_some() {
                if let Some(next) = dynamic_iter.next() {
                    *kind = next;
                }
            }
        }

        let _ = self.save_config();
    }
}
