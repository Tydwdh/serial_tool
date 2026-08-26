use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError, unbounded};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use tool_core::{Direction, Event, Payload};

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
    perf: DataBusPerfCounters,
}

#[derive(Default)]
struct DataBusPerfCounters {
    publish_count: AtomicU64,
    publish_bytes: AtomicU64,
    publish_nanos: AtomicU64,
    rx_bytes: AtomicU64,
    tx_bytes: AtomicU64,
    dropped: AtomicU64,
}

struct Subscriber {
    filter: TopicFilter,
    sink: SubscriberSink,
    dropped: Arc<AtomicU64>,
    backlog: SubscriptionBacklog,
}

enum SubscriberSink {
    Channel(Sender<Arc<Event>>),
    Ring {
        queue: Weak<Mutex<VecDeque<Arc<Event>>>>,
        capacity: usize,
    },
}

pub struct Subscription {
    receiver: Receiver<Arc<Event>>,
    dropped: Arc<AtomicU64>,
    backlog: SubscriptionBacklog,
}

/// 订阅队列积压的轻量级共享快照句柄。
///
/// `queued_bytes` 是按事件字段估算的大小，用于背压告警与性能诊断，
/// 不承诺等于最终序列化后的文件大小。
#[derive(Clone, Default)]
pub struct SubscriptionBacklog {
    queued_events: Arc<AtomicU64>,
    queued_bytes: Arc<AtomicU64>,
    oldest_timestamp_ms: Arc<AtomicU64>,
}

impl SubscriptionBacklog {
    pub fn queued_events(&self) -> u64 {
        self.queued_events.load(Ordering::Relaxed)
    }

    pub fn queued_bytes(&self) -> u64 {
        self.queued_bytes.load(Ordering::Relaxed)
    }

    pub fn seconds_behind(&self) -> f64 {
        let oldest = self.oldest_timestamp_ms.load(Ordering::Relaxed);
        if oldest == 0 {
            return 0.0;
        }
        tool_core::now_timestamp_ms().saturating_sub(oldest) as f64 / 1000.0
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DataBusPerfSnapshot {
    pub publish_count: u64,
    pub publish_bytes: u64,
    pub publish_nanos: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub subscriber_queued_events: u64,
    pub subscriber_queued_bytes: u64,
    pub subscriber_dropped: u64,
}

fn estimated_event_bytes(event: &Event) -> u64 {
    let payload = match &event.payload {
        Payload::Empty => 0,
        Payload::Bytes(bytes) => bytes.len(),
        Payload::Text(text) => text.len(),
        Payload::Json(value) => value.to_string().len(),
    };
    let metadata = event.metadata.to_string().len();
    (event.topic.len() + event.source.len() + payload + metadata + 64) as u64
}

fn event_payload_bytes(event: &Event) -> u64 {
    match &event.payload {
        Payload::Empty => 0,
        Payload::Bytes(bytes) => bytes.len() as u64,
        Payload::Text(text) => text.len() as u64,
        Payload::Json(value) => value.to_string().len() as u64,
    }
}

fn enqueue_backlog(backlog: &SubscriptionBacklog, event: &Event) {
    let was_empty = backlog.queued_events.fetch_add(1, Ordering::Relaxed) == 0;
    if was_empty {
        backlog
            .oldest_timestamp_ms
            .store(event.timestamp_ms, Ordering::Relaxed);
    }
    backlog
        .queued_bytes
        .fetch_add(estimated_event_bytes(event), Ordering::Relaxed);
}

fn decrement_counter(counter: &AtomicU64, amount: u64) -> u64 {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let next = current.saturating_sub(amount);
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(observed) => current = observed,
        }
    }
}

fn dequeue_backlog(backlog: &SubscriptionBacklog, event: &Event) {
    let remaining = decrement_counter(&backlog.queued_events, 1);
    backlog
        .queued_bytes
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_sub(estimated_event_bytes(event)))
        })
        .ok();
    if remaining == 0 {
        backlog.oldest_timestamp_ms.store(0, Ordering::Relaxed);
    }
}

/// Minimal event publishing capability used by presentation adapters.
///
/// Keeping this capability as a trait avoids making UI components depend on a
/// concrete application bus implementation.
pub trait EventPublisher {
    fn publish_event(&self, event: Event);
}

/// 发布线程直接写入的有界环形订阅。
///
/// 队列满时丢弃最旧事件并保留最新事件，适合 UI 消息流：窗口最小化时不依赖
/// UI 帧消费，内存仍有明确上限，并可通过 `take_dropped_count` 报告数据缺口。
pub struct RingSubscription {
    queue: Arc<Mutex<VecDeque<Arc<Event>>>>,
    dropped: Arc<AtomicU64>,
    backlog: SubscriptionBacklog,
}

impl RingSubscription {
    pub fn try_recv(&self) -> Option<Event> {
        self.queue.lock().pop_front().map(|arc| {
            let event = (*arc).clone();
            dequeue_backlog(&self.backlog, &event);
            event
        })
    }

    pub fn drain_limited(&self, max: usize) -> Vec<Event> {
        let mut queue = self.queue.lock();
        let take = max.min(queue.len());
        queue
            .drain(..take)
            .map(|arc| {
                let event = (*arc).clone();
                dequeue_backlog(&self.backlog, &event);
                event
            })
            .collect()
    }

    pub fn clear(&self) {
        let mut queue = self.queue.lock();
        queue.clear();
        self.backlog.queued_events.store(0, Ordering::Relaxed);
        self.backlog.queued_bytes.store(0, Ordering::Relaxed);
        self.backlog.oldest_timestamp_ms.store(0, Ordering::Relaxed);
        drop(queue);
    }

    pub fn len(&self) -> usize {
        self.queue.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.lock().is_empty()
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn take_dropped_count(&self) -> u64 {
        self.dropped.swap(0, Ordering::Relaxed)
    }

    pub fn backlog(&self) -> SubscriptionBacklog {
        self.backlog.clone()
    }
}

impl Subscription {
    pub fn try_recv(&self) -> Option<Event> {
        self.receiver.try_recv().ok().map(|arc| {
            let event = (*arc).clone();
            dequeue_backlog(&self.backlog, &event);
            event
        })
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout).map(|arc| {
            let event = (*arc).clone();
            dequeue_backlog(&self.backlog, &event);
            event
        })
    }

    pub fn drain(&self) -> Vec<Event> {
        self.receiver
            .try_iter()
            .map(|arc| {
                let event = (*arc).clone();
                dequeue_backlog(&self.backlog, &event);
                event
            })
            .collect()
    }

    /// 有限消费，防止单帧消费过多事件导致卡顿。
    pub fn drain_limited(&self, max: usize) -> Vec<Event> {
        self.receiver
            .try_iter()
            .take(max)
            .map(|arc| {
                let event = (*arc).clone();
                dequeue_backlog(&self.backlog, &event);
                event
            })
            .collect()
    }

    /// 零 clone 消费：返回 `Arc<Event>` 引用，避免 clone 开销。
    /// 适用于高频场景下只需要读取事件数据的消费者。
    pub fn try_recv_arc(&self) -> Option<Arc<Event>> {
        self.receiver
            .try_recv()
            .ok()
            .inspect(|event| dequeue_backlog(&self.backlog, event))
    }

    /// 暴露底层接收端，用于 `crossbeam_channel::select!` 同时等待多个事件源。
    /// 调用者应只在需要零延迟唤醒的内部调度路径使用。
    pub fn receiver_arc(&self) -> &Receiver<Arc<Event>> {
        &self.receiver
    }

    /// 零 clone 批量消费：返回 `Arc<Event>` 引用列表。
    pub fn drain_arc(&self) -> Vec<Arc<Event>> {
        self.receiver
            .try_iter()
            .inspect(|event| dequeue_backlog(&self.backlog, event))
            .collect()
    }

    /// 零 clone 有限消费。
    pub fn drain_limited_arc(&self, max: usize) -> Vec<Arc<Event>> {
        self.receiver
            .try_iter()
            .take(max)
            .inspect(|event| dequeue_backlog(&self.backlog, event))
            .collect()
    }

    /// 此订阅自创建以来丢弃的事件总数（仅 bounded channel 有效）。
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn queued_len(&self) -> u64 {
        self.backlog.queued_events()
    }

    pub fn queued_bytes(&self) -> u64 {
        self.backlog.queued_bytes()
    }

    pub fn backlog(&self) -> SubscriptionBacklog {
        self.backlog.clone()
    }

    pub fn clear(&self) {
        for event in self.receiver.try_iter() {
            dequeue_backlog(&self.backlog, &event);
        }
        self.backlog.queued_events.store(0, Ordering::Relaxed);
        self.backlog.queued_bytes.store(0, Ordering::Relaxed);
        self.backlog.oldest_timestamp_ms.store(0, Ordering::Relaxed);
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
                perf: DataBusPerfCounters::default(),
            }),
        }
    }

    pub fn publish(&self, mut event: Event) -> Event {
        let started = tool_core::monotonic_now_nanos();
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
                match &subscriber.sink {
                    SubscriberSink::Channel(sender) => match sender.try_send(Arc::clone(&arc)) {
                        Ok(()) => {
                            enqueue_backlog(&subscriber.backlog, &arc);
                            true
                        }
                        Err(TrySendError::Full(_)) => {
                            subscriber.dropped.fetch_add(1, Ordering::Relaxed);
                            self.inner.perf.dropped.fetch_add(1, Ordering::Relaxed);
                            true
                        }
                        Err(TrySendError::Disconnected(_)) => false,
                    },
                    SubscriberSink::Ring { queue, capacity } => {
                        let Some(queue) = queue.upgrade() else {
                            return false;
                        };
                        let mut queue = queue.lock();
                        if queue.len() >= *capacity {
                            if let Some(old) = queue.pop_front() {
                                dequeue_backlog(&subscriber.backlog, &old);
                            }
                            subscriber.dropped.fetch_add(1, Ordering::Relaxed);
                            self.inner.perf.dropped.fetch_add(1, Ordering::Relaxed);
                        }
                        queue.push_back(Arc::clone(&arc));
                        enqueue_backlog(&subscriber.backlog, &arc);
                        true
                    }
                }
            } else {
                true
            }
        });

        self.inner
            .perf
            .publish_count
            .fetch_add(1, Ordering::Relaxed);
        self.inner
            .perf
            .publish_bytes
            .fetch_add(estimated_event_bytes(&arc), Ordering::Relaxed);
        let payload_bytes = event_payload_bytes(&arc);
        if matches!(arc.direction, Direction::Rx) {
            self.inner
                .perf
                .rx_bytes
                .fetch_add(payload_bytes, Ordering::Relaxed);
        }
        if matches!(arc.direction, Direction::Tx) {
            self.inner
                .perf
                .tx_bytes
                .fetch_add(payload_bytes, Ordering::Relaxed);
        }
        self.inner.perf.publish_nanos.fetch_add(
            tool_core::monotonic_now_nanos().saturating_sub(started),
            Ordering::Relaxed,
        );

        event
    }

    /// 无界（lossless）订阅：永不因队列满而丢弃事件。
    /// 适用于录制、测试断言等完整性敏感的场景。
    /// 极端情况下生产者快于消费者会导致内存增长，需配合背压或限速使用。
    pub fn subscribe_lossless(&self, filter: TopicFilter) -> Subscription {
        let (sender, receiver) = unbounded();
        let dropped = Arc::new(AtomicU64::new(0));
        let backlog = SubscriptionBacklog::default();
        self.inner.subscribers.lock().push(Subscriber {
            filter,
            sink: SubscriberSink::Channel(sender),
            dropped: Arc::clone(&dropped),
            backlog: backlog.clone(),
        });
        Subscription {
            receiver,
            dropped,
            backlog,
        }
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
        let backlog = SubscriptionBacklog::default();
        self.inner.subscribers.lock().push(Subscriber {
            filter,
            sink: SubscriberSink::Channel(sender),
            dropped: Arc::clone(&dropped),
            backlog: backlog.clone(),
        });
        Subscription {
            receiver,
            dropped,
            backlog,
        }
    }

    /// 有界环形订阅：队列满时丢弃最旧事件，始终优先保留最新状态。
    pub fn subscribe_ring_bounded(&self, filter: TopicFilter, capacity: usize) -> RingSubscription {
        let capacity = capacity.max(1);
        let queue = Arc::new(Mutex::new(VecDeque::with_capacity(capacity)));
        let dropped = Arc::new(AtomicU64::new(0));
        let backlog = SubscriptionBacklog::default();
        self.inner.subscribers.lock().push(Subscriber {
            filter,
            sink: SubscriberSink::Ring {
                queue: Arc::downgrade(&queue),
                capacity,
            },
            dropped: Arc::clone(&dropped),
            backlog: backlog.clone(),
        });
        RingSubscription {
            queue,
            dropped,
            backlog,
        }
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

    pub fn perf_snapshot(&self) -> DataBusPerfSnapshot {
        let subscribers = self.inner.subscribers.lock();
        let subscriber_queued_events = subscribers
            .iter()
            .map(|subscriber| subscriber.backlog.queued_events())
            .sum();
        let subscriber_queued_bytes = subscribers
            .iter()
            .map(|subscriber| subscriber.backlog.queued_bytes())
            .sum();
        DataBusPerfSnapshot {
            publish_count: self.inner.perf.publish_count.load(Ordering::Relaxed),
            publish_bytes: self.inner.perf.publish_bytes.load(Ordering::Relaxed),
            publish_nanos: self.inner.perf.publish_nanos.load(Ordering::Relaxed),
            rx_bytes: self.inner.perf.rx_bytes.load(Ordering::Relaxed),
            tx_bytes: self.inner.perf.tx_bytes.load(Ordering::Relaxed),
            subscriber_queued_events,
            subscriber_queued_bytes,
            subscriber_dropped: self.inner.perf.dropped.load(Ordering::Relaxed),
        }
    }
}

impl EventPublisher for DataBus {
    fn publish_event(&self, event: Event) {
        let _ = self.publish(event);
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
        assert_eq!(sub.queued_len(), 2);
        assert!(sub.queued_bytes() > 0);
        let events = sub.drain();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].topic, "transport.serial.default.rx");
        assert_eq!(events[1].topic, "transport.serial.default.tx");
        assert_eq!(sub.dropped_count(), 0);
        assert_eq!(sub.queued_len(), 0);
        assert_eq!(sub.queued_bytes(), 0);
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
    fn ring_bounded_keeps_the_latest_events_and_counts_oldest_drops() {
        let bus = DataBus::new();
        let sub = bus.subscribe_ring_bounded(TopicFilter::All, 2);
        for index in 0..4 {
            bus.publish(ev(&format!("t{index}")));
        }

        let events = sub.drain_limited(10);
        assert_eq!(
            events
                .iter()
                .map(|event| event.topic.as_str())
                .collect::<Vec<_>>(),
            ["t2", "t3"]
        );
        assert_eq!(sub.take_dropped_count(), 2);
        assert_eq!(sub.take_dropped_count(), 0);
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

    #[test]
    #[ignore = "fixed pressure benchmark; run with --release --ignored --nocapture"]
    fn pressure_3mbps_rx_publish_and_drain() {
        let bus = DataBus::new();
        let subscription = bus.subscribe_lossless(TopicFilter::exact("transport.serial.rx"));
        let payload = "x".repeat(375);
        let started = std::time::Instant::now();
        for _ in 0..1_000 {
            bus.publish(Event::new(
                "transport.serial.rx",
                "COM1",
                Direction::Rx,
                Payload::Text(payload.clone()),
            ));
        }
        let queued = subscription.queued_len();
        let drained = subscription.drain_limited(2_000).len();
        let elapsed = started.elapsed();
        let snapshot = bus.perf_snapshot();
        println!(
            "databus pressure events={} queued={} drained={} rx_bytes={} elapsed={:?} publish_nanos={}",
            snapshot.publish_count,
            queued,
            drained,
            snapshot.rx_bytes,
            elapsed,
            snapshot.publish_nanos
        );
        assert_eq!(queued, 1_000);
        assert_eq!(drained, 1_000);
    }
}
