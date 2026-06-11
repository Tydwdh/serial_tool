use crate::app::WorkbenchApp;
use eframe::egui;
use tool_panels::Activity;
impl WorkbenchApp {
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            if i.modifiers.ctrl && i.key_pressed(egui::Key::R) && !i.modifiers.shift {
                self.refresh_ports();
            }
            if i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::O) {
                self.open_selected_port();
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::B) {
                self.toggle_bottom_panel();
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::I) {
                self.panels.inspector_visible = !self.panels.inspector_visible;
            }
            if i.modifiers.ctrl {
                for (k, a) in [
                    (egui::Key::Num1, Activity::Devices),
                    (egui::Key::Num2, Activity::Replay),
                    (egui::Key::Num3, Activity::Plugins),
                    (egui::Key::Num4, Activity::Settings),
                ] {
                    if i.key_pressed(k) {
                        self.panels.select_activity(a);
                    }
                }
            }
        });
    }
}
