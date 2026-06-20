//! 录制文件格式与过滤策略：纯函数，便于独立测试。
//!
//! 这里的函数定义了“哪些事件会被写入录制文件”（`should_record_event_with_mode`）
//! 以及“单条事件如何序列化为 jsonl 行”（`write_event_counted`），
//! 是录制文件格式的规范，与 `JsonlRecorder` 的线程/IO 逻辑解耦。

use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use tool_core::Event;

// ── RecordMode ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RecordMode {
    /// 只记录 transport.serial.* 原始事件
    RawSerial,
    /// 默认：串口 + protocol.* + ui.panel.create
    #[default]
    StandardReplay,
    /// 记录所有事件（除 replay/derived/recordable=false）
    FullDebug,
}

pub(crate) fn write_event_counted(writer: &mut impl Write, event: &Event) -> io::Result<u64> {
    let line =
        serde_json::to_string(event).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    Ok(line.len() as u64 + 1)
}

/// 所有 mode 都统一排除的事件。
pub(crate) fn is_excluded_event(event: &Event) -> bool {
    // 回放事件
    if event.is_replay() {
        return true;
    }
    // replay / replay_derived 来源
    if let Some(origin) = event.origin()
        && (origin == "replay" || origin == "replay_derived")
    {
        return true;
    }
    // recordable = false
    if !event.meta_bool("recordable") && event.meta_get("recordable").is_some() {
        return true;
    }
    false
}

pub(crate) fn should_record_event_with_mode(event: &Event, mode: RecordMode) -> bool {
    if is_excluded_event(event) {
        return false;
    }

    // 录制控制事件（暂停/继续）在所有模式下都写入
    if event.topic == "recorder.pause" || event.topic == "recorder.resume" {
        return true;
    }

    match mode {
        RecordMode::RawSerial => event.topic.starts_with("transport.serial."),
        RecordMode::StandardReplay => {
            event.topic.starts_with("transport.serial.")
                || event.topic.starts_with("protocol.")
                || event.topic == "ui.panel.create"
        }
        RecordMode::FullDebug => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_core::{Direction, Payload};

    fn ev(topic: &str) -> Event {
        Event::new(topic, "test", Direction::Internal, Payload::Empty)
    }

    #[test]
    fn raw_serial_only_records_serial() {
        assert!(should_record_event_with_mode(
            &ev("transport.serial.default.rx"),
            RecordMode::RawSerial
        ));
        assert!(!should_record_event_with_mode(
            &ev("protocol.imu.attitude"),
            RecordMode::RawSerial
        ));
        assert!(!should_record_event_with_mode(
            &ev("log.system"),
            RecordMode::RawSerial
        ));
    }

    #[test]
    fn standard_replay_includes_protocol_and_panel_create() {
        assert!(should_record_event_with_mode(
            &ev("transport.serial.default.rx"),
            RecordMode::StandardReplay
        ));
        assert!(should_record_event_with_mode(
            &ev("protocol.pid.sample"),
            RecordMode::StandardReplay
        ));
        assert!(should_record_event_with_mode(
            &ev("ui.panel.create"),
            RecordMode::StandardReplay
        ));
        assert!(!should_record_event_with_mode(
            &ev("log.system"),
            RecordMode::StandardReplay
        ));
    }

    #[test]
    fn full_debug_records_almost_everything() {
        assert!(should_record_event_with_mode(
            &ev("log.system"),
            RecordMode::FullDebug
        ));
        assert!(should_record_event_with_mode(
            &ev("anything.else"),
            RecordMode::FullDebug
        ));
    }

    #[test]
    fn recorder_markers_always_recorded() {
        // 暂停/继续 marker 在所有 mode 下都写入
        for mode in [
            RecordMode::RawSerial,
            RecordMode::StandardReplay,
            RecordMode::FullDebug,
        ] {
            assert!(should_record_event_with_mode(&ev("recorder.pause"), mode));
            assert!(should_record_event_with_mode(&ev("recorder.resume"), mode));
        }
    }

    #[test]
    fn replay_events_excluded_in_all_modes() {
        let mut event = ev("transport.serial.default.rx");
        event.meta_set("replay", serde_json::Value::Bool(true));
        for mode in [
            RecordMode::RawSerial,
            RecordMode::StandardReplay,
            RecordMode::FullDebug,
        ] {
            assert!(
                !should_record_event_with_mode(&event, mode),
                "replay event should be excluded in {:?}",
                mode
            );
        }
    }

    #[test]
    fn replay_derived_origin_excluded() {
        let mut event = ev("protocol.x");
        event.meta_set(
            "origin",
            serde_json::Value::String("replay_derived".to_owned()),
        );
        assert!(!should_record_event_with_mode(
            &event,
            RecordMode::FullDebug
        ));
    }

    #[test]
    fn recordable_false_excluded() {
        let mut event = ev("transport.serial.default.rx");
        event.meta_set("recordable", serde_json::Value::Bool(false));
        assert!(!should_record_event_with_mode(
            &event,
            RecordMode::RawSerial
        ));
        // 未设置 recordable 不排除
        assert!(should_record_event_with_mode(
            &ev("transport.serial.default.rx"),
            RecordMode::RawSerial
        ));
    }

    #[test]
    fn write_event_counted_produces_jsonl_line() {
        let event = ev("t");
        let mut buf = Vec::new();
        let bytes = write_event_counted(&mut buf, &event).unwrap();
        let line = String::from_utf8(buf).unwrap();
        assert!(line.ends_with('\n'));
        // 字节数 = 行长（含换行）
        assert_eq!(bytes, line.len() as u64);
        // 内容是合法 JSON
        let _: tool_core::Event = serde_json::from_str(line.trim()).unwrap();
    }
}
