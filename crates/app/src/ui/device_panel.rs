use crate::app::StatusLevel;
use crate::app::WorkbenchApp;
use crate::app::{pick_recorder_path, record_mode_label};
use crate::ui::top_bar::{
    baud_combo, serial_action_button, serial_action_button_enabled, serial_combo,
};
use eframe::egui;
use std::collections::BTreeSet;
use tool_panels::theme;
use tool_recorder::RecordMode;

impl WorkbenchApp {
    pub(crate) fn device_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("设备");

        ui.horizontal(|ui| {
            self.serial_connect_controls(ui, "dev-port", "dev-baud", 180.0, 90.0, false);
        });

        ui.horizontal(|ui| {
            ui.label("数据位");
            egui::ComboBox::from_id_salt("dev-db")
                .width(60.0)
                .selected_text(&self.data_bits)
                .show_ui(ui, |ui| {
                    for &v in &["5", "6", "7", "8"] {
                        ui.selectable_value(&mut self.data_bits, v.to_owned(), v);
                    }
                });

            ui.label("停止位");
            egui::ComboBox::from_id_salt("dev-sb")
                .width(60.0)
                .selected_text(&self.stop_bits)
                .show_ui(ui, |ui| {
                    for &v in &["1", "2"] {
                        ui.selectable_value(&mut self.stop_bits, v.to_owned(), v);
                    }
                });

            ui.label("校验");
            egui::ComboBox::from_id_salt("dev-par")
                .width(70.0)
                .selected_text(&self.parity)
                .show_ui(ui, |ui| {
                    for &(v, l) in &[("none", "无"), ("odd", "奇"), ("even", "偶")] {
                        ui.selectable_value(&mut self.parity, v.to_owned(), l);
                    }
                });

            ui.label("超时(ms)");
            ui.add(egui::TextEdit::singleline(&mut self.timeout_ms).desired_width(50.0));
        });

        // 显示已打开但不在系统端口列表中的 stale 连接
        let transport_open = self.transport.open_ports();
        if !transport_open.is_empty() {
            let system_names: BTreeSet<&str> =
                self.ports.iter().map(|d| d.port_name.as_str()).collect();
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
            {
                if let Some(path) = pick_recorder_path(&self.recorder_path) {
                    self.recorder_path = path.display().to_string();
                }
            }

            if ui.button(if recording { "停止" } else { "录制" }).clicked() {
                self.start_or_stop_recording();
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

        ui.separator();

        ui.heading("可用端口");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for port in &self.ports {
                ui.monospace(format!("{} {}", port.port_name, port.port_type));
            }
        });
    }
}
