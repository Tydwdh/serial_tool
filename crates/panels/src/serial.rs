//! Shared serial UI primitives used by the Native and Web composition roots.
//!
//! The panel deliberately knows nothing about `Workbench`, `WebApplication`,
//! or a concrete transport. It renders a [`SerialView`] and returns
//! [`SerialAction`] values for the composition root to dispatch.

use egui::Ui;
use std::collections::BTreeMap;
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
    pub connecting: Option<&'a str>,
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
    /// Optional shared alias/group editor. Native can keep its richer device
    /// management surface, while Web uses this same row/group presentation
    /// instead of falling back to a flat port list.
    pub metadata: Option<SerialPortMetadata<'a>>,
}

pub struct SerialPortMetadata<'a> {
    pub aliases: &'a mut BTreeMap<String, String>,
    pub groups: &'a mut BTreeMap<String, String>,
}

pub struct SerialPanel;

impl SerialPanel {
    pub fn ui(ui: &mut Ui, view: &mut SerialView<'_>) -> Vec<SerialAction> {
        ui.heading("串口");
        ui.separator();
        Self::settings_ui(ui, view.settings);

        let mut actions = Self::port_list_ui(ui, view);
        if view.show_sender {
            Self::sender_ui(ui, view, &mut actions);
        }
        Self::signal_ui(ui, view, &mut actions);
        actions
    }

    /// Render the shared refresh/status/port-list section.
    ///
    /// Native places this section after its recording card while Web places
    /// it after the same card. Keeping it independently callable prevents a
    /// platform composition root from changing the visual order of the
    /// shared device panel.
    pub fn port_list_ui(ui: &mut Ui, view: &mut SerialView<'_>) -> Vec<SerialAction> {
        if view.show_ports && view.metadata.is_some() {
            let mut actions = Vec::new();
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
            actions.extend(Self::grouped_port_list_ui(ui, view));
            return actions;
        }
        let mut actions = Vec::new();

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
                let connecting = view.connecting == Some(port.id.as_str());
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
                        .add_enabled(
                            !connecting && view.capabilities.connect,
                            egui::Button::new(if connecting { "连接中" } else { "连接" }),
                        )
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

        actions
    }

    fn grouped_port_list_ui(ui: &mut Ui, view: &mut SerialView<'_>) -> Vec<SerialAction> {
        let metadata = view
            .metadata
            .take()
            .expect("metadata was checked before rendering grouped ports");
        let mut actions = Vec::new();
        let mut groups: BTreeMap<String, Vec<&SerialPortItem>> = BTreeMap::new();
        for port in view.ports {
            let group = metadata
                .groups
                .get(&port.id)
                .cloned()
                .unwrap_or_else(|| "未分组".to_owned());
            groups.entry(group).or_default().push(port);
        }
        let group_names: Vec<String> = groups
            .keys()
            .filter(|name| name.as_str() != "未分组")
            .cloned()
            .collect();

        for (group_name, ports) in groups {
            let group_id = ui.make_persistent_id(("shared-port-group", &group_name));
            let mut open = ui
                .ctx()
                .data_mut(|data| data.get_persisted::<bool>(group_id).unwrap_or(true));
            ui.horizontal_wrapped(|ui| {
                if ui
                    .selectable_label(false, if open { "▾" } else { "▸" })
                    .clicked()
                {
                    open = !open;
                }
                ui.label(
                    egui::RichText::new(&group_name).color(if group_name == "未分组" {
                        crate::theme::text_secondary()
                    } else {
                        crate::theme::text_primary()
                    }),
                );
                ui.label(
                    egui::RichText::new(format!("({})", ports.len()))
                        .color(crate::theme::text_dimmed()),
                );
            });
            ui.ctx()
                .data_mut(|data| data.insert_persisted(group_id, open));

            if open {
                for port in ports {
                    let mut alias = metadata.aliases.get(&port.id).cloned().unwrap_or_default();
                    let mut selected_group = group_name.clone();
                    let connected = view.connected == Some(port.id.as_str());
                    let connecting = view.connecting == Some(port.id.as_str());
                    ui.horizontal_wrapped(|ui| {
                        let status_label = if connected {
                            "●开"
                        } else if connecting {
                            "◌中"
                        } else {
                            "○关"
                        };
                        let status_color = if connected {
                            crate::theme::green()
                        } else if connecting {
                            crate::theme::yellow()
                        } else {
                            crate::theme::red()
                        };
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(status_label).color(status_color),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            if connected {
                                if view.capabilities.disconnect {
                                    actions.push(SerialAction::Disconnect {
                                        port: port.id.clone(),
                                    });
                                }
                            } else if !connecting && view.capabilities.connect {
                                actions.push(SerialAction::Connect {
                                    port: port.id.clone(),
                                    settings: *view.settings,
                                });
                            }
                        }
                        ui.monospace(if alias.trim().is_empty() {
                            port.label.as_str()
                        } else {
                            alias.trim()
                        })
                        .on_hover_text(&port.id);
                        ui.label(&port.kind);
                        ui.label("别名");
                        let alias_response = ui.add(
                            egui::TextEdit::singleline(&mut alias)
                                .desired_width((ui.available_width() - 210.0).clamp(110.0, 240.0))
                                .hint_text("例如 主控板"),
                        );
                        if alias_response.changed() {
                            if alias.trim().is_empty() {
                                metadata.aliases.remove(&port.id);
                            } else {
                                metadata
                                    .aliases
                                    .insert(port.id.clone(), alias.trim().to_owned());
                            }
                        }
                        egui::ComboBox::from_id_salt(("shared-port-group-select", &port.id))
                            .selected_text(&selected_group)
                            .width(125.0)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_value(
                                        &mut selected_group,
                                        "未分组".to_owned(),
                                        "未分组",
                                    )
                                    .changed()
                                {
                                    metadata.groups.remove(&port.id);
                                }
                                for group in &group_names {
                                    if ui
                                        .selectable_value(&mut selected_group, group.clone(), group)
                                        .changed()
                                    {
                                        metadata.groups.insert(port.id.clone(), group.clone());
                                    }
                                }
                            });
                    });
                }
            }
        }

        let mut rename_group = None;
        let mut delete_group = None;
        if !group_names.is_empty() {
            ui.collapsing("分组管理", |ui| {
                for group in &group_names {
                    let edit_id = ui.make_persistent_id(("shared-port-rename", group));
                    let mut replacement = ui.ctx().data_mut(|data| {
                        data.get_persisted::<String>(edit_id)
                            .unwrap_or_else(|| group.clone())
                    });
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut replacement).desired_width(140.0));
                        if ui.small_button("改名").clicked()
                            && !replacement.trim().is_empty()
                            && replacement.trim() != group
                        {
                            rename_group = Some((group.clone(), replacement.trim().to_owned()));
                        }
                        if ui.small_button("删除").clicked() {
                            delete_group = Some(group.clone());
                        }
                    });
                    ui.ctx()
                        .data_mut(|data| data.insert_persisted(edit_id, replacement));
                }
            });
        }

        // A new group is created by assigning it to a selected port. This
        // keeps the persisted model small and mirrors Native's “group only
        // exists once it contains a port” behavior.
        let new_group_id = ui.make_persistent_id("shared-port-new-group-name");
        let new_port_id = ui.make_persistent_id("shared-port-new-group-port");
        let mut new_group = ui.ctx().data_mut(|data| {
            data.get_persisted::<String>(new_group_id)
                .unwrap_or_default()
        });
        let mut new_port = ui.ctx().data_mut(|data| {
            data.get_persisted::<String>(new_port_id)
                .or_else(|| view.ports.first().map(|port| port.id.clone()))
                .unwrap_or_default()
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("新建分组");
            egui::ComboBox::from_id_salt("shared-port-new-group-port")
                .selected_text(if new_port.is_empty() {
                    "选择端口"
                } else {
                    new_port.as_str()
                })
                .show_ui(ui, |ui| {
                    for port in view.ports {
                        ui.selectable_value(&mut new_port, port.id.clone(), &port.id);
                    }
                });
            ui.add(
                egui::TextEdit::singleline(&mut new_group)
                    .desired_width(130.0)
                    .hint_text("组名"),
            );
            if ui
                .add_enabled(
                    !new_port.is_empty() && !new_group.trim().is_empty(),
                    egui::Button::new("添加"),
                )
                .clicked()
            {
                metadata
                    .groups
                    .insert(new_port.clone(), new_group.trim().to_owned());
                new_group.clear();
            }
        });
        ui.ctx()
            .data_mut(|data| data.insert_persisted(new_group_id, new_group));
        ui.ctx()
            .data_mut(|data| data.insert_persisted(new_port_id, new_port));

        if let Some(group) = delete_group {
            metadata.groups.retain(|_, value| value != &group);
        }
        if let Some((old_group, new_group)) = rename_group {
            metadata.groups.values_mut().for_each(|value| {
                if value == &old_group {
                    *value = new_group.clone();
                }
            });
        }

        view.metadata = Some(metadata);
        actions
    }

    /// Render DTR/RTS controls for the currently connected port.
    pub fn signal_ui(ui: &mut Ui, view: &mut SerialView<'_>, actions: &mut Vec<SerialAction>) {
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
    }

    pub fn settings_ui(ui: &mut Ui, settings: &mut SerialSettings) {
        // The settings row is also used inside the shared left Dock. At
        // roughly 500px of usable width four default ComboBox widths are
        // enough to push parity outside the card. Keep the same one-row
        // composition with a compact width breakpoint instead of relying on
        // horizontal_wrapped to hide the last control below the next section.
        let combo_width = if ui.available_width() < 620.0 {
            88.0
        } else {
            118.0
        };
        ui.horizontal_wrapped(|ui| {
            ui.label("串口参数");
            egui::ComboBox::from_id_salt("shared-serial-baud-rate")
                .width(combo_width)
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
                .width(combo_width)
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
                .width(combo_width)
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
                .width(combo_width)
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

    pub fn sender_ui(ui: &mut Ui, view: &mut SerialView<'_>, actions: &mut Vec<SerialAction>) {
        let Some(port) = view.connected else {
            ui.colored_label(crate::theme::text_secondary(), "请先连接串口后再发送数据");
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
