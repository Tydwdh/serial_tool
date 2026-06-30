use crate::app::WorkbenchApp;
use crate::config::windows_open_dialog;
use crate::state::StatusLevel;
use eframe::egui;
use tool_core::{Direction, Event, Payload};
use tool_recorder::{ReplayBlockReason, ReplayState};

impl WorkbenchApp {
    /// 回放相关：seek/step/pick_file/analyzer 调度。
    pub(super) fn tick_replay(&mut self, ctx: &egui::Context) {
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
}
