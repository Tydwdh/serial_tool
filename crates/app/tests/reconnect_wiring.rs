//! 自动重连与"断开事件"接线的形状守卫。
//!
//! 为什么是形状守卫而不是行为用例：这条链路的每一端都需要真串口（拔出设备）、真时间
//! （等退避到点）或 `eframe::CreationContext`（构造 `WorkbenchApp`，无头环境拿不到），
//! 与 `startup_order.rs` / `architecture.rs` 同一处境。判定逻辑本身（退避、上限、取消）
//! 已经在 `crates/app/src/reconnect.rs` 的纯状态机用例里逐格驱动过了；这里只钉**接线**：
//! 驱动被调用、事件流只有一个消费者、旧的死状态没有再回来、退避公式只有一份。
//!
//! 每一条都对着一个具体的回归：谁把 `tick_reconnects` 从帧循环里删掉（于是重连永不推进）、
//! 谁把 `pending_reconnect` 那个死字段搬回来、谁又抄一份退避公式。

use std::fs;

fn read(relative: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", path.display()))
}

/// 帧循环里必须有且只有一处驱动自动重连。
#[test]
fn the_frame_tick_drives_the_reconnect_loop_exactly_once() {
    let tick = read("src/runtime/mod.rs");
    assert_eq!(
        tick.matches("self.tick_reconnects(ctx);").count(),
        1,
        "每帧必须推进一次自动重连（多了会重复派发、少了整个重连表就冻住）"
    );
}

/// 断开事件流只有一个消费者，而且它就是上面那个驱动。
#[test]
fn only_the_reconnect_tick_consumes_the_serial_disconnect_stream() {
    let mut consumers = Vec::new();
    for entry in fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")).unwrap()
    {
        let entry = entry.expect("遍历 src");
        let path = entry.path();
        if !path.is_file() || path.extension().is_some_and(|ext| ext != "rs") {
            continue;
        }
        let source = fs::read_to_string(&path).expect("读源文件");
        if source.contains("drain_serial_disconnects") {
            consumers.push(path.file_name().unwrap().to_string_lossy().to_string());
        }
    }
    assert_eq!(
        consumers,
        vec!["commands.rs".to_owned()],
        "`transport.serial.closed` 只该由重连驱动消费：两个消费者会让同一条断开被处理两遍"
    );
    let commands = read("src/commands.rs");
    assert!(
        commands.contains("self.ui_events.drain_serial_disconnects(32)"),
        "驱动里必须真的去读这条流"
    );
}

/// 旧的「单槽 + 死字段」待重连状态不许回来。
#[test]
fn the_dead_pending_reconnect_state_is_gone() {
    let state = read("src/state.rs");
    assert!(
        state.contains("pub(crate) reconnects: crate::reconnect::ReconnectTracker"),
        "串口 UI 状态必须持有重连表"
    );
    // 死状态的本体：那个结构体与它在 UI 状态里的槽位。
    for relative in ["src/state.rs", "src/commands.rs", "src/ui/top_bar.rs"] {
        let source = read(relative);
        assert!(
            !source.contains("struct PendingReconnect") && !source.contains("PendingReconnect {"),
            "{relative} 里又出现了 `PendingReconnect`：它的 attempts/next_try_at 从前没有任何写入点，\
             UI 因此永远显示 `0.0s (1/10)`，派发也只发生在「端口回到枚举列表」那一帧。\
             重连状态属于 `crate::reconnect::ReconnectTracker`"
        );
    }
    // 端口行那个 ⟳ 必须来自重连表（`pending_reconnect` 是 tool-panels 的展示字段，这里钉的是它的来源）。
    let ui = read("src/ui/mod.rs");
    assert!(
        ui.contains("self.serial.reconnects.entry(port.id.as_str()).is_some()"),
        "端口行的重连标记必须由重连表算出"
    );
}

/// 退避公式与上限只有一份实现，native 与浏览器共用。
#[test]
fn the_backoff_formula_and_attempt_cap_are_shared_by_both_hosts() {
    let shared = read("src/reconnect.rs");
    assert!(
        shared.contains("pub(crate) fn reconnect_delay(attempts: u32) -> f64"),
        "共享退避公式必须在 `src/reconnect.rs`"
    );
    assert!(
        shared.contains("pub(crate) const RECONNECT_MAX_ATTEMPTS: u32 = 10;"),
        "尝试上限必须只有一个定义"
    );

    let web = read("src/web/serial.rs");
    assert!(
        !web.contains("fn web_reconnect_delay"),
        "浏览器侧不许再抄一份退避公式"
    );
    assert!(
        web.contains("crate::reconnect::reconnect_delay("),
        "浏览器侧必须用共享公式"
    );
    assert!(
        !web.contains(">= 10 {"),
        "浏览器侧的重试上限必须用共享常量，不许再写死 10"
    );

    let native = read("src/commands.rs");
    assert!(
        native.contains("RECONNECT_MAX_ATTEMPTS"),
        "native 的重连文案与判定必须用共享上限"
    );
}
