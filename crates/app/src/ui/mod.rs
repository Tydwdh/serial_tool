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
use crate::bootstrap::{ACTIVITY_BAR_WIDTH, BOTTOM_PANEL_MIN};
use eframe::egui;
use tool_panels::{DockArea, PanelKind, theme};

impl WorkbenchApp {
    pub(crate) fn draw_shell(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.bottom_dock_rect = None;
        self.right_dock_rect = None;

        egui::Panel::top("top-bar").show_inside(ui, |ui| self.top_bar(ui));

        if self.panels.dock.activity_bar_visible {
            egui::Panel::left("activity-bar")
                .resizable(false)
                .exact_size(ACTIVITY_BAR_WIDTH)
                .show_inside(ui, |ui| self.activity_bar(ui));
        }

        if self.panels.dock.right_visible {
            let shown = egui::Panel::right("right-dock")
                .resizable(true)
                .default_size(self.panels.dock.right_size)
                .min_size(220.0)
                .show_separator_line(true)
                .show_inside(ui, |ui| {
                    self.dock_stack_ui(ui, DockArea::Right);
                });

            self.right_dock_rect = Some(shown.response.rect);
            self.panels.dock.right_size = shown.response.rect.width().max(220.0);
        }

        if self.panels.dock.bottom_visible {
            let shown = egui::Panel::bottom("bottom-dock")
                .resizable(true)
                .default_size(self.panels.dock.bottom_size)
                .min_size(BOTTOM_PANEL_MIN)
                .show_separator_line(true)
                .show_inside(ui, |ui| {
                    let total_height = ui.available_height();
                    ui.allocate_ui_with_layout(
                        ui.available_size(),
                        egui::Layout::bottom_up(egui::Align::Min),
                        |ui| {
                            self.status_bar(ui);
                            ui.separator();

                            // 下层：发送器（固定区域）
                            if self.panels.dock.bottom_sender_visible
                                && !self.send.popup_open
                            {
                                let sender_h = self
                                    .panels
                                    .dock
                                    .bottom_sender_height
                                    .clamp(72.0, total_height * 0.45);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(ui.available_width(), sender_h),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        self.send_bar(ui);
                                    },
                                );
                                ui.separator();
                            }

                            // 上层：接收/日志/图表的 DockStack
                            let remain_h = ui.available_height().max(80.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), remain_h),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    self.dock_stack_ui(ui, DockArea::Bottom);
                                },
                            );
                        },
                    );
                });

            self.bottom_dock_rect = Some(shown.response.rect);
            self.panels.dock.bottom_size = shown.response.rect.height().max(BOTTOM_PANEL_MIN);
            // 同步发送器区域高度
            if self.panels.dock.bottom_sender_visible {
                self.panels.dock.bottom_sender_height =
                    (shown.response.rect.height() * 0.25).clamp(72.0, 200.0);
            }
        }
        //中心面板
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG_PRIMARY))
            .show_inside(ui, |ui| {
                egui::Frame::default()
                    .fill(theme::BG_PRIMARY)
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        self.dock_stack_ui(ui, DockArea::Center);
                    });
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
}

pub(crate) fn baud_combo(ui: &mut egui::Ui, id: &'static str, w: f32, baud: &mut String) {
    let r = [
        "9600", "19200", "38400", "57600", "115200", "230400", "460800", "921600",
    ];
    egui::ComboBox::from_id_salt(id)
        .width(w)
        .selected_text(baud.clone())
        .show_ui(ui, |ui| {
            for x in r {
                ui.selectable_value(baud, x.to_owned(), x);
            }
        });
}
