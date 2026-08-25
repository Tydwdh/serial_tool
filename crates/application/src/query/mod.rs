//! Query — 只读视图，UI 通过 `Workbench::query_*()` 读取状态。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tool_transport::{SerialPortDescriptor, TransportStatus};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkPortConfig {
    pub host: String,
    pub port: u16,
    pub api_key: Option<String>,
}

impl Default for NetworkPortConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 7125,
            api_key: None,
        }
    }
}

impl NetworkPortConfig {
    pub fn display_name(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

impl From<tool_transport::NetworkSerialConfig> for NetworkPortConfig {
    fn from(config: tool_transport::NetworkSerialConfig) -> Self {
        Self {
            host: config.host,
            port: config.port,
            api_key: config.api_key,
        }
    }
}

impl From<NetworkPortConfig> for tool_transport::NetworkSerialConfig {
    fn from(config: NetworkPortConfig) -> Self {
        Self {
            host: config.host,
            port: config.port,
            api_key: config.api_key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortTypeView {
    Usb(String),
    Bluetooth,
    Pci,
    Network,
    Unknown,
}

impl PortTypeView {
    pub fn is_network(&self) -> bool {
        matches!(self, Self::Network)
    }
}

impl std::fmt::Display for PortTypeView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usb(product) if !product.is_empty() => write!(f, "{product}"),
            Self::Usb(_) => write!(f, "USB"),
            Self::Bluetooth => write!(f, "Bluetooth"),
            Self::Pci => write!(f, "PCI"),
            Self::Network => write!(f, "网络"),
            Self::Unknown => Ok(()),
        }
    }
}

impl From<tool_transport::PortType> for PortTypeView {
    fn from(port_type: tool_transport::PortType) -> Self {
        match port_type {
            tool_transport::PortType::Usb(product) => Self::Usb(product),
            tool_transport::PortType::Bluetooth => Self::Bluetooth,
            tool_transport::PortType::Pci => Self::Pci,
            tool_transport::PortType::Network => Self::Network,
            tool_transport::PortType::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortView {
    pub port_name: String,
    pub port_type: PortTypeView,
}

impl From<SerialPortDescriptor> for PortView {
    fn from(port: SerialPortDescriptor) -> Self {
        Self {
            port_name: port.port_name,
            port_type: port.port_type.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportStatusView {
    pub open: bool,
    pub port_name: Option<String>,
    pub baud_rate: Option<u32>,
    pub connecting: bool,
}

impl TransportStatusView {
    pub fn closed() -> Self {
        Self {
            open: false,
            port_name: None,
            baud_rate: None,
            connecting: false,
        }
    }
}

impl From<TransportStatus> for TransportStatusView {
    fn from(status: TransportStatus) -> Self {
        Self {
            open: status.open,
            port_name: status.port_name,
            baud_rate: status.baud_rate,
            connecting: status.connecting,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialConfigView {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: String,
    pub stop_bits: String,
    pub parity: String,
}

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
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecordingStatusView {
    pub stats: RecorderStatsView,
    pub path: Option<PathBuf>,
    pub mode: RecordModeView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayStateView {
    Empty,
    Loaded,
    Playing,
    Paused,
    Finished,
}

impl From<tool_recorder::ReplayState> for ReplayStateView {
    fn from(state: tool_recorder::ReplayState) -> Self {
        match state {
            tool_recorder::ReplayState::Empty => Self::Empty,
            tool_recorder::ReplayState::Loaded => Self::Loaded,
            tool_recorder::ReplayState::Playing => Self::Playing,
            tool_recorder::ReplayState::Paused => Self::Paused,
            tool_recorder::ReplayState::Finished => Self::Finished,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayPolicyView {
    AutoPreferRecorded,
    ExactRecorded,
    ReparseRaw,
}

impl From<tool_recorder::ReplayPolicy> for ReplayPolicyView {
    fn from(policy: tool_recorder::ReplayPolicy) -> Self {
        match policy {
            tool_recorder::ReplayPolicy::AutoPreferRecorded => Self::AutoPreferRecorded,
            tool_recorder::ReplayPolicy::ExactRecorded => Self::ExactRecorded,
            tool_recorder::ReplayPolicy::ReparseRaw => Self::ReparseRaw,
        }
    }
}

impl From<ReplayPolicyView> for tool_recorder::ReplayPolicy {
    fn from(policy: ReplayPolicyView) -> Self {
        match policy {
            ReplayPolicyView::AutoPreferRecorded => Self::AutoPreferRecorded,
            ReplayPolicyView::ExactRecorded => Self::ExactRecorded,
            ReplayPolicyView::ReparseRaw => Self::ReparseRaw,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReplayLoadReportView {
    pub loaded: usize,
    pub skipped: usize,
    pub first_errors: Vec<String>,
}

impl From<tool_recorder::ReplayLoadReport> for ReplayLoadReportView {
    fn from(report: tool_recorder::ReplayLoadReport) -> Self {
        Self {
            loaded: report.loaded,
            skipped: report.skipped,
            first_errors: report.first_errors,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReplayStatusView {
    pub state: ReplayStateView,
    pub path: Option<PathBuf>,
    pub total_events: usize,
    pub cursor: usize,
    pub speed: f64,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub policy: ReplayPolicyView,
    pub effective_policy: ReplayPolicyView,
    pub has_recorded_protocol: bool,
    pub analyzer_cache_entries: usize,
    pub analyzer_error: Option<String>,
    pub analyzer_warning: Option<String>,
    pub load_report: Option<ReplayLoadReportView>,
}

#[derive(Debug, Clone)]
pub struct TransportView {
    pub ports: Vec<PortView>,
    pub open_ports: Vec<String>,
    pub statuses: Vec<TransportStatusView>,
    pub config: SerialConfigView,
    pub auto_reconnect: bool,
}

#[derive(Debug, Clone)]
pub struct PluginView {
    pub summaries: Vec<tool_extension::PluginSummary>,
    pub diagnostics: Vec<tool_extension::PluginDiagnostic>,
}
