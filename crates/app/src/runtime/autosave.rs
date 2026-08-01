use crate::app::WorkbenchApp;
use crate::state::StatusLevel;
use eframe::egui;

impl WorkbenchApp {
    /// 自动保存工作区（每60秒）。
    pub(super) fn tick_auto_save(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if now - self.last_auto_save_time > 60.0 {
            self.last_auto_save_time = now;
            if let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            };
        }
    }

    /// 录制状态检测：收割已停止的录制、worker 线程错误反馈。
    pub(super) fn tick_recorder_status(&mut self) {
        match self.recorder.reap_stopping() {
            Some(Ok(path)) => {
                self.set_status_force(StatusLevel::Info, format!("录制已保存: {}", path.display()))
            }
            Some(Err(e)) => self.set_status_force(StatusLevel::Error, format!("录制失败: {e}")),
            None => {}
        }
        if let Some(error) = self.recorder.reap_error() {
            self.set_status(StatusLevel::Error, format!("录制失败：{error}"));
        }
    }
}
