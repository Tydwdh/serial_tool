use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::ops::Bound;

use tool_core::{Direction, Event};

/// Presentation-only block size. It is not a protocol frame boundary.
pub const MAX_TERMINAL_BLOCK_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Continuation {
    Complete,
    Start,
    Middle,
    End,
}

#[derive(Debug, Clone)]
pub struct TerminalRecord {
    pub id: u64,
    pub first_event_id: u64,
    pub last_event_id: u64,
    pub first_timestamp_ms: u64,
    pub last_timestamp_ms: u64,
    pub port: String,
    pub direction: Direction,
    pub bytes: Vec<u8>,
    pub continuation: Continuation,
}

#[derive(Debug, Clone)]
pub struct LiveTail {
    pub id: u64,
    pub first_event_id: u64,
    pub last_event_id: u64,
    pub first_timestamp_ms: u64,
    pub last_timestamp_ms: u64,
    pub port: String,
    pub direction: Direction,
    pub bytes: Vec<u8>,
    pub continuation: Continuation,
}

#[derive(Debug, Clone)]
pub enum TerminalItem {
    Sealed(TerminalRecord),
    Live(LiveTail),
}

#[derive(Debug, Default)]
pub struct TerminalStoreUpdate {
    pub changed_ids: Vec<u64>,
    pub removed_ids: Vec<u64>,
}

impl TerminalItem {
    pub fn id(&self) -> u64 {
        match self {
            Self::Sealed(record) => record.id,
            Self::Live(tail) => tail.id,
        }
    }

    pub fn first_event_id(&self) -> u64 {
        match self {
            Self::Sealed(record) => record.first_event_id,
            Self::Live(tail) => tail.first_event_id,
        }
    }

    pub fn first_timestamp_ms(&self) -> u64 {
        match self {
            Self::Sealed(record) => record.first_timestamp_ms,
            Self::Live(tail) => tail.first_timestamp_ms,
        }
    }

    pub fn last_timestamp_ms(&self) -> u64 {
        match self {
            Self::Sealed(record) => record.last_timestamp_ms,
            Self::Live(tail) => tail.last_timestamp_ms,
        }
    }

    pub fn port(&self) -> &str {
        match self {
            Self::Sealed(record) => &record.port,
            Self::Live(tail) => &tail.port,
        }
    }

    pub fn direction(&self) -> Direction {
        match self {
            Self::Sealed(record) => record.direction,
            Self::Live(tail) => tail.direction,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Sealed(record) => &record.bytes,
            Self::Live(tail) => &tail.bytes,
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live(_))
    }

    pub fn continuation(&self) -> Continuation {
        match self {
            Self::Sealed(record) => record.continuation,
            Self::Live(tail) => tail.continuation,
        }
    }
}

#[derive(Debug, Clone)]
struct StreamKey {
    port: String,
    direction: Direction,
}

impl PartialEq for StreamKey {
    fn eq(&self, other: &Self) -> bool {
        self.port == other.port && self.direction == other.direction
    }
}

impl Eq for StreamKey {}

impl Hash for StreamKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.port.hash(state);
        let direction = match self.direction {
            Direction::Rx => 0_u8,
            Direction::Tx => 1,
            Direction::Internal => 2,
        };
        direction.hash(state);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TerminalAssembler {
    /// Idle timeout used to end a display block, not to define a protocol frame.
    pub idle_finalize_ms: u64,
    pub max_block_bytes: usize,
}

impl Default for TerminalAssembler {
    fn default() -> Self {
        Self {
            idle_finalize_ms: 5,
            max_block_bytes: MAX_TERMINAL_BLOCK_BYTES,
        }
    }
}

impl TerminalAssembler {
    fn should_finalize_idle(&self, tail: &LiveTail, timestamp_ms: u64) -> bool {
        timestamp_ms.saturating_sub(tail.last_timestamp_ms) > self.idle_finalize_ms
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingContinuation {
    continuation: Continuation,
    last_timestamp_ms: u64,
}

pub struct TerminalStore {
    items: BTreeMap<u64, TerminalItem>,
    live_by_stream: HashMap<StreamKey, u64>,
    pending_continuation: HashMap<StreamKey, PendingContinuation>,
    next_id: u64,
    max_entries: usize,
}

impl TerminalStore {
    pub fn new(max_entries: usize) -> Self {
        Self {
            items: BTreeMap::new(),
            live_by_stream: HashMap::new(),
            pending_continuation: HashMap::new(),
            next_id: 1,
            max_entries: max_entries.max(100),
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.live_by_stream.clear();
        self.pending_continuation.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn get(&self, id: u64) -> Option<&TerminalItem> {
        self.items.get(&id)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &TerminalItem> {
        self.items.values()
    }

    /// Iterate items strictly after a stable terminal id.  Presentation
    /// export cursors use this to scan a large store over multiple UI ticks
    /// without rebuilding or rescanning the already exported prefix.
    pub fn iter_after(&self, id: Option<u64>) -> impl Iterator<Item = &TerminalItem> {
        let lower = id.map_or(Bound::Unbounded, Bound::Excluded);
        self.items
            .range((lower, Bound::Unbounded))
            .map(|(_, item)| item)
    }

    pub fn port_names(&self) -> Vec<String> {
        let mut names = self
            .items
            .values()
            .map(|item| item.port().to_owned())
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names
    }

    pub fn set_max_entries(&mut self, max_entries: usize) -> Vec<u64> {
        self.max_entries = max_entries.max(100);
        self.trim_to_limit()
    }

    pub fn ingest(
        &mut self,
        assembler: TerminalAssembler,
        event: &Event,
        port: String,
        bytes: &[u8],
    ) -> TerminalStoreUpdate {
        if bytes.is_empty() {
            return TerminalStoreUpdate::default();
        }

        let key = StreamKey {
            port: port.clone(),
            direction: event.direction,
        };
        let mut changed_ids = Vec::new();
        let mut changed_set = HashSet::new();
        let mut mark_changed = |id: u64| {
            if changed_set.insert(id) {
                changed_ids.push(id);
            }
        };
        let mut next_continuation = self
            .pending_continuation
            .remove(&key)
            .filter(|pending| {
                event.timestamp_ms.saturating_sub(pending.last_timestamp_ms)
                    <= assembler.idle_finalize_ms
            })
            .map_or(Continuation::Start, |pending| pending.continuation);

        if let Some(live_id) = self.live_by_stream.get(&key).copied() {
            let should_finalize = self
                .items
                .get(&live_id)
                .and_then(|item| match item {
                    TerminalItem::Live(tail) => {
                        Some(assembler.should_finalize_idle(tail, event.timestamp_ms))
                    }
                    TerminalItem::Sealed(_) => None,
                })
                .unwrap_or(true);
            if should_finalize {
                self.seal_live(live_id, false);
                self.live_by_stream.remove(&key);
                self.pending_continuation.remove(&key);
                next_continuation = Continuation::Start;
                mark_changed(live_id);
            } else if let Some(TerminalItem::Live(tail)) = self.items.get(&live_id) {
                next_continuation = tail.continuation;
            }
        }

        let mut offset = 0;
        // Keep the next newline as an absolute event offset. Otherwise a huge
        // no-newline packet would rescan the entire remaining slice for every
        // 4 KiB presentation block and become quadratic.
        let mut next_newline = bytes.iter().position(|byte| *byte == b'\n');
        while offset < bytes.len() {
            let live_id = self.ensure_live(
                &key,
                &port,
                event.direction,
                event.id,
                event.timestamp_ms,
                next_continuation,
            );
            mark_changed(live_id);

            let current_len = self
                .items
                .get(&live_id)
                .map(|item| item.bytes().len())
                .unwrap_or(0);
            if current_len >= assembler.max_block_bytes.max(1) {
                self.seal_live(live_id, false);
                self.live_by_stream.remove(&key);
                next_continuation = Continuation::Middle;
                continue;
            }

            let room = assembler.max_block_bytes.max(1) - current_len;
            let remaining = &bytes[offset..];
            let newline_end = next_newline
                .filter(|newline| *newline >= offset)
                .map(|newline| newline + 1 - offset);
            let take = newline_end.unwrap_or(remaining.len()).min(room).max(1);

            if let Some(TerminalItem::Live(tail)) = self.items.get_mut(&live_id) {
                tail.bytes.extend_from_slice(&remaining[..take]);
                tail.last_event_id = event.id;
                tail.last_timestamp_ms = event.timestamp_ms;
            }
            offset += take;

            let ended_line = self
                .items
                .get(&live_id)
                .is_some_and(|item| item.bytes().last() == Some(&b'\n'));
            if ended_line {
                next_newline = bytes[offset..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map(|index| index + offset);
            }
            let hit_limit = self
                .items
                .get(&live_id)
                .is_some_and(|item| item.bytes().len() >= assembler.max_block_bytes.max(1));
            if ended_line || hit_limit {
                self.seal_live(live_id, ended_line);
                self.live_by_stream.remove(&key);
                next_continuation = if ended_line {
                    Continuation::Start
                } else {
                    Continuation::Middle
                };

                // If the event ended exactly at the presentation block limit,
                // remember that the next event may still continue the same
                // physical line. The timestamp makes this state obey the same
                // rolling idle boundary as a live tail.
                if !ended_line && offset == bytes.len() {
                    self.pending_continuation.insert(
                        key.clone(),
                        PendingContinuation {
                            continuation: Continuation::Middle,
                            last_timestamp_ms: event.timestamp_ms,
                        },
                    );
                }
            }
        }

        TerminalStoreUpdate {
            changed_ids,
            removed_ids: self.trim_to_limit(),
        }
    }

    fn ensure_live(
        &mut self,
        key: &StreamKey,
        port: &str,
        direction: Direction,
        event_id: u64,
        timestamp_ms: u64,
        continuation: Continuation,
    ) -> u64 {
        if let Some(id) = self.live_by_stream.get(key).copied()
            && matches!(self.items.get(&id), Some(TerminalItem::Live(_)))
        {
            return id;
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.items.insert(
            id,
            TerminalItem::Live(LiveTail {
                id,
                first_event_id: event_id,
                last_event_id: event_id,
                first_timestamp_ms: timestamp_ms,
                last_timestamp_ms: timestamp_ms,
                port: port.to_owned(),
                direction,
                bytes: Vec::new(),
                continuation,
            }),
        );
        self.live_by_stream.insert(key.clone(), id);
        id
    }

    fn seal_live(&mut self, id: u64, ended_line: bool) {
        let Some(item) = self.items.remove(&id) else {
            return;
        };
        let TerminalItem::Live(tail) = item else {
            self.items.insert(id, item);
            return;
        };

        let continuation = if ended_line {
            match tail.continuation {
                Continuation::Start => Continuation::Complete,
                Continuation::Middle => Continuation::End,
                other => other,
            }
        } else {
            tail.continuation
        };
        self.items.insert(
            id,
            TerminalItem::Sealed(TerminalRecord {
                id: tail.id,
                first_event_id: tail.first_event_id,
                last_event_id: tail.last_event_id,
                first_timestamp_ms: tail.first_timestamp_ms,
                last_timestamp_ms: tail.last_timestamp_ms,
                port: tail.port,
                direction: tail.direction,
                bytes: tail.bytes,
                continuation,
            }),
        );
    }

    fn trim_to_limit(&mut self) -> Vec<u64> {
        let mut removed = Vec::new();
        while self.items.len() > self.max_entries {
            let Some((&id, _)) = self.items.first_key_value() else {
                break;
            };
            let Some(item) = self.items.remove(&id) else {
                break;
            };
            if let TerminalItem::Live(tail) = &item {
                let key = StreamKey {
                    port: tail.port.clone(),
                    direction: tail.direction,
                };
                self.live_by_stream.remove(&key);
                self.pending_continuation.remove(&key);
            }
            removed.push(id);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_core::Payload;

    fn event(id: u64, timestamp_ms: u64, port: &str, direction: Direction, bytes: &[u8]) -> Event {
        let mut event = Event::with_timestamp(
            timestamp_ms,
            "transport.serial.rx",
            format!("serial:{port}"),
            direction,
            Payload::Bytes(bytes.to_vec()),
        );
        event.id = id;
        event
    }

    fn item_bytes(item: &TerminalItem) -> &[u8] {
        item.bytes()
    }

    #[test]
    fn live_tail_keeps_one_stable_id_when_it_is_sealed() {
        let mut store = TerminalStore::new(100);
        let assembler = TerminalAssembler::default();

        store.ingest(
            assembler,
            &event(1, 1_000, "COM1", Direction::Rx, b"abc"),
            "COM1".to_owned(),
            b"abc",
        );
        let live_id = store.iter().next().expect("live item").id();

        store.ingest(
            assembler,
            &event(2, 1_001, "COM1", Direction::Rx, b"def\n"),
            "COM1".to_owned(),
            b"def\n",
        );

        let item = store.get(live_id).expect("stable item id");
        assert!(!item.is_live());
        assert_eq!(item_bytes(item), b"abcdef\n");
    }

    #[test]
    fn newline_splits_a_single_event_into_records_and_live_tail() {
        let mut store = TerminalStore::new(100);
        let event = event(1, 1_000, "COM1", Direction::Rx, b"a\nb\nc");

        store.ingest(
            TerminalAssembler::default(),
            &event,
            "COM1".to_owned(),
            b"a\nb\nc",
        );

        let items = store.iter().collect::<Vec<_>>();
        assert_eq!(items.len(), 3);
        assert_eq!(item_bytes(items[0]), b"a\n");
        assert_eq!(item_bytes(items[1]), b"b\n");
        assert_eq!(item_bytes(items[2]), b"c");
        assert!(!items[0].is_live());
        assert!(!items[1].is_live());
        assert!(items[2].is_live());
    }

    #[test]
    fn idle_timeout_uses_the_previous_event_as_the_rolling_boundary() {
        let mut store = TerminalStore::new(100);
        let assembler = TerminalAssembler {
            idle_finalize_ms: 5,
            max_block_bytes: 4096,
        };

        for (id, timestamp_ms, bytes) in [(1, 1_000, b"a".as_slice()), (2, 1_004, b"b".as_slice())]
        {
            let event = event(id, timestamp_ms, "COM1", Direction::Rx, bytes);
            store.ingest(assembler, &event, "COM1".to_owned(), bytes);
        }
        assert_eq!(store.iter().count(), 1);
        assert_eq!(item_bytes(store.iter().next().unwrap()), b"ab");

        let event = event(3, 1_010, "COM1", Direction::Rx, b"c");
        store.ingest(assembler, &event, "COM1".to_owned(), b"c");

        let items = store.iter().collect::<Vec<_>>();
        assert_eq!(items.len(), 2);
        assert_eq!(item_bytes(items[0]), b"ab");
        assert_eq!(item_bytes(items[1]), b"c");
        assert!(!items[0].is_live());
        assert!(items[1].is_live());
    }

    #[test]
    fn max_block_split_remembers_continuation_across_events() {
        let mut store = TerminalStore::new(100);
        let assembler = TerminalAssembler {
            idle_finalize_ms: 5,
            max_block_bytes: 4,
        };

        let first = event(1, 1_000, "COM1", Direction::Rx, b"abcd");
        store.ingest(assembler, &first, "COM1".to_owned(), b"abcd");
        let second = event(2, 1_004, "COM1", Direction::Rx, b"ef\n");
        store.ingest(assembler, &second, "COM1".to_owned(), b"ef\n");

        let items = store.iter().collect::<Vec<_>>();
        assert_eq!(items.len(), 2);
        assert_eq!(item_bytes(items[0]), b"abcd");
        assert_eq!(item_bytes(items[1]), b"ef\n");
        assert_eq!(items[0].continuation(), Continuation::Start);
        assert_eq!(items[1].continuation(), Continuation::End);
    }

    #[test]
    fn global_order_is_stable_when_a_older_live_item_is_sealed_later() {
        let mut store = TerminalStore::new(100);
        let assembler = TerminalAssembler::default();

        let first = event(1, 1_000, "COM1", Direction::Rx, b"a");
        store.ingest(assembler, &first, "COM1".to_owned(), b"a");
        let second = event(2, 1_001, "COM2", Direction::Rx, b"b\n");
        store.ingest(assembler, &second, "COM2".to_owned(), b"b\n");
        let third = event(3, 1_002, "COM1", Direction::Rx, b"c\n");
        store.ingest(assembler, &third, "COM1".to_owned(), b"c\n");

        let items = store.iter().collect::<Vec<_>>();
        assert_eq!(
            items.iter().map(|item| item.id()).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(item_bytes(items[0]), b"ac\n");
        assert_eq!(item_bytes(items[1]), b"b\n");
    }

    #[test]
    fn raw_bytes_are_not_replaced_by_lossy_text() {
        let mut store = TerminalStore::new(100);
        let bytes = [0xff, 0x00, b'A'];
        let event = event(1, 1_000, "COM1", Direction::Rx, &bytes);
        store.ingest(
            TerminalAssembler::default(),
            &event,
            "COM1".to_owned(),
            &bytes,
        );

        assert_eq!(item_bytes(store.iter().next().unwrap()), &bytes);
    }

    #[test]
    fn large_no_newline_packet_is_split_into_bounded_blocks() {
        let mut store = TerminalStore::new(2_000);
        let assembler = TerminalAssembler {
            idle_finalize_ms: 5,
            max_block_bytes: 4_096,
        };
        let bytes = vec![b'x'; 1_024 * 1_024];
        let event = event(1, 1_000, "COM1", Direction::Rx, &bytes);

        store.ingest(assembler, &event, "COM1".to_owned(), &bytes);

        let items = store.iter().collect::<Vec<_>>();
        assert_eq!(items.len(), 256);
        assert!(
            items[..255]
                .iter()
                .all(|item| item.bytes().len() == 4_096 && !item.is_live())
        );
        assert_eq!(items[255].bytes().len(), 4_096);
        assert!(!items[255].is_live());
    }

    #[test]
    fn clear_keeps_terminal_ids_monotonic_for_incremental_clients() {
        let mut store = TerminalStore::new(100);
        let first = event(1, 1_000, "COM1", Direction::Rx, b"one\n");
        store.ingest(
            TerminalAssembler::default(),
            &first,
            "COM1".to_owned(),
            b"one\n",
        );
        assert_eq!(store.iter().next().unwrap().id(), 1);

        store.clear();

        let second = event(2, 1_001, "COM1", Direction::Rx, b"two\n");
        store.ingest(
            TerminalAssembler::default(),
            &second,
            "COM1".to_owned(),
            b"two\n",
        );
        assert_eq!(store.iter().next().unwrap().id(), 2);
    }
}
