use crate::app::StatusLevel;

#[derive(Clone)]
pub(crate) struct StatusState {
    pub(crate) message: String,
    pub(crate) level: StatusLevel,
    pub(crate) deadline_ms: u64,
}

impl Default for StatusState {
    fn default() -> Self {
        Self {
            message: "就绪".into(),
            level: StatusLevel::Info,
            deadline_ms: 0,
        }
    }
}

pub(crate) struct SendUiState {
    pub(crate) input: String,
    pub(crate) hex_mode: bool,
    pub(crate) append_lf: bool,
    pub(crate) error: Option<String>,
    pub(crate) popup_open: bool,
}

impl Default for SendUiState {
    fn default() -> Self {
        Self {
            input: String::new(),
            hex_mode: false,
            append_lf: false,
            error: None,
            popup_open: false,
        }
    }
}
use tool_transport::SerialPortDescriptor;

#[derive(Clone)]
pub(crate) struct SerialUiState {
    pub(crate) ports: Vec<SerialPortDescriptor>,
    pub(crate) selected_port: Option<String>,
    pub(crate) baud_rate: String,
    pub(crate) data_bits: String,
    pub(crate) stop_bits: String,
    pub(crate) parity: String,
    pub(crate) timeout_ms: String,
    pub(crate) last_port_refresh: f64,
}

impl Default for SerialUiState {
    fn default() -> Self {
        Self {
            ports: Vec::new(),
            selected_port: None,
            baud_rate: "115200".into(),
            data_bits: "8".into(),
            stop_bits: "1".into(),
            parity: "none".into(),
            timeout_ms: "50".into(),
            last_port_refresh: 0.0,
        }
    }
}
