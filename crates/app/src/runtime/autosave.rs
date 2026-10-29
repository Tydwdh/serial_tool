use crate::app::WorkbenchApp;
use eframe::egui;

impl WorkbenchApp {
    /// 自动保存工作区（每60秒）。
    pub(super) fn tick_auto_save(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        if now - self.last_auto_save_time > 60.0 {
            self.last_auto_save_time = now;
            if let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            }
        }
    }
}
