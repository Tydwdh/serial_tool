//! Shared scrollback retention and incremental-export state.
//!
//! Terminal and Log used to carry byte-identical copies of the drop-episode
//! window, the truncation episode, and the export-cursor state machine. This
//! module owns those rules; the panels keep only their row stores and row
//! construction.

use tool_core::Event;
use tool_databus::{DataBus, RingSubscription, TopicFilter};

use crate::DROP_EPISODE_QUIET_MS;

/// One "drop episode" for a lossy ring subscription.
///
/// A burst keeps incrementing the dropped count every frame; reporting it each
/// frame makes the toast text change continuously and defeats deduplication.
/// Report the first positive count, then stay quiet until the subscription has
/// been quiet for [`DROP_EPISODE_QUIET_MS`].
#[derive(Debug, Default)]
pub(crate) struct DropEpisode {
    reported: bool,
    last_drop_ms: u64,
}

impl DropEpisode {
    pub(crate) fn observe(&mut self, dropped: u64, now_ms: u64) -> u64 {
        if dropped > 0 {
            self.last_drop_ms = now_ms;
            if self.reported {
                return 0;
            }
            self.reported = true;
            return dropped;
        }
        if self.reported && now_ms.saturating_sub(self.last_drop_ms) >= DROP_EPISODE_QUIET_MS {
            self.reported = false;
        }
        0
    }

    pub(crate) fn reset(&mut self) {
        self.reported = false;
        self.last_drop_ms = 0;
    }
}

/// The bus subscription plus the retention policy shared by both scrollbacks.
///
/// It owns the subscription handle and the reusable drain batch. Panels call
/// [`take_batch`](Self::take_batch), push rows into their own store, then
/// [`recycle_batch`](Self::recycle_batch) to keep the allocation.
pub(crate) struct Scrollback {
    subscription: RingSubscription,
    batch: Vec<Event>,
    max_entries: usize,
    truncated: bool,
    limit_episode_reported: bool,
    drop: DropEpisode,
}

impl Scrollback {
    pub(crate) fn new(
        bus: &DataBus,
        filter: TopicFilter,
        buffer_capacity: usize,
        max_entries: usize,
    ) -> Self {
        Self {
            subscription: bus.subscribe_ring_bounded(filter, buffer_capacity),
            batch: Vec::new(),
            max_entries: max_entries.max(100),
            truncated: false,
            limit_episode_reported: false,
            drop: DropEpisode::default(),
        }
    }

    /// Take up to `budget` events into the reusable batch. The caller must
    /// return the vector through [`recycle_batch`](Self::recycle_batch).
    pub(crate) fn take_batch(&mut self, budget: usize) -> Vec<Event> {
        let mut batch = std::mem::take(&mut self.batch);
        batch.extend(self.subscription.drain_limited(budget));
        batch
    }

    pub(crate) fn recycle_batch(&mut self, mut batch: Vec<Event>) {
        batch.clear();
        self.batch = batch;
    }

    pub(crate) fn take_dropped_events(&mut self, now_ms: u64) -> u64 {
        let dropped = self.subscription.take_dropped_count();
        self.drop.observe(dropped, now_ms)
    }

    /// Record a trim pass. `removed` is the number of rows evicted by this
    /// pass; `remaining` is the current store length.
    pub(crate) fn note_trimmed(&mut self, removed: usize, remaining: usize) {
        if removed > 0 && !self.limit_episode_reported {
            self.limit_episode_reported = true;
            self.truncated = true;
        }
        // Back below the limit (clear or raised cap) ends this episode.
        if remaining < self.max_entries {
            self.limit_episode_reported = false;
        }
    }

    pub(crate) fn take_truncated(&mut self) -> bool {
        let truncated = self.truncated;
        self.truncated = false;
        truncated
    }

    pub(crate) fn truncated(&self) -> bool {
        self.truncated
    }

    pub(crate) fn clear_truncated(&mut self) {
        self.truncated = false;
    }

    pub(crate) fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub(crate) fn max_entries_mut(&mut self) -> &mut usize {
        &mut self.max_entries
    }

    pub(crate) fn set_max_entries(&mut self, max_entries: usize) {
        self.max_entries = max_entries.max(100);
    }

    pub(crate) fn clear(&mut self) {
        self.subscription.clear();
        let _ = self.subscription.take_dropped_count();
        self.batch.clear();
        self.truncated = false;
        self.limit_episode_reported = false;
        self.drop.reset();
    }
}

/// First/previous/done bookkeeping for an incremental export cursor.
#[derive(Debug, Default)]
pub(crate) struct ExportChunkState {
    started: bool,
    emitted: bool,
    done: bool,
}

impl ExportChunkState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Returns `(first_chunk, has_previous_rows)`.
    pub(crate) fn begin(&mut self) -> (bool, bool) {
        let first = !self.started;
        let has_previous = self.emitted;
        self.started = true;
        (first, has_previous)
    }

    /// Finish a chunk and return the new terminal state.
    pub(crate) fn finish(&mut self, done: bool, exported: usize) -> bool {
        self.emitted |= exported > 0;
        self.done = done;
        done
    }

    pub(crate) fn done(&self) -> bool {
        self.done
    }
}
