use crate::app::WorkbenchApp;
use crate::config::windows_open_dialog;
use crate::state::StatusLevel;
use eframe::egui;
use tool_application::AppCommand;
use tool_application::query::{ReplayBlockReasonView, ReplayStateView};
use tool_core::{Direction, Event, Payload};
use tool_panels::ReplayUiCommand;

impl WorkbenchApp {
    /// 回放相关：应用命令、seek/step、文件选择和 analyzer 调度。
    pub(super) fn tick_replay(&mut self, ctx: &egui::Context) {
        self.dispatch_replay_commands(ctx);

        if self.replay_panel.want_clear_on_play {
            self.replay_panel.want_clear_on_play = false;
            self.terminal_panel.clear();
            self.bottom_log_panel.clear();
            self.dynamic_panels.clear_charts();
        }

        if let Some(steps) = self.replay_panel.want_step_backward.take() {
            self.do_replay_step_backward(steps, ctx);
        }

        if let Some(position) = self.replay_panel.want_seek_replay.take() {
            self.do_replay_seek_rebuild(position, ctx);
        }

        if self.replay_panel.want_pick_file {
            self.replay_panel.want_pick_file = false;
            if let Some(path) = windows_open_dialog() {
                self.replay_panel.path = path.display().to_string();
                self.replay_panel.auto_load = true;
            }
        }

        if self.replay_panel.want_run_analyzers {
            self.launch_replay_analyzer_background();
        }
        if self.replay_panel.want_cancel_analyzers {
            self.replay_panel.want_cancel_analyzers = false;
            if let Some(job) = &self.replay_analyzer.job {
                job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                self.set_status(StatusLevel::Warn, "回放：正在取消 analyzer...");
            }
        }
        self.replay_panel.analyzer_busy = self
            .replay_analyzer
            .job
            .as_ref()
            .and_then(|job| job.handle.as_ref())
            .is_some_and(|handle| !handle.is_finished());

        self.poll_replay_analyzer_result();
        self.replay_panel
            .set_load_pending(self.workbench.has_active_task_kind("load_replay"));

        let was_playing = self.workbench.query_replay().state == ReplayStateView::Playing;
        let published = self.workbench.tick_replay();
        let mut loop_restarted = false;
        let status = self.workbench.query_replay();
        if self.replay_panel.loop_playback && status.state == ReplayStateView::Finished {
            self.replay_panel.want_clear_on_play = true;
            let _ = self.workbench.dispatch(AppCommand::ReplayStop);
            let _ = self.workbench.dispatch(AppCommand::ReplayPlay);
            loop_restarted = true;
        }

        if was_playing || published > 0 || loop_restarted {
            ctx.request_repaint();
        }
    }

    fn dispatch_replay_commands(&mut self, ctx: &egui::Context) {
        for command in self.replay_panel.take_commands() {
            let app_command = match command {
                ReplayUiCommand::Load { path } => {
                    self.replay_panel.message = Some(format!("正在后台加载 {path}…"));
                    AppCommand::LoadReplay {
                        file: tool_platform::storage::FileHandle::from_native_path(
                            std::path::PathBuf::from(path),
                        ),
                    }
                }
                ReplayUiCommand::Play => AppCommand::ReplayPlay,
                ReplayUiCommand::Pause => AppCommand::ReplayPause,
                ReplayUiCommand::Stop => AppCommand::ReplayStop,
                ReplayUiCommand::Seek { position_ms } => AppCommand::ReplaySeek { position_ms },
                ReplayUiCommand::SetSpeed(speed) => AppCommand::SetReplaySpeed { speed },
                ReplayUiCommand::SetPolicy(policy) => AppCommand::SetReplayPolicy { policy },
                ReplayUiCommand::AddReplayBookmark { name } => {
                    AppCommand::AddReplayBookmark { name }
                }
                ReplayUiCommand::RemoveReplayBookmark { position_ms } => {
                    AppCommand::RemoveReplayBookmark { position_ms }
                }
                ReplayUiCommand::SeekCursorDataPhase { target_cursor } => {
                    let current = self.workbench.query_replay().cursor;
                    if target_cursor > current {
                        AppCommand::ReplayStep {
                            delta: (target_cursor - current).min(i32::MAX as usize) as i32,
                        }
                    } else {
                        continue;
                    }
                }
                ReplayUiCommand::StepBackward { steps } => {
                    self.replay_panel.want_step_backward = Some(steps);
                    continue;
                }
                ReplayUiCommand::SeekPanelPhase { position_ms }
                | ReplayUiCommand::SeekDataPhase { position_ms } => {
                    self.replay_panel.want_seek_replay = Some(position_ms);
                    continue;
                }
                ReplayUiCommand::SeekCursorPanelPhase { target_cursor } => {
                    let current = self.workbench.query_replay().cursor;
                    if target_cursor < current {
                        self.do_replay_cursor_rebuild(target_cursor, ctx);
                    } else if target_cursor > current {
                        let delta = (target_cursor - current).min(i32::MAX as usize) as i32;
                        if let Err(error) =
                            self.workbench.dispatch(AppCommand::ReplayStep { delta })
                        {
                            self.set_status(StatusLevel::Error, format!("回放步进失败：{error}"));
                        }
                    }
                    continue;
                }
                ReplayUiCommand::PickFile
                | ReplayUiCommand::SetLoop(_)
                | ReplayUiCommand::SetStepSize(_)
                | ReplayUiCommand::SetAnalyzerCache(_)
                | ReplayUiCommand::SetAnalyzerError(_)
                | ReplayUiCommand::SetAnalyzerWarning(_)
                | ReplayUiCommand::ClearAnalyzerError
                | ReplayUiCommand::PushAnalyzerLog(_) => continue,
            };

            if let Err(error) = self.workbench.dispatch(app_command) {
                self.set_status(StatusLevel::Error, format!("回放操作失败：{error}"));
            }
        }
    }

    fn do_replay_step_backward(&mut self, steps: usize, ctx: &egui::Context) {
        let status = self.workbench.query_replay();
        if status.cursor == 0 {
            return;
        }
        if let Some(target_cursor) = self.workbench.replay_backward_cursor_by(steps.max(1)) {
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
        self.rebuild_replay(ctx, |app| {
            if let Some(position) = position {
                app.workbench.replay_seek_panel_phase(position);
                app.dynamic_panels.ingest(&mut app.panels);
                app.workbench.replay_seek_data_phase(position);
            }
        });
    }

    fn do_replay_cursor_rebuild(&mut self, target_cursor: usize, ctx: &egui::Context) {
        if self.warn_if_replay_blocked() {
            return;
        }
        self.rebuild_replay(ctx, |app| {
            app.workbench.replay_seek_cursor_panel_phase(target_cursor);
            app.dynamic_panels.ingest(&mut app.panels);
            app.workbench.replay_seek_cursor_data_phase(target_cursor);
        });
    }

    /// 回放重建的共用骨架：清理面板 → 发 reset → 执行 seek 阶段 → ingest。
    fn rebuild_replay(&mut self, ctx: &egui::Context, seek_phase: impl FnOnce(&mut Self)) {
        self.terminal_panel.clear();
        self.bottom_log_panel.clear();
        self.dynamic_panels.clear_charts();
        self.workbench.publish_event(Event::new(
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
        let Some(reason) = self.workbench.query_replay().block_reason else {
            return false;
        };
        let message = match reason {
            ReplayBlockReasonView::NeedAnalyzer => {
                "当前回放策略需要先完成 Replay Analyzer".to_owned()
            }
            ReplayBlockReasonView::AnalyzerFailed(error) => {
                format!("Replay Analyzer 失败，无法重建回放：{error}")
            }
        };
        self.set_status(StatusLevel::Warn, message);
        true
    }
}
