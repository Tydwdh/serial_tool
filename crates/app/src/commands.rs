use crate::app::WorkbenchApp;
use crate::config::{PersistedConfig, config_path};
use crate::state::{BottomTab, StatusLevel};
use crate::ui::top_bar::{pdb, ppar, psb};
use std::collections::BTreeSet;
use std::path::PathBuf;
use tool_core::{LogLevel, now_timestamp_ms};
use tool_transport::SerialConfig;

impl WorkbenchApp {
    /// 统一状态入口。低级别不能覆盖未过期的高级消息。
    pub(crate) fn set_status(&mut self, level: StatusLevel, text: impl Into<String>) {
        let now = now_timestamp_ms();
        if level as u8 >= self.status.level as u8 || now > self.status.deadline_ms {
            self.status.level = level;
            self.status.message = text.into();
            self.status.deadline_ms = now + level.ttl_ms();
        }
    }

    /// 用户主动操作：总是更新状态（不被旧错误阻塞）。
    pub(crate) fn set_status_force(&mut self, level: StatusLevel, text: impl Into<String>) {
        let now = now_timestamp_ms();
        self.status.level = level;
        self.status.message = text.into();
        self.status.deadline_ms = now + level.ttl_ms();
    }

    /// 过期后重置为就绪。每帧调用。
    pub(crate) fn clear_status_if_expired(&mut self) {
        if now_timestamp_ms() > self.status.deadline_ms {
            self.status.level = StatusLevel::Info;
            self.status.message = "就绪".into();
        }
    }
    pub(crate) fn refresh_ports(&mut self) {
        self.refresh_ports_impl(true);
    }

    pub(crate) fn refresh_ports_silent(&mut self) {
        self.refresh_ports_impl(false);
    }

    pub(crate) fn refresh_ports_impl(&mut self, show_status: bool) {
        let old_names: BTreeSet<String> = self
            .ports
            .iter()
            .map(|port| port.port_name.clone())
            .collect();

        let old_selected = self.selected_port.clone();

        match self.transport.list_serial_ports() {
            Ok(new_ports) => {
                let new_names: BTreeSet<String> = new_ports
                    .iter()
                    .map(|port| port.port_name.clone())
                    .collect();

                let added_ports: Vec<String> = new_names.difference(&old_names).cloned().collect();

                let removed_ports: Vec<String> =
                    old_names.difference(&new_names).cloned().collect();

                self.ports = new_ports;

                let selected_still_exists = self
                    .selected_port
                    .as_ref()
                    .is_some_and(|selected| new_names.contains(selected));

                // 只在端口消失且 transport 也未打开时才清空选择
                if !selected_still_exists {
                    if let Some(ref selected) = self.selected_port {
                        if self.transport.status_port(selected).open {
                            self.set_status(
                                StatusLevel::Warn,
                                format!("{selected} 已打开但不在系统列表中"),
                            );
                        } else {
                            let stale_port = self.selected_port.take();
                            if let Some(ref p) = stale_port {
                                self.set_status(StatusLevel::Warn, format!("{p} 已拔出或不可用"));
                            }
                        }
                    }
                }

                if show_status {
                    self.set_status(StatusLevel::Info, format!("{} 个串口", self.ports.len()));
                    return;
                }

                if !added_ports.is_empty() {
                    self.set_status(
                        StatusLevel::Info,
                        format!("发现串口 {}", added_ports.join(", ")),
                    );
                } else if !removed_ports.is_empty() {
                    self.set_status(
                        StatusLevel::Info,
                        format!("移除串口 {}", removed_ports.join(", ")),
                    );
                } else if self.selected_port != old_selected {
                    self.set_status(StatusLevel::Info, "请选择串口");
                }
            }
            Err(error) => {
                self.set_status(StatusLevel::Error, error.to_string());
            }
        }
    }

    pub(crate) fn open_selected_port(&mut self) {
        self.refresh_ports_silent();

        let Some(p) = self.selected_port.clone() else {
            self.log(LogLevel::Warn, "请选择串口");
            self.set_status(StatusLevel::Warn, "请选择串口");
            return;
        };

        let selected_exists = self.ports.iter().any(|port| port.port_name == p);

        if !selected_exists {
            self.set_status(StatusLevel::Error, format!("串口 {p} 不存在，请重新选择"));
            return;
        }

        let baud_rate = match self.baud_rate.trim().parse::<u32>() {
            Ok(v) if v > 0 => v,
            _ => {
                self.set_status_force(StatusLevel::Warn, "波特率格式错误");
                return;
            }
        };

        let timeout_ms = match self.timeout_ms.trim().parse::<u64>() {
            Ok(v) if (1..=1000).contains(&v) => v,
            _ => {
                self.set_status_force(StatusLevel::Warn, "超时时间必须为 1..=1000 ms");
                return;
            }
        };

        let cfg = SerialConfig {
            port_name: p.clone(),
            baud_rate,
            data_bits: pdb(&self.data_bits),
            stop_bits: psb(&self.stop_bits),
            parity: ppar(&self.parity),
            timeout_ms,
        };

        match self.transport.open_serial(cfg) {
            Ok(()) => {
                self.set_status_force(StatusLevel::Info, format!("{p} 已连接"));
                self.open_bottom_panel();
            }
            Err(e) => {
                self.set_status_force(StatusLevel::Error, e.to_string());
            }
        }
    }
    pub(crate) fn start_or_stop_recording(&mut self) {
        if self.recorder.is_running() || self.recorder.is_stopping() {
            self.recorder.stop();
            self.set_status_force(StatusLevel::Info, "正在停止录制...");
        } else {
            match self.recorder.start(PathBuf::from(&self.recorder_path)) {
                Ok(()) => {
                    self.set_status_force(StatusLevel::Info, "录制中");
                }
                Err(e) => {
                    self.set_status_force(StatusLevel::Error, e.to_string());
                }
            }
        }
    }
    pub(crate) fn save_config(&mut self) -> Result<(), String> {
        self.panels.bottom_logs_visible = self.bottom_panel_visible;
        let mut p = self.panels.clone();
        p.discard_dynamic_tabs();
        p.bottom_logs_visible = self.bottom_panel_visible;
        let cfg = PersistedConfig {
            panels: p,
            selected_port: self.selected_port.clone(),
            baud_rate: self.baud_rate.clone(),
            data_bits: self.data_bits.clone(),
            stop_bits: self.stop_bits.clone(),
            parity: self.parity.clone(),
            timeout_ms: self.timeout_ms.clone(),
            recorder_path: self.recorder_path.clone(),
            activity_order: self.activity_order.clone(),
            enabled_plugins: self
                .plugin_manager
                .summaries()
                .into_iter()
                .filter(|s| {
                    matches!(
                        s.state,
                        tool_extension::PluginState::Enabled | tool_extension::PluginState::Running
                    )
                })
                .map(|s| s.id)
                .collect(),
        };
        let t = serde_json::to_string_pretty(&cfg).map_err(|e| format!("序列化失败：{e}"))?;
        std::fs::write(config_path(), t).map_err(|e| format!("写入失败：{e}"))
    }
    pub(crate) fn available_bottom_tabs(&self) -> Vec<BottomTab> {
        BottomTab::ALL
            .into_iter()
            .filter(|tab| tab.is_available(self.terminal_popup_open))
            .collect()
    }

    pub(crate) fn ensure_bottom_tab_available(&mut self) {
        if self.bottom_tab.is_available(self.terminal_popup_open) {
            return;
        }
        if let Some(tab) = self.available_bottom_tabs().into_iter().next() {
            self.bottom_tab = tab;
        }
    }

    pub(crate) fn open_bottom_panel(&mut self) {
        self.bottom_panel_visible = true;
        if BottomTab::Terminal.is_available(self.terminal_popup_open) {
            self.bottom_tab = BottomTab::Terminal;
        } else {
            self.ensure_bottom_tab_available();
        }
    }

    pub(crate) fn toggle_bottom_panel(&mut self) {
        if self.bottom_panel_visible {
            self.bottom_panel_visible = false;
        } else {
            self.open_bottom_panel();
            self.set_status(StatusLevel::Info, "底部面板已打开");
        }
    }
}
