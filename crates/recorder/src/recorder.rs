//! 录制器：订阅 DataBus 全量事件，worker 线程异步写入 jsonl 文件，
//! 支持暂停/继续、周期 flush、会话完整性摘要。
//!
//! 与 `replay.rs`（回放）零耦合。录制文件格式与过滤策略见 `format.rs`。

use parking_lot::Mutex;
use std::fs::{File, create_dir_all};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tool_core::{Direction, Event, LogLevel, Payload};
use tool_databus::{DataBus, SubscriptionBacklog, TopicFilter};

use crate::format::{RecordMode, should_record_event_with_mode, write_event_counted};

#[derive(Debug, Clone, Default)]
pub struct RecorderStats {
    pub events_written: u64,
    pub bytes_written: u64,
    pub last_flush_elapsed_ms: u64,
    pub last_error: Option<String>,
    pub running: bool,
    pub stopping: bool,
    pub paused: bool,
    pub pause_count: u64,
    pub queued_events: u64,
    pub queued_bytes: u64,
    pub seconds_behind: f64,
    pub write_throughput_bytes_per_sec: u64,
    pub backlog_warning: bool,
    pub incomplete: bool,
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct RecorderBackpressureConfig {
    pub soft_events: u64,
    pub soft_bytes: u64,
    pub soft_seconds_behind: f64,
    pub hard_events: u64,
    pub hard_bytes: u64,
    pub hard_seconds_behind: f64,
}

impl Default for RecorderBackpressureConfig {
    fn default() -> Self {
        Self {
            soft_events: 100_000,
            soft_bytes: 256 * 1024 * 1024,
            soft_seconds_behind: 10.0,
            hard_events: 1_000_000,
            hard_bytes: 512 * 1024 * 1024,
            hard_seconds_behind: 60.0,
        }
    }
}

pub struct JsonlRecorder {
    bus: DataBus,
    worker: Option<RecorderWorker>,
    stopping: Option<StoppingRecorder>,
    current_path: Option<PathBuf>,
    mode: RecordMode,
    stats: Arc<Mutex<RecorderStats>>,
    backpressure: RecorderBackpressureConfig,
    backlog: Option<SubscriptionBacklog>,
    backlog_warning_sent: bool,
}

struct StoppingRecorder {
    join: JoinHandle<()>,
    last_error: Arc<Mutex<Option<String>>>,
    path: PathBuf,
}

struct RecorderWorker {
    stop: Arc<AtomicBool>,
    discard_pending: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    last_error: Arc<Mutex<Option<String>>>,
    join: Option<JoinHandle<()>>,
    #[allow(dead_code)]
    stats: Arc<Mutex<RecorderStats>>,
}

impl JsonlRecorder {
    pub fn new(bus: DataBus) -> Self {
        Self {
            bus,
            worker: None,
            stopping: None,
            current_path: None,
            mode: RecordMode::default(),
            stats: Arc::new(Mutex::new(RecorderStats::default())),
            backpressure: RecorderBackpressureConfig::default(),
            backlog: None,
            backlog_warning_sent: false,
        }
    }

    pub fn stats(&self) -> RecorderStats {
        self.stats.lock().clone()
    }

    pub fn set_mode(&mut self, mode: RecordMode) {
        self.mode = mode;
    }

    pub fn set_backpressure_config(&mut self, config: RecorderBackpressureConfig) {
        self.backpressure = config;
    }

    pub fn backpressure_config(&self) -> RecorderBackpressureConfig {
        self.backpressure
    }

    pub fn mode(&self) -> RecordMode {
        self.mode
    }

    pub fn start(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        if self.is_running() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "recorder is already running, stop it first",
            ));
        }
        if self.is_stopping() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "recorder is still stopping previous session, please wait",
            ));
        }
        // 同步清理上一个 worker（如果有残留的 finished worker）
        if let Some(mut worker) = self.worker.take() {
            worker.stop.store(true, Ordering::Relaxed);
            if let Some(join) = worker.join.take() {
                let _ = join.join();
            }
        }
        self.stopping = None;

        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            create_dir_all(parent)?;
        }

        let file = File::create(&path)?;
        let path_for_summary = path.clone();
        // recorder 是可靠性链路，不能用 bounded 订阅。
        // UI 面板可以 bounded，recorder 必须 lossless。
        let subscription = self.bus.subscribe_lossless(TopicFilter::All);
        self.backlog = Some(subscription.backlog());
        self.backlog_warning_sent = false;
        let stop = Arc::new(AtomicBool::new(false));
        let discard_pending = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        let pause_for_worker = Arc::clone(&pause);
        let stop_thread = Arc::clone(&stop);
        let discard_pending_thread = Arc::clone(&discard_pending);
        let finished = Arc::new(AtomicBool::new(false));
        let finished_thread = Arc::clone(&finished);
        let last_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let last_error_thread = Arc::clone(&last_error);
        let mode = self.mode;

        let bus = self.bus.clone();
        let stats_thread = Arc::clone(&self.stats);
        let stats_thread_for_worker = Arc::clone(&self.stats);

        {
            let mut s = stats_thread.lock();
            *s = RecorderStats {
                running: true,
                ..RecorderStats::default()
            };
        }

        let join = thread::spawn(move || {
            let mut writer = BufWriter::new(file);
            let mut written_since_flush = 0u64;
            let mut last_flush = Instant::now();
            let started_at = Instant::now();

            // 统一的错误处理：记录错误、停止 worker、发布日志
            let handle_fatal = |msg: &str,
                                stats: &Arc<Mutex<RecorderStats>>,
                                last_err: &Arc<Mutex<Option<String>>>,
                                bus: &DataBus,
                                stop: &AtomicBool| {
                {
                    let mut s = stats.lock();
                    s.last_error = Some(msg.to_owned());
                    s.running = false;
                }
                *last_err.lock() = Some(msg.to_owned());
                bus.publish(Event::system_log(LogLevel::Error, "recorder", msg));
                stop.store(true, Ordering::SeqCst);
            };

            while !stop_thread.load(Ordering::Relaxed) {
                match subscription.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => {
                        // 暂停时只消费事件不写入
                        if pause_for_worker.load(Ordering::Relaxed) {
                            continue;
                        }
                        if should_record_event_with_mode(&event, mode) {
                            match write_event_counted(&mut writer, &event) {
                                Ok(bytes) => {
                                    let mut s = stats_thread.lock();
                                    s.events_written += 1;
                                    s.bytes_written += bytes;
                                    let elapsed = started_at.elapsed().as_secs_f64();
                                    if elapsed > 0.0 {
                                        s.write_throughput_bytes_per_sec =
                                            (s.bytes_written as f64 / elapsed) as u64;
                                    }
                                }
                                Err(e) => {
                                    let msg = format!("write failed: {e}");
                                    handle_fatal(
                                        &msg,
                                        &stats_thread,
                                        &last_error_thread,
                                        &bus,
                                        &stop_thread,
                                    );
                                    break;
                                }
                            }
                            written_since_flush += 1;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        stats_thread.lock().last_flush_elapsed_ms =
                            last_flush.elapsed().as_millis() as u64;
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }

                // 周期性 flush：每 500 条或 1 秒，防止崩溃/断电丢失尾部数据
                if written_since_flush >= 500 || last_flush.elapsed() > Duration::from_secs(1) {
                    if let Err(e) = writer.flush() {
                        let msg = format!("flush failed: {e}");
                        handle_fatal(&msg, &stats_thread, &last_error_thread, &bus, &stop_thread);
                        break;
                    }
                    written_since_flush = 0;
                    last_flush = Instant::now();
                }
            }

            stats_thread.lock().last_flush_elapsed_ms = last_flush.elapsed().as_millis() as u64;

            if !discard_pending_thread.load(Ordering::Relaxed) {
                for event in subscription.drain() {
                    if should_record_event_with_mode(&event, mode) {
                        match write_event_counted(&mut writer, &event) {
                            Ok(bytes) => {
                                let mut s = stats_thread.lock();
                                s.events_written += 1;
                                s.bytes_written += bytes;
                                let elapsed = started_at.elapsed().as_secs_f64();
                                if elapsed > 0.0 {
                                    s.write_throughput_bytes_per_sec =
                                        (s.bytes_written as f64 / elapsed) as u64;
                                }
                            }
                            Err(e) => {
                                let msg = format!("drain write failed: {e}");
                                {
                                    let mut s = stats_thread.lock();
                                    s.last_error = Some(msg.clone());
                                }
                                *last_error_thread.lock() = Some(msg.clone());
                                bus.publish(Event::system_log(LogLevel::Error, "recorder", msg));
                                break;
                            }
                        }
                    }
                }
            }

            if let Err(e) = writer.flush() {
                let msg = format!("flush failed: {e}");
                {
                    let mut s = stats_thread.lock();
                    s.last_error = Some(msg.clone());
                }
                *last_error_thread.lock() = Some(msg.clone());
                bus.publish(Event::system_log(
                    LogLevel::Error,
                    "recorder",
                    format!("写入失败：{e}"),
                ));
            }

            {
                let mut s = stats_thread.lock();
                s.running = false;
                s.stopping = false;
            }

            // ── 生成会话完整性摘要 ──
            let summary_path = path_for_summary.with_extension("summary.json");
            let (events_written, bytes_written, pause_count) = {
                let s = stats_thread.lock();
                (s.events_written, s.bytes_written, s.pause_count)
            };
            let (is_clean, error_clone, incomplete) = {
                let incomplete = stats_thread.lock().incomplete;
                let guard = last_error_thread.lock();
                (guard.is_none() && !incomplete, guard.clone(), incomplete)
            };
            let summary = serde_json::json!({
                "ended_at_ms": tool_core::now_timestamp_ms(),
                "events_written": events_written,
                "bytes_written": bytes_written,
                "record_mode": format!("{:?}", mode),
                "closed_cleanly": is_clean,
                "incomplete": incomplete,
                "pause_count": pause_count,
                "app_version": env!("CARGO_PKG_VERSION"),
                "error": error_clone,
            });
            if let Ok(text) = serde_json::to_string_pretty(&summary)
                && let Err(e) = std::fs::write(&summary_path, text)
            {
                log::warn!(
                    "recorder: failed to write summary {}: {e}",
                    summary_path.display()
                );
            }

            finished_thread.store(true, Ordering::SeqCst);
        });

        self.worker = Some(RecorderWorker {
            stop,
            discard_pending,
            pause,
            finished,
            last_error,
            join: Some(join),
            stats: stats_thread_for_worker,
        });
        self.current_path = Some(path.clone());
        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "recorder",
            format!("正在录制到 {}（模式：{:?}）", path.display(), self.mode),
        ));
        Ok(())
    }

    pub fn stop(&mut self) {
        self.stop_with_reason(false, None);
    }

    fn stop_with_reason(&mut self, discard_pending: bool, reason: Option<String>) {
        if let Some(mut worker) = self.worker.take() {
            {
                let mut stats = self.stats.lock();
                stats.stopping = true;
                if let Some(reason) = reason.as_ref() {
                    stats.incomplete = true;
                    stats.stop_reason = Some(reason.clone());
                }
            }
            self.bus.publish(Event::system_log(
                LogLevel::Info,
                "recorder",
                reason.as_deref().unwrap_or("正在停止录制..."),
            ));
            worker
                .discard_pending
                .store(discard_pending, Ordering::Relaxed);
            worker.stop.store(true, Ordering::Relaxed);
            // 异步停止：不阻塞 UI，spin 到 Stopping 状态
            let Some(join) = worker.join.take() else {
                log::warn!("recorder worker has no join handle, skipping stop");
                return;
            };
            self.stopping = Some(StoppingRecorder {
                join,
                last_error: worker.last_error,
                path: self.current_path.take().unwrap_or_default(),
            });
        }
    }

    /// worker 线程已经退出、却没有走到自己的收尾（`finished` 标志未置位）
    /// ⇒ 只剩一种可能：被 panic 展开掀掉了。
    ///
    /// `is_running()` 回答的是"用户点过停止没有"，不是"线程还活着"；只看它的
    /// 话，崩掉的 recorder 会一直报告"正在录制"，而磁盘上一个字节都不会再增加
    /// —— 用户失去的正是他此行的目的（本次会话的抓取）。
    ///
    /// 正常收尾（含 `handle_fatal` 的写失败路径）会把 `finished` 置 true，那种
    /// 情况归 `reap_error()` 报告，这里必须不误报。
    fn worker_panicked(&self) -> bool {
        self.worker.as_ref().is_some_and(|worker| {
            worker.join.as_ref().is_some_and(|join| join.is_finished())
                && !worker.finished.load(Ordering::SeqCst)
        })
    }

    /// 在 UI tick 中调用，监控 lossless 录制队列。
    ///
    /// 软阈值只产生一次告警；硬阈值会停止 worker、丢弃尚未写入的尾部并把摘要
    /// 标记为 incomplete。这样不会静默丢数据，也不会让无界队列把进程拖到 OOM。
    pub fn tick_backpressure(&mut self) {
        // worker 存活检查放在 backlog 早退**之前**：`backlog` 为 None 的录制路径
        // （未启用积压监控）同样必须被盯着，否则这条修复只在开了阈值时才生效。
        // 本函数由 `Workbench::tick` 每帧调用，故检测延迟为一帧。
        if self.worker_panicked() {
            let reason = "录制线程异常退出（panic），本次录制不完整".to_owned();
            self.stop_with_reason(true, Some(reason.clone()));
            self.bus
                .publish(Event::system_log(LogLevel::Error, "recorder", reason));
            return;
        }
        let Some(backlog) = self.backlog.clone() else {
            return;
        };
        let queued_events = backlog.queued_events();
        let queued_bytes = backlog.queued_bytes();
        let seconds_behind = backlog.seconds_behind();
        {
            let mut stats = self.stats.lock();
            stats.queued_events = queued_events;
            stats.queued_bytes = queued_bytes;
            stats.seconds_behind = seconds_behind;
        }

        let soft = queued_events >= self.backpressure.soft_events
            || queued_bytes >= self.backpressure.soft_bytes
            || seconds_behind >= self.backpressure.soft_seconds_behind;
        if soft && !self.backlog_warning_sent {
            self.backlog_warning_sent = true;
            self.stats.lock().backlog_warning = true;
            self.bus.publish(Event::system_log(
                LogLevel::Warn,
                "recorder",
                format!(
                    "录制写入落后：{} events / {} bytes / {:.1}s",
                    queued_events, queued_bytes, seconds_behind
                ),
            ));
        }

        let hard = queued_events >= self.backpressure.hard_events
            || queued_bytes >= self.backpressure.hard_bytes
            || seconds_behind >= self.backpressure.hard_seconds_behind;
        if hard && self.worker.is_some() {
            let reason = format!(
                "录制积压超过硬阈值，已停止（{} events / {} bytes / {:.1}s behind）",
                queued_events, queued_bytes, seconds_behind
            );
            self.stop_with_reason(true, Some(reason));
        }
    }

    pub fn is_running(&self) -> bool {
        self.worker.is_some()
    }

    pub fn is_paused(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|w| w.pause.load(Ordering::Relaxed))
    }

    pub fn pause(&mut self) {
        if let Some(ref worker) = self.worker {
            // 先发布 marker 事件（worker 尚未暂停，会写入文件）
            self.bus.publish(Event::new(
                "recorder.pause",
                "recorder",
                Direction::Internal,
                Payload::Text("paused".to_owned()),
            ));
            self.bus
                .publish(Event::system_log(LogLevel::Info, "recorder", "录制已暂停"));
            worker.pause.store(true, Ordering::Relaxed);
            let mut s = self.stats.lock();
            s.paused = true;
            s.pause_count += 1;
        }
    }

    pub fn resume(&mut self) {
        if let Some(ref worker) = self.worker {
            worker.pause.store(false, Ordering::Relaxed);
            self.stats.lock().paused = false;
            // 恢复后发布 marker 事件
            self.bus.publish(Event::new(
                "recorder.resume",
                "recorder",
                Direction::Internal,
                Payload::Text("resumed".to_owned()),
            ));
            self.bus
                .publish(Event::system_log(LogLevel::Info, "recorder", "录制已恢复"));
        }
    }

    /// 添加录制标记点。仅在录制中有效。
    pub fn add_bookmark(&self, name: &str) {
        if self.worker.is_some() {
            self.bus.publish(Event::new(
                "recorder.bookmark",
                "recorder",
                Direction::Internal,
                Payload::Text(name.to_owned()),
            ));
            self.bus.publish(Event::system_log(
                LogLevel::Info,
                "recorder",
                if name.is_empty() {
                    "bookmark added".to_owned()
                } else {
                    format!("bookmark: {name}")
                },
            ));
        }
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping.is_some()
    }

    /// 检查异步停止是否完成。UI 每帧调用。
    /// 返回 Some(Ok(path)) 表示完成无错误，Some(Err(err)) 表示完成但有错误。
    pub fn reap_stopping(&mut self) -> Option<Result<PathBuf, String>> {
        if let Some(s) = self.stopping.take() {
            if s.join.is_finished() {
                let _ = s.join.join();
                let error = s.last_error.lock().take();
                match error {
                    Some(e) => {
                        self.bus.publish(Event::system_log(
                            LogLevel::Error,
                            "recorder",
                            format!("录制失败：{}：{e}", s.path.display()),
                        ));
                        return Some(Err(e));
                    }
                    None => {
                        let incomplete = self.stats.lock().incomplete;
                        let level = if incomplete {
                            LogLevel::Warn
                        } else {
                            LogLevel::Info
                        };
                        let message = if incomplete {
                            format!("录制已停止，但文件不完整：{}", s.path.display())
                        } else {
                            format!("录制已保存到 {}", s.path.display())
                        };
                        self.bus
                            .publish(Event::system_log(level, "recorder", message));
                        return Some(Ok(s.path));
                    }
                }
            }
            self.stopping = Some(s);
        }
        None
    }

    /// 检查 worker 线程是否已结束，返回 error。UI 每帧调用。
    /// 返回 None 表示 worker 未完成或已完成且无错误。
    /// 正常完成时保留 current_path 供调用者读取，调用者需在读取后调用 clear_completed_path()。
    pub fn reap_error(&mut self) -> Option<String> {
        let finished = self
            .worker
            .as_ref()
            .is_some_and(|w| w.finished.load(Ordering::SeqCst));
        if !finished {
            return None;
        }
        let mut worker = self.worker.take()?;
        let error = worker.last_error.lock().clone();
        if let Some(join) = worker.join.take() {
            let _ = join.join();
        }
        if error.is_some() {
            self.current_path = None;
        }
        error
    }

    /// 录制正常完成后，调用此方法清除保留的路径信息
    pub fn clear_completed_path(&mut self) {
        if self.worker.is_none() {
            self.current_path = None;
        }
    }

    pub fn current_path(&self) -> Option<&Path> {
        self.current_path.as_deref()
    }
}

impl Drop for JsonlRecorder {
    fn drop(&mut self) {
        self.stop();
        // 兜底：等待还在 flush/drain 的 stopping 线程，防止尾部数据丢失
        if let Some(s) = self.stopping.take() {
            let _ = s.join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;
    use tool_core::{Direction, Event, Payload};

    fn test_event(topic: &str) -> Event {
        Event::new(
            topic,
            "test",
            Direction::Internal,
            Payload::Text("test".into()),
        )
    }

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(name)
    }

    // ── Test 1: new recorder is not running ──

    #[test]
    fn new_recorder_is_not_running() {
        let bus = DataBus::new();
        let rec = JsonlRecorder::new(bus);
        assert!(!rec.is_running());
        assert!(!rec.is_stopping());
        assert!(!rec.is_paused());
        let s = rec.stats();
        assert!(!s.running);
        assert!(!s.stopping);
        assert!(!s.paused);
    }

    // ── Test 2: start/stop lifecycle ──

    #[test]
    fn start_stop_lifecycle() {
        let bus = DataBus::new();
        let mut rec = JsonlRecorder::new(bus);
        let path = temp_file(&format!(
            "test-lifecycle-{}.jsonl",
            tool_core::now_timestamp_ms()
        ));

        // Before start
        assert!(!rec.is_running());
        assert!(rec.current_path().is_none());

        // Start
        rec.start(&path).unwrap();
        assert!(rec.is_running());
        assert!(!rec.is_stopping());
        assert!(rec.current_path().is_some());

        // Stop
        rec.stop();
        assert!(!rec.is_running());
        assert!(rec.is_stopping());

        // Reap — wait for worker thread to finish
        let deadline = Instant::now() + Duration::from_secs(5);
        let result = loop {
            if let Some(r) = rec.reap_stopping() {
                break r;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for recorder to stop"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(result.is_ok());
        assert!(!rec.is_stopping());

        // Verify file was created
        assert!(path.exists());

        // Cleanup
        let _ = fs::remove_file(&path);
        let summary = path.with_extension("summary.json");
        if summary.exists() {
            let _ = fs::remove_file(&summary);
        }
    }

    // ── Test 3: pause/resume ──

    #[test]
    fn pause_resume() {
        let bus = DataBus::new();
        let mut rec = JsonlRecorder::new(bus);
        let path = temp_file(&format!(
            "test-pause-{}.jsonl",
            tool_core::now_timestamp_ms()
        ));

        rec.start(&path).unwrap();
        assert!(!rec.is_paused());

        rec.pause();
        assert!(rec.is_paused());
        assert!(rec.stats().paused);

        rec.resume();
        assert!(!rec.is_paused());
        assert!(!rec.stats().paused);

        rec.stop();
        while rec.reap_stopping().is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }

        let _ = fs::remove_file(&path);
        let summary = path.with_extension("summary.json");
        if summary.exists() {
            let _ = fs::remove_file(&summary);
        }
    }

    // ── Test 4: start fails when already running ──

    #[test]
    fn start_fails_when_already_running() {
        let bus = DataBus::new();
        let mut rec = JsonlRecorder::new(bus);
        let path1 = temp_file(&format!(
            "test-double1-{}.jsonl",
            tool_core::now_timestamp_ms()
        ));
        let path2 = temp_file(&format!(
            "test-double2-{}.jsonl",
            tool_core::now_timestamp_ms()
        ));

        rec.start(&path1).unwrap();
        assert!(rec.is_running());

        let err = rec.start(&path2).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);

        // Cleanup: stop the running recorder
        rec.stop();
        while rec.reap_stopping().is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = fs::remove_file(&path1);
        let _ = fs::remove_file(&path2);
        let s1 = path1.with_extension("summary.json");
        if s1.exists() {
            let _ = fs::remove_file(&s1);
        }
    }

    // ── Test 5: stats are updated during recording ──

    #[test]
    fn stats_updated_during_recording() {
        let bus = DataBus::new();
        let mut rec = JsonlRecorder::new(bus);
        let path = temp_file(&format!(
            "test-stats-{}.jsonl",
            tool_core::now_timestamp_ms()
        ));

        // Set StandardReplay mode before start so the worker captures it
        rec.set_mode(RecordMode::StandardReplay);
        rec.start(&path).unwrap();
        let s = rec.stats();
        assert!(s.running);
        assert_eq!(s.events_written, 0);

        // Publish events that will be recorded (StandardReplay records serial/protocol/ui.panel.create)
        for i in 0..10 {
            rec.bus
                .publish(test_event(&format!("transport.serial.test.{i}")));
        }

        // Give the worker time to process
        std::thread::sleep(Duration::from_millis(500));

        let s = rec.stats();
        assert!(
            s.events_written > 0,
            "expected some events written, got {}",
            s.events_written
        );

        rec.stop();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if rec.reap_stopping().is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for recorder to stop"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let _ = fs::remove_file(&path);
        let summary = path.with_extension("summary.json");
        if summary.exists() {
            let _ = fs::remove_file(&summary);
        }
    }

    // ── Test 6: stop is idempotent (calling stop when not running doesn't crash) ──

    #[test]
    fn stop_is_idempotent() {
        let bus = DataBus::new();
        let mut rec = JsonlRecorder::new(bus);

        // Calling stop on a fresh recorder should not panic
        rec.stop();
        assert!(!rec.is_running());
        assert!(!rec.is_stopping());

        // Start, stop, then stop again
        let path = temp_file(&format!(
            "test-idempotent-{}.jsonl",
            tool_core::now_timestamp_ms()
        ));
        rec.start(&path).unwrap();
        rec.stop();
        // Second stop on already-stopping recorder should not panic
        rec.stop();
        assert!(!rec.is_running());

        while rec.reap_stopping().is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }

        // Stop after reap should not panic
        rec.stop();
        assert!(!rec.is_running());
        assert!(!rec.is_stopping());

        let _ = fs::remove_file(&path);
        let summary = path.with_extension("summary.json");
        if summary.exists() {
            let _ = fs::remove_file(&summary);
        }
    }

    // ── Test 7: recording to an invalid path fails ──

    #[test]
    fn start_fails_with_invalid_path() {
        let bus = DataBus::new();
        let mut rec = JsonlRecorder::new(bus);

        // On Windows, a path with invalid characters like NUL should fail
        // Use a path to a non-existent directory under a file (not a directory)
        let invalid_path = temp_file(&format!(
            "test-invalid-{}.jsonl",
            tool_core::now_timestamp_ms()
        ));
        // Create a file at that path, then try to use it as a directory
        fs::write(&invalid_path, b"blocker").unwrap();
        let nested = invalid_path.join("subdir").join("recording.jsonl");

        let result = rec.start(&nested);
        assert!(result.is_err(), "expected error for invalid path, got Ok");
        assert!(!rec.is_running());

        let _ = fs::remove_file(&invalid_path);
    }

    // ── worker 线程存活：panic 不得让 recorder 继续自称"正在录制" ──

    /// 自旋等待线程终止。断言对象是"线程已终止"这一事实本身，时限只是防止测试
    /// 永久卡死的兜底，**不是**通过条件（被测线程除了 panic 什么都不做）。
    fn wait_thread_finished(join: &JoinHandle<()>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !join.is_finished() {
            assert!(Instant::now() < deadline, "worker 线程应在合理时间内终止");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// 手工装配一个 worker 字段齐全、但线程已经终止的 recorder。
    /// `orderly = true` 模拟正常收尾（含写失败的 `handle_fatal` 路径：线程照样走到
    /// 尾部并置 `finished`）；`orderly = false` 模拟 panic 展开，尾部那行到不了。
    fn attach_exited_worker(rec: &mut JsonlRecorder, orderly: bool) {
        let finished = Arc::new(AtomicBool::new(false));
        let stats = Arc::clone(&rec.stats);
        let finished_for_thread = Arc::clone(&finished);
        let join: JoinHandle<()> = thread::spawn(move || {
            if orderly {
                // 与生产闭包尾部的 `finished_thread.store(true, SeqCst)` 同一件事。
                finished_for_thread.store(true, Ordering::SeqCst);
            } else {
                panic!("模拟录制 worker panic 展开");
            }
        });
        wait_thread_finished(&join);
        rec.worker = Some(RecorderWorker {
            stop: Arc::new(AtomicBool::new(false)),
            discard_pending: Arc::new(AtomicBool::new(false)),
            pause: Arc::new(AtomicBool::new(false)),
            finished,
            last_error: Arc::new(Mutex::new(None)),
            join: Some(join),
            stats,
        });
    }

    #[test]
    fn worker_panicked_is_false_when_not_started_and_while_recording() {
        let bus = DataBus::new();
        let mut rec = JsonlRecorder::new(bus);
        assert!(
            !rec.worker_panicked(),
            "未启动时不得报告 panic，否则每帧都会自杀"
        );

        let path = temp_file(&format!(
            "test-worker-panic-{}.jsonl",
            tool_core::now_timestamp_ms()
        ));
        rec.start(&path).unwrap();
        assert!(!rec.worker_panicked(), "正常录制中不得误报 worker panic");
        assert!(rec.is_running());

        rec.stop();
        while rec.reap_stopping().is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !rec.worker_panicked(),
            "停止后 worker 已移出，不得报告 panic"
        );

        let _ = fs::remove_file(&path);
        let summary = path.with_extension("summary.json");
        if summary.exists() {
            let _ = fs::remove_file(&summary);
        }
    }

    #[test]
    fn worker_panicked_is_true_only_for_thread_exiting_without_finished_flag() {
        // true 分支：线程已终止 + `finished` 未置位 = 只可能是 panic 展开。
        let mut panicked_rec = JsonlRecorder::new(DataBus::new());
        attach_exited_worker(&mut panicked_rec, false);
        assert!(
            panicked_rec.worker_panicked(),
            "worker 线程已终止且没走到尾部，必须判定为异常退出"
        );

        // 反向（防误报）：`handle_fatal` 写失败等正常收尾会置 finished=true，
        // 那种情况归 reap_error() 报告，这里不得冒充 panic。
        let mut clean_rec = JsonlRecorder::new(DataBus::new());
        attach_exited_worker(&mut clean_rec, true);
        assert!(
            !clean_rec.worker_panicked(),
            "worker 已置 finished 的正常退出不得判为 panic"
        );
    }

    #[test]
    fn tick_backpressure_stops_exited_worker_even_without_backlog_monitoring() {
        let bus = DataBus::new();
        let logs = bus.subscribe_lossless(TopicFilter::exact(tool_core::topics::LOG_SYSTEM));
        let mut rec = JsonlRecorder::new(bus.clone());
        // 关键前提：未启用积压监控。存活检查若排在 `backlog` 早退之后，这条路径
        // 上崩掉的 recorder 依旧会一直自称"正在录制"。
        rec.backlog = None;
        attach_exited_worker(&mut rec, false);
        assert!(rec.is_running(), "装配前提：worker 仍在（用户没点过停止）");

        rec.tick_backpressure();

        assert!(
            !rec.is_running(),
            "worker 已异常退出的 recorder 必须停止自称\"正在录制\""
        );
        let stats = rec.stats();
        assert!(stats.incomplete, "本次录制必须标记为不完整");
        assert!(
            stats
                .stop_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("异常退出")),
            "停止原因需说明线程异常退出，got {:?}",
            stats.stop_reason
        );
        let error_logs: Vec<String> = logs
            .drain()
            .into_iter()
            .filter(|event| {
                event
                    .metadata
                    .get("level")
                    .and_then(serde_json::Value::as_str)
                    == Some("error")
            })
            .map(|event| event.payload.text_lossy())
            .collect();
        assert!(
            error_logs.iter().any(|text| text.contains("异常退出")),
            "必须发布 Error 级日志告知用户录制线程异常退出，got {error_logs:?}"
        );
    }
}
