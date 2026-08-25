use crate::app::WorkbenchApp;
use crate::config::resolve_recorder_path;
use crate::state::PendingPortOpenNotice;
use crate::state::PendingReconnect;
use crate::state::StatusLevel;
use std::collections::BTreeSet;
use tool_application::api::core::now_timestamp_ms;
use tool_panels::TerminalExportFormat;

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

        let format_name = format_name.to_ascii_lowercase();
        let job = self.terminal_panel.export_job();
        let task_format = format;
        let outcome = self.workbench.spawn_file_export(
            "export_terminal",
            format_name.clone(),
            path.clone(),
            move || Ok(job.render(task_format)),
        );
        match outcome {
            tool_application::CommandOutcome::Pending { .. } => self.notifications.push(
                "terminal-export",
                StatusLevel::Info,
                format!("正在导出 {format_name}：{}", path.display()),
            ),
            tool_application::CommandOutcome::Done => {}
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

        let format_name = format_name.to_ascii_lowercase();
        let job = self.bottom_log_panel.export_job();
        let outcome = self.workbench.spawn_file_export(
            "export_log",
            format_name.clone(),
            path.clone(),
            move || Ok(job.render(format)),
        );
        match outcome {
            tool_application::CommandOutcome::Pending { .. } => self.notifications.push(
                "log-export",
                StatusLevel::Info,
                format!("正在导出 {format_name}：{}", path.display()),
            ),
            tool_application::CommandOutcome::Done => {}
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

    // ── 内置命令 handler（由 CommandRegistry 注册）──

    /// `$Send`：目标端口打开且输入非空时发送。
    pub(crate) fn cmd_send_if_ready(&mut self) {
        if self.send_target_port_open() && !self.send.input.trim().is_empty() {
            self.do_send();
        }
    }

    /// `$ToggleRightDock`：切换右侧边栏并持久化。
    pub(crate) fn cmd_toggle_right_dock(&mut self) {
        let visible = self.panels.right_visible();
        self.panels.set_right_visible(!visible);
        if let Err(e) = self.save_config() {
            log::warn!("save_config failed: {e}")
        };
    }

    /// `$CommandPalette`：切换命令面板开关。
    pub(crate) fn cmd_toggle_command_palette(&mut self) {
        self.command_palette.open = !self.command_palette.open;
        self.command_palette.query.clear();
        self.command_palette.selected = None;
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
        if let Err(error) = self
            .workbench
            .dispatch(tool_application::AppCommand::RefreshPorts)
        {
            self.set_status(StatusLevel::Error, error.to_string());
        }
        self.sync_ports_from_workbench(show_status);
    }

    /// 将 Workbench 已经完成的后台刷新结果同步到 egui 状态。
    /// 这里仅做小型状态合并，不访问串口硬件。
    pub(crate) fn sync_ports_from_workbench(&mut self, show_status: bool) {
        // 初始/周期刷新尚未完成时，Workbench 的旧列表可能还是空的；不能因此
        // 把配置中的 selected_port 误判为拔出。
        if self.workbench.has_active_task_kind("refresh_ports")
            && self.workbench.query_transport().ports.is_empty()
        {
            return;
        }

        let old_names: BTreeSet<String> = self
            .serial
            .ports
            .iter()
            .map(|port| port.port_name.clone())
            .collect();

        let old_selected = self.serial.selected_port.clone();
        let mut new_ports = self.workbench.query_transport().ports;
        for network in &self.serial.network_ports {
            let name = network.display_name();
            if !new_ports.iter().any(|port| port.port_name == name) {
                new_ports.push(tool_application::query::PortView {
                    port_name: name,
                    port_type: tool_application::query::PortTypeView::Network,
                });
            }
        }
        new_ports.sort_by_key(|port| {
            tool_application::api::transport::natural_sort_key(&port.port_name)
        });
        let new_names: BTreeSet<String> = new_ports
            .iter()
            .map(|port| port.port_name.clone())
            .collect();
        let added_ports: Vec<String> = new_names.difference(&old_names).cloned().collect();
        let removed_ports: Vec<String> = old_names.difference(&new_names).cloned().collect();

        self.serial.ports = new_ports;
        self.dynamic_panels.set_ports(
            &self
                .serial
                .ports
                .iter()
                .map(|d| tool_panels::PortItem {
                    port_name: d.port_name.clone(),
                })
                .collect::<Vec<_>>(),
        );

        let selected_still_exists = self
            .serial
            .selected_port
            .as_ref()
            .is_some_and(|selected| new_names.contains(selected));

        if !selected_still_exists {
            let selected_val = self.serial.selected_port.clone();
            if let Some(ref selected) = selected_val {
                if self.workbench.transport_status(selected).open {
                    self.set_status_force(
                        StatusLevel::Warn,
                        format!("{selected} 已打开但不在系统列表中"),
                    );
                } else {
                    self.serial.selected_port = None;
                    self.set_status_force(StatusLevel::Warn, format!("{selected} 已拔出或不可用"));
                }
                if self.serial.auto_reconnect {
                    self.serial.pending_reconnect = Some(PendingReconnect {
                        port_name: selected.clone(),
                        attempts: 0,
                        next_try_at: 0.0,
                    });
                }
            }
        }

        // 自动重连现在只提交统一后台任务，UI tick 不再直接触碰 open_serial。
        if self.serial.auto_reconnect
            && let Some(pending) = self.serial.pending_reconnect.clone()
            && new_names.contains(&pending.port_name)
        {
            let name = pending.port_name.clone();
            self.workbench.set_serial_parameters(
                self.serial.baud_rate.clone(),
                self.serial.data_bits.clone(),
                self.serial.stop_bits.clone(),
                self.serial.parity.clone(),
            );
            match self
                .workbench
                .dispatch(tool_application::AppCommand::Reconnect {
                    port_name: name.clone(),
                }) {
                Ok(tool_application::CommandOutcome::Pending { .. }) => {
                    self.serial.pending_reconnect = None;
                    self.defer_port_open_notice(&name, format!("已自动重连 {name}"));
                    self.set_status_force(StatusLevel::Info, format!("正在自动重连 {name}..."));
                }
                Ok(tool_application::CommandOutcome::Done) => {
                    self.serial.pending_reconnect = None;
                }
                Err(error) => self.set_status_force(StatusLevel::Warn, error.to_string()),
            }
        }

        if show_status {
            self.set_status_force(
                StatusLevel::Info,
                format!("{} 个串口", self.serial.ports.len()),
            );
        } else if !added_ports.is_empty() {
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

        if self.workbench.transport_status(name).open {
            // 已打开：关闭
            self.cancel_pending_port_open_notice(name);
            match self
                .workbench
                .dispatch(tool_application::AppCommand::Disconnect {
                    port_name: name.to_owned(),
                }) {
                Ok(tool_application::CommandOutcome::Pending { .. }) => {
                    self.set_status_force(StatusLevel::Info, format!("正在断开 {name}..."));
                }
                Ok(tool_application::CommandOutcome::Done) => {}
                Err(error) => self.set_status_force(StatusLevel::Error, error.to_string()),
            }
            return;
        }

        // 网络端口连接中：取消连接（worker 正在异步 connect）
        if self.workbench.transport_status(name).connecting {
            self.cancel_pending_port_open_notice(name);
            let _ = self
                .workbench
                .dispatch(tool_application::AppCommand::Disconnect {
                    port_name: name.to_owned(),
                });
            self.set_status_force(StatusLevel::Info, format!("已取消 {name} 的连接"));
            return;
        }

        // 未打开：切换 selected_port（恢复该端口的配置档案）后打开
        let old = self.serial.selected_port.clone();
        if old.as_deref() != Some(name) {
            self.switch_port_selection(old.as_deref(), name);
        }
        match self.open_selected_port_result() {
            Ok(()) => self.defer_port_open_notice(name, format!("{name} 已连接")),
            Err(e) => self.set_status_force(StatusLevel::Error, e),
        }
    }

    pub(crate) fn open_selected_port(&mut self) {
        match self.open_selected_port_result() {
            Ok(()) => {
                let p = self
                    .serial
                    .selected_port
                    .clone()
                    .unwrap_or_else(|| "?".to_owned());
                self.defer_port_open_notice(&p, format!("{p} 已连接"));
            }
            Err(e) => {
                self.set_status_force(StatusLevel::Error, e);
            }
        }
    }

    fn open_selected_port_result(&mut self) -> Result<(), String> {
        let Some(p) = self.serial.selected_port.clone() else {
            return Err("请选择串口".to_owned());
        };
        if !self.serial.ports.iter().any(|port| port.port_name == p) {
            return Err(format!("{p} 不存在"));
        }
        if !self
            .serial
            .network_ports
            .iter()
            .any(|net| net.display_name() == p)
        {
            let baud_rate = self
                .serial
                .baud_rate
                .trim()
                .parse::<u32>()
                .map_err(|_| "波特率格式错误".to_owned())?;
            if baud_rate == 0 {
                return Err("波特率格式错误".to_owned());
            }
        }

        self.workbench.set_serial_parameters(
            self.serial.baud_rate.clone(),
            self.serial.data_bits.clone(),
            self.serial.stop_bits.clone(),
            self.serial.parity.clone(),
        );
        self.workbench
            .dispatch(tool_application::AppCommand::Connect {
                port_name: p,
                settings: self.workbench.serial_settings(),
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// 提交统一后台重连任务；关闭和重新打开均在线程中完成。
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

        self.workbench.set_serial_parameters(
            self.serial.baud_rate.clone(),
            self.serial.data_bits.clone(),
            self.serial.stop_bits.clone(),
            self.serial.parity.clone(),
        );
        match self
            .workbench
            .dispatch(tool_application::AppCommand::Reconnect {
                port_name: p.clone(),
            }) {
            Ok(tool_application::CommandOutcome::Pending { .. }) => {
                self.defer_port_open_notice(&p, format!("{p} 已重新连接"));
                self.set_status_force(StatusLevel::Info, format!("正在重连 {p}..."));
            }
            Ok(tool_application::CommandOutcome::Done) => {}
            Err(error) => self.set_status_force(StatusLevel::Error, error.to_string()),
        }
    }

    /// 打开请求已提交，但成功提示要等下一帧确认 transport 状态后再显示。
    pub(crate) fn defer_port_open_notice(&mut self, port_name: &str, success_message: String) {
        self.serial.pending_open_notice = Some(PendingPortOpenNotice {
            port_name: port_name.to_owned(),
            success_message,
            requested_at: 0.0,
        });
    }

    pub(crate) fn cancel_pending_port_open_notice(&mut self, port_name: &str) {
        if self
            .serial
            .pending_open_notice
            .as_ref()
            .is_some_and(|pending| pending.port_name == port_name)
        {
            self.serial.pending_open_notice = None;
        }
    }

    pub(crate) fn start_or_stop_recording(&mut self) {
        if self.workbench.recording_is_running() || self.workbench.recording_is_stopping() {
            self.workbench.stop_recording();
            self.set_status_force(StatusLevel::Info, "正在停止录制...");
        } else {
            let recorder_path = resolve_recorder_path(std::path::Path::new(&self.recorder_path));
            match self.workbench.start_recording(recorder_path) {
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
