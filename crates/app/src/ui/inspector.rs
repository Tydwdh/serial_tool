use crate::app::WorkbenchApp;
use eframe::egui;
use tool_panels::theme;
use tool_transport::TransportStatus;

impl WorkbenchApp {
    pub(crate) fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("检查器");
        let st = self
            .selected_port
            .as_deref()
            .map(|p| self.transport.status_port(p))
            .unwrap_or_else(TransportStatus::closed);
        ui.label(egui::RichText::new("串口").strong());
        if st.open {
            ui.colored_label(
                theme::GREEN,
                format!(
                    "● {} @ {}",
                    st.port_name.as_deref().unwrap_or("?"),
                    st.baud_rate.unwrap_or(0)
                ),
            );
        } else {
            ui.colored_label(theme::TEXT_SECONDARY, "○ 已关闭");
        }
        ui.separator();
        ui.label(egui::RichText::new("录制").strong());
        ui.label(if self.recorder.is_running() {
            "⏺ 运行中"
        } else {
            "⏹ 已停止"
        });
        if let Some(p) = self.recorder.current_path() {
            ui.monospace(p.display().to_string());
        }
        ui.separator();
        ui.label(egui::RichText::new("运行时").strong());
        ui.label(format!("插件: {}", self.plugin_manager.count()));
        ui.label(format!("动态面板: {}", self.dynamic_panels.count()));
        if let Some(e) = self.dynamic_panels.last_error() {
            ui.colored_label(theme::RED, e);
        }
        ui.separator();
        ui.label(egui::RichText::new("DataBus").strong());
        ui.label(format!(
            "事件 {} | {:.0}/s",
            self.bus.history_len(),
            self.event_rate
        ));
    }
}
