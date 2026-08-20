use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum StatusLevel {
    Info,
    Warn,
    Error,
}
impl StatusLevel {
    pub fn ttl_ms(self) -> Option<u64> {
        match self {
            Self::Info => Some(5_000),
            Self::Warn => Some(8_000),
            Self::Error => Some(15_000),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Clone)]
pub struct Notification {
    pub id: u64,
    pub level: StatusLevel,
    pub text: String,
    pub deadline_ms: Option<u64>,
}
impl Notification {
    fn is_expired(&self, now: u64) -> bool {
        self.deadline_ms.is_some_and(|dl| now > dl)
    }
}

#[derive(Clone)]
pub struct NotificationQueue {
    entries: std::collections::VecDeque<(String, Notification)>,
    next_id: u64,
}
impl NotificationQueue {
    pub fn new() -> Self {
        Self {
            entries: std::collections::VecDeque::new(),
            next_id: 1,
        }
    }
    pub fn push(&mut self, source: &str, level: StatusLevel, text: impl Into<String>) {
        let now = tool_core::now_timestamp_ms();
        let deadline_ms = level.ttl_ms().map(|ttl| now + ttl);
        let notification = Notification {
            id: self.next_id,
            level,
            text: text.into(),
            deadline_ms,
        };
        self.next_id = self.next_id.wrapping_add(1).max(1);
        for (s, n) in self.entries.iter_mut().rev() {
            if s == source {
                *n = notification;
                return;
            }
        }
        self.entries.push_back((source.to_owned(), notification));
    }
    pub fn current(&mut self) -> Vec<Notification> {
        let now = tool_core::now_timestamp_ms();
        while self.entries.front().is_some_and(|(_, n)| n.is_expired(now)) {
            self.entries.pop_front();
        }
        self.entries.retain(|(_, n)| !n.is_expired(now));
        self.entries.iter().map(|(_, n)| n.clone()).collect()
    }
    pub fn dismiss(&mut self, source: &str) {
        self.entries.retain(|(s, _)| s != source);
    }
    pub fn is_empty_mut(&mut self) -> bool {
        self.current().is_empty()
    }
}
impl Default for NotificationQueue {
    fn default() -> Self {
        Self::new()
    }
}

pub const MAX_SEND_HISTORY: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineEnding {
    None,
    Lf,
    Cr,
    Crlf,
}
impl LineEnding {
    pub const ALL: [Self; 4] = [Self::None, Self::Lf, Self::Cr, Self::Crlf];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::Lf => "LF",
            Self::Cr => "CR",
            Self::Crlf => "CRLF",
        }
    }
    pub fn suffix(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Lf => "\n",
            Self::Cr => "\r",
            Self::Crlf => "\r\n",
        }
    }
}
impl Default for LineEnding {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone)]
pub struct SendUiState {
    pub input: String,
    pub hex_mode: bool,
    pub line_ending: LineEnding,
    pub error: Option<String>,
    pub target_port: Option<String>,
    pub send_history: std::collections::VecDeque<String>,
    pub history_search: String,
    pub history_index: Option<usize>,
    pub saved_input: String,
    pub hex_strict: bool,
    pub dtr_high: bool,
    pub rts_high: bool,
    pub periodic_enabled: bool,
    pub periodic_interval_ms: String,
    pub periodic_send_count: u64,
    pub periodic_max_count: Option<u64>,
}
impl Default for SendUiState {
    fn default() -> Self {
        Self {
            input: String::new(),
            hex_mode: false,
            line_ending: LineEnding::None,
            error: None,
            target_port: None,
            send_history: std::collections::VecDeque::new(),
            history_search: String::new(),
            history_index: None,
            saved_input: String::new(),
            hex_strict: true,
            dtr_high: true,
            rts_high: true,
            periodic_enabled: false,
            periodic_interval_ms: "1000".to_owned(),
            periodic_send_count: 0,
            periodic_max_count: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SerialUiState {
    pub selected_port: Option<String>,
    pub baud_rate: String,
    pub data_bits: String,
    pub stop_bits: String,
    pub parity: String,
    pub port_aliases: std::collections::HashMap<String, String>,
    pub port_groups: std::collections::HashMap<String, String>,
    pub port_profiles: std::collections::HashMap<String, crate::config::PortProfile>,
    pub network_ports: Vec<tool_transport::NetworkSerialConfig>,
    pub auto_reconnect: bool,
}
impl Default for SerialUiState {
    fn default() -> Self {
        Self {
            selected_port: None,
            baud_rate: "115200".to_owned(),
            data_bits: "8".to_owned(),
            stop_bits: "1".to_owned(),
            parity: "none".to_owned(),
            port_aliases: Default::default(),
            port_groups: Default::default(),
            port_profiles: Default::default(),
            network_ports: Vec::new(),
            auto_reconnect: true,
        }
    }
}
impl SerialUiState {
    pub fn port_label(&self, port: &str) -> String {
        if let Some(alias) = self.port_aliases.get(port).filter(|a| !a.trim().is_empty()) {
            format!("{alias} ({port})")
        } else {
            port.to_owned()
        }
    }
}
