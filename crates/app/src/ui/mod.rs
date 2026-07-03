mod activity_bar;
mod bottom_panel;
mod command_palette;
mod contributions;
mod device_panel;
mod dock;
mod layout_buttons;
mod popups;
mod settings_panel;
mod status_bar;
mod top_bar;

use crate::app::WorkbenchApp;
use crate::bootstrap::{ACTIVITY_BAR_WIDTH, BOTTOM_PANEL_MIN};
use eframe::egui;
use tool_panels::{DockArea, theme};

impl WorkbenchApp {
    pub(crate) fn draw_shell(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.bottom_dock_rect = None;
        self.right_dock_rect = None;

        egui::Panel::top("top-bar").show(ui, |ui| {
            self.top_bar(ui);
        });

        if self.panels.dock.activity_bar_visible {
            egui::Panel::left("activity-bar")
                .resizable(false)
                .exact_size(ACTIVITY_BAR_WIDTH)
                .show(ui, |ui| {
                    self.activity_bar(ui);
                });
        }

        // 固定状态栏：永远贴在窗口最底部，不参与 bottom-dock resize。
        egui::Panel::bottom("status-bar")
            .resizable(false)
            .exact_size(26.0)
            .show_separator_line(true)
            .show(ui, |ui| {
                self.status_bar(ui);
            });

        if self.panels.dock.right_visible {
            let shown = egui::Panel::right("right-dock")
                .resizable(true)
                .default_size(self.panels.dock.right_size)
                .min_size(220.0)
                .frame(egui::Frame::NONE)
                .show_separator_line(true)
                .show(ui, |ui| {
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
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    self.dock_stack_ui(ui, DockArea::Bottom);
                });
            self.bottom_dock_rect = Some(shown.response.rect);
        }

        self.paint_dock_drop_overlay(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG_DEEP))
            .show(ui, |ui| {
                egui::Frame::default()
                    .fill(theme::BG_DEEP)
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        self.dock_stack_ui(ui, DockArea::Center);
                    });
            });
    }
}

pub(crate) fn baud_combo(ui: &mut egui::Ui, id: &'static str, w: f32, baud: &mut String) {
    // 常用档位（含高速）：点选直接设置。不在列表的值（如自定义 3_000_000）
    // 仍显示在 selected_text，下方 TextEdit 提供自由输入入口。
    let rates = [
        "9600", "19200", "38400", "57600", "115200", "230400", "460800", "921600", "128000",
        "256000", "512000", "1000000", "2000000", "3000000",
    ];

    egui::ComboBox::from_id_salt(id)
        .width(w)
        .selected_text(baud.clone())
        .show_ui(ui, |ui| {
            for rate in rates {
                ui.selectable_value(baud, rate.to_owned(), rate);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("自定义");
                ui.add(
                    egui::TextEdit::singleline(baud)
                        .desired_width(80.0)
                        .hint_text("bps"),
                );
            });
        });
}
