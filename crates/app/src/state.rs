#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StatusLevel {
    Info,
    Warn,
    Error,
}

impl StatusLevel {
    pub(crate) fn ttl_ms(self) -> u64 {
        match self {
            Self::Info => 5_000,
            Self::Warn => 8_000,
            Self::Error => 15_000,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetachedPanelAction {
    None,
    Attach,
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BottomTab {
    Terminal,
    Logs,
}

impl BottomTab {
    pub(crate) const ALL: [Self; 2] = [Self::Terminal, Self::Logs];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Terminal => "接收",
            Self::Logs => "日志",
        }
    }

    pub(crate) fn is_available(self, terminal_popup_open: bool) -> bool {
        !matches!(self, Self::Terminal) || !terminal_popup_open
    }
}

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
