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
use tool_databus::{DataBus, TopicFilter};

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
}

pub struct JsonlRecorder {
    bus: DataBus,
    worker: Option<RecorderWorker>,
    stopping: Option<StoppingRecorder>,
    current_path: Option<PathBuf>,
    mode: RecordMode,
    stats: Arc<Mutex<RecorderStats>>,
}

struct StoppingRecorder {
    join: JoinHandle<()>,
    last_error: Arc<Mutex<Option<String>>>,
    path: PathBuf,
}

struct RecorderWorker {
    stop: Arc<AtomicBool>,
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
        }
    }

    pub fn stats(&self) -> RecorderStats {
        self.stats.lock().clone()
    }

    pub fn set_mode(&mut self, mode: RecordMode) {
        self.mode = mode;
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
        let stop = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        let pause_for_worker = Arc::clone(&pause);
        let stop_thread = Arc::clone(&stop);
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

            for event in subscription.drain() {
                if should_record_event_with_mode(&event, mode) {
                    match write_event_counted(&mut writer, &event) {
                        Ok(bytes) => {
                            let mut s = stats_thread.lock();
                            s.events_written += 1;
                            s.bytes_written += bytes;
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
            let (is_clean, error_clone) = {
                let guard = last_error_thread.lock();
                (guard.is_none(), guard.clone())
            };
            let summary = serde_json::json!({
                "ended_at_ms": tool_core::now_timestamp_ms(),
                "events_written": events_written,
                "bytes_written": bytes_written,
                "record_mode": format!("{:?}", mode),
                "closed_cleanly": is_clean,
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
        if let Some(mut worker) = self.worker.take() {
            self.stats.lock().stopping = true;
            self.bus.publish(Event::system_log(
                LogLevel::Info,
                "recorder",
                "正在停止录制...",
            ));
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
                        self.bus.publish(Event::system_log(
                            LogLevel::Info,
                            "recorder",
                            format!("录制已保存到 {}", s.path.display()),
                        ));
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
}
