use crate::app::{PendingReconnect, WorkbenchApp};
use crate::config::{PersistedConfig, config_path};
use crate::state::{BottomTab, MAX_SEND_HISTORY, StatusLevel};
use crate::ui::top_bar::{pdb, ppar, psb};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;
use tool_core::now_timestamp_ms;
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
    /// 切换串口时：保存旧端口设置到 profile，从 profile 恢复新端口设置。
    pub(crate) fn switch_port_selection(&mut self, new_port: &str) {
        // 保存旧端口配置
        if let Some(ref old) = self.selected_port {
            self.port_profiles.insert(
                old.clone(),
                crate::config::PortProfile {
                    baud_rate: self.baud_rate.clone(),
                    data_bits: self.data_bits.clone(),
                    stop_bits: self.stop_bits.clone(),
                    parity: self.parity.clone(),
                    timeout_ms: self.timeout_ms.clone(),
                },
            );
        }
        // 恢复新端口配置
        if let Some(profile) = self.port_profiles.get(new_port) {
            self.baud_rate = profile.baud_rate.clone();
            self.data_bits = profile.data_bits.clone();
            self.stop_bits = profile.stop_bits.clone();
            self.parity = profile.parity.clone();
            self.timeout_ms = profile.timeout_ms.clone();
        }
        self.selected_port = Some(new_port.to_owned());
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
                self.dynamic_panels.set_ports(&self.ports);

                let selected_still_exists = self
                    .selected_port
                    .as_ref()
                    .is_some_and(|selected| new_names.contains(selected));

                // 只在端口消失且 transport 也未打开时才清空选择
                if !selected_still_exists {
                    let selected_val = self.selected_port.clone();
                    if let Some(ref selected) = selected_val {
                        // 保存配置快照（在 reap_dead_ports 移除 handle 前）
                        let snapshot = SerialConfig {
                            port_name: selected.clone(),
                            baud_rate: self.baud_rate.parse().unwrap_or(115200),
                            data_bits: pdb(&self.data_bits),
                            stop_bits: psb(&self.stop_bits),
                            parity: ppar(&self.parity),
                            timeout_ms: self.timeout_ms.parse().unwrap_or(50),
                        };
                        if self.transport.status_port(selected).open {
                            self.set_status_force(
                                StatusLevel::Warn,
                                format!("{selected} 已打开但不在系统列表中"),
                            );
                        } else {
                            self.selected_port = None;
                            self.set_status_force(
                                StatusLevel::Warn,
                                format!("{selected} 已拔出或不可用"),
                            );
                        }
                        if self.auto_reconnect {
                            self.pending_reconnect = Some(PendingReconnect {
                                port_name: selected.clone(),
                                config: snapshot,
                                attempts: 0,
                                next_try_at: 0.0,
                            });
                        }
                    }
                }

                // 自动重连：使用完整配置快照，带 backoff 和最大尝试次数
                if self.auto_reconnect {
                    let pending = self.pending_reconnect.clone();
                    if let Some(mut pending) = pending {
                        if new_names.contains(&pending.port_name) {
                            let now = now_timestamp_ms() as f64 / 1000.0;
                            if now < pending.next_try_at {
                                // cooldown not expired, keep waiting
                            } else {
                                if pending.attempts >= 10 {
                                    self.set_status(
                                        StatusLevel::Error,
                                        format!(
                                            "自动重连 {} 失败，已达最大尝试次数，放弃",
                                            pending.port_name
                                        ),
                                    );
                                    self.pending_reconnect = None;
                                } else {
                                    pending.attempts += 1;
                                    let backoff_ms = (2u64.pow(pending.attempts) * 100).min(30_000);
                                    pending.next_try_at = now + backoff_ms as f64 / 1000.0;

                                    match self.transport.open_serial(pending.config.clone()) {
                                        Ok(()) => {
                                            self.selected_port = Some(pending.port_name.clone());
                                            self.pending_reconnect = None;
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
                                            self.pending_reconnect = Some(pending);
                                        }
                                    }
                                }
                            }
                        }
                        // else: port not yet reappeared, keep waiting
                    }
                }

                if show_status {
                    self.set_status_force(
                        StatusLevel::Info,
                        format!("{} 个串口", self.ports.len()),
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
                } else if self.selected_port != old_selected {
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
                let p = self.selected_port.as_deref().unwrap_or("?");
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
        let Some(p) = self.selected_port.clone() else {
            return Err("请选择串口".to_owned());
        };
        if !self.ports.iter().any(|port| port.port_name == p) {
            return Err(format!("{p} 不存在"));
        }
        let baud_rate = self
            .baud_rate
            .trim()
            .parse::<u32>()
            .map_err(|_| "波特率格式错误".to_owned())?;
        if baud_rate == 0 {
            return Err("波特率格式错误".to_owned());
        }
        let timeout_ms = self
            .timeout_ms
            .trim()
            .parse::<u64>()
            .map_err(|_| "超时格式错误".to_owned())?;
        if !(1..=1000).contains(&timeout_ms) {
            return Err("超时 1..=1000 ms".to_owned());
        }
        let cfg = SerialConfig {
            port_name: p,
            baud_rate,
            data_bits: pdb(&self.data_bits),
            stop_bits: psb(&self.stop_bits),
            parity: ppar(&self.parity),
            timeout_ms,
        };
        self.transport.open_serial(cfg).map_err(|e| e.to_string())
    }

    /// 真正重连：先关闭端口并等待 worker 退出，再用当前配置重新打开。
    pub(crate) fn reconnect_selected_port(&mut self) {
        let Some(p) = self.selected_port.clone() else {
            self.set_status_force(StatusLevel::Warn, "请选择串口");
            return;
        };

        let selected_exists = self.ports.iter().any(|port| port.port_name == p);
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
            terminal_popup_always_on_top: self.terminal_popup_always_on_top,
            send_popup_always_on_top: self.send_popup_always_on_top,
            port_aliases: self.port_aliases.clone(),
            send_history: self.send.send_history.iter().cloned().collect(),
            port_profiles: self.port_profiles.clone(),
            recent_workspaces: self.recent_workspaces.clone(),
            auto_reconnect: self.auto_reconnect,
        };
        let path = config_path();
        // 备份旧文件
        if path.exists() {
            let _ = std::fs::copy(&path, path.with_extension("json.backup"));
        }
        let t = serde_json::to_string_pretty(&cfg).map_err(|e| format!("序列化失败：{e}"))?;
        std::fs::write(&path, t).map_err(|e| format!("写入失败：{e}"))
    }

    pub(crate) fn save_config_to_path(&mut self, path: &std::path::Path) -> Result<(), String> {
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
            terminal_popup_always_on_top: self.terminal_popup_always_on_top,
            send_popup_always_on_top: self.send_popup_always_on_top,
            port_aliases: self.port_aliases.clone(),
            send_history: self.send.send_history.iter().cloned().collect(),
            port_profiles: self.port_profiles.clone(),
            recent_workspaces: self.recent_workspaces.clone(),
            auto_reconnect: self.auto_reconnect,
        };
        let t = serde_json::to_string_pretty(&cfg).map_err(|e| format!("序列化失败：{e}"))?;
        std::fs::write(path, t).map_err(|e| format!("写入失败：{e}"))
    }

    pub(crate) fn load_config_from_path(&mut self, path: &std::path::Path) -> Result<(), String> {
        let t = std::fs::read_to_string(path).map_err(|e| format!("读取失败：{e}"))?;
        let cfg: PersistedConfig =
            serde_json::from_str(&t).map_err(|e| format!("解析失败：{e}"))?;
        self.selected_port = cfg.selected_port.clone();
        self.baud_rate = cfg.baud_rate.clone();
        self.data_bits = cfg.data_bits.clone();
        self.stop_bits = cfg.stop_bits.clone();
        self.parity = cfg.parity.clone();
        self.timeout_ms = cfg.timeout_ms.clone();
        self.recorder_path = cfg.recorder_path.clone();
        self.activity_order = cfg.activity_order.clone();
        self.terminal_popup_always_on_top = cfg.terminal_popup_always_on_top;
        self.send_popup_always_on_top = cfg.send_popup_always_on_top;
        self.port_aliases = cfg.port_aliases.clone();
        self.send.send_history = cfg
            .send_history
            .iter()
            .filter(|item| !item.trim().is_empty())
            .take(MAX_SEND_HISTORY)
            .cloned()
            .collect();
        self.panels = cfg.panels.clone();
        self.apply_loaded_workspace_postprocess();
        Ok(())
    }

    pub(crate) fn add_recent_workspace(&mut self, path: &std::path::Path) {
        let s = path.display().to_string();
        self.recent_workspaces.retain(|p| p != &s);
        self.recent_workspaces.insert(0, s);
        self.recent_workspaces.truncate(10);
    }

    pub(crate) fn apply_loaded_workspace_postprocess(&mut self) {
        self.panels.discard_dynamic_tabs();
        self.panels.dock.normalize_tool_layout();
        self.bottom_panel_visible = self.panels.dock.bottom_visible;
        self.panels.bottom_logs_visible = self.panels.dock.bottom_visible;
        self.ensure_bottom_tab_available();
        self.refresh_ports_silent();
        self.dynamic_panels.set_ports(&self.ports);
        self.send.target_port = None;
        self.ensure_send_target_port();
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
        self.set_bottom_visible(true);
        self.panels.dock.move_panel(
            tool_panels::PanelKind::Terminal,
            tool_panels::DockArea::Bottom,
        );
        self.bottom_tab = BottomTab::Terminal;
    }

    pub(crate) fn set_bottom_visible(&mut self, visible: bool) {
        self.bottom_panel_visible = visible;
        self.panels.bottom_logs_visible = visible;
        self.panels.dock.bottom_visible = visible;
    }

    pub(crate) fn toggle_bottom_panel(&mut self) {
        self.set_bottom_visible(!self.panels.dock.bottom_visible);

        if self.panels.dock.bottom_visible {
            self.set_status(StatusLevel::Info, "底部面板已打开");
        }
    }
}
