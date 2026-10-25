//! G文件 的端到端：发送区不是文件时，插件必须在**真实宿主**上走通
//! 「请求文件选择框 → 宿主回路径并授权 → 插件读文件 → 按行发出去」。
//!
//! 为什么不写成场景文件：场景运行器没有 `set_host_services`，插件在它那里连 `ctx.dialog` /
//! `ctx.fs` 都没有（两个 API 由宿主服务决定是否暴露），而这条链路的两端恰好是宿主侧。
//! 于是这里自己起一个宿主：`ctx.dialog.open_file` 是**阻塞式**的（`dialog.rs` 里 100ms 轮询
//! 等 `response_sender`），所以扮演 UI 的那一步放在独立线程里 —— 与本仓 `poll_dialog_requests`
//! 的角色一致（收到请求 → 授权选中文件 → 回路径）。
//!
//! 三个方向各一格：不是文件的一行、（看起来像路径却打不开的一行，例如 `G1 X1.5`）、
//! 以及在选择框里取消。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tool_core::{Direction, Event, Payload, topics};
use tool_databus::{DataBus, TopicFilter};
use tool_extension::PluginManager;
use tool_extension::host_services::{DialogRequest, FileAccessBroker};
use tool_transport::{TransportManager, serial_topics};

/// 插件要发的行数（M110 同步 + 文件两行 + 默认追加的 M2）：收齐即可停，不必等满超时。
const EXPECTED_LINES: usize = 4;

fn plugins_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins")
}

/// 跑一次「点 G文件」：`input` 是发送区内容，`pick` 为 false 表示在选择框里取消。
///
/// 返回插件真实写进串口的行（按到达顺序）。
fn click_send_file(case: &str, input: &str, pick: bool, wait: Duration) -> Vec<String> {
    let bus = DataBus::new();
    let transport = TransportManager::new(bus.clone());
    let mut manager = PluginManager::new(bus.clone(), transport.clone());

    let (dialog_sender, dialog_requests) = crossbeam_channel::unbounded::<DialogRequest>();
    let broker = Arc::new(FileAccessBroker::default());
    manager.set_host_services(dialog_sender, broker.clone());

    let port = transport.open_virtual_serial("TEST").expect("打开虚拟串口");
    manager
        .discover_roots([plugins_root()])
        .expect("扫描插件目录");
    manager.enable("gcode-sender").expect("启用 gcode-sender");

    // 每个用例一份自己的夹具目录：两个用例在同一个进程里并行跑。
    let dir = std::env::temp_dir().join(format!("hw-gcode-dialog-{case}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建夹具目录");
    let picked = dir.join("picked.gcode");
    std::fs::write(&picked, "; comment\nG1 X1\nG1 X2\n").expect("写夹具文件");

    // ── 扮演宿主 UI：等插件的选择框请求，按 `pick` 授权并回答 ──
    let picked_for_host = picked.clone();
    let broker_for_host = broker.clone();
    let host = std::thread::spawn(move || {
        let request = dialog_requests
            .recv_timeout(Duration::from_secs(10))
            .expect("插件必须请求文件选择框");
        assert_eq!(request.plugin_id, "gcode-sender");
        assert_eq!(request.title, "选择 G-code 文件");
        let accepted = pick.then(|| {
            broker_for_host.authorize("gcode-sender", picked_for_host.clone());
            picked_for_host
        });
        let _ = request.response_sender.send(accepted);
    });

    let subscription = bus.subscribe(TopicFilter::All);
    bus.publish(Event::new(
        topics::PLUGIN_COMMAND_EXECUTE,
        "test",
        Direction::Internal,
        Payload::Json(serde_json::json!({
            "plugin_id": "gcode-sender",
            "command": "gcode-sender.send_file",
            "context": {
                "send": { "input": input, "target_port": "TEST", "target_port_open": true }
            },
            "payload": null
        })),
    ));

    let deadline = Instant::now() + wait;
    let mut lines = Vec::new();
    while Instant::now() < deadline && lines.len() < EXPECTED_LINES {
        manager.process_pending();
        // 设备侧应答：每轮补一条 ok，插件写完一行就有一条等着它。
        port.inject_rx(b"ok\n".to_vec());
        for event in subscription.drain() {
            if event.topic == serial_topics::SERIAL_TX
                && let Payload::Bytes(bytes) = &event.payload
            {
                lines.push(String::from_utf8_lossy(bytes).to_string());
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    host.join().expect("宿主线程");

    let _ = std::fs::remove_file(&picked);
    let _ = std::fs::remove_dir(&dir);
    lines
}

/// 发送区里是一行 G-code（不是文件）⇒ 打开选择框，并把选中的文件按行发出去。
#[test]
fn non_file_input_opens_the_picker_and_sends_the_chosen_file() {
    let lines = click_send_file("plain", "G1 X9", true, Duration::from_secs(15));
    assert!(
        lines.iter().any(|line| line.starts_with("N1 G1 X1*")),
        "选中文件的第一行必须发出去，实际 {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.starts_with("N2 G1 X2*")),
        "选中文件的第二行必须发出去，实际 {lines:?}"
    );
}

/// `G1 X1.5` 结尾像扩展名，会被当成路径；它在磁盘上打不开，探针必须把它挡回选择框。
#[test]
fn path_looking_command_still_opens_the_picker() {
    let lines = click_send_file("dotted", "G1 X1.5", true, Duration::from_secs(15));
    assert!(
        lines.iter().any(|line| line.starts_with("N1 G1 X1*")),
        "挡回选择框后必须发送选中的文件，实际 {lines:?}"
    );
}

/// 在选择框里取消 ⇒ 一行都不发（`None` 就是宿主 UI 取消时回的值）。
#[test]
fn cancelling_the_picker_sends_nothing() {
    let lines = click_send_file("cancel", "G1 X9", false, Duration::from_millis(700));
    assert!(lines.is_empty(), "取消选择后不得写串口，实际 {lines:?}");
}
