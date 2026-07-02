use eframe::egui;
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
                        format!("{alias} ({})", port.port_name)
                    } else {
                        port.port_name.to_string()
                    };
                    ui.selectable_value(sel, Some(port.port_name.clone()), label);
                }
            }
        });
}

pub(super) fn serial_action_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_sized(SERIAL_ACTION_BUTTON_SIZE, egui::Button::new(text))
}

pub(super) fn serial_action_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    text: &str,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(text).min_size(SERIAL_ACTION_BUTTON_SIZE),
    )
}

use crate::app::WorkbenchApp;
use crate::state::StatusLevel;
use crate::ui::layout_buttons::{LayoutButtonKind, layout_icon_button};
use tool_panels::theme;

impl WorkbenchApp {
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
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
                    egui::RichText::new(format!("{} {sl}", if so { "●" } else { "○" }))
                        .color(if so { theme::GREEN } else { theme::RED }),
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
                let selected_open = self
                    .serial
                    .selected_port
                    .as_deref()
                    .is_some_and(|port| self.transport.status_port(port).open);

                if selected_open {
                    if serial_action_button(ui, "重连").clicked() {
                        self.reconnect_selected_port();
                    }
                } else if serial_action_button(ui, "打开").clicked() {
                    self.open_selected_port();
                }

                if serial_action_button_enabled(ui, selected_open, "关闭").clicked()
                    && let Some(ref port) = self.serial.selected_port
                {
                    self.transport.close_port(port);
                    self.set_status(StatusLevel::Info, format!("{port} 已关闭"));
                }
            } else if so {
                // 折叠时显示当前配置摘要
                ui.label(
                    egui::RichText::new(format!(
                        "· {} {}N{}",
                        self.serial.baud_rate, self.serial.data_bits, self.serial.stop_bits,
                    ))
                    .color(theme::TEXT_SECONDARY),
                );
            }
            // 自动重连进度：拔串口后顶部栏直接可见，无需展开 device_panel。
            if let Some(ref pending) = self.serial.pending_reconnect {
                let now = tool_core::now_timestamp_ms() as f64 / 1000.0;
                let remaining = (pending.next_try_at - now).max(0.0);
                let label = format!(
                    "⟳ 重连中 {} {:.1}s ({}/{})",
                    pending.port_name,
                    remaining,
                    pending.attempts + 1,
                    10
                );
                ui.label(egui::RichText::new(label).color(theme::YELLOW))
                    .on_hover_text("点击关闭按钮可取消等待重连");
            }
            ui.separator();
            // ── 插件贡献：top_bar.left ──
            self.ui_contribution_slot(ui, "top_bar.left");
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
                // ── 插件贡献：top_bar.right ──
                self.ui_contribution_slot(ui, "top_bar.right");

                if layout_icon_button(
                    ui,
                    LayoutButtonKind::RightDock,
                    self.panels.dock.right_visible,
                    "显示/隐藏右侧停靠区",
                )
                .clicked()
                {
                    self.panels.dock.right_visible = !self.panels.dock.right_visible;
                    if let Err(e) = self.save_config() {
                        log::warn!("save_config failed: {e}")
                    };
                }

                if layout_icon_button(
                    ui,
                    LayoutButtonKind::BottomPanel,
                    self.panels.dock.bottom_visible,
                    "显示/隐藏底部面板",
                )
                .clicked()
                {
                    self.toggle_bottom_panel();
                    if let Err(e) = self.save_config() {
                        log::warn!("save_config failed: {e}")
                    };
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
                    if let Err(e) = self.save_config() {
                        log::warn!("save_config failed: {e}")
                    };
                }

                if layout_icon_button(ui, LayoutButtonKind::Menu, false, "重置布局").clicked() {
                    self.panels.dock = tool_panels::DockLayout::default();
                    self.set_bottom_visible(self.panels.dock.bottom_visible);
                    if let Err(e) = self.save_config() {
                        log::warn!("save_config failed: {e}")
                    };
                }
            });
        });
    }
}
