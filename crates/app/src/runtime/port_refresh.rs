use crate::app::WorkbenchApp;
use eframe::egui;

impl WorkbenchApp {
    /// 串口刷新。
    pub(super) fn tick_port_refresh(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let refresh_interval = if ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
            0.5
        } else {
            2.0
        };
        if now - self.serial.last_port_refresh > refresh_interval {
            self.serial.last_port_refresh = now;
            self.refresh_ports_silent();
        }
    }
}
