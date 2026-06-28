use crate::app::WorkbenchApp;
use crate::state::StatusLevel;
use eframe::egui;
use egui::Color32;
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
            // 串口状态 — 带发光效果的圆点
            let (dot_color, label) =
                if let (Some(p), Some(b)) = (self.serial.selected_port.clone(), st.baud_rate) {
                    let label = self.serial.port_label(&p);
                    (if st.open { theme::GREEN } else { theme::TEXT_SECONDARY }, format!("{label} @ {b}"))
                } else {
                    (theme::TEXT_SECONDARY, "串口已关闭".into())
                };
            // 发光圆点：外层半透明大圆 + 内层实心小圆
            let dot_rect = egui::Rect::from_center_size(
                egui::pos2(ui.cursor().left() + 6.0, ui.cursor().center().y),
                egui::vec2(10.0, 10.0),
            );
            if st.open {
                ui.painter().circle_filled(dot_rect.center(), 5.0, dot_color.linear_multiply(0.3));
            }
            ui.painter().circle_filled(dot_rect.center(), 3.0, dot_color);
            ui.add_space(12.0);
            ui.label(label);
            ui.separator();

            // 录制状态 — 发光圆点
            let rec = self.recorder.is_running();
            let rec_dot_rect = egui::Rect::from_center_size(
                egui::pos2(ui.cursor().left() + 6.0, ui.cursor().center().y),
                egui::vec2(10.0, 10.0),
            );
            if rec {
                ui.painter().circle_filled(rec_dot_rect.center(), 5.0, theme::RED.linear_multiply(0.3));
            }
            ui.painter().circle_filled(rec_dot_rect.center(), 3.0, if rec { theme::RED } else { theme::TEXT_SECONDARY });
            ui.add_space(12.0);
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

            // DTR/RTS 标签
            if st.open {
                ui.separator();
                let tag = |ui: &mut egui::Ui, label: &str, high: bool, color: Color32| {
                    let size = egui::vec2(42.0, 16.0);
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    let bg = if high { color.linear_multiply(0.25) } else { theme::BG_INPUT };
                    ui.painter().rect_filled(rect, 3.0, bg);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(10.0),
                        color,
                    );
                };
                tag(ui, "DTR⬆", self.send.dtr_high, theme::GREEN);
                tag(ui, "RTS⬆", self.send.rts_high, theme::BLUE);
            }

            // ── 插件贡献：status_bar.left ──
            self.ui_contribution_slot(ui, "status_bar.left");

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

            // ── 插件贡献：status_bar.right ──
            self.ui_contribution_slot(ui, "status_bar.right");

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
            if let Some(ref err) = us.error {
                ui.label(egui::RichText::new("⚠ 更新失败").color(theme::YELLOW))
                    .on_hover_text(err);
            }
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
            } else if ui.button("下载更新").clicked() {
                self.start_update_download();
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
