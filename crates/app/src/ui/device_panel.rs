use crate::app::WorkbenchApp;
use crate::config::pick_recorder_path;
use crate::state::StatusLevel;
use eframe::egui;
use egui_material_icons::icons::{ICON_CABLE, ICON_TUNE, ICON_WARNING};
use std::collections::{BTreeMap, BTreeSet};
use tool_application::query::RecordModeView;
use tool_panels::{
    NetworkSerialAction, NetworkSerialFormView, RecordingAction, RecordingMode, RecordingView,
    SerialPanel, design, network_serial_form_ui, recording_ui, theme,
};
use tool_platform::{SerialParity, SerialSettings};

impl WorkbenchApp {
    fn recording_panel(&mut self, ui: &mut egui::Ui) {
        let recording = self.workbench.query_recording();
        let stats = recording.stats.clone();
        let running = stats.running;
        let stopping = stats.stopping;
        let paused = stats.paused;
        let mut mode = match recording.mode {
            RecordModeView::StandardReplay => RecordingMode::StandardReplay,
            RecordModeView::RawSerial => RecordingMode::RawSerial,
        };
        let actions = {
            let mut view = RecordingView {
                file_name: &mut self.recorder_path,
                mode: &mut mode,
                running,
                stopping,
                paused,
                events_written: stats.events_written,
                bytes_written: Some(stats.bytes_written),
                flush_elapsed_ms: Some(stats.last_flush_elapsed_ms),
                backlog_events: None,
                backlog_bytes: None,
                current_path: recording.path.as_deref(),
                last_error: stats.last_error.as_deref(),
                show_browse: true,
            };
            recording_ui(ui, &mut view)
        };

        let native_mode = match mode {
            RecordingMode::StandardReplay => RecordModeView::StandardReplay,
            RecordingMode::RawSerial => RecordModeView::RawSerial,
        };
        if native_mode != recording.mode
            && let Err(error) = self
                .workbench
                .dispatch(tool_application::AppCommand::SetRecordingMode { mode: native_mode })
        {
            self.set_status_force(StatusLevel::Error, error.to_string());
        }

        for action in actions {
            match action {
                RecordingAction::Browse => {
                    if let Some(path) = pick_recorder_path(&self.recorder_path) {
                        self.recorder_path = path.display().to_string();
                    }
                }
                RecordingAction::StartStop => self.start_or_stop_recording(),
                RecordingAction::PauseResume => {
                    let command = if paused {
                        tool_application::AppCommand::ResumeRecording
                    } else {
                        tool_application::AppCommand::PauseRecording
                    };
                    if let Err(error) = self.workbench.dispatch(command) {
                        self.set_status_force(StatusLevel::Error, error.to_string());
                    }
                }
            }
        }
    }

    pub(crate) fn device_panel(&mut self, ui: &mut egui::Ui) {
        // ── 串口参数 ──
        design::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            design::section_header(ui, ICON_TUNE, "串口参数");
            ui.separator();

            let mut settings = SerialSettings {
                baud_rate: self.serial.baud_rate.parse().unwrap_or(115_200),
                data_bits: self.serial.data_bits.parse().unwrap_or(8),
                stop_bits: self.serial.stop_bits.parse().unwrap_or(1),
                parity: match self.serial.parity.as_str() {
                    "odd" => SerialParity::Odd,
                    "even" => SerialParity::Even,
                    _ => SerialParity::None,
                },
            };
            let previous_settings = settings;
            SerialPanel::settings_ui(ui, &mut settings);
            if settings != previous_settings {
                self.serial.baud_rate = settings.baud_rate.to_string();
                self.serial.data_bits = settings.data_bits.to_string();
                self.serial.stop_bits = settings.stop_bits.to_string();
                self.serial.parity = match settings.parity {
                    SerialParity::None => "none",
                    SerialParity::Odd => "odd",
                    SerialParity::Even => "even",
                }
                .to_owned();
                if let Err(error) = self
                    .workbench
                    .dispatch(tool_application::AppCommand::SetSerialSettings { settings })
                {
                    self.set_status_force(StatusLevel::Error, error.to_string());
                }
            }

            ui.checkbox(&mut self.serial.auto_reconnect, "串口拔出后自动重连");
            if self.serial.auto_reconnect
                && let Some(ref pending) = self.serial.pending_reconnect
            {
                let now = tool_core::now_timestamp_ms() as f64 / 1000.0;
                let remaining = (pending.next_try_at - now).max(0.0);
                ui.label(
                    egui::RichText::new(format!(
                        "等待 {} {:2.1}s 后重试 (第 {}/10 次)",
                        pending.port_name,
                        remaining,
                        pending.attempts + 1
                    ))
                    .color(theme::yellow()),
                );
            }
        });

        ui.add_space(8.0);

        self.recording_panel(ui);

        ui.add_space(8.0);

        // ── 可用端口 ──
        design::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            design::section_header(ui, ICON_CABLE, "可用端口");
            ui.separator();

            // ── 网络模拟串口（Nexus Prime 等 Klipper 服务器，WebSocket + JSON-RPC gcode 桥）──
            let add_network = {
                let mut form = NetworkSerialFormView {
                    host: &mut self.serial.network_host,
                    port: &mut self.serial.network_port,
                    api_key: &mut self.serial.network_api_key,
                };
                match network_serial_form_ui(ui, &mut form).into_iter().next() {
                    Some(NetworkSerialAction::Submit(config)) => Some(config),
                    Some(NetworkSerialAction::Error(error)) => {
                        self.set_status(StatusLevel::Error, error);
                        None
                    }
                    None => None,
                }
            };
            if let Some(cfg) = add_network {
                let name = cfg.display_name();
                if self
                    .serial
                    .network_ports
                    .iter()
                    .any(|n| n.display_name() == name)
                {
                    self.set_status_force(
                        StatusLevel::Warn,
                        format!("{name} 已存在，点击圆点连接"),
                    );
                } else {
                    self.serial.network_ports.push(cfg.clone());
                    let _ = self.workbench.dispatch(
                        tool_application::AppCommand::RegisterNetworkPort {
                            config: cfg.clone(),
                        },
                    );
                    if let Err(e) = self.save_config() {
                        log::warn!("save_config failed: {e}");
                    }
                    self.refresh_ports_silent();
                    // 添加即连接：异步建立 WebSocket，UI 进入“连接中”过渡态。
                    match self
                        .workbench
                        .dispatch(tool_application::AppCommand::Connect {
                            port: tool_platform::PortId::new(name.clone()),
                            settings: self.workbench.serial_settings(),
                        }) {
                        Ok(tool_application::CommandOutcome::Pending { .. }) => {
                            self.serial.selected_port = Some(name.clone());
                            self.defer_port_open_notice(&name, format!("{name} 已连接"));
                            self.set_status_force(StatusLevel::Info, format!("正在连接 {name}..."));
                        }
                        Ok(tool_application::CommandOutcome::Done) => {}
                        Err(error) => self.set_status_force(StatusLevel::Error, error.to_string()),
                    }
                }
                self.serial.selected_port = Some(name);
            }
            ui.separator();

            // 显示已打开但不在系统端口列表中的 stale 连接
            let transport_open = self.workbench.open_port_names();
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
                    ui.colored_label(
                        theme::orange(),
                        format!("{} 以下端口已打开但可能已拔出：", ICON_WARNING.codepoint),
                    );
                    for port in &stale {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(*port)
                                    .monospace()
                                    .color(theme::orange()),
                            );
                            // 两步确认：首次点击 → 变红"确认?" → 再次点击才执行。
                            // 5 秒后自动解除武装。
                            let confirm_id = ui.id().with(("force_close_confirm", *port));
                            let now = ui.input(|i| i.time);
                            let armed_ts: Option<f64> =
                                ui.ctx().memory(|m| m.data.get_temp(confirm_id));
                            let armed = armed_ts.is_some_and(|t| now - t < 5.0);
                            let label = if armed { "确认?" } else { "强制关闭" };
                            let btn =
                                egui::Button::new(egui::RichText::new(label).color(if armed {
                                    theme::red()
                                } else {
                                    theme::orange()
                                }))
                                .small();
                            if ui.add(btn).clicked() {
                                if armed {
                                    match self.workbench.dispatch(
                                        tool_application::AppCommand::Disconnect {
                                            port: tool_platform::PortId::new((*port).clone()),
                                        },
                                    ) {
                                        Ok(tool_application::CommandOutcome::Pending {
                                            ..
                                        }) => {
                                            self.set_status_force(
                                                StatusLevel::Info,
                                                format!("正在强制关闭 {port}..."),
                                            );
                                        }
                                        Ok(tool_application::CommandOutcome::Done) => {}
                                        Err(error) => self.set_status_force(
                                            StatusLevel::Error,
                                            error.to_string(),
                                        ),
                                    }
                                    ui.ctx()
                                        .memory_mut(|m| m.data.remove_temp::<f64>(confirm_id));
                                } else {
                                    ui.ctx().memory_mut(|m| m.data.insert_temp(confirm_id, now));
                                }
                            }
                            if armed && ui.small_button("取消").clicked() {
                                ui.ctx()
                                    .memory_mut(|m| m.data.remove_temp::<f64>(confirm_id));
                            }
                        });
                    }
                    ui.separator();
                }
            }

            // The grouped rows are rendered by the same shared component as
            // Web. Native only adapts its legacy string-keyed state and
            // dispatches the returned actions.
            let shared_ports: Vec<tool_panels::SerialPortItem> = self
                .serial
                .ports
                .iter()
                .map(|port| {
                    let status = self.workbench.transport_status(&port.port_name);
                    tool_panels::SerialPortItem {
                        id: port.port_name.clone(),
                        label: port.port_name.clone(),
                        kind: port.port_type.to_string(),
                        open: status.open,
                        connecting: status.connecting,
                        pending_reconnect: self
                            .serial
                            .pending_reconnect
                            .as_ref()
                            .is_some_and(|pending| pending.port_name == port.port_name),
                    }
                })
                .collect();
            let previous_aliases = self.serial.port_aliases.clone();
            let previous_groups = self.serial.port_groups.clone();
            let mut aliases: BTreeMap<String, String> =
                previous_aliases.clone().into_iter().collect();
            let mut groups: BTreeMap<String, String> =
                previous_groups.clone().into_iter().collect();
            let status = String::new();
            let mut settings = self.workbench.serial_settings();
            let mut send_input = String::new();
            let mut tx_hex = false;
            let mut dtr = false;
            let mut rts = false;
            let mut shared_view = tool_panels::SerialView {
                ports: &shared_ports,
                connected: None,
                connecting: None,
                status: &status,
                settings: &mut settings,
                send_input: &mut send_input,
                tx_hex: &mut tx_hex,
                dtr: &mut dtr,
                rts: &mut rts,
                capabilities: tool_platform::TransportCapabilities {
                    request_port: false,
                    ..tool_platform::TransportCapabilities::NATIVE_SERIAL
                },
                show_ports: true,
                show_sender: false,
                metadata: Some(tool_panels::SerialPortMetadata {
                    aliases: &mut aliases,
                    groups: &mut groups,
                }),
            };
            let shared_actions = tool_panels::SerialPanel::port_list_ui(ui, &mut shared_view);
            let metadata = shared_view
                .metadata
                .take()
                .expect("shared grouped port view must return metadata");
            let new_aliases: std::collections::HashMap<String, String> = metadata
                .aliases
                .iter()
                .map(|(port, alias)| (port.clone(), alias.clone()))
                .collect();
            let new_groups: std::collections::HashMap<String, String> = metadata
                .groups
                .iter()
                .map(|(port, group)| (port.clone(), group.clone()))
                .collect();
            let metadata_changed = previous_aliases != new_aliases || previous_groups != new_groups;
            self.serial.port_aliases = new_aliases;
            self.serial.port_groups = new_groups;
            if metadata_changed && let Err(error) = self.save_config() {
                log::warn!("save_config failed: {error}");
            }

            for action in shared_actions {
                match action {
                    tool_panels::SerialAction::Refresh | tool_panels::SerialAction::RequestPort => {
                        self.refresh_ports()
                    }
                    tool_panels::SerialAction::Connect { port, .. } => {
                        if self.serial.selected_port.as_deref() != Some(port.as_str()) {
                            let old_port = self.serial.selected_port.clone();
                            self.switch_port_selection(old_port.as_deref(), &port);
                        }
                        self.open_selected_port();
                    }
                    tool_panels::SerialAction::Disconnect { port } => {
                        self.cancel_pending_port_open_notice(&port);
                        if let Err(error) =
                            self.workbench
                                .dispatch(tool_application::AppCommand::Disconnect {
                                    port: tool_platform::PortId::new(port),
                                })
                        {
                            self.set_status_force(StatusLevel::Error, error.to_string());
                        }
                    }
                    tool_panels::SerialAction::CancelReconnect { port } => {
                        if let Err(error) =
                            self.workbench
                                .dispatch(tool_application::AppCommand::CancelReconnect {
                                    port: tool_platform::PortId::new(port),
                                })
                        {
                            self.set_status_force(StatusLevel::Error, error.to_string());
                        }
                    }
                    tool_panels::SerialAction::RemoveNetwork { port } => {
                        self.serial
                            .network_ports
                            .retain(|network| network.display_name() != port);
                        let _ = self.workbench.dispatch(
                            tool_application::AppCommand::RemoveNetworkPort {
                                port: tool_platform::PortId::new(port.clone()),
                            },
                        );
                        let _ = self
                            .workbench
                            .dispatch(tool_application::AppCommand::Disconnect {
                                port: tool_platform::PortId::new(port.clone()),
                            });
                        if self.serial.selected_port.as_deref() == Some(port.as_str()) {
                            self.serial.selected_port = None;
                        }
                        if let Err(error) = self.save_config() {
                            log::warn!("save_config failed: {error}");
                        }
                        self.refresh_ports_silent();
                        self.set_status_force(StatusLevel::Info, format!("{port} 已移除"));
                    }
                    tool_panels::SerialAction::SendText { .. }
                    | tool_panels::SerialAction::SendHex { .. }
                    | tool_panels::SerialAction::SetDtr { .. }
                    | tool_panels::SerialAction::SetRts { .. } => {}
                }
            }
        });
    }
}
