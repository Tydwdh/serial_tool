//! Application Command — 业务意图（不含 UI 操作）。

use crate::TaskId;
use tool_platform::storage::FileHandle;
use tool_platform::{NetworkSerialConfig, PortId, SerialSettings};

/// 应用层命令：描述“我要做什么”，由 `Workbench::dispatch` 执行。
#[derive(Debug, Clone)]
pub enum AppCommand {
    RefreshPorts,

    /// Browser-only permission request. Native backends may implement this as
    /// a best-effort selection from already known ports.
    RequestPort,

    /// Register a user-configured network endpoint in the platform port list.
    /// Native and Web compositions can then use the same connect/send flow.
    RegisterNetworkPort {
        config: NetworkSerialConfig,
    },

    RemoveNetworkPort {
        port: PortId,
    },

    Connect {
        port: PortId,
        settings: SerialSettings,
    },

    /// Update the active serial line parameters used by subsequent connects
    /// and reconnects.  Keeping this as an application command prevents the
    /// presentation layer from mutating a platform-specific config mirror.
    SetSerialSettings {
        settings: SerialSettings,
    },

    Disconnect {
        port: PortId,
    },

    Reconnect {
        port: PortId,
    },

    CancelReconnect {
        port: PortId,
    },

    CancelTask {
        task_id: TaskId,
    },

    SendText {
        port: PortId,
        text: String,
    },

    SendHex {
        port: PortId,
        hex: String,
        /// Reject odd/single-nibble tokens when enabled.
        strict: bool,
    },

    SendRaw {
        port: PortId,
        bytes: Vec<u8>,
    },

    SetDtr {
        port: PortId,
        value: bool,
    },

    SetRts {
        port: PortId,
        value: bool,
    },

    StartRecording {
        /// The platform-specific FileService owns the actual handle/path
        /// semantics. Native carries an opaque path-backed handle; Web keeps
        /// only the browser download name.
        file: FileHandle,
        mode: crate::recording::RecordModeView,
    },

    /// Change the recorder format for the next/current recording. The
    /// backend owns the actual recorder implementation; the UI only submits
    /// the platform-neutral mode.
    SetRecordingMode {
        mode: crate::recording::RecordModeView,
    },

    StopRecording,

    PauseRecording,

    ResumeRecording,

    AddBookmark {
        name: Option<String>,
    },
    AddReplayBookmark {
        name: Option<String>,
    },

    #[cfg(not(target_arch = "wasm32"))]
    LoadReplay {
        file: FileHandle,
    },
    #[cfg(target_arch = "wasm32")]
    LoadReplayText {
        name: String,
        text: String,
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
    RemoveReplayBookmark {
        position_ms: u64,
    },

    SetReplaySpeed {
        speed: f64,
    },

    SetReplayPolicy {
        policy: crate::replay::ReplayPolicyView,
    },

    EnablePlugin {
        plugin_id: String,
    },

    DisablePlugin {
        plugin_id: String,
    },

    ReloadPlugins,

    RefreshMarketplace {
        url: String,
    },

    /// Install the same plugin.json + main.lua sources published by a
    /// marketplace registry. Application owns the asynchronous fetch task.
    #[cfg(target_arch = "wasm32")]
    InstallMarketplacePlugin {
        plugin_id: String,
        manifest_url: String,
        main_url: String,
    },

    CheckForUpdate,

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

    #[cfg(not(target_arch = "wasm32"))]
    ExportTerminal {
        format: String,
        file: FileHandle,
    },

    #[cfg(not(target_arch = "wasm32"))]
    ExportLog {
        format: String,
        file: FileHandle,
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
