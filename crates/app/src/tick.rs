use crate::app::WorkbenchApp;
use crate::config::windows_open_dialog;
use crate::state::StatusLevel;
use eframe::egui;
use tool_core::{Direction, Event, Payload};
use tool_transport::send_impl_to;
use tool_panels::Activity;

impl WorkbenchApp {
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            if i.modifiers.ctrl && i.key_pressed(egui::Key::R) && !i.modifiers.shift {
                self.refresh_ports();
            }
            if i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::O) {
                self.open_selected_port();
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::B) {
                self.toggle_bottom_panel();
            }
            if i.modifiers.ctrl {
                for (k, a) in [
                    (egui::Key::Num1, Activity::Devices),
                    (egui::Key::Num2, Activity::Replay),
                    (egui::Key::Num3, Activity::Plugins),
                    (egui::Key::Num4, Activity::Settings),
                ] {
                    if i.key_pressed(k) {
                        self.panels.select_activity(a);
                    }
                }
            }
        });
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
        // 检查是否被外部关闭
        if self.periodic_send_cancel.is_some() && !self.send.periodic_enabled {
            if let Some(cancel) = self.periodic_send_cancel.take() {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
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
            set_realtime_priority();
            let mut count: u64 = 0;
            let mut next = std::time::Instant::now() + interval;

            loop {
                // 纯 spin-wait 到 next 时刻，us 级精度。
                // 长间隔先用 sleep 省 CPU，最后 5ms 纯 spin 精确对齐。
                if interval > std::time::Duration::from_millis(10) {
                    loop {
                        let rem = next.saturating_duration_since(std::time::Instant::now());
                        if rem.is_zero() {
                            break;
                        }
                        if rem > std::time::Duration::from_millis(5) {
                            std::thread::sleep(rem - std::time::Duration::from_millis(5));
                        } else {
                            std::hint::spin_loop();
                        }
                        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                    }
                } else {
                    while next > std::time::Instant::now() {
                        std::hint::spin_loop();
                        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                    }
                }

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
                    bus.publish(tool_core::Event::system_log(
                        tool_core::LogLevel::Info,
                        "periodic",
                        format!("周期发送已完成 ({max} 次)"),
                    ));
                    return;
                }

                next += interval;
                if next <= std::time::Instant::now() {
                    next = std::time::Instant::now() + interval;
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

/// 将当前线程提升为实时优先级，减少 OS 调度延迟。
fn set_realtime_priority() {
    #[cfg(target_os = "windows")]
    {
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        }
        const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL); }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::set_realtime_priority;

    fn measure_spin_precision(interval: Duration, samples: usize) -> (Duration, Duration) {
        set_realtime_priority();
        let mut max_late = Duration::ZERO;
        let mut total_late = Duration::ZERO;
        let mut late_count = 0u64;

        let mut next = Instant::now() + interval;
        for _ in 0..samples {
            while next > Instant::now() {
                std::hint::spin_loop();
            }
            let now = Instant::now();
            if now > next {
                let late = now - next;
                total_late += late;
                late_count += 1;
                max_late = max_late.max(late);
            }
            next += interval;
            if next <= Instant::now() {
                next = Instant::now() + interval;
            }
        }
        let avg_late = if late_count > 0 {
            total_late / late_count as u32
        } else {
            Duration::ZERO
        };
        (avg_late, max_late)
    }

    #[test]
    fn spin_wait_100us_precision() {
        let (avg, max) = measure_spin_precision(Duration::from_micros(100), 1000);
        println!("100us: avg_late={}us max_late={}us", avg.as_micros(), max.as_micros());
        assert!(max <= Duration::from_micros(200), "max_late {}us > 200us", max.as_micros());
    }

    #[test]
    fn spin_wait_1ms_precision() {
        let (avg, max) = measure_spin_precision(Duration::from_millis(1), 1000);
        println!("1ms: avg_late={}us max_late={}us", avg.as_micros(), max.as_micros());
        assert!(max <= Duration::from_micros(300), "max_late {}us > 300us", max.as_micros());
    }

    #[test]
    fn spin_wait_10ms_precision() {
        let (avg, max) = measure_spin_precision(Duration::from_millis(10), 500);
        println!("10ms: avg_late={}us max_late={}us", avg.as_micros(), max.as_micros());
        assert!(max <= Duration::from_micros(300), "max_late {}us > 300us", max.as_micros());
    }

    #[test]
    fn spin_wait_100ms_precision() {
        let (avg, max) = measure_spin_precision(Duration::from_millis(100), 100);
        println!("100ms: avg_late={}us max_late={}us", avg.as_micros(), max.as_micros());
        assert!(max <= Duration::from_micros(300), "max_late {}us > 300us", max.as_micros());
    }

    #[test]
    fn spin_wait_no_drift() {
        set_realtime_priority();
        let interval = Duration::from_millis(1);
        let samples = 1000;
        let start = Instant::now();
        let mut next = start + interval;
        for _ in 0..samples {
            while next > Instant::now() {
                std::hint::spin_loop();
            }
            next += interval;
        }
        let expected = interval * samples as u32;
        let elapsed = Instant::now().saturating_duration_since(start);
        let drift = elapsed.abs_diff(expected);
        println!("1000x1ms: expected={}ms actual={}ms drift={}us", expected.as_millis(), elapsed.as_millis(), drift.as_micros());
        assert!(drift <= Duration::from_millis(5), "drift {}us", drift.as_micros());
    }
}
