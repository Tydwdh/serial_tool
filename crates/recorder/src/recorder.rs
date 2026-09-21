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
    /// 与 `RecorderWorker::finished` 同一个标志：Stop 之后线程归本结构管，
    /// 收割侧仍要能区分"走到尾部正常退出"与"panic 展开"。
    finished: Arc<AtomicBool>,
}

struct RecorderWorker {
    stop: Arc<AtomicBool>,
    discard_pending: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    last_error: Arc<Mutex<Option<String>>>,
    join: Option<JoinHandle<()>>,
}

/// 记账一条已写入的事件：条数、字节数与平均吞吐。
///
/// 主循环与收尾 drain 走的是同一条账，两处必须算得一样，故只留这一份实现。
fn account_written(stats: &Mutex<RecorderStats>, bytes: u64, started_at: Instant) {
    let mut s = stats.lock();
    s.events_written += 1;
    s.bytes_written += bytes;
    let elapsed = started_at.elapsed().as_secs_f64();
    if elapsed > 0.0 {
        s.write_throughput_bytes_per_sec = (s.bytes_written as f64 / elapsed) as u64;
    }
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
            let handle_fatal = |msg: &str| {
                {
                    let mut s = stats_thread.lock();
                    s.last_error = Some(msg.to_owned());
                    s.running = false;
                }
                *last_error_thread.lock() = Some(msg.to_owned());
                bus.publish(Event::system_log(LogLevel::Error, "recorder", msg));
                stop_thread.store(true, Ordering::SeqCst);
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
                                Ok(bytes) => account_written(&stats_thread, bytes, started_at),
                                Err(e) => {
                                    handle_fatal(&format!("write failed: {e}"));
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
                        handle_fatal(&format!("flush failed: {e}"));
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
                            Ok(bytes) => account_written(&stats_thread, bytes, started_at),
                            Err(e) => {
                                // 这里不走 handle_fatal：worker 已经在收尾，紧接着的尾部代码就会把
                                // `running` / `stopping` 落回 false，只需把错误登记到两处副本并停止排空。
                                let msg = format!("drain write failed: {e}");
                                stats_thread.lock().last_error = Some(msg.clone());
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
                stats_thread.lock().last_error = Some(msg.clone());
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
                finished: worker.finished,
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
            // `running` / `stopping` 的正常清零点是 worker 闭包的尾部，而 panic 展开
            // 永远走不到那里；`stop_with_reason` 又只置 `stopping`、从不清 `running`。
            // 两条合起来的结果就是本分支必须就地兜住：否则状态栏会一直挂着一个冻住的
            // 红色"录制中"徽标（`status_bar.rs` / `top_bar.rs` / `device_panel.rs` /
            // `web.rs` 渲染读的都是 `stats.running`，不是 `is_running()`），Stop 按钮
            // 也退化成 no-op（`self.worker` 已被 take）。
            // **只能写在这条分支里**：放进 `stop_with_reason` 就会连带抹掉正常 Stop
            // 的 Stop→Stopping 中间态（那侧的线程随后会自己走到尾部清零）。
            {
                let mut stats = self.stats.lock();
                stats.running = false;
                stats.stopping = false;
            }
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
        let s = self.stopping.take()?;
        if !s.join.is_finished() {
            self.stopping = Some(s);
            return None;
        }
        let _ = s.join.join();
        // Stop **之后**才 panic 的那一侧只能在这里兜住：`stats.running` /
        // `stats.stopping` 的唯一其它清零点是 worker 闭包尾部（panic 展开到不了），
        // 而 `worker_panicked()` 只看 `self.worker`，线程已被 `stop_with_reason`
        // 搬进 `self.stopping`，那条分支结构上永远不再为它触发。两个字段是 UI
        // 唯一的读取源，留着 true 的后果是：`commands.rs` 把 `running || stopping`
        // 映射成 StopRecording ⇒ 本次会话再也派发不出 StartRecording，面板与
        // 状态栏也一直冻在"正在停止"。
        // 正常 Stop（含 `handle_fatal` 的写失败路径）会置 `finished`，那两个字段
        // 由 worker 自己清零，这里不得代劳。
        if !s.finished.load(Ordering::SeqCst) {
            let mut stats = self.stats.lock();
            stats.running = false;
            stats.stopping = false;
        }
        // 单独一条语句取出错误：`last_error` 的 guard 到此即释放，不留到下面拿
        // `stats` 锁、发日志的时候还握着。
        let error = s.last_error.lock().take();
        match error {
            Some(e) => {
                self.bus.publish(Event::system_log(
                    LogLevel::Error,
                    "recorder",
                    format!("录制失败：{}：{e}", s.path.display()),
                ));
                Some(Err(e))
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
                Some(Ok(s.path))
            }
        }
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

    /// 清掉一次录制留下的两个文件：`.jsonl` 本体与同名 `.summary.json`。
    /// 摘要未必会生成（例如 start 就失败了），删不到即忽略。
    fn remove_recording(path: &Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("summary.json"));
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

        remove_recording(&path);
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

        remove_recording(&path);
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
        remove_recording(&path1);
        let _ = fs::remove_file(&path2);
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

        remove_recording(&path);
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

        remove_recording(&path);
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
    ///
    /// `stats.running` 一并置为 true，复刻 `start()` 里的 `RecorderStats { running:
    /// true, .. }` —— UI 渲染读的是这个字段（不是 `is_running()`），fixture 不置起来
    /// 就测不到"冻住的录制中徽标"。orderly 一侧再复刻 worker 尾部的清零，panic 一侧
    /// 刻意不清：两条用例的差别正是被测的那条展开路径。
    fn attach_exited_worker(rec: &mut JsonlRecorder, orderly: bool) {
        let finished = Arc::new(AtomicBool::new(false));
        let stats = Arc::clone(&rec.stats);
        {
            // 与 `start()` 的 `RecorderStats { running: true, ..Default::default() }` 一致。
            let mut s = stats.lock();
            s.running = true;
        }
        let finished_for_thread = Arc::clone(&finished);
        let stats_for_thread = Arc::clone(&stats);
        let join: JoinHandle<()> = thread::spawn(move || {
            if orderly {
                // 与生产闭包尾部的 `finished_thread.store(true, SeqCst)` 同一件事。
                finished_for_thread.store(true, Ordering::SeqCst);
                let mut s = stats_for_thread.lock();
                s.running = false;
                s.stopping = false;
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

        remove_recording(&path);
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
        assert!(
            rec.stats().running,
            "装配前提：UI 渲染用的 `stats.running` 此时必须是 true（复刻 start() 之后）"
        );

        rec.tick_backpressure();

        assert!(
            !rec.is_running(),
            "worker 已异常退出的 recorder 必须停止自称\"正在录制\""
        );
        let stats = rec.stats();
        // 关键断言：`is_running()` 不是 UI 读的字段。状态栏/顶栏/录制面板/web 渲染的
        // 都是 `stats.running`，它只有 worker 闭包尾部一处清零点，而展开路径到不了 ——
        // 不在这里清，用户看到的就是一个永久冻住的红色"录制中"徽标。
        assert!(
            !stats.running,
            "UI 渲染的 `stats.running` 必须被 panic 分支清零，否则状态栏永远显示\"录制中\""
        );
        // 同一个新调用点会经 `stop_with_reason` 置 `stopping = true`，而线程不会再走到
        // 尾部去清它 —— 不在这里清，就把"冻住的录制中"换成了"冻住的停止中"。
        assert!(
            !stats.stopping,
            "`stats.stopping` 同样不得留在 true：worker 已死，没有线程会去清它"
        );
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

    // ── I1：Stop **之后**才 panic 的那一侧 ──

    /// `attach_exited_worker` 的后继孪生：复刻 `stop_with_reason()` 已经跑完、而
    /// worker 随后 panic 退出时的状态。
    ///
    /// 与上面那个 fixture 的区别就是本条缺陷的全部内容：worker 已从 `self.worker`
    /// **搬进** `self.stopping`，于是 `worker_panicked()`（只看 `self.worker`）从此
    /// 再也不可能为这个线程返回 true；而 `stop_with_reason` 置起 `stats.stopping`，
    /// 这两个渲染位唯一的其它清零点在 worker 闭包尾部，panic 展开到不了。
    /// `reap_stopping()` 虽然看得到 `join.is_finished()`，修复前却从不碰 `stats`。
    ///
    /// 正常 Stop 那一侧由 `reap_stopping_after_orderly_stop_leaves_rendered_flags_clear`
    /// 用**真**线程走 start → stop → reap 覆盖：它钉的正是"worker 尾部会把这两个字段
    /// 落回 false"，而本修复的 `!finished` 门就建立在这个性质上。
    fn attach_stopping_worker_that_panicked(rec: &mut JsonlRecorder) {
        // `finished` 保持 false = 线程没有走到自己的尾部。
        let finished = Arc::new(AtomicBool::new(false));
        {
            // `stop_with_reason()` 之后 stats 的样子：stopping 刚被置起，
            // running 仍归 worker 尾部去清 —— 而这一次它永远到不了。
            let mut s = rec.stats.lock();
            s.running = true;
            s.stopping = true;
        }
        let join: JoinHandle<()> = thread::spawn(|| {
            panic!("模拟 Stop 之后录制 worker panic 展开");
        });
        wait_thread_finished(&join);
        rec.stopping = Some(StoppingRecorder {
            join,
            last_error: Arc::new(Mutex::new(None)),
            path: PathBuf::from("panic-after-stop.jsonl"),
            finished,
        });
    }

    #[test]
    fn reap_stopping_clears_rendered_flags_when_worker_panicked_after_stop() {
        let mut rec = JsonlRecorder::new(DataBus::new());
        attach_stopping_worker_that_panicked(&mut rec);

        // 装配前提：本条路径上 `worker_panicked()` 结构上就是瞎的 —— 线程已经不在
        // `self.worker` 里了。它必须保持 false，否则"修好了"其实是被别的分支修的。
        assert!(
            !rec.worker_panicked(),
            "装配前提：Stop 之后 worker 已移出 `self.worker`，`worker_panicked()` 看不到它"
        );
        assert!(
            rec.is_stopping(),
            "装配前提：Stop 已把 worker 搬进 `self.stopping`"
        );
        let before = rec.stats();
        assert!(
            before.running && before.stopping,
            "装配前提：两个渲染位在 Stop 之后必须都是 true，got {before:?}"
        );

        let reaped = rec.reap_stopping();
        assert!(
            reaped.is_some(),
            "线程已终止时 `reap_stopping()` 必须收割，不能永远停在 Stopping"
        );

        let stats = rec.stats();
        // 关键断言（本条缺陷的全部后果）：这两个字段是 UI 唯一的读取源，
        // 留着 true 就等于把 `commands.rs` 的 StartRecording 永久锁死
        // （它把 `running || stopping` 映射成 StopRecording），并让
        // `device_panel.rs` / `recording.rs` / `web.rs` 一直渲染"正在停止"。
        assert!(
            !stats.running,
            "Stop 之后 panic 的 worker 也必须清掉 `stats.running`，got {stats:?}"
        );
        assert!(
            !stats.stopping,
            "Stop 之后 panic 的 worker 也必须清掉 `stats.stopping`，got {stats:?}"
        );
    }

    /// 上一条用例的**正常**孪生：真线程走完 start → stop → reap。
    ///
    /// 它钉的是 `reap_stopping()` 里 `!finished` 那道门的前提 —— worker 闭包尾部自己
    /// 会把这两个渲染位落回 false。修复前没有任何用例断言过这件事：正常停止后的既有
    /// 用例只查 `is_stopping()`，而那读的是 `stopping: Option<_>` 字段，不是 `stats`
    /// 上的这两位。缺了这条，"收割时一律清零"这种过度修复也能全绿通过。
    #[test]
    fn reap_stopping_after_orderly_stop_leaves_rendered_flags_clear() {
        let mut rec = JsonlRecorder::new(DataBus::new());
        let path = temp_file(&format!(
            "test-orderly-stop-{}.jsonl",
            tool_core::now_timestamp_ms()
        ));

        rec.start(&path).unwrap();
        let started = rec.stats();
        assert!(
            started.running && !started.stopping,
            "装配前提：start() 之后 running=true、stopping=false，got {started:?}"
        );

        rec.stop();
        let stopping = rec.stats();
        assert!(
            stopping.running && stopping.stopping,
            "装配前提：Stop 之后 `stats.stopping` 置起、`running` 仍等 worker 尾部去清，got {stopping:?}"
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        while rec.reap_stopping().is_none() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for recorder to stop"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let stats = rec.stats();
        assert!(
            !stats.running && !stats.stopping,
            "正常 Stop 收割后两个渲染位都必须落回 false（worker 尾部自己清的），got {stats:?}"
        );
        assert!(
            !stats.incomplete,
            "正常 Stop 不得被标记为不完整，got {stats:?}"
        );

        remove_recording(&path);
    }
}
