use crate::app::WorkbenchApp;
use crate::config::{pick_recorder_path, record_mode_label};
use crate::state::StatusLevel;
use crate::ui::baud_combo;
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};
use tool_panels::theme;
use tool_recorder::RecordMode;

impl WorkbenchApp {
    pub(super) fn device_panel(&mut self, ui: &mut egui::Ui) {
        // ── 串口参数 ──
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::card_accent_bar(ui, theme::CARD_ACCENT_DEVICE);
                    ui.label(egui::RichText::new("📟 串口参数").heading());
                });
                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("波特率");
                    baud_combo(ui, "dev-port-rate", 180.0, &mut self.serial.baud_rate);
                    ui.label("数据位");
                    egui::ComboBox::from_id_salt("dev-db")
                        .width(60.0)
                        .selected_text(&self.serial.data_bits)
                        .show_ui(ui, |ui| {
                            for &v in &["5", "6", "7", "8"] {
                                ui.selectable_value(&mut self.serial.data_bits, v.to_owned(), v);
                            }
                        });

                    ui.label("停止位");
                    egui::ComboBox::from_id_salt("dev-sb")
                        .width(60.0)
                        .selected_text(&self.serial.stop_bits)
                        .show_ui(ui, |ui| {
                            for &v in &["1", "2"] {
                                ui.selectable_value(&mut self.serial.stop_bits, v.to_owned(), v);
                            }
                        });

                    ui.label("校验");
                    egui::ComboBox::from_id_salt("dev-par")
                        .width(70.0)
                        .selected_text(&self.serial.parity)
                        .show_ui(ui, |ui| {
                            for &(v, l) in &[("none", "无"), ("odd", "奇"), ("even", "偶")] {
                                ui.selectable_value(&mut self.serial.parity, v.to_owned(), l);
                            }
                        });
                });

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
                        .color(theme::YELLOW),
                    );
                }
            });

        ui.add_space(8.0);

        // ── 录制 ──
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::card_accent_bar(ui, theme::CARD_ACCENT_RECORD);
                    ui.label(egui::RichText::new("⏺ 录制").heading());
                });
                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("路径");

                    let recording = self.recorder.is_running();

                    ui.add_enabled(
                        !recording,
                        egui::TextEdit::singleline(&mut self.recorder_path).desired_width(360.0),
                    );

                    if ui
                        .add_enabled(!recording, egui::Button::new("浏览"))
                        .on_hover_text(if recording {
                            "录制中不能修改保存路径"
                        } else {
                            "选择录制保存路径"
                        })
                        .clicked()
                        && let Some(path) = pick_recorder_path(&self.recorder_path)
                    {
                        self.recorder_path = path.display().to_string();
                    }

                    let stopping = self.recorder.is_stopping();
                    if stopping {
                        ui.ctx().request_repaint();
                        ui.spinner();
                    }
                    if ui
                        .add_enabled(
                            !stopping,
                            egui::Button::new(if recording { "停止" } else { "录制" }),
                        )
                        .on_disabled_hover_text("正在停止中...")
                        .clicked()
                    {
                        self.start_or_stop_recording();
                    }
                    if recording {
                        let paused = self.recorder.is_paused();
                        if ui
                            .add_enabled(
                                !stopping,
                                egui::Button::new(if paused { "继续" } else { "暂停" }),
                            )
                            .on_disabled_hover_text("正在停止中...")
                            .clicked()
                        {
                            if paused {
                                self.recorder.resume();
                            } else {
                                self.recorder.pause();
                            }
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("模式");
                    let recording = self.recorder.is_running();
                    let mut mode = self.recorder.mode();
                    ui.add_enabled_ui(!recording, |ui| {
                        egui::ComboBox::from_id_salt("record-mode")
                            .width(160.0)
                            .selected_text(record_mode_label(mode))
                            .show_ui(ui, |ui| {
                                for &m in &[RecordMode::StandardReplay, RecordMode::RawSerial] {
                                    ui.selectable_value(&mut mode, m, record_mode_label(m));
                                }
                            });
                    });
                    self.recorder.set_mode(mode);
                });

                // ── 录制健康状态 ──
                let stats = self.recorder.stats();
                if stats.running || stats.stopping {
                    ui.separator();
                    ui.horizontal(|ui| {
                        if stats.paused {
                            ui.colored_label(theme::YELLOW, "⏸ 已暂停，未写入新事件");
                        } else if stats.running {
                            ui.colored_label(theme::GREEN, "● 录制中");
                        } else {
                            ui.colored_label(theme::YELLOW, "● 正在停止");
                        }

                        ui.label(format!("事件 {}", stats.events_written));
                        ui.label(format!(
                            "{:.1} MB",
                            stats.bytes_written as f64 / 1024.0 / 1024.0
                        ));
                        ui.label(format!("flush {} ms 前", stats.last_flush_elapsed_ms));
                    });

                    if let Some(path) = self.recorder.current_path() {
                        ui.label(format!("路径：{}", path.display()));
                    }
                    if let Some(ref error) = stats.last_error {
                        ui.colored_label(theme::RED, format!("录制错误：{error}"));
                    }
                }
            });

        ui.add_space(8.0);

        // ── 可用端口 ──
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::card_accent_bar(ui, theme::CARD_ACCENT_PORT);
                    ui.label(egui::RichText::new("🔌 可用端口").heading());
                });
                ui.separator();

                // 显示已打开但不在系统端口列表中的 stale 连接
                let transport_open = self.transport.open_ports();
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
                        ui.colored_label(theme::ORANGE, "⚠ 以下端口已打开但可能已拔出：");
                        for port in &stale {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(*port).monospace().color(theme::ORANGE),
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
                                        theme::RED
                                    } else {
                                        theme::ORANGE
                                    }))
                                    .small();
                                if ui.add(btn).clicked() {
                                    if armed {
                                        self.transport.close_port(port);
                                        self.set_status_force(
                                            StatusLevel::Info,
                                            format!("{port} 已强制关闭"),
                                        );
                                        ui.ctx()
                                            .memory_mut(|m| m.data.remove_temp::<f64>(confirm_id));
                                    } else {
                                        ui.ctx()
                                            .memory_mut(|m| m.data.insert_temp(confirm_id, now));
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

                ui.label(
                    egui::RichText::new("提示：别名会显示在串口选择、发送目标和设备列表中")
                        .color(theme::TEXT_SECONDARY),
                );

                // 收集所有已有组名
                let group_names: Vec<String> = {
                    let names: BTreeSet<String> =
                        self.serial.port_groups.values().cloned().collect();
                    names.into_iter().collect()
                };

                let mut alias_changes: Vec<(String, Option<String>)> = Vec::new();
                let mut group_changes: Vec<(String, Option<String>)> = Vec::new();
                let mut rename_group: Option<(String, String)> = None;
                let mut delete_group: Option<String> = None;
                // 圆点点击切换开/关：ScrollArea 闭包持有 &self.serial 不可变借用，
                // 无法在其中 &mut self，故收集到闭包外统一处理。
                let mut toggled_port: Option<String> = None;

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
                let mut groups: BTreeMap<String, Vec<&tool_transport::SerialPortDescriptor>> =
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
                        let state_id =
                            ui.make_persistent_id(format!("port-group-state-{group_name}"));
                        let mut open =
                            ui.data_mut(|d| d.get_persisted::<bool>(state_id).unwrap_or(true));

                        // ── 组标题行 ──
                        ui.horizontal(|ui| {
                            let toggle = if open { "▾" } else { "▸" };
                            if ui.selectable_label(false, toggle).clicked() {
                                open = !open;
                            }
                            ui.label(egui::RichText::new(group_name).color(if is_default_group {
                                theme::TEXT_SECONDARY
                            } else {
                                theme::TEXT_PRIMARY
                            }));
                            ui.label(
                                egui::RichText::new(format!("({})", ports.len()))
                                    .color(theme::TEXT_DIMMED),
                            );

                            // 分组操作菜单（非默认组）
                            if !is_default_group {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let menu_id = ui
                                            .make_persistent_id(format!("group-menu-{group_name}"));
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
                                                if (ui.small_button("改名").clicked()
                                                    || enter_pressed)
                                                    && !rename_input.trim().is_empty()
                                                    && rename_input.trim() != group_name.as_str()
                                                {
                                                    rename_group = Some((
                                                        group_name.clone(),
                                                        rename_input.trim().to_owned(),
                                                    ));
                                                    ui.data_mut(|d| {
                                                        d.insert_persisted(menu_id, false)
                                                    });
                                                }
                                                if ui.small_button("删除").clicked() {
                                                    delete_group = Some(group_name.clone());
                                                    ui.data_mut(|d| {
                                                        d.insert_persisted(menu_id, false)
                                                    });
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

                                ui.horizontal(|ui| {
                                    ui.add_space(16.0);
                                    let port_open = self.transport.status_port(&name).open;
                                    let pending_reconnect = self
                                        .serial
                                        .pending_reconnect
                                        .as_ref()
                                        .is_some_and(|p| p.port_name == name);
                                    // 端口状态按钮：带文字 + 颜色，加大命中区，色盲友好。
                                    // ●开(绿)=已开→点击关闭，○关(红)=未开→点击打开，⟳连(黄)=重连中→点击取消。
                                    let (icon, text, color, tooltip) = if pending_reconnect {
                                        ("⟳", "连", theme::YELLOW, "重连中，点击取消")
                                    } else if port_open {
                                        ("●", "开", theme::GREEN, "已打开，点击关闭")
                                    } else {
                                        ("○", "关", theme::RED, "未打开，点击打开")
                                    };
                                    let btn_label = format!("{icon}{text}");
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new(btn_label).color(color).small(),
                                            )
                                            .frame(false)
                                            .min_size(egui::vec2(36.0, 18.0)),
                                        )
                                        .on_hover_text(tooltip)
                                        .clicked()
                                    {
                                        toggled_port = Some(name.clone());
                                    }
                                    ui.monospace(&name);
                                    ui.label(port.port_type.to_string());

                                    // 别名
                                    ui.label("别名");
                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut alias_buf)
                                            .desired_width(100.0)
                                            .hint_text("例如 主控板"),
                                    );
                                    // alias_buf 每帧从 port_aliases 重新 clone，必须即时回写，
                                    // 否则下一帧旧值会覆盖用户输入。save_config 有 60s autosave 兜底。
                                    if resp.changed() {
                                        let new_alias = if alias_buf.trim().is_empty() {
                                            None
                                        } else {
                                            Some(alias_buf.trim().to_owned())
                                        };
                                        alias_changes.push((name.clone(), new_alias));
                                    }
                                    if self.serial.port_aliases.contains_key(&name)
                                        && ui.small_button("×").clicked()
                                    {
                                        alias_changes.push((name.clone(), None));
                                    }

                                    // 分组选择
                                    let mut selected = current_group.clone();
                                    egui::ComboBox::from_id_salt(format!("port-group-{name}"))
                                        .width(100.0)
                                        .selected_text(&selected)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut selected,
                                                "未分组".to_owned(),
                                                "未分组",
                                            );
                                            for gn in &group_names {
                                                ui.selectable_value(
                                                    &mut selected,
                                                    gn.clone(),
                                                    gn.as_str(),
                                                );
                                            }
                                            ui.separator();
                                            if ui.button("+ 新建分组...").clicked() {
                                                new_group_active = true;
                                                new_group_port = name.clone();
                                            }
                                        });

                                    if selected != current_group {
                                        let new_group = if selected == "未分组" {
                                            None
                                        } else {
                                            Some(selected)
                                        };
                                        group_changes.push((name.clone(), new_group));
                                    }
                                });
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

                // ── 应用变更 ──
                // 分组重命名
                if let Some((old_name, new_name)) = rename_group {
                    for (_port, group) in self.serial.port_groups.iter_mut() {
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
                    for (_port, group) in self.serial.port_groups.iter_mut() {
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
