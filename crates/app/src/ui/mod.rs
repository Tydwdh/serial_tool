use crate::ui::activity_bar::aicon;
pub mod activity_bar;
pub mod bottom_panel;
pub mod contributions;
pub mod device_panel;
pub mod dock;
pub mod layout_buttons;
pub mod popups;
pub mod settings_panel;
pub mod status_bar;
pub mod top_bar;

use crate::app::WorkbenchApp;
use crate::bootstrap::{ACTIVITY_BAR_WIDTH, BOTTOM_PANEL_HEIGHT, BOTTOM_PANEL_MIN};
use eframe::egui;
use tool_panels::{Activity, DockArea, PanelKind, theme};

impl WorkbenchApp {
    pub(crate) fn draw_shell(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::top("top-bar").show_inside(ui, |ui| self.top_bar(ui));

        if self.panels.dock.activity_bar_visible {
            egui::Panel::left("activity-bar")
                .resizable(false)
                .default_size(ACTIVITY_BAR_WIDTH)
                .show_inside(ui, |ui| self.activity_bar(ui));
        }

        if self.panels.dock.right_visible {
            egui::Panel::right("right-dock")
                .resizable(true)
                .default_size(self.panels.dock.right_size)
                .min_size(220.0)
                .show_separator_line(true)
                .show_inside(ui, |ui| {
                    self.dock_stack_ui(ui, DockArea::Right);
                });
        }

        if self.panels.dock.bottom_visible {
            egui::Panel::bottom("bottom-dock")
                .resizable(true)
                .min_size(BOTTOM_PANEL_MIN)
                .default_size(self.panels.dock.bottom_size)
                .show_separator_line(true)
                .show_inside(ui, |ui| {
                    let total = ui.available_size();
                    ui.allocate_ui_with_layout(
                        total,
                        egui::Layout::bottom_up(egui::Align::Min),
                        |ui| {
                            self.status_bar(ui);

                            ui.separator();

                            let bottom_active = self.panels.dock.bottom.active_or_first();
                            if matches!(bottom_active, Some(PanelKind::Terminal))
                                && !self.send.popup_open
                                && !self.terminal_popup_open
                            {
                                self.send_bar(ui);
                                ui.separator();
                            }

                            let dock_height = ui.available_height().max(80.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), dock_height),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    self.dock_stack_ui(ui, DockArea::Bottom);
                                },
                            );
                        },
                    );
                });
        } else {
            egui::Panel::bottom("status-only")
                .resizable(false)
                .show_separator_line(false)
                .default_size(24.0)
                .show_inside(ui, |ui| self.status_bar(ui));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG_PRIMARY))
            .show_inside(ui, |ui| {
                self.dock_stack_ui(ui, DockArea::Center);
            });

        self.paint_dock_drop_overlay(ctx);

        // 浮动拖拽副本
        if let Some(s) = self.activity_drag_source
            && s < self.activity_order.len()
            && let Some(p) = ctx.pointer_latest_pos()
        {
            let act = self.activity_order[s];
            let label = format!("{} {}", aicon(act), act.label());
            let gal = ctx.fonts_mut(|f| {
                f.layout(
                    label.clone(),
                    egui::FontId::proportional(12.0),
                    theme::TEXT_PRIMARY,
                    f32::INFINITY,
                )
            });
            let rect = egui::Rect::from_min_size(
                p + egui::vec2(8.0, -12.0),
                egui::vec2(gal.size().x + 16.0, 26.0),
            );
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("dghost"),
            ));
            painter.rect_filled(
                rect,
                5.0,
                egui::Color32::from_rgba_premultiplied(46, 80, 120, 210),
            );
            painter.galley(
                rect.center() - gal.size() * 0.5,
                gal,
                egui::Color32::from_rgba_premultiplied(255, 255, 255, 240),
            );
        }
    }

    pub(crate) fn draw_drag_ghost(&self, ctx: &egui::Context) {
        if let Some(s) = self.activity_drag_source
            && s < self.activity_order.len()
            && let Some(p) = ctx.pointer_latest_pos()
        {
            let act = self.activity_order[s];
            let label = format!("{} {}", aicon(act), act.label());
            let gal = ctx.fonts_mut(|f| {
                f.layout(
                    label.clone(),
                    egui::FontId::proportional(12.0),
                    theme::TEXT_PRIMARY,
                    f32::INFINITY,
                )
            });
            let rect = egui::Rect::from_min_size(
                p + egui::vec2(8.0, -12.0),
                egui::vec2(gal.size().x + 16.0, 26.0),
            );
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("dghost"),
            ));
            painter.rect_filled(
                rect,
                5.0,
                egui::Color32::from_rgba_premultiplied(46, 80, 120, 210),
            );
            painter.galley(
                rect.center() - gal.size() * 0.5,
                gal,
                egui::Color32::from_rgba_premultiplied(255, 255, 255, 240),
            );
        }
    }
}
