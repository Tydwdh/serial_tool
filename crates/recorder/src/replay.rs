//! 回放管理：加载录制文件、seek/step/play 控制、analyzer cache、书签。
//!
//! 与 `recorder.rs`（录制）零耦合：`ReplayManager` 不引用 `JsonlRecorder` 的任何类型。

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;
use tool_core::{Event, LogLevel};
use tool_databus::DataBus;

use serde::{Deserialize, Serialize};

// ── 加载限制 ──
//
// 原生文件加载（`prepare_load*`）与浏览器文本加载（`ReplayTextLoader`）必须共用同一套
// 限额与文案，否则同一个录制文件在两端解析出的事件数/错误数会不一致。

/// 单个录制文件大小上限。
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024; // 512MB total

/// 单行长度上限，超过则跳过该行。
const MAX_LINE_BYTES: u64 = 4 * 1024 * 1024; // 4MB per line

/// 一次加载最多回传给 UI 的解析错误条数。
const MAX_REPORTED_ERRORS: usize = 5;

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

/// 在后台线程完成文件读取和 JSON 解析后的回放数据。
#[derive(Debug)]
pub struct ReplayLoadData {
    pub path: PathBuf,
    pub events: Vec<Event>,
    pub report: ReplayLoadReport,
    pub timestamp_index: Vec<(u64, usize)>,
    pub has_recorded_protocol: bool,
    pub bookmarks: Vec<(u64, Option<String>)>,
}

/// Cooperative browser-side JSONL parser.
///
/// The browser can read a File asynchronously, but JSON parsing itself still
/// runs on the UI thread.  This loader keeps the parser state between ticks so
/// the Application layer can consume a bounded number of lines per frame.
pub struct ReplayTextLoader {
    path: PathBuf,
    text: String,
    offset: usize,
    line_number: usize,
    events: Vec<Event>,
    skipped: usize,
    first_errors: Vec<String>,
}

impl ReplayTextLoader {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> io::Result<Self> {
        let text = text.into();
        if text.len() as u64 > MAX_FILE_BYTES {
            return Err(oversize_file_error(text.len() as u64));
        }
        Ok(Self {
            path: PathBuf::from(name.into()),
            text,
            offset: 0,
            line_number: 1,
            events: Vec::new(),
            skipped: 0,
            first_errors: Vec::new(),
        })
    }

    /// Consume at most `budget` lines. `Some` means parsing is complete.
    pub fn step(&mut self, budget: usize) -> io::Result<Option<ReplayLoadData>> {
        let mut consumed = 0;
        while self.offset < self.text.len() && consumed < budget {
            let remaining = &self.text[self.offset..];
            let (line_end, next_offset) = match remaining.find('\n') {
                Some(relative_end) => {
                    let end = self.offset + relative_end;
                    (end, end + 1)
                }
                None => (self.text.len(), self.text.len()),
            };
            parse_jsonl_line(
                &self.text[self.offset..line_end],
                self.line_number,
                &mut self.events,
                &mut self.skipped,
                &mut self.first_errors,
            );
            self.offset = next_offset;
            self.line_number += 1;
            consumed += 1;
        }

        if self.offset < self.text.len() {
            return Ok(None);
        }

        Ok(Some(finish_replay_load(
            self.path.clone(),
            std::mem::take(&mut self.events),
            self.skipped,
            std::mem::take(&mut self.first_errors),
        )))
    }
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
    /// 按时间戳的稀疏索引：每 INDEX_STRIDE 个事件记录一个 (timestamp_ms, index) 条目。
    /// 用于 seek_ms 时二分查找，将 O(n) 降为 O(log n)。
    timestamp_index: Vec<(u64, usize)>,
    path: Option<PathBuf>,
    cursor: usize,
    state: ReplayState,
    speed: f64,
    replay_start: Option<u64>,
    position_at_start_ms: u64,

    // ── 策略与 analyzer 缓存投递状态 ──
    policy: ReplayPolicy,
    has_recorded_protocol: bool,
    analyzer_cache: Vec<Event>,
    analyzer_cache_valid: bool,
    analyzer_error: Option<String>,
    analyzer_warning: Option<String>,
    analyzer_cursor: usize,
    last_load_report: Option<ReplayLoadReport>,
    bookmarks: Vec<Bookmark>,
}

#[derive(Debug, Clone)]
pub struct Bookmark {
    pub pos_ms: u64,
    pub name: Option<String>,
}

/// 稀疏索引入口间隔：每 INDEX_STRIDE 个事件记录一条索引。
/// 200 是平衡内存开销和查找精度的选择（50 万事件 ≈ 2500 条索引 ≈ 40KB）。
const INDEX_STRIDE: usize = 200;

impl ReplayManager {
    pub fn new(bus: DataBus) -> Self {
        Self {
            bus,
            events: Vec::new(),
            timestamp_index: Vec::new(),
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

    pub fn add_bookmark(&mut self, name: Option<String>) {
        let pos = self.position_ms();
        if !self.bookmarks.iter().any(|b| b.pos_ms == pos) {
            self.bookmarks.push(Bookmark { pos_ms: pos, name });
            self.bookmarks.sort_by_key(|b| b.pos_ms);
        }
    }

    pub fn remove_bookmark(&mut self, pos_ms: u64) {
        self.bookmarks.retain(|b| b.pos_ms != pos_ms);
    }

    pub fn bookmarks(&self) -> &[Bookmark] {
        &self.bookmarks
    }

    /// 只做文件读取和解析，不触碰 ReplayManager，可安全放到 worker 线程。
    ///
    /// **从这里出去的每一个 `io::Error` 都自带中文框架**：派发侧（`dispatch_load_replay`）
    /// 只做 `to_string()`，框架少补一处就是裸的操作系统原文冒出去；反过来在派发侧再包一层，
    /// 就成了两重中文框架的同义反复。所以三条出口（取大小、打开、逐行读）各自在生产处
    /// 补一句，新增出口时照同一条规则办。框架里点名**是哪个路径**失败的 —— 用户在手输
    /// 路径的那一格要改的就是它。
    ///
    /// 原生侧读这两个值的地方是 `load_replay` 的任务消息与 `AppEvent::TaskFailed` 那句
    /// `后台任务失败: …` 系统日志（本轮普查核出来的出口；状态栏不读它）。
    ///
    /// 包装必须用 `io::Error::new(e.kind(), ..)`，不能图省事用 `io::Error::other(..)`：
    /// `kind()` 在本工作区是承重的（第 9/11 轮的重试判定与 `tool_transport::translate_error`
    /// 的 `Io` 臂都读它），折成 `Other` 不会让 Display 有任何变化，只会让将来按 kind 分派的
    /// 分支静默认不出「是权限、是不存在、还是盘没了」。OS 细节原样留在冒号后，不改写、
    /// 不翻译（与 `recorder.rs` 的 `create_recording_file` 同一个形状、同一份理由）。
    pub fn prepare_load(path: impl AsRef<std::path::Path>) -> io::Result<ReplayLoadData> {
        let path = path.as_ref().to_path_buf();

        // 文件大小检查：防止加载超大文件导致 OOM
        let metadata = std::fs::metadata(&path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("获取文件大小 {} 失败：{e}", path.display()),
            )
        })?;
        if metadata.len() > MAX_FILE_BYTES {
            return Err(oversize_file_error(metadata.len()));
        }

        // 走到这里文件是存在的（上一步 `metadata` 已成功），所以这一条管的是
        // 「路径是目录」「被别的进程独占」「权限不够」这三类打开失败。
        let file = File::open(&path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("打开回放文件 {} 失败：{e}", path.display()),
            )
        })?;
        Self::prepare_load_lines(path, BufReader::new(file).lines())
    }

    /// Parse browser-provided JSONL text with the same limits and ordering as
    /// native file loading. The browser composition root supplies the text
    /// after its asynchronous file capability has completed.
    pub fn prepare_load_text(
        name: impl Into<String>,
        text: impl Into<String>,
    ) -> io::Result<ReplayLoadData> {
        let mut loader = ReplayTextLoader::new(name, text)?;
        // 这一条 `other` 是**对的**，别跟着第 43 轮那三处一起改：它底下没有 OS 错误，
        // 没有 kind 可保（`step(usize::MAX)` 读尽文本后必定返回 `Some`，这里只是把
        // 「不可能的分支」写成类型）。上面三条出口要保 kind，是因为那里 `e` 是操作系统给的。
        loader
            .step(usize::MAX)?
            .ok_or_else(|| io::Error::other("回放文本解析未完成"))
    }

    fn prepare_load_lines<I>(path: PathBuf, lines: I) -> io::Result<ReplayLoadData>
    where
        I: Iterator<Item = io::Result<String>>,
    {
        let mut events = Vec::new();
        let mut skipped = 0usize;
        let mut first_errors: Vec<String> = Vec::new();
        for (line_index, line) in lines.enumerate() {
            // 读行失败（网络盘中途掉线、文件读到一半被收回）也是原样冒出去的那一个
            // `?`，所以它同样在生产处补框架，与 `prepare_load` 的另两条出口同一个形状：
            // 点名路径、kind 照搬（这一条的 kind 是 OS 给的那一个，不是 `Other`）。
            let line = line.map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("读取回放文件 {} 失败：{e}", path.display()),
                )
            })?;
            // 行号 1-based 且包含空行：与浏览器侧 `ReplayTextLoader` 的错误定位一致。
            parse_jsonl_line(
                &line,
                line_index + 1,
                &mut events,
                &mut skipped,
                &mut first_errors,
            );
        }
        Ok(finish_replay_load(path, events, skipped, first_errors))
    }

    /// 在 Workbench 所在线程应用已经解析好的回放数据。
    pub fn load_prepared(&mut self, prepared: ReplayLoadData) -> usize {
        let ReplayLoadData {
            path,
            events,
            report,
            timestamp_index,
            has_recorded_protocol,
            bookmarks,
        } = prepared;
        self.last_load_report = Some(report);
        self.has_recorded_protocol = has_recorded_protocol;

        self.events = events;
        self.timestamp_index = timestamp_index;
        self.path = Some(path.clone());
        self.cursor = 0;
        self.position_at_start_ms = 0;
        self.replay_start = None;

        // load 时 invalidate analyzer cache（因为事件变了）
        self.invalidate_analyzer_cache();

        self.bookmarks = bookmarks
            .into_iter()
            .map(|(pos_ms, name)| Bookmark { pos_ms, name })
            .collect();

        self.state = self.resting_state();
        self.log_replay(
            LogLevel::Info,
            format!(
                "loaded {} event(s) from {} (recorded_protocol: {}, policy: {:?})",
                self.events.len(),
                path.display(),
                self.has_recorded_protocol,
                self.policy,
            ),
        );
        self.events.len()
    }

    // ── Policy ──

    pub fn set_policy(&mut self, policy: ReplayPolicy) {
        if self.policy != policy {
            self.policy = policy;
            // 切换 policy 时 invalidate cache
            self.invalidate_analyzer_cache();
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
        self.invalidate_analyzer_cache();
        self.analyzer_error = Some(error);
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

    /// analyzer 缓存整体失效：事件集或策略变化后，旧输出和已投递到的位置都不可再用，
    /// 因此缓存、有效标记、错误/警告、投递游标必须一起重置，不能只清其中一项。
    fn invalidate_analyzer_cache(&mut self) {
        self.analyzer_cache.clear();
        self.analyzer_cache_valid = false;
        self.clear_analyzer_messages();
        self.analyzer_cursor = 0;
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

    /// 回放状态变更日志：source 固定为 `"replay"`（UI 按 source 过滤），各处只给级别和文案。
    fn log_replay(&mut self, level: LogLevel, message: impl Into<String>) {
        let event = Event::system_log(level, "replay", message);
        self.bus.publish(event);
    }

    /// 开始播放。返回 false 表示被门控阻止（如需要 analyzer）。
    pub fn play(&mut self) -> bool {
        if self.events.is_empty() {
            self.state = ReplayState::Empty;
            return false;
        }

        if !self.replay_ready() {
            self.log_replay(
                LogLevel::Warn,
                "playback blocked: replay analyzer is required",
            );
            return false;
        }

        if self.cursor >= self.events.len() {
            self.seek_ms(0);
        }
        self.position_at_start_ms = self.position_ms();
        self.replay_start = Some(tool_core::monotonic_now_nanos());
        self.state = ReplayState::Playing;
        self.log_replay(LogLevel::Info, "playback started");
        true
    }

    pub fn pause(&mut self) {
        if self.state == ReplayState::Playing {
            self.position_at_start_ms = self.position_ms();
            self.replay_start = None;
            self.state = ReplayState::Paused;
            self.log_replay(LogLevel::Info, "playback paused");
        }
    }

    pub fn stop(&mut self) {
        self.cursor = 0;
        self.analyzer_cursor = 0;
        self.position_at_start_ms = 0;
        self.replay_start = None;
        self.state = self.resting_state();
        self.log_replay(LogLevel::Info, "playback stopped");
    }

    pub fn seek_ms(&mut self, position_ms: u64) {
        let base = self.base_timestamp_ms().unwrap_or_default();
        // 二分查找跳表，找到 >= target 的最小索引位置
        let start_idx = self
            .timestamp_index
            .binary_search_by_key(&(base.saturating_add(position_ms)), |(ts, _)| *ts)
            .map(|exact| self.timestamp_index[exact].1)
            .unwrap_or_else(|insert| {
                // insert 是 >= target 的第一个位置，它的前一个 < target
                if insert > 0 {
                    self.timestamp_index[insert - 1].1
                } else {
                    0
                }
            });
        // 从索引位置线性扫描剩余的 < INDEX_STRIDE 个事件
        self.cursor = self.events[start_idx..]
            .iter()
            .position(|event| event.timestamp_ms.saturating_sub(base) >= position_ms)
            .map(|offset| start_idx + offset)
            .unwrap_or(self.events.len());
        self.position_at_start_ms = position_ms.min(self.duration_ms());
        self.replay_start = if self.state == ReplayState::Playing {
            Some(tool_core::monotonic_now_nanos())
        } else {
            None
        };
    }

    /// 回退并重放到指定位置（用于拖动进度条）
    /// 返回重放的事件数，调用方应在调用前清空 UI 面板
    pub fn seek_with_replay(&mut self, position_ms: u64) -> usize {
        if !self.replay_ready() {
            self.log_replay(LogLevel::Warn, "seek blocked: replay analyzer is required");
            return 0;
        }

        let panel_count = self.seek_panel_phase(position_ms);
        let data_count = self.seek_data_phase(position_ms);
        panel_count + data_count
    }

    /// 回退/步进专用：按事件 cursor 精确重建，而不是按毫秒重建。
    pub fn seek_cursor_with_replay(&mut self, target_cursor: usize) -> usize {
        if !self.replay_ready() {
            self.log_replay(LogLevel::Warn, "seek blocked: replay analyzer is required");
            return 0;
        }

        let panel_count = self.seek_cursor_panel_phase(target_cursor);
        let data_count = self.seek_cursor_data_phase(target_cursor);
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
        // ReparseRaw 下录制的 protocol.* 不投递：它由 analyzer_cache 替代。
        let keep_recorded_protocol = policy != ReplayPolicy::ReparseRaw;
        let is_data_event = move |event: &Event| {
            event.topic != tool_core::topics::UI_PANEL_CREATE
                && (keep_recorded_protocol || !event.topic.starts_with("protocol."))
        };

        // 先从 0 扫描到 cursor（seek_panel_phase 已处理过的事件范围），
        // 发布非 panel.create 事件。
        let before_cursor = self.publish_range_filtered(0, self.cursor, position_ms, is_data_event);

        // 再从 cursor 继续扫描剩余事件（如有）
        let after_cursor = self.publish_until_filtered(position_ms, is_data_event);

        let analyzer_count = if policy == ReplayPolicy::ReparseRaw && self.analyzer_cache_valid {
            self.publish_analyzer_cache_until(position_ms)
        } else {
            0
        };

        before_cursor + after_cursor + analyzer_count
    }

    /// cursor 版本的第一阶段：只发布 target_cursor 之前的 ui.panel.create。
    pub fn seek_cursor_panel_phase(&mut self, target_cursor: usize) -> usize {
        let target_cursor = target_cursor.min(self.events.len());
        self.cursor = 0;
        self.analyzer_cursor = 0;
        self.position_at_start_ms = self.position_for_cursor(target_cursor);
        self.replay_start = None;
        self.state = if self.events.is_empty() {
            ReplayState::Empty
        } else {
            ReplayState::Paused
        };

        let count = self.publish_index_range_filtered(0, target_cursor, |event| {
            event.topic == tool_core::topics::UI_PANEL_CREATE
        });
        self.cursor = target_cursor;
        count
    }

    /// cursor 版本的第二阶段：发布 target_cursor 之前的非 ui.panel.create 事件。
    pub fn seek_cursor_data_phase(&mut self, target_cursor: usize) -> usize {
        let target_cursor = target_cursor.min(self.events.len());
        let end = self.cursor.min(target_cursor);
        let policy = self.effective_policy();

        let event_count = self.publish_index_range_filtered(0, end, |event| {
            event.topic != tool_core::topics::UI_PANEL_CREATE
                && (policy != ReplayPolicy::ReparseRaw || !event.topic.starts_with("protocol."))
        });

        let analyzer_count =
            if target_cursor > 0 && policy == ReplayPolicy::ReparseRaw && self.analyzer_cache_valid
            {
                self.publish_analyzer_cache_until(self.position_at_start_ms)
            } else {
                0
            };

        event_count + analyzer_count
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
    /// 纯按索引投递，不看时间戳；不推进 cursor，供 cursor-based seek 精确重建使用。
    fn publish_index_range_filtered(
        &mut self,
        start: usize,
        end: usize,
        predicate: impl Fn(&Event) -> bool,
    ) -> usize {
        let mut count = 0;
        for index in start..end.min(self.events.len()) {
            let Some(event) = self.events.get(index) else {
                break;
            };
            if predicate(event) {
                self.bus.publish(mark_replay_event(event.clone()));
                count += 1;
            }
        }
        count
    }

    /// 在指定索引范围 [start..end) 内，按 predicate 过滤发布事件，
    /// 并在事件位置超过 `target_position_ms` 时提前停止；不推进 cursor（由调用方管理）。
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

            let position_ms = self
                .position_for_cursor(self.cursor)
                .min(self.duration_ms());
            self.position_at_start_ms = position_ms;

            if self.state == ReplayState::Playing {
                self.replay_start = Some(tool_core::monotonic_now_nanos());
            } else {
                self.replay_start = None;
            }

            if self.effective_policy() == ReplayPolicy::ReparseRaw && self.analyzer_cache_valid {
                self.publish_analyzer_cache_until(position_ms);
            }
        }

        self.cursor
    }

    /// 逐事件后退：回到上一个事件位置
    pub fn step_backward(&mut self) {
        if let Some(target_cursor) = self.backward_cursor_by(1) {
            self.seek_cursor_with_replay(target_cursor);
        }
    }

    pub fn backward_position(&self) -> Option<u64> {
        self.backward_position_by(1)
    }

    pub fn backward_position_by(&self, steps: usize) -> Option<u64> {
        let target_cursor = self.backward_cursor_by(steps)?;
        if target_cursor == 0 {
            return None;
        }
        Some(self.position_for_cursor(target_cursor))
    }

    pub fn backward_cursor_by(&self, steps: usize) -> Option<usize> {
        if self.events.is_empty() || self.cursor == 0 {
            return None;
        }

        // cursor 表示“下一个要发布的事件索引”。
        // 回退 N 步，就是希望最终重放到 cursor - N 之前的位置。
        Some(self.cursor.saturating_sub(steps.max(1)))
    }

    /// `target_cursor` 之前最后一个事件（即已投递的最后一个）相对首帧的位置（ms）。
    fn position_for_cursor(&self, target_cursor: usize) -> u64 {
        if target_cursor == 0 {
            return 0;
        }

        let Some(base) = self.base_timestamp_ms() else {
            return 0;
        };
        self.events
            .get(target_cursor - 1)
            .map(|event| event.timestamp_ms.saturating_sub(base))
            .unwrap_or_else(|| self.duration_ms())
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.position_at_start_ms = self.position_ms();
        self.replay_start = if self.state == ReplayState::Playing {
            Some(tool_core::monotonic_now_nanos())
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
            self.log_replay(LogLevel::Info, "playback finished");
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
            let elapsed = tool_core::monotonic_now_nanos().saturating_sub(started) as f64
                / 1_000_000.0
                * self.speed;
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

    /// 静止状态：空文件/未加载是 Empty，有事件则是已停在起点的 Loaded。
    fn resting_state(&self) -> ReplayState {
        if self.events.is_empty() {
            ReplayState::Empty
        } else {
            ReplayState::Loaded
        }
    }
}

/// 超限错误：两条加载路径共用同一 `ErrorKind` 与文案。
fn oversize_file_error(size_bytes: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "录制文件过大 ({} MB)，限制 {} MB",
            size_bytes / 1024 / 1024,
            MAX_FILE_BYTES / 1024 / 1024
        ),
    )
}

/// 解析一行 JSONL：空行忽略；超长行与非法 JSON 计入 `skipped`，错误交给
/// `record_parse_error` 收集。`line_number` 由调用方给出（1-based，空行也占号），
/// 两端的错误定位必须完全一致。
fn parse_jsonl_line(
    line: &str,
    line_number: usize,
    events: &mut Vec<Event>,
    skipped: &mut usize,
    first_errors: &mut Vec<String>,
) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    // 单行长度限制：拒绝解析超长行，防止畸形文件 OOM
    if trimmed.len() as u64 > MAX_LINE_BYTES {
        *skipped += 1;
        record_parse_error(
            first_errors,
            format!(
                "第 {line_number} 行: 行长度 {} 字节，超过限制 4MB",
                trimmed.len()
            ),
        );
        return;
    }
    match serde_json::from_str::<Event>(trimmed) {
        Ok(event) => events.push(event),
        Err(error) => {
            *skipped += 1;
            record_parse_error(first_errors, format!("第 {line_number} 行: {error}"));
        }
    }
}

/// 只保留前 `MAX_REPORTED_ERRORS` 条错误：UI 只需少量线索，畸形长文件不能撑爆内存。
fn record_parse_error(first_errors: &mut Vec<String>, error: String) {
    if first_errors.len() < MAX_REPORTED_ERRORS {
        first_errors.push(error);
    }
}

/// 解析完成后的统一收尾：稳定排序、建稀疏时间戳索引、提取书签，组装 `ReplayLoadData`。
/// 书签以相对首帧的位置（ms）去重，同一位置只保留第一条的 name。
fn finish_replay_load(
    path: PathBuf,
    mut events: Vec<Event>,
    skipped: usize,
    first_errors: Vec<String>,
) -> ReplayLoadData {
    let loaded = events.len();
    events.sort_by_key(|event| (event.timestamp_ms, event.id));
    let timestamp_index = build_timestamp_index(&events);
    let has_recorded_protocol = events
        .iter()
        .any(|event| event.topic.starts_with("protocol.") && !event.is_replay());
    let first_timestamp = events.first().map(|event| event.timestamp_ms).unwrap_or(0);
    let mut bookmarks = Vec::new();
    for event in &events {
        if event.topic != "recorder.bookmark" {
            continue;
        }
        let name = event.payload.text_lossy();
        let name = (!name.is_empty()).then_some(name);
        let position_ms = event.timestamp_ms.saturating_sub(first_timestamp);
        if !bookmarks
            .iter()
            .any(|(position, _)| *position == position_ms)
        {
            bookmarks.push((position_ms, name));
        }
    }
    ReplayLoadData {
        path,
        events,
        report: ReplayLoadReport {
            loaded,
            skipped,
            first_errors,
        },
        timestamp_index,
        has_recorded_protocol,
        bookmarks,
    }
}

/// 构建稀疏时间戳索引：每 INDEX_STRIDE 个事件记录一条 (timestamp_ms, index)。
/// 索引保证按时间戳单调递增（events 已排序），可用于二分查找。
fn build_timestamp_index(events: &[Event]) -> Vec<(u64, usize)> {
    events
        .iter()
        .enumerate()
        .step_by(INDEX_STRIDE)
        .map(|(i, e)| (e.timestamp_ms, i))
        .collect()
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
    use tool_databus::TopicFilter;

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
    fn prepare_load_text_uses_the_same_parser_as_native_loading() {
        let valid = serde_json::to_string(&ev("transport.serial.default.rx")).unwrap();
        let text = format!("{valid}\nnot-json\n");
        let prepared = ReplayManager::prepare_load_text("browser-session.jsonl", text).unwrap();

        assert_eq!(prepared.path, PathBuf::from("browser-session.jsonl"));
        assert_eq!(prepared.events.len(), 1);
        assert_eq!(prepared.report.loaded, 1);
        assert_eq!(prepared.report.skipped, 1);
        assert_eq!(prepared.report.first_errors.len(), 1);
    }

    /// 原生加载的三条 IO 出口各有一句中文框架（规则记在 `prepare_load` 的文档注释里）。
    ///
    /// 派发侧（`dispatch_load_replay`）只做 `to_string()`，所以这里少一句，用户就在
    /// `load_replay` 的任务消息与那句系统日志里读到裸的操作系统原文。三条出口在本用例里
    /// 一次驱动到位：任何一条退回裸 `?`，对应那一格立刻红。
    ///
    /// 冒号一律**全角** `：`（与 `recorder.rs` 的第 40 轮框架一致）。这一条不再单设
    /// `contains` 断言：三条期望里的字面量就带全角冒号，逐字相等已经把标点钉住了 ——
    /// 实测只把生产侧退回半角，①② 两格即红（`left: …失败: … / right: …失败：…`），
    /// 再加一条 `contains(" 失败：")` 只是同一事实的弱重复。
    #[test]
    fn native_load_exits_all_mint_a_chinese_frame() {
        let dir = std::env::temp_dir().join(format!(
            "replay-wording-test-{}",
            tool_core::now_timestamp_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // ① 取大小：路径不存在。OS 细节随机器与 locale 变，钉不住，钉的是
        // 「框架 + 点名的路径 + 细节逐字原样」这一整个形状。
        let missing = dir.join("nope.jsonl");
        let raw = std::fs::metadata(&missing).expect_err("测试前提：不存在的路径 stat 不出来");
        let error = ReplayManager::prepare_load(&missing).expect_err("不存在的路径必须失败");
        assert_eq!(
            error.to_string(),
            format!("获取文件大小 {} 失败：{raw}", missing.display()),
            "① 必须是「一句中文框架 + 点名的路径 + 原样带出的 OS 细节」，多一句少一句都算变"
        );

        // ② 打开：这个路径 stat 得到，却不能当文件打开。
        //    夹具形状与为什么不能用目录（Linux 上目录是**可读打开**的）见
        //    `tool_testing::UnopenablePath`。
        let unopenable = tool_testing::UnopenablePath::new();
        let target = unopenable.path();
        let raw = File::open(target).expect_err("测试前提：这个路径不能当文件打开");
        let error = ReplayManager::prepare_load(target).expect_err("这个路径必须打不开");
        assert_eq!(
            error.to_string(),
            format!("打开回放文件 {} 失败：{raw}", target.display()),
            "② 必须是「一句中文框架 + 点名的路径 + 原样带出的 OS 细节」，多一句少一句都算变"
        );

        // ③ 逐行读：读盘中途失败（网络盘掉线一类）驱动不出来，只能注入失败的迭代器。
        let lines: [io::Result<String>; 1] = [Err(io::Error::other("网络磁盘已断开"))];
        let error = ReplayManager::prepare_load_lines(dir.clone(), lines.into_iter())
            .expect_err("读行失败必须冒出来");
        assert_eq!(
            error.to_string(),
            format!("读取回放文件 {} 失败：网络磁盘已断开", dir.display()),
            "③ 的框架或载荷被改动了"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// 三条出口的框架**不得改掉 `kind()`**（第 43 轮）。
    ///
    /// 为什么单独钉 kind 而不并入上面的文案用例：`io::Error::other(文案)` 与
    /// `io::Error::new(kind, 文案)` 的 `Display` **逐字相同**，所以框架全对时文案用例照样全绿，
    /// 只有 kind 能看出「权限失败」被说成了「别的失败」。而 kind 在本工作区是承重的
    /// （第 9/11 轮的重试判定、`tool_transport::translate_error` 的 `Io` 臂都按它分派）。
    ///
    /// 手法照 `recorder.rs` 的 `create_recording_file_frames_both_io_exits_in_chinese`：
    /// **同一个用例里先跑一次不加框架的裸操作**，取本机的 kind 再比对 —— 不依赖权限
    /// （Windows 上 chmod 造不出来）、不分平台 `#[cfg]`、不硬编 OS 原话。
    #[test]
    fn native_load_exits_keep_the_kind_of_the_underlying_os_error() {
        let dir = std::env::temp_dir().join(format!(
            "replay-kind-test-{}",
            tool_core::now_timestamp_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // ① 取大小：不存在的路径。这一格的 kind 与平台无关（Windows `ERROR_FILE_NOT_FOUND` /
        //    POSIX `ENOENT` 都映成 `NotFound`），所以除了"等于裸操作"再钉死一次。
        let missing = dir.join("nope.jsonl");
        let raw = std::fs::metadata(&missing).expect_err("测试前提：不存在的路径 stat 不出来");
        let error = ReplayManager::prepare_load(&missing).expect_err("① 的失败必须冒出来");
        assert_eq!(error.kind(), raw.kind(), "① 的框架把 kind 折掉了");
        assert_eq!(
            error.kind(),
            io::ErrorKind::NotFound,
            "① 的失败应还是 NotFound，got {:?}",
            error.kind()
        );

        // ② 打开（本轮要证明的那一格）：这个路径 stat 得到却打不开。这一格的 kind **随平台不同**
        //    （Windows `ERROR_ACCESS_DENIED` ⇒ `PermissionDenied`；Unix 是 `ENXIO` ⇒ 无对应变体），
        //    所以钉的是"等于裸操作给的 kind"这一条 —— 它同时也是"没退回 `io::Error::other(..)`"
        //    的判据：那种写法无论 OS 说什么都折成 `Other`，两边立刻不等。
        //    夹具形状与为什么不能用目录（Linux 上目录是**可读打开**的）见 `tool_testing::UnopenablePath`。
        let unopenable = tool_testing::UnopenablePath::new();
        let target = unopenable.path();
        let raw = File::open(target).expect_err("测试前提：这个路径不能当文件打开");
        let error = ReplayManager::prepare_load(target).expect_err("② 的失败必须冒出来");
        assert_eq!(
            error.to_string(),
            format!("打开回放文件 {} 失败：{raw}", target.display()),
            "② 框架的形状变了（路径没点名 / OS 细节被改写）"
        );
        assert_eq!(error.kind(), raw.kind(), "② 的框架把 kind 折掉了");

        // ③ 逐行读：真掉一次网络盘驱动不出来，所以注入 —— 钉的是"迭代器递来什么 kind，
        // 出口就还什么 kind"。两格而不是一格：只调通一格的写法在这里看不出来。
        for kind in [io::ErrorKind::TimedOut, io::ErrorKind::UnexpectedEof] {
            let lines = [Err(io::Error::new(kind, "网络磁盘已断开"))];
            let error = ReplayManager::prepare_load_lines(dir.clone(), lines.into_iter())
                .expect_err("③ 的失败必须冒出来");
            assert_eq!(
                error.kind(),
                kind,
                "③ 的框架把读行递过来的 {kind:?} 折成了 {:?}",
                error.kind()
            );
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn replay_text_loader_can_finish_incrementally() {
        let first = serde_json::to_string(&ev("transport.serial.default.rx")).unwrap();
        let second = serde_json::to_string(&ev("transport.serial.default.tx")).unwrap();
        let mut loader =
            ReplayTextLoader::new("incremental.jsonl", format!("{first}\n{second}\n")).unwrap();
        assert!(loader.step(1).unwrap().is_none());
        let prepared = loader.step(1).unwrap().unwrap();
        assert_eq!(prepared.events.len(), 2);
        assert_eq!(prepared.report.loaded, 2);
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
    fn step_backward_rebuilds_by_cursor_when_timestamps_match() {
        let bus = DataBus::new();
        let sub = bus.subscribe_lossless(TopicFilter::All);
        let mut mgr = ReplayManager::new(bus);
        mgr.set_policy(ReplayPolicy::ExactRecorded);

        let mut first = Event::with_timestamp(
            1000,
            tool_core::topics::SERIAL_RX,
            "serial:COM1",
            Direction::Rx,
            Payload::Text("first".to_owned()),
        );
        first.id = 1;
        let mut second = Event::with_timestamp(
            1000,
            tool_core::topics::SERIAL_RX,
            "serial:COM1",
            Direction::Rx,
            Payload::Text("second".to_owned()),
        );
        second.id = 2;
        let mut third = Event::with_timestamp(
            1010,
            tool_core::topics::SERIAL_RX,
            "serial:COM1",
            Direction::Rx,
            Payload::Text("third".to_owned()),
        );
        third.id = 3;

        mgr.events = vec![first, second, third];
        mgr.cursor = 2;
        mgr.state = ReplayState::Paused;

        mgr.step_backward();

        assert_eq!(mgr.status().cursor, 1);
        let replayed: Vec<_> = sub
            .drain()
            .into_iter()
            .filter(|event| event.is_replay() && event.topic == tool_core::topics::SERIAL_RX)
            .collect();
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].payload.text_lossy(), "first");
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
        mgr.load_prepared(ReplayManager::prepare_load(&jsonl_path).unwrap());

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
        // 不存在的路径应该返回错误（`prepare_load` 就是生产加载路径的第一步）
        let result = ReplayManager::prepare_load(std::path::PathBuf::from(
            "/nonexistent/path/for/test.jsonl",
        ))
        .map(|prepared| mgr.load_prepared(prepared));
        assert!(result.is_err());
    }

    /// 验证 `seek_with_replay` 的耗时**随「这次 seek 重放了多少事件」线性增长**。
    ///
    /// ## 为什么不再是「1 万条 seek < 1 秒」这种定长墙钟（两条理由，都有实测数字）
    /// 1. **假红**：绝对毫秒数取决于这台机器此刻有多忙。第 39 轮按 `target/r36-load.py` 那套 hog
    ///    实测它 churn×4 跑整包改前 **6/10** 红、churn×6 **2/8** 红，红点原文
    ///    `seek_with_replay took 1.14s for 10000 events — should be < 1s`。第 44 轮把旧尺子原样
    ///    留着、与新尺子在**同一批 hog 里按轮交替**重测（`target/r39-ab.py`）：churn×4 **1/10** 红、
    ///    churn×6 **2/8** 红、cpu×2 0/10 红，而新尺子同场 **0/10、0/8、0/10**。一次**通过**的运行
    ///    就能吃掉那条线的 85%（本轮空载实测旧尺子那一次 seek = 22~26ms，但整包并行时能冲到 1.1s）：
    ///    它断的是机器快慢，不是它想守的性质。
    /// 2. **没牙**：旧注释自己写着「O(n²) 的旧实现在 100k 事件时会显著变慢」，也就是说它想抓的是
    ///    「别把 seek 退回平方级」。第 44 轮把两个语义不变的人为平方级变异装到生产代码上量它：
    ///    M1（每次发布前从 0 重扫前缀）下旧断言实测 **157~169ms**、M1b（前缀纯累加，常数最小的
    ///    平方级）下 **93~96ms** —— 都远在 1000ms 之下，**旧尺子全绿**（它连 fixture 时间戳全塌成
    ///    同一毫秒都能绿：那次实测 count=10000、耗时 38~40ms，仍然「通过」）。
    ///    一条永远抓不到它声称要抓的东西的断言，比没有断言更糟。
    ///
    /// ## 换成什么
    /// 同一次运行里取三档「被 seek 覆盖的事件数」N、2N、4N，断耗时随 N 增长的**指数**有界：
    /// 覆盖数翻倍时线性 ⟹ 2×（指数≈1）、平方 ⟹ 4×（指数≈2）。比值对「机器多慢」不敏感、
    /// 对「算法形状」极敏感。负载只能把绝对时间**抬高**、抬不低，所以每档取 K 次采样的
    /// **最小值**，不用平均值。
    ///
    /// ## 前提核实（读生产代码 + 实测两边都对上）
    /// `seek_with_replay` = `seek_panel_phase` + `seek_data_phase`：
    /// · `seek_panel_phase` 先把 `cursor = 0`，再 `publish_until_filtered(position_ms, 是 panel.create)`
    ///   —— 从 0 扫到第一条位置 > target 的事件，**cursor 停在那里**；
    /// · `seek_data_phase` 先 `publish_range_filtered(0, cursor, …)` 把同一段前缀**再扫一遍**发布其余事件，
    ///   然后 `publish_until_filtered(…)`：此时 `events[cursor]` 的位置已 > target，它第一拍就 break。
    /// 所以一次 seek 的工作量 = Θ(覆盖事件数)，两阶段各发布一次 ⟹ **返回值恰等于覆盖事件数**
    /// （本用例 24 个采样每次都实测 `count == covered`；最大档 40k 的那一版同样每档全等），而且每档都从
    /// `cursor = 0` 重新起（档与档之间不残留进度，第三档不会因为前两档跑过而变快）。
    /// 每个事件 `DataBus::publish` 是 O(1)（history 是 `DEFAULT_HISTORY_LIMIT`=20000 封顶的
    /// `VecDeque`，满了就 push+pop；本用例不注册订阅者）。fixture 每毫秒一条事件 ⟹ 位置 p 覆盖
    /// p+1 条，所以「覆盖 covered 条」的位置就是 `covered - 1`。
    ///
    /// ## 这些数字是怎么量出来的（16 核本机、debug 构建；脚本都在 `target/` 下，不入库）
    /// · **空载**（`target/r44-probe.py --load 0 --full` 与 `cargo test -p tool-recorder`）三档 min =
    ///   **19.5~25.2ms / 38.9~49.5ms / 78.1~103.2ms**（相邻比值 1.99~2.10，即指数 ≤1.09 —— 线性成立）；
    ///   本用例单独 **1.27~1.68s**；recorder 整包测试二进制 **0.10s → 1.34~3.04s**（上限那几次是
    ///   同机另有 5 个 `rustc` 在编译整个工作区）、
    ///   `cargo test -p tool-recorder` 墙钟 **1.07s → 2.18~2.96s**；
    ///   那次并发编译下三档 min 抬到 **35.7 / 83.3 / 169.5ms**（≈1.8 倍天气），两个估计量读数
    ///   仍是 **1.22 / 0.58** —— 绝对时间被抬走、形状不动，这就是不用定长墙钟的理由。
    ///   工作区 `cargo test --workspace --all-targets` **热构建后 7.71s → 8.5~8.7s**（+0.8~1.0s，
    ///   两侧都是 32 条 `test result:` 行、**731 passed / 0 failed / 7 ignored**，用例数一条没变）。
    /// · **为什么三档、为什么最大档是 20k、为什么 K=8**：三档才谈得上「形状」（两档只能谈一个比值，
    ///   一个撞车的样本就能把它抬过界）。最大档试过 40k（K=5）：本用例单独 **1.81~1.83s**、
    ///   churn×4 整包最坏 **117s** 一轮，并且出现过一次「小档 min 被饿到 1.93s、比中档的 0.37s
    ///   还慢」的单调性反转 —— 那意味着整条断言在负载下不再可靠。降到 20k + K=8 之后，
    ///   整包 **44 轮负载**（churn×4 18 + churn×6 16 + cpu×2 10）+ 6 轮空载整包里单调性反转
    ///   **0 次**，而最弱的平方级变异（M1b）在 20k 档
    ///   仍读到 exp **1.77~1.89** —— 牙齿没变弱，墙钟减半。K 取 8 也是同一批测量定的：
    ///   每档取最小值，K 越大越能等到一个真正没被撞车的窗口，8 已经让两个估计量在负载下都留有余量。
    /// · **负载**（`target/r39-ab.py`，同一批 hog 下新旧二进制**按轮交替**跑整包 libtest）：
    ///   | 配方 | 轮数 | 旧尺子红 | 新尺子红 | 新尺子 exp(min比值) 最大 | exp(每轮最小) 最大 |
    ///   | --- | --- | --- | --- | --- | --- |
    ///   | churn×4（64 个搅缓存 hog） | 10 | **1/10** | **0/10** | 1.47 | 1.01 |
    ///   | churn×6（96 个） | 8 | **2/8** | **0/8** | 1.22 | 0.93 |
    ///   | cpu×2（32 个纯忙等） | 10 | 0/10 | 0/10 | 1.25 | 0.95 |
    ///   换成终版字节（只动了注释）后又各复跑一轮：churn×4 8 轮、churn×6 8 轮，两侧仍 **0 红**，
    ///   新尺子两个估计量的最大读数 1.40 与 0.93 —— 上面引的两条上界对全部 44 轮负载成立。
    ///   合计：**同一批交替跑的 44 轮里旧尺子红 3 轮（churn×4 1/10、churn×6 2/8、cpu×2 0/10、
    ///   终版复跑 0/16），新尺子 0/44 红**。第 39 轮同配方量到旧尺子 churn×4 **6/10**、churn×6
    ///   **2/8** 红 —— 同一条配方在不同时刻数出来的红数会漂，所以本轮两侧在同一批 hog 里交替跑：
    ///   旧尺子红不红取决于天气，而天气不是它想守的那个性质。
    /// · **变异体**（`target/r44-mutate.py` + `target/r44-verdict.py`，同一份三档 K=8 采样、空载，
    ///   每个变异都核过「其余 35 条用例全绿 + 返回值与发布内容逐条不变」）：
    ///   M1「每次发布前从 0 重扫前缀」= exp(min比值) **1.90~2.23**、exp(每轮最小) **1.56~1.91** → **红**；
    ///   M1b「前缀纯累加」= **1.99~2.08** / **1.77~1.89** → **红**；
    ///   M3「数据阶段多扫三遍前缀并 clone」（仍然线性、实测把三档 min 抬高 5~8%）= **≤1.08** → 绿；
    ///   M4「每次 seek 重建跳表索引」（与各档覆盖数无关的常数）= **≤1.08** → 绿。
    ///   再往上更重的平方级不必再测：两个估计量都随平方项系数**单调上升**（t(n)=a·n+q·n²/2 时
    ///   t(2n)/t(n) 从 2 单调趋向 4），M1 已读到 1.90~2.23，更重的只会更高。
    ///   **没被抓到的是 M3 与 M4 这类「常数」回归**（旧尺子同样抓不到：两者实测 21~26ms，
    ///   离 1000ms 差 40 倍）—— 这把尺子只判形状，这是它换取「不因机器慢而假红」的代价，写在下面。
    ///
    /// ## 两条上界怎么定的，以及它们的分工
    /// 门界必须**高于**线性实现的最坏观测（含负载）、又**低于**平方实现的最优观测：
    /// · `MIN_RATIO_EXPONENT_CEIL = 1.75`：空载最坏 **1.09**（探针版 1.12）、三种负载配方最坏 **1.47**
    ///   → 上方余量 0.28；下方到 M1b 的 1.99 还有 0.24。这条负责**牙齿**。
    /// · `EVERY_ROUND_EXPONENT_CEIL = 1.50`：每轮比值取最小，负载抬不动它（44 轮负载最坏 1.01，
    ///   余量 0.49），代价是撞车的轮会把它压低（空载见过 0.62、负载见过 −3.14，即比值 0.29×）；
    ///   平方变异在它上面仍读到 1.56~1.91。这条负责**绝不因负载假红**。
    /// 两条各自独立断言：任何一条红都说明「这个形状不是线性」，而 churn×4/×6 实测 0 红。
    /// **代价**：只增大常数、不改形状的回归（M3/M4）看不见 —— 要守那种回归就得钉绝对预算，
    /// 而绝对预算正是本轮删掉的那条 `< 1000ms`：它既在负载下假红、又在平方级下没牙。
    #[test]
    fn seek_performance_with_many_events() {
        use std::io::Write;
        use std::time::{Duration, Instant};

        // 三档「被这次 seek 覆盖的事件数」：N、2N、4N（相邻两档覆盖数正好翻倍 ⟹ 指数 = log2 比值）。
        const TIER_UNIT: usize = 5_000;
        let covered_tiers = [TIER_UNIT, 2 * TIER_UNIT, 4 * TIER_UNIT];
        // 每档 K 次采样的最小值。K=8 是量出来的：K=5 时（40k 最大档那一版）churn×4 整包跑出现过
        // 一次「小档 min 被饿得比中档还慢」的单调性反转；K=8 + 20k 在 44 轮负载 + 6 轮空载整包里 0 次。
        const SAMPLES: usize = 8;
        // 估计量 A：各档 min 之比 → 指数。上界 1.75 的依据见本用例文档注释（线性最坏读数 1.47、
        // 平方变异最低读数 1.90）。
        const MIN_RATIO_EXPONENT_CEIL: f64 = 1.75;
        // 估计量 B：每一轮各算一次比值，取 K 轮里**最小**的那个 → 指数。它在负载下抬不起来
        // （44 轮负载最坏读数 1.01），但一轮撞车就能把它压低，所以只给到 1.50。
        const EVERY_ROUND_EXPONENT_CEIL: f64 = 1.50;

        let event_count = *covered_tiers.last().unwrap();
        let dir =
            std::env::temp_dir().join(format!("replay-perf-{}", tool_core::now_timestamp_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let jsonl_path = dir.join("perf.jsonl");

        // 生成事件：每毫秒一个，部分是 UI_PANEL_CREATE，大部分是串口数据
        // —— 只生成**一份**最大档的文件、只 load 一次，档位靠 seek 到递增位置区分：变的是
        // 「这次 seek 要重放多少事件」（正是要守的量），20k 行 JSONL 的生成与解析开销一次付清。
        {
            // BufWriter：每条事件一次 `writeln!` 到裸 File 是 2 万次写系统调用，它把建 fixture
            // 的开销堆进整包墙钟（不进尺子，但拖慢整个套件）。聚成大块一次刷出去。
            let mut file = std::io::BufWriter::new(std::fs::File::create(&jsonl_path).unwrap());
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
        mgr.load_prepared(ReplayManager::prepare_load(&jsonl_path).unwrap());

        assert_eq!(mgr.status().total_events, event_count);

        // 预热一次满量程：把 bus 的 history 环推到 `DEFAULT_HISTORY_LIMIT` 封顶、把代码页与
        // 分支预测器热起来。不预热的话第一轮要额外付「环从 0 长到 20k」的开销，那不是 seek 的形状。
        let warmup = mgr.seek_with_replay(event_count as u64 - 1);
        assert_eq!(
            warmup, event_count,
            "预热这次 seek 就该重放满量程 {event_count} 条事件"
        );

        // 采样按「轮」交错：每一轮把三档各跑一次，再进下一轮。这样某一轮撞上负载时三档一起撞，
        // 而不是「小档在风平浪静时跑完、大档整段撞上 hog」—— 那会把比值凭空抬高。
        let mut samples: Vec<Vec<Duration>> =
            vec![Vec::with_capacity(SAMPLES); covered_tiers.len()];
        let mut published = Vec::with_capacity(SAMPLES * covered_tiers.len());
        for _round in 0..SAMPLES {
            for (tier, covered) in covered_tiers.iter().enumerate() {
                // fixture 每毫秒一条事件 ⟹ 覆盖 `covered` 条事件的位置是 `covered - 1`。
                let position_ms = (*covered - 1) as u64;
                let started = Instant::now();
                let count = mgr.seek_with_replay(position_ms);
                samples[tier].push(started.elapsed());
                published.push((tier, *covered, count));
            }
        }

        // ── 防空转（必须先于任何比值断言）：每一档都要真的重放了它声称覆盖的那些事件 ──
        // 少了这条，fixture 没喂进数据（解析整批跳过、位置算错、policy 不匹配直接 return 0）时
        // 三档会一起退化成「耗时全是最小的同一个数、比值恒为 1」，缩放比就成了永远为真的空转。
        for (tier, covered, count) in published {
            assert_eq!(
                count, covered,
                "第 {tier} 档（覆盖 {covered} 条事件）实际重放了 {count} 条 —— 期望恰等于覆盖数：\
                 panel 阶段与 data 阶段各发布一次，不多不少。不相等说明尺子量的不是「重放多少事件」",
            );
        }
        let mins: Vec<Duration> = samples
            .iter()
            .map(|tier_samples| *tier_samples.iter().min().expect("每档都有 SAMPLES 个样本"))
            .collect();
        for (tier, min) in mins.iter().enumerate() {
            assert!(
                !min.is_zero(),
                "第 {tier} 档（覆盖 {} 条事件）的 {} 次采样最小值为 0 —— 尺子没量到东西\
                 （Instant 分辨率不够？），缩放比在这个尺寸下无意义",
                covered_tiers[tier],
                SAMPLES,
            );
        }
        for tier in 1..mins.len() {
            assert!(
                mins[tier] >= mins[tier - 1],
                "耗时不随覆盖事件数单调不减：第 {tier} 档（覆盖 {} 条）min {:?} < 第 {} 档\
                 （覆盖 {} 条）min {:?} —— 这是尺子坏了（某一档整段被饿住），不是实现变快了",
                covered_tiers[tier],
                mins[tier],
                tier - 1,
                covered_tiers[tier - 1],
                mins[tier - 1],
            );
        }

        // ── 缩放比：对每对相邻档算两个指数估计量，取这一对里更「像平方」的那个 ──
        let mut report = Vec::new();
        let mut max_mins_exponent = 0f64;
        let mut max_every_round_exponent = 0f64;
        for tier in 1..mins.len() {
            let mins_exponent = (mins[tier].as_secs_f64() / mins[tier - 1].as_secs_f64()).log2();
            // 估计量 B：每一轮各算一次「这一档 / 前一档」的指数，取 K 轮里**最小**的那个。
            // 取最小是因为负载只会把某一档抬慢、不会压低：一轮撞车只会让这个估计量偏「像线性」，
            // 反过来它要超过上界，就意味着**每一轮**都看见了超线性增长。
            let every_round_exponent = samples[tier]
                .iter()
                .zip(&samples[tier - 1])
                .map(|(cur, prev)| (cur.as_secs_f64() / prev.as_secs_f64()).log2())
                .fold(f64::INFINITY, f64::min);
            max_mins_exponent = max_mins_exponent.max(mins_exponent);
            max_every_round_exponent = max_every_round_exponent.max(every_round_exponent);
            report.push(format!(
                "档{}→{}: exp(min比值)={:.2} exp(每轮最小)={:.2}",
                tier - 1,
                tier,
                mins_exponent,
                every_round_exponent,
            ));
        }

        println!(
            "[r44] covered={covered_tiers:?} K={SAMPLES} min={:?} 估计量 max: exp(min比值)={max_mins_exponent:.2}\
             (<{MIN_RATIO_EXPONENT_CEIL}) exp(每轮最小)={max_every_round_exponent:.2}\
             (<{EVERY_ROUND_EXPONENT_CEIL}) | {}",
            mins.iter().map(|d| d.as_micros()).collect::<Vec<_>>(),
            report.join(" | "),
        );

        assert!(
            max_mins_exponent < MIN_RATIO_EXPONENT_CEIL,
            "seek_with_replay 的耗时随覆盖事件数呈 **指数 {max_mins_exponent:.2}** 增长\
             （上界 {MIN_RATIO_EXPONENT_CEIL}）：三档 {covered_tiers:?} 的 min 耗时 {mins:?}。\
             线性应≈1.00、平方≈2.00 —— 这个形状是「每次 seek 都从头逐事件重扫」那一类回归的形状，\
             不是机器慢（机器慢抬绝对值、三档同摊）。",
        );
        assert!(
            max_every_round_exponent < EVERY_ROUND_EXPONENT_CEIL,
            "即使取每一轮比值里最**有利**的那个，指数仍达 {max_every_round_exponent:.2}\
             （上界 {EVERY_ROUND_EXPONENT_CEIL}）：三档 {covered_tiers:?} 的 min 耗时 {mins:?}。\
             这条比上一条更难被负载抬高（44 轮整包负载实测最坏 1.01），它红说明每一轮都看见超线性增长。",
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
