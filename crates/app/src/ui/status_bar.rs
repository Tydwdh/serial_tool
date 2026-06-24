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

            // ── 状态消息（左对齐） ──
            if !self.status.message.is_empty() {
                ui.separator();
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
            }

            // ── 更新图标（最右边） ──
            self.draw_update_status(ui);
        });
    }

    /// 更新图标（靠右对齐）。
    fn draw_update_status(&mut self, ui: &mut egui::Ui) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            self.draw_update_icon(ui);
        });
    }

    /// 更新图标：🔄 / ✓ / ⚠ + 下载进度/按钮。
    fn draw_update_icon(&mut self, ui: &mut egui::Ui) {
        let us = &self.update_state;

        // 正在检查
        if us.checking {
            ui.spinner();
            ui.label("检查更新...");
            return;
        }

        // 有新版本可用
        if us.update_available {
            let version_str = us.latest_version.as_deref().unwrap_or("?");
            let label =
                ui.label(egui::RichText::new(format!("🔄 v{version_str} 可用")).color(theme::CYAN));

            // hover 显示 changelog
            if !us.changelog.is_empty() {
                let changelog_text = us
                    .changelog
                    .iter()
                    .map(|c| format!("• {c}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                label.on_hover_text(format!("v{version_str} 更新内容：\n{changelog_text}"));
            }

            // 正在下载
            if us.downloading {
                let pct = us.download_progress * 100.0;
                ui.spinner();
                ui.label(format!("下载中 {pct:.0}%"));
            } else if us.downloaded {
                if ui
                    .button(egui::RichText::new("更新并重启").color(theme::GREEN))
                    .clicked()
                {
                    self.update_state.want_restart = true;
                }
            } else {
                if ui.button("下载更新").clicked() {
                    self.start_update_download();
                }
            }
            return;
        }

        // 错误时显示 ⚠
        if let Some(ref err) = us.error {
            ui.label(egui::RichText::new("⚠").color(theme::YELLOW))
                .on_hover_text(err);
        }

        // 图标：未检查=🔄，已检查无更新=✓
        let (icon, color, hover) = if us.latest_version.is_some() && us.error.is_none() {
            ("✓", theme::GREEN, "已是最新版本，点击重新检查")
        } else {
            ("🔄", theme::TEXT_SECONDARY, "检查更新")
        };
        if ui
            .add(
                egui::Label::new(egui::RichText::new(icon).color(color))
                    .sense(egui::Sense::click()),
            )
            .on_hover_text(hover)
            .clicked()
        {
            self.force_check_update();
        }
    }
}
