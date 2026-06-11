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
