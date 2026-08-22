//! Application Event — 已发生的事实（复用 `tool_core::Event` / `tool_databus::DataBus`）。
//!
//! 本模块不新建 EventBus，仅定义应用层语义化 Event 的构造/识别辅助。

use tool_core::{Event, LogLevel};

pub use tool_core::topics;

/// 应用层事件构造辅助（发布仍走 `DataBus::publish`）。
pub fn app_event(topic: &str, source: &str, message: &str) -> Event {
    let mut e = Event::system_log(LogLevel::Info, source, message);
    e.topic = topic.to_owned();
    e
}
