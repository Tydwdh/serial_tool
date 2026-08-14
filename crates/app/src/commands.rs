use crate::app::WorkbenchApp;
use crate::state::PendingReconnect;
use crate::state::StatusLevel;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;
use tool_core::now_timestamp_ms;
use tool_panels::TerminalExportFormat;
use tool_transport::{SerialConfig, parse_data_bits, parse_parity, parse_stop_bits};

impl WorkbenchApp {
    pub(crate) fn export_terminal_data(&mut self, format: TerminalExportFormat) {
        let (format_name, extension) = match format {
            TerminalExportFormat::Txt => ("TXT", "txt"),
            TerminalExportFormat::Csv => ("CSV", "csv"),
            TerminalExportFormat::Json => ("JSON", "json"),
        };
        let default_name = format!("serial-export-{}.{}", now_timestamp_ms(), extension);
        let Some(mut path) = rfd::FileDialog::new()
            .set_title(format!("导出接收数据为 {format_name}"))
            .add_filter(format_name, &[extension])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };

        // 用户手动输入其它后缀时仍以菜单中选定的格式为准，避免内容与扩展名不一致。
        if !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            path.set_extension(extension);
        }

        let result = match format {
            TerminalExportFormat::Txt => {
                std::fs::write(&path, self.terminal_panel.export_visible_text())
            }
            TerminalExportFormat::Csv => {
                let content = self.terminal_panel.export_visible_csv();
                // UTF-8 BOM 让 Windows Excel 直接打开时能正确识别中文。
                write_utf8_csv(&path, &content)
            }
            TerminalExportFormat::Json => {
                std::fs::write(&path, self.terminal_panel.export_visible_json())
            }
        };

        match result {
            Ok(()) => self.notifications.push(
                "terminal-export",
                StatusLevel::Info,
                format!("已导出 {format_name}：{}", path.display()),
            ),
            Err(error) => self.notifications.push(
                "terminal-export",
                StatusLevel::Error,
                format!("导出失败：{error}"),
            ),
        }
    }

    pub(crate) fn export_log_data(&mut self, format: TerminalExportFormat) {
        let (format_name, extension) = match format {
            TerminalExportFormat::Txt => ("TXT", "txt"),
            TerminalExportFormat::Csv => ("CSV", "csv"),
            TerminalExportFormat::Json => ("JSON", "json"),
        };
        let default_name = format!("log-export-{}.{}", now_timestamp_ms(), extension);
        let Some(mut path) = rfd::FileDialog::new()
            .set_title(format!("导出日志为 {format_name}"))
            .add_filter(format_name, &[extension])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        if !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            path.set_extension(extension);
        }

        let result = match format {
            TerminalExportFormat::Txt => {
                std::fs::write(&path, self.bottom_log_panel.export_visible_text())
            }
            TerminalExportFormat::Csv => {
                write_utf8_csv(&path, &self.bottom_log_panel.export_visible_csv())
            }
            TerminalExportFormat::Json => {
                std::fs::write(&path, self.bottom_log_panel.export_visible_json())
            }
        };
        match result {
            Ok(()) => self.notifications.push(
                "log-export",
                StatusLevel::Info,
                format!("已导出 {format_name}：{}", path.display()),
            ),
            Err(error) => self.notifications.push(
                "log-export",
                StatusLevel::Error,
                format!("导出失败：{error}"),
            ),
        }
    }

    /// 发布一条通知到状态栏。source 相同的旧通知会被替换（避免刷屏）。
    pub(crate) fn set_status(&mut self, level: StatusLevel, text: impl Into<String>) {
        self.notifications.push("general", level, text);
    }

    /// 来自特定 source 的通知（如 "terminal", "log", "replay" 等）。
    /// 同 source 的新消息替换旧消息。
    #[allow(dead_code)]
    pub(crate) fn set_status_source(
        &mut self,
        source: &str,
        level: StatusLevel,
        text: impl Into<String>,
    ) {
        self.notifications.push(source, level, text);
    }

    /// 用户主动操作：总是更新状态（不被旧错误阻塞）。
    /// 等价于 set_status，因为通知队列不会阻塞。
    pub(crate) fn set_status_force(&mut self, level: StatusLevel, text: impl Into<String>) {
        self.notifications.push("general", level, text);
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
            self.set_status(
                StatusLevel::Info,
                format!(
                    "已恢复 {new_port} 的串口配置: {} {}{}{}",
                    profile.baud_rate, profile.data_bits, profile.parity, profile.stop_bits,
                ),
            );
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
            Ok(mut new_ports) => {
                // 网络模拟串口（WebSocket + JSON-RPC gcode 桥）作为固定端口并入列表：
                // 与系统串口一起排序、分组、别名、开关，表现完全一致。
                new_ports.extend(self.serial.network_ports.iter().map(|net| {
                    tool_transport::SerialPortDescriptor {
                        port_name: net.display_name(),
                        port_type: tool_transport::PortType::Network,
                    }
                }));
                new_ports.sort_by_key(|port| tool_transport::natural_sort_key(&port.port_name));

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

    /// 切换选中串口的打开/关闭状态（快捷键用）。
    pub(crate) fn toggle_selected_port(&mut self) {
        let Some(port) = self.serial.selected_port.clone() else {
            self.set_status_force(StatusLevel::Warn, "请选择串口");
            return;
        };
        self.toggle_port_by_name(&port);
    }

    /// 点击端口列表圆点切换开/关（支持任意端口，不限当前选中）。
    ///
    /// 三种状态：
    /// - 已打开（●）：关闭该端口
    /// - 自动重连中（⟳）：取消重连
    /// - 未打开（○）：切换 selected_port 到该端口（恢复其配置档案）并打开
    pub(crate) fn toggle_port_by_name(&mut self, name: &str) {
        // 自动重连中：取消重连
        if let Some(pending) = &self.serial.pending_reconnect
            && pending.port_name == name
        {
            self.serial.pending_reconnect = None;
            self.set_status_force(StatusLevel::Info, format!("已取消 {name} 的自动重连"));
            return;
        }

        if self.transport.status_port(name).open {
            // 已打开：关闭
            self.transport.close_port(name);
            self.set_status_force(StatusLevel::Info, format!("{name} 已断开"));
            return;
        }

        // 未打开：切换 selected_port（恢复该端口的配置档案）后打开
        let old = self.serial.selected_port.clone();
        if old.as_deref() != Some(name) {
            self.switch_port_selection(old.as_deref(), name);
        }
        match self.open_selected_port_result() {
            Ok(()) => self.set_status_force(StatusLevel::Info, format!("{name} 已连接")),
            Err(e) => self.set_status_force(StatusLevel::Error, e),
        }
    }

    pub(crate) fn open_selected_port(&mut self) {
        match self.open_selected_port_result() {
            Ok(()) => {
                let p = self.serial.selected_port.as_deref().unwrap_or("?");
                self.set_status_force(StatusLevel::Info, format!("{p} 已连接"));
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
        // 网络模拟串口：走 WebSocket + JSON-RPC gcode 桥
        if let Some(net) = self
            .serial
            .network_ports
            .iter()
            .find(|net| net.display_name() == p)
        {
            self.transport
                .open_network_serial(net.clone())
                .map_err(|e| e.to_string())?;
            return Ok(());
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

fn write_utf8_csv(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    let file = std::fs::File::create(path)?;
    let mut writer = std::io::BufWriter::new(file);
    writer.write_all(b"\xEF\xBB\xBF")?;
    writer.write_all(content.as_bytes())?;
    writer.flush()
}
