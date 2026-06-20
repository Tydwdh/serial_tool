use crate::app::WorkbenchApp;
use crate::config::{pick_recorder_path, record_mode_label};
use crate::state::StatusLevel;
use crate::ui::baud_combo;
use eframe::egui;
use std::collections::BTreeSet;
use tool_panels::theme;
use tool_recorder::RecordMode;

impl WorkbenchApp {
    pub(crate) fn device_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("设备");
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("波特率");
            baud_combo(ui, "dev-port-rate", 180.0, &mut self.serial.baud_rate);
            ui.label("数据位");
            egui::ComboBox::from_id_salt("dev-db")
                .width(60.0)
                .selected_text(&self.serial.data_bits)
                .show_ui(ui, |ui| {
                    for &v in &["5", "6", "7", "8"] {
                        ui.selectable_value(&mut self.serial.data_bits, v.to_owned(), v);
                    }
                });

            ui.label("停止位");
            egui::ComboBox::from_id_salt("dev-sb")
                .width(60.0)
                .selected_text(&self.serial.stop_bits)
                .show_ui(ui, |ui| {
                    for &v in &["1", "2"] {
                        ui.selectable_value(&mut self.serial.stop_bits, v.to_owned(), v);
                    }
                });

            ui.label("校验");
            egui::ComboBox::from_id_salt("dev-par")
                .width(70.0)
                .selected_text(&self.serial.parity)
                .show_ui(ui, |ui| {
                    for &(v, l) in &[("none", "无"), ("odd", "奇"), ("even", "偶")] {
                        ui.selectable_value(&mut self.serial.parity, v.to_owned(), l);
                    }
                });

            ui.label("超时(ms)");
            ui.add(egui::TextEdit::singleline(&mut self.serial.timeout_ms).desired_width(50.0));
        });

        // 显示已打开但不在系统端口列表中的 stale 连接
        let transport_open = self.transport.open_ports();
        if !transport_open.is_empty() {
            let system_names: BTreeSet<&str> = self
                .serial
                .ports
                .iter()
                .map(|d| d.port_name.as_str())
                .collect();
            let stale: Vec<&String> = transport_open
                .iter()
                .filter(|p| !system_names.contains(p.as_str()))
                .collect();
            if !stale.is_empty() {
                ui.separator();
                ui.colored_label(theme::ORANGE, "⚠ 以下端口已打开但可能已拔出：");
                for port in &stale {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(*port).monospace().color(theme::ORANGE));
                        if ui.small_button("强制关闭").clicked() {
                            self.transport.close_port(port);
                            self.set_status_force(StatusLevel::Info, format!("{port} 已强制关闭"));
                        }
                    });
                }
            }
        }

        ui.separator();

        ui.heading("录制");

        ui.horizontal(|ui| {
            ui.label("路径");

            let recording = self.recorder.is_running();

            ui.add_enabled(
                !recording,
                egui::TextEdit::singleline(&mut self.recorder_path).desired_width(360.0),
            );

            if ui
                .add_enabled(!recording, egui::Button::new("浏览"))
                .on_hover_text(if recording {
                    "录制中不能修改保存路径"
                } else {
                    "选择录制保存路径"
                })
                .clicked()
                && let Some(path) = pick_recorder_path(&self.recorder_path)
            {
                self.recorder_path = path.display().to_string();
            }

            let stopping = self.recorder.is_stopping();
            if ui
                .add_enabled(
                    !stopping,
                    egui::Button::new(if recording { "停止" } else { "录制" }),
                )
                .on_disabled_hover_text("正在停止中...")
                .clicked()
            {
                self.start_or_stop_recording();
            }
            if recording {
                let paused = self.recorder.is_paused();
                if ui
                    .add_enabled(
                        !stopping,
                        egui::Button::new(if paused { "继续" } else { "暂停" }),
                    )
                    .on_disabled_hover_text("正在停止中...")
                    .clicked()
                {
                    if paused {
                        self.recorder.resume();
                    } else {
                        self.recorder.pause();
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            ui.label("模式");
            let recording = self.recorder.is_running();
            let mut mode = self.recorder.mode();
            ui.add_enabled_ui(!recording, |ui| {
                egui::ComboBox::from_id_salt("record-mode")
                    .width(160.0)
                    .selected_text(record_mode_label(mode))
                    .show_ui(ui, |ui| {
                        for &m in &[
                            RecordMode::StandardReplay,
                            RecordMode::RawSerial,
                            RecordMode::FullDebug,
                        ] {
                            ui.selectable_value(&mut mode, m, record_mode_label(m));
                        }
                    });
            });
            self.recorder.set_mode(mode);
        });

        // ── 录制健康状态 ──
        let stats = self.recorder.stats();
        if stats.running || stats.stopping {
            ui.separator();
            ui.horizontal(|ui| {
                if stats.paused {
                    ui.colored_label(theme::YELLOW, "⏸ 已暂停，未写入新事件");
                } else if stats.running {
                    ui.colored_label(theme::GREEN, "● 录制中");
                } else {
                    ui.colored_label(theme::YELLOW, "● 正在停止");
                }

                ui.label(format!("事件 {}", stats.events_written));
                ui.label(format!(
                    "{:.1} MB",
                    stats.bytes_written as f64 / 1024.0 / 1024.0
                ));
                ui.label(format!("flush {} ms 前", stats.last_flush_elapsed_ms));
            });

            if let Some(path) = self.recorder.current_path() {
                ui.label(format!("路径：{}", path.display()));
            }
            if let Some(ref error) = stats.last_error {
                ui.colored_label(theme::RED, format!("录制错误：{error}"));
            }
        }

        ui.separator();

        ui.checkbox(&mut self.serial.auto_reconnect, "串口拔出后自动重连");
        if self.serial.auto_reconnect
            && let Some(ref pending) = self.serial.pending_reconnect
        {
            let now = tool_core::now_timestamp_ms() as f64 / 1000.0;
            let remaining = (pending.next_try_at - now).max(0.0);
            ui.label(format!(
                "等待 {} {:2.1}s 后重试 (第 {}/10 次)",
                pending.port_name,
                remaining,
                pending.attempts + 1
            ));
        }

        ui.separator();

        ui.heading("可用端口");
        ui.label(
            egui::RichText::new("提示：别名会显示在串口选择、发送目标和设备列表中")
                .color(theme::TEXT_SECONDARY),
        );
        let mut alias_changes: Vec<(String, Option<String>)> = Vec::new();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for port in &self.serial.ports {
                let name = port.port_name.clone();
                let mut alias_buf = self
                    .serial
                    .port_aliases
                    .get(&name)
                    .cloned()
                    .unwrap_or_default();

                ui.horizontal(|ui| {
                    let open = self.transport.status_port(&name).open;
                    ui.label(if open { "●" } else { "○" });
                    ui.monospace(&name);
                    ui.label(port.port_type.to_string());

                    ui.label("别名");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut alias_buf)
                            .desired_width(140.0)
                            .hint_text("例如 主控板 / GPS"),
                    );

                    if resp.changed() {
                        let new_alias = if alias_buf.trim().is_empty() {
                            None
                        } else {
                            Some(alias_buf.trim().to_owned())
                        };
                        alias_changes.push((name.clone(), new_alias));
                    }

                    if self.serial.port_aliases.contains_key(&name)
                        && ui.small_button("清除").clicked()
                    {
                        alias_changes.push((name.clone(), None));
                    }
                });
            }
        });
        let has_changes = !alias_changes.is_empty();
        for (name, new_alias) in alias_changes {
            match new_alias {
                Some(alias) => {
                    self.serial.port_aliases.insert(name, alias);
                }
                None => {
                    self.serial.port_aliases.remove(&name);
                }
            }
        }
        if has_changes && let Err(e) = self.save_config() {
            log::warn!("save_config failed: {e}")
        };
    }
}
