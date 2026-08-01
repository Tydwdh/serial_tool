use std::time::{Duration, Instant};

/// 提升当前线程为实时优先级，减少 OS 调度延迟。
///
/// 生产代码与精度测试共用此入口，避免 SetThreadPriority FFI 块散落重复。
#[cfg(target_os = "windows")]
pub(super) fn boost_thread_priority_realtime() {
    unsafe {
        unsafe extern "system" {
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
            fn GetCurrentThread() -> isize;
        }
        SetThreadPriority(GetCurrentThread(), 15); // THREAD_PRIORITY_TIME_CRITICAL
    }
}

#[cfg(not(target_os = "windows"))]
pub(super) fn boost_thread_priority_realtime() {}

/// 临近 deadline 的最后阶段用纯 spin 以保精度。
///
/// 小于该阈值时 sleep 的粒度误差会超过 spin，因此最后这段不再 sleep。
const SPIN_THRESHOLD: Duration = Duration::from_millis(2);

/// sleep 阶段每段最长睡眠，保证 `cancel` 响应延迟有上界（≤ 该值）。
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// 等待直到 `deadline` 或 `cancel` 被置为 true。
///
/// 与纯 spin 不同：剩余时间 > [`SPIN_THRESHOLD`] 时先 `thread::sleep` 让出 CPU，
/// 只在最后 ≤2ms 阶段纯 spin。这样既保住亚毫秒级到期精度，又不会在长间隔
/// （如 100ms、1s）下 100% 占满一个 CPU 核心。
///
/// sleep 期间 `cancel` 的响应会受 sleep 粒度影响，因此把剩余时间切成
/// ≤[`CANCEL_POLL_INTERVAL`] 的小段，每段醒来查一次 cancel，保证取消响应延迟有上界。
///
/// `cancel` 为 None 时不做取消检查（用于不可取消的等待）。
/// 返回 `true` 表示因 cancel 提前返回，`false` 表示正常到期。
pub(super) fn wait_until_deadline(
    deadline: Instant,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> bool {
    boost_thread_priority_realtime();

    loop {
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let remaining = deadline - now;
        if remaining > SPIN_THRESHOLD {
            // sleep 期间 cancel 响应可能滞后；先查一次，再把剩余切成小段睡
            if let Some(c) = cancel
                && c.load(std::sync::atomic::Ordering::Relaxed)
            {
                return true;
            }
            // 留 SPIN_THRESHOLD 给最后阶段；每段不超过 CANCEL_POLL_INTERVAL 以便及时查 cancel
            let sleep_dur = (remaining - SPIN_THRESHOLD).min(CANCEL_POLL_INTERVAL);
            std::thread::sleep(sleep_dur);
        } else {
            // 最后阶段：纯 spin，频繁查 cancel
            std::hint::spin_loop();
            if let Some(c) = cancel
                && c.load(std::sync::atomic::Ordering::Relaxed)
            {
                return true;
            }
        }
    }
}

#[cfg(test)]
fn measure_spin_precision(interval: Duration, samples: usize) -> (Duration, Duration, Duration) {
    boost_thread_priority_realtime();

    let start = Instant::now();
    let mut lates: Vec<Duration> = Vec::with_capacity(samples);
    for i in 0..samples {
        let deadline = start + interval * (i as u32 + 1);
        while Instant::now() < deadline {
            std::hint::spin_loop();
        }
        let now = Instant::now();
        if now > deadline {
            lates.push(now - deadline);
        }
    }

    if lates.is_empty() {
        return (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    }

    let total: Duration = lates.iter().sum();
    let avg = total / lates.len() as u32;
    // P99：排除极端 OS 调度 spike（非实时 OS 偶尔会有 1-50ms 的调度延迟）
    let p99_index = (lates.len() * 99) / 100;
    lates.sort_unstable();
    (
        avg,
        lates[p99_index.min(lates.len() - 1)],
        lates[lates.len() - 1],
    )
}

#[test]
#[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
fn spin_wait_100us_precision() {
    let (_, p99, _) = measure_spin_precision(Duration::from_micros(100), 1000);
    assert!(
        p99 <= Duration::from_micros(500),
        "p99_late {}us > 500us",
        p99.as_micros()
    );
}

#[test]
#[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
fn spin_wait_1ms_precision() {
    let (_, p99, _) = measure_spin_precision(Duration::from_millis(1), 1000);
    assert!(
        p99 <= Duration::from_millis(2),
        "p99_late {}us > 2ms",
        p99.as_micros()
    );
}

#[test]
#[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
fn spin_wait_10ms_precision() {
    let (_, p99, _) = measure_spin_precision(Duration::from_millis(10), 500);
    assert!(
        p99 <= Duration::from_micros(300),
        "p99_late {}us > 300us",
        p99.as_micros()
    );
}

#[test]
#[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
fn spin_wait_100ms_precision() {
    let (_, p99, _) = measure_spin_precision(Duration::from_millis(100), 100);
    assert!(
        p99 <= Duration::from_micros(300),
        "p99_late {}us > 300us",
        p99.as_micros()
    );
}

#[test]
#[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
fn spin_wait_no_drift() {
    boost_thread_priority_realtime();
    let interval = Duration::from_millis(1);
    let samples = 1000;
    let start = Instant::now();
    for i in 0..samples {
        let deadline = start + interval * (i as u32 + 1);
        while Instant::now() < deadline {
            std::hint::spin_loop();
        }
    }
    let expected = interval * samples as u32;
    let elapsed = Instant::now().saturating_duration_since(start);
    let drift = elapsed.abs_diff(expected);
    assert!(
        drift <= Duration::from_millis(5),
        "drift {}us",
        drift.as_micros()
    );
}

#[test]
fn wait_until_deadline_completes_when_not_cancelled() {
    // 不依赖高精度调度：只验证正常到期返回 false
    let deadline = Instant::now() + Duration::from_millis(5);
    let cancelled = wait_until_deadline(deadline, None);
    assert!(!cancelled);
    assert!(Instant::now() >= deadline);
}

#[test]
fn wait_until_deadline_returns_early_on_cancel() {
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel_clone = cancel.clone();
    // 另一线程在 10ms 后置 cancel，deadline 设 1s，应远提前返回
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        cancel_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    let start = Instant::now();
    let cancelled = wait_until_deadline(deadline, Some(&cancel));
    let elapsed = start.elapsed();
    assert!(cancelled, "should return true on cancel");
    assert!(
        elapsed < Duration::from_millis(500),
        "should return early, took {:?}",
        elapsed
    );
}
