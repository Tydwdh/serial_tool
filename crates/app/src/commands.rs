use crate::app::WorkbenchApp;
use crate::config::resolve_recorder_path;
use crate::reconnect::{RECONNECT_MAX_ATTEMPTS, ReconnectOutcome};
use crate::state::PendingPortOpenNotice;
use crate::state::StatusLevel;
use std::collections::BTreeSet;
use std::path::PathBuf;
use tool_core::now_timestamp_ms;
use tool_panels::{LogExportCursor, TerminalExportCursor, TerminalExportFormat};
use tool_platform::storage::FileHandle;

pub(crate) enum NativeExportJob {
    Terminal(TerminalExportCursor),
    Logs(LogExportCursor),
}

impl NativeExportJob {
    /// 通知来源：终端与日志各自独立，避免互相顶掉。
    fn source(&self) -> &'static str {
        match self {
            Self::Terminal(_) => "terminal-export",
            Self::Logs(_) => "log-export",
        }
    }

    /// Workbench 后台任务 kind。
    fn spawn_kind(&self) -> &'static str {
        match self {
            Self::Terminal(_) => "export_terminal",
            Self::Logs(_) => "export_log",
        }
    }
}

/// 导出格式对应的（菜单显示名, 文件后缀）。
fn export_format_labels(format: TerminalExportFormat) -> (&'static str, &'static str) {
    match format {
        TerminalExportFormat::Txt => ("TXT", "txt"),
        TerminalExportFormat::Csv => ("CSV", "csv"),
        TerminalExportFormat::Json => ("JSON", "json"),
    }
}

/// 弹出"另存为"对话框取导出目标路径；用户取消时返回 `None`。
///
/// 对话框标题为「{title_prefix} {格式名}」。用户手动输入其它后缀时仍以菜单中
/// 选定的格式为准，避免内容与扩展名不一致。
fn pick_export_path(
    title_prefix: &str,
    name_prefix: &str,
    format: TerminalExportFormat,
) -> Option<PathBuf> {
    let (format_name, extension) = export_format_labels(format);
    let default_name = format!("{name_prefix}-{}.{}", now_timestamp_ms(), extension);
    let mut path = rfd::FileDialog::new()
        .set_title(format!("{title_prefix} {format_name}"))
        .add_filter(format_name, &[extension])
        .set_file_name(default_name)
        .save_file()?;
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
    {
        path.set_extension(extension);
    }
    Some(path)
}

pub(crate) struct NativeExportState {
    pub(crate) format: TerminalExportFormat,
    pub(crate) path: PathBuf,
    pub(crate) job: NativeExportJob,
    pub(crate) content: String,
    pub(crate) exported_rows: usize,
}

impl WorkbenchApp {
    pub(crate) fn export_terminal_data(&mut self, format: TerminalExportFormat) {
        if self.native_export.is_some() {
            self.notifications
                .push("terminal-export", StatusLevel::Warn, "已有导出任务正在运行");
            return;
        }
        let Some(path) = pick_export_path("导出接收数据为", "serial-export", format) else {
            return;
        };
        let job = NativeExportJob::Terminal(self.terminal_panel.begin_export_cursor());
        self.begin_native_export(format, path, job);
    }

    pub(crate) fn export_log_data(&mut self, format: TerminalExportFormat) {
        if self.native_export.is_some() {
            self.notifications
                .push("log-export", StatusLevel::Warn, "已有导出任务正在运行");
            return;
        }
        let Some(path) = pick_export_path("导出日志为", "log-export", format) else {
            return;
        };
        let job = NativeExportJob::Logs(self.bottom_log_panel.begin_export_cursor());
        self.begin_native_export(format, path, job);
    }

    /// 登记导出任务并提示"正在准备导出"，分帧读取与写入由 `tick_native_export` 推进。
    fn begin_native_export(
        &mut self,
        format: TerminalExportFormat,
        path: PathBuf,
        job: NativeExportJob,
    ) {
        self.notifications.push(
            job.source(),
            StatusLevel::Info,
            format!("正在准备导出：{}", path.display()),
        );
        self.native_export = Some(NativeExportState {
            format,
            path,
            job,
            content: String::new(),
            exported_rows: 0,
        });
    }

    pub(crate) fn tick_native_export(&mut self, ctx: &eframe::egui::Context) {
        const EXPORT_SCAN_BATCH: usize = 512;
        let Some(state) = self.native_export.as_mut() else {
            return;
        };
        let format = state.format;
        let notification_source = state.job.source();
        let (chunk, done, exported) = match &mut state.job {
            NativeExportJob::Terminal(cursor) => {
                self.terminal_panel
                    .export_cursor_chunk(cursor, format, EXPORT_SCAN_BATCH)
            }
            NativeExportJob::Logs(cursor) => {
                self.bottom_log_panel
                    .export_cursor_chunk(cursor, format, EXPORT_SCAN_BATCH)
            }
        };
        state.content.push_str(&chunk);
        state.exported_rows += exported;
        if !done {
            self.notifications.push(
                notification_source,
                StatusLevel::Info,
                format!("正在准备导出：已处理 {} 条", state.exported_rows),
            );
            ctx.request_repaint();
            return;
        }

        let Some(state) = self.native_export.take() else {
            return;
        };
        let notification_source = state.job.source();
        let spawn_kind = state.job.spawn_kind();
        let format_name = export_format_labels(state.format).1.to_owned();
        let path = state.path.clone();
        let outcome = self.workbench.spawn_file_export(
            spawn_kind,
            format_name,
            FileHandle::from_native_path(path.clone()),
            move || Ok(state.content),
        );
        match outcome {
            Ok(tool_application::CommandOutcome::Pending { .. }) => {
                self.notifications.push(
                    notification_source,
                    StatusLevel::Info,
                    format!("正在写入导出文件：{}", path.display()),
                );
            }
            Ok(tool_application::CommandOutcome::Done) => {}
            Err(error) => {
                self.notifications
                    .push(notification_source, StatusLevel::Error, error.to_string())
            }
        }
    }

    /// 发布一条通知到状态栏。source 相同的旧通知会被替换（避免刷屏）。
    pub(crate) fn set_status(&mut self, level: StatusLevel, text: impl Into<String>) {
        self.notifications.push("general", level, text);
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
        new_ports.sort_by_key(|port| tool_transport::natural_sort_key(&port.port_name));
        let new_names: BTreeSet<String> = new_ports
            .iter()
            .map(|port| port.port_name.clone())
            .collect();
        let added_ports: Vec<String> = new_names.difference(&old_names).cloned().collect();
        let removed_ports: Vec<String> = old_names.difference(&new_names).cloned().collect();

        self.serial.ports = new_ports;
        let port_items: Vec<tool_panels::PortItem> = self
            .serial
            .ports
            .iter()
            .map(|port| tool_panels::PortItem {
                port_name: port.port_name.clone(),
            })
            .collect();
        self.dynamic_panels.set_ports(&port_items);

        let selected_still_exists = self
            .serial
            .selected_port
            .as_ref()
            .is_some_and(|selected| new_names.contains(selected));

        // 端口从系统列表里消失：**任何**已打开却没了的端口都算失联，不只是选中的那个。
        // 从前只检查 `selected_port`，于是"拔了另一个已打开的端口"既不报警也不重连。
        if !selected_still_exists && let Some(selected) = self.serial.selected_port.clone() {
            if self.workbench.transport_status(&selected).open {
                self.set_status_force(
                    StatusLevel::Warn,
                    format!("{selected} 已打开但不在系统列表中"),
                );
            } else {
                self.serial.selected_port = None;
                self.set_status_force(StatusLevel::Warn, format!("{selected} 已拔出或不可用"));
            }
        }
        let missing: Vec<String> = self
            .workbench
            .open_port_names()
            .into_iter()
            .filter(|port| !new_names.contains(port))
            .filter(|port| {
                !self
                    .serial
                    .network_ports
                    .iter()
                    .any(|net| net.display_name() == *port)
            })
            .collect();
        let now = tool_core::now_timestamp_ms() as f64 / 1000.0;
        for port in missing {
            self.schedule_auto_reconnect(&port, now, format!("{port} 不在系统端口列表中了"));
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
        // 自动重连中：取消重连（与顶栏 chip 的 × 走同一条路径）
        if self.serial.reconnects.entry(name).is_some() {
            self.cancel_auto_reconnect(name, "已取消 ");
            return;
        }

        if self.workbench.transport_status(name).open {
            // 已打开：关闭（用户自己的动作 —— 记住它，别再自动重连回来）
            self.serial.manual_disconnects.insert(name.to_owned());
            self.serial.reconnects.cancel(name);
            self.cancel_pending_port_open_notice(name);
            match self
                .workbench
                .dispatch(tool_application::AppCommand::Disconnect {
                    port: tool_platform::PortId::new(name),
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
                    port: tool_platform::PortId::new(name),
                });
            self.set_status_force(StatusLevel::Info, format!("已取消 {name} 的连接"));
            return;
        }

        // 未打开：切换 selected_port（恢复该端口的配置档案）后打开
        self.serial.manual_disconnects.remove(name);
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
                // `defer_port_open_notice` 要 `&mut self`，这里必须先持有端口名的副本。
                let port = self
                    .serial
                    .selected_port
                    .clone()
                    .unwrap_or_else(|| "?".to_owned());
                self.defer_port_open_notice(&port, format!("{port} 已连接"));
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
            Self::parsed_baud_rate(&self.serial)?;
        }

        let settings = Self::serial_settings_from_state(&self.serial)?;
        self.note_manual_connect(&p);
        self.workbench
            .dispatch(tool_application::AppCommand::SetSerialSettings { settings })
            .map_err(|error| error.to_string())?;
        self.workbench
            .dispatch(tool_application::AppCommand::Connect {
                port: tool_platform::PortId::new(p),
                settings,
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// 用户主动连接某个端口：清掉"手动关过"的记忆并撤掉它的待重连条目。
    ///
    /// 从前这个集合只在端口圆点的"未打开→打开"分支里清，于是「手动关 COM3 → 从顶栏/设备面板
    /// 再连 COM3 → 拔线」会被判成"用户手动断开的"，自动重连再也挂不上。所有连接入口都走这里，
    /// 记忆就与"最后一次动作是谁做的"对齐。
    pub(crate) fn note_manual_connect(&mut self, port: &str) {
        self.serial.manual_disconnects.remove(port);
        self.serial.reconnects.cancel(port);
    }

    /// 挂起一个端口的自动重连（拔线/失联的**唯一**入口）。
    ///
    /// `reason` 只用于写一句给用户看的话：挂起本身是同一个动作。自动重连关着、或这个端口
    /// 是用户手动关掉的，都不挂 —— 后者由 `manual_disconnects` 记住，任何一次显式连接会清掉它。
    pub(crate) fn schedule_auto_reconnect(
        &mut self,
        port: &str,
        now: f64,
        reason: impl Into<String>,
    ) {
        if !self.serial.auto_reconnect || self.serial.manual_disconnects.contains(port) {
            return;
        }
        if !self.serial.reconnects.schedule(port, now) {
            return;
        }
        let reason = reason.into();
        self.set_status_force(StatusLevel::Warn, format!("{reason}，正在自动重连"));
        self.notifications.push(
            "serial-reconnect",
            StatusLevel::Warn,
            format!("{port} 已断开，正在自动重连"),
        );
    }

    /// 取消一个端口的自动重连：停止等待、撤掉在飞的任务，并记住"这是用户自己不要的"。
    ///
    /// 三条 UI 入口（顶栏 chip 的 ×、端口行的 ⟳、关掉自动重连勾选）都走这里，
    /// 从前它们各清各的状态：chip 只清 pending、端口行只派发 `CancelReconnect`，
    /// 于是"取消"要么对在飞任务无效，要么下一帧又被挂回来。
    pub(crate) fn cancel_auto_reconnect(&mut self, port: &str, note: &str) {
        self.serial.reconnects.cancel(port);
        self.serial.manual_disconnects.insert(port.to_owned());
        self.cancel_pending_port_open_notice(port);
        let _ = self
            .workbench
            .dispatch(tool_application::AppCommand::CancelReconnect {
                port: tool_platform::PortId::new(port),
            });
        self.set_status_force(StatusLevel::Info, format!("{note}{port}"));
    }

    /// 自动重连的每帧驱动：先收断开事件，再派发到点的尝试，最后回收在飞任务的结果。
    pub(crate) fn tick_reconnects(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        self.ingest_serial_disconnects(now);
        self.dispatch_due_reconnects(now);
        self.reap_reconnect_tasks(now);
    }

    /// 收 `transport.serial.closed`：只有 `device_lost` 才需要反应（`user_close` 是用户自己的动作）。
    fn ingest_serial_disconnects(&mut self, now: f64) {
        for closed in self.ui_events.drain_serial_disconnects(32) {
            if closed.reason != tool_transport::CloseReason::DeviceLost {
                continue;
            }
            let port = closed.port.clone();
            // 选中的端口若已从系统列表消失，交给 `sync_ports_from_workbench` 的既有分支
            // 去清 selected（它还会用中文说清"已拔出/不在列表里"），这里只负责挂重连。
            let vanished = !self.serial.ports.iter().any(|item| item.port_name == port);
            let note = if vanished {
                format!("{port} 已断开（设备或链路已失效）")
            } else {
                format!("{port} 已断开（设备可能已被拔出），端口仍在系统列表中")
            };
            self.schedule_auto_reconnect(&port, now, note);
        }
        // 关掉自动重连勾选时，表里还挂着的等待一律清掉。
        if !self.serial.auto_reconnect && !self.serial.reconnects.is_empty() {
            self.serial.reconnects.clear();
        }
    }

    fn dispatch_due_reconnects(&mut self, now: f64) {
        for port in self.serial.reconnects.take_due(now) {
            let settings = match Self::serial_settings_from_state(&self.serial) {
                Ok(settings) => settings,
                Err(error) => {
                    self.set_status_force(StatusLevel::Warn, error);
                    return;
                }
            };
            if let Err(error) = self
                .workbench
                .dispatch(tool_application::AppCommand::SetSerialSettings { settings })
            {
                self.set_status_force(StatusLevel::Warn, error.to_string());
                return;
            }
            let attempt = self
                .serial
                .reconnects
                .entry(&port)
                .map(|entry| entry.attempts + 1)
                .unwrap_or(1);
            // 用 `Connect` 而不是 `Reconnect`：重连表里的端口要么已死、要么已关，
            // `Reconnect` 在"同名句柄还活着"时会直接返回 Ok（连接其实已经坏了）。
            match self
                .workbench
                .dispatch(tool_application::AppCommand::Connect {
                    port: tool_platform::PortId::new(port.clone()),
                    settings,
                }) {
                Ok(tool_application::CommandOutcome::Pending { task_id, .. }) => {
                    self.serial.reconnects.mark_dispatched(&port, task_id);
                    self.set_status_force(
                        StatusLevel::Info,
                        format!("正在自动重连 {port}（第 {attempt}/{RECONNECT_MAX_ATTEMPTS} 次）"),
                    );
                }
                Ok(tool_application::CommandOutcome::Done) => {
                    self.serial.reconnects.record_success(&port);
                    self.set_status_force(StatusLevel::Info, format!("已自动重连 {port}"));
                }
                Err(error) => {
                    self.report_reconnect_attempt(&port, now, &error.to_string());
                }
            }
        }
    }

    /// 回收在飞的重连任务：完成 → 摘掉条目；失败/被取消 → 记一次并退避。
    fn reap_reconnect_tasks(&mut self, now: f64) {
        let snapshots = self.workbench.task_snapshots();
        let inflight: Vec<(String, tool_application::TaskId)> = self
            .serial
            .reconnects
            .in_flight()
            .map(|(port, task_id)| (port.to_owned(), task_id))
            .collect();
        for (port, task_id) in inflight {
            // 端口已经打开即成功：任务快照有保留上限（256 条），丢掉的终态不该把重连卡住，
            // 而 transport 的状态才是权威判据。
            if self.workbench.transport_status(&port).open {
                self.serial.reconnects.record_success(&port);
                self.set_status_force(StatusLevel::Info, format!("已自动重连 {port}"));
                continue;
            }
            let Some(snapshot) = snapshots.iter().find(|snapshot| snapshot.id == task_id) else {
                continue;
            };
            match snapshot.state {
                tool_application::TaskState::Completed => {
                    self.serial.reconnects.record_success(&port);
                    self.set_status_force(StatusLevel::Info, format!("已自动重连 {port}"));
                }
                tool_application::TaskState::Failed => {
                    let message = snapshot.message.clone();
                    self.report_reconnect_attempt(&port, now, &message);
                }
                // 被取消也算一次尝试：否则"取消 → 立刻重试"会变成热循环。
                tool_application::TaskState::Cancelled => {
                    self.report_reconnect_attempt(&port, now, "重连任务被取消");
                }
                tool_application::TaskState::Pending | tool_application::TaskState::Running => {}
            }
        }
    }

    /// 一次重连尝试没成：按 tracker 的判定写回退避，或宣布放弃。
    fn report_reconnect_attempt(&mut self, port: &str, now: f64, error: &str) {
        match self.serial.reconnects.record_attempt_failed(port, now) {
            Some(ReconnectOutcome::Retry { attempts, delay }) => {
                self.set_status_force(
                    StatusLevel::Warn,
                    format!("{port} 自动重连失败（第 {attempts}/{RECONNECT_MAX_ATTEMPTS} 次）：{error}，{delay:.1}s 后重试"),
                );
            }
            Some(ReconnectOutcome::GaveUp { attempts }) => {
                self.set_status_force(
                    StatusLevel::Error,
                    format!("{port} 自动重连失败（已尝试 {attempts} 次）：{error}"),
                );
                self.notifications.push(
                    "serial-reconnect",
                    StatusLevel::Error,
                    format!("{port} 自动重连失败（已尝试 {attempts} 次）"),
                );
            }
            None => {}
        }
    }

    /// 提交统一后台重连任务；关闭和重新打开均在线程中完成。
    pub(crate) fn reconnect_selected_port(&mut self) {
        let Some(p) = self.serial.selected_port.clone() else {
            self.set_status_force(StatusLevel::Warn, "请选择串口");
            return;
        };

        if !self.serial.ports.iter().any(|port| port.port_name == p) {
            self.set_status_force(StatusLevel::Error, format!("串口 {p} 不存在"));
            return;
        }

        let settings = match Self::serial_settings_from_state(&self.serial) {
            Ok(settings) => settings,
            Err(error) => {
                self.set_status_force(StatusLevel::Error, error);
                return;
            }
        };
        if let Err(error) = self
            .workbench
            .dispatch(tool_application::AppCommand::SetSerialSettings { settings })
        {
            self.set_status_force(StatusLevel::Error, error.to_string());
            return;
        }
        self.note_manual_connect(&p);
        match self
            .workbench
            .dispatch(tool_application::AppCommand::Reconnect {
                port: tool_platform::PortId::new(p.clone()),
            }) {
            Ok(tool_application::CommandOutcome::Pending { .. }) => {
                self.defer_port_open_notice(&p, format!("{p} 已重新连接"));
                self.set_status_force(StatusLevel::Info, format!("正在重连 {p}..."));
            }
            Ok(tool_application::CommandOutcome::Done) => {}
            Err(error) => self.set_status_force(StatusLevel::Error, error.to_string()),
        }
    }

    /// 波特率必须是正整数字符串；无效时统一给出设置面板同款提示。
    fn parsed_baud_rate(serial: &crate::state::SerialUiState) -> Result<u32, String> {
        let baud_rate = serial
            .baud_rate
            .trim()
            .parse::<u32>()
            .map_err(|_| "波特率格式错误".to_owned())?;
        if baud_rate == 0 {
            return Err("波特率格式错误".to_owned());
        }
        Ok(baud_rate)
    }

    fn serial_settings_from_state(
        serial: &crate::state::SerialUiState,
    ) -> Result<tool_platform::SerialSettings, String> {
        let baud_rate = Self::parsed_baud_rate(serial)?;
        Ok(tool_platform::SerialSettings {
            baud_rate,
            data_bits: serial.data_bits.parse().unwrap_or(8),
            stop_bits: serial.stop_bits.parse().unwrap_or(1),
            parity: match serial.parity.as_str() {
                "odd" => tool_platform::SerialParity::Odd,
                "even" => tool_platform::SerialParity::Even,
                _ => tool_platform::SerialParity::None,
            },
        })
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
        let recording = self.workbench.query_recording();
        let is_recording = recording.stats.running || recording.stats.stopping;
        let command = if is_recording {
            tool_application::AppCommand::StopRecording
        } else {
            let recorder_path = resolve_recorder_path(std::path::Path::new(&self.recorder_path));
            tool_application::AppCommand::StartRecording {
                file: FileHandle::from_native_path(recorder_path),
                mode: recording.mode,
            }
        };
        match self.workbench.dispatch(command) {
            Ok(tool_application::CommandOutcome::Pending { message, .. }) => {
                self.set_status_force(StatusLevel::Info, message);
            }
            Ok(tool_application::CommandOutcome::Done) => {
                if is_recording {
                    self.set_status_force(StatusLevel::Info, "正在停止录制...");
                } else {
                    self.set_status_force(StatusLevel::Info, "录制中");
                }
            }
            Err(error) => self.set_status_force(StatusLevel::Error, error.to_string()),
        }
    }
}
