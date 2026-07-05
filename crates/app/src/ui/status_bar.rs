use crate::app::WorkbenchApp;
use crate::state::StatusLevel;
use eframe::egui;
use egui::Color32;
use tool_panels::theme;
use tool_transport::TransportStatus;

impl WorkbenchApp {
    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
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
                    (
                        if st.open {
                            theme::GREEN
                        } else {
                            theme::TEXT_SECONDARY
                        },
                        format!("{label} @ {b}"),
                    )
                } else {
                    (theme::TEXT_SECONDARY, "串口已关闭".into())
                };
            // 发光圆点：外层半透明大圆 + 内层实心小圆
            let dot_rect = egui::Rect::from_center_size(
                egui::pos2(ui.cursor().left() + 6.0, ui.cursor().center().y),
                egui::vec2(10.0, 10.0),
            );
            if st.open {
                ui.painter()
                    .circle_filled(dot_rect.center(), 5.0, dot_color.linear_multiply(0.3));
            }
            ui.painter()
                .circle_filled(dot_rect.center(), 3.0, dot_color);
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
                ui.painter().circle_filled(
                    rec_dot_rect.center(),
                    5.0,
                    theme::RED.linear_multiply(0.3),
                );
            }
            ui.painter().circle_filled(
                rec_dot_rect.center(),
                3.0,
                if rec {
                    theme::RED
                } else {
                    theme::TEXT_SECONDARY
                },
            );
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

            // DTR/RTS 标签：可点击切换，复用发送面板的信号控制路径。
            // 注意：dtr_high/rts_high 是 send.target_port 的状态；状态栏显示 selected_port，
            // 通常两者一致（ensure_send_target_port 会回退到 selected_port）。
            if st.open {
                ui.separator();
                let port = self.send.target_port.clone();
                let target_open = self.send_target_port_open();
                let tag = |ui: &mut egui::Ui, label: &str, high: bool, color: Color32, tooltip: &str| -> bool {
                    let size = egui::vec2(42.0, 16.0);
                    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                    let bg = if high {
                        color.linear_multiply(0.25)
                    } else {
                        theme::BG_INPUT
                    };
                    ui.painter().rect_filled(rect, 3.0, bg);
                    if resp.hovered() {
                        ui.painter().rect_stroke(
                            rect,
                            3.0,
                            egui::Stroke::new(1.0, color),
                            egui::StrokeKind::Inside,
                        );
                    }
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(10.0),
                        color,
                    );
                    resp.on_hover_text(tooltip).clicked()
                };
                // 仅当 target_port 打开时才允许点击切换；否则仅展示。
                if target_open && port.is_some() {
                    let port = port.clone().expect("checked Some above");
                    if tag(
                        ui,
                        "DTR⬆",
                        self.send.dtr_high,
                        theme::GREEN,
                        "数据终端就绪 (DTR)。点击切换会立即驱动该线路，部分设备会用它触发复位/进入 bootload，请谨慎。",
                    ) {
                        let new_dtr = !self.send.dtr_high;
                        match self.transport.set_dtr(&port, new_dtr) {
                            Ok(()) => self.send.dtr_high = new_dtr,
                            Err(e) => self.set_status_force(StatusLevel::Error, e.to_string()),
                        }
                    }
                    if tag(
                        ui,
                        "RTS⬆",
                        self.send.rts_high,
                        theme::BLUE,
                        "请求发送 (RTS)。点击切换会立即驱动该线路，部分设备会用它触发复位/进入 bootload，请谨慎。",
                    ) {
                        let new_rts = !self.send.rts_high;
                        match self.transport.set_rts(&port, new_rts) {
                            Ok(()) => self.send.rts_high = new_rts,
                            Err(e) => self.set_status_force(StatusLevel::Error, e.to_string()),
                        }
                    }
                } else {
                    tag(
                        ui,
                        "DTR⬆",
                        self.send.dtr_high,
                        theme::GREEN,
                        "数据终端就绪 (DTR)。打开串口后可切换电平。",
                    );
                    tag(
                        ui,
                        "RTS⬆",
                        self.send.rts_high,
                        theme::BLUE,
                        "请求发送 (RTS)。打开串口后可切换电平。",
                    );
                }
            }

            // ── 插件贡献：status_bar.left ──
            self.ui_contribution_slot(ui, "status_bar.left");

            // ── 通知队列（多来源独立，互不覆盖） ──
            // 截断提醒也走通知队列
            if self.terminal_panel.truncated {
                self.notifications.push(
                    "terminal",
                    StatusLevel::Warn,
                    format!(
                        "终端已截断，仅保留最近 {} 条",
                        self.terminal_panel.max_entries
                    ),
                );
                self.terminal_panel.truncated = false;
            }
            if self.bottom_log_panel.truncated {
                self.notifications.push(
                    "log",
                    StatusLevel::Warn,
                    format!(
                        "日志已截断，仅保留最近 {} 条",
                        self.bottom_log_panel.max_entries
                    ),
                );
                self.bottom_log_panel.truncated = false;
            }
            // 重新获取（包含刚推送的截断通知）
            let notifications = self.notifications.current();

            if !notifications.is_empty() {
                ui.separator();
                // 最多显示 3 条，超出显示可点击的 "…及 N 条消息" 弹出全部。
                let max_show = 3;
                let total = notifications.len();
                let shown: Vec<_> = notifications.iter().take(max_show).collect();
                for n in &shown {
                    let color = match n.level {
                        StatusLevel::Info => theme::TEXT_SECONDARY,
                        StatusLevel::Warn => theme::YELLOW,
                        StatusLevel::Error => theme::RED,
                    };
                    let text = truncate_for_status(&n.text, 60);
                    ui.label(egui::RichText::new(&text).color(color))
                        .on_hover_text(&n.text);
                }
                if total > max_show {
                    let overflow_id = ui.id().with("notification_overflow");
                    let overflow_text =
                        egui::RichText::new(format!("…及 {} 条消息", total - max_show))
                            .small()
                            .color(theme::TEXT_SECONDARY);
                    let overflow_resp =
                        ui.selectable_label(false, overflow_text);
                    let mut popup = egui::Popup::from_response(&overflow_resp)
                        .id(overflow_id)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
                    if overflow_resp.clicked() {
                        popup = popup.open_memory(Some(egui::SetOpenCommand::Toggle));
                    }
                    popup.show(|ui| {
                        ui.set_min_width(320.0);
                        ui.set_max_height(200.0);
                        ui.style_mut().spacing.item_spacing.y = 2.0;
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for n in &notifications {
                                let color = match n.level {
                                    StatusLevel::Info => theme::TEXT_SECONDARY,
                                    StatusLevel::Warn => theme::YELLOW,
                                    StatusLevel::Error => theme::RED,
                                };
                                let level_mark = match n.level {
                                    StatusLevel::Error => "✕ ",
                                    StatusLevel::Warn => "⚠ ",
                                    StatusLevel::Info => "",
                                };
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{level_mark}{}",
                                        &n.text
                                    ))
                                    .color(color)
                                    .small(),
                                );
                            }
                        });
                    });
                }
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

/// 状态栏消息截断：保留前 max_chars 个字符，超出加 …。
fn truncate_for_status(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}
