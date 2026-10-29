//! Application Command — 业务意图（不含 UI 操作）。

mod execute;

pub use execute::{CommandEffects, execute};

use crate::TaskId;
use thiserror::Error;
use tool_platform::{NetworkSerialConfig, PortId, SerialSettings};

/// Platform-neutral command failure.
///
/// `Display` is the bare Chinese body; the native composition root wraps it in
/// `AppError` to add its classification prefix.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CommandError {
    #[error("{0}")]
    Transport(String),
    #[error("{0}")]
    Storage(String),
    #[error("{0}")]
    Plugin(String),
    #[error("{0}")]
    InvalidState(String),
    #[error("{0}")]
    InvalidCommand(String),
}

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
