//! Terminal Read Model — 增量查询用。

use serde::{Deserialize, Serialize};

/// 单调递增的终端条目序号，用于增量查询。
pub type TerminalSeq = u64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalEntry {
    pub seq: TerminalSeq,
    pub event_id: u64,
    pub timestamp_ms: u64,
    pub port: String,
    pub direction: tool_core::Direction,
    pub raw_text: String,
    pub display_text: String,
    pub hex_text: String,
    pub preview_text: String,
}

#[derive(Debug, Clone)]
pub struct TerminalDelta {
    pub entries: Vec<TerminalEntry>,
    pub next_seq: TerminalSeq,
    pub truncated: bool,
    pub dropped: u64,
}
