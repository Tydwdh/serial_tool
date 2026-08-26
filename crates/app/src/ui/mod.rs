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

impl WorkbenchApp {
    pub(crate) fn draw_shell(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        crate::shared_shell::show_shell(self, ui);
    }
}

impl crate::shared_shell::AppShellHost for WorkbenchApp {
    fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        self.top_bar(ui);
    }

    fn render_status_bar(&mut self, ui: &mut egui::Ui) {
        self.status_bar(ui);
    }

    fn after_dock(&mut self, ui: &egui::Ui) {
        if self.layout_dirty && !ui.input(|input| input.pointer.primary_down()) {
            self.layout_dirty = false;
            if let Err(error) = self.save_config() {
                log::warn!("save_config failed: {error}");
            }
        }
    }
}
