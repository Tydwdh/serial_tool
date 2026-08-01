//! DataBus → FFI 事件桥接。
//!
//! 订阅 DataBus 上的各种事件，转换为 `BackendEvent` 并推入队列，
//! 供 `wb_poll_events` 读取后通过 FFI 回调发送给 Flutter。

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use tool_core::{Event, LogLevel, Payload, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};

use crate::event::BackendEvent;

/// 事件桥接：订阅 DataBus 事件并推入队列。
pub struct EventBridge {
    /// 事件队列（发送端）
    tx: Sender<BackendEvent>,
    /// 事件队列（接收端，供 poll 使用）
    rx: Receiver<BackendEvent>,
    /// 各订阅的持有者
    _subscriptions: Vec<Subscription>,
    /// 串口数据事件是否暂停
    paused: Arc<Mutex<bool>>,
}

impl EventBridge {
    /// 创建新的 EventBridge，订阅所有需要的 DataBus 主题。
    pub fn new(bus: &DataBus) -> Self {
        let (tx, rx) = unbounded();
        let paused = Arc::new(Mutex::new(false));
        let subs = vec![
            bus.subscribe(TopicFilter::Prefix("serial.rx.".into())),
            bus.subscribe(TopicFilter::Prefix("serial.tx.".into())),
            bus.subscribe(TopicFilter::Prefix("sys.log.".into())),
            bus.subscribe(TopicFilter::Exact(topics::UI_SET_STATUS.into())),
            bus.subscribe(TopicFilter::Prefix("plugin.".into())),
            // Plugin-created panels and form updates use the ui.* topics.
            bus.subscribe(TopicFilter::Prefix("ui.".into())),
            bus.subscribe(TopicFilter::Exact("recorder.status".into())),
            bus.subscribe(TopicFilter::Prefix("replay.".into())),
            bus.subscribe(TopicFilter::Prefix("serial.status.".into())),
            bus.subscribe(TopicFilter::Prefix("protocol.".into())),
        ];

        Self {
            tx,
            rx,
            _subscriptions: subs,
            paused,
        }
    }

    /// 设置串口数据暂停状态。
    pub fn set_paused(&self, paused: bool) {
        *self.paused.lock() = paused;
    }

    /// 获取暂停状态。
    pub fn is_paused(&self) -> bool {
        *self.paused.lock()
    }

    /// 推入一个事件（供内部使用）。
    pub fn push_event(&self, event: BackendEvent) {
        let _ = self.tx.send(event);
    }

    /// 轮询所有待处理事件，返回一个批次。
    /// 总条数不超过 max_count（跨所有订阅累计）。
    pub fn poll(&self, max_count: usize) -> Vec<BackendEvent> {
        let mut events = Vec::with_capacity(max_count.min(64));
        let paused = self.is_paused();
        let mut remaining = max_count;

        // 1. Drain DataBus 订阅中的事件，总量不超过 max_count
        for sub in &self._subscriptions {
            if remaining == 0 {
                break;
            }
            for event in sub.drain_limited(remaining) {
                if let Some(be) = process_databus_event(&event, paused) {
                    events.push(be);
                    remaining -= 1;
                    if remaining == 0 {
                        break;
                    }
                }
            }
        }

        // 2. Drain 手动推送队列
        for _ in 0..remaining {
            match self.rx.try_recv() {
                Ok(evt) => events.push(evt),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        events
    }
}

/// 处理 DataBus 事件并转换为 BackendEvent。
pub fn process_databus_event(event: &Event, paused: bool) -> Option<BackendEvent> {
    // 串口数据
    if event.topic.starts_with("serial.rx.") || event.topic.starts_with("serial.tx.") {
        if paused {
            return None;
        }
        let direction = if event.topic.starts_with("serial.rx.") {
            tool_core::Direction::Rx
        } else {
            tool_core::Direction::Tx
        };
        let port = event
            .source
            .strip_prefix("serial:")
            .unwrap_or(&event.source)
            .to_owned();
        let data = match &event.payload {
            Payload::Bytes(b) => b.clone(),
            Payload::Text(t) => t.as_bytes().to_vec(),
            _ => return None,
        };
        return Some(BackendEvent::SerialData {
            port,
            direction,
            data,
            timestamp: event.timestamp_ms,
        });
    }

    // 系统日志
    if event.topic.starts_with("sys.log.") {
        let level = event
            .topic
            .strip_prefix("sys.log.")
            .and_then(|s| s.parse::<LogLevel>().ok())
            .unwrap_or(LogLevel::Info);
        let message = match &event.payload {
            Payload::Text(t) => t.clone(),
            Payload::Json(v) => v.to_string(),
            _ => event.source.clone(),
        };
        return Some(BackendEvent::Log {
            level,
            source: event.source.clone(),
            message,
        });
    }

    // 通知
    if event.topic == topics::UI_SET_STATUS
        && let Payload::Json(v) = &event.payload
    {
        let message = v
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_owned();
        return Some(BackendEvent::Notification {
            level: "info".to_owned(),
            message,
        });
    }

    // 录制状态
    if event.topic == "recorder.status"
        && let Payload::Json(v) = &event.payload
    {
        let recording = v
            .get("recording")
            .and_then(|r| r.as_bool())
            .unwrap_or(false);
        return Some(BackendEvent::RecorderStatus {
            recording,
            stats: None,
        });
    }

    // 串口状态
    if event.topic.starts_with("serial.status.") {
        let kind = event
            .topic
            .strip_prefix("serial.status.")
            .unwrap_or("unknown")
            .to_owned();
        let port = event
            .source
            .strip_prefix("serial:")
            .unwrap_or(&event.source)
            .to_owned();
        let message = match &event.payload {
            Payload::Text(t) => t.clone(),
            _ => String::new(),
        };
        return Some(BackendEvent::SerialEvent {
            port,
            kind,
            message,
        });
    }

    // 协议解码器输出：即使事件不来自插件，也应转给 Flutter 的动态面板。
    if event.topic.starts_with("protocol.")
        && let Payload::Json(data) = &event.payload
    {
        return Some(BackendEvent::ProtocolData {
            topic: event.topic.clone(),
            data: data.clone(),
            timestamp: event.timestamp_ms,
        });
    }

    // 插件事件。`ui.*` 的完整 topic 必须保留；Flutter 用它区分
    // `ui.panel.create`、`ui.form.set_value` 等动态面板协议。
    if event.source.starts_with("plugin:")
        && (event.topic.starts_with("plugin.") || event.topic.starts_with("ui."))
    {
        let plugin_id = event
            .source
            .strip_prefix("plugin:")
            .unwrap_or("")
            .to_owned();
        if !plugin_id.is_empty() {
            let kind = event.topic.clone();
            let data = match &event.payload {
                Payload::Json(v) => v.clone(),
                Payload::Text(t) => Value::String(t.clone()),
                _ => Value::Null,
            };
            return Some(BackendEvent::PluginEvent {
                plugin_id,
                kind,
                data,
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_core::Direction;

    #[test]
    fn preserves_dynamic_panel_topic_for_flutter() {
        let event = Event::new(
            "ui.panel.create",
            "plugin:demo",
            Direction::Internal,
            Payload::Json(serde_json::json!({"id": "demo.chart", "kind": "chart"})),
        );
        let result = process_databus_event(&event, false);
        match result {
            Some(BackendEvent::PluginEvent {
                plugin_id,
                kind,
                data,
            }) => {
                assert_eq!(plugin_id, "demo");
                assert_eq!(kind, "ui.panel.create");
                assert_eq!(data["id"], "demo.chart");
            }
            other => panic!("unexpected bridge result: {other:?}"),
        }
    }

    #[test]
    fn forwards_protocol_json_for_dynamic_visualizations() {
        let event = Event::new(
            "protocol.telemetry",
            "plugin:demo",
            Direction::Internal,
            Payload::Json(serde_json::json!({"value": 42.5, "roll": 2})),
        );
        let result = process_databus_event(&event, false);
        match result {
            Some(BackendEvent::ProtocolData { topic, data, .. }) => {
                assert_eq!(topic, "protocol.telemetry");
                assert_eq!(data["value"], 42.5);
            }
            other => panic!("unexpected bridge result: {other:?}"),
        }
    }
}
