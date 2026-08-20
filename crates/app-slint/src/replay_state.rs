use std::collections::VecDeque;

use tool_databus::DataBus;
use tool_recorder::{ReplayManager, ReplayPolicy, ReplayState};

pub struct ReplayUiState {
    pub manager: ReplayManager,
    pub path: String,
    pub speed: f64,
    pub loop_playback: bool,
    pub step_size: usize,
    pub message: Option<String>,
    pub want_pick_file: bool,
    pub auto_load: bool,
    pub want_clear_on_play: bool,
    pub want_seek_replay: Option<u64>,
    pub want_step_backward: Option<usize>,
    pub want_run_analyzers: bool,
    pub want_cancel_analyzers: bool,
    pub analyzer_busy: bool,
    pub analyzer_logs: VecDeque<String>,
}

impl ReplayUiState {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            manager: ReplayManager::new(bus.clone()),
            path: "logs/session.jsonl".to_owned(),
            speed: 1.0,
            loop_playback: false,
            step_size: 1,
            message: None,
            want_pick_file: false,
            auto_load: false,
            want_clear_on_play: false,
            want_seek_replay: None,
            want_step_backward: None,
            want_run_analyzers: false,
            want_cancel_analyzers: false,
            analyzer_busy: false,
            analyzer_logs: VecDeque::new(),
        }
    }

    pub fn try_load(&mut self) {
        match self.manager.load(&self.path) {
            Ok(count) => {
                let effective = self.manager.effective_policy();
                let mut msg = format!("已加载 {count} 个事件");
                if effective == ReplayPolicy::ReparseRaw {
                    msg.push_str(" (需要运行 analyzer)");
                }
                self.message = Some(msg);
                self.manager.set_speed(self.speed);
                self.want_run_analyzers = self.manager.needs_analyzer();
            }
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    pub fn do_seek_replay(&mut self, pos: u64) {
        self.manager.seek_with_replay(pos);
    }
    pub fn do_seek_panel_phase(&mut self, pos: u64) -> usize {
        self.manager.seek_panel_phase(pos)
    }
    pub fn do_seek_data_phase(&mut self, pos: u64) -> usize {
        self.manager.seek_data_phase(pos)
    }
    pub fn do_step_backward(&mut self, steps: usize) {
        let steps = steps.max(1);
        if let Some(cur) = self.manager.backward_cursor_by(steps) {
            self.manager.seek_cursor_with_replay(cur);
        }
    }
    pub fn tick_playback(&mut self) -> (usize, bool) {
        let published = self.manager.tick();
        if published > 0 {
            self.message = Some(format!("回放中 ({published} 事件)"));
        }
        let mut loop_restarted = false;
        if self.loop_playback && self.manager.status().state == ReplayState::Finished {
            self.want_clear_on_play = true;
            self.manager.stop();
            self.manager.play();
            loop_restarted = true;
        }
        (published, loop_restarted)
    }
    pub fn push_analyzer_log(&mut self, msg: impl Into<String>) {
        self.analyzer_logs.push_back(msg.into());
        while self.analyzer_logs.len() > 200 {
            self.analyzer_logs.pop_front();
        }
    }
    pub fn set_analyzer_cache(&mut self, events: Vec<tool_core::Event>) {
        let status = self.manager.status();
        self.manager.set_analyzer_cache(events);
        self.want_run_analyzers = false;
        if status.total_events > 0 && status.state != ReplayState::Playing {
            self.want_seek_replay = Some(status.position_ms);
        }
    }
    pub fn set_analyzer_error(&mut self, e: String) {
        self.manager.set_analyzer_error(e);
        self.want_run_analyzers = false;
    }

    pub fn progress01(&self) -> f32 {
        let s = self.manager.status();
        if s.duration_ms == 0 || s.total_events == 0 {
            0.0
        } else {
            (s.position_ms as f32 / s.duration_ms as f32).clamp(0.0, 1.0)
        }
    }
    pub fn status_text(&self) -> String {
        if let Some(m) = &self.message {
            return m.clone();
        }
        let s = self.manager.status();
        format!("{:?} {}/{} @{:.2}x", s.state, s.cursor, s.total_events, s.speed)
    }
}
