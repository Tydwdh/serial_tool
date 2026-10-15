use eframe::egui;
use egui_material_icons::icons::{ICON_CANCEL, ICON_FIBER_MANUAL_RECORD, ICON_REFRESH, ICON_STOP};
use std::collections::BTreeMap;
use tool_panels::design::{self, ButtonKind};
use tool_panels::{SerialPanel, SerialTopBarAction, SerialTopBarView, theme};

use crate::app::WorkbenchApp;
use crate::reconnect::RECONNECT_MAX_ATTEMPTS;
use crate::state::StatusLevel;

impl WorkbenchApp {
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let selected_before = self.serial.selected_port.clone();
        let ports = self.native_port_items();
        let aliases: BTreeMap<String, String> = self
            .serial
            .port_aliases
            .iter()
            .map(|(port, alias)| (port.clone(), alias.clone()))
            .collect();
        let connected = selected_before.as_deref().and_then(|port| {
            self.workbench
                .transport_status(port)
                .open
                .then(|| port.to_owned())
        });
        let connecting = selected_before
            .as_deref()
            .is_some_and(|port| self.workbench.transport_status(port).connecting);
        let settings = self.workbench.serial_settings();
        let mut selected_port = selected_before.clone();
        let mut collapsed = self.serial.top_bar_serial_collapsed;
        let mut serial_actions = Vec::new();

        ui.horizontal_wrapped(|ui| {
            let mut view = SerialTopBarView {
                ports: &ports,
                aliases: &aliases,
                selected: &mut selected_port,
                connected: connected.as_deref(),
                connecting,
                collapsed: &mut collapsed,
                settings,
                show_request_port: false,
            };
            serial_actions = SerialPanel::top_bar_contents_ui(ui, &mut view);

            // 自动重连进度：拔串口后顶部栏直接可见，无需展开 device_panel。
            // 多端口同时重连时这里只展示一条（选中的优先），其余由设备面板与日志承载。
            let pending = self
                .serial
                .reconnects
                .display(self.serial.selected_port.as_deref())
                .cloned();
            if let Some(pending) = pending {
                let now = tool_core::now_timestamp_ms() as f64 / 1000.0;
                let remaining = (pending.next_try_at - now).max(0.0);
                let extra = self.serial.reconnects.len().saturating_sub(1);
                let mut label = format!(
                    "{} 重连中 {} {:.1}s ({}/{})",
                    ICON_REFRESH.codepoint,
                    pending.port,
                    remaining,
                    pending.attempts + 1,
                    RECONNECT_MAX_ATTEMPTS
                );
                if extra > 0 {
                    label.push_str(&format!(" +{extra}"));
                }
                ui.label(egui::RichText::new(label).color(theme::yellow()))
                    .on_hover_text("点击 × 取消这个端口的自动重连");
                if design::icon_button(ui, ICON_CANCEL, "取消重连").clicked() {
                    self.cancel_auto_reconnect(&pending.port, "已取消自动重连 ");
                }
            }
            ui.separator();
            // ── 插件贡献：top_bar.left ──
            self.ui_contribution_slot(ui, "top_bar.left");
            let recording_running = self.workbench.query_recording().stats.running;
            let record_response = if recording_running {
                design::button(ui, ICON_STOP, "停止录制", ButtonKind::Danger)
            } else {
                design::button(ui, ICON_FIBER_MANUAL_RECORD, "开始录制", ButtonKind::Ghost)
            };
            if record_response.clicked() {
                self.start_or_stop_recording();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // ── 插件贡献：top_bar.right ──
                self.ui_contribution_slot(ui, "top_bar.right");
            });
        });

        if selected_port != selected_before
            && let Some(new_port) = selected_port.as_deref()
        {
            self.switch_port_selection(selected_before.as_deref(), new_port);
        }
        self.serial.top_bar_serial_collapsed = collapsed;
        if let Some(action) = serial_actions.into_iter().next() {
            match action {
                SerialTopBarAction::Refresh => self.refresh_ports_silent(),
                SerialTopBarAction::RequestPort => self.refresh_ports_silent(),
                SerialTopBarAction::Connect { port } => {
                    if self.serial.selected_port.as_deref() != Some(port.as_str()) {
                        let old_port = self.serial.selected_port.clone();
                        self.switch_port_selection(old_port.as_deref(), &port);
                    }
                    self.open_selected_port();
                }
                SerialTopBarAction::Disconnect { port } => {
                    self.cancel_pending_port_open_notice(&port);
                    match self
                        .workbench
                        .dispatch(tool_application::AppCommand::Disconnect {
                            port: tool_platform::PortId::new(port.clone()),
                        }) {
                        Ok(tool_application::CommandOutcome::Pending { .. }) => {
                            self.set_status(StatusLevel::Info, format!("正在关闭 {port}..."));
                        }
                        Ok(tool_application::CommandOutcome::Done) => {}
                        Err(error) => self.set_status(StatusLevel::Error, error.to_string()),
                    }
                }
                SerialTopBarAction::Reconnect { port } => {
                    if self.serial.selected_port.as_deref() != Some(port.as_str()) {
                        let old_port = self.serial.selected_port.clone();
                        self.switch_port_selection(old_port.as_deref(), &port);
                    }
                    self.reconnect_selected_port();
                }
            }
        }
    }
}
