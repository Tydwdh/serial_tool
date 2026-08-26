//! Platform-neutral recording DTOs.
//!
//! The recorder backend differs between Native (filesystem writer) and Web
//! (lossless in-memory buffer followed by a browser download), but the UI and
//! Application command boundary must observe the same state model.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordModeView {
    RawSerial,
    StandardReplay,
}

impl From<tool_recorder::RecordMode> for RecordModeView {
    fn from(mode: tool_recorder::RecordMode) -> Self {
        match mode {
            tool_recorder::RecordMode::RawSerial => Self::RawSerial,
            tool_recorder::RecordMode::StandardReplay => Self::StandardReplay,
        }
    }
}

impl From<RecordModeView> for tool_recorder::RecordMode {
    fn from(mode: RecordModeView) -> Self {
        match mode {
            RecordModeView::RawSerial => Self::RawSerial,
            RecordModeView::StandardReplay => Self::StandardReplay,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RecorderStatsView {
    pub events_written: u64,
    pub bytes_written: u64,
    pub last_flush_elapsed_ms: u64,
    pub last_error: Option<String>,
    pub running: bool,
    pub stopping: bool,
    pub paused: bool,
    pub pause_count: u64,
    pub incomplete: bool,
    pub stop_reason: Option<String>,
    pub backlog_events: u64,
    pub backlog_bytes: u64,
    pub seconds_behind: f64,
}

impl From<tool_recorder::RecorderStats> for RecorderStatsView {
    fn from(stats: tool_recorder::RecorderStats) -> Self {
        Self {
            events_written: stats.events_written,
            bytes_written: stats.bytes_written,
            last_flush_elapsed_ms: stats.last_flush_elapsed_ms,
            last_error: stats.last_error,
            running: stats.running,
            stopping: stats.stopping,
            paused: stats.paused,
            pause_count: stats.pause_count,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecordingStatusView {
    pub stats: RecorderStatsView,
    /// Native returns a display path; Web returns the browser download name.
    pub path: Option<String>,
    pub mode: RecordModeView,
}
