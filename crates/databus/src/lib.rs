use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError, unbounded};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use tool_core::Event;

/// 默认历史记录限制
pub const DEFAULT_HISTORY_LIMIT: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TopicFilter {
    All,
    Exact(String),
    Prefix(String),
    And(Vec<TopicFilter>),
    MetadataEq { key: String, value: String },
}

impl TopicFilter {
    pub fn exact(topic: impl Into<String>) -> Self {
        Self::Exact(topic.into())
    }

    pub fn prefix(prefix: impl Into<String>) -> Self {
        Self::Prefix(prefix.into())
    }

    pub fn metadata_eq(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self::MetadataEq {
            key: key.into(),
            value: value.into(),
        }
    }

    pub fn and(filters: impl IntoIterator<Item = TopicFilter>) -> Self {
        Self::And(filters.into_iter().collect())
    }

    /// 仅匹配 topic 字符串（metadata 过滤条件在 topic 级别总是通过）。
    pub fn matches(&self, topic: &str) -> bool {
        match self {
            Self::All => true,
            Self::Exact(expected) => topic == expected,
            Self::Prefix(prefix) => topic.starts_with(prefix),
            Self::And(filters) => filters.iter().all(|f| f.matches(topic)),
            Self::MetadataEq { .. } => true,
        }
    }

    /// 完整匹配（包括 metadata）。DataBus publish 内部使用。
    pub fn matches_event(&self, event: &Event) -> bool {
        match self {
            Self::All => true,
            Self::Exact(expected) => event.topic == *expected,
            Self::Prefix(prefix) => event.topic.starts_with(prefix),
            Self::And(filters) => filters.iter().all(|filter| filter.matches_event(event)),
            Self::MetadataEq { key, value } => {
                event.meta_str(key).is_some_and(|actual| actual == value)
            }
        }
    }
}

#[derive(Clone)]
pub struct DataBus {
    inner: Arc<Inner>,
}

struct Inner {
    subscribers: Mutex<Vec<Subscriber>>,
    history: Mutex<VecDeque<Arc<Event>>>,
    next_id: AtomicU64,
    history_limit: usize,
}

struct Subscriber {
    filter: TopicFilter,
    sender: Sender<Arc<Event>>,
    dropped: Arc<AtomicU64>,
}

pub struct Subscription {
    receiver: Receiver<Arc<Event>>,
    dropped: Arc<AtomicU64>,
}

impl Subscription {
    pub fn try_recv(&self) -> Option<Event> {
        self.receiver.try_recv().ok().map(|arc| (*arc).clone())
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event, RecvTimeoutError> {
        self.receiver
            .recv_timeout(timeout)
            .map(|arc| (*arc).clone())
    }

    pub fn drain(&self) -> Vec<Event> {
        self.receiver.try_iter().map(|arc| (*arc).clone()).collect()
    }

    /// 有限消费，防止单帧消费过多事件导致卡顿。
    pub fn drain_limited(&self, max: usize) -> Vec<Event> {
        self.receiver
            .try_iter()
            .take(max)
            .map(|arc| (*arc).clone())
            .collect()
    }

    /// 零 clone 消费：返回 `Arc<Event>` 引用，避免 clone 开销。
    /// 适用于高频场景下只需要读取事件数据的消费者。
    pub fn try_recv_arc(&self) -> Option<Arc<Event>> {
        self.receiver.try_recv().ok()
    }

    /// 暴露底层接收端，用于 `crossbeam_channel::select!` 同时等待多个事件源。
    /// 调用者应只在需要零延迟唤醒的内部调度路径使用。
    pub fn receiver_arc(&self) -> &Receiver<Arc<Event>> {
        &self.receiver
    }

    /// 零 clone 批量消费：返回 `Arc<Event>` 引用列表。
    pub fn drain_arc(&self) -> Vec<Arc<Event>> {
        self.receiver.try_iter().collect()
    }

    /// 零 clone 有限消费。
    pub fn drain_limited_arc(&self, max: usize) -> Vec<Arc<Event>> {
        self.receiver.try_iter().take(max).collect()
    }

    /// 此订阅自创建以来丢弃的事件总数（仅 bounded channel 有效）。
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl DataBus {
    pub fn new() -> Self {
        Self::with_history_limit(DEFAULT_HISTORY_LIMIT)
    }

    pub fn with_history_limit(history_limit: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                subscribers: Mutex::new(Vec::new()),
                history: Mutex::new(VecDeque::new()),
                next_id: AtomicU64::new(1),
                history_limit,
            }),
        }
    }

    pub fn publish(&self, mut event: Event) -> Event {
        if event.id == 0 {
            event.id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        }

        let arc = Arc::new(event.clone());

        {
            let mut history = self.inner.history.lock();
            history.push_back(Arc::clone(&arc));
            while history.len() > self.inner.history_limit {
                history.pop_front();
            }
        }

        let mut subscribers = self.inner.subscribers.lock();
        subscribers.retain(|subscriber| {
            if subscriber.filter.matches_event(&arc) {
                // Arc::clone 只增加引用计数，避免对每个 subscriber 都完整 clone Event
                match subscriber.sender.try_send(Arc::clone(&arc)) {
                    Ok(()) => true,
                    Err(TrySendError::Full(_)) => {
                        subscriber.dropped.fetch_add(1, Ordering::Relaxed);
                        true
                    }
                    Err(TrySendError::Disconnected(_)) => false,
                }
            } else {
                true
            }
        });

        event
    }

    /// 无界（lossless）订阅：永不因队列满而丢弃事件。
    /// 适用于录制、测试断言等完整性敏感的场景。
    /// 极端情况下生产者快于消费者会导致内存增长，需配合背压或限速使用。
    pub fn subscribe_lossless(&self, filter: TopicFilter) -> Subscription {
        let (sender, receiver) = unbounded();
        let dropped = Arc::new(AtomicU64::new(0));
        self.inner.subscribers.lock().push(Subscriber {
            filter,
            sender,
            dropped: Arc::clone(&dropped),
        });
        Subscription { receiver, dropped }
    }

    /// [`subscribe_lossless`] 的别名，向后兼容。
    pub fn subscribe(&self, filter: TopicFilter) -> Subscription {
        self.subscribe_lossless(filter)
    }

    /// 有界（lossy）订阅：超过容量时丢弃新事件并计入 dropped_count（DropIncoming 策略）。
    /// 适用于 UI 面板、图表、日志等可容忍丢帧的场景。
    /// 完整性需求请用 [`subscribe_lossless`]。
    pub fn subscribe_lossy_bounded(&self, filter: TopicFilter, capacity: usize) -> Subscription {
        let (sender, receiver) = crossbeam_channel::bounded(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        self.inner.subscribers.lock().push(Subscriber {
            filter,
            sender,
            dropped: Arc::clone(&dropped),
        });
        Subscription { receiver, dropped }
    }

    /// 有界订阅：超过容量时丢弃新事件并计入 dropped_count（DropIncoming 策略）。
    /// 适用于 UI 面板等可容忍丢帧的场景。完整性需求请用 `subscribe_lossless()`。
    #[deprecated(
        note = "Use subscribe_lossy_bounded for UI lossy consumers, or subscribe_lossless for integrity-sensitive consumers. Never use for recorder."
    )]
    pub fn subscribe_bounded(&self, filter: TopicFilter, capacity: usize) -> Subscription {
        self.subscribe_lossy_bounded(filter, capacity)
    }

    pub fn history(&self) -> Vec<Event> {
        self.inner
            .history
            .lock()
            .iter()
            .map(|arc| (**arc).clone())
            .collect()
    }

    pub fn clear_history(&self) {
        self.inner.history.lock().clear();
    }

    pub fn history_len(&self) -> usize {
        self.inner.history.lock().len()
    }

    pub fn published_count(&self) -> u64 {
        self.inner.next_id.load(Ordering::Relaxed).saturating_sub(1)
    }
}

impl Default for DataBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_core::{Direction, Event, Payload};

    fn ev(topic: &str) -> Event {
        Event::new(topic, "test", Direction::Internal, Payload::Empty)
    }
    #[test]
    fn topic_filter_matches_variants() {
        // All
        assert!(TopicFilter::All.matches("anything"));
        // Exact
        assert!(TopicFilter::exact("a.b").matches("a.b"));
        assert!(!TopicFilter::exact("a.b").matches("a.c"));
        // Prefix
        assert!(TopicFilter::prefix("transport.serial.").matches("transport.serial.default.rx"));
        assert!(!TopicFilter::prefix("transport.serial.").matches("transport.usb.x"));
        // And
        let and = TopicFilter::and([TopicFilter::prefix("a."), TopicFilter::exact("a.b")]);
        assert!(and.matches("a.b"));
        assert!(!and.matches("a.c"));
        // MetadataEq 在 topic 级别总是通过（matches 不看 metadata）
        assert!(TopicFilter::metadata_eq("port", "COM1").matches("any.topic"));
    }

    #[test]
    fn metadata_eq_only_matches_string_values() {
        let mut event = ev("t");
        event.meta_set("port", serde_json::Value::String("COM1".to_owned()));
        assert!(TopicFilter::metadata_eq("port", "COM1").matches_event(&event));

        let mut bool_event = ev("t");
        bool_event.meta_set("replay", serde_json::Value::Bool(true));
        assert!(!TopicFilter::metadata_eq("replay", "true").matches_event(&bool_event));

        assert!(!TopicFilter::metadata_eq("missing", "x").matches_event(&ev("t")));
    }

    #[test]
    fn publish_assigns_monotonic_ids() {
        let bus = DataBus::new();
        let e1 = bus.publish(ev("t"));
        let e2 = bus.publish(ev("t"));
        assert!(e1.id > 0);
        assert_eq!(e2.id, e1.id + 1);
        assert_eq!(bus.published_count(), 2);
    }

    #[test]
    fn publish_preserves_nonzero_id() {
        let bus = DataBus::new();
        let mut event = ev("t");
        event.id = 999;
        let out = bus.publish(event);
        assert_eq!(out.id, 999);
        assert_eq!(bus.published_count(), 0);
    }

    #[test]
    fn history_truncates_to_limit() {
        let bus = DataBus::with_history_limit(3);
        for i in 0..5 {
            bus.publish(ev(&format!("t{i}")));
        }
        let history = bus.history();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].topic, "t2");
        assert_eq!(history[2].topic, "t4");
        assert_eq!(bus.history_len(), 3);
    }

    #[test]
    fn lossless_subscriber_receives_matching_events() {
        let bus = DataBus::new();
        let sub = bus.subscribe_lossless(TopicFilter::prefix("transport.serial."));
        bus.publish(ev("transport.serial.default.rx"));
        bus.publish(ev("log.system")); // 不匹配
        bus.publish(ev("transport.serial.default.tx"));
        let events = sub.drain();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].topic, "transport.serial.default.rx");
        assert_eq!(events[1].topic, "transport.serial.default.tx");
        assert_eq!(sub.dropped_count(), 0);
    }

    #[test]
    fn lossy_bounded_counts_drops_when_full() {
        let bus = DataBus::new();
        let sub = bus.subscribe_lossy_bounded(TopicFilter::All, 1);
        for _ in 0..4 {
            bus.publish(ev("t"));
        }
        assert_eq!(sub.drain().len(), 1);
        assert_eq!(sub.dropped_count(), 3);
    }

    #[test]
    fn drain_limited_caps_consumption() {
        let bus = DataBus::new();
        let sub = bus.subscribe_lossless(TopicFilter::All);
        for _ in 0..10 {
            bus.publish(ev("t"));
        }
        let first = sub.drain_limited(3);
        assert_eq!(first.len(), 3);
        let rest = sub.drain();
        assert_eq!(rest.len(), 7);
    }

    #[test]
    fn matches_event_on_real_event() {
        let event = ev("protocol.imu.attitude");
        assert!(TopicFilter::prefix("protocol.").matches_event(&event));
        assert!(TopicFilter::exact("protocol.imu.attitude").matches_event(&event));
        assert!(!TopicFilter::exact("protocol.imu.gps").matches_event(&event));
    }

    #[test]
    fn clear_history_empties() {
        let bus = DataBus::new();
        bus.publish(ev("t"));
        assert_eq!(bus.history_len(), 1);
        bus.clear_history();
        assert_eq!(bus.history_len(), 0);
    }

    #[test]
    fn arc_sharing_reduces_clones() {
        // 验证 publish 只 clone Event 一次（创建 Arc），多个 subscriber 共享同一 Arc
        let bus = DataBus::new();
        let sub1 = bus.subscribe_lossless(TopicFilter::All);
        let sub2 = bus.subscribe_lossless(TopicFilter::All);

        bus.publish(ev("t"));

        // 使用 drain_arc 验证 Arc 引用计数
        let arcs1 = sub1.drain_arc();
        let arcs2 = sub2.drain_arc();
        assert_eq!(arcs1.len(), 1);
        assert_eq!(arcs2.len(), 1);
        // 两个 Arc 指向同一个 Event（引用计数为 2，加上 history 中的 = 3）
        assert!(Arc::ptr_eq(&arcs1[0], &arcs2[0]));
    }
}
