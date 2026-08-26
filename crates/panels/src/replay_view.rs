use tool_application::replay::{
    ReplayBlockReasonView, ReplayPolicyView, ReplayStateView, ReplayStatusView,
};

/// Panel 侧的回放只读视图 — 不持有 ReplayManager，直接由 Workbench 提供。
#[derive(Debug, Clone)]
pub struct ReplayView {
    pub path: String,
    pub speed: f64,
    pub loop_playback: bool,
    pub step_size: usize,
    pub state: ReplayStateView,
    pub total_events: usize,
    pub cursor: usize,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub policy: ReplayPolicyView,
    pub effective_policy: ReplayPolicyView,
    pub has_recorded_protocol: bool,
    pub analyzer_cache_valid: bool,
    pub analyzer_error: Option<String>,
    pub analyzer_warning: Option<String>,
    pub can_play: bool,
    pub can_seek: bool,
    pub block_reason: Option<ReplayBlockReasonView>,
    pub message: Option<String>,
}

impl Default for ReplayView {
    fn default() -> Self {
        Self {
            path: "logs/session.jsonl".to_owned(),
            speed: 1.0,
            loop_playback: false,
            step_size: 1,
            state: ReplayStateView::Empty,
            total_events: 0,
            cursor: 0,
            position_ms: 0,
            duration_ms: 0,
            policy: ReplayPolicyView::AutoPreferRecorded,
            effective_policy: ReplayPolicyView::AutoPreferRecorded,
            has_recorded_protocol: false,
            analyzer_cache_valid: false,
            analyzer_error: None,
            analyzer_warning: None,
            can_play: false,
            can_seek: false,
            block_reason: None,
            message: None,
        }
    }
}

impl From<&ReplayStatusView> for ReplayView {
    fn from(status: &ReplayStatusView) -> Self {
        Self {
            path: status.path.clone().unwrap_or_default(),
            speed: status.speed,
            loop_playback: false,
            step_size: 1,
            state: status.state,
            total_events: status.total_events,
            cursor: status.cursor,
            position_ms: status.position_ms,
            duration_ms: status.duration_ms,
            policy: status.policy,
            effective_policy: status.effective_policy,
            has_recorded_protocol: status.has_recorded_protocol,
            analyzer_cache_valid: status.analyzer_cache_valid,
            analyzer_error: status.analyzer_error.clone(),
            analyzer_warning: status.analyzer_warning.clone(),
            can_play: status.can_play,
            can_seek: status.can_seek,
            block_reason: status.block_reason.clone(),
            message: None,
        }
    }
}

/// Panel 向上回传的回放意图 — 由 Workbench::dispatch 承接。
#[derive(Debug, Clone)]
pub enum ReplayUiCommand {
    PickFile,
    Load { path: String },
    Play,
    Pause,
    Stop,
    Seek { position_ms: u64 },
    StepBackward { steps: usize },
    SeekPanelPhase { position_ms: u64 },
    SeekDataPhase { position_ms: u64 },
    SeekCursorPanelPhase { target_cursor: usize },
    SeekCursorDataPhase { target_cursor: usize },
    SetSpeed(f64),
    SetPolicy(ReplayPolicyView),
    AddReplayBookmark { name: Option<String> },
    RemoveReplayBookmark { position_ms: u64 },
    SetLoop(bool),
    SetStepSize(usize),
    SetAnalyzerCache(Vec<tool_core::Event>),
    SetAnalyzerError(String),
    SetAnalyzerWarning(String),
    ClearAnalyzerError,
    PushAnalyzerLog(String),
}
