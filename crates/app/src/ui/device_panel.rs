use crate::app::WorkbenchApp;
use crate::config::pick_recorder_path;
use crate::state::StatusLevel;
use eframe::egui;
use egui_material_icons::icons::{ICON_CABLE, ICON_TUNE, ICON_WARNING};
use std::collections::{BTreeMap, BTreeSet};
use tool_application::query::RecordModeView;
use tool_panels::{
    RecordingAction, RecordingMode, RecordingView, SerialPanel, design, recording_ui, theme,
};
use tool_platform::{SerialParity, SerialSettings};

// 端口行的实际最小内容宽度约为 500px（状态、名称、别名编辑器和分组选择器）。
// 留出少量余量即可，不能把卡片的可用宽度误当成 Dock 的断点。
const PORT_INLINE_MIN_WIDTH: f32 = 560.0;

impl WorkbenchApp {
    fn recording_panel(&mut self, ui: &mut egui::Ui) {
        let recording = self.workbench.query_recording();
        let stats = recording.stats.clone();
        let running = stats.running;
        let stopping = stats.stopping;
        let paused = stats.paused;
        let mut mode = match recording.mode {
            RecordModeView::StandardReplay => RecordingMode::StandardReplay,
            RecordModeView::RawSerial => RecordingMode::RawSerial,
        };
        let actions = {
            let mut view = RecordingView {
                file_name: &mut self.recorder_path,
                mode: &mut mode,
                running,
                stopping,
                paused,
                events_written: stats.events_written,
                bytes_written: Some(stats.bytes_written),
                flush_elapsed_ms: Some(stats.last_flush_elapsed_ms),
                backlog_events: None,
                backlog_bytes: None,
                current_path: recording.path.as_deref(),
                last_error: stats.last_error.as_deref(),
                show_browse: true,
            };
            recording_ui(ui, &mut view)
        };

        let native_mode = match mode {
            RecordingMode::StandardReplay => RecordModeView::StandardReplay,
            RecordingMode::RawSerial => RecordModeView::RawSerial,
        };
        if native_mode != recording.mode
            && let Err(error) = self
                .workbench
                .dispatch(tool_application::AppCommand::SetRecordingMode { mode: native_mode })
        {
            self.set_status_force(StatusLevel::Error, error.to_string());
        }

        for action in actions {
            match action {
                RecordingAction::Browse => {
                    if let Some(path) = pick_recorder_path(&self.recorder_path) {
                        self.recorder_path = path.display().to_string();
                    }
                }
                RecordingAction::StartStop => self.start_or_stop_recording(),
                RecordingAction::PauseResume => {
                    let command = if paused {
                        tool_application::AppCommand::ResumeRecording
                    } else {
                        tool_application::AppCommand::PauseRecording
                    };
                    if let Err(error) = self.workbench.dispatch(command) {
                        self.set_status_force(StatusLevel::Error, error.to_string());
                    }
                }
            }
        }
    }

    pub(crate) fn device_panel(&mut self, ui: &mut egui::Ui) {
        // ── 串口参数 ──
        design::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            design::section_header(ui, ICON_TUNE, "串口参数");
            ui.separator();

            let mut settings = SerialSettings {
                baud_rate: self.serial.baud_rate.parse().unwrap_or(115_200),
                data_bits: self.serial.data_bits.parse().unwrap_or(8),
                stop_bits: self.serial.stop_bits.parse().unwrap_or(1),
                parity: match self.serial.parity.as_str() {
                    "odd" => SerialParity::Odd,
                    "even" => SerialParity::Even,
                    _ => SerialParity::None,
                },
            };
            let previous_settings = settings;
            SerialPanel::settings_ui(ui, &mut settings);
            if settings != previous_settings {
                self.serial.baud_rate = settings.baud_rate.to_string();
                self.serial.data_bits = settings.data_bits.to_string();
                self.serial.stop_bits = settings.stop_bits.to_string();
                self.serial.parity = match settings.parity {
                    SerialParity::None => "none",
                    SerialParity::Odd => "odd",
                    SerialParity::Even => "even",
                }
                .to_owned();
                if let Err(error) = self
                    .workbench
                    .dispatch(tool_application::AppCommand::SetSerialSettings { settings })
                {
                    self.set_status_force(StatusLevel::Error, error.to_string());
                }
            }

            ui.checkbox(&mut self.serial.auto_reconnect, "串口拔出后自动重连");
            if self.serial.auto_reconnect
                && let Some(ref pending) = self.serial.pending_reconnect
            {
                let now = tool_core::now_timestamp_ms() as f64 / 1000.0;
                let remaining = (pending.next_try_at - now).max(0.0);
                ui.label(
                    egui::RichText::new(format!(
                        "等待 {} {:2.1}s 后重试 (第 {}/10 次)",
                        pending.port_name,
                        remaining,
                        pending.attempts + 1
                    ))
                    .color(theme::yellow()),
                );
            }
        });

        ui.add_space(8.0);

        self.recording_panel(ui);

        ui.add_space(8.0);

        // ── 可用端口 ──
        design::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            design::section_header(ui, ICON_CABLE, "可用端口");
            ui.separator();

            // ── 网络模拟串口（Nexus Prime 等 Klipper 服务器，WebSocket + JSON-RPC gcode 桥）──
            let mut add_network: Option<tool_application::query::NetworkPortConfig> = None;
            ui.horizontal_wrapped(|ui| {
                ui.label("网络");
                ui.add(
                    egui::TextEdit::singleline(&mut self.serial.network_host)
                        .desired_width(150.0)
                        .hint_text("IP 或主机名"),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.serial.network_port)
                        .desired_width(56.0)
                        .hint_text("7125"),
                );
                if ui.button("连接").clicked() {
                    let host = self.serial.network_host.trim().to_owned();
                    if host.is_empty() {
                        self.set_status(StatusLevel::Error, "请输入服务器 IP 或主机名");
                    } else {
                        match self.serial.network_port.trim().parse::<u16>() {
                            Ok(port) if port > 0 => {
                                add_network = Some(tool_application::query::NetworkPortConfig {
                                    host,
                                    port,
                                    api_key: None,
                                });
                            }
                            _ => self.set_status(StatusLevel::Error, "端口格式错误（1-65535）"),
                        }
                    }
                }
            });
            if let Some(cfg) = add_network {
                let name = cfg.display_name();
                if self
                    .serial
                    .network_ports
                    .iter()
                    .any(|n| n.display_name() == name)
                {
                    self.set_status_force(
                        StatusLevel::Warn,
                        format!("{name} 已存在，点击圆点连接"),
                    );
                } else {
                    self.serial.network_ports.push(cfg.clone());
                    let _ = self.workbench.dispatch(
                        tool_application::AppCommand::RegisterNetworkPort {
                            config: cfg.clone(),
                        },
                    );
                    if let Err(e) = self.save_config() {
                        log::warn!("save_config failed: {e}");
                    }
                    self.refresh_ports_silent();
                    // 添加即连接：异步建立 WebSocket，UI 进入“连接中”过渡态。
                    match self
                        .workbench
                        .dispatch(tool_application::AppCommand::Connect {
                            port: tool_platform::PortId::new(name.clone()),
                            settings: self.workbench.serial_settings(),
                        }) {
                        Ok(tool_application::CommandOutcome::Pending { .. }) => {
                            self.serial.selected_port = Some(name.clone());
                            self.defer_port_open_notice(&name, format!("{name} 已连接"));
                            self.set_status_force(StatusLevel::Info, format!("正在连接 {name}..."));
                        }
                        Ok(tool_application::CommandOutcome::Done) => {}
                        Err(error) => self.set_status_force(StatusLevel::Error, error.to_string()),
                    }
                }
                self.serial.selected_port = Some(name);
            }
            ui.separator();

            // 显示已打开但不在系统端口列表中的 stale 连接
            let transport_open = self.workbench.open_port_names();
            if !transport_open.is_empty() {
                let system_names: BTreeSet<&str> = self
                    .serial
                    .ports
                    .iter()
                    .map(|d| d.port_name.as_str())
                    .collect();
                let stale: Vec<&String> = transport_open
                    .iter()
                    .filter(|p| !system_names.contains(p.as_str()))
                    .collect();
                if !stale.is_empty() {
                    ui.colored_label(
                        theme::orange(),
                        format!("{} 以下端口已打开但可能已拔出：", ICON_WARNING.codepoint),
                    );
                    for port in &stale {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(*port)
                                    .monospace()
                                    .color(theme::orange()),
                            );
                            // 两步确认：首次点击 → 变红"确认?" → 再次点击才执行。
                            // 5 秒后自动解除武装。
                            let confirm_id = ui.id().with(("force_close_confirm", *port));
                            let now = ui.input(|i| i.time);
                            let armed_ts: Option<f64> =
                                ui.ctx().memory(|m| m.data.get_temp(confirm_id));
                            let armed = armed_ts.is_some_and(|t| now - t < 5.0);
                            let label = if armed { "确认?" } else { "强制关闭" };
                            let btn =
                                egui::Button::new(egui::RichText::new(label).color(if armed {
                                    theme::red()
                                } else {
                                    theme::orange()
                                }))
                                .small();
                            if ui.add(btn).clicked() {
                                if armed {
                                    match self.workbench.dispatch(
                                        tool_application::AppCommand::Disconnect {
                                            port: tool_platform::PortId::new((*port).clone()),
                                        },
                                    ) {
                                        Ok(tool_application::CommandOutcome::Pending {
                                            ..
                                        }) => {
                                            self.set_status_force(
                                                StatusLevel::Info,
                                                format!("正在强制关闭 {port}..."),
                                            );
                                        }
                                        Ok(tool_application::CommandOutcome::Done) => {}
                                        Err(error) => self.set_status_force(
                                            StatusLevel::Error,
                                            error.to_string(),
                                        ),
                                    }
                                    ui.ctx()
                                        .memory_mut(|m| m.data.remove_temp::<f64>(confirm_id));
                                } else {
                                    ui.ctx().memory_mut(|m| m.data.insert_temp(confirm_id, now));
                                }
                            }
                            if armed && ui.small_button("取消").clicked() {
                                ui.ctx()
                                    .memory_mut(|m| m.data.remove_temp::<f64>(confirm_id));
                            }
                        });
                    }
                    ui.separator();
                }
            }

            // 收集所有已有组名
            let group_names: Vec<String> = {
                let names: BTreeSet<String> = self.serial.port_groups.values().cloned().collect();
                names.into_iter().collect()
            };

            let mut alias_changes: Vec<(String, Option<String>)> = Vec::new();
            let mut group_changes: Vec<(String, Option<String>)> = Vec::new();
            let mut rename_group: Option<(String, String)> = None;
            let mut delete_group: Option<String> = None;
            // 圆点点击切换开/关：ScrollArea 闭包持有 &self.serial 不可变借用，
            // 无法在其中 &mut self，故收集到闭包外统一处理。
            let mut toggled_port: Option<String> = None;
            // 网络模拟串口的移除请求（同样在闭包外处理）。
            let mut removed_network: Option<String> = None;

            // 新建分组（通过 ComboBox 触发）
            let new_group_state_id = ui.make_persistent_id("port-new-group-active");
            let mut new_group_active =
                ui.data_mut(|d| d.get_persisted::<bool>(new_group_state_id).unwrap_or(false));
            let new_group_name_id = ui.make_persistent_id("port-new-group-name");
            let mut new_group_name = ui.data_mut(|d| {
                d.get_persisted::<String>(new_group_name_id)
                    .unwrap_or_default()
            });
            let new_group_port_id = ui.make_persistent_id("port-new-group-port");
            let mut new_group_port = ui.data_mut(|d| {
                d.get_persisted::<String>(new_group_port_id)
                    .unwrap_or_default()
            });

            // 按分组整理端口
            let mut groups: BTreeMap<String, Vec<&tool_application::query::PortView>> =
                BTreeMap::new();
            for port in &self.serial.ports {
                let group = self
                    .serial
                    .port_groups
                    .get(&port.port_name)
                    .cloned()
                    .unwrap_or_else(|| "未分组".to_owned());
                groups.entry(group).or_default().push(port);
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                for (group_name, ports) in &groups {
                    let is_default_group = group_name == "未分组";
                    let state_id = ui.make_persistent_id(format!("port-group-state-{group_name}"));
                    let mut open =
                        ui.data_mut(|d| d.get_persisted::<bool>(state_id).unwrap_or(true));

                    // ── 组标题行 ──
                    ui.horizontal_wrapped(|ui| {
                        let toggle = if open { "▾" } else { "▸" };
                        if ui.selectable_label(false, toggle).clicked() {
                            open = !open;
                        }
                        ui.label(egui::RichText::new(group_name).color(if is_default_group {
                            theme::text_secondary()
                        } else {
                            theme::text_primary()
                        }));
                        ui.label(
                            egui::RichText::new(format!("({})", ports.len()))
                                .color(theme::text_dimmed()),
                        );

                        // 分组操作菜单（非默认组）
                        if !is_default_group {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let menu_id =
                                        ui.make_persistent_id(format!("group-menu-{group_name}"));
                                    let mut show_menu = ui.data_mut(|d| {
                                        d.get_persisted::<bool>(menu_id).unwrap_or(false)
                                    });

                                    if ui.small_button("···").clicked() {
                                        show_menu = !show_menu;
                                    }
                                    ui.data_mut(|d| d.insert_persisted(menu_id, show_menu));

                                    if show_menu {
                                        // Escape 关闭菜单（与"新建分组"弹窗一致）
                                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                            ui.data_mut(|d| d.insert_persisted(menu_id, false));
                                        } else {
                                            let mut rename_input = group_name.clone();
                                            let text_edit =
                                                egui::TextEdit::singleline(&mut rename_input)
                                                    .desired_width(100.0);
                                            let resp = ui.add(text_edit);
                                            // Enter 确认改名
                                            let enter_pressed = ui
                                                .input(|i| i.key_pressed(egui::Key::Enter))
                                                && resp.has_focus();
                                            if (ui.small_button("改名").clicked() || enter_pressed)
                                                && !rename_input.trim().is_empty()
                                                && rename_input.trim() != group_name.as_str()
                                            {
                                                rename_group = Some((
                                                    group_name.clone(),
                                                    rename_input.trim().to_owned(),
                                                ));
                                                ui.data_mut(|d| d.insert_persisted(menu_id, false));
                                            }
                                            if ui.small_button("删除").clicked() {
                                                delete_group = Some(group_name.clone());
                                                ui.data_mut(|d| d.insert_persisted(menu_id, false));
                                            }
                                        }
                                    }
                                },
                            );
                        }
                    });

                    ui.data_mut(|d| d.insert_persisted(state_id, open));

                    // ── 端口列表 ──
                    if open {
                        for port in ports {
                            let name = port.port_name.clone();
                            let mut alias_buf = self
                                .serial
                                .port_aliases
                                .get(&name)
                                .cloned()
                                .unwrap_or_default();
                            let current_group = self
                                .serial
                                .port_groups
                                .get(&name)
                                .cloned()
                                .unwrap_or_else(|| "未分组".to_owned());

                            let status = self.workbench.transport_status(&name);
                            let pending_reconnect = self
                                .serial
                                .pending_reconnect
                                .as_ref()
                                .is_some_and(|p| p.port_name == name);
                            let port_type = port.port_type.to_string();
                            let is_network = port.port_type.is_network();
                            let has_alias = self.serial.port_aliases.contains_key(&name);
                            let inline = ui.available_width() >= PORT_INLINE_MIN_WIDTH;

                            if inline {
                                ui.horizontal(|ui| {
                                    render_port_status_controls(
                                        ui,
                                        &name,
                                        &port_type,
                                        is_network,
                                        status.open,
                                        status.connecting,
                                        pending_reconnect,
                                        &mut toggled_port,
                                        &mut removed_network,
                                    );
                                    render_port_editor_controls(
                                        ui,
                                        &name,
                                        &mut alias_buf,
                                        &current_group,
                                        &group_names,
                                        has_alias,
                                        true,
                                        &mut new_group_active,
                                        &mut new_group_port,
                                        &mut alias_changes,
                                        &mut group_changes,
                                    );
                                });
                            } else {
                                // 窄 Dock：信息行和编辑行分开，避免 wrapped 布局把
                                // “别名”插入状态行造成重叠。
                                ui.vertical(|ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        render_port_status_controls(
                                            ui,
                                            &name,
                                            &port_type,
                                            is_network,
                                            status.open,
                                            status.connecting,
                                            pending_reconnect,
                                            &mut toggled_port,
                                            &mut removed_network,
                                        );
                                    });
                                    ui.horizontal_wrapped(|ui| {
                                        render_port_editor_controls(
                                            ui,
                                            &name,
                                            &mut alias_buf,
                                            &current_group,
                                            &group_names,
                                            has_alias,
                                            false,
                                            &mut new_group_active,
                                            &mut new_group_port,
                                            &mut alias_changes,
                                            &mut group_changes,
                                        );
                                    });
                                    ui.add_space(4.0);
                                });
                            }
                        }
                    }
                }
            });

            // ── 新建分组弹窗（独立 Window，贴近触发点、支持 Escape、自动聚焦） ──
            if new_group_active {
                let mut keep_open = true;
                egui::Window::new("新建分组")
                    .id(egui::Id::new("new-group-window"))
                    .open(&mut keep_open)
                    .resizable(false)
                    .collapsible(false)
                    // 固定初始位置，避免每帧漂移导致 IME 候选框跳动打断中文输入。
                    .current_pos(egui::pos2(120.0, 80.0))
                    .show(ui.ctx(), |ui| {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut new_group_name)
                                .desired_width(160.0)
                                .hint_text("输入组名"),
                        );
                        // 只在未聚焦时 request_focus：每帧重复 request 会中断 IME
                        // 组合状态，导致中文输入法候选框被打断、拼音被强制提交。
                        if !resp.has_focus() {
                            resp.request_focus();
                        }
                        ui.horizontal(|ui| {
                            if ui.button("确定").clicked()
                                || (ui.input(|i| i.key_pressed(egui::Key::Enter))
                                    && !new_group_name.trim().is_empty())
                            {
                                let name = new_group_name.trim().to_owned();
                                if !name.is_empty() && !new_group_port.is_empty() {
                                    group_changes.push((new_group_port.clone(), Some(name)));
                                }
                                new_group_name.clear();
                                new_group_active = false;
                                new_group_port.clear();
                            }
                            if ui.small_button("取消").clicked()
                                || ui.input(|i| i.key_pressed(egui::Key::Escape))
                            {
                                new_group_name.clear();
                                new_group_active = false;
                                new_group_port.clear();
                            }
                        });
                    });
                if !keep_open {
                    new_group_name.clear();
                    new_group_active = false;
                    new_group_port.clear();
                }
                ui.data_mut(|d| d.insert_persisted(new_group_state_id, new_group_active));
                ui.data_mut(|d| d.insert_persisted(new_group_name_id, new_group_name.clone()));
                ui.data_mut(|d| d.insert_persisted(new_group_port_id, new_group_port.clone()));
            }

            // 圆点点击：在 ScrollArea 闭包外处理（避免与 &self.serial 不可变借用冲突）。
            if let Some(name) = toggled_port {
                self.toggle_port_by_name(&name);
            }

            // 移除网络端口：从配置列表删除、关闭连接、清空选择。
            if let Some(name) = removed_network {
                self.cancel_pending_port_open_notice(&name);
                self.serial
                    .network_ports
                    .retain(|n| n.display_name() != name);
                let _ = self
                    .workbench
                    .dispatch(tool_application::AppCommand::RemoveNetworkPort {
                        port: tool_platform::PortId::new(name.clone()),
                    });
                let _ = self
                    .workbench
                    .dispatch(tool_application::AppCommand::Disconnect {
                        port: tool_platform::PortId::new(name.clone()),
                    });
                if self.serial.selected_port.as_deref() == Some(name.as_str()) {
                    self.serial.selected_port = None;
                }
                if let Err(e) = self.save_config() {
                    log::warn!("save_config failed: {e}");
                }
                self.refresh_ports_silent();
                self.set_status_force(StatusLevel::Info, format!("{name} 已移除"));
            }

            // ── 应用变更 ──
            // 分组重命名
            if let Some((old_name, new_name)) = rename_group {
                for group in self.serial.port_groups.values_mut() {
                    if *group == old_name {
                        *group = new_name.clone();
                    }
                }
                if let Err(e) = self.save_config() {
                    log::warn!("save_config failed: {e}")
                };
            }
            // 删除分组（将组内端口移回未分组）
            if let Some(group_name) = delete_group {
                for group in self.serial.port_groups.values_mut() {
                    if *group == group_name {
                        *group = String::new(); // will be removed below
                    }
                }
                self.serial.port_groups.retain(|_, v| !v.is_empty());
                if let Err(e) = self.save_config() {
                    log::warn!("save_config failed: {e}")
                };
            }

            let has_changes = !alias_changes.is_empty() || !group_changes.is_empty();
            for (name, new_alias) in alias_changes {
                match new_alias {
                    Some(alias) => {
                        self.serial.port_aliases.insert(name, alias);
                    }
                    None => {
                        self.serial.port_aliases.remove(&name);
                    }
                }
            }
            for (name, new_group) in group_changes {
                match new_group {
                    Some(group) => {
                        self.serial.port_groups.insert(name, group);
                    }
                    None => {
                        self.serial.port_groups.remove(&name);
                    }
                }
            }
            if has_changes && let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            };
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn render_port_status_controls(
    ui: &mut egui::Ui,
    name: &str,
    port_type: &str,
    is_network: bool,
    port_open: bool,
    connecting: bool,
    pending_reconnect: bool,
    toggled_port: &mut Option<String>,
    removed_network: &mut Option<String>,
) {
    ui.add_space(16.0);

    // 端口状态按钮：带文字 + 颜色，加大命中区，色盲友好。
    // ●开(绿)=已开→点击关闭，○关(红)=未开→点击打开，
    // ⟳连(黄)=重连中/连接中→点击取消。
    let (icon, text, color, tooltip) = if pending_reconnect {
        ("⟳", "连", theme::yellow(), "重连中，点击取消")
    } else if connecting {
        ("⟳", "连", theme::yellow(), "连接中，点击取消")
    } else if port_open {
        ("●", "开", theme::green(), "已打开，点击关闭")
    } else {
        ("○", "关", theme::red(), "未打开，点击打开")
    };
    let btn_label = format!("{icon}{text}");
    if ui
        .add(
            egui::Button::new(egui::RichText::new(btn_label).color(color).small())
                .frame(false)
                .min_size(egui::vec2(36.0, 18.0)),
        )
        .on_hover_text(tooltip)
        .clicked()
    {
        *toggled_port = Some(name.to_owned());
    }

    ui.monospace(name).on_hover_text(name);
    ui.label(port_type);

    if is_network && ui.small_button("×").on_hover_text("移除网络端口").clicked() {
        *removed_network = Some(name.to_owned());
    }
}

#[allow(clippy::too_many_arguments)]
fn render_port_editor_controls(
    ui: &mut egui::Ui,
    name: &str,
    alias_buf: &mut String,
    current_group: &str,
    group_names: &[String],
    has_alias: bool,
    inline: bool,
    new_group_active: &mut bool,
    new_group_port: &mut String,
    alias_changes: &mut Vec<(String, Option<String>)>,
    group_changes: &mut Vec<(String, Option<String>)>,
) {
    if !inline {
        ui.add_space(16.0);
    }
    ui.label("别名");

    let alias_width = if inline {
        (ui.available_width() - 150.0).clamp(120.0, 240.0)
    } else {
        (ui.available_width() - 120.0).clamp(80.0, 160.0)
    };
    let response = ui.add(
        egui::TextEdit::singleline(alias_buf)
            .desired_width(alias_width)
            .hint_text("例如 主控板"),
    );
    // alias_buf 每帧从 port_aliases 重新 clone，必须即时回写，
    // 否则下一帧旧值会覆盖用户输入。save_config 有 60s autosave 兜底。
    if response.changed() {
        let new_alias = if alias_buf.trim().is_empty() {
            None
        } else {
            Some(alias_buf.trim().to_owned())
        };
        alias_changes.push((name.to_owned(), new_alias));
    }
    if has_alias && ui.small_button("×").clicked() {
        alias_changes.push((name.to_owned(), None));
    }

    let mut selected = current_group.to_owned();
    let group_width = if inline {
        100.0
    } else {
        ui.available_width().clamp(80.0, 100.0)
    };
    egui::ComboBox::from_id_salt(format!("port-group-{name}"))
        .width(group_width)
        .selected_text(&selected)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut selected, "未分组".to_owned(), "未分组");
            for group_name in group_names {
                ui.selectable_value(&mut selected, group_name.clone(), group_name.as_str());
            }
            ui.separator();
            if ui.button("+ 新建分组...").clicked() {
                *new_group_active = true;
                *new_group_port = name.to_owned();
            }
        });

    if selected != current_group {
        let new_group = (selected != "未分组").then_some(selected);
        group_changes.push((name.to_owned(), new_group));
    }
}
