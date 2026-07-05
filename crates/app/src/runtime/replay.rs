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
            if let Some(ref job) = self.replay_analyzer.job {
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                self.set_status(StatusLevel::Warn, "回放：正在取消 analyzer...");
            }
        }
        self.replay_panel.analyzer_busy = self
            .replay_analyzer
            .job
            .as_ref()
            .and_then(|job| job.handle.as_ref())
            .is_some_and(|h| !h.is_finished());

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
        // 与 cursor 版本不同：仅在 position.is_some() 时检查 block。
        // None 表示走整段重建路径，不触发 seek 阻断判定。
        if position.is_some() && self.warn_if_replay_blocked() {
            return;
        }
        let pos = position;
        self.rebuild_replay(ctx, |app| {
            if let Some(pos) = pos {
                app.replay_panel.do_seek_panel_phase(pos);
                app.dynamic_panels.ingest(&mut app.panels);
                app.replay_panel.do_seek_data_phase(pos);
            }
        });
    }

    fn do_replay_cursor_rebuild(&mut self, target_cursor: usize, ctx: &egui::Context) {
        if self.warn_if_replay_blocked() {
            return;
        }
        self.rebuild_replay(ctx, |app| {
            app.replay_panel.do_seek_cursor_panel_phase(target_cursor);
            app.dynamic_panels.ingest(&mut app.panels);
            app.replay_panel.do_seek_cursor_data_phase(target_cursor);
        });
    }

    /// 回放重建的共用骨架：清理面板 → 发 reset → 执行 seek 阶段（由调用方闭包提供）
    /// → ingest 全部待处理事件 → 状态栏汇报 → 请求重绘。
    ///
    /// `seek_phase` 闭包接收 `&mut Self`，内部完成 seek 到目标位置并触发数据注入。
    fn rebuild_replay(&mut self, ctx: &egui::Context, seek_phase: impl FnOnce(&mut Self)) {
        self.terminal_panel.clear();
        self.bottom_log_panel.clear();
        self.dynamic_panels.clear_charts();

        self.bus.publish(Event::new(
            "ui.replay.reset",
            "ui.replay",
            Direction::Internal,
            Payload::Empty,
        ));

        seek_phase(self);

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
