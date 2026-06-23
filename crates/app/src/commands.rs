use crate::app::WorkbenchApp;
use crate::state::PendingReconnect;
use crate::state::StatusLevel;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;
use tool_core::now_timestamp_ms;
use tool_transport::{SerialConfig, parse_data_bits, parse_parity, parse_stop_bits};

impl WorkbenchApp {
    /// 统一状态入口。低级别不能覆盖未过期的高级消息。
    pub(crate) fn set_status(&mut self, level: StatusLevel, text: impl Into<String>) {
        self.status.set(level, text);
    }

    /// 用户主动操作：总是更新状态（不被旧错误阻塞）。
    pub(crate) fn set_status_force(&mut self, level: StatusLevel, text: impl Into<String>) {
        self.status.set_force(level, text);
    }

    /// 过期后重置为就绪。每帧调用。
    pub(crate) fn clear_status_if_expired(&mut self) {
        self.status.clear_if_expired();
    }
    /// 切换串口时：保存旧端口设置到 profile，从 profile 恢复新端口设置。
    pub(crate) fn switch_port_selection(&mut self, old_port: Option<&str>, new_port: &str) {
        // 保存旧端口配置
        if let Some(old) = old_port {
            self.serial.port_profiles.insert(
                old.to_owned(),
                crate::config::PortProfile {
                    baud_rate: self.serial.baud_rate.clone(),
                    data_bits: self.serial.data_bits.clone(),
                    stop_bits: self.serial.stop_bits.clone(),
                    parity: self.serial.parity.clone(),
                },
            );
        }
        // 恢复新端口配置
        if let Some(profile) = self.serial.port_profiles.get(new_port) {
            self.serial.baud_rate = profile.baud_rate.clone();
            self.serial.data_bits = profile.data_bits.clone();
            self.serial.stop_bits = profile.stop_bits.clone();
            self.serial.parity = profile.parity.clone();
        }
        self.serial.selected_port = Some(new_port.to_owned());
    }

    pub(crate) fn refresh_ports(&mut self) {
        self.refresh_ports_impl(true);
    }

    pub(crate) fn refresh_ports_silent(&mut self) {
        self.refresh_ports_impl(false);
    }

    pub(crate) fn refresh_ports_impl(&mut self, show_status: bool) {
        let old_names: BTreeSet<String> = self
            .serial
            .ports
            .iter()
            .map(|port| port.port_name.clone())
            .collect();

        let old_selected = self.serial.selected_port.clone();

        match self.transport.list_serial_ports() {
            Ok(new_ports) => {
                let new_names: BTreeSet<String> = new_ports
                    .iter()
                    .map(|port| port.port_name.clone())
                    .collect();

                let added_ports: Vec<String> = new_names.difference(&old_names).cloned().collect();

                let removed_ports: Vec<String> =
                    old_names.difference(&new_names).cloned().collect();

                self.serial.ports = new_ports;
                self.dynamic_panels.set_ports(&self.serial.ports);

                let selected_still_exists = self
                    .serial
                    .selected_port
                    .as_ref()
                    .is_some_and(|selected| new_names.contains(selected));

                // 只在端口消失且 transport 也未打开时才清空选择
                if !selected_still_exists {
                    let selected_val = self.serial.selected_port.clone();
                    if let Some(ref selected) = selected_val {
                        // 保存配置快照（在 reap_dead_ports 移除 handle 前）
                        let snapshot = SerialConfig {
                            port_name: selected.clone(),
                            baud_rate: self.serial.baud_rate.parse().unwrap_or(115200),
                            data_bits: parse_data_bits(&self.serial.data_bits),
                            stop_bits: parse_stop_bits(&self.serial.stop_bits),
                            parity: parse_parity(&self.serial.parity),
                        };
                        if self.transport.status_port(selected).open {
                            self.set_status_force(
                                StatusLevel::Warn,
                                format!("{selected} 已打开但不在系统列表中"),
                            );
                        } else {
                            self.serial.selected_port = None;
                            self.set_status_force(
                                StatusLevel::Warn,
                                format!("{selected} 已拔出或不可用"),
                            );
                        }
                        if self.serial.auto_reconnect {
                            self.serial.pending_reconnect = Some(PendingReconnect {
                                port_name: selected.clone(),
                                config: snapshot,
                                attempts: 0,
                                next_try_at: 0.0,
                            });
                        }
                    }
                }

                // 自动重连：使用完整配置快照，带 backoff 和最大尝试次数
                if self.serial.auto_reconnect {
                    let pending = self.serial.pending_reconnect.clone();
                    if let Some(mut pending) = pending
                        && new_names.contains(&pending.port_name)
                    {
                        let now = now_timestamp_ms() as f64 / 1000.0;
                        if now < pending.next_try_at {
                            // cooldown not expired, keep waiting
                        } else if pending.attempts >= 10 {
                            self.set_status(
                                StatusLevel::Error,
                                format!(
                                    "自动重连 {} 失败，已达最大尝试次数，放弃",
                                    pending.port_name
                                ),
                            );
                            self.serial.pending_reconnect = None;
                        } else {
                            pending.attempts += 1;
                            // 使用 saturating 避免 attempts >= 64 时 2u64.pow 溢出/ panic
                            let backoff_ms = if pending.attempts >= 64 {
                                30_000
                            } else {
                                (1u64 << pending.attempts.saturating_sub(1))
                                    .saturating_mul(100)
                                    .min(30_000)
                            };
                            pending.next_try_at = now + backoff_ms as f64 / 1000.0;

                            match self.transport.open_serial(pending.config.clone()) {
                                Ok(()) => {
                                    self.serial.selected_port = Some(pending.port_name.clone());
                                    self.serial.pending_reconnect = None;
                                    self.set_status_force(
                                        StatusLevel::Info,
                                        format!("已自动重连 {}", pending.port_name),
                                    );
                                }
                                Err(e) => {
                                    self.set_status_force(
                                        StatusLevel::Warn,
                                        format!(
                                            "自动重连 {} 失败 (第 {} 次): {e}",
                                            pending.port_name, pending.attempts
                                        ),
                                    );
                                    self.serial.pending_reconnect = Some(pending);
                                }
                            }
                        }
                    }
                    // else: port not yet reappeared, keep waiting
                }

                if show_status {
                    self.set_status_force(
                        StatusLevel::Info,
                        format!("{} 个串口", self.serial.ports.len()),
                    );
                    return;
                }

                if !added_ports.is_empty() {
                    self.set_status_force(
                        StatusLevel::Info,
                        format!("发现串口 {}", added_ports.join(", ")),
                    );
                } else if !removed_ports.is_empty() {
                    self.set_status_force(
                        StatusLevel::Info,
                        format!("移除串口 {}", removed_ports.join(", ")),
                    );
                } else if self.serial.selected_port != old_selected {
                    self.set_status_force(StatusLevel::Info, "请选择串口");
                }
            }
            Err(error) => {
                self.set_status(StatusLevel::Error, error.to_string());
            }
        }
    }

    pub(crate) fn open_selected_port(&mut self) {
        match self.open_selected_port_result() {
            Ok(()) => {
                let p = self.serial.selected_port.as_deref().unwrap_or("?");
                self.set_status_force(StatusLevel::Info, format!("{p} 已连接"));
                self.open_bottom_panel();
            }
            Err(e) => {
                self.set_status_force(StatusLevel::Error, e);
            }
        }
    }

    fn open_selected_port_result(&mut self) -> Result<(), String> {
        self.refresh_ports_silent();
        let Some(p) = self.serial.selected_port.clone() else {
            return Err("请选择串口".to_owned());
        };
        if !self.serial.ports.iter().any(|port| port.port_name == p) {
            return Err(format!("{p} 不存在"));
        }
        let baud_rate = self
            .serial
            .baud_rate
            .trim()
            .parse::<u32>()
            .map_err(|_| "波特率格式错误".to_owned())?;
        if baud_rate == 0 {
            return Err("波特率格式错误".to_owned());
        }
        let cfg = SerialConfig {
            port_name: p,
            baud_rate,
            data_bits: parse_data_bits(&self.serial.data_bits),
            stop_bits: parse_stop_bits(&self.serial.stop_bits),
            parity: parse_parity(&self.serial.parity),
        };
        self.transport.open_serial(cfg).map_err(|e| e.to_string())
    }

    /// 真正重连：先关闭端口并等待 worker 退出，再用当前配置重新打开。
    pub(crate) fn reconnect_selected_port(&mut self) {
        let Some(p) = self.serial.selected_port.clone() else {
            self.set_status_force(StatusLevel::Warn, "请选择串口");
            return;
        };

        let selected_exists = self.serial.ports.iter().any(|port| port.port_name == p);
        if !selected_exists {
            self.set_status_force(StatusLevel::Error, format!("串口 {p} 不存在"));
            return;
        }

        // 先关闭，阻塞等待 worker 退出
        if let Err(e) = self
            .transport
            .close_port_blocking(&p, Duration::from_millis(3000))
        {
            self.set_status_force(StatusLevel::Error, format!("关闭 {p} 失败：{e}"));
            return;
        }

        // 重新打开
        match self.open_selected_port_result() {
            Ok(()) => self.set_status_force(StatusLevel::Info, format!("{p} 已重新连接")),
            Err(e) => self.set_status_force(StatusLevel::Error, format!("重连 {p} 失败：{e}")),
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
}
