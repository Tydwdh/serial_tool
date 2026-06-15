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
                    let width = ui.available_width();
                    let total_h = ui.available_height();
                    const STATUS_H: f32 = 26.0;
                    const SEP_H: f32 = 8.0;
                    const OUTPUT_MIN_H: f32 = 120.0;
                    const SENDER_MIN_H: f32 = 190.0;

                    let sender_visible = self.panels.dock.bottom_sender_visible
                        && !self.send.popup_open;

                    let max_sender_h = if sender_visible {
                        (total_h - STATUS_H - SEP_H * 2.0 - OUTPUT_MIN_H).max(0.0)
                    } else {
                        0.0
                    };

                    let sender_h = if sender_visible && max_sender_h >= SENDER_MIN_H {
                        self.panels.dock.bottom_sender_height.clamp(SENDER_MIN_H, max_sender_h)
                    } else if sender_visible {
                        max_sender_h
                    } else {
                        0.0
                    };

                    let output_h = (total_h - sender_h - STATUS_H - SEP_H * 2.0).max(0.0);

                    // 上层：接收 / 日志
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, output_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            self.dock_stack_ui(ui, DockArea::Bottom);
                        },
                    );

                    ui.separator();

                    // 中层：发送器
                    if sender_h > 0.0 {
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, sender_h),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                self.send_panel_horizontal(ui);
                            },
                        );
                        ui.separator();
                    }

                    // 底层：状态栏
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, STATUS_H),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            self.status_bar(ui);
                        },
                    );
                });

            self.bottom_dock_rect = Some(shown.response.rect);
            // 保存高度（仅变化>1px时，避免干扰拖拽）
            let h = shown.response.rect.height();
            if (h - self.panels.dock.bottom_size).abs() > 1.0 {
                self.panels.dock.bottom_size = h.max(BOTTOM_PANEL_MIN);
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
