//! Query — 只读视图，UI 通过 `Workbench::query()` 读取状态。

use std::path::PathBuf;
use tool_recorder::{RecorderStats, ReplayStatus};
use tool_transport::{SerialConfig, SerialPortDescriptor, TransportStatus};

/// 不可变只读视图：DTO，不含可变引用与 egui 类型。
#[derive(Debug, Clone)]
pub struct DeviceView {
    pub descriptor: SerialPortDescriptor,
    pub status: TransportStatus,
    pub alias: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RecordingStatusView {
    pub stats: RecorderStats,
    pub path: Option<PathBuf>,
    pub mode: tool_recorder::RecordMode,
}

#[derive(Debug, Clone)]
pub struct ReplayStatusView {
    pub status: Option<ReplayStatus>,
}

#[derive(Debug, Clone)]
pub struct TransportView {
    pub ports: Vec<SerialPortDescriptor>,
    pub open_ports: Vec<String>,
    pub statuses: Vec<TransportStatus>,
    pub config: SerialConfig,
    pub auto_reconnect: bool,
}

#[derive(Debug, Clone)]
pub struct PluginView {
    pub summaries: Vec<tool_extension::PluginSummary>,
    pub diagnostics: Vec<tool_extension::PluginDiagnostic>,
}
