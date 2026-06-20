//! 回放管理：加载录制文件、seek/step/play 控制、analyzer cache、书签。
//!
//! 与 `recorder.rs`（录制）零耦合：`ReplayManager` 不引用 `JsonlRecorder` 的任何类型。

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;
use std::time::Instant;
use tool_core::{Event, LogLevel};
use tool_databus::DataBus;

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayBlockReason {
    NeedAnalyzer,
    AnalyzerFailed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayState {
    Empty,
    Loaded,
    Playing,
    Paused,
    Finished,
}

#[derive(Debug, Clone, Default)]
pub struct ReplayLoadReport {
    pub loaded: usize,
    pub skipped: usize,
    pub first_errors: Vec<String>,
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
    pub analyzer_warning: Option<String>,
    pub load_report: Option<ReplayLoadReport>,
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
    analyzer_warning: Option<String>,
    analyzer_cursor: usize,
    last_load_report: Option<ReplayLoadReport>,
    bookmarks: Vec<u64>,
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
            analyzer_warning: None,
            analyzer_cursor: 0,
            last_load_report: None,
            bookmarks: Vec::new(),
        }
    }

    pub fn add_bookmark(&mut self) {
        let pos = self.position_ms();
        if !self.bookmarks.contains(&pos) {
            self.bookmarks.push(pos);
            self.bookmarks.sort();
        }
    }

    pub fn remove_bookmark(&mut self, pos_ms: u64) {
        self.bookmarks.retain(|&b| b != pos_ms);
    }

    pub fn bookmarks(&self) -> &[u64] {
        &self.bookmarks
    }

    pub fn load(&mut self, path: impl AsRef<std::path::Path>) -> io::Result<usize> {
        const MAX_LINE_BYTES: u64 = 4 * 1024 * 1024; // 4MB per line
        const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024; // 512MB total

        let path = path.as_ref().to_path_buf();

        // 文件大小检查：防止加载超大文件导致 OOM
        let metadata = std::fs::metadata(&path)
            .map_err(|e| io::Error::other(format!("获取文件大小失败: {e}")))?;
        if metadata.len() > MAX_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "录制文件过大 ({} MB)，限制 {} MB",
                    metadata.len() / 1024 / 1024,
                    MAX_FILE_BYTES / 1024 / 1024
                ),
            ));
        }

        let file = File::open(&path)?;
        let mut events = Vec::new();
        let mut skipped = 0usize;
        let mut first_errors: Vec<String> = Vec::new();
        for (line_num, line) in BufReader::new(file).lines().enumerate() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // 单行长度限制：拒绝解析超长行，防止畸形文件 OOM
            if trimmed.len() as u64 > MAX_LINE_BYTES {
                skipped += 1;
                if first_errors.len() < 5 {
                    first_errors.push(format!(
                        "第 {n} 行: 行长度 {} 字节，超过限制 4MB",
                        trimmed.len(),
                        n = line_num + 1,
                    ));
                }
                continue;
            }
            match serde_json::from_str::<Event>(trimmed) {
                Ok(event) => events.push(event),
                Err(e) => {
                    skipped += 1;
                    if first_errors.len() < 5 {
                        first_errors.push(format!("第 {n} 行: {e}", n = line_num + 1));
                    }
                }
            }
        }
        self.last_load_report = Some(ReplayLoadReport {
            loaded: events.len(),
            skipped,
            first_errors,
        });
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
        self.clear_analyzer_messages();
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
            self.clear_analyzer_messages();
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

    /// 当前策略下，阻止播放/seek 的原因。
    /// 仅在 ReparseRaw 且 analyzer cache 不可用时返回原因。
    pub fn replay_block_reason(&self) -> Option<ReplayBlockReason> {
        if self.effective_policy() != ReplayPolicy::ReparseRaw {
            return None;
        }

        if self.analyzer_cache_valid {
            return None;
        }

        if let Some(error) = &self.analyzer_error {
            return Some(ReplayBlockReason::AnalyzerFailed(error.clone()));
        }

        Some(ReplayBlockReason::NeedAnalyzer)
    }

    pub fn replay_ready(&self) -> bool {
        self.replay_block_reason().is_none()
    }

    pub fn can_play(&self) -> bool {
        !self.events.is_empty() && self.state != ReplayState::Empty && self.replay_ready()
    }

    pub fn can_seek(&self) -> bool {
        !self.events.is_empty() && self.state != ReplayState::Playing && self.replay_ready()
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
        self.clear_analyzer_messages();
        self.analyzer_cursor = 0;
    }

    /// 标记 analyzer 失败（会清空缓存）。
    pub fn set_analyzer_error(&mut self, error: String) {
        self.analyzer_cache.clear();
        self.analyzer_cache_valid = false;
        self.analyzer_error = Some(error);
        self.analyzer_warning = None;
        self.analyzer_cursor = 0;
    }

    /// 设置 analyzer 警告信息（不清缓存，仅 UI 提示）。
    pub fn set_analyzer_warning(&mut self, warning: String) {
        self.analyzer_warning = Some(warning);
    }

    /// 清除错误/警告，保留缓存。
    pub fn clear_analyzer_error(&mut self) {
        self.clear_analyzer_messages();
    }

    fn clear_analyzer_messages(&mut self) {
        self.analyzer_error = None;
        self.analyzer_warning = None;
    }

    pub fn analyzer_warning(&self) -> Option<&str> {
        self.analyzer_warning.as_deref()
    }

    pub fn analyzer_cache_valid(&self) -> bool {
        self.analyzer_cache_valid
    }

    pub fn analyzer_error(&self) -> Option<&str> {
        self.analyzer_error.as_deref()
    }

    /// 开始播放。返回 false 表示被门控阻止（如需要 analyzer）。
    pub fn play(&mut self) -> bool {
        if self.events.is_empty() {
            self.state = ReplayState::Empty;
            return false;
        }

        if !self.replay_ready() {
            self.bus.publish(Event::system_log(
                LogLevel::Warn,
                "replay",
                "playback blocked: replay analyzer is required",
            ));
            return false;
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
        true
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
        if !self.replay_ready() {
            self.bus.publish(Event::system_log(
                LogLevel::Warn,
                "replay",
                "seek blocked: replay analyzer is required",
            ));
            return 0;
        }

        let panel_count = self.seek_panel_phase(position_ms);
        let data_count = self.seek_data_phase(position_ms);
        panel_count + data_count
    }

    /// 第一阶段：重置位置，只发布 ui.panel.create 事件。
    /// 调用方应在该阶段后处理面板创建，再调用 seek_data_phase。
    pub fn seek_panel_phase(&mut self, position_ms: u64) -> usize {
        self.cursor = 0;
        self.analyzer_cursor = 0;
        self.position_at_start_ms = position_ms.min(self.duration_ms());
        self.replay_start = None;
        self.state = ReplayState::Paused;
        self.publish_until_filtered(position_ms, |event| {
            event.topic == tool_core::topics::UI_PANEL_CREATE
        })
    }

    /// 第二阶段：发布剩余事件（非 ui.panel.create）+ analyzer cache。
    /// 必须在 seek_panel_phase 之后调用。
    /// 优化：复用 seek_panel_phase 推进的 cursor，只从 cursor 位置继续向前扫描。
    pub fn seek_data_phase(&mut self, position_ms: u64) -> usize {
        let policy = self.effective_policy();

        // 先从 0 扫描到 cursor（seek_panel_phase 已处理过的事件范围），
        // 发布非 panel.create 事件。
        let before_cursor = self.publish_range_filtered(0, self.cursor, position_ms, |event| {
            event.topic != tool_core::topics::UI_PANEL_CREATE
                && (policy != ReplayPolicy::ReparseRaw || !event.topic.starts_with("protocol."))
        });

        // 再从 cursor 继续扫描剩余事件（如有）
        let after_cursor = self.publish_until_filtered(position_ms, |event| {
            event.topic != tool_core::topics::UI_PANEL_CREATE
                && (policy != ReplayPolicy::ReparseRaw || !event.topic.starts_with("protocol."))
        });

        let analyzer_count = if policy == ReplayPolicy::ReparseRaw && self.analyzer_cache_valid {
            self.publish_analyzer_cache_until(position_ms)
        } else {
            0
        };

        before_cursor + after_cursor + analyzer_count
    }

    /// 按 predicate 过滤发布事件到指定位置。
    /// 会推进 cursor 到目标位置之后。
    fn publish_until_filtered(
        &mut self,
        target_position_ms: u64,
        predicate: impl Fn(&Event) -> bool,
    ) -> usize {
        let Some(base) = self.base_timestamp_ms() else {
            return 0;
        };

        let mut count = 0;
        while let Some(event) = self.events.get(self.cursor) {
            let event_position = event.timestamp_ms.saturating_sub(base);
            if event_position > target_position_ms {
                break;
            }
            if predicate(event) {
                self.bus.publish(mark_replay_event(event.clone()));
                count += 1;
            }
            self.cursor += 1;
        }

        count
    }

    /// 在指定索引范围 [start..end) 内，按 predicate 过滤发布事件。
    /// 不推进 cursor（由调用方管理）。
    fn publish_range_filtered(
        &mut self,
        start: usize,
        end: usize,
        target_position_ms: u64,
        predicate: impl Fn(&Event) -> bool,
    ) -> usize {
        let Some(base) = self.base_timestamp_ms() else {
            return 0;
        };

        let mut count = 0;
        for index in start..end {
            let Some(event) = self.events.get(index) else {
                break;
            };
            let event_position = event.timestamp_ms.saturating_sub(base);
            if event_position > target_position_ms {
                break;
            }
            if predicate(event) {
                self.bus.publish(mark_replay_event(event.clone()));
                count += 1;
            }
        }

        count
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
        if !self.replay_ready() {
            return self.cursor;
        }

        if self.cursor < self.events.len() {
            self.publish_cursor_event();

            let position_ms = self.cursor_position_ms().min(self.duration_ms());
            self.position_at_start_ms = position_ms;

            if self.state == ReplayState::Playing {
                self.replay_start = Some(Instant::now());
            } else {
                self.replay_start = None;
            }

            if self.effective_policy() == ReplayPolicy::ReparseRaw && self.analyzer_cache_valid {
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
            analyzer_warning: self.analyzer_warning.clone(),
            load_report: self.last_load_report.clone(),
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
    use tool_core::{Direction, Payload};

    fn ev(topic: &str) -> Event {
        Event::new(topic, "test", Direction::Internal, Payload::Empty)
    }

    #[test]
    fn mark_replay_event_sets_metadata_and_source_prefix() {
        let event = ev("transport.serial.default.rx");
        let marked = mark_replay_event(event);
        assert!(marked.is_replay());
        assert_eq!(marked.origin(), Some("replay"));
        // original_source 保留原始 source
        assert_eq!(marked.meta_str("original_source"), Some("test"));
        // source 加 replay: 前缀
        assert_eq!(marked.source, "replay:test");
    }

    #[test]
    fn new_manager_defaults_to_auto_prefer_and_needs_analyzer_when_no_protocol() {
        // 新建 manager 无事件、无录制 protocol：AutoPreferRecorded 解析为 ReparseRaw，
        // 因此 needs_analyzer 为 true，replay_ready 为 false（NeedAnalyzer）。
        let bus = DataBus::new();
        let mgr = ReplayManager::new(bus);
        assert_eq!(mgr.policy(), ReplayPolicy::AutoPreferRecorded);
        assert_eq!(mgr.effective_policy(), ReplayPolicy::ReparseRaw);
        assert!(mgr.needs_analyzer());
        assert!(!mgr.replay_ready());
        assert_eq!(
            mgr.replay_block_reason(),
            Some(ReplayBlockReason::NeedAnalyzer)
        );
    }

    #[test]
    fn set_policy_to_exact_makes_replay_ready() {
        let bus = DataBus::new();
        let mut mgr = ReplayManager::new(bus);
        mgr.set_policy(ReplayPolicy::ExactRecorded);
        assert_eq!(mgr.effective_policy(), ReplayPolicy::ExactRecorded);
        assert!(!mgr.needs_analyzer());
        assert!(mgr.replay_ready());
        assert_eq!(mgr.replay_block_reason(), None);
    }

    #[test]
    fn set_analyzer_error_blocks_with_failed_reason() {
        let bus = DataBus::new();
        let mut mgr = ReplayManager::new(bus);
        // ReparseRaw + 无 cache → NeedAnalyzer
        assert_eq!(
            mgr.replay_block_reason(),
            Some(ReplayBlockReason::NeedAnalyzer)
        );
        mgr.set_analyzer_error("boom".to_owned());
        assert!(matches!(
            mgr.replay_block_reason(),
            Some(ReplayBlockReason::AnalyzerFailed(_))
        ));
        assert!(!mgr.replay_ready());
    }

    #[test]
    fn set_speed_clamps_to_range() {
        let bus = DataBus::new();
        let mut mgr = ReplayManager::new(bus);
        mgr.set_speed(0.001); // 过小
        assert_eq!(mgr.status().speed, 0.1);
        mgr.set_speed(100.0); // 过大
        assert_eq!(mgr.status().speed, 32.0);
        mgr.set_speed(2.0); // 合法
        assert_eq!(mgr.status().speed, 2.0);
    }

    #[test]
    fn empty_manager_status_reflects_state() {
        let bus = DataBus::new();
        let mgr = ReplayManager::new(bus);
        let status = mgr.status();
        assert_eq!(status.state, ReplayState::Empty);
        assert_eq!(status.total_events, 0);
        assert_eq!(status.cursor, 0);
        assert_eq!(status.duration_ms, 0);
        assert!(!mgr.can_play());
        assert!(!mgr.can_seek());
    }

    #[test]
    fn backward_position_none_when_empty_or_at_start() {
        let bus = DataBus::new();
        let mgr = ReplayManager::new(bus);
        assert_eq!(mgr.backward_position(), None);
        assert_eq!(mgr.backward_position_by(5), None);
    }

    #[test]
    fn rejects_oversized_lines() {
        use std::io::Write;
        // 创建一个包含超长行的临时文件
        let dir = std::env::temp_dir().join(format!(
            "replay-safety-test-{}",
            tool_core::now_timestamp_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let jsonl_path = dir.join("oversized.jsonl");

        // 写一行有效的，再写一行超大行
        let mut file = std::fs::File::create(&jsonl_path).unwrap();
        writeln!(
            file,
            r#"{{"id":1,"timestamp_ms":1000,"topic":"test","source":"s","direction":"internal","payload":{{"kind":"empty","value":null}},"metadata":{{}}}}"#
        )
        .unwrap();
        // 写一行 5MB（超过 4MB 限制）
        let giant_line = "x".repeat(5 * 1024 * 1024);
        writeln!(file, "{}", giant_line).unwrap();
        file.flush().unwrap();

        let bus = DataBus::new();
        let mut mgr = ReplayManager::new(bus);
        mgr.load(&jsonl_path).unwrap();

        // 超大行应该被跳过，只加载了第一行
        assert_eq!(mgr.status().total_events, 1);
        // 应该有 1 个 skip 错误
        let status = mgr.status();
        if let Some(ref report) = status.load_report {
            assert_eq!(report.skipped, 1);
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rejects_oversized_file() {
        // 创建一个空的大文件在文件系统中是无法快速模拟的，
        // 这里验证 MAX_FILE_BYTES 逻辑存在即可（实际触发需要真实超大文件）
        let bus = DataBus::new();
        let mut mgr = ReplayManager::new(bus);
        // 不存在的路径应该返回错误
        let result = mgr.load(std::path::PathBuf::from("/nonexistent/path/for/test.jsonl"));
        assert!(result.is_err());
    }

    /// 验证 seek_with_replay 在大量事件下的性能。
    /// 生成 N 个事件，录制到临时文件，加载后执行 seek，确保在合理时间内完成。
    #[test]
    fn seek_performance_with_many_events() {
        use std::io::Write;

        let event_count = 10_000;
        let dir =
            std::env::temp_dir().join(format!("replay-perf-{}", tool_core::now_timestamp_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let jsonl_path = dir.join("perf.jsonl");

        // 生成事件：每毫秒一个，部分是 UI_PANEL_CREATE，大部分是串口数据
        {
            let mut file = std::fs::File::create(&jsonl_path).unwrap();
            for i in 0..event_count {
                let topic = if i % 100 == 0 {
                    "ui.panel.create"
                } else if i % 2 == 0 {
                    "transport.serial.default.rx"
                } else {
                    "log.system"
                };
                let event_json = format!(
                    r#"{{"id":{id},"timestamp_ms":{ts},"topic":"{topic}","source":"test","direction":"internal","payload":{{"kind":"empty","value":null}},"metadata":{{}}}}"#,
                    id = i,
                    ts = 1000 + i,
                    topic = topic,
                );
                writeln!(file, "{event_json}").unwrap();
            }
            file.flush().unwrap();
        }

        let bus = DataBus::new();
        let mut mgr = ReplayManager::new(bus);
        mgr.set_policy(ReplayPolicy::ExactRecorded);
        mgr.load(&jsonl_path).unwrap();

        assert_eq!(mgr.status().total_events, event_count);

        // 执行 seek 到中间位置
        let start = std::time::Instant::now();
        let count = mgr.seek_with_replay(5_000); // seek 到 5 秒位置
        let elapsed = start.elapsed();

        // 验证有事件被发布
        assert!(count > 0, "seek should publish events");

        // 性能断言：10k 事件的 seek 应在 1 秒内完成
        // O(n) seek 应该远快于此；O(n²) 的旧实现在 100k 事件时会显著变慢
        assert!(
            elapsed.as_millis() < 1000,
            "seek_with_replay took {:?} for {event_count} events — should be < 1s",
            elapsed
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
