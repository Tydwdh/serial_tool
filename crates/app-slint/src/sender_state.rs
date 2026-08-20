use std::collections::VecDeque;

use tool_transport::TransportManager;

use crate::state::LineEnding;

pub struct SenderState {
    pub input: String,
    pub hex_mode: bool,
    pub line_ending: LineEnding,
    pub error: Option<String>,
    pub target_port: Option<String>,
    pub send_history: VecDeque<String>,
    pub history_search: String,
    pub hex_strict: bool,
    pub periodic_enabled: bool,
    pub periodic_interval_ms: String,
    pub dtr_high: bool,
    pub rts_high: bool,
}

impl SenderState {
    pub fn from_config(cfg: &crate::config::PersistedConfig) -> Self {
        let mut history = VecDeque::new();
        for item in &cfg.send_history {
            if !item.trim().is_empty() {
                history.push_back(item.clone());
            }
        }
        Self {
            input: String::new(),
            hex_mode: false,
            line_ending: match cfg.line_ending {
                crate::config::LineEnding::None => LineEnding::None,
                crate::config::LineEnding::Lf => LineEnding::Lf,
                crate::config::LineEnding::Cr => LineEnding::Cr,
                crate::config::LineEnding::Crlf => LineEnding::Crlf,
            },
            error: None,
            target_port: cfg.selected_port.clone(),
            send_history: history,
            history_search: String::new(),
            hex_strict: true,
            periodic_enabled: false,
            periodic_interval_ms: "1000".to_owned(),
            dtr_high: true,
            rts_high: true,
        }
    }

    pub fn push_history(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        self.send_history.retain(|x| x != &text);
        self.send_history.push_back(text);
        while self.send_history.len() > crate::state::MAX_SEND_HISTORY {
            self.send_history.pop_front();
        }
    }

    pub fn do_send(&mut self, transport: &TransportManager) -> Result<(), String> {
        let port = self.target_port.clone().ok_or_else(|| "未选择端口".to_owned())?;
        let mut bytes = if self.hex_mode {
            tool_transport::parse_hex(&self.input).map_err(|e| e.to_string())?
        } else {
            self.input.as_bytes().to_vec()
        };
        let suffix = self.line_ending.suffix();
        if !suffix.is_empty() {
            bytes.extend_from_slice(suffix.as_bytes());
        }
        transport.send_to(&port, bytes).map_err(|e| e.to_string())?;
        self.push_history(self.input.clone());
        self.error = None;
        Ok(())
    }

    pub fn filtered_history(&self) -> Vec<String> {
        if self.history_search.trim().is_empty() {
            return self.send_history.iter().cloned().collect();
        }
        let q = self.history_search.to_lowercase();
        self.send_history
            .iter()
            .filter(|s| s.to_lowercase().contains(&q))
            .cloned()
            .collect()
    }
}
