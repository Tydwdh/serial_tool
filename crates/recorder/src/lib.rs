use serde::{Deserialize, Serialize};
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

// ── ReplayPolicy ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReplayPolicy {
    /// 默认：有 protocol.* 就用 Exact，否则尝试 ReparseRaw
    #[default]
    AutoPreferRecorded,
    /// 使用录制的 protocol.*，不运行 analyzer
    ExactRecorded,
    /// 忽略录制的 protocol.*，使用 analyzer 输出
    ReparseRaw,
}

pub struct JsonlRecorder {
    bus: DataBus,
    worker: Option<RecorderWorker>,
    current_path: Option<PathBuf>,
    mode: RecordMode,
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
            mode: RecordMode::default(),
        }
    }

    pub fn set_mode(&mut self, mode: RecordMode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> RecordMode {
        self.mode
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
        let mode = self.mode;

        let join = thread::spawn(move || {
            let mut writer = BufWriter::new(file);

            while !stop_thread.load(Ordering::Relaxed) {
                match subscription.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => {
                        if should_record_event_with_mode(&event, mode) {
                            let _ = write_event(&mut writer, &event);
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }

            for event in subscription.drain() {
                if should_record_event_with_mode(&event, mode) {
                    let _ = write_event(&mut writer, &event);
                }
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
            format!("recording to {} (mode: {:?})", path.display(), self.mode),
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

/// 所有 mode 都统一排除的事件。
fn is_excluded_event(event: &Event) -> bool {
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

fn should_record_event_with_mode(event: &Event, mode: RecordMode) -> bool {
    if is_excluded_event(event) {
        return false;
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
    pub policy: ReplayPolicy,
    pub effective_policy: ReplayPolicy,
    pub has_recorded_protocol: bool,
    pub analyzer_cache_entries: usize,
    pub analyzer_error: Option<String>,
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

    // ── 新增 ──
    policy: ReplayPolicy,
    has_recorded_protocol: bool,
    analyzer_cache: Vec<Event>,
    analyzer_cache_valid: bool,
    analyzer_error: Option<String>,
    analyzer_cursor: usize,
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
            policy: ReplayPolicy::default(),
            has_recorded_protocol: false,
            analyzer_cache: Vec::new(),
            analyzer_cache_valid: false,
            analyzer_error: None,
            analyzer_cursor: 0,
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
        events.sort_by_key(|event| (event.timestamp_ms, event.id));

        // 扫描是否存在录制的 protocol.*（非 replay 事件）
        self.has_recorded_protocol = events
            .iter()
            .any(|e| e.topic.starts_with("protocol.") && !e.is_replay());

        self.events = events;
        self.path = Some(path.clone());
        self.cursor = 0;
        self.position_at_start_ms = 0;
        self.replay_start = None;

        // load 时 invalidate analyzer cache（因为事件变了）
        self.analyzer_cache.clear();
        self.analyzer_cache_valid = false;
        self.analyzer_error = None;
        self.analyzer_cursor = 0;

        self.state = if self.events.is_empty() {
            ReplayState::Empty
        } else {
            ReplayState::Loaded
        };
        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "replay",
            format!(
                "loaded {} event(s) from {} (recorded_protocol: {}, policy: {:?})",
                self.events.len(),
                path.display(),
                self.has_recorded_protocol,
                self.policy,
            ),
        ));
        Ok(self.events.len())
    }

    // ── Policy ──

    pub fn set_policy(&mut self, policy: ReplayPolicy) {
        if self.policy != policy {
            self.policy = policy;
            // 切换 policy 时 invalidate cache
            self.analyzer_cache.clear();
            self.analyzer_cache_valid = false;
            self.analyzer_error = None;
            self.analyzer_cursor = 0;
        }
    }

    pub fn policy(&self) -> ReplayPolicy {
        self.policy
    }

    /// 解析当前实际执行的策略。
    /// AutoPreferRecorded → 有 protocol.* 就 Exact，否则 ReparseRaw
    pub fn effective_policy(&self) -> ReplayPolicy {
        match self.policy {
            ReplayPolicy::AutoPreferRecorded => {
                if self.has_recorded_protocol {
                    ReplayPolicy::ExactRecorded
                } else {
                    ReplayPolicy::ReparseRaw
                }
            }
            other => other,
        }
    }

    /// 是否在 ReparseRaw 模式下需要 analyzer 输出
    pub fn needs_analyzer(&self) -> bool {
        self.effective_policy() == ReplayPolicy::ReparseRaw
    }

    // ── Analyzer cache ──

    /// 获取所有原始串口事件（供 analyzer 使用）。
    /// 返回的是录制文件中非 replay、topic 以 transport.serial. 开头的事件。
    pub fn raw_serial_events(&self) -> Vec<Event> {
        self.events
            .iter()
            .filter(|e| e.topic.starts_with("transport.serial.") && !e.is_replay())
            .cloned()
            .collect()
    }

    /// 设置 analyzer 输出缓存。内部会按 (timestamp_ms, id) 排序。
    pub fn set_analyzer_cache(&mut self, mut events: Vec<Event>) {
        events.sort_by_key(|event| (event.timestamp_ms, event.id));
        self.analyzer_cache = events;
        self.analyzer_cache_valid = true;
        self.analyzer_error = None;
        self.analyzer_cursor = 0;
    }

    /// 标记 analyzer 失败。
    pub fn set_analyzer_error(&mut self, error: String) {
        self.analyzer_cache.clear();
        self.analyzer_cache_valid = false;
        self.analyzer_error = Some(error);
        self.analyzer_cursor = 0;
    }

    pub fn analyzer_cache_valid(&self) -> bool {
        self.analyzer_cache_valid
    }

    pub fn analyzer_error(&self) -> Option<&str> {
        self.analyzer_error.as_deref()
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
        self.analyzer_cursor = 0;
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
        self.analyzer_cursor = 0;
        self.position_at_start_ms = position_ms.min(self.duration_ms());
        self.replay_start = None;
        self.state = ReplayState::Paused;

        let recorded_count = self.publish_until(position_ms);

        // 在 ReparseRaw 模式下，额外发布 analyzer_cache 中 <= position_ms 的事件
        let analyzer_count =
            if self.effective_policy() == ReplayPolicy::ReparseRaw && self.analyzer_cache_valid {
                self.publish_analyzer_cache_until(position_ms)
            } else {
                0
            };

        recorded_count + analyzer_count
    }

    /// 发布 analyzer_cache 中时间戳 <= target 的未发布事件。
    fn publish_analyzer_cache_until(&mut self, target_position_ms: u64) -> usize {
        let base = self.base_timestamp_ms().unwrap_or_default();
        let mut count = 0;

        while let Some(event) = self.analyzer_cache.get(self.analyzer_cursor) {
            let event_position = event.timestamp_ms.saturating_sub(base);
            if event_position > target_position_ms {
                break;
            }

            self.bus.publish(event.clone());
            self.analyzer_cursor += 1;
            count += 1;
        }

        count
    }

    /// 逐事件前进：发布当前事件，并同步更新位置。
    pub fn step_forward(&mut self) -> usize {
        if self.cursor < self.events.len() {
            self.publish_cursor_event();

            let position_ms = self.cursor_position_ms().min(self.duration_ms());
            self.position_at_start_ms = position_ms;

            if self.state == ReplayState::Playing {
                self.replay_start = Some(Instant::now());
            } else {
                self.replay_start = None;
            }

            if self.effective_policy() == ReplayPolicy::ReparseRaw
                && self.analyzer_cache_valid
            {
                self.publish_analyzer_cache_until(position_ms);
            }
        }

        self.cursor
    }

    fn cursor_position_ms(&self) -> u64 {
        let Some(base) = self.base_timestamp_ms() else {
            return 0;
        };

        if self.cursor == 0 {
            return 0;
        }

        self.events
            .get(self.cursor.saturating_sub(1))
            .map(|event| event.timestamp_ms.saturating_sub(base))
            .unwrap_or_else(|| self.duration_ms())
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

    pub fn backward_position(&self) -> Option<u64> {
        self.backward_position_by(1)
    }

    pub fn backward_position_by(&self, steps: usize) -> Option<u64> {
        if self.events.is_empty() || self.cursor == 0 {
            return None;
        }

        let base = self.base_timestamp_ms()?;
        let steps = steps.max(1);

        // cursor 表示“下一个要发布的事件索引”。
        // 回退 N 步，就是希望最终重放到 cursor - N 之前的位置。
        let target_cursor = self.cursor.saturating_sub(steps);

        if target_cursor == 0 {
            return Some(0);
        }

        self.events
            .get(target_cursor - 1)
            .map(|event| event.timestamp_ms.saturating_sub(base))
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
        let mut published = self.publish_until(target_position);

        // 播放模式也发布 analyzer_cache
        if self.effective_policy() == ReplayPolicy::ReparseRaw && self.analyzer_cache_valid {
            published += self.publish_analyzer_cache_until(target_position);
        }

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
            policy: self.policy,
            effective_policy: self.effective_policy(),
            has_recorded_protocol: self.has_recorded_protocol,
            analyzer_cache_entries: self.analyzer_cache.len(),
            analyzer_error: self.analyzer_error.clone(),
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

        // ReparseRaw: 跳过录制的 protocol.* （由 analyzer_cache 替代）
        if self.effective_policy() == ReplayPolicy::ReparseRaw
            && event.topic.starts_with("protocol.")
        {
            return;
        }

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
    let original_source = event.source.clone();
    event.meta_set("replay", serde_json::Value::Bool(true));
    event.meta_set(
        "original_source",
        serde_json::Value::String(original_source),
    );
    event.meta_set("origin", serde_json::Value::String("replay".to_owned()));
    event.source = format!("replay:{}", event.source);
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, thread};
    use tool_core::{Direction, Payload, now_timestamp_ms, topics};

    // ── 旧测试（适配新 API） ──

    #[test]
    fn records_bus_events_as_jsonl() {
        let bus = DataBus::new();
        let path = std::env::temp_dir().join(format!(
            "hardware-workbench-test-{}.jsonl",
            now_timestamp_ms()
        ));
        let mut recorder = JsonlRecorder::new(bus.clone());
        recorder.set_mode(RecordMode::FullDebug);

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
        assert!(events[0].is_replay());
        assert!(events[0].source.starts_with("replay:"));
    }

    #[test]
    fn replay_preserves_protocol_topics_for_panels() {
        let bus = DataBus::new();
        let pid_rx = bus.subscribe(TopicFilter::exact(topics::PROTOCOL_PID_SAMPLE));
        let imu_rx = bus.subscribe(TopicFilter::exact(topics::PROTOCOL_IMU_ATTITUDE));
        let path = std::env::temp_dir().join(format!(
            "hardware-workbench-replay-protocol-test-{}.jsonl",
            now_timestamp_ms()
        ));
        let pid = Event::json(
            topics::PROTOCOL_PID_SAMPLE,
            "fixture",
            serde_json::json!({ "t": 1, "target": 10, "actual": 9, "output": 0.2 }),
        );
        let mut imu = Event::json(
            topics::PROTOCOL_IMU_ATTITUDE,
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

    // ── RecordMode 测试 ──

    #[test]
    fn raw_serial_mode_only_records_serial_topics() {
        assert!(should_record_event_with_mode(
            &Event::new(
                topics::SERIAL_RX,
                "test",
                Direction::Rx,
                Payload::Text("data".to_owned()),
            ),
            RecordMode::RawSerial
        ));

        assert!(!should_record_event_with_mode(
            &Event::new(
                topics::PROTOCOL_PID_SAMPLE,
                "test",
                Direction::Internal,
                Payload::Json(serde_json::json!({"t": 1})),
            ),
            RecordMode::RawSerial
        ));
    }

    #[test]
    fn standard_replay_mode_records_serial_protocol_and_panel_create() {
        let mode = RecordMode::StandardReplay;

        assert!(should_record_event_with_mode(
            &Event::new(
                topics::SERIAL_RX,
                "test",
                Direction::Rx,
                Payload::Text("data".to_owned()),
            ),
            mode
        ));

        assert!(should_record_event_with_mode(
            &Event::new(
                topics::PROTOCOL_PID_SAMPLE,
                "test",
                Direction::Internal,
                Payload::Json(serde_json::json!({"t": 1})),
            ),
            mode
        ));

        assert!(should_record_event_with_mode(
            &Event::new(
                topics::UI_PANEL_CREATE,
                "test",
                Direction::Internal,
                Payload::Json(serde_json::json!({"id": "chart"})),
            ),
            mode
        ));

        // 不记录 log.system
        assert!(!should_record_event_with_mode(
            &Event::new(
                topics::LOG_SYSTEM,
                "test",
                Direction::Internal,
                Payload::Text("msg".to_owned()),
            ),
            mode
        ));
    }

    #[test]
    fn full_debug_mode_records_all() {
        let mode = RecordMode::FullDebug;

        assert!(should_record_event_with_mode(
            &Event::new(
                topics::LOG_SYSTEM,
                "test",
                Direction::Internal,
                Payload::Text("msg".to_owned()),
            ),
            mode
        ));
    }

    #[test]
    fn all_modes_exclude_replay_events() {
        let mut event = Event::new(
            topics::SERIAL_RX,
            "test",
            Direction::Rx,
            Payload::Text("data".to_owned()),
        );
        event.meta_set("replay", serde_json::Value::Bool(true));

        for mode in [
            RecordMode::RawSerial,
            RecordMode::StandardReplay,
            RecordMode::FullDebug,
        ] {
            assert!(
                !should_record_event_with_mode(&event, mode),
                "mode {:?} should exclude replay events",
                mode
            );
        }
    }

    #[test]
    fn all_modes_exclude_recordable_false() {
        let mut event = Event::new(
            topics::SERIAL_RX,
            "test",
            Direction::Rx,
            Payload::Text("data".to_owned()),
        );
        event.meta_set("recordable", serde_json::Value::Bool(false));

        for mode in [
            RecordMode::RawSerial,
            RecordMode::StandardReplay,
            RecordMode::FullDebug,
        ] {
            assert!(
                !should_record_event_with_mode(&event, mode),
                "mode {:?} should exclude recordable=false",
                mode
            );
        }
    }

    // ── ReplayPolicy 测试 ──

    #[test]
    fn auto_prefer_recorded_detects_protocol() {
        let bus = DataBus::new();
        let path =
            std::env::temp_dir().join(format!("hw-policy-auto-{}.jsonl", now_timestamp_ms()));

        let serial = Event::new(
            topics::SERIAL_RX,
            "fixture",
            Direction::Rx,
            Payload::Text("data".to_owned()),
        );
        let protocol = Event::json(
            topics::PROTOCOL_PID_SAMPLE,
            "fixture",
            serde_json::json!({"t": 1, "target": 50, "actual": 43, "output": 0.71}),
        );
        {
            let mut file = File::create(&path).unwrap();
            write_event(&mut file, &serial).unwrap();
            write_event(&mut file, &protocol).unwrap();
        }

        let mut replay = ReplayManager::new(bus);
        replay.load(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert!(replay.has_recorded_protocol);
        assert_eq!(replay.effective_policy(), ReplayPolicy::ExactRecorded);
        assert!(!replay.needs_analyzer());
    }

    #[test]
    fn auto_prefer_recorded_falls_back_to_reparse() {
        let bus = DataBus::new();
        let path = std::env::temp_dir().join(format!(
            "hw-policy-auto-noproto-{}.jsonl",
            now_timestamp_ms()
        ));

        let serial = Event::new(
            topics::SERIAL_RX,
            "fixture",
            Direction::Rx,
            Payload::Text("data".to_owned()),
        );
        {
            let mut file = File::create(&path).unwrap();
            write_event(&mut file, &serial).unwrap();
        }

        let mut replay = ReplayManager::new(bus);
        replay.load(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert!(!replay.has_recorded_protocol);
        assert_eq!(replay.effective_policy(), ReplayPolicy::ReparseRaw);
        assert!(replay.needs_analyzer());
    }

    #[test]
    fn reparse_raw_skips_protocol_events() {
        let bus = DataBus::new();
        let rx = bus.subscribe(TopicFilter::exact(topics::PROTOCOL_PID_SAMPLE));
        let path =
            std::env::temp_dir().join(format!("hw-policy-reparse-{}.jsonl", now_timestamp_ms()));

        let serial = Event::new(
            topics::SERIAL_RX,
            "fixture",
            Direction::Rx,
            Payload::Text("data".to_owned()),
        );
        let protocol = Event::json(
            topics::PROTOCOL_PID_SAMPLE,
            "fixture",
            serde_json::json!({"t": 1}),
        );
        {
            let mut file = File::create(&path).unwrap();
            write_event(&mut file, &serial).unwrap();
            write_event(&mut file, &protocol).unwrap();
        }

        let mut replay = ReplayManager::new(bus);
        replay.set_policy(ReplayPolicy::ReparseRaw);
        replay.load(&path).unwrap();
        replay.publish_next_for_test(2);
        let _ = fs::remove_file(&path);

        // ReparseRaw 跳过 protocol.* → 不会发布 PID sample
        let protocol_events: Vec<_> = rx.drain().into_iter().collect();
        assert!(
            protocol_events.is_empty(),
            "ReparseRaw should skip recorded protocol.* events"
        );
    }

    #[test]
    fn exact_recorded_publishes_protocol_events() {
        let bus = DataBus::new();
        let rx = bus.subscribe(TopicFilter::exact(topics::PROTOCOL_PID_SAMPLE));
        let path =
            std::env::temp_dir().join(format!("hw-policy-exact-{}.jsonl", now_timestamp_ms()));

        let protocol = Event::json(
            topics::PROTOCOL_PID_SAMPLE,
            "fixture",
            serde_json::json!({"t": 1}),
        );
        {
            let mut file = File::create(&path).unwrap();
            write_event(&mut file, &protocol).unwrap();
        }

        let mut replay = ReplayManager::new(bus);
        replay.set_policy(ReplayPolicy::ExactRecorded);
        replay.load(&path).unwrap();
        replay.publish_next_for_test(1);
        let _ = fs::remove_file(&path);

        assert_eq!(rx.drain().len(), 1);
    }

    #[test]
    fn analyzer_cache_is_published_in_reparse_mode() {
        let bus = DataBus::new();
        let rx = bus.subscribe(TopicFilter::exact(topics::PROTOCOL_PID_SAMPLE));
        let path =
            std::env::temp_dir().join(format!("hw-cache-publish-{}.jsonl", now_timestamp_ms()));

        let serial = Event::new(
            topics::SERIAL_RX,
            "fixture",
            Direction::Rx,
            Payload::Text("raw".to_owned()),
        );
        {
            let mut file = File::create(&path).unwrap();
            write_event(&mut file, &serial).unwrap();
        }

        let mut replay = ReplayManager::new(bus);
        replay.set_policy(ReplayPolicy::ReparseRaw);
        replay.load(&path).unwrap();

        // 模拟 analyzer 输出
        let mut derived = Event::json(
            topics::PROTOCOL_PID_SAMPLE,
            "replay-analyzer:demo",
            serde_json::json!({"t": 1, "value": 100.0}),
        );
        derived.meta_set("replay", serde_json::Value::Bool(true));
        derived.meta_set(
            "origin",
            serde_json::Value::String("replay_derived".to_owned()),
        );
        replay.set_analyzer_cache(vec![derived]);

        // seek 应该发布 analyzer cache
        replay.seek_with_replay(1000);
        let _ = fs::remove_file(&path);

        assert_eq!(rx.drain().len(), 1);
    }

    #[test]
    fn no_duplicate_protocol_demo_sample() {
        // 核心测试：确保不会同时吃到录制的 protocol.demo.sample 和 analyzer 生成的
        let topic = "protocol.demo.sample";
        let path = std::env::temp_dir().join(format!("hw-no-dup-{}.jsonl", now_timestamp_ms()));

        let serial = Event::new(
            topics::SERIAL_RX,
            "fixture",
            Direction::Rx,
            Payload::Text("data".to_owned()),
        );
        let protocol = Event::json(topic, "fixture", serde_json::json!({"t": 1, "value": 50.0}));
        {
            let mut file = File::create(&path).unwrap();
            write_event(&mut file, &serial).unwrap();
            write_event(&mut file, &protocol).unwrap();
        }

        // 1. ExactRecorded: 只有录制的 protocol（1 条）
        {
            let bus = DataBus::new();
            let rx = bus.subscribe(TopicFilter::exact(topic));
            let mut replay = ReplayManager::new(bus);
            replay.load(&path).unwrap();
            replay.set_policy(ReplayPolicy::ExactRecorded);
            replay.seek_with_replay(1000);
            let count = rx.drain().len();
            assert_eq!(
                count, 1,
                "ExactRecorded: should get exactly 1 protocol event"
            );
        }

        // 2. ReparseRaw: 跳过录制的 protocol，只有 analyzer cache（1 条）
        {
            let bus = DataBus::new();
            let rx = bus.subscribe(TopicFilter::exact(topic));
            let mut replay = ReplayManager::new(bus);
            replay.load(&path).unwrap();
            replay.set_policy(ReplayPolicy::ReparseRaw);

            let mut derived = Event::json(
                topic,
                "replay-analyzer:demo",
                serde_json::json!({"t": 1, "value": 99.0}),
            );
            derived.meta_set("replay", serde_json::Value::Bool(true));
            derived.meta_set(
                "origin",
                serde_json::Value::String("replay_derived".to_owned()),
            );
            replay.set_analyzer_cache(vec![derived]);

            replay.seek_with_replay(1000);
            let count = rx.drain().len();
            assert_eq!(
                count, 1,
                "ReparseRaw: should get exactly 1 analyzer event, not 2"
            );
        }

        // 3. AutoPreferRecorded: 有 protocol → ExactRecorded → 1 条
        {
            let bus = DataBus::new();
            let rx = bus.subscribe(TopicFilter::exact(topic));
            let mut replay = ReplayManager::new(bus);
            replay.load(&path).unwrap();
            // Auto is default
            replay.seek_with_replay(1000);
            let count = rx.drain().len();
            assert_eq!(
                count, 1,
                "AutoPreferRecorded: should get exactly 1 protocol event"
            );
        }

        let _ = fs::remove_file(&path);
    }
}
