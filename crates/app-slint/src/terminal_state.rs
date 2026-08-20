use std::collections::{BTreeMap, BTreeSet, VecDeque};

use tool_core::{Direction, Event, Payload};
use tool_databus::{DataBus, RingSubscription, TopicFilter};
use tool_transport::serial_topics;

use crate::search::SearchQuery;

fn short_port_display(port: &str) -> std::borrow::Cow<'_, str> {
    let host = port.rsplit_once(':').map(|(h, _)| h).unwrap_or(port);
    let segs: Vec<&str> = host.split('.').collect();
    if segs.len() == 4 && segs.iter().all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())) {
        std::borrow::Cow::Owned(format!("{}.{}", segs[segs.len() - 2], segs[segs.len() - 1]))
    } else {
        std::borrow::Cow::Borrowed(port)
    }
}
fn port_display_name<'b>(port: &'b str, aliases: &'b std::collections::HashMap<String, String>) -> std::borrow::Cow<'b, str> {
    if let Some(alias) = aliases.get(port).filter(|a| !a.trim().is_empty()) {
        std::borrow::Cow::Borrowed(alias.as_str())
    } else {
        short_port_display(port)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalExportFormat {
    Txt,
    Csv,
    Json,
}

#[derive(Clone)]
struct TerminalEntry {
    id: u64,
    event_id: u64,
    timestamp_ms: u64,
    timestamp_label: String,
    direction: Direction,
    raw_text: String,
    display_text: String,
    hex_text: String,
    preview_text: String,
}
struct PortData {
    entries: VecDeque<TerminalEntry>,
}
impl Default for PortData {
    fn default() -> Self {
        Self { entries: VecDeque::new() }
    }
}

pub struct TerminalState {
    subscription: RingSubscription,
    ports: BTreeMap<String, PortData>,
    port_aliases: std::collections::HashMap<String, String>,
    pub show_rx: bool,
    pub show_tx: bool,
    pub show_hex: bool,
    pub show_raw: bool,
    pub search_text: String,
    pub search_case: bool,
    pub port_filter: Option<String>,
    pub max_entries: usize,
    pub truncated: bool,
    next_entry_id: u64,
    pub selected_ids: BTreeSet<u64>,
    pub font_size: f32,
    pub merge_window_ms: u64,
}

impl TerminalState {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            subscription: bus.subscribe_ring_bounded(TopicFilter::prefix(String::from("transport.serial.")), 65536),
            ports: BTreeMap::new(),
            port_aliases: Default::default(),
            show_rx: true,
            show_tx: true,
            show_hex: false,
            show_raw: false,
            search_text: String::new(),
            search_case: false,
            port_filter: None,
            max_entries: 50_000,
            truncated: false,
            next_entry_id: 1,
            selected_ids: BTreeSet::new(),
            font_size: 13.0,
            merge_window_ms: 5,
        }
    }

    pub fn set_port_aliases(&mut self, aliases: &std::collections::HashMap<String, String>) {
        self.port_aliases = aliases.clone();
    }

    pub fn set_max_entries(&mut self, n: usize) {
        self.max_entries = n.clamp(500, 50000);
        self.trim_excess();
    }

    fn trim_excess(&mut self) {
        let total: usize = self.ports.values().map(|p| p.entries.len()).sum();
        if total <= self.max_entries {
            return;
        }
        // 按 event_id 最旧优先淘汰
        let mut all: Vec<(u64, String, u64)> = Vec::new(); // (event_id, port, entry_id)
        for (port, data) in &self.ports {
            for e in &data.entries {
                all.push((e.event_id, port.clone(), e.id));
            }
        }
        all.sort_by_key(|(eid, _, _)| *eid);
        let to_remove = total - self.max_entries;
        let remove_ids: BTreeSet<u64> = all.iter().take(to_remove).map(|(_, _, id)| *id).collect();
        for data in self.ports.values_mut() {
            data.entries.retain(|e| !remove_ids.contains(&e.id));
        }
        self.truncated = to_remove > 0;
    }

    pub fn ingest(&mut self) {
        let events = self.subscription.drain_limited(500);
        for e in events {
            self.push_event(e);
        }
        self.trim_excess();
    }

    fn push_event(&mut self, event: Event) {
        let port = event
            .meta_str("port")
            .map(|s| s.to_owned())
            .unwrap_or_else(|| event.source.clone());
        let dir = event.direction;
        let ts = event.timestamp_ms;
        let label = crate::util::fmt_ts(ts);
        let (raw, display, hex, preview) = event_payload_texts(&event);
        // 合并判断：同端口同方向、间隔≤merge_window_ms 且不含 \n
        if let Some(data) = self.ports.get_mut(&port) {
            if let Some(last) = data.entries.back_mut() {
                if last.direction == dir
                    && ts.saturating_sub(last.timestamp_ms) <= self.merge_window_ms
                    && !last.raw_text.contains('\n')
                    && !raw.contains('\n')
                {
                    last.raw_text.push_str(&raw);
                    last.display_text.push_str(&display);
                    last.hex_text.push(' ');
                    last.hex_text.push_str(&hex);
                    last.preview_text.push_str(&preview);
                    last.timestamp_ms = ts;
                    last.timestamp_label = label;
                    return;
                }
            }
        }
        let entry = TerminalEntry {
            id: self.next_entry_id,
            event_id: event.id,
            timestamp_ms: ts,
            timestamp_label: label,
            direction: dir,
            raw_text: raw,
            display_text: display,
            hex_text: hex,
            preview_text: preview,
        };
        self.next_entry_id += 1;
        self.ports.entry(port).or_default().entries.push_back(entry);
    }

    pub fn clear(&mut self) {
        self.ports.clear();
        self.subscription.clear();
        self.truncated = false;
        self.selected_ids.clear();
    }

    pub fn dropped_count(&self) -> u64 {
        self.subscription.dropped_count()
    }

    pub fn ports_list(&self) -> Vec<String> {
        self.ports.keys().cloned().collect()
    }

    pub fn visible_rows(&self) -> Vec<VisibleRow> {
        let query = SearchQuery::new(&self.search_text, self.search_case);
        let mut rows: Vec<VisibleRow> = Vec::new();
        for (port, data) in &self.ports {
            if let Some(ref f) = self.port_filter {
                if port != f {
                    continue;
                }
            }
            for e in &data.entries {
                if !self.show_rx && e.direction == Direction::Rx {
                    continue;
                }
                if !self.show_tx && e.direction == Direction::Tx {
                    continue;
                }
                let text_for_search = if self.show_raw { &e.raw_text } else { &e.display_text };
                if !query.is_empty() && !query.matches(text_for_search) {
                    continue;
                }
                let preview = if self.show_hex { &e.hex_text } else if self.show_raw { &e.raw_text } else { &e.preview_text };
                rows.push(VisibleRow {
                    id: e.id,
                    port: port_display_name(port, &self.port_aliases).into_owned(),
                    ts: e.timestamp_label.clone(),
                    dir: match e.direction {
                        Direction::Rx => "RX".to_owned(),
                        Direction::Tx => "TX".to_owned(),
                        Direction::Internal => "IN".to_owned(),
                    },
                    preview: preview.clone(),
                    selected: self.selected_ids.contains(&e.id),
                });
            }
        }
        rows.sort_by_key(|r| r.id);
        rows
    }

    pub fn export_visible_text(&self) -> String {
        self.visible_rows()
            .into_iter()
            .map(|r| format!("{} {} {} {}", r.ts, r.port, r.dir, r.preview))
            .collect::<Vec<_>>()
            .join("\n")
    }
    pub fn export_visible_csv(&self) -> String {
        let mut out = String::from("timestamp,port,direction,text\n");
        for r in self.visible_rows() {
            out.push_str(&format!("\"{}\",\"{}\",\"{}\",\"{}\"\n", r.ts, r.port, r.dir, r.preview.replace('"', "\"\"")));
        }
        out
    }
    pub fn export_visible_json(&self) -> String {
        let rows = self.visible_rows();
        serde_json::to_string_pretty(&rows.iter().map(|r| serde_json::json!({"id":r.id,"ts":r.ts,"port":r.port,"dir":r.dir,"text":r.preview})).collect::<Vec<_>>()).unwrap_or_default()
    }

    pub fn toggle_selected(&mut self, id: u64) {
        if !self.selected_ids.remove(&id) {
            self.selected_ids.insert(id);
        }
    }
    pub fn select_all_visible(&mut self) {
        for r in self.visible_rows() {
            self.selected_ids.insert(r.id);
        }
    }
    pub fn clear_selection(&mut self) {
        self.selected_ids.clear();
    }
}

#[derive(Clone)]
pub struct VisibleRow {
    pub id: u64,
    pub port: String,
    pub ts: String,
    pub dir: String,
    pub preview: String,
    pub selected: bool,
}

fn event_payload_texts(event: &Event) -> (String, String, String, String) {
    let raw = match &event.payload {
        Payload::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        Payload::Text(t) => t.clone(),
        Payload::Json(v) => v.to_string(),
        Payload::Empty => String::new(),
    };
    let display = raw.clone();
    let hex = match &event.payload {
        Payload::Bytes(b) => b.iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" "),
        Payload::Text(t) => t.as_bytes().iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" "),
        Payload::Json(v) => v.to_string().as_bytes().iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" "),
        Payload::Empty => String::new(),
    };
    let preview = display.lines().next().unwrap_or("").to_owned();
    (raw, display, hex, preview)
}

// ── LogState（镜像 Terminal，订阅 log.*） ──────────────────────
pub struct LogState {
    subscription: RingSubscription,
    pub entries: VecDeque<LogEntry>,
    pub search_text: String,
    pub search_case: bool,
    pub min_level: tool_core::LogLevel,
    pub source_filter: Option<String>,
    pub max_entries: usize,
    pub selected_ids: BTreeSet<u64>,
    next_id: u64,
}
#[derive(Clone)]
pub struct LogEntry {
    pub id: u64,
    pub ts: String,
    pub level: tool_core::LogLevel,
    pub source: String,
    pub text: String,
}
impl LogState {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            subscription: bus.subscribe_ring_bounded(TopicFilter::prefix(String::from("log.")), 65536),
            entries: VecDeque::new(),
            search_text: String::new(),
            search_case: false,
            min_level: tool_core::LogLevel::Info,
            source_filter: None,
            max_entries: 50_000,
            selected_ids: BTreeSet::new(),
            next_id: 1,
        }
    }
    pub fn ingest(&mut self) {
        for e in self.subscription.drain_limited(500) {
            let level = e.meta_str("level").and_then(|s| s.parse().ok()).unwrap_or(tool_core::LogLevel::Info);
            if level < self.min_level {
                continue;
            }
            let source = e.meta_str("source").unwrap_or("").to_owned();
            let text = match &e.payload {
                Payload::Text(t) => t.clone(),
                Payload::Json(v) => v.to_string(),
                Payload::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                Payload::Empty => String::new(),
            };
            let entry = LogEntry {
                id: self.next_id,
                ts: crate::util::fmt_ts(e.timestamp_ms),
                level,
                source,
                text,
            };
            self.next_id += 1;
            self.entries.push_back(entry);
            while self.entries.len() > self.max_entries {
                self.entries.pop_front();
            }
        }
    }
    pub fn visible_rows(&self) -> Vec<LogEntry> {
        let q = SearchQuery::new(&self.search_text, self.search_case);
        self.entries
            .iter()
            .filter(|e| {
                if let Some(ref f) = self.source_filter {
                    if &e.source != f {
                        return false;
                    }
                }
                if !q.is_empty() && !q.matches(&format!("{} {}", e.source, e.text)) {
                    return false;
                }
                true
            })
            .cloned()
            .collect()
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.subscription.clear();
        self.selected_ids.clear();
    }
    pub fn dropped_count(&self) -> u64 {
        self.subscription.dropped_count()
    }
}
