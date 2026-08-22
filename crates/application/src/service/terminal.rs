//! Terminal 领域服务 — entry 存储、merge、保留上限、增量查询。

use std::collections::{BTreeMap, VecDeque};

use crate::model::terminal::{TerminalDelta, TerminalEntry, TerminalSeq};
use tool_core::{Direction, Event, Payload};
use tool_databus::{DataBus, RingSubscription, TopicFilter};

const DEFAULT_MAX_ENTRIES: usize = 50_000;
const DEFAULT_MERGE_WINDOW_MS: u64 = 5;

#[derive(Debug, Clone)]
struct PortData {
    entries: VecDeque<TerminalEntry>,
}

pub struct TerminalService {
    subscription: RingSubscription,
    ports: BTreeMap<String, PortData>,
    next_seq: TerminalSeq,
    max_entries: usize,
    merge_window_ms: u64,
}

impl TerminalService {
    pub fn new(bus: DataBus) -> Self {
        let subscription =
            bus.subscribe_ring_bounded(TopicFilter::prefix("transport.serial."), 65_536);
        Self {
            subscription,
            ports: BTreeMap::new(),
            next_seq: 1,
            max_entries: DEFAULT_MAX_ENTRIES,
            merge_window_ms: DEFAULT_MERGE_WINDOW_MS,
        }
    }

    pub fn set_max_entries(&mut self, max: usize) {
        self.max_entries = max.max(100);
        self.enforce_limit();
    }

    pub fn set_merge_window_ms(&mut self, ms: u64) {
        self.merge_window_ms = ms;
    }

    pub fn clear(&mut self) {
        self.ports.clear();
        self.subscription.clear();
    }

    /// 消费 DataBus 中待处理的串口事件，填充内部存储。
    pub fn ingest_pending(&mut self) {
        for event in self.subscription.drain_limited(2048) {
            self.push_event(&event);
        }
    }

    /// 增量查询：返回 `since_seq` 之后最多 `limit` 条。
    pub fn entries_since(&self, since_seq: TerminalSeq, limit: usize) -> TerminalDelta {
        let mut out = Vec::new();
        let mut truncated = false;
        // 按 seq 递增遍历（ports 内 VecDeque 已按 seq 递增追加）
        // 为保持全局 seq 顺序，收集后按 seq 排序。
        let mut all: Vec<&TerminalEntry> = self
            .ports
            .values()
            .flat_map(|p| p.entries.iter())
            .filter(|e| e.seq > since_seq)
            .collect();
        all.sort_by_key(|e| e.seq);
        let dropped = self.subscription.dropped_count();
        for e in all {
            if out.len() >= limit {
                truncated = true;
                break;
            }
            out.push(e.clone());
        }
        let next_seq = out.last().map(|e| e.seq).unwrap_or(since_seq);
        TerminalDelta {
            entries: out,
            next_seq,
            truncated,
            dropped,
        }
    }

    pub fn total_entries(&self) -> usize {
        self.ports.values().map(|p| p.entries.len()).sum()
    }

    pub fn export_text(&self) -> String {
        let mut all: Vec<&TerminalEntry> =
            self.ports.values().flat_map(|p| p.entries.iter()).collect();
        all.sort_by_key(|e| e.seq);
        all.into_iter()
            .map(|e| {
                format!(
                    "{} [{}] {}",
                    e.port,
                    format_dir(e.direction),
                    e.display_text
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn export_csv(&self) -> String {
        let mut all: Vec<&TerminalEntry> =
            self.ports.values().flat_map(|p| p.entries.iter()).collect();
        all.sort_by_key(|e| e.seq);
        let mut out = String::from("seq,port,direction,text\n");
        for e in all {
            out.push_str(&format!(
                "{},{},{},{}\n",
                e.seq,
                csv_escape(&e.port),
                format_dir(e.direction),
                csv_escape(&e.display_text)
            ));
        }
        out
    }

    pub fn export_json(&self) -> String {
        let mut all: Vec<&TerminalEntry> =
            self.ports.values().flat_map(|p| p.entries.iter()).collect();
        all.sort_by_key(|e| e.seq);
        serde_json::to_string_pretty(&all).unwrap_or_default()
    }

    fn push_event(&mut self, event: &Event) {
        let is_rx = event.topic == tool_transport::serial_topics::SERIAL_RX;
        let is_tx = event.topic == tool_transport::serial_topics::SERIAL_TX;
        if !is_rx && !is_tx {
            return;
        }
        let direction = if is_rx { Direction::Rx } else { Direction::Tx };
        let port = event
            .metadata
            .get("port")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        if port.is_empty() {
            return;
        }
        let raw_bytes: Vec<u8> = match &event.payload {
            Payload::Bytes(b) => b.clone(),
            Payload::Text(s) => s.as_bytes().to_vec(),
            _ => event.payload.text_lossy().into_bytes(),
        };
        let raw_text = String::from_utf8_lossy(&raw_bytes).into_owned();
        let display_text = raw_text.clone();
        let hex_text = raw_bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        let preview_text = display_text.clone();

        let entry = TerminalEntry {
            seq: self.next_seq,
            event_id: event.id,
            timestamp_ms: event.timestamp_ms,
            port: port.clone(),
            direction,
            raw_text,
            display_text,
            hex_text,
            preview_text,
        };
        self.next_seq = self.next_seq.wrapping_add(1).max(1);

        let port_data = self.ports.entry(port).or_insert_with(|| PortData {
            entries: VecDeque::new(),
        });

        // 简单 merge：同端口同方向、时间差 ≤ merge_window_ms 且上一条不以 \n 结尾则拼接。
        let should_merge = port_data.entries.back().is_some_and(|prev| {
            prev.direction == direction
                && event.timestamp_ms.saturating_sub(prev.timestamp_ms) <= self.merge_window_ms
                && !prev.display_text.ends_with('\n')
                && prev.event_id != event.id
        });

        if should_merge {
            if let Some(prev) = port_data.entries.back_mut() {
                prev.raw_text.push_str(&entry.raw_text);
                prev.display_text.push_str(&entry.display_text);
                if !prev.hex_text.is_empty() && !entry.hex_text.is_empty() {
                    prev.hex_text.push(' ');
                    prev.hex_text.push_str(&entry.hex_text);
                } else if !entry.hex_text.is_empty() {
                    prev.hex_text = entry.hex_text.clone();
                }
                prev.preview_text = prev.display_text.clone();
                // seq 保持原值，不新增条目
                self.next_seq = self.next_seq.wrapping_sub(1).max(1);
                return;
            }
        }

        port_data.entries.push_back(entry);
        self.enforce_limit();
    }

    fn enforce_limit(&mut self) {
        let total: usize = self.ports.values().map(|p| p.entries.len()).sum();
        if total <= self.max_entries {
            return;
        }
        let mut to_remove = total - self.max_entries;
        while to_remove > 0 {
            let oldest_port = self
                .ports
                .iter()
                .filter(|(_, p)| !p.entries.is_empty())
                .min_by_key(|(_, p)| p.entries.front().map(|e| e.seq).unwrap_or(u64::MAX))
                .map(|(k, _)| k.clone());
            if let Some(key) = oldest_port {
                if let Some(data) = self.ports.get_mut(&key) {
                    data.entries.pop_front();
                    to_remove -= 1;
                    if data.entries.is_empty() {
                        self.ports.remove(&key);
                    }
                }
            } else {
                break;
            }
        }
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

fn format_dir(d: Direction) -> &'static str {
    match d {
        Direction::Rx => "RX",
        Direction::Tx => "TX",
        Direction::Internal => "IN",
    }
}

fn write_utf8_csv(_path: &std::path::Path, _content: &str) -> std::io::Result<()> {
    Ok(())
}
