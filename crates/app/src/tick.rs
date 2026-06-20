use crate::app::WorkbenchApp;
use crate::config::windows_open_dialog;
use crate::keymap::Action;
use crate::state::StatusLevel;
use eframe::egui;
use tool_core::{Direction, Event, Payload};
use tool_transport::send_impl_to;
use tool_panels::Activity;

impl WorkbenchApp {
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        let keymap = self.keymap.clone();
        let mut triggered: Option<Action> = None;

        ctx.input(|i| {
            for (action, bindings) in &keymap.bindings {
                for binding in bindings {
                    if let Some(key) = parse_egui_key(&binding.key) {
                        let mods_match = i.modifiers.ctrl == binding.ctrl
                            && i.modifiers.shift == binding.shift
                            && i.modifiers.alt == binding.alt;
                        if mods_match && i.key_pressed(key) {
                            triggered = Some(*action);
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
            Action::OpenPort => self.open_selected_port(),
            Action::ToggleActivityBar => {
                self.panels.dock.activity_bar_visible = !self.panels.dock.activity_bar_visible;
                if let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
            }
            Action::ToggleBottomPanel => self.toggle_bottom_panel(),
            Action::ToggleRightSidebar => {
                self.panels.dock.right_visible = !self.panels.dock.right_visible;
                if let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
            }
            Action::SelectActivity1 => self.panels.select_activity(Activity::Devices),
            Action::SelectActivity2 => self.panels.select_activity(Activity::Replay),
            Action::SelectActivity3 => self.panels.select_activity(Activity::Plugins),
            Action::SelectActivity4 => self.panels.select_activity(Activity::Settings),
            Action::Send => {
                if self.send_target_port_open() && !self.send.input.trim().is_empty() {
                    self.do_send();
                }
            }
            Action::StartRecording => self.start_or_stop_recording(),
            Action::ReconnectPort => self.reconnect_selected_port(),
        }
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
        self.tick_port_refresh(ctx);
        self.tick_periodic_send(ctx);
        self.tick_auto_save(ctx);
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
            if let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
        }
    }

    fn do_replay_step_backward(&mut self, steps: usize, ctx: &egui::Context) {
        let pos = self
            .replay_panel
            .manager()
            .backward_position_by(steps.max(1));
        self.do_replay_rebuild(pos, ctx);
    }

    fn do_replay_seek_rebuild(&mut self, position: u64, ctx: &egui::Context) {
        self.do_replay_rebuild(Some(position), ctx);
    }

    fn do_replay_rebuild(&mut self, position: Option<u64>, ctx: &egui::Context) {
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
            self.set_status_force(crate::state::StatusLevel::Error, "周期发送已停止：目标串口未打开");
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
                unsafe extern "system" { fn SetThreadPriority(thread: isize, priority: i32) -> i32; fn GetCurrentThread() -> isize; }
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

    fn measure_spin_precision(interval: Duration, samples: usize) -> (Duration, Duration, Duration) {
        // 提升实时优先级
        #[cfg(target_os = "windows")]
        unsafe {
            unsafe extern "system" { fn SetThreadPriority(thread: isize, priority: i32) -> i32; fn GetCurrentThread() -> isize; }
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
        (avg, lates[p99_index.min(lates.len() - 1)], lates[lates.len() - 1])
    }

    #[test]
    fn spin_wait_100us_precision() {
        let (avg, p99, max) = measure_spin_precision(Duration::from_micros(100), 1000);
        eprintln!("100us: avg_late={}us p99_late={}us max_late={}us",
            avg.as_micros(), p99.as_micros(), max.as_micros());
        assert!(p99 <= Duration::from_micros(500), "p99_late {}us > 500us", p99.as_micros());
    }

    #[test]
    fn spin_wait_1ms_precision() {
        let (avg, p99, max) = measure_spin_precision(Duration::from_millis(1), 1000);
        eprintln!("1ms: avg_late={}us p99_late={}us max_late={}us",
            avg.as_micros(), p99.as_micros(), max.as_micros());
        assert!(p99 <= Duration::from_millis(2), "p99_late {}us > 2ms", p99.as_micros());
    }

    #[test]
    fn spin_wait_10ms_precision() {
        let (avg, p99, max) = measure_spin_precision(Duration::from_millis(10), 500);
        eprintln!("10ms: avg_late={}us p99_late={}us max_late={}us",
            avg.as_micros(), p99.as_micros(), max.as_micros());
        assert!(p99 <= Duration::from_micros(300), "p99_late {}us > 300us", p99.as_micros());
    }

    #[test]
    fn spin_wait_100ms_precision() {
        let (avg, p99, max) = measure_spin_precision(Duration::from_millis(100), 100);
        eprintln!("100ms: avg_late={}us p99_late={}us max_late={}us",
            avg.as_micros(), p99.as_micros(), max.as_micros());
        assert!(p99 <= Duration::from_micros(300), "p99_late {}us > 300us", p99.as_micros());
    }

    #[test]
    fn spin_wait_no_drift() {
        #[cfg(target_os = "windows")]
        unsafe {
            unsafe extern "system" { fn SetThreadPriority(thread: isize, priority: i32) -> i32; fn GetCurrentThread() -> isize; }
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
        eprintln!("1000x1ms: expected={}ms actual={}ms drift={}us",
            expected.as_millis(), elapsed.as_millis(), drift.as_micros());
        assert!(drift <= Duration::from_millis(5), "drift {}us", drift.as_micros());
    }
}
