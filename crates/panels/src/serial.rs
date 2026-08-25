//! Shared serial UI primitives used by the Native and Web composition roots.
//!
//! The panel deliberately knows nothing about `Workbench`, `WebApplication`,
//! or a concrete transport. It renders a [`SerialView`] and returns
//! [`SerialAction`] values for the composition root to dispatch.

use egui::Ui;
use tool_platform::{SerialParity, SerialSettings, TransportCapabilities};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialPortItem {
    pub id: String,
    pub label: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerialAction {
    Refresh,
    RequestPort,
    Connect {
        port: String,
        settings: SerialSettings,
    },
    Disconnect {
        port: String,
    },
    SendText {
        port: String,
        text: String,
    },
    SendHex {
        port: String,
        hex: String,
    },
    SetDtr {
        port: String,
        value: bool,
    },
    SetRts {
        port: String,
        value: bool,
    },
}

/// Mutable UI state plus read-only transport data for one frame.
pub struct SerialView<'a> {
    pub ports: &'a [SerialPortItem],
    pub connected: Option<&'a str>,
    pub status: &'a str,
    pub settings: &'a mut SerialSettings,
    pub send_input: &'a mut String,
    pub tx_hex: &'a mut bool,
    pub dtr: &'a mut bool,
    pub rts: &'a mut bool,
    pub capabilities: TransportCapabilities,
    /// Native keeps its richer grouped-port editor beside this shared core.
    pub show_ports: bool,
    /// The Native sender has a dedicated panel; Web keeps TX in Serial.
    pub show_sender: bool,
}

pub struct SerialPanel;

impl SerialPanel {
    pub fn ui(ui: &mut Ui, view: &mut SerialView<'_>) -> Vec<SerialAction> {
        let mut actions = Vec::new();
        ui.heading("串口");
        ui.separator();
        Self::settings_ui(ui, view.settings);

        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    view.capabilities.list_known_ports,
                    egui::Button::new("刷新已授权设备"),
                )
                .clicked()
            {
                actions.push(SerialAction::Refresh);
            }
            if ui
                .add_enabled(
                    view.capabilities.request_port,
                    egui::Button::new("添加设备"),
                )
                .clicked()
            {
                actions.push(SerialAction::RequestPort);
            }
        });
        ui.label(view.status);

        if view.show_ports {
            for port in view.ports {
                ui.horizontal_wrapped(|ui| {
                    let label = if port.kind.is_empty() {
                        port.label.clone()
                    } else {
                        format!("{} · {}", port.label, port.kind)
                    };
                    ui.label(label);
                    ui.monospace(&port.id);
                    if view.connected == Some(port.id.as_str()) {
                        if ui
                            .add_enabled(view.capabilities.disconnect, egui::Button::new("断开"))
                            .clicked()
                        {
                            actions.push(SerialAction::Disconnect {
                                port: port.id.clone(),
                            });
                        }
                    } else if ui
                        .add_enabled(view.capabilities.connect, egui::Button::new("连接"))
                        .clicked()
                    {
                        actions.push(SerialAction::Connect {
                            port: port.id.clone(),
                            settings: *view.settings,
                        });
                    }
                });
            }
        }

        if view.show_sender {
            Self::sender_ui(ui, view, &mut actions);
        }

        if let Some(port) = view.connected {
            ui.horizontal_wrapped(|ui| {
                let dtr_changed = ui.checkbox(view.dtr, "DTR").changed();
                if dtr_changed && view.capabilities.set_dtr {
                    actions.push(SerialAction::SetDtr {
                        port: port.to_owned(),
                        value: *view.dtr,
                    });
                }
                let rts_changed = ui.checkbox(view.rts, "RTS").changed();
                if rts_changed && view.capabilities.set_rts {
                    actions.push(SerialAction::SetRts {
                        port: port.to_owned(),
                        value: *view.rts,
                    });
                }
            });
        }

        actions
    }

    pub fn settings_ui(ui: &mut Ui, settings: &mut SerialSettings) {
        ui.horizontal_wrapped(|ui| {
            ui.label("串口参数");
            egui::ComboBox::from_id_salt("shared-serial-baud-rate")
                .selected_text(settings.baud_rate.to_string())
                .show_ui(ui, |ui| {
                    for baud_rate in [
                        9_600, 19_200, 38_400, 57_600, 115_200, 230_400, 460_800, 921_600,
                        1_000_000, 2_000_000, 3_000_000,
                    ] {
                        ui.selectable_value(
                            &mut settings.baud_rate,
                            baud_rate,
                            baud_rate.to_string(),
                        );
                    }
                });
            egui::ComboBox::from_id_salt("shared-serial-data-bits")
                .selected_text(settings.data_bits.to_string())
                .show_ui(ui, |ui| {
                    for data_bits in [5, 6, 7, 8] {
                        ui.selectable_value(
                            &mut settings.data_bits,
                            data_bits,
                            data_bits.to_string(),
                        );
                    }
                });
            egui::ComboBox::from_id_salt("shared-serial-stop-bits")
                .selected_text(settings.stop_bits.to_string())
                .show_ui(ui, |ui| {
                    for stop_bits in [1, 2] {
                        ui.selectable_value(
                            &mut settings.stop_bits,
                            stop_bits,
                            stop_bits.to_string(),
                        );
                    }
                });
            egui::ComboBox::from_id_salt("shared-serial-parity")
                .selected_text(parity_label(settings.parity))
                .show_ui(ui, |ui| {
                    for (parity, label) in [
                        (SerialParity::None, "无"),
                        (SerialParity::Odd, "奇"),
                        (SerialParity::Even, "偶"),
                    ] {
                        ui.selectable_value(&mut settings.parity, parity, label);
                    }
                });
        });
    }

    fn sender_ui(ui: &mut Ui, view: &mut SerialView<'_>, actions: &mut Vec<SerialAction>) {
        let Some(port) = view.connected else {
            return;
        };
        ui.horizontal(|ui| {
            ui.selectable_value(view.tx_hex, false, "TEXT");
            ui.selectable_value(view.tx_hex, true, "HEX");
            ui.text_edit_singleline(view.send_input);
            if ui
                .add_enabled(view.capabilities.send, egui::Button::new("发送"))
                .clicked()
            {
                if *view.tx_hex {
                    actions.push(SerialAction::SendHex {
                        port: port.to_owned(),
                        hex: view.send_input.clone(),
                    });
                } else {
                    actions.push(SerialAction::SendText {
                        port: port.to_owned(),
                        text: view.send_input.clone(),
                    });
                }
            }
        });
    }
}

fn parity_label(parity: SerialParity) -> &'static str {
    match parity {
        SerialParity::None => "无",
        SerialParity::Odd => "奇",
        SerialParity::Even => "偶",
    }
}
