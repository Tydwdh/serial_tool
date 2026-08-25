//! Application Command — 业务意图（不含 UI 操作）。

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

use crate::TaskId;
use tool_platform::SerialSettings;

/// 应用层命令：描述“我要做什么”，由 `Workbench::dispatch` 执行。
#[derive(Debug, Clone)]
pub enum AppCommand {
    RefreshPorts,

    /// Browser-only permission request. Native backends may implement this as
    /// a best-effort selection from already known ports.
    RequestPort,

    Connect {
        port_name: String,
        settings: SerialSettings,
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
        task_id: TaskId,
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

    #[cfg(not(target_arch = "wasm32"))]
    StartRecording {
        path: PathBuf,
    },

    #[cfg(not(target_arch = "wasm32"))]
    StopRecording,

    #[cfg(not(target_arch = "wasm32"))]
    PauseRecording,

    #[cfg(not(target_arch = "wasm32"))]
    ResumeRecording,

    #[cfg(not(target_arch = "wasm32"))]
    AddBookmark {
        name: Option<String>,
    },

    #[cfg(not(target_arch = "wasm32"))]
    LoadReplay {
        path: PathBuf,
    },

    #[cfg(not(target_arch = "wasm32"))]
    ReplayPlay,
    #[cfg(not(target_arch = "wasm32"))]
    ReplayPause,
    #[cfg(not(target_arch = "wasm32"))]
    ReplayStop,

    #[cfg(not(target_arch = "wasm32"))]
    ReplaySeek {
        position_ms: u64,
    },

    #[cfg(not(target_arch = "wasm32"))]
    ReplaySeekBy {
        delta_ms: i64,
    },

    #[cfg(not(target_arch = "wasm32"))]
    ReplayStep {
        delta: i32,
    },

    #[cfg(not(target_arch = "wasm32"))]
    SetReplaySpeed {
        speed: f64,
    },

    #[cfg(not(target_arch = "wasm32"))]
    SetReplayPolicy {
        policy: crate::query::ReplayPolicyView,
    },

    #[cfg(not(target_arch = "wasm32"))]
    EnablePlugin {
        plugin_id: String,
    },

    #[cfg(not(target_arch = "wasm32"))]
    DisablePlugin {
        plugin_id: String,
    },

    #[cfg(not(target_arch = "wasm32"))]
    ReloadPlugins,

    #[cfg(not(target_arch = "wasm32"))]
    DiscoverPlugins {
        roots: Vec<std::path::PathBuf>,
    },

    #[cfg(not(target_arch = "wasm32"))]
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

    #[cfg(not(target_arch = "wasm32"))]
    ExportTerminal {
        format: String,
        path: PathBuf,
    },

    #[cfg(not(target_arch = "wasm32"))]
    ExportLog {
        format: String,
        path: PathBuf,
    },
}

/// `dispatch` 的返回值：调用方可据此更新 UI 或展示错误。
#[derive(Debug, Clone)]
pub enum CommandOutcome {
    Done,
    /// 已触发异步操作（如重连），结果通过 Event 回传。
    Pending {
        task_id: TaskId,
        message: String,
    },
}
