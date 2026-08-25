//! Application Command — 业务意图（不含 UI 操作）。

use std::path::PathBuf;

/// 应用层命令：描述“我要做什么”，由 `Workbench::dispatch` 执行。
#[derive(Debug, Clone)]
pub enum AppCommand {
    RefreshPorts,

    Connect {
        port_name: String,
    },

    Disconnect {
        port_name: String,
    },

    Reconnect {
        port_name: String,
    },

    CancelReconnect {
        port_name: String,
    },

    CancelTask {
        task_id: crate::task::TaskId,
    },

    SendText {
        port_name: String,
        text: String,
    },

    SendHex {
        port_name: String,
        hex: String,
    },

    SendRaw {
        port_name: String,
        bytes: Vec<u8>,
    },

    SetDtr {
        port_name: String,
        value: bool,
    },

    SetRts {
        port_name: String,
        value: bool,
    },

    StartRecording {
        path: PathBuf,
    },

    StopRecording,

    PauseRecording,

    ResumeRecording,

    AddBookmark {
        name: Option<String>,
    },

    LoadReplay {
        path: PathBuf,
    },

    ReplayPlay,
    ReplayPause,
    ReplayStop,

    ReplaySeek {
        position_ms: u64,
    },

    ReplaySeekBy {
        delta_ms: i64,
    },

    ReplayStep {
        delta: i32,
    },

    SetReplaySpeed {
        speed: f64,
    },

    SetReplayPolicy {
        policy: crate::query::ReplayPolicyView,
    },

    EnablePlugin {
        plugin_id: String,
    },

    DisablePlugin {
        plugin_id: String,
    },

    ReloadPlugins,

    DiscoverPlugins {
        roots: Vec<std::path::PathBuf>,
    },

    ExecutePluginCommand {
        plugin_id: String,
        command_id: String,
        context: serde_json::Value,
    },

    ClearTerminal,

    SetTerminalMergeWindow {
        ms: u64,
    },

    SetTerminalMaxEntries {
        max: usize,
    },

    ExportTerminal {
        format: String,
        path: std::path::PathBuf,
    },

    ExportLog {
        format: String,
        path: std::path::PathBuf,
    },
}

/// `dispatch` 的返回值：调用方可据此更新 UI 或展示错误。
#[derive(Debug, Clone)]
pub enum CommandOutcome {
    Done,
    /// 已触发异步操作（如重连），结果通过 Event 回传。
    Pending {
        task_id: crate::task::TaskId,
        message: String,
    },
}
