use eframe::egui;
use tool_transport::SerialPortDescriptor;

const SERIAL_ACTION_BUTTON_SIZE: egui::Vec2 = egui::vec2(52.0, 26.0);

pub(crate) fn serial_combo(
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
                    format!("{alias} ({})  {}", p.port_name, p.port_type)
                } else {
                    format!("{}  {}", p.port_name, p.port_type)
                }
            })
        })
        .unwrap_or_else(|| {
            if ports.is_empty() {
                "无端口".to_owned()
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
                        format!("{alias} ({})  {}", port.port_name, port.port_type)
                    } else {
                        format!("{}  {}", port.port_name, port.port_type)
                    };
                    ui.selectable_value(sel, Some(port.port_name.clone()), label);
                }
            }
        });
}

pub(crate) fn baud_combo(ui: &mut egui::Ui, id: &'static str, w: f32, baud: &mut String) {
    let r = [
        "9600", "19200", "38400", "57600", "115200", "230400", "460800", "921600",
    ];
    egui::ComboBox::from_id_salt(id)
        .width(w)
        .selected_text(baud.clone())
        .show_ui(ui, |ui| {
            for x in r {
                ui.selectable_value(baud, x.to_owned(), x);
            }
        });
}

pub(crate) fn serial_action_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_sized(SERIAL_ACTION_BUTTON_SIZE, egui::Button::new(text))
}

pub(crate) fn serial_action_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    text: &str,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(text).min_size(SERIAL_ACTION_BUTTON_SIZE),
    )
}

use tool_transport::{DataBits, Parity, StopBits};

pub(crate) fn pdb(v: &str) -> DataBits {
    match v {
        "5" => DataBits::Five,
        "6" => DataBits::Six,
        "7" => DataBits::Seven,
        _ => DataBits::Eight,
    }
}
pub(crate) fn psb(v: &str) -> StopBits {
    match v {
        "2" => StopBits::Two,
        _ => StopBits::One,
    }
}
pub(crate) fn ppar(v: &str) -> Parity {
    match v {
        "odd" => Parity::Odd,
        "even" => Parity::Even,
        _ => Parity::None,
    }
}

use crate::app::WorkbenchApp;
use crate::state::StatusLevel;
use crate::ui::layout_buttons::{LayoutButtonKind, layout_icon_button};
use tool_panels::theme;

impl WorkbenchApp {
    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let so = self
                .selected_port
                .as_deref()
                .is_some_and(|p| self.transport.status_port(p).open);
            let sl = if so {
                format!("串口 ▸ {}", self.selected_port.as_deref().unwrap_or("?"))
            } else {
                "串口 ▸ 未连接".into()
            };
            if ui
                .selectable_label(
                    !self.top_bar_serial_collapsed,
                    egui::RichText::new(format!("{} {sl}", if so { "●" } else { "○" }))
                        .color(if so { theme::GREEN } else { theme::RED }),
                )
                .clicked()
            {
                self.top_bar_serial_collapsed = !self.top_bar_serial_collapsed;
            }
            if !self.top_bar_serial_collapsed {
                self.serial_connect_controls(ui, "top-port", "top-baud", 130.0, 80.0, true);
            }
            ui.separator();
            let rec = self.recorder.is_running();
            if ui
                .button(if rec {
                    egui::RichText::new("⏹ 停止").color(theme::RED)
                } else {
                    egui::RichText::new("⏺ 录制").color(theme::TEXT_SECONDARY)
                })
                .clicked()
            {
                self.start_or_stop_recording();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if layout_icon_button(
                    ui,
                    LayoutButtonKind::RightDock,
                    self.panels.dock.right_visible,
                    "显示/隐藏右侧停靠区",
                )
                .clicked()
                {
                    self.panels.dock.right_visible = !self.panels.dock.right_visible;
                    let _ = self.save_config();
                }

                if layout_icon_button(
                    ui,
                    LayoutButtonKind::BottomPanel,
                    self.panels.dock.bottom_visible,
                    "显示/隐藏底部面板",
                )
                .clicked()
                {
                    self.panels.dock.bottom_visible = !self.panels.dock.bottom_visible;
                    self.bottom_panel_visible = self.panels.dock.bottom_visible;
                    let _ = self.save_config();
                }

                if layout_icon_button(
                    ui,
                    LayoutButtonKind::ActivityBar,
                    self.panels.dock.activity_bar_visible,
                    "显示/隐藏左侧活动栏",
                )
                .clicked()
                {
                    self.panels.dock.activity_bar_visible = !self.panels.dock.activity_bar_visible;
                    let _ = self.save_config();
                }

                if layout_icon_button(ui, LayoutButtonKind::Menu, false, "重置布局").clicked() {
                    self.panels.dock = tool_panels::DockLayout::default();
                    self.bottom_panel_visible = self.panels.dock.bottom_visible;
                    let _ = self.save_config();
                }
            });
        });
    }

    pub(crate) fn serial_connect_controls(
        &mut self,
        ui: &mut egui::Ui,
        port_combo_id: &'static str,
        baud_combo_id: &'static str,
        port_width: f32,
        baud_width: f32,
        compact: bool,
    ) {
        if !compact {
            ui.label("端口");
        }

        serial_combo(
            ui,
            port_combo_id,
            port_width,
            &self.ports,
            &mut self.selected_port,
            &self.port_aliases,
        );

        if compact {
            // 顶栏只显示连接状态，详细参数统一在设备页
        } else {
            ui.label("波特率");
            baud_combo(ui, baud_combo_id, baud_width, &mut self.baud_rate);
        }

        let selected_open = self
            .selected_port
            .as_deref()
            .is_some_and(|port| self.transport.status_port(port).open);

        if selected_open {
            if serial_action_button(ui, "重连").clicked() {
                self.open_selected_port();
            }
        } else if serial_action_button(ui, "打开").clicked() {
            self.open_selected_port();
        }

        if serial_action_button_enabled(ui, selected_open, "关闭").clicked() {
            if let Some(ref port) = self.selected_port {
                self.transport.close_port(port);
                self.set_status(StatusLevel::Info, format!("{port} 已关闭"));
            }
        }

        if !compact {
            match self.selected_port.as_deref() {
                Some(port) => {
                    let st = self.transport.status_port(port);

                    if st.open {
                        ui.label(
                            egui::RichText::new(format!(
                                "● {} @ {} {}N{}",
                                port,
                                st.baud_rate.unwrap_or(0),
                                &self.data_bits,
                                &self.stop_bits
                            ))
                            .color(theme::GREEN),
                        );
                    } else {
                        ui.label(egui::RichText::new("○ 未连接").color(theme::TEXT_SECONDARY));
                    }
                }
                None => {
                    ui.label(egui::RichText::new("○ 未选择串口").color(theme::TEXT_SECONDARY));
                }
            }
        }
    }
}

// ══════════════════════════════════════════
//  eframe::App
// ══════════════════════════════════════════
