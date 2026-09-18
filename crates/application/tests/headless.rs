use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use tool_application::plugin::PluginStateView;
use tool_application::query::{ReplayBlockReasonView, ReplayPolicyView, ReplayStateView};
use tool_application::{AppCommand, AppError, CommandOutcome, TaskId, TaskState, Workbench};
use tool_core::{Direction, Event, LogLevel, Payload, topics};
use tool_databus::{DataBus, TopicFilter};
use tool_platform::storage::FileHandle;
use tool_platform::{NetworkSerialConfig, PortId, SerialSettings};
use tungstenite::Message;

#[test]
fn headless_workbench_can_dispatch_and_query() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);

    // RefreshPorts should be executable without egui.
    let refresh = wb.dispatch(AppCommand::RefreshPorts).expect("refresh");
    assert!(matches!(
        refresh,
        tool_application::CommandOutcome::Pending { .. }
    ));

    // ClearTerminal must not require egui.
    wb.dispatch(AppCommand::ClearTerminal).expect("clear");

    // Query APIs must be accessible.
    let _ = wb.query_transport();
    let _ = wb.query_recording();
    let _ = wb.query_replay();
    let _ = wb.query_plugins();

    // Incremental terminal query returns delta without cloning full history.
    let d1 = wb.query_terminal_since(0, 100);
    assert_eq!(d1.entries.len(), 0);
    assert_eq!(d1.next_seq, 0);
    assert!(!d1.truncated);

    // Invalid connect should return transport error, not panic.
    let connect = wb.dispatch(AppCommand::Connect {
        port: tool_platform::PortId::new("COM_NOT_EXIST_999"),
        settings: tool_platform::SerialSettings::default(),
    });
    assert!(matches!(
        connect,
        Ok(tool_application::CommandOutcome::Pending { .. })
    ));

    // Tick 必须能回收后台任务；无效连接最终应落到 Failed，而不是在 dispatch
    // 阶段阻塞或直接把硬件错误同步抛回 UI。
    for i in 0..100 {
        wb.tick(i as f64 * 0.01);
        if wb.task_snapshots().iter().any(|snapshot| {
            snapshot.kind == "connect_serial"
                && matches!(snapshot.state, tool_application::TaskState::Failed)
        }) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(wb.task_snapshots().iter().any(|snapshot| {
        snapshot.kind == "connect_serial"
            && matches!(snapshot.state, tool_application::TaskState::Failed)
    }));
}

#[test]
fn terminal_delta_is_incremental() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus.clone());

    // publish a serial RX event
    bus.publish(tool_transport::serial_rx_event(
        "serial:COM1",
        b"hello\n".to_vec(),
    ));

    // terminal ingests via tick
    wb.tick(0.0);

    let d = wb.query_terminal_since(0, 10);
    assert_eq!(d.entries.len(), 1);
    assert!(d.next_seq > 0);

    // second query with next_seq should be empty (incremental)
    let d2 = wb.query_terminal_since(d.next_seq, 10);
    assert_eq!(d2.entries.len(), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Headless 契约测试（docs/ARCHITECTURE.md「剩余工作」3 / todo.txt §39）
//
// 五类行为：send routing / terminal bound / replay lifecycle /
// plugin enable-disable / event emission
//
// 全部用例只经由 Workbench + AppCommand + query_* 的真实 API 驱动：不启动 egui，
// 不打开真实串口，不访问仓库 plugins/ 目录，不发网络请求。断言目标是真实状态
// 变化（任务 kind / 终端 seq 上限淘汰 / 回放状态机 / 插件状态机 / DataBus 事件），
// 不是"调用不报错"。
// ─────────────────────────────────────────────────────────────────────────────

/// 读取任务快照的 kind；任务快照被回收或任务不存在时返回 None。
fn task_kind(wb: &Workbench, task_id: TaskId) -> Option<String> {
    wb.task_snapshots()
        .into_iter()
        .find(|snapshot| snapshot.id == task_id)
        .map(|snapshot| snapshot.kind)
}

/// 读取任务快照的状态；任务不存在时返回 None。
fn task_state(wb: &Workbench, task_id: TaskId) -> Option<TaskState> {
    wb.task_snapshots()
        .into_iter()
        .find(|snapshot| snapshot.id == task_id)
        .map(|snapshot| snapshot.state)
}

/// 断言命令进入异步执行，并返回其 task id。
fn expect_pending(wb: &mut Workbench, command: AppCommand) -> TaskId {
    match wb.dispatch(command).expect("命令必须被接受") {
        CommandOutcome::Pending { task_id, .. } => task_id,
        CommandOutcome::Done => panic!("该命令必须返回 Pending，而不是同步完成"),
    }
}

/// 断言命令被接受并同步完成（`CommandOutcome::Done`）。
///
/// `CommandOutcome` 不实现 `PartialEq`（Application 有意不暴露比较语义），
/// 因此这里用 match 而不是 assert_eq。
fn expect_done(wb: &mut Workbench, command: AppCommand) {
    match wb.dispatch(command).expect("命令必须被接受") {
        CommandOutcome::Done => {}
        CommandOutcome::Pending { task_id, message } => {
            panic!("命令应同步完成，实际 Pending({task_id:?}, {message})")
        }
    }
}

/// 反复 tick（模拟 UI 帧）直到条件成立或超时，返回条件是否成立。
fn tick_until(
    wb: &mut Workbench,
    timeout: Duration,
    mut done: impl FnMut(&Workbench) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        wb.tick(0.0);
        if done(wb) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// 生成一份真实 JSONL 录制片段：工作台 recorder 的落盘格式就是每行一个 Event。
fn write_replay_fixture(name: &str, event_count: usize) -> PathBuf {
    let dir = std::env::temp_dir().join("serial-tool-headless-replay");
    std::fs::create_dir_all(&dir).expect("创建回放临时目录");
    let path = dir.join(format!("{name}-{}.jsonl", std::process::id()));
    let base_ms = 1_700_000_000_000_u64;
    let mut text = String::new();
    for index in 0..event_count {
        let mut event = Event::with_timestamp(
            base_ms + index as u64 * 10,
            tool_transport::serial_topics::SERIAL_RX,
            "serial:COM_REPLAY",
            Direction::Rx,
            Payload::Bytes(format!("replay-{index}\n").into_bytes()),
        );
        event.id = index as u64 + 1;
        text.push_str(&serde_json::to_string(&event).expect("序列化回放事件"));
        text.push('\n');
    }
    std::fs::write(&path, text).expect("写入回放临时文件");
    path
}

/// 生成一个最小 Lua 插件（写在临时目录，不触碰仓库 plugins/）。
///
/// `ctx.bus.on` 是必须的：没有 callback/command/timer/task 的脚本执行完，运行时
/// 会立即退出（lua_host「插件已完成（无回调）」），启用后状态马上会变 Finished。
fn write_probe_plugin() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "serial-tool-headless-plugin-{}",
        std::process::id()
    ));
    let plugin_dir = root.join("headless.probe");
    std::fs::create_dir_all(&plugin_dir).expect("创建插件临时目录");
    std::fs::write(
        plugin_dir.join("plugin.json"),
        r#"{
  "id": "headless.probe",
  "name": "Headless Probe",
  "version": "0.0.1",
  "api_version": "0.1",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["log", "bus"],
  "live": {
    "main": "main.lua",
    "permissions": ["log", "bus"],
    "subscriptions": ["ui.set.status"]
  }
}
"#,
    )
    .expect("写入 plugin.json");
    std::fs::write(
        plugin_dir.join("main.lua"),
        "ctx.log.info(\"headless probe plugin started\")\nctx.bus.on(\"ui.set.status\", function(event) end)\n",
    )
    .expect("写入 main.lua");
    root
}

#[test]
fn send_routing_dispatches_by_port_kind_and_rejects_invalid_hex() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus.clone());
    let serial = PortId::new("COM_ROUTE_TEST");

    // 串口端口：文本 / 原始字节 / HEX 都路由到 send_serial 有序任务。
    let text_task = expect_pending(
        &mut wb,
        AppCommand::SendText {
            port: serial.clone(),
            text: "AT\r\n".to_owned(),
        },
    );
    assert_eq!(task_kind(&wb, text_task).as_deref(), Some("send_serial"));
    let raw_task = expect_pending(
        &mut wb,
        AppCommand::SendRaw {
            port: serial.clone(),
            bytes: vec![0x01, 0x02, 0x03],
        },
    );
    assert_eq!(task_kind(&wb, raw_task).as_deref(), Some("send_serial"));
    let hex_task = expect_pending(
        &mut wb,
        AppCommand::SendHex {
            port: serial.clone(),
            hex: "AB CD".to_owned(),
            strict: true,
        },
    );
    assert_eq!(task_kind(&wb, hex_task).as_deref(), Some("send_serial"));

    // 非法 HEX 必须在 dispatch 阶段失败：不产生后台任务，也不把脏数据交给 transport。
    let tasks_before = wb.task_snapshots().len();
    let strict_error = wb
        .dispatch(AppCommand::SendHex {
            port: serial.clone(),
            hex: "AB C".to_owned(),
            strict: true,
        })
        .expect_err("strict 模式必须拒绝单 nibble token");
    assert!(
        matches!(strict_error, AppError::Transport(_)),
        "strict hex 错误类型应为 AppError::Transport，实际: {strict_error:?}"
    );
    let garbage_error = wb
        .dispatch(AppCommand::SendHex {
            port: serial.clone(),
            hex: "ZZ".to_owned(),
            strict: false,
        })
        .expect_err("非 HEX 字符必须被拒绝");
    assert!(matches!(garbage_error, AppError::Transport(_)));
    assert_eq!(
        wb.task_snapshots().len(),
        tasks_before,
        "HEX 解析失败不得产生后台任务"
    );

    // 同一输入在非严格模式下按兼容规则（单 nibble 左补 0）放行，证明 strict 语义真实生效。
    let lenient_task = expect_pending(
        &mut wb,
        AppCommand::SendHex {
            port: serial.clone(),
            hex: "AB C".to_owned(),
            strict: false,
        },
    );
    assert_eq!(task_kind(&wb, lenient_task).as_deref(), Some("send_serial"));

    // 虚拟网络端口：注册后发送走 send_network 分支，DTR/RTS 被明确拒绝。
    let network = NetworkSerialConfig {
        host: "127.0.0.1".to_owned(),
        port: 9,
        api_key: None,
    };
    let network_name = network.display_name();
    expect_done(&mut wb, AppCommand::RegisterNetworkPort { config: network });
    assert!(
        wb.query_transport()
            .ports
            .iter()
            .any(|port| port.port_name == network_name && port.port_type.is_network()),
        "注册后 query_transport().ports 应包含类型为 Network 的 {network_name}"
    );

    let network_task = expect_pending(
        &mut wb,
        AppCommand::SendText {
            port: PortId::new(network_name.clone()),
            text: "ping".to_owned(),
        },
    );
    assert_eq!(
        task_kind(&wb, network_task).as_deref(),
        Some("send_network"),
        "网络端口的发送不得落到 send_serial"
    );
    let signal_error = wb
        .dispatch(AppCommand::SetDtr {
            port: PortId::new(network_name.clone()),
            value: true,
        })
        .expect_err("网络端口不支持 DTR");
    assert!(matches!(signal_error, AppError::Transport(_)));

    // 端口未打开时，所有发送任务最终必须落 Failed，而不是静默成功。
    let sends = [text_task, raw_task, hex_task, lenient_task, network_task];
    let all_failed = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        sends
            .iter()
            .all(|task_id| task_state(wb, *task_id) == Some(TaskState::Failed))
    });
    assert!(
        all_failed,
        "端口未打开时发送任务必须以 Failed 收尾，实际: {:?}",
        sends.map(|task_id| task_state(&wb, task_id))
    );
}

#[test]
fn terminal_bound_clamps_max_entries_and_pages_incrementally() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus.clone());

    // 设 5 条上限：store 有 max(100) 的夹取，因此实际保留 100 条。
    wb.dispatch(AppCommand::SetTerminalMaxEntries { max: 5 })
        .expect("设置终端上限");
    for index in 0..150 {
        bus.publish(tool_transport::serial_rx_event(
            "serial:COM_BOUND",
            format!("line-{index}\n").into_bytes(),
        ));
    }
    wb.tick(0.0);

    let all = wb.query_terminal_since(0, usize::MAX);
    assert_eq!(all.entries.len(), 100, "终端上限必须被 max(100) 夹取");
    assert_eq!(
        all.entries.first().map(|entry| entry.seq),
        Some(51),
        "超过上限时必须按 seq 淘汰最旧条目"
    );
    assert_eq!(all.entries.last().map(|entry| entry.seq), Some(150));
    assert_eq!(all.next_seq, 150);
    assert!(!all.truncated, "limit 足够大时不得标记截断");
    assert_eq!(all.dropped, 0, "150 条事件不应触发订阅环丢包");

    // limit 截断：一次只取 1 条时必须标记 truncated，且 next_seq 只推进到该条。
    let first_page = wb.query_terminal_since(0, 1);
    assert_eq!(first_page.entries.len(), 1);
    assert!(
        first_page.truncated,
        "limit 小于剩余条数时必须标记 truncated"
    );
    assert_eq!(first_page.next_seq, 51);

    // 按 next_seq 逐页消费：必须不重不漏。
    let mut cursor = 0_u64;
    let mut seen = Vec::new();
    loop {
        let page = wb.query_terminal_since(cursor, 7);
        if page.entries.is_empty() {
            break;
        }
        assert_eq!(page.next_seq, page.entries.last().expect("page 非空").seq);
        seen.extend(page.entries.iter().map(|entry| entry.seq));
        cursor = page.next_seq;
        if !page.truncated {
            break;
        }
    }
    assert_eq!(
        seen,
        (51..=150).collect::<Vec<u64>>(),
        "增量分页必须按 seq 不重不漏地覆盖保留区间"
    );

    // ClearTerminal 清空条目，但已消费的游标不得回退。
    wb.dispatch(AppCommand::ClearTerminal).expect("清空终端");
    let cleared = wb.query_terminal_since(cursor, usize::MAX);
    assert!(cleared.entries.is_empty());
    assert_eq!(cleared.next_seq, cursor, "清空后 next_seq 不得回退");

    // 清空后新条目 seq 继续单调递增（稳定 ID 不复用），旧游标不会重复收到历史。
    for index in 0..3 {
        bus.publish(tool_transport::serial_rx_event(
            "serial:COM_BOUND",
            format!("after-{index}\n").into_bytes(),
        ));
    }
    wb.tick(0.0);
    let after = wb.query_terminal_since(cursor, usize::MAX);
    assert_eq!(after.entries.len(), 3);
    assert!(
        after.entries.iter().all(|entry| entry.seq > cursor),
        "清空后新条目 seq 必须继续递增"
    );
    assert!(
        !wb.query_terminal_since(0, usize::MAX).entries.is_empty(),
        "清空后写入的条目必须可从 seq=0 重新读回"
    );
}

#[test]
fn replay_lifecycle_loads_plays_finishes_stops_and_survives_bad_file() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus.clone());

    // 未加载：状态 Empty、不可播放/不可 seek。
    let empty = wb.query_replay();
    assert_eq!(empty.state, ReplayStateView::Empty);
    assert!(!empty.can_play);
    assert!(!empty.can_seek);
    assert_eq!(empty.total_events, 0);

    // 控制类命令在未加载时也必须安全返回 Ok，且不得把状态推进到 Playing。
    for command in [
        AppCommand::ReplayPlay,
        AppCommand::ReplayPause,
        AppCommand::ReplayStop,
        AppCommand::ReplaySeek { position_ms: 50 },
        AppCommand::ReplaySeekBy { delta_ms: -25 },
        AppCommand::ReplayStep { delta: 3 },
        AppCommand::SetReplaySpeed { speed: 4.0 },
        AppCommand::SetReplayPolicy {
            policy: ReplayPolicyView::ExactRecorded,
        },
    ] {
        expect_done(&mut wb, command);
    }
    let after_commands = wb.query_replay();
    assert_eq!(
        after_commands.state,
        ReplayStateView::Empty,
        "未加载时 ReplayPlay 不得假装进入 Playing"
    );
    assert_eq!(after_commands.position_ms, 0, "空回放不得产生非零位置");
    wb.dispatch(AppCommand::SetReplayPolicy {
        policy: ReplayPolicyView::AutoPreferRecorded,
    })
    .expect("恢复默认策略");

    // 未加载时的书签增删：位置停在 0，增删必须真实反映到 query_replay()。
    wb.dispatch(AppCommand::AddReplayBookmark { name: None })
        .expect("添加书签");
    let bookmarked_empty = wb.query_replay();
    assert_eq!(bookmarked_empty.bookmarks.len(), 1);
    assert_eq!(bookmarked_empty.bookmarks[0].position_ms, 0);
    wb.dispatch(AppCommand::RemoveReplayBookmark { position_ms: 0 })
        .expect("删除书签");
    assert!(wb.query_replay().bookmarks.is_empty());

    // 加载真实 JSONL（20 条 10ms 间隔的串口 RX）。
    let fixture = write_replay_fixture("lifecycle", 20);
    let load_task = expect_pending(
        &mut wb,
        AppCommand::LoadReplay {
            file: FileHandle::from_native_path(&fixture),
        },
    );
    assert_eq!(task_kind(&wb, load_task).as_deref(), Some("load_replay"));
    assert!(
        tick_until(&mut wb, Duration::from_secs(10), |wb| matches!(
            task_state(wb, load_task),
            Some(TaskState::Completed | TaskState::Failed)
        )),
        "load_replay 任务必须在超时前结束"
    );
    assert_eq!(
        task_state(&wb, load_task),
        Some(TaskState::Completed),
        "合法录制文件必须加载成功"
    );

    let loaded = wb.query_replay();
    assert_eq!(loaded.state, ReplayStateView::Loaded);
    assert_eq!(loaded.total_events, 20);
    assert_eq!(
        loaded.load_report.as_ref().map(|report| report.loaded),
        Some(20)
    );
    assert_eq!(loaded.duration_ms, 190);
    assert!(
        !loaded.has_recorded_protocol,
        "只有 transport.serial.* 的录制不应含 protocol.* 事件"
    );
    // 无 protocol.* → 自动策略落到 ReparseRaw，必须有 analyzer 输出才能播放。
    assert_eq!(loaded.effective_policy, ReplayPolicyView::ReparseRaw);
    assert_eq!(
        loaded.block_reason,
        Some(ReplayBlockReasonView::NeedAnalyzer)
    );
    assert!(!loaded.can_play, "缺少 analyzer 输出时必须阻断播放");
    assert_eq!(wb.replay_raw_serial_events().len(), 20);

    // 被门控的 ReplayPlay：返回 Done 但状态/cursor 都不推进（不假装播放成功）。
    expect_done(&mut wb, AppCommand::ReplayPlay);
    let blocked = wb.query_replay();
    assert_eq!(
        blocked.state,
        ReplayStateView::Loaded,
        "被 analyzer 门控时不得进入 Playing"
    );
    assert_eq!(blocked.cursor, 0, "被 analyzer 门控时不得推进 cursor");

    // 提供 analyzer 输出（空输出也是有效输出）后放行。
    wb.replay_set_analyzer_cache(Vec::new());
    let ready = wb.query_replay();
    assert!(ready.analyzer_cache_valid);
    assert!(ready.can_play, "analyzer 缓存有效后必须允许播放");
    assert!(ready.can_seek);

    // 32x 播放到自然结束，并验证事件真的发布到了 DataBus。
    expect_done(&mut wb, AppCommand::SetReplaySpeed { speed: 32.0 });
    assert_eq!(wb.query_replay().speed, 32.0);
    let replay_events = bus.subscribe(TopicFilter::prefix("transport.serial."));
    expect_done(&mut wb, AppCommand::ReplayPlay);
    assert_eq!(wb.query_replay().state, ReplayStateView::Playing);

    let mut published = 0_usize;
    let deadline = Instant::now() + Duration::from_secs(10);
    while wb.query_replay().state != ReplayStateView::Finished && Instant::now() < deadline {
        published += wb.tick_replay();
        std::thread::sleep(Duration::from_millis(2));
    }
    let finished = wb.query_replay();
    assert_eq!(
        finished.state,
        ReplayStateView::Finished,
        "播放必须自然结束"
    );
    assert_eq!(published, 20, "所有录制事件都应在播放过程中发布");
    assert_eq!(finished.cursor, 20);
    assert_eq!(finished.position_ms, finished.duration_ms);

    let emitted: Vec<Event> = replay_events
        .drain()
        .into_iter()
        .filter(|event| event.is_replay())
        .collect();
    assert_eq!(emitted.len(), 20, "回放事件必须真实发布到 DataBus");
    assert!(
        emitted
            .iter()
            .all(|event| event.topic == tool_transport::serial_topics::SERIAL_RX),
        "回放事件必须保留原始 topic"
    );
    assert!(
        emitted
            .iter()
            .all(|event| event.source == "replay:serial:COM_REPLAY"),
        "回放事件必须带 replay: 前缀与原始 source"
    );

    // Stop 回到 Loaded 并把位置归零。
    expect_done(&mut wb, AppCommand::ReplayStop);
    let stopped = wb.query_replay();
    assert_eq!(stopped.state, ReplayStateView::Loaded);
    assert_eq!(stopped.cursor, 0);
    assert_eq!(stopped.position_ms, 0);

    // 加载失败：任务落 Failed，且不得破坏已加载的回放状态。
    let missing = fixture.with_file_name(format!("missing-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&missing);
    let bad_task = expect_pending(
        &mut wb,
        AppCommand::LoadReplay {
            file: FileHandle::from_native_path(&missing),
        },
    );
    assert!(
        tick_until(&mut wb, Duration::from_secs(5), |wb| matches!(
            task_state(wb, bad_task),
            Some(TaskState::Completed | TaskState::Failed)
        )),
        "坏路径的 load_replay 任务必须结束"
    );
    assert_eq!(task_state(&wb, bad_task), Some(TaskState::Failed));
    let after_failure = wb.query_replay();
    assert_eq!(
        after_failure.state,
        ReplayStateView::Loaded,
        "加载失败不得清空已有回放数据"
    );
    assert_eq!(after_failure.total_events, 20);

    let _ = std::fs::remove_file(&fixture);
}

#[test]
fn plugin_enable_disable_transitions_state_and_rejects_unknown_ids() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus.clone());
    let plugin_logs = bus.subscribe(TopicFilter::exact(topics::LOG_SYSTEM));

    // 未发现的 id：必须返回 AppError::Plugin，且不影响 query/tick。
    let enable_error = wb
        .dispatch(AppCommand::EnablePlugin {
            plugin_id: "__no_such_plugin__".to_owned(),
        })
        .expect_err("启用未知插件必须报错");
    assert!(
        matches!(enable_error, AppError::Plugin(_)),
        "启用未知插件的错误类型应为 AppError::Plugin，实际: {enable_error:?}"
    );
    let disable_error = wb
        .dispatch(AppCommand::DisablePlugin {
            plugin_id: "__no_such_plugin__".to_owned(),
        })
        .expect_err("禁用未知插件必须报错");
    assert!(matches!(disable_error, AppError::Plugin(_)));
    assert!(wb.query_plugins().summaries.is_empty());
    assert!(!wb.plugin_ids().contains(&"__no_such_plugin__".to_owned()));
    assert!(
        !wb.query_enabled_plugin_ids()
            .contains(&"__no_such_plugin__".to_owned())
    );
    wb.tick(0.0);

    // 真实发现 → 启用 → 禁用 → 再启用（插件写在临时目录，不触碰仓库 plugins/）。
    let root = write_probe_plugin();
    let discover_task = expect_pending(
        &mut wb,
        AppCommand::DiscoverPlugins {
            roots: vec![root.clone()],
        },
    );
    assert_eq!(
        task_kind(&wb, discover_task).as_deref(),
        Some("discover_plugins")
    );
    assert!(
        tick_until(&mut wb, Duration::from_secs(10), |wb| matches!(
            task_state(wb, discover_task),
            Some(TaskState::Completed | TaskState::Failed)
        )),
        "discover_plugins 任务必须在超时前结束"
    );
    assert_eq!(task_state(&wb, discover_task), Some(TaskState::Completed));

    assert_eq!(
        wb.plugin_state("headless.probe"),
        Some(PluginStateView::Discovered)
    );
    assert!(
        wb.query_plugins()
            .summaries
            .iter()
            .any(|summary| summary.id == "headless.probe"
                && summary.state == PluginStateView::Discovered),
        "发现后 query_plugins() 必须暴露 Discovered 状态"
    );

    expect_done(
        &mut wb,
        AppCommand::EnablePlugin {
            plugin_id: "headless.probe".to_owned(),
        },
    );
    assert_eq!(
        wb.plugin_state("headless.probe"),
        Some(PluginStateView::Running),
        "启用后插件必须进入 Running"
    );

    // 探针插件的 main.lua 在 Lua 运行时里真实执行后才会产生这条日志，
    // 把"状态是 Running"升级为"脚本确实在 headless 环境跑起来了"。
    let mut script_started = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        wb.tick(0.0);
        if plugin_logs.drain().iter().any(|event| {
            event
                .payload
                .text_lossy()
                .contains("headless probe plugin started")
        }) {
            script_started = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        script_started,
        "探针插件的 Lua 脚本必须在 headless 环境下真实执行"
    );
    assert!(
        wb.query_plugins()
            .summaries
            .iter()
            .any(|summary| summary.id == "headless.probe"
                && summary.state == PluginStateView::Running),
        "query_plugins() 必须同步反映 Running"
    );

    expect_done(
        &mut wb,
        AppCommand::DisablePlugin {
            plugin_id: "headless.probe".to_owned(),
        },
    );
    assert_eq!(
        wb.plugin_state("headless.probe"),
        Some(PluginStateView::Disabled),
        "禁用后插件必须进入 Disabled"
    );

    // 停止是异步的：收割运行时之后必须可以再次启用。
    let mut reenabled = false;
    for _ in 0..200 {
        wb.process_plugin_lifecycle();
        if wb
            .dispatch(AppCommand::EnablePlugin {
                plugin_id: "headless.probe".to_owned(),
            })
            .is_ok()
        {
            reenabled = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(reenabled, "运行时停止后必须可以重新启用");
    assert_eq!(
        wb.plugin_state("headless.probe"),
        Some(PluginStateView::Running)
    );
    expect_done(
        &mut wb,
        AppCommand::DisablePlugin {
            plugin_id: "headless.probe".to_owned(),
        },
    );

    // 收尾：等待运行时退出，不留后台线程与临时目录。
    for _ in 0..200 {
        wb.process_plugin_lifecycle();
        if wb.plugin_state("headless.probe") == Some(PluginStateView::Disabled)
            && wb.take_plugin_cleanup_requests().is_empty()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn event_emission_reaches_bus_subscribers_from_all_publishers() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus.clone());

    // 1) publish_event 与 EventSink 必须发布到同一个 DataBus。
    let custom = bus.subscribe(TopicFilter::exact("headless.custom"));
    wb.publish_event(Event::new(
        "headless.custom",
        "direct",
        Direction::Internal,
        Payload::Text("one".to_owned()),
    ));
    wb.event_sink().publish(Event::new(
        "headless.custom",
        "sink",
        Direction::Internal,
        Payload::Text("two".to_owned()),
    ));
    let received = custom.drain();
    assert_eq!(received.len(), 2, "两个发布者都必须到达订阅者");
    assert_eq!(received[0].source, "direct");
    assert_eq!(received[1].source, "sink");
    assert_eq!(received[0].payload.text_lossy(), "one");

    // 2) log() 必须产生带 level metadata 的 log.system 事件。
    let logs = bus.subscribe(TopicFilter::exact(topics::LOG_SYSTEM));
    wb.log(LogLevel::Warn, "headless log line");
    let log_events = logs.drain();
    assert_eq!(log_events.len(), 1);
    assert_eq!(log_events[0].topic, topics::LOG_SYSTEM);
    assert_eq!(log_events[0].source, "app");
    assert_eq!(log_events[0].meta_str("level"), Some("warn"));
    assert_eq!(log_events[0].payload.text_lossy(), "headless log line");

    // 3) UI 订阅集合必须只暴露约定的三条事件流。
    let ui = wb.subscribe_ui_events();
    wb.publish_event(Event::new(
        topics::UI_FORM_FILE_BROWSE,
        "panel",
        Direction::Internal,
        Payload::Text("pick".to_owned()),
    ));
    for index in 0..2 {
        wb.publish_event(Event::new(
            topics::UI_CONTRIBUTION_SET_VALUE,
            "panel",
            Direction::Internal,
            Payload::Text(format!("value-{index}")),
        ));
    }
    wb.publish_event(Event::new(
        topics::UI_SET_STATUS,
        "panel",
        Direction::Internal,
        Payload::Text("ready".to_owned()),
    ));
    assert!(ui.try_file_browse().is_some(), "file_browse 必须可读");
    assert_eq!(ui.drain_contribution_set_value(10).len(), 2);
    assert_eq!(ui.drain_status(10).len(), 1);
    assert!(
        ui.drain_contribution_set_value(10).is_empty(),
        "drain 必须消费掉事件，不能重复返回"
    );

    // 4) ExecutePluginCommand 必须发布 plugin.command.execute 并补齐路由字段。
    let commands = bus.subscribe(TopicFilter::exact(topics::PLUGIN_COMMAND_EXECUTE));
    expect_done(
        &mut wb,
        AppCommand::ExecutePluginCommand {
            plugin_id: "demo.plugin".to_owned(),
            command_id: "demo.cmd".to_owned(),
            context: serde_json::json!({ "input": "AT" }),
        },
    );
    let executed = commands.drain();
    assert_eq!(executed.len(), 1);
    assert_eq!(executed[0].topic, topics::PLUGIN_COMMAND_EXECUTE);
    assert_eq!(executed[0].source, "plugin.command");
    assert_eq!(executed[0].direction, Direction::Internal);
    let Payload::Json(payload) = &executed[0].payload else {
        panic!("插件命令事件载荷必须是 JSON: {:?}", executed[0].payload);
    };
    assert_eq!(payload["plugin_id"], "demo.plugin");
    assert_eq!(payload["command"], "demo.cmd");
    assert_eq!(payload["origin"], "host.command");
    assert_eq!(payload["input"], "AT");

    // 5) 后台任务失败必须转成系统日志事件（TaskFailed → log.system）。
    let connect = wb.dispatch(AppCommand::Connect {
        port: PortId::new("COM_EMIT_MISSING_999"),
        settings: SerialSettings::default(),
    });
    assert!(matches!(connect, Ok(CommandOutcome::Pending { .. })));
    assert!(
        tick_until(&mut wb, Duration::from_secs(10), |wb| wb
            .task_snapshots()
            .iter()
            .any(
                |snapshot| snapshot.kind == "connect_serial" && snapshot.state == TaskState::Failed
            )),
        "无效串口连接必须落到 Failed"
    );
    let failure_logs = logs
        .drain()
        .into_iter()
        .filter(|event| event.payload.text_lossy().contains("后台任务失败"))
        .count();
    assert_eq!(failure_logs, 1, "任务失败必须发布 1 条系统日志事件");
}

// ── 发送链路字节级契约 ─────────────────────────────────────────────────────
//
// `send_routing_dispatches_by_port_kind_and_rejects_invalid_hex` 只断言「任务种类对了、
// 最后因为端口从未打开而 Failed」。把 `Workbench::send_transport_bytes` 的入参换成
// `let bytes: Vec<u8> = Vec::new();`（每一次发送都真的投递 0 字节）后，全工作区仍会
// 全绿 —— 用户按下发送键什么都不发，CI 看不见。下面的用例把契约钉在**对端收到的字节**上。

/// 收满期望帧数之后，回路服务器继续观察的静默窗口：让「多投递一帧」也能被看见。
const QUIET_WINDOW: Duration = Duration::from_millis(300);

/// 回路服务器观测到的一条事件。
enum Observed {
    /// 收到一条 WebSocket 文本帧（原文，未解析）。
    Frame(String),
    /// 观测线程停止，附带原因：让「没收到」变成断言消息，而不是挂死。
    Stopped(String),
}

/// 在 127.0.0.1 上起一个真实的 WebSocket 服务器（Nexus Prime / Moonraker 替身，
/// 端口 0 由内核分配），把客户端发来的每条文本帧原样转发给调用方。
///
/// 形状照抄 `crates/transport/src/network.rs` 的 `e2e_send_gcode_and_receive_response`：
/// 裸 `TcpListener` + `tungstenite::accept`，不经过 `reqwest`，因此与 `HTTP_PROXY`
/// 之类环境无关；也不猜空闲端口，不存在端口竞争。
///
/// 观测结果走 channel 而非 `join()`：调用方只按 deadline 等待，绝不阻塞在一个可能
/// 卡在 `accept()` 上的线程上。线程内部每次读都设了 socket 读超时，并用绝对 deadline
/// 收口，所以它自己一定会退出。
fn spawn_loopback_ws_server(expected_frames: usize) -> (SocketAddr, Receiver<Observed>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 loopback 监听端口");
    let addr = listener.local_addr().expect("读取内核分配的监听地址");
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        let stop = |reason: String| {
            let _ = tx.send(Observed::Stopped(reason));
        };
        // 握手阶段给足余量（客户端 TCP 连上后才发 GET Upgrade），随后收紧成轮询间隔，
        // 让 read() 能周期性返回以检查 deadline。
        let (stream, _) = match listener.accept() {
            Ok(peer) => peer,
            Err(error) => {
                stop(format!("accept 失败：{error}"));
                return;
            }
        };
        if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(5))) {
            stop(format!("设置握手读超时失败：{error}"));
            return;
        }
        let mut ws = match tungstenite::accept(stream) {
            Ok(ws) => ws,
            Err(error) => {
                stop(format!("WebSocket 握手失败：{error}"));
                return;
            }
        };
        if let Err(error) = ws
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(50)))
        {
            stop(format!("设置读帧超时失败：{error}"));
            return;
        }
        let hard_deadline = Instant::now() + Duration::from_secs(15);
        let mut quiet_deadline: Option<Instant> = None;
        let mut forwarded = 0_usize;
        loop {
            // 收满期望条数后再静默观察一会儿：只读到「期望的那几条」会让多投递一帧的
            // 缺陷隐形（重复派发正是 Task 6 统一路由时最容易引入的那一类）。
            if let Some(quiet) = quiet_deadline
                && Instant::now() >= quiet
            {
                break; // 观测完成，正常退出（丢弃 tx → 调用方看到 Disconnected）
            }
            if Instant::now() >= hard_deadline {
                if forwarded < expected_frames {
                    stop(format!(
                        "服务器 15s 内只等到 {forwarded} 帧（期望 {expected_frames}）"
                    ));
                }
                break;
            }
            match ws.read() {
                Ok(Message::Text(text)) => {
                    if tx.send(Observed::Frame(text.as_str().to_owned())).is_err() {
                        return; // 调用方已退出
                    }
                    forwarded += 1;
                    if forwarded >= expected_frames && quiet_deadline.is_none() {
                        quiet_deadline = Some(Instant::now() + QUIET_WINDOW);
                    }
                }
                Ok(Message::Close(_)) => {
                    stop("对端发送 Close 帧后连接结束".to_owned());
                    return;
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) => {}
                Err(error) => {
                    stop(format!("读帧失败：{error}"));
                    return;
                }
            }
        }
    });
    (addr, rx)
}

/// 在 `budget` 内把观测线程报告的帧全部收完，返回 `(帧原文, 提前停止的原因)`。
///
/// 结束方式只有三种，且每一种都有绝对截止：观测线程正常收口（`Disconnected`，
/// 返回 `None`）、观测线程给出原因、或 `budget` 到期。每轮 `recv_timeout` 都只用
/// 剩余预算，所以「没收到字节」一定是断言失败，不可能挂死。
fn drain_frames(rx: &Receiver<Observed>, budget: Duration) -> (Vec<String>, Option<String>) {
    let mut frames = Vec::new();
    let deadline = Instant::now() + budget;
    let stopped = loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break Some(format!(
                "{}s 内观测未结束，只收到 {} 帧",
                budget.as_secs(),
                frames.len()
            ));
        };
        match rx.recv_timeout(remaining) {
            Ok(Observed::Frame(text)) => frames.push(text),
            Ok(Observed::Stopped(reason)) => {
                break Some(format!("{reason}（已收到 {} 帧）", frames.len()));
            }
            Err(RecvTimeoutError::Timeout) => {
                break Some(format!(
                    "{}s 内没有新的帧（已收到 {} 帧）",
                    budget.as_secs(),
                    frames.len()
                ));
            }
            Err(RecvTimeoutError::Disconnected) => break None,
        }
    };
    (frames, stopped)
}

/// 解析一条 JSON-RPC 文本帧为 `(method, id, params.script)`。
///
/// 帧不是 JSON、`jsonrpc` 不是 `"2.0"`、或缺少 `method` 都当场 panic：下面的断言读的是
/// `method`/`id`/`params.script` 三个字段，只有先确认「这确实是一条 JSON-RPC 2.0 请求」，
/// 取不到字段时补默认值才不会把缺陷读成合法值。`params.script` 缺失同样落成空串：
/// gcode 请求缺它会被下面的按序字节断言判红，identify 请求本就无 script，由显式的
/// method/id/params 断言单独钉住。
fn json_rpc_frame(frame: &str) -> (String, u64, String) {
    let value: serde_json::Value = serde_json::from_str(frame)
        .unwrap_or_else(|error| panic!("对端帧不是合法 JSON：{error}，原文：{frame}"));
    assert_eq!(
        value
            .get("jsonrpc")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
        "2.0",
        "对端帧必须是 JSON-RPC 2.0，原文：{frame}"
    );
    let method = value
        .get("method")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("对端帧必须有 method 字段，原文：{frame}"))
        .to_owned();
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let script = value
        .pointer("/params/script")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    (method, id, script)
}

#[test]
fn send_bytes_reach_a_connected_network_port() {
    // 派发命令数 = identify(1) + 之后的 gcode 请求数。
    const SENT_COMMANDS: usize = 5;

    let (addr, observed) = spawn_loopback_ws_server(1 + SENT_COMMANDS);

    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);
    let config = NetworkSerialConfig {
        host: "127.0.0.1".to_owned(),
        port: addr.port(),
        api_key: None,
    };
    let name = config.display_name();
    expect_done(&mut wb, AppCommand::RegisterNetworkPort { config });

    // 1) 端口必须先**真的打开**：连的是活着的 loopback 服务器，不是死端口 9。
    let connect_task = expect_pending(
        &mut wb,
        AppCommand::Connect {
            port: PortId::new(name.clone()),
            settings: SerialSettings::default(),
        },
    );
    assert_eq!(
        task_kind(&wb, connect_task).as_deref(),
        Some("connect_network"),
        "已注册网络端口的连接不得落到 connect_serial"
    );
    let connect_ended = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        matches!(
            task_state(wb, connect_task),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        connect_ended,
        "connect_network 任务必须在 10s 内结束，实际: {:?}",
        task_state(&wb, connect_task)
    );
    assert_eq!(
        task_state(&wb, connect_task),
        Some(TaskState::Completed),
        "连上真实 loopback WebSocket 服务器必须成功，而不是静默降级成「端口没开」"
    );
    let handshaked = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        wb.query_transport().statuses.iter().any(|status| {
            status.port_name.as_deref() == Some(name.as_str()) && status.open && !status.connecting
        })
    });
    assert!(
        handshaked,
        "WebSocket 握手必须完成（transport 状态应为 open 且不在 connecting）：{:?}",
        wb.query_transport().statuses
    );

    // 2) 按序派发 5 条命令。per-port 有序队列（`spawn_ordered`）保证到帧顺序 == 派发顺序。
    let text_task = expect_pending(
        &mut wb,
        AppCommand::SendText {
            port: PortId::new(name.clone()),
            text: "G28\n".to_owned(),
        },
    );
    let raw_task = expect_pending(
        &mut wb,
        AppCommand::SendRaw {
            port: PortId::new(name.clone()),
            bytes: b"M105".to_vec(),
        },
    );
    // 解码后全是可打印 ASCII 的 HEX：script 字段与投递的字节逐字节相同，最直白的字节级证据。
    let ascii_hex_task = expect_pending(
        &mut wb,
        AppCommand::SendHex {
            port: PortId::new(name.clone()),
            hex: "48 69".to_owned(),
            strict: true,
        },
    );
    // HEX 解码契约的两条主角：严格 "AB CD" 必须投递解码后的 0xAB 0xCD；
    // 宽松 "AB C" 必须按生产解析器的单 nibble 规则投递 0xAB 0x0C。
    let strict_hex_task = expect_pending(
        &mut wb,
        AppCommand::SendHex {
            port: PortId::new(name.clone()),
            hex: "AB CD".to_owned(),
            strict: true,
        },
    );
    let lenient_hex_task = expect_pending(
        &mut wb,
        AppCommand::SendHex {
            port: PortId::new(name.clone()),
            hex: "AB C".to_owned(),
            strict: false,
        },
    );
    let sends = [
        text_task,
        raw_task,
        ascii_hex_task,
        strict_hex_task,
        lenient_hex_task,
    ];
    for task in sends {
        assert_eq!(
            task_kind(&wb, task).as_deref(),
            Some("send_network"),
            "已注册网络端口 {name} 的发送（task {task:?}）不得落到 send_serial"
        );
    }

    // 3) 先收对端的字节。`send_to` 是 fire-and-forget（写进端口命令队列就算成功），
    //    所以「任务 Completed」绝不等于「字节到了线上」—— 这个先后顺序本身就是用例的要点。
    //    同时，观测一定在 Workbench 仍被持有、端口仍打开时跑完：万一 loopback 连接受环境
    //    影响而失败，变红的只会是本用例的字节断言，不会牵连本进程里其他串口路径用例。
    let (frames, stopped) = drain_frames(&observed, Duration::from_secs(20));

    // 4) 任务侧也必须 Completed：端口开着还静默失败，就是路由/后端的缺陷。
    let sends_ended = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        sends.iter().all(|task| {
            matches!(
                task_state(wb, *task),
                Some(TaskState::Completed | TaskState::Failed)
            )
        })
    });
    assert!(
        sends_ended,
        "发送任务必须在 10s 内结束，实际: {:?}",
        sends.map(|task| task_state(&wb, task))
    );
    let states: Vec<Option<TaskState>> = sends.iter().map(|task| task_state(&wb, *task)).collect();
    assert_eq!(
        states,
        vec![Some(TaskState::Completed); sends.len()],
        "端口已打开时每一次发送都必须让任务 Completed，而不是静默失败"
    );

    // 5) 契约本体：对端**收到了什么**。收到几条就断言几条，多一条也算失败。
    assert_eq!(
        stopped, None,
        "回路服务器观测中断：{stopped:?}；已收到 {frames:?}"
    );
    assert_eq!(
        frames.len(),
        1 + SENT_COMMANDS,
        "对端应恰好收到 identify + {SENT_COMMANDS} 条 gcode 请求，实际 {} 帧: {frames:?}",
        frames.len()
    );
    let parsed: Vec<(String, u64, String)> =
        frames.iter().map(|frame| json_rpc_frame(frame)).collect();

    // identify 必须先于任何 gcode 请求，且占用 id 1（gcode 请求从 2 开始编号）。
    assert_eq!(
        parsed[0].0, "server.connection.identify",
        "第一条帧必须是客户端自我标识，实际: {:?}",
        parsed[0]
    );
    assert_eq!(parsed[0].1, 1, "identify 请求必须是 id 1");
    let identify_params: serde_json::Value =
        serde_json::from_str(&frames[0]).expect("json_rpc_frame 已确认是 JSON");
    assert!(
        identify_params.get("params").is_some(),
        "identify 请求必须带 params（客户端标识），原文：{}",
        frames[0]
    );
    let methods: Vec<&str> = parsed[1..]
        .iter()
        .map(|(method, _, _)| method.as_str())
        .collect();
    assert_eq!(
        methods,
        vec!["printer.gcode.script"; SENT_COMMANDS],
        "每条发送命令都必须以 printer.gcode.script 投递到对端"
    );
    let ids: Vec<u64> = parsed[1..].iter().map(|(_, id, _)| *id).collect();
    assert_eq!(
        ids,
        vec![2, 3, 4, 5, 6],
        "gcode 请求必须按派发顺序取得连续 id（identify 用掉 1）"
    );
    let scripts: Vec<&str> = parsed[1..]
        .iter()
        .map(|(_, _, script)| script.as_str())
        .collect();
    // transport 用 `String::from_utf8_lossy` 把原始字节渲染成 gcode 文本，非法 UTF-8
    // 字节逐个替换成 U+FFFD，所以「恰好两个 U+FFFD」就是 0xAB 0xCD 两字节在对端的投影。
    assert_eq!(
        scripts,
        vec![
            "G28\n",            // SendText：原文投递
            "M105",             // SendRaw：字节投递
            "Hi",               // 严格 HEX "48 69" → 0x48 0x69
            "\u{FFFD}\u{FFFD}", // 严格 HEX "AB CD" → 0xAB 0xCD（两字节，非 ASCII 原文）
            "\u{FFFD}\u{0C}",   // 宽松 HEX "AB C" → 0xAB 0x0C（单 nibble 左补 0）
        ],
        "对端按序收到的 script 必须与派发的 5 条命令逐字节对应：HEX 变成 \"48 69\"/\"AB CD\"/\"AB C\" \
         这类 ASCII 原文就说明没有解码；帧数变少或整条消失就说明投递了 0 字节；\
         第 5 条若不再是「U+FFFD + U+000C」就说明宽松模式的单 nibble 左补 0 规则被改掉\
         （右补 0 会得到 0xC0，即第二个 U+FFFD）"
    );
}
