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

    /// 便捷构造：匹配特定串口的所有事件（topic 前缀 + port metadata）。
    pub fn serial_port(port: impl Into<String>) -> Self {
        Self::and([
            Self::prefix("transport.serial."),
            Self::metadata_eq("port", port),
        ])
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
    history: Mutex<VecDeque<Event>>,
    next_id: AtomicU64,
    history_limit: usize,
}

struct Subscriber {
    filter: TopicFilter,
    sender: Sender<Event>,
    dropped: Arc<AtomicU64>,
}

pub struct Subscription {
    receiver: Receiver<Event>,
    dropped: Arc<AtomicU64>,
}

impl Subscription {
    pub fn try_recv(&self) -> Option<Event> {
        self.receiver.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Event, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    pub fn drain(&self) -> Vec<Event> {
        self.receiver.try_iter().collect()
    }

    /// 有限消费，防止单帧消费过多事件导致卡顿。
    pub fn drain_limited(&self, max: usize) -> Vec<Event> {
        self.receiver.try_iter().take(max).collect()
    }

    /// 此订阅自创建以来丢弃的事件总数（仅 bounded channel 有效）。
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl DataBus {
    pub fn new() -> Self {
        Self::with_history_limit(20_000)
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

        {
            let mut history = self.inner.history.lock();
            history.push_back(event.clone());
            while history.len() > self.inner.history_limit {
                history.pop_front();
            }
        }

        let mut subscribers = self.inner.subscribers.lock();
        subscribers.retain(|subscriber| {
            if subscriber.filter.matches_event(&event) {
                // 区分 Full（队列满，保留 subscriber 但计丢）和 Disconnected（真正断开，删除）
                match subscriber.sender.try_send(event.clone()) {
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

    /// 有界（lossy）订阅：超过容量时丢弃当前事件并计入 dropped_count（DropNewest 策略）。
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

    /// 有界订阅：超过容量时丢弃当前事件并计入 dropped_count（DropNewest 策略）。
    /// 适用于 UI 面板等可容忍丢帧的场景。完整性需求请用 `subscribe_lossless()`。
    #[deprecated(
        note = "Use subscribe_lossy_bounded for UI lossy consumers, or subscribe_lossless for integrity-sensitive consumers. Never use for recorder."
    )]
    pub fn subscribe_bounded(&self, filter: TopicFilter, capacity: usize) -> Subscription {
        self.subscribe_lossy_bounded(filter, capacity)
    }

    pub fn history(&self) -> Vec<Event> {
        self.inner.history.lock().iter().cloned().collect()
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
    use tool_core::{Direction, Payload, topics};

    #[test]
    fn publishes_to_matching_subscribers() {
        let bus = DataBus::new();
        let rx = bus.subscribe(TopicFilter::exact(topics::SERIAL_RX));
        let logs = bus.subscribe(TopicFilter::prefix("log."));

        bus.publish(Event::new(
            topics::SERIAL_RX,
            "test",
            Direction::Rx,
            Payload::Bytes(vec![1, 2, 3]),
        ));

        assert_eq!(rx.try_recv().unwrap().payload_len(), 3);
        assert!(logs.try_recv().is_none());
    }

    #[test]
    fn prefix_filter_receives_log_events() {
        let bus = DataBus::new();
        let logs = bus.subscribe(TopicFilter::prefix("log."));

        bus.publish(Event::system_log(
            tool_core::LogLevel::Info,
            "test",
            "hello",
        ));

        assert_eq!(logs.try_recv().unwrap().payload.text_lossy(), "hello");
    }

    #[test]
    fn history_is_limited() {
        let bus = DataBus::with_history_limit(2);
        for index in 0..3 {
            bus.publish(Event::new(
                format!("test.{index}"),
                "test",
                Direction::Internal,
                Payload::Empty,
            ));
        }

        let history = bus.history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].topic, "test.1");
    }
}
