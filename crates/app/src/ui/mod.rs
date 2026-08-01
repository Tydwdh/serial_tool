mod bottom_panel;
pub(crate) mod command_palette;
mod contributions;
mod device_panel;
pub(crate) mod popups;
mod settings_panel;
mod status_bar;
mod tiles;
pub(crate) mod toast;
mod top_bar;

use crate::app::WorkbenchApp;
use eframe::egui;
use tool_panels::theme;

impl WorkbenchApp {
    pub(crate) fn draw_shell(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        egui::Panel::top("top-bar").show(ui, |ui| {
            self.top_bar(ui);
        });

        // 固定状态栏：永远贴在窗口最底部，不参与 bottom-dock resize。
        egui::Panel::bottom("status-bar")
            .resizable(false)
            .exact_size(26.0)
            .show_separator_line(true)
            .show(ui, |ui| {
                self.status_bar(ui);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::bg_deep()))
            .show(ui, |ui| {
                egui::Frame::default()
                    .fill(theme::bg_deep())
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        self.tiles_ui(ui);
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
