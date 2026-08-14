use crate::app::WorkbenchApp;
use crate::state::StatusLevel;
use eframe::egui;
use egui::Color32;
use egui_material_icons::icons::{
    ICON_CHECK_CIRCLE, ICON_CLOUD_DOWNLOAD, ICON_ERROR, ICON_NOTIFICATIONS, ICON_REFRESH,
    ICON_SYSTEM_UPDATE, ICON_WARNING,
};
use tool_panels::{design, theme};
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
            // 串口状态
            let (dot_color, label) =
                if let (Some(p), Some(b)) = (self.serial.selected_port.clone(), st.baud_rate) {
                    let label = self.serial.port_label(&p);
                    if st.connecting {
                        (
                            theme::yellow(),
                            format!("{label} 连接中"),
                        )
                    } else {
                        // 网络模拟串口：波特率字段复用作服务器端口，不显示数字波特率。
                        let is_network = self
                            .serial
                            .network_ports
                            .iter()
                            .any(|n| n.display_name() == p);
                        let suffix = if is_network { "网络".to_owned() } else { format!("{b}") };
                        (
                            if st.open {
                                theme::green()
                            } else {
                                theme::text_secondary()
                            },
                            format!("{label} @ {suffix}"),
                        )
                    }
                } else {
                    (theme::text_secondary(), "串口已关闭".into())
                };
            design::status_pill(ui, dot_color, label);

            // 录制状态
            let rec = self.recorder.is_running();
            let recording_label = if rec {
                let stats = self.recorder.stats();
                if stats.paused {
                    format!(
                        "已暂停 {} 条 {:.1}MB",
                        stats.events_written,
                        stats.bytes_written as f64 / 1024.0 / 1024.0
                    )
                } else {
                    format!(
                        "录制中 {} 条 {:.1}MB",
                        stats.events_written,
                        stats.bytes_written as f64 / 1024.0 / 1024.0
                    )
                }
            } else {
                "未录制".to_owned()
            };
            design::status_pill(
                ui,
                if rec {
                    theme::red()
                } else {
                    theme::text_dimmed()
                },
                recording_label,
            );
            if rec {
                let stats = self.recorder.stats();
                if let Some(ref err) = stats.last_error {
                    ui.colored_label(theme::red(), format!("错误: {err}"));
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
                    let size = egui::vec2(46.0, 20.0); // 最小触控目标约 24px，此处为视觉标签适度增大
                    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                    let bg = if high {
                        color.linear_multiply(0.25)
                    } else {
                        theme::bg_input()
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
                        theme::green(),
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
                        theme::blue(),
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
                        theme::green(),
                        "数据终端就绪 (DTR)。打开串口后可切换电平。",
                    );
                    tag(
                        ui,
                        "RTS⬆",
                        self.send.rts_high,
                        theme::blue(),
                        "请求发送 (RTS)。打开串口后可切换电平。",
                    );
                }
            }

            // ── 插件贡献：status_bar.left ──
            self.ui_contribution_slot(ui, "status_bar.left");

            // ── 通知队列（多来源独立，互不覆盖） ──
            // 截断提醒也走通知队列
            let terminal_dropped = self.terminal_panel.take_dropped_events();
            if terminal_dropped > 0 {
                self.notifications.push(
                    "terminal-data-loss",
                    StatusLevel::Error,
                    format!("接收区缓冲已满，丢失 {terminal_dropped} 条最旧事件"),
                );
            }
            let log_dropped = self.bottom_log_panel.take_dropped_events();
            if log_dropped > 0 {
                self.notifications.push(
                    "log-data-loss",
                    StatusLevel::Warn,
                    format!("日志缓冲已满，丢失 {log_dropped} 条最旧事件"),
                );
            }
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
                // 状态栏只保留一条摘要，其余消息放入通知列表，避免挤压串口状态。
                let max_show = 1;
                let total = notifications.len();
                let shown: Vec<_> = notifications.iter().take(max_show).collect();
                for n in &shown {
                    let color = match n.level {
                        StatusLevel::Info => theme::text_secondary(),
                        StatusLevel::Warn => theme::yellow(),
                        StatusLevel::Error => theme::red(),
                    };
                    let text = truncate_for_status(&n.text, 60);
                    ui.label(egui::RichText::new(&text).color(color))
                        .on_hover_text(&n.text);
                }
                if total > max_show {
                    let overflow_id = ui.id().with("notification_overflow");
                    let overflow_text = egui::RichText::new(format!(
                        "{} {} 条",
                        ICON_NOTIFICATIONS.codepoint,
                        total - max_show
                    ))
                            .small()
                            .color(theme::text_secondary());
                    let overflow_resp =
                        ui.selectable_label(false, overflow_text);
                    let mut overflow_open = ui.ctx().memory_mut(|m| {
                        m.data
                            .get_persisted::<bool>(overflow_id)
                            .unwrap_or(false)
                    });
                    if overflow_resp.clicked() {
                        overflow_open = !overflow_open;
                        ui.ctx().memory_mut(|m| m.data.insert_persisted(overflow_id, overflow_open));
                    }
                    if overflow_open {
                        egui::Window::new("通知列表")
                            .id(overflow_id)
                            .collapsible(false)
                            .resizable(false)
                            .anchor(egui::Align2::CENTER_CENTER, [0.0, 100.0])
                            .auto_sized()
                            .show(ui.ctx(), |ui| {
                                ui.set_min_width(320.0);
                                ui.set_max_height(300.0);
                                ui.spacing_mut().item_spacing.y = 2.0;
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    for n in &notifications {
                                        let color = match n.level {
                                            StatusLevel::Info => theme::text_secondary(),
                                            StatusLevel::Warn => theme::yellow(),
                                            StatusLevel::Error => theme::red(),
                                        };
                                        let level_mark = match n.level {
                                            StatusLevel::Error => ICON_ERROR.codepoint,
                                            StatusLevel::Warn => ICON_WARNING.codepoint,
                                            StatusLevel::Info => "",
                                        };
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{level_mark} {}",
                                                n.text
                                            ))
                                            .color(color)
                                            .small(),
                                        );
                                    }
                                });
                            });
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            ui.ctx().memory_mut(|m| m.data.insert_persisted(overflow_id, false));
                        }
                    }
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

    /// 更新图标与下载进度/按钮。
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
                ui.label(
                    egui::RichText::new(format!("{} 更新失败", ICON_WARNING.codepoint))
                        .color(theme::yellow()),
                )
                .on_hover_text(err);
            }
            let label = ui.label(
                egui::RichText::new(format!(
                    "{} v{version_str} 可用",
                    ICON_SYSTEM_UPDATE.codepoint
                ))
                .color(theme::cyan()),
            );

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
                if design::button(ui, ICON_REFRESH, "更新并重启", design::ButtonKind::Primary)
                    .clicked()
                {
                    self.update_state.want_restart = true;
                }
            } else if design::button(
                ui,
                ICON_CLOUD_DOWNLOAD,
                "下载更新",
                design::ButtonKind::Secondary,
            )
            .clicked()
            {
                self.start_update_download();
            }
            return;
        }

        // 错误时显示警告
        if let Some(ref err) = us.error {
            ui.label(design::icon_only(ICON_WARNING, theme::yellow(), 17.0))
                .on_hover_text(err);
        }

        // 图标：未检查=刷新，已检查无更新=完成。
        let (icon, color, hover) = if us.latest_version.is_some() && us.error.is_none() {
            (
                ICON_CHECK_CIRCLE,
                theme::green(),
                "已是最新版本，点击重新检查",
            )
        } else {
            (ICON_REFRESH, theme::text_secondary(), "检查更新")
        };
        if ui
            .add(egui::Label::new(design::icon_only(icon, color, 17.0)).sense(egui::Sense::click()))
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
