use crate::app::CheckResult;
use crate::app::WorkbenchApp;
use crate::config::windows_open_dialog;
use crate::keymap::{Action, KeyBinding};
use crate::state::StatusLevel;
use eframe::egui;
use std::sync::Arc;
use tool_core::{Direction, Event, Payload, topics};
use tool_panels::PanelKind;
use tool_recorder::{ReplayBlockReason, ReplayState};
use tool_transport::send_impl_to;

impl WorkbenchApp {
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        // 快捷键录制期间跳过全局快捷键，避免误触发其他动作导致离开设置页
        if self.key_recording.is_some() {
            return;
        }
        let keymap = self.keymap.clone();
        let mut triggered: Option<Action> = None;

        ctx.input(|i| {
            for (key, bindings) in &keymap.bindings {
                for binding in bindings {
                    if let Some(egui_key) = parse_egui_key(&binding.key) {
                        let mods_match = i.modifiers.ctrl == binding.ctrl
                            && i.modifiers.shift == binding.shift
                            && i.modifiers.alt == binding.alt;
                        if mods_match && i.key_pressed(egui_key) {
                            triggered = Action::from_key(key);
                        }
                    }
                }
            }
        });

        if let Some(action) = triggered {
            self.pending_action = Some(action);
        }
    }

    /// 执行当前帧触发的快捷键动作（在 tick_pre_ui 中调用）。
    fn flush_pending_action(&mut self) {
        if let Some(action) = self.pending_action.take() {
            self.execute_action(action);
        }
    }

    /// 执行快捷键对应的操作。
    fn execute_action(&mut self, action: Action) {
        match action {
            Action::RefreshPorts => self.refresh_ports(),
            Action::OpenPort => self.toggle_selected_port(),
            Action::ToggleActivityBar => {
                self.panels.dock.activity_bar_visible = !self.panels.dock.activity_bar_visible;
                if let Err(e) = self.save_config() {
                    log::warn!("save_config failed: {e}")
                };
            }
            Action::ToggleBottomPanel => self.toggle_bottom_panel(),
            Action::Send => {
                if self.send_target_port_open() && !self.send.input.trim().is_empty() {
                    self.do_send();
                }
            }
            Action::StartRecording => self.start_or_stop_recording(),
            Action::ReconnectPort => self.reconnect_selected_port(),
            Action::PluginCommand(plugin_id, command_id) => {
                self.publish_plugin_command_action(&plugin_id, &command_id);
            }
        }
    }

    /// 发布插件命令动作（模拟 UI 按钮点击）。
    fn publish_plugin_command_action(&mut self, plugin_id: &str, command_id: &str) {
        // 查找该插件的 UI contribution 信息以确定是否 record_send_input
        let summaries = self.plugin_manager.summaries();
        let record_send_input = summaries
            .iter()
            .find(|s| s.id == plugin_id)
            .and_then(|s| {
                s.contributes
                    .ui
                    .iter()
                    .find(|ui| ui.command.as_deref() == Some(command_id))
            })
            .map(|ui| ui.record_send_input)
            .unwrap_or(false);

        if record_send_input {
            self.record_send_history(self.send.input.clone());
        }

        // Authorize file access if plugin has fs.read.user_selected permission
        let has_fs_permission = summaries
            .iter()
            .find(|s| s.id == plugin_id)
            .map(|s| s.permissions.iter().any(|p| p == "fs.read.user_selected"))
            .unwrap_or(false);

        if has_fs_permission {
            let input = self.send.input.trim();
            if !input.is_empty() && input.lines().count() == 1 {
                let path = std::path::PathBuf::from(input.trim_matches('"'));
                if path.is_file() {
                    self.file_broker.authorize(plugin_id, path);
                }
            }
        }

        let context = serde_json::json!({
            "slot": "send.toolbar",
            "send": {
                "input": self.send.input.clone(),
                "target_port": self.send.target_port.clone(),
                "target_port_open": self.send_target_port_open(),
                "hex_mode": self.send.hex_mode,
                "line_ending": {
                    "label": self.send.line_ending.label(),
                    "suffix": self.send.line_ending.suffix(),
                },
                "periodic_enabled": self.send.periodic_enabled,
                "periodic_interval_ms": self.send.periodic_interval_ms,
            },
            "serial": {
                "selected_port": self.serial.selected_port.clone(),
                "open_ports": self.transport.open_ports(),
            }
        });

        let payload = serde_json::json!({
            "plugin_id": plugin_id,
            "contribution_id": command_id,
            "slot": "send.toolbar",
            "kind": "button",
            "command": command_id,
            "context": context,
        });

        self.publish_plugin_command_execute(plugin_id, command_id, &payload);
    }

    pub(crate) fn publish_plugin_command_execute(
        &mut self,
        plugin_id: &str,
        command_id: &str,
        payload: &serde_json::Value,
    ) {
        let mut payload = payload.clone();
        if let Some(object) = payload.as_object_mut() {
            object.insert("plugin_id".to_owned(), serde_json::json!(plugin_id));
            object.insert("command".to_owned(), serde_json::json!(command_id));
            object.insert("origin".to_owned(), serde_json::json!("host.command"));
        }

        self.bus.publish(Event::new(
            topics::PLUGIN_COMMAND_EXECUTE,
            "plugin.command",
            Direction::Internal,
            Payload::Json(payload),
        ));
    }
}

/// 将 keymap 中的键名字符串转换为 egui::Key。
fn parse_egui_key(name: &str) -> Option<egui::Key> {
    match name {
        "A" => Some(egui::Key::A),
        "B" => Some(egui::Key::B),
        "C" => Some(egui::Key::C),
        "D" => Some(egui::Key::D),
        "E" => Some(egui::Key::E),
        "F" => Some(egui::Key::F),
        "G" => Some(egui::Key::G),
        "H" => Some(egui::Key::H),
        "I" => Some(egui::Key::I),
        "J" => Some(egui::Key::J),
        "K" => Some(egui::Key::K),
        "L" => Some(egui::Key::L),
        "M" => Some(egui::Key::M),
        "N" => Some(egui::Key::N),
        "O" => Some(egui::Key::O),
        "P" => Some(egui::Key::P),
        "Q" => Some(egui::Key::Q),
        "R" => Some(egui::Key::R),
        "S" => Some(egui::Key::S),
        "T" => Some(egui::Key::T),
        "U" => Some(egui::Key::U),
        "V" => Some(egui::Key::V),
        "W" => Some(egui::Key::W),
        "X" => Some(egui::Key::X),
        "Y" => Some(egui::Key::Y),
        "Z" => Some(egui::Key::Z),
        "Num0" => Some(egui::Key::Num0),
        "Num1" => Some(egui::Key::Num1),
        "Num2" => Some(egui::Key::Num2),
        "Num3" => Some(egui::Key::Num3),
        "Num4" => Some(egui::Key::Num4),
        "Num5" => Some(egui::Key::Num5),
        "Num6" => Some(egui::Key::Num6),
        "Num7" => Some(egui::Key::Num7),
        "Num8" => Some(egui::Key::Num8),
        "Num9" => Some(egui::Key::Num9),
        "Escape" => Some(egui::Key::Escape),
        "Enter" => Some(egui::Key::Enter),
        "Tab" => Some(egui::Key::Tab),
        "Space" => Some(egui::Key::Space),
        "Backspace" => Some(egui::Key::Backspace),
        "Delete" => Some(egui::Key::Delete),
        "Insert" => Some(egui::Key::Insert),
        "Home" => Some(egui::Key::Home),
        "End" => Some(egui::Key::End),
        "PageUp" => Some(egui::Key::PageUp),
        "PageDown" => Some(egui::Key::PageDown),
        "ArrowUp" => Some(egui::Key::ArrowUp),
        "ArrowDown" => Some(egui::Key::ArrowDown),
        "ArrowLeft" => Some(egui::Key::ArrowLeft),
        "ArrowRight" => Some(egui::Key::ArrowRight),
        "F1" => Some(egui::Key::F1),
        "F2" => Some(egui::Key::F2),
        "F3" => Some(egui::Key::F3),
        "F4" => Some(egui::Key::F4),
        "F5" => Some(egui::Key::F5),
        "F6" => Some(egui::Key::F6),
        "F7" => Some(egui::Key::F7),
        "F8" => Some(egui::Key::F8),
        "F9" => Some(egui::Key::F9),
        "F10" => Some(egui::Key::F10),
        "F11" => Some(egui::Key::F11),
        "F12" => Some(egui::Key::F12),
        "Backtick" => Some(egui::Key::Backtick),
        "Minus" => Some(egui::Key::Minus),
        "Equals" => Some(egui::Key::Equals),
        "Comma" => Some(egui::Key::Comma),
        "Period" => Some(egui::Key::Period),
        "Slash" => Some(egui::Key::Slash),
        "Backslash" => Some(egui::Key::Backslash),
        "Semicolon" => Some(egui::Key::Semicolon),
        "Quote" => Some(egui::Key::Quote),
        "OpenBracket" => Some(egui::Key::OpenBracket),
        "CloseBracket" => Some(egui::Key::CloseBracket),
        _ => None,
    }
}

impl WorkbenchApp {
    pub(crate) fn tick_pre_ui(&mut self, ctx: &egui::Context) {
        self.clear_status_if_expired();
        self.tick_recorder_status();
        self.tick_terminal_maximize();
        self.tick_replay(ctx);
        self.tick_plugin_lifecycle();
        self.handle_keys(ctx);
        self.flush_pending_action();
        self.tick_key_recording(ctx);
        self.tick_port_refresh(ctx);
        self.tick_periodic_send(ctx);
        self.tick_auto_save(ctx);
        self.tick_update();
    }

    /// 快捷键录制：按 Escape 取消，离开设置页时取消。检测到按键则保存绑定。
    fn tick_key_recording(&mut self, ctx: &egui::Context) {
        if self.key_recording.is_none() {
            return;
        }
        // 离开设置页时取消录制。这里看 dock 的实际可见面板，而不是可能滞后的 active_tab。
        if !self.panels.is_panel_visible(&PanelKind::Settings) {
            self.key_recording = None;
            return;
        }
        // Escape 只取消录制，不能被保存成快捷键。
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.key_recording = None;
            return;
        }
        // 检查是否有按键事件
        if let Some((key_name, modifiers)) = Self::capture_key_for_recording(ctx) {
            let action = self.key_recording.take().unwrap();
            let new_binding =
                KeyBinding::new(&key_name, modifiers.ctrl, modifiers.shift, modifiers.alt);

            self.keymap.remove_binding_everywhere(&new_binding);
            let mut bindings = self.keymap.get_bindings(&action);
            bindings.retain(|b| {
                !(b.ctrl == new_binding.ctrl
                    && b.shift == new_binding.shift
                    && b.alt == new_binding.alt)
            });
            bindings.push(new_binding);
            self.keymap.set_bindings(&action, bindings);
            if let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            };
            self.set_status_force(
                StatusLevel::Info,
                format!(
                    "{} 快捷键已更新",
                    action.label_with_plugins(&self.plugin_manager.summaries())
                ),
            );
        }
    }

    /// 捕获按键事件用于快捷键录制。返回按下的键名。
    fn capture_key_for_recording(ctx: &egui::Context) -> Option<(String, egui::Modifiers)> {
        ctx.input(|i| {
            for event in &i.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                {
                    return Some((format!("{key:?}"), *modifiers));
                }
            }
            None
        })
    }

    /// 录制状态检测：收割已停止的录制、worker 线程错误反馈。
    fn tick_recorder_status(&mut self) {
        match self.recorder.reap_stopping() {
            Some(Ok(path)) => {
                self.set_status_force(StatusLevel::Info, format!("录制已保存: {}", path.display()))
            }
            Some(Err(e)) => self.set_status_force(StatusLevel::Error, format!("录制失败: {e}")),
            None => {}
        }
        if let Some(error) = self.recorder.reap_error() {
            self.set_status(StatusLevel::Error, format!("录制失败：{error}"));
        }
    }

    /// 终端放大按钮：打开悬浮窗并切走底部 Terminal。
    fn tick_terminal_maximize(&mut self) {
        if self.terminal_panel.maximize_clicked {
            self.terminal_panel.maximize_clicked = false;
            self.terminal_popup_open = true;
            if matches!(
                self.panels.dock.bottom.active_or_first(),
                Some(tool_panels::PanelKind::Terminal)
            ) {
                self.panels.dock.bottom.active = Some(tool_panels::PanelKind::Logs);
            }
        }
    }

    /// 回放相关：seek/step/pick_file/analyzer 调度。
    fn tick_replay(&mut self, ctx: &egui::Context) {
        if self.replay_panel.want_clear_on_play {
            self.replay_panel.want_clear_on_play = false;
            self.terminal_panel.clear();
            self.bottom_log_panel.clear();
            self.dynamic_panels.clear_charts();
        }

        if let Some(steps) = self.replay_panel.want_step_backward.take() {
            self.do_replay_step_backward(steps, ctx);
        }

        if let Some(p) = self.replay_panel.want_seek_replay.take() {
            self.do_replay_seek_rebuild(p, ctx);
        }

        if self.replay_panel.want_pick_file {
            self.replay_panel.want_pick_file = false;
            if let Some(p) = windows_open_dialog() {
                self.replay_panel.path = p.display().to_string();
                self.replay_panel.auto_load = true;
            }
        }
        if self.replay_panel.want_run_analyzers {
            self.launch_replay_analyzer_background();
        }
        if self.replay_panel.want_cancel_analyzers {
            self.replay_panel.want_cancel_analyzers = false;
            if let Some(ref job) = self.replay_analyzer_job {
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                self.set_status(StatusLevel::Warn, "回放：正在取消 analyzer...");
            }
        }
        self.replay_panel.analyzer_busy = self
            .replay_analyzer_job
            .as_ref()
            .is_some_and(|job| !job.handle.is_finished());

        self.poll_replay_analyzer_result();

        let was_playing = matches!(
            self.replay_panel.manager().status().state,
            ReplayState::Playing
        );
        let replay_tick = self.replay_panel.tick_playback();
        if was_playing || replay_tick.published > 0 || replay_tick.loop_restarted {
            ctx.request_repaint();
        }
    }

    /// 插件生命周期：禁用清理 + ingest + 事件处理。
    fn tick_plugin_lifecycle(&mut self) {
        self.poll_dialog_requests();
        self.handle_file_browse_requests();

        for plugin_id in self.plugins_panel.take_recently_disabled() {
            let removed = self.dynamic_panels.remove_by_plugin(&plugin_id);
            for id in &removed {
                self.detached_dynamic_panels.remove(id);
                self.panels
                    .close_tab(tool_panels::PanelKind::Dynamic(id.clone()));
            }
            self.file_broker.clear(&plugin_id);
        }

        self.dynamic_panels.ingest(&mut self.panels);
        let _terminal_ingested = self.terminal_panel.ingest_pending();
        self.plugin_manager.process_pending();
        self.process_contribution_set_value();
    }

    /// 处理插件通过 ctx.ui.set_contribution_value 对 UI contribution 的状态更新。
    /// 使用专用 topic `ui.contribution.set_value`，与动态面板的 `ui.form.set_value` 隔离。
    fn process_contribution_set_value(&mut self) {
        for event in self.contribution_set_value_subscription.drain_limited(64) {
            let tool_core::Payload::Json(payload) = event.payload else {
                continue;
            };
            // 要求 panel_id == "__contribution__" 作为哨兵，防止误消费面板事件
            if payload.get("panel_id").and_then(serde_json::Value::as_str)
                != Some("__contribution__")
            {
                continue;
            }
            let Some(contribution_id) = payload.get("field_id").and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let Some(value) = payload.get("value") else {
                continue;
            };
            // 从事件 source 提取 plugin_id（格式 "plugin:{plugin_id}"）
            let plugin_id = event
                .source
                .strip_prefix("plugin:")
                .unwrap_or(&event.source);
            self.set_contribution_value(plugin_id, contribution_id, value.clone());
        }
    }

    /// 串口刷新。
    fn tick_port_refresh(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let refresh_interval = if ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
            0.5
        } else {
            2.0
        };
        if now - self.serial.last_port_refresh > refresh_interval {
            self.serial.last_port_refresh = now;
            self.refresh_ports_silent();
        }
    }

    /// 自动保存工作区（每60秒）。
    fn tick_auto_save(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if now - self.last_auto_save_time > 60.0 {
            self.last_auto_save_time = now;
            if let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            };
        }
    }

    fn do_replay_step_backward(&mut self, steps: usize, ctx: &egui::Context) {
        if self.replay_panel.manager().status().cursor == 0 {
            return;
        }
        if let Some(target_cursor) = self.replay_panel.manager().backward_cursor_by(steps.max(1)) {
            self.do_replay_cursor_rebuild(target_cursor, ctx);
        }
    }

    fn do_replay_seek_rebuild(&mut self, position: u64, ctx: &egui::Context) {
        self.do_replay_rebuild(Some(position), ctx);
    }

    fn do_replay_rebuild(&mut self, position: Option<u64>, ctx: &egui::Context) {
        if position.is_some() && self.warn_if_replay_blocked() {
            return;
        }

        self.terminal_panel.clear();
        self.bottom_log_panel.clear();
        self.dynamic_panels.clear_charts();

        self.bus.publish(Event::new(
            "ui.replay.reset",
            "ui.replay",
            Direction::Internal,
            Payload::Empty,
        ));

        if let Some(pos) = position {
            self.replay_panel.do_seek_panel_phase(pos);
            self.dynamic_panels.ingest(&mut self.panels);
            self.replay_panel.do_seek_data_phase(pos);
        }

        let terminal_count = self.terminal_panel.ingest_all_pending();
        let log_count = self.bottom_log_panel.ingest_all_pending();
        let chart_count = self.dynamic_panels.ingest_all_pending();

        self.set_status(
            StatusLevel::Info,
            format!(
                "回放重建完成：接收 {terminal_count} 条，日志 {log_count} 条，图表 {chart_count} 条"
            ),
        );
        ctx.request_repaint();
    }

    fn do_replay_cursor_rebuild(&mut self, target_cursor: usize, ctx: &egui::Context) {
        if self.warn_if_replay_blocked() {
            return;
        }

        self.terminal_panel.clear();
        self.bottom_log_panel.clear();
        self.dynamic_panels.clear_charts();

        self.bus.publish(Event::new(
            "ui.replay.reset",
            "ui.replay",
            Direction::Internal,
            Payload::Empty,
        ));

        self.replay_panel.do_seek_cursor_panel_phase(target_cursor);
        self.dynamic_panels.ingest(&mut self.panels);
        self.replay_panel.do_seek_cursor_data_phase(target_cursor);

        let terminal_count = self.terminal_panel.ingest_all_pending();
        let log_count = self.bottom_log_panel.ingest_all_pending();
        let chart_count = self.dynamic_panels.ingest_all_pending();

        self.set_status(
            StatusLevel::Info,
            format!(
                "回放重建完成：接收 {terminal_count} 条，日志 {log_count} 条，图表 {chart_count} 条"
            ),
        );
        ctx.request_repaint();
    }

    fn warn_if_replay_blocked(&mut self) -> bool {
        let Some(reason) = self.replay_panel.manager().replay_block_reason() else {
            return false;
        };

        let message = match reason {
            ReplayBlockReason::NeedAnalyzer => "当前回放策略需要先完成 Replay Analyzer".to_owned(),
            ReplayBlockReason::AnalyzerFailed(error) => {
                format!("Replay Analyzer 失败，无法重建回放：{error}")
            }
        };
        self.set_status(StatusLevel::Warn, message);
        true
    }

    fn tick_periodic_send(&mut self, _ctx: &egui::Context) {
        // 检查是否被外部关闭，或线程已自然结束（cancel flag 被线程设为 true）
        if self.periodic_send_cancel.is_some() && !self.send.periodic_enabled {
            if let Some(cancel) = self.periodic_send_cancel.take() {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            return;
        }
        // 线程已结束（cancel flag 为 true），清理状态
        if self
            .periodic_send_cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
        {
            self.periodic_send_cancel = None;
            self.send.periodic_enabled = false;
            self.send.periodic_send_count = 0;
            return;
        }
        if self.periodic_send_cancel.is_some() {
            return;
        }
        if !self.send.periodic_enabled {
            return;
        }

        let interval_ms: f64 = match self.send.periodic_interval_ms.trim().parse() {
            Ok(v) if v > 0.0 => v,
            _ => {
                self.send.periodic_enabled = false;
                self.set_status_force(crate::state::StatusLevel::Warn, "周期发送间隔必须 > 0ms");
                return;
            }
        };
        if !self.send_target_port_open() {
            self.send.periodic_enabled = false;
            self.set_status_force(
                crate::state::StatusLevel::Error,
                "周期发送已停止：目标串口未打开",
            );
            return;
        }
        if self.send.input.is_empty() {
            self.send.periodic_enabled = false;
            self.set_status_force(crate::state::StatusLevel::Warn, "周期发送已停止：输入为空");
            return;
        }

        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.periodic_send_cancel = Some(cancel.clone());

        let port = self.send.target_port.clone().unwrap_or_default();
        let input = self.send.input.clone();
        let hex_mode = self.send.hex_mode;
        let line_ending = self.send.line_ending;
        let hex_strict = self.send.hex_strict;
        let transport = self.transport.clone();
        let max_count = self.send.periodic_max_count;
        let bus = self.bus.clone();
        let interval = std::time::Duration::from_secs_f64(interval_ms / 1000.0);

        std::thread::spawn(move || {
            // 提升为实时优先级，减少 OS 调度延迟
            #[cfg(target_os = "windows")]
            unsafe {
                unsafe extern "system" {
                    fn SetThreadPriority(thread: isize, priority: i32) -> i32;
                    fn GetCurrentThread() -> isize;
                }
                SetThreadPriority(GetCurrentThread(), 15); // THREAD_PRIORITY_TIME_CRITICAL
            }

            let start = std::time::Instant::now();
            let mut count: u64 = 0;

            loop {
                // 基于 start_time + count * interval 计算 absolute deadline
                let deadline = start + interval * (count as u32 + 1);

                // 纯 spin-wait 到 deadline
                let mut spin_count = 0u32;
                while std::time::Instant::now() < deadline {
                    std::hint::spin_loop();
                    spin_count = spin_count.wrapping_add(1);
                    if spin_count & 0xFF == 0 && cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                }

                // 恰好到期，发送
                let err = send_impl_to(
                    &port,
                    &input,
                    hex_mode,
                    line_ending.suffix(),
                    hex_strict,
                    &transport,
                )
                .err()
                .map(|e| e.to_string());

                if let Some(e) = err {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    bus.publish(tool_core::Event::system_log(
                        tool_core::LogLevel::Error,
                        "periodic",
                        format!("周期发送失败: {e}"),
                    ));
                    return;
                }

                count += 1;
                if let Some(max) = max_count
                    && count >= max
                {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    bus.publish(tool_core::Event::system_log(
                        tool_core::LogLevel::Info,
                        "periodic",
                        format!("周期发送已完成 ({max} 次)"),
                    ));
                    return;
                }
            }
        });
    }

    /// 自动更新调度：启动检查、收割结果、启动下载、收割下载、处理重启。
    fn tick_update(&mut self) {
        // 从 Arc 读取下载进度
        if let Some(ref progress_arc) = self.update_state_download_progress {
            let raw = progress_arc.load(std::sync::atomic::Ordering::Relaxed);
            self.update_state.download_progress = raw as f32 / 1000.0;
        }

        // 1. 用户点击"更新并重启"
        if self.update_state.want_restart {
            let manifest_info = self
                .update_state
                .latest_version
                .as_ref()
                .zip(self.update_state.downloaded_sha256.as_ref())
                .map(|(v, s)| (v.clone(), s.clone()));

            if let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            };

            if let Some((version, sha256)) = manifest_info
                && let Err(e) = tool_updater::write_update_manifest(&version, &sha256)
            {
                log::error!("write_update_manifest failed: {e}");
                self.update_state.want_restart = false;
                self.update_state.error = Some(format!("写入更新标记失败：{e}"));
                return;
            }
            std::process::exit(0);
        }

        // 2. 收割检查线程结果
        if let Some(handle) = self.update_state.check_handle.take() {
            if handle.is_finished() {
                match handle.join() {
                    Ok(Ok(result)) => {
                        self.update_state.checking = false;
                        if result.cached {
                            self.update_state.latest_version = Some(result.version.clone());
                        } else if result.download_url.is_empty() {
                            self.update_state.latest_version = Some(result.version.clone());
                            log::info!("updater: 已是最新版本");
                        } else {
                            self.update_state.latest_version = Some(result.version.clone());
                            self.update_state.changelog = result.changelog;
                            self.update_state.update_available = true;
                            self.update_state.download_url = Some(result.download_url);
                            self.update_state.error = None;
                            log::info!("updater: 发现新版本 v{}", result.version);
                        }
                    }
                    Ok(Err(e)) => {
                        self.update_state.checking = false;
                        self.update_state.error = Some(e);
                        log::warn!(
                            "updater: 检查更新失败：{}",
                            self.update_state.error.as_deref().unwrap_or("")
                        );
                    }
                    Err(_) => {
                        self.update_state.checking = false;
                        self.update_state.error = Some("检查更新线程异常退出".into());
                    }
                }
            } else {
                self.update_state.check_handle = Some(handle);
            }
        }

        // 3. 收割下载线程结果
        if let Some(handle) = self.update_state.download_handle.take() {
            if handle.is_finished() {
                match handle.join() {
                    Ok(Ok(actual_sha256)) => {
                        self.update_state.downloading = false;
                        self.update_state.download_progress = 1.0;
                        self.update_state.downloaded = true;
                        self.update_state.downloaded_sha256 = Some(actual_sha256);
                        self.update_state.error = None;
                        log::info!("updater: 更新包下载完成");
                    }
                    Ok(Err(e)) => {
                        self.update_state.downloading = false;
                        self.update_state.error = Some(e);
                        log::warn!(
                            "updater: 下载更新失败：{}",
                            self.update_state.error.as_deref().unwrap_or("")
                        );
                    }
                    Err(_) => {
                        self.update_state.downloading = false;
                        self.update_state.error = Some("下载更新线程异常退出".into());
                    }
                }
            } else {
                self.update_state.download_handle = Some(handle);
            }
        }

        // 4. 首次自动检查（非强制时考虑 24h 缓存）
        if !self.update_state.checking
            && self.update_state.check_handle.is_none()
            && self.update_state.error.is_none()
            && self.update_state.latest_version.is_none()
            && !self.update_state.force_check
        {
            self.start_update_check(false);
        }

        // 5. 用户手动触发检查
        if self.update_state.force_check
            && !self.update_state.checking
            && self.update_state.check_handle.is_none()
        {
            self.start_update_check(true);
        }
    }

    /// 启动后台检查更新线程。
    /// `force` = true 时跳过 24h 缓存。
    fn start_update_check(&mut self, force: bool) {
        self.update_state.checking = true;
        self.update_state.force_check = false;
        self.update_state.error = None;
        let current_version = env!("CARGO_PKG_VERSION").to_owned();

        self.update_state.check_handle = Some(std::thread::spawn(move || {
            // 先检查 24h 缓存（非强制时）
            if !force
                && let Some(cache) = tool_updater::read_check_cache()
                && tool_updater::is_cache_valid(&cache)
            {
                return Ok(CheckResult {
                    version: cache.latest_version.clone(),
                    download_url: String::new(),
                    changelog: Vec::new(),
                    cached: true,
                });
            }

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("创建 tokio runtime 失败：{e}"))?;

            rt.block_on(async {
                let info =
                    tool_updater::update_info::fetch_update_info(tool_updater::UPDATE_JSON_URL)
                        .await?;

                let had_update =
                    tool_updater::update_info::is_newer_version(&info.version, &current_version);

                // 写入缓存
                if let Err(e) = tool_updater::write_check_cache(&info.version, had_update) {
                    log::warn!("write_check_cache failed: {e}");
                }

                if !had_update {
                    return Ok(CheckResult {
                        version: info.version.clone(),
                        download_url: String::new(),
                        changelog: Vec::new(),
                        cached: false,
                    });
                }

                Ok(CheckResult {
                    version: info.version.clone(),
                    download_url: info.download_url.clone(),
                    changelog: info.changelog.clone(),
                    cached: false,
                })
            })
        }));
    }

    /// 启动后台下载更新线程。
    pub(crate) fn start_update_download(&mut self) {
        let url = match &self.update_state.download_url {
            Some(u) => u.clone(),
            None => {
                self.update_state.error = Some("无下载 URL".into());
                return;
            }
        };

        self.update_state.downloading = true;
        self.update_state.download_progress = 0.0;
        self.update_state.error = None;

        let progress = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let progress_clone = progress.clone();

        self.update_state.download_handle = Some(std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("创建 tokio runtime 失败：{e}"))?;

            rt.block_on(async {
                tool_updater::download_update(&url, move |downloaded, total| {
                    let pct = if total > 0 {
                        ((downloaded as f64 / total as f64) * 1000.0) as u64
                    } else {
                        0
                    };
                    progress_clone.store(pct, std::sync::atomic::Ordering::Relaxed);
                })
                .await
            })
        }));

        self.update_state_download_progress = Some(progress);
    }

    /// 用户手动触发检查更新（跳过 24h 缓存）。
    pub(crate) fn force_check_update(&mut self) {
        self.update_state.force_check = true;
        self.update_state.error = None;
        self.update_state.latest_version = None;
        self.update_state.update_available = false;
        self.update_state.changelog.clear();
    }

    pub(crate) fn tick_post_ui(&mut self, ctx: &egui::Context) {
        self.bottom_log_panel.ingest_pending();
        self.detached_dynamic_panel_viewports(ctx);
        self.send_popup(ctx);
        self.terminal_popup(ctx);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    fn measure_spin_precision(
        interval: Duration,
        samples: usize,
    ) -> (Duration, Duration, Duration) {
        // 提升实时优先级
        #[cfg(target_os = "windows")]
        unsafe {
            unsafe extern "system" {
                fn SetThreadPriority(thread: isize, priority: i32) -> i32;
                fn GetCurrentThread() -> isize;
            }
            SetThreadPriority(GetCurrentThread(), 15);
        }

        let start = Instant::now();
        let mut lates: Vec<Duration> = Vec::with_capacity(samples);
        for i in 0..samples {
            let deadline = start + interval * (i as u32 + 1);
            while Instant::now() < deadline {
                std::hint::spin_loop();
            }
            let now = Instant::now();
            if now > deadline {
                lates.push(now - deadline);
            }
        }

        if lates.is_empty() {
            return (Duration::ZERO, Duration::ZERO, Duration::ZERO);
        }

        let total: Duration = lates.iter().sum();
        let avg = total / lates.len() as u32;
        // P99：排除极端 OS 调度 spike（非实时 OS 偶尔会有 1-50ms 的调度延迟）
        let p99_index = (lates.len() * 99) / 100;
        lates.sort_unstable();
        (
            avg,
            lates[p99_index.min(lates.len() - 1)],
            lates[lates.len() - 1],
        )
    }

    #[test]
    #[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
    fn spin_wait_100us_precision() {
        let (avg, p99, max) = measure_spin_precision(Duration::from_micros(100), 1000);
        eprintln!(
            "100us: avg_late={}us p99_late={}us max_late={}us",
            avg.as_micros(),
            p99.as_micros(),
            max.as_micros()
        );
        assert!(
            p99 <= Duration::from_micros(500),
            "p99_late {}us > 500us",
            p99.as_micros()
        );
    }

    #[test]
    #[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
    fn spin_wait_1ms_precision() {
        let (avg, p99, max) = measure_spin_precision(Duration::from_millis(1), 1000);
        eprintln!(
            "1ms: avg_late={}us p99_late={}us max_late={}us",
            avg.as_micros(),
            p99.as_micros(),
            max.as_micros()
        );
        assert!(
            p99 <= Duration::from_millis(2),
            "p99_late {}us > 2ms",
            p99.as_micros()
        );
    }

    #[test]
    #[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
    fn spin_wait_10ms_precision() {
        let (avg, p99, max) = measure_spin_precision(Duration::from_millis(10), 500);
        eprintln!(
            "10ms: avg_late={}us p99_late={}us max_late={}us",
            avg.as_micros(),
            p99.as_micros(),
            max.as_micros()
        );
        assert!(
            p99 <= Duration::from_micros(300),
            "p99_late {}us > 300us",
            p99.as_micros()
        );
    }

    #[test]
    #[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
    fn spin_wait_100ms_precision() {
        let (avg, p99, max) = measure_spin_precision(Duration::from_millis(100), 100);
        eprintln!(
            "100ms: avg_late={}us p99_late={}us max_late={}us",
            avg.as_micros(),
            p99.as_micros(),
            max.as_micros()
        );
        assert!(
            p99 <= Duration::from_micros(300),
            "p99_late {}us > 300us",
            p99.as_micros()
        );
    }

    #[test]
    #[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
    fn spin_wait_no_drift() {
        #[cfg(target_os = "windows")]
        unsafe {
            unsafe extern "system" {
                fn SetThreadPriority(thread: isize, priority: i32) -> i32;
                fn GetCurrentThread() -> isize;
            }
            SetThreadPriority(GetCurrentThread(), 15);
        }
        let interval = Duration::from_millis(1);
        let samples = 1000;
        let start = Instant::now();
        for i in 0..samples {
            let deadline = start + interval * (i as u32 + 1);
            while Instant::now() < deadline {
                std::hint::spin_loop();
            }
        }
        let expected = interval * samples as u32;
        let elapsed = Instant::now().saturating_duration_since(start);
        let drift = elapsed.abs_diff(expected);
        eprintln!(
            "1000x1ms: expected={}ms actual={}ms drift={}us",
            expected.as_millis(),
            elapsed.as_millis(),
            drift.as_micros()
        );
        assert!(
            drift <= Duration::from_millis(5),
            "drift {}us",
            drift.as_micros()
        );
    }
}
