use std::time::{Duration, Instant};

fn measure_spin_precision(interval: Duration, samples: usize) -> (Duration, Duration, Duration) {
    // 提升实时优先级
    #[cfg(target_os = "windows")]
    unsafe {
        unsafe extern "system" {
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
            fn GetCurrentThread() -> isize;
        }
        SetThreadPriority(GetCurrentThread(), 15);
    }

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
    let (avg, p99, max) = measure_spin_precision(Duration::from_micros(100), 1000);
    eprintln!(
        "100us: avg_late={}us p99_late={}us max_late={}us",
        avg.as_micros(),
        p99.as_micros(),
        max.as_micros()
    );
    assert!(
        p99 <= Duration::from_micros(500),
        "p99_late {}us > 500us",
        p99.as_micros()
    );
}

#[test]
#[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
fn spin_wait_1ms_precision() {
    let (avg, p99, max) = measure_spin_precision(Duration::from_millis(1), 1000);
    eprintln!(
        "1ms: avg_late={}us p99_late={}us max_late={}us",
        avg.as_micros(),
        p99.as_micros(),
        max.as_micros()
    );
    assert!(
        p99 <= Duration::from_millis(2),
        "p99_late {}us > 2ms",
        p99.as_micros()
    );
}

#[test]
#[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
fn spin_wait_10ms_precision() {
    let (avg, p99, max) = measure_spin_precision(Duration::from_millis(10), 500);
    eprintln!(
        "10ms: avg_late={}us p99_late={}us max_late={}us",
        avg.as_micros(),
        p99.as_micros(),
        max.as_micros()
    );
    assert!(
        p99 <= Duration::from_micros(300),
        "p99_late {}us > 300us",
        p99.as_micros()
    );
}

#[test]
#[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
fn spin_wait_100ms_precision() {
    let (avg, p99, max) = measure_spin_precision(Duration::from_millis(100), 100);
    eprintln!(
        "100ms: avg_late={}us p99_late={}us max_late={}us",
        avg.as_micros(),
        p99.as_micros(),
        max.as_micros()
    );
    assert!(
        p99 <= Duration::from_micros(300),
        "p99_late {}us > 300us",
        p99.as_micros()
    );
}

#[test]
#[ignore = "requires a quiet local machine; CI runners have unstable sub-millisecond scheduling"]
fn spin_wait_no_drift() {
    #[cfg(target_os = "windows")]
    unsafe {
        unsafe extern "system" {
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
            fn GetCurrentThread() -> isize;
        }
        SetThreadPriority(GetCurrentThread(), 15);
    }
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
    eprintln!(
        "1000x1ms: expected={}ms actual={}ms drift={}us",
        expected.as_millis(),
        elapsed.as_millis(),
        drift.as_micros()
    );
    assert!(
        drift <= Duration::from_millis(5),
        "drift {}us",
        drift.as_micros()
    );
}
