//! Platform-neutral replay presentation DTOs.
//!
//! The native replay manager and the browser replay runtime have different
//! storage and execution backends, but the UI contract is the same. Keeping
//! these types outside the native-only query module lets both composition
//! roots render the same replay panel.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayStateView {
    Empty,
    Loaded,
    Playing,
    Paused,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayPolicyView {
    AutoPreferRecorded,
    ExactRecorded,
    ReparseRaw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayBlockReasonView {
    NeedAnalyzer,
    AnalyzerFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayBookmarkView {
    pub position_ms: u64,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ReplayLoadReportView {
    pub loaded: usize,
    pub skipped: usize,
    pub first_errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ReplayStatusView {
    pub state: ReplayStateView,
    pub path: Option<String>,
    pub total_events: usize,
    pub cursor: usize,
    pub speed: f64,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub policy: ReplayPolicyView,
    pub effective_policy: ReplayPolicyView,
    pub has_recorded_protocol: bool,
    pub analyzer_cache_entries: usize,
    pub analyzer_cache_valid: bool,
    pub analyzer_error: Option<String>,
    pub analyzer_warning: Option<String>,
    pub can_play: bool,
    pub can_seek: bool,
    pub block_reason: Option<ReplayBlockReasonView>,
    pub bookmarks: Vec<ReplayBookmarkView>,
    pub load_report: Option<ReplayLoadReportView>,
}
