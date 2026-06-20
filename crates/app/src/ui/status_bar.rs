use crate::app::WorkbenchApp;
use crate::state::StatusLevel;
use eframe::egui;
use tool_panels::theme;
use tool_transport::TransportStatus;

impl WorkbenchApp {
    pub(crate) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let st = self
            .serial
            .selected_port
            .as_deref()
            .map(|p| self.transport.status_port(p))
            .unwrap_or_else(TransportStatus::closed);
        ui.horizontal(|ui| {
            // 状态消息：优先显示在最左边，确保可见
            let status_color = match self.status.level {
                StatusLevel::Info => theme::TEXT_SECONDARY,
                StatusLevel::Warn => theme::YELLOW,
                StatusLevel::Error => theme::RED,
            };
            let shown = {
                let mut chars = self.status.message.chars();
                let head: String = chars.by_ref().take(80).collect();
                if chars.next().is_some() {
                    format!("{head}…")
                } else {
                    head
                }
            };
            ui.label(egui::RichText::new(&shown).color(status_color))
                .on_hover_text(&self.status.message);

            ui.separator();

            // 串口状态
            let (d, l) =
                if let (Some(p), Some(b)) = (self.serial.selected_port.clone(), st.baud_rate) {
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

            // 录制状态
            let rec = self.recorder.is_running();
            ui.label(egui::RichText::new("●").color(if rec {
                theme::RED
            } else {
                theme::TEXT_SECONDARY
            }));
            if rec {
                let stats = self.recorder.stats();
                if stats.paused {
                    ui.label(format!(
                        "已暂停 {} 条 {:.1}MB",
                        stats.events_written,
                        stats.bytes_written as f64 / 1024.0 / 1024.0
                    ));
                } else {
                    ui.label(format!(
                        "录制中 {} 条 {:.1}MB",
                        stats.events_written,
                        stats.bytes_written as f64 / 1024.0 / 1024.0
                    ));
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

            // DTR/RTS 状态
            if st.open {
                ui.separator();
                let dtr = if self.send.dtr_high {
                    "DTR⬆"
                } else {
                    "DTR⬇"
                };
                let rts = if self.send.rts_high {
                    "RTS⬆"
                } else {
                    "RTS⬇"
                };
                ui.label(format!("{dtr} {rts}"));
            }
        });
    }
}
