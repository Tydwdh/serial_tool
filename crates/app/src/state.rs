#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
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

pub(crate) const MAX_SEND_HISTORY: usize = 50;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineEnding {
    None,
    Lf,
    Cr,
    Crlf,
}

impl LineEnding {
    pub(crate) const ALL: [Self; 4] = [Self::None, Self::Lf, Self::Cr, Self::Crlf];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::Lf => "LF",
            Self::Cr => "CR",
            Self::Crlf => "CRLF",
        }
    }

    pub(crate) fn suffix(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Lf => "\n",
            Self::Cr => "\r",
            Self::Crlf => "\r\n",
        }
    }
}

pub(crate) struct SendUiState {
    pub(crate) input: String,
    pub(crate) hex_mode: bool,
    pub(crate) line_ending: LineEnding,
    pub(crate) error: Option<String>,
    pub(crate) popup_open: bool,
    pub(crate) target_port: Option<String>,
    pub(crate) send_history: std::collections::VecDeque<String>,
    pub(crate) hex_strict: bool,
    pub(crate) dtr_high: bool,
    pub(crate) rts_high: bool,
    pub(crate) periodic_enabled: bool,
    pub(crate) periodic_interval_ms: String,
    pub(crate) next_periodic_send_time: f64,
    pub(crate) periodic_send_count: u64,
    pub(crate) periodic_max_count: Option<u64>,
}

impl Default for SendUiState {
    fn default() -> Self {
        Self {
            input: String::new(),
            hex_mode: false,
            line_ending: LineEnding::Lf,
            error: None,
            popup_open: false,
            target_port: None,
            send_history: std::collections::VecDeque::new(),
            hex_strict: true,
            dtr_high: true,
            rts_high: true,
            periodic_enabled: false,
            periodic_interval_ms: "1000".to_owned(),
            next_periodic_send_time: 0.0,
            periodic_send_count: 0,
            periodic_max_count: None,
        }
    }
}
