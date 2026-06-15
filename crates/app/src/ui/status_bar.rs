use crate::app::WorkbenchApp;
use eframe::egui;
use tool_panels::theme;
use tool_transport::TransportStatus;

impl WorkbenchApp {
    pub(crate) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let st = self
            .selected_port
            .as_deref()
            .map(|p| self.transport.status_port(p))
            .unwrap_or_else(TransportStatus::closed);
        ui.horizontal(|ui| {
            let (d, l) = if let (Some(p), Some(b)) = (self.selected_port.clone(), st.baud_rate) {
                let label = self.port_label(&p);
                (if st.open { "●" } else { "○" }, format!("{label} @ {b}"))
            } else {
                ("○", "串口已关闭".into())
            };
            ui.label(egui::RichText::new(d).color(if st.open {
                theme::GREEN
            } else {
                theme::TEXT_SECONDARY
            }));
            ui.label(l);
            ui.separator();
            let rec = self.recorder.is_running();
            ui.label(egui::RichText::new("●").color(if rec {
                theme::RED
            } else {
                theme::TEXT_SECONDARY
            }));
            if rec {
                let stats = self.recorder.stats();
                if stats.paused {
                    ui.label(format!("已暂停 {} 条 {:.1}MB", stats.events_written, stats.bytes_written as f64 / 1024.0 / 1024.0));
                } else {
                    ui.label(format!("录制中 {} 条 {:.1}MB", stats.events_written, stats.bytes_written as f64 / 1024.0 / 1024.0));
                }
            } else {
                ui.label("未录制");
            }
            if rec {
                let stats = self.recorder.stats();
                if let Some(ref err) = stats.last_error {
                    ui.colored_label(theme::RED, format!("错误: {err}"));
                }
            }
            ui.separator();
            ui.label(format!("{:.0} 事件/秒", self.event_rate));
            ui.separator();
            let shown = {
                let mut chars = self.status.message.chars();
                let head: String = chars.by_ref().take(80).collect();
                if chars.next().is_some() {
                    format!("{head}…")
                } else {
                    head
                }
            };
            ui.label(&shown).on_hover_text(&self.status.message);
        });
    }
}
