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
use tool_panels::{DockArea, theme};

impl WorkbenchApp {
    pub(crate) fn draw_shell(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.bottom_dock_rect = None;
        self.right_dock_rect = None;

        egui::Panel::top("top-bar").show_inside(ui, |ui| {
            self.top_bar(ui);
        });

        if self.panels.dock.activity_bar_visible {
            egui::Panel::left("activity-bar")
                .resizable(false)
                .exact_size(ACTIVITY_BAR_WIDTH)
                .show_inside(ui, |ui| {
                    self.activity_bar(ui);
                });
        }

        // 固定状态栏：永远贴在窗口最底部，不参与 bottom-dock resize。
        egui::Panel::bottom("status-bar")
            .resizable(false)
            .exact_size(26.0)
            .show_separator_line(true)
            .show_inside(ui, |ui| {
                self.status_bar(ui);
            });

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
                .default_size(self.panels.dock.bottom_size.max(BOTTOM_PANEL_MIN))
                .min_size(BOTTOM_PANEL_MIN)
                .show_inside(ui, |ui| {
                    let width = ui.available_width();
                    let total_h = ui.available_height();
                    // 关键：让 resizable bottom panel 的内容吃掉拖拽后的可用高度。
                    // 否则 egui 会按内容实际高度保存 PanelState，松手后自动收缩。
                    ui.take_available_height();
                    const OUTPUT_MIN_H: f32 = 120.0;
                    const SENDER_MIN_H: f32 = 190.0;

                    let sender_visible =
                        self.panels.dock.bottom_sender_visible && !self.send.popup_open;

                    let max_sender_h = if sender_visible {
                        (total_h - OUTPUT_MIN_H).max(0.0)
                    } else {
                        0.0
                    };

                    let sender_h = if sender_visible && max_sender_h >= SENDER_MIN_H {
                        self.panels
                            .dock
                            .bottom_sender_height
                            .clamp(SENDER_MIN_H, max_sender_h)
                    } else if sender_visible {
                        max_sender_h
                    } else {
                        0.0
                    };

                    let output_h = if sender_h > 0.0 {
                        (total_h - sender_h).max(0.0)
                    } else {
                        total_h
                    };

                    // 上层：接收 / 日志 / 图表输出 dock。
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, output_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            self.dock_stack_ui(ui, DockArea::Bottom);
                        },
                    );

                    if sender_h > 0.0 {
                        ui.separator();

                        // 下层：发送器。
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, sender_h),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                self.send_panel_horizontal(ui);
                            },
                        );
                    }
                });

            self.bottom_dock_rect = Some(shown.response.rect);
        }

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
    }
}

pub(crate) fn baud_combo(ui: &mut egui::Ui, id: &'static str, w: f32, baud: &mut String) {
    let rates = [
        "9600", "19200", "38400", "57600", "115200", "230400", "460800", "921600",
    ];

    egui::ComboBox::from_id_salt(id)
        .width(w)
        .selected_text(baud.clone())
        .show_ui(ui, |ui| {
            for rate in rates {
                ui.selectable_value(baud, rate.to_owned(), rate);
            }
        });
}
