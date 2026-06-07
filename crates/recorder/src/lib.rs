use std::fs::{File, create_dir_all};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tool_core::{Event, LogLevel};
use tool_databus::{DataBus, TopicFilter};

pub struct JsonlRecorder {
    bus: DataBus,
    worker: Option<RecorderWorker>,
    current_path: Option<PathBuf>,
}

struct RecorderWorker {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl JsonlRecorder {
    pub fn new(bus: DataBus) -> Self {
        Self {
            bus,
            worker: None,
            current_path: None,
        }
    }

    pub fn start(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        self.stop();

        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            create_dir_all(parent)?;
        }

        let file = File::create(&path)?;
        let subscription = self.bus.subscribe(TopicFilter::All);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);

        let join = thread::spawn(move || {
            let mut writer = BufWriter::new(file);

            while !stop_thread.load(Ordering::Relaxed) {
                match subscription.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => {
                        if !event
                            .metadata
                            .get("replay")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                        {
                            let _ = write_event(&mut writer, &event);
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }

            for event in subscription.drain() {
                let _ = write_event(&mut writer, &event);
            }

            let _ = writer.flush();
        });

        self.worker = Some(RecorderWorker {
            stop,
            join: Some(join),
        });
        self.current_path = Some(path.clone());
        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "recorder",
            format!("recording to {}", path.display()),
        ));
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            self.bus.publish(Event::system_log(
                LogLevel::Info,
                "recorder",
                "recording stopped",
            ));
            worker.stop.store(true, Ordering::Relaxed);
            if let Some(join) = worker.join.take() {
                let _ = join.join();
            }
        }
        self.current_path = None;
    }

    pub fn is_running(&self) -> bool {
        self.worker.is_some()
    }

    pub fn current_path(&self) -> Option<&Path> {
        self.current_path.as_deref()
    }
}

impl Drop for JsonlRecorder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn write_event(writer: &mut impl Write, event: &Event) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, event)?;
    writer.write_all(b"\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayState {
    Empty,
    Loaded,
    Playing,
    Paused,
    Finished,
}

#[derive(Debug, Clone)]
pub struct ReplayStatus {
    pub state: ReplayState,
    pub path: Option<PathBuf>,
    pub total_events: usize,
    pub cursor: usize,
    pub speed: f64,
    pub position_ms: u64,
    pub duration_ms: u64,
}

pub struct ReplayManager {
    bus: DataBus,
    events: Vec<Event>,
    path: Option<PathBuf>,
    cursor: usize,
    state: ReplayState,
    speed: f64,
    replay_start: Option<Instant>,
    position_at_start_ms: u64,
}

impl ReplayManager {
    pub fn new(bus: DataBus) -> Self {
        Self {
            bus,
            events: Vec::new(),
            path: None,
            cursor: 0,
            state: ReplayState::Empty,
            speed: 1.0,
            replay_start: None,
            position_at_start_ms: 0,
        }
    }

    pub fn load(&mut self, path: impl AsRef<Path>) -> io::Result<usize> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        let mut events = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(event) = serde_json::from_str::<Event>(trimmed) {
                events.push(event);
            }
        }
        events.sort_by_key(|event| event.timestamp_ms);

        self.events = events;
        self.path = Some(path.clone());
        self.cursor = 0;
        self.position_at_start_ms = 0;
        self.replay_start = None;
        self.state = if self.events.is_empty() {
            ReplayState::Empty
        } else {
            ReplayState::Loaded
        };
        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "replay",
            format!(
                "loaded {} event(s) from {}",
                self.events.len(),
                path.display()
            ),
        ));
        Ok(self.events.len())
    }

    pub fn play(&mut self) {
        if self.events.is_empty() {
            self.state = ReplayState::Empty;
            return;
        }
        if self.cursor >= self.events.len() {
            self.seek_ms(0);
        }
        self.position_at_start_ms = self.position_ms();
        self.replay_start = Some(Instant::now());
        self.state = ReplayState::Playing;
        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "replay",
            "playback started",
        ));
    }

    pub fn pause(&mut self) {
        if self.state == ReplayState::Playing {
            self.position_at_start_ms = self.position_ms();
            self.replay_start = None;
            self.state = ReplayState::Paused;
            self.bus.publish(Event::system_log(
                LogLevel::Info,
                "replay",
                "playback paused",
            ));
        }
    }

    pub fn stop(&mut self) {
        self.cursor = 0;
        self.position_at_start_ms = 0;
        self.replay_start = None;
        self.state = if self.events.is_empty() {
            ReplayState::Empty
        } else {
            ReplayState::Loaded
        };
        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "replay",
            "playback stopped",
        ));
    }

    pub fn seek_ms(&mut self, position_ms: u64) {
        let base = self.base_timestamp_ms().unwrap_or_default();
        self.cursor = self
            .events
            .iter()
            .position(|event| event.timestamp_ms.saturating_sub(base) >= position_ms)
            .unwrap_or(self.events.len());
        self.position_at_start_ms = position_ms.min(self.duration_ms());
        self.replay_start = if self.state == ReplayState::Playing {
            Some(Instant::now())
        } else {
            None
        };
    }

    /// 回退并重放到指定位置（用于拖动进度条）
    /// 返回重放的事件数，调用方应在调用前清空 UI 面板
    pub fn seek_with_replay(&mut self, position_ms: u64) -> usize {
        self.cursor = 0;
        self.position_at_start_ms = position_ms.min(self.duration_ms());
        self.replay_start = None;
        self.state = ReplayState::Paused;
        self.publish_until(position_ms)
    }

    /// 逐事件前进：发布当前事件
    pub fn step_forward(&mut self) -> usize {
        if self.cursor < self.events.len() {
            self.publish_cursor_event();
        }
        self.cursor
    }

    /// 逐事件后退：回到上一个事件位置
    pub fn step_backward(&mut self) {
        if self.cursor > 0 {
            let prev = self.cursor - 1;
            let base = self.base_timestamp_ms().unwrap_or(0);
            let pos = if prev == 0 {
                0
            } else {
                self.events[prev - 1].timestamp_ms.saturating_sub(base)
            };
            self.seek_ms(pos);
            // 重放到新位置
            self.publish_until(self.position_ms());
        }
    }

    /// 计算后退一步的目标位置（不发布事件）
    pub fn backward_position(&self) -> Option<u64> {
        if self.cursor == 0 {
            return None;
        }
        let base = self.base_timestamp_ms()?;
        let prev = self.cursor - 1;
        let pos = if prev == 0 {
            0
        } else {
            self.events[prev - 1].timestamp_ms.saturating_sub(base)
        };
        Some(pos)
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.position_at_start_ms = self.position_ms();
        self.replay_start = if self.state == ReplayState::Playing {
            Some(Instant::now())
        } else {
            None
        };
        self.speed = speed.clamp(0.1, 32.0);
    }

    pub fn tick(&mut self) -> usize {
        if self.state != ReplayState::Playing {
            return 0;
        }
        let target_position = self.position_ms();
        let published = self.publish_until(target_position);
        if self.cursor >= self.events.len() {
            self.state = ReplayState::Finished;
            self.replay_start = None;
            self.position_at_start_ms = self.duration_ms();
            self.bus.publish(Event::system_log(
                LogLevel::Info,
                "replay",
                "playback finished",
            ));
        }
        published
    }

    pub fn status(&self) -> ReplayStatus {
        ReplayStatus {
            state: self.state,
            path: self.path.clone(),
            total_events: self.events.len(),
            cursor: self.cursor,
            speed: self.speed,
            position_ms: self.position_ms(),
            duration_ms: self.duration_ms(),
        }
    }

    pub fn publish_next_for_test(&mut self, count: usize) -> usize {
        let end = (self.cursor + count).min(self.events.len());
        let mut published = 0;
        while self.cursor < end {
            self.publish_cursor_event();
            published += 1;
        }
        published
    }

    fn publish_until(&mut self, target_position_ms: u64) -> usize {
        let Some(base) = self.base_timestamp_ms() else {
            return 0;
        };
        let mut published = 0;
        while let Some(event) = self.events.get(self.cursor) {
            let event_position = event.timestamp_ms.saturating_sub(base);
            if event_position > target_position_ms {
                break;
            }
            self.publish_cursor_event();
            published += 1;
        }
        published
    }

    fn publish_cursor_event(&mut self) {
        let Some(event) = self.events.get(self.cursor).cloned() else {
            return;
        };
        self.cursor += 1;
        self.bus.publish(mark_replay_event(event));
    }

    fn position_ms(&self) -> u64 {
        if self.state == ReplayState::Playing
            && let Some(started) = self.replay_start
        {
            let elapsed = started.elapsed().as_millis() as f64 * self.speed;
            return self
                .position_at_start_ms
                .saturating_add(elapsed.max(0.0) as u64)
                .min(self.duration_ms());
        }
        self.position_at_start_ms.min(self.duration_ms())
    }

    fn duration_ms(&self) -> u64 {
        let Some(base) = self.base_timestamp_ms() else {
            return 0;
        };
        self.events
            .last()
            .map(|event| event.timestamp_ms.saturating_sub(base))
            .unwrap_or_default()
    }

    fn base_timestamp_ms(&self) -> Option<u64> {
        self.events.first().map(|event| event.timestamp_ms)
    }
}

fn mark_replay_event(mut event: Event) -> Event {
    let metadata = event.metadata.as_object_mut();
    if let Some(metadata) = metadata {
        metadata.insert("replay".to_owned(), serde_json::Value::Bool(true));
        metadata.insert(
            "original_source".to_owned(),
            serde_json::Value::String(event.source.clone()),
        );
    } else {
        event.metadata = serde_json::json!({
            "replay": true,
            "original_source": event.source
        });
    }
    event.source = format!("replay:{}", event.source);
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, thread};
    use tool_core::{Direction, Payload, now_timestamp_ms};

    #[test]
    fn records_bus_events_as_jsonl() {
        let bus = DataBus::new();
        let path = std::env::temp_dir().join(format!(
            "hardware-workbench-test-{}.jsonl",
            now_timestamp_ms()
        ));
        let mut recorder = JsonlRecorder::new(bus.clone());

        recorder.start(&path).unwrap();
        bus.publish(Event::new(
            "test.topic",
            "test",
            Direction::Internal,
            Payload::Text("hello".to_owned()),
        ));
        thread::sleep(Duration::from_millis(150));
        recorder.stop();

        let text = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert!(text.contains("\"topic\":\"test.topic\""));
        assert!(text.contains("hello"));
    }

    #[test]
    fn replay_loads_and_republishes_jsonl_events() {
        let bus = DataBus::new();
        let rx = bus.subscribe(TopicFilter::exact("test.topic"));
        let path = std::env::temp_dir().join(format!(
            "hardware-workbench-replay-test-{}.jsonl",
            now_timestamp_ms()
        ));
        let events = [
            Event::new(
                "test.topic",
                "fixture",
                Direction::Internal,
                Payload::Text("one".to_owned()),
            ),
            Event::new(
                "test.topic",
                "fixture",
                Direction::Internal,
                Payload::Text("two".to_owned()),
            ),
        ];
        {
            let mut file = File::create(&path).unwrap();
            for event in events {
                write_event(&mut file, &event).unwrap();
            }
        }

        let mut replay = ReplayManager::new(bus);
        assert_eq!(replay.load(&path).unwrap(), 2);
        assert_eq!(replay.publish_next_for_test(2), 2);
        let _ = fs::remove_file(&path);

        let events = rx.drain();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].payload.text_lossy(), "one");
        assert_eq!(events[0].metadata["replay"], true);
        assert!(events[0].source.starts_with("replay:"));
    }

    #[test]
    fn replay_preserves_protocol_topics_for_panels() {
        let bus = DataBus::new();
        let pid_rx = bus.subscribe(TopicFilter::exact(tool_core::topics::PROTOCOL_PID_SAMPLE));
        let imu_rx = bus.subscribe(TopicFilter::exact(tool_core::topics::PROTOCOL_IMU_ATTITUDE));
        let path = std::env::temp_dir().join(format!(
            "hardware-workbench-replay-protocol-test-{}.jsonl",
            now_timestamp_ms()
        ));
        let pid = Event::json(
            tool_core::topics::PROTOCOL_PID_SAMPLE,
            "fixture",
            serde_json::json!({ "t": 1, "target": 10, "actual": 9, "output": 0.2 }),
        );
        let mut imu = Event::json(
            tool_core::topics::PROTOCOL_IMU_ATTITUDE,
            "fixture",
            serde_json::json!({ "roll": 1, "pitch": 2, "yaw": 3 }),
        );
        imu.timestamp_ms = pid.timestamp_ms + 10;
        {
            let mut file = File::create(&path).unwrap();
            write_event(&mut file, &pid).unwrap();
            write_event(&mut file, &imu).unwrap();
        }

        let mut replay = ReplayManager::new(bus);
        replay.load(&path).unwrap();
        replay.publish_next_for_test(2);
        let _ = fs::remove_file(&path);

        assert_eq!(pid_rx.drain().len(), 1);
        assert_eq!(imu_rx.drain().len(), 1);
    }
}
