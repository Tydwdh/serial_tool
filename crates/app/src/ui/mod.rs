mod bottom_panel;
pub(crate) mod command_palette;
mod contributions;
mod device_panel;
mod dialogs;
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
        egui::Panel::top("top-bar")
            .frame(
                egui::Frame::new()
                    .fill(theme::bg_secondary())
                    .stroke(egui::Stroke::new(1.0, theme::border()))
                    .inner_margin(egui::Margin::symmetric(10, 7)),
            )
            .show(ui, |ui| {
                self.top_bar(ui);
            });

        // 固定状态栏：永远贴在窗口最底部，不参与 bottom-dock resize。
        egui::Panel::bottom("status-bar")
            .resizable(false)
            .exact_size(30.0)
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
