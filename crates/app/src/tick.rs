use crate::app::WorkbenchApp;
use crate::config::windows_open_dialog;
use crate::state::StatusLevel;
use eframe::egui;
use tool_core::{Direction, Event, Payload};
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
        match self.recorder.reap_stopping() {
            Some(Ok(path)) => {
                self.set_status_force(StatusLevel::Info, format!("录制已保存: {}", path.display()))
            }
            Some(Err(e)) => self.set_status_force(StatusLevel::Error, format!("录制失败: {e}")),
            None => {}
        }
        // 终端放大按钮：打开悬浮窗并切走底部 Terminal
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
        // 回放清理
        if self.replay_panel.want_clear_on_play {
            self.replay_panel.want_clear_on_play = false;
            self.terminal_panel.clear();
            self.bottom_log_panel.clear();
            self.dynamic_panels.clear_charts();
        }

        if let Some(steps) = self.replay_panel.want_step_backward.take() {
            self.terminal_panel.clear();
            self.bottom_log_panel.clear();
            self.dynamic_panels.clear_charts();

            self.bus.publish(Event::new(
                "ui.replay.reset",
                "ui.replay",
                Direction::Internal,
                Payload::Empty,
            ));

            let steps = steps.max(1);
            let pos = self.replay_panel.manager().backward_position_by(steps);

            if let Some(pos) = pos {
                // 阶段 1：先发布 ui.panel.create 并创建图表面板
                self.replay_panel.do_seek_panel_phase(pos);
                self.dynamic_panels.ingest(&mut self.panels);
                // 阶段 2：再发布数据事件
                self.replay_panel.do_seek_data_phase(pos);
            }

            let terminal_count = self.terminal_panel.ingest_all_pending();
            let log_count = self.bottom_log_panel.ingest_all_pending();
            let chart_count = self.dynamic_panels.ingest_all_pending();

            self.set_status(StatusLevel::Info, format!(
                "回放重建完成：接收 {terminal_count} 条，日志 {log_count} 条，图表 {chart_count} 条"
            ));
            ctx.request_repaint();
        }

        if let Some(p) = self.replay_panel.want_seek_replay.take() {
            self.terminal_panel.clear();
            self.bottom_log_panel.clear();
            self.dynamic_panels.clear_charts();

            self.bus.publish(Event::new(
                "ui.replay.reset",
                "ui.replay",
                Direction::Internal,
                Payload::Empty,
            ));

            // 阶段 1：先发布 ui.panel.create 并创建图表面板
            self.replay_panel.do_seek_panel_phase(p);
            self.dynamic_panels.ingest(&mut self.panels);
            // 阶段 2：再发布数据事件
            self.replay_panel.do_seek_data_phase(p);

            let terminal_count = self.terminal_panel.ingest_all_pending();
            let log_count = self.bottom_log_panel.ingest_all_pending();
            let chart_count = self.dynamic_panels.ingest_all_pending();

            self.set_status(StatusLevel::Info, format!(
                "回放重建完成：接收 {terminal_count} 条，日志 {log_count} 条，图表 {chart_count} 条"
            ));
            ctx.request_repaint();
        }
        if self.replay_panel.want_pick_file {
            self.replay_panel.want_pick_file = false;
            if let Some(p) = windows_open_dialog() {
                self.replay_panel.path = p.display().to_string();
                self.replay_panel.auto_load = true;
            }
        }
        // 运行 replay analyzer（后台线程，不卡 UI）
        if self.replay_panel.want_run_analyzers {
            self.launch_replay_analyzer_background();
        }
        // 取消后台 analyzer
        if self.replay_panel.want_cancel_analyzers {
            self.replay_panel.want_cancel_analyzers = false;
            if let Some(ref job) = self.replay_analyzer_job {
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                self.set_status(StatusLevel::Warn, "回放：正在取消 analyzer...");
            }
        }
        // 同步 analyzer 状态到 UI
        self.replay_panel.analyzer_busy = self
            .replay_analyzer_job
            .as_ref()
            .is_some_and(|job| !job.handle.is_finished());

        // 检查后台 analyzer 是否完成
        self.poll_replay_analyzer_result();

        // 录制状态检测：worker 线程因错误退出时反馈给 UI
        if let Some(error) = self.recorder.reap_error() {
            self.set_status(StatusLevel::Error, format!("录制失败：{error}"));
        }

        // 处理 dialog 请求（Lua ctx.dialog.open_file）
        self.poll_dialog_requests();

        // 处理 file 字段浏览请求
        self.handle_file_browse_requests();

        // 处理插件禁用后的资源清理
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
        let n = self.plugin_manager.process_pending();
        if n > 0 {
            self.set_status_force(StatusLevel::Info, format!("{n} 个插件事件"));
        }
        self.handle_keys(&ctx);

        // 速率统计
        let now = ctx.input(|i| i.time);
        if self.last_rate_check_time > 0.0 {
            let el = now - self.last_rate_check_time;
            if el >= 1.0 {
                let c = self.bus.published_count();
                self.event_rate = c.saturating_sub(self.last_event_count) as f64 / el;
                self.last_event_count = c;
                self.last_rate_check_time = now;
            }
        } else {
            self.last_rate_check_time = now;
            self.last_event_count = self.bus.published_count();
        }
        let refresh_interval = if ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
            0.5
        } else {
            2.0
        };
        if now - self.last_port_refresh > refresh_interval {
            self.last_port_refresh = now;
            self.refresh_ports_silent();
        }

        // 周期发送
        self.tick_periodic_send(ctx);
    }

    fn tick_periodic_send(&mut self, ctx: &egui::Context) {
        if !self.send.periodic_enabled {
            return;
        }

        let now = ctx.input(|i| i.time);
        if now < self.send.next_periodic_send_time {
            return;
        }

        let interval_ms = match self.send.periodic_interval_ms.trim().parse::<u64>() {
            Ok(v) if v >= 10 => v,
            _ => {
                self.send.periodic_enabled = false;
                self.set_status_force(crate::state::StatusLevel::Warn, "周期发送间隔必须 >= 10ms");
                return;
            }
        };

        if self.send_target_port_open() && !self.send.input.is_empty() {
            self.do_send();
            if self.send.error.is_none() {
                self.send.periodic_send_count += 1;
            } else {
                self.send.periodic_enabled = false;
                self.set_status_force(crate::state::StatusLevel::Error, "周期发送已停止：发送失败");
                return;
            }
        }

        self.send.next_periodic_send_time = now + interval_ms as f64 / 1000.0;
    }
    pub(crate) fn tick_post_ui(&mut self, ctx: &egui::Context) {
        self.bottom_log_panel.ingest_pending();
        self.detached_dynamic_panel_viewports(&ctx);
        self.send_popup(&ctx);
        self.terminal_popup(&ctx);
    }
}
