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

    /// 只在 transport 进入真实 open 状态后显示打开成功提示。
    pub(super) fn tick_pending_port_open_notice(&mut self, ctx: &egui::Context) {
        let Some(mut pending) = self.serial.pending_open_notice.clone() else {
            return;
        };

        if pending.requested_at == 0.0 {
            pending.requested_at = ctx.input(|i| i.time);
            self.serial.pending_open_notice = Some(pending.clone());
            return;
        }

        let status = self.workbench.transport_status(&pending.port_name);
        if status.open {
            self.serial.pending_open_notice = None;
            self.set_status_force(crate::state::StatusLevel::Info, pending.success_message);
        } else if !status.connecting && ctx.input(|i| i.time) - pending.requested_at >= 3.0 {
            self.serial.pending_open_notice = None;
            self.set_status_force(
                crate::state::StatusLevel::Error,
                format!("{} 打开失败", pending.port_name),
            );
        }
    }
}
