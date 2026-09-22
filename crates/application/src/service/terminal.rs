//! Terminal domain service.
//!
//! The service and the egui panel share the same byte-preserving store. The
//! service exposes the legacy read-model DTO for headless callers, while the
//! store remains the single source of truth for ordering and assembly.

use crate::model::terminal::{TerminalDelta, TerminalEntry, TerminalSeq};
use crate::service::terminal_store::{
    MAX_TERMINAL_BLOCK_BYTES, TerminalAssembler, TerminalItem, TerminalStore,
};
use parking_lot::RwLock;
use std::sync::Arc;
use tool_core::{Event, Payload};
use tool_databus::{DataBus, RingSubscription, TopicFilter};

const DEFAULT_MAX_ENTRIES: usize = 50_000;

pub struct TerminalService {
    subscription: RingSubscription,
    store: Arc<RwLock<TerminalStore>>,
    assembler: TerminalAssembler,
}

impl TerminalService {
    pub fn new(bus: DataBus) -> Self {
        Self {
            subscription: bus
                .subscribe_ring_bounded(TopicFilter::prefix("transport.serial."), 65_536),
            store: Arc::new(RwLock::new(TerminalStore::new(DEFAULT_MAX_ENTRIES))),
            assembler: TerminalAssembler {
                idle_finalize_ms: 5,
                max_block_bytes: MAX_TERMINAL_BLOCK_BYTES,
            },
        }
    }

    pub fn set_max_entries(&mut self, max: usize) {
        self.store.write().set_max_entries(max);
    }

    /// Keep the public setting name for compatibility; the value is an idle
    /// boundary for display assembly, not a protocol-frame merge rule.
    pub fn set_merge_window_ms(&mut self, ms: u64) {
        self.assembler.idle_finalize_ms = ms;
    }

    pub fn clear(&mut self) {
        self.store.write().clear();
        self.subscription.clear();
    }

    /// Consume serial events without delaying their insertion into the store.
    pub fn ingest_pending(&mut self) {
        for event in self.subscription.drain_limited(2048) {
            self.push_event(&event);
        }
    }

    /// Incremental query over the globally ordered stable IDs.
    pub fn entries_since(&self, since_seq: TerminalSeq, limit: usize) -> TerminalDelta {
        let mut out = Vec::new();
        let mut truncated = false;
        for item in self
            .store
            .read()
            .iter()
            .filter(|item| item.id() > since_seq)
        {
            if out.len() >= limit {
                truncated = true;
                break;
            }
            out.push(entry_from_item(item));
        }

        let next_seq = out.last().map(|entry| entry.seq).unwrap_or(since_seq);
        TerminalDelta {
            entries: out,
            next_seq,
            truncated,
            dropped: self.subscription.dropped_count(),
        }
    }

    fn push_event(&mut self, event: &Event) {
        let is_serial = matches!(
            event.topic.as_str(),
            tool_transport::serial_topics::SERIAL_RX | tool_transport::serial_topics::SERIAL_TX
        );
        if !is_serial {
            return;
        }

        let port = event
            .metadata
            .get("port")
            .and_then(|value| value.as_str())
            .or_else(|| event.source.strip_prefix("serial:"))
            .unwrap_or("default")
            .to_owned();
        let bytes = payload_bytes(&event.payload);
        if bytes.is_empty() {
            return;
        }

        self.store
            .write()
            .ingest(self.assembler, event, port, bytes.as_ref());
    }
}

fn payload_bytes(payload: &Payload) -> std::borrow::Cow<'_, [u8]> {
    if let Some(bytes) = payload.as_bytes() {
        std::borrow::Cow::Borrowed(bytes)
    } else {
        std::borrow::Cow::Owned(payload.text_lossy().into_bytes())
    }
}

fn entry_from_item(item: &TerminalItem) -> TerminalEntry {
    // 同一个字节序列的有损解码只做一次：三个文本字段读的就是同一份内容。
    let text = String::from_utf8_lossy(item.bytes()).into_owned();
    TerminalEntry {
        seq: item.id(),
        event_id: item.first_event_id(),
        timestamp_ms: item.first_timestamp_ms(),
        port: item.port().to_owned(),
        direction: item.direction(),
        display_text: text.clone(),
        raw_text: text.clone(),
        hex_text: format_hex(item.bytes()),
        preview_text: text,
    }
}

fn format_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(3).saturating_sub(1));
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{byte:02X}"));
    }
    out
}
