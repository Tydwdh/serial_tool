use eframe::egui;
use egui_material_icons::{
    MaterialIcon,
    icons::{
        ICON_CANCEL, ICON_FIBER_MANUAL_RECORD, ICON_LINK_OFF, ICON_POWER_SETTINGS_NEW,
        ICON_REFRESH, ICON_STOP,
    },
};
use tool_panels::design::{self, ButtonKind};
use tool_transport::SerialPortDescriptor;

const SERIAL_ACTION_BUTTON_SIZE: egui::Vec2 = egui::vec2(52.0, 26.0);

pub(super) fn serial_combo(
    ui: &mut egui::Ui,
    id: &'static str,
    w: f32,
    ports: &[SerialPortDescriptor],
    sel: &mut Option<String>,
    aliases: &std::collections::HashMap<String, String>,
) {
    let selected_text = sel
        .as_deref()
        .and_then(|name| {
            ports.iter().find(|p| p.port_name == name).map(|p| {
                if let Some(alias) = aliases.get(&p.port_name).filter(|s| !s.trim().is_empty()) {
                    format!("{alias} ({})", p.port_name)
                } else {
                    p.port_name.to_string()
                }
            })
        })
        .unwrap_or_else(|| {
            if ports.is_empty() {
                "无可用串口".to_owned()
            } else {
                "请选择串口".to_owned()
            }
        });

    egui::ComboBox::from_id_salt(id)
        .width(w)
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            if ports.is_empty() {
                ui.add_enabled(false, egui::Label::new("无可用串口"));
            } else {
                for port in ports {
                    let label = if let Some(alias) = aliases
                        .get(&port.port_name)
                        .filter(|s| !s.trim().is_empty())
                    {
                        format!("{alias} ({})", port.port_name)
                    } else {
                        port.port_name.to_string()
                    };
                    ui.selectable_value(sel, Some(port.port_name.clone()), label);
                }
            }
        });
}

pub(super) fn serial_action_button(
    ui: &mut egui::Ui,
    icon: MaterialIcon,
    text: &str,
) -> egui::Response {
    ui.add_sized(
        SERIAL_ACTION_BUTTON_SIZE,
        egui::Button::new(design::icon_text(icon, text)).corner_radius(6.0),
    )
}

pub(super) fn serial_action_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: MaterialIcon,
    text: &str,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(design::icon_text(icon, text))
            .corner_radius(6.0)
            .min_size(SERIAL_ACTION_BUTTON_SIZE),
    )
}

use crate::app::WorkbenchApp;
use crate::state::StatusLevel;
use tool_panels::theme;

impl WorkbenchApp {
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let so = self
                .serial
                .selected_port
                .as_deref()
                .is_some_and(|p| self.transport.status_port(p).open);
            let sl = if so {
                format!(
                    "串口 ▸ {}",
                    self.serial
                        .selected_port
                        .as_deref()
                        .map(|p| self.serial.port_label(p))
                        .unwrap_or_else(|| "?".to_owned())
                )
            } else {
                "串口 ▸ 未连接".into()
            };
            if ui
                .selectable_label(
                    !self.serial.top_bar_serial_collapsed,
                    egui::RichText::new(format!(
                        "{} {sl}",
                        egui_material_icons::icons::ICON_CABLE.codepoint
                    ))
                    .color(if so { theme::green() } else { theme::red() }),
                )
                .clicked()
            {
                self.serial.top_bar_serial_collapsed = !self.serial.top_bar_serial_collapsed;
            }
            if !self.serial.top_bar_serial_collapsed {
                let before = self.serial.selected_port.clone();
                serial_combo(
                    ui,
                    "top-port",
                    120.0,
                    &self.serial.ports,
                    &mut self.serial.selected_port,
                    &self.serial.port_aliases,
                );
                // 端口切换时：保存旧配置、恢复新配置
                if self.serial.selected_port != before {
                    let new_port = self.serial.selected_port.clone();
                    if let Some(ref new) = new_port {
                        self.switch_port_selection(before.as_deref(), new);
                    }
                }
                let selected_status = self
                    .serial
                    .selected_port
                    .as_deref()
                    .map(|port| self.transport.status_port(port))
                    .unwrap_or_else(tool_transport::TransportStatus::closed);
                let selected_open = selected_status.open;
                let selected_connecting = selected_status.connecting;

                if selected_open {
                    if serial_action_button(ui, ICON_REFRESH, "重连").clicked() {
                        self.reconnect_selected_port();
                    }
                } else if selected_connecting {
                    // 连接中：按钮禁用，防止重复打开
                    ui.add_enabled_ui(false, |ui| {
                        serial_action_button(ui, ICON_POWER_SETTINGS_NEW, "连接中");
                    });
                } else if serial_action_button(ui, ICON_POWER_SETTINGS_NEW, "打开").clicked() {
                    self.open_selected_port();
                }

                let mut close_btn =
                    serial_action_button_enabled(ui, selected_open, ICON_LINK_OFF, "关闭");
                if !selected_open {
                    close_btn = close_btn.on_disabled_hover_text("端口未打开");
                }
                if close_btn.clicked()
                    && let Some(ref port) = self.serial.selected_port
                {
                    self.transport.close_port(port);
                    self.set_status(StatusLevel::Info, format!("{port} 已关闭"));
                }
            } else if so {
                // 折叠时显示当前配置摘要：波特率 数据位 校验位 停止位
                // 校验位缩写：none→N, odd→O, even→E（标准串口缩写 8N1）
                let parity_abbr = match self.serial.parity.as_str() {
                    "odd" => 'O',
                    "even" => 'E',
                    _ => 'N',
                };
                ui.label(
                    egui::RichText::new(format!(
                        "· {} {}{}{}",
                        self.serial.baud_rate,
                        self.serial.data_bits,
                        parity_abbr,
                        self.serial.stop_bits,
                    ))
                    .color(theme::text_secondary()),
                );
            }
            // 自动重连进度：拔串口后顶部栏直接可见，无需展开 device_panel。
            if let Some(ref pending) = self.serial.pending_reconnect {
                let now = tool_core::now_timestamp_ms() as f64 / 1000.0;
                let remaining = (pending.next_try_at - now).max(0.0);
                let label = format!(
                    "{} 重连中 {} {:.1}s ({}/{})",
                    ICON_REFRESH.codepoint,
                    pending.port_name,
                    remaining,
                    pending.attempts + 1,
                    10
                );
                ui.label(egui::RichText::new(label).color(theme::yellow()))
                    .on_hover_text("点击 × 取消等待重连");
                if design::icon_button(ui, ICON_CANCEL, "取消重连").clicked() {
                    self.serial.pending_reconnect = None;
                    self.set_status_force(StatusLevel::Info, "已取消自动重连".to_owned());
                }
            }
            ui.separator();
            // ── 插件贡献：top_bar.left ──
            self.ui_contribution_slot(ui, "top_bar.left");
            let rec = self.recorder.is_running();
            let record_response = if rec {
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
    }
}
