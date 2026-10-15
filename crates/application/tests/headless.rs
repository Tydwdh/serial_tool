use std::io::Read;
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
    assert!(matches!(refresh, CommandOutcome::Pending { .. }));

    // ClearTerminal must not require egui.
    wb.dispatch(AppCommand::ClearTerminal).expect("clear");

    // Query APIs must be accessible.
    let _ = wb.query_transport();
    let _ = wb.query_recording();
    let _ = wb.query_replay();
    let _ = wb.query_plugins();

    // Invalid connect should return transport error, not panic.
    let connect = wb.dispatch(AppCommand::Connect {
        port: PortId::new("COM_NOT_EXIST_999"),
        settings: SerialSettings::default(),
    });
    assert!(matches!(connect, Ok(CommandOutcome::Pending { .. })));

    // Tick 必须能回收后台任务；无效连接最终应落到 Failed，而不是在 dispatch
    // 阶段阻塞或直接把硬件错误同步抛回 UI。
    //
    // 这里等的是「任务真的落到 Failed」这个终态本身，不是「tick 满 100 轮」：
    // 连接 worker 是后台线程，机器满载时它什么时候拿到 CPU 无人能猜（此前在
    // 64 个占满 CPU 的进程下 30 次红 27 次）。tick_until 每轮真的 tick + 查快照，
    // 条件一成立立即返回；30s 只是给饱和 CI 留余量的**真上界**，等不到就带着
    // 实际耗时和最后一轮看到的快照红掉，既不静默通过也不挂死。
    let started = Instant::now();
    let budget = Duration::from_secs(30);
    let reached_failed = tick_until(&mut wb, budget, |workbench| {
        workbench.task_snapshots().iter().any(|snapshot| {
            snapshot.kind == "connect_serial" && matches!(snapshot.state, TaskState::Failed)
        })
    });
    assert!(
        reached_failed,
        "无效串口连接应在 {budget:?} 内落到 Failed，实际等了 {:?}，最后一轮看到 {:?}",
        started.elapsed(),
        wb.task_snapshots()
            .iter()
            .map(|snapshot| (snapshot.kind.clone(), snapshot.state))
            .collect::<Vec<_>>(),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Headless 契约测试（docs/ARCHITECTURE.md「剩余工作」3 / todo.txt §39）
//
// 四类行为：send routing / replay lifecycle / plugin enable-disable / event emission
//
// 全部用例只经由 Workbench + AppCommand + query_* 的真实 API 驱动：不启动 egui，
// 不打开真实串口，不访问仓库 plugins/ 目录，不发网络请求。断言目标是真实状态
// 变化（任务 kind / 回放状态机 / 插件状态机 / DataBus 事件），不是"调用不报错"。
// 终端的保留条数与按 seq 淘汰最旧条目不在这里测：那份读取模型（`query_terminal_since`）
// 第 46 轮证实零生产使用者后已删除，活路径的上限行为由 `tool-panels` 的
// `max_entries_is_global_across_ports_and_keeps_newest_events` /
// `lowering_max_entries_trims_immediately` 与 `TerminalStore` 单测守着。
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

/// 读取任务快照对用户说的话：成功/进行中的文案之外，**失败时这里就是错误原文**，
/// 所以它才是「用户到底看到了哪句话」的观测点。
fn task_message(wb: &Workbench, task_id: TaskId) -> Option<String> {
    wb.task_snapshots()
        .into_iter()
        .find(|snapshot| snapshot.id == task_id)
        .map(|snapshot| snapshot.message)
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
/// `dir` 只决定临时目录名：用例之间各用一份，免得一方收尾 `remove_dir_all`
/// 把另一方正在读的脚本删掉。插件 ID 固定为 `headless.probe`。
///
/// `ctx.bus.on` 是必须的：没有 callback/command/timer/task 的脚本执行完，运行时
/// 会立即退出（lua_host「插件已完成（无回调）」），启用后状态马上会变 Finished。
fn write_probe_plugin(dir: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "serial-tool-headless-plugin-{dir}-{}",
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
    // 这句话是 `docs/ARCHITECTURE.md`「发送路径的残留差异 → 错误文案」一栏作为证据发布的
    // 原文，所以在这里钉住它：改 `SendPlanError::Display`、改 `AppError::Transport` 的
    // `传输错误：` 分类前缀（第 24 轮之前是英文 `transport: `）、或改 `tool_core` 严格模式的
    // 文案，都会让本断言变红，而不是留下一份与代码无关的文档句子。
    assert_eq!(
        strict_error.to_string(),
        "传输错误：HEX 解析失败：严格模式: \"C\" 规范化后为 1 个字符，必须恰为 2（偶数 hex 长度），请补0或关闭严格模式",
        "native 非法 HEX 的渲染句串与 docs/ARCHITECTURE.md 发布的证据不一致（文档在说谎）"
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
fn network_port_ids_are_recognized_in_both_naming_forms() {
    // `plan_send` 的 `is_network` 实参在 native 侧由 `Workbench::is_network_port` 算出。
    // native 的 `RemoveNetworkPort` 一直同时接受 `display_name()`（`host:port`）与
    // `port_id()`（`network://host:port`）两种形态，而 `is_network_port` 此前只认前者 ——
    // 于是同一个 id 删得掉、发的时候却被静默降级成 `send_serial`（字节交给串口后端）。
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);
    let config = NetworkSerialConfig {
        host: "127.0.0.1".to_owned(),
        port: 9,
        api_key: None,
    };
    let display = config.display_name();
    let network_id = config.port_id().to_string();
    expect_done(&mut wb, AppCommand::RegisterNetworkPort { config });

    let display_task = expect_pending(
        &mut wb,
        AppCommand::SendText {
            port: PortId::new(display.clone()),
            text: "ping".to_owned(),
        },
    );
    let network_id_task = expect_pending(
        &mut wb,
        AppCommand::SendText {
            port: PortId::new(network_id.clone()),
            text: "ping".to_owned(),
        },
    );
    let serial_task = expect_pending(
        &mut wb,
        AppCommand::SendText {
            port: PortId::new("COM_NOT_A_NETWORK_PORT"),
            text: "ping".to_owned(),
        },
    );

    assert_eq!(
        task_kind(&wb, display_task).as_deref(),
        Some("send_network"),
        "display_name 形态（{display}）的发送必须走 send_network"
    );
    assert_eq!(
        task_kind(&wb, network_id_task).as_deref(),
        Some("send_network"),
        "port_id 形态（{network_id}）的发送必须走 send_network：同一个 id 不能删得掉却发不出去"
    );
    assert_eq!(
        task_kind(&wb, serial_task).as_deref(),
        Some("send_serial"),
        "普通串口名不得被 `network://` 形态的匹配误判成网络端口（误判=字节交给网络后端）"
    );

    // 三个端口都没打开（`network://…` 形态在 native transport 里连端口表项都不存在），
    // 所以任务必须全部以 Failed 收尾：这条路径不会静默"发送成功"。
    let sends = [display_task, network_id_task, serial_task];
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
    let root = write_probe_plugin("lifecycle");
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
    //
    // 这个预算循环此前走完就直接 `remove_dir_all`：`break` 有条件，但没人对结果负责，
    // 后台收割没完成时用例照样绿。补上同文件姊妹形状（`reenabled` / `window_closed`）
    // 那条带消息的 `assert!`，让「200×5ms 用完」成为一次响亮的红。
    // 200 这个预算有依据（`target/r56-budget.py` 打点量的实际迭代数）：这条循环空闲与
    // cpu×4、churn×6 负载下都是**第 1 次迭代就满足**，离用满还差着 200 倍 —— 也就是说
    // 它红的时候几乎只会是"真没收割干净"，不是"机器慢"。
    let mut harvested = false;
    for _ in 0..200 {
        wb.process_plugin_lifecycle();
        if wb.plugin_state("headless.probe") == Some(PluginStateView::Disabled)
            && wb.take_plugin_cleanup_requests().is_empty()
        {
            harvested = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        harvested,
        "插件运行时必须在 200×5ms 内被收割干净（状态回到 Disabled 且清理请求取空），\
         收尾时实际停在 state={:?}、剩余清理请求 {:?} —— 没收割完不许静默通过",
        wb.plugin_state("headless.probe"),
        wb.take_plugin_cleanup_requests()
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// 到用户眼前的插件错误必须是中文（第 22 轮：Enable/Disable 的派发与
/// `discover_plugins` / `reload_plugins` 两个后台任务都改走 `tool_extension::translate_error`）。
///
/// 逐字钉的是**没有外来细节**的那格（`NotFound`：整句都是我们自己写的，而它正是桌面用户
/// 点「启用」后状态栏读到的东西）。带 OS 原文的两格只钉中文框架：框架是契约，那半句细节
/// 跟随机器与系统语言（本机实测 `"read_dir"` 失败给的是「目录名称无效。 (os error 267)」），
/// 钉死它会让本用例变成一台机器的快照 —— 与 `crates/transport` 的 `Io` 臂同一取舍。
///
/// 第 24 轮之后这里也钉 `AppError` 那句的完整形状：`plugin: ` 前缀换成中文 `插件错误：` 之后，
/// 桌面状态栏内插的那半句（`ui/tiles.rs` 把它接在 `启用插件 {id} 失败：` 之后）已无英文，
/// 那半句由本用例第一格逐字钉住。
///
/// 本文件**没有**驱动的三格，如实记下原因（逐字中文都在 `crates/extension/tests/error_wording.rs`）：
/// - `AlreadyEnabled` / `Stopping` 要一个真在跑的 Lua 探针，其行为已由本文件另外两条用例
///   （状态迁移、停机窗口谓词）覆盖，再为文案起一次运行时不划算；
/// - `UnsupportedRuntime` 的输入进不来：`scan_roots` 先跑 `manifest.validate()`，
///   而非 `lua` 的 runtime 在登记成 record 之前就被折成诊断了；
/// - `Lua` 臂当前根本没有生产者（`run_plugin` 一律返回 `Ok`，脚本错误走运行时状态与诊断）。
#[test]
fn plugin_failures_reach_the_user_in_chinese() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);
    let missing = "__no_such_plugin__";

    // ① 启用 / 禁用一个从未发现的插件：两格都是整句中文，逐字钉。
    let enable_error = wb
        .dispatch(AppCommand::EnablePlugin {
            plugin_id: missing.to_owned(),
        })
        .expect_err("启用未知插件必须报错");
    let AppError::Plugin(enable_words) = &enable_error else {
        panic!("启用未知插件的错误类型应为 AppError::Plugin，实际: {enable_error:?}");
    };
    assert_eq!(
        enable_words.as_str(),
        "未找到插件 __no_such_plugin__（请先重新扫描插件目录）"
    );
    // 整句（分类前缀 + 正文）：这一行就是 `ui/tiles.rs` 接在「启用插件 X 失败：」后面
    // 交给状态栏的东西，前缀在第 24 轮之前是英文 `plugin: `。
    assert_eq!(
        enable_error.to_string(),
        "插件错误：未找到插件 __no_such_plugin__（请先重新扫描插件目录）"
    );

    let disable_error = wb
        .dispatch(AppCommand::DisablePlugin {
            plugin_id: missing.to_owned(),
        })
        .expect_err("禁用未知插件必须报错");
    let AppError::Plugin(disable_words) = &disable_error else {
        panic!("禁用未知插件的错误类型应为 AppError::Plugin，实际: {disable_error:?}");
    };
    assert_eq!(
        disable_words.as_str(),
        "未找到插件 __no_such_plugin__（请先重新扫描插件目录）"
    );

    // ② 插件目录在、`main.lua` 不在：启用时读脚本失败 → `Io` 臂。
    //    扫描只看 manifest，所以这一步本身必须成功，失败点才有得测。
    let root = write_probe_plugin("error-wording");
    std::fs::remove_file(root.join("headless.probe").join("main.lua")).expect("删掉探针脚本");
    let discovered = expect_pending(
        &mut wb,
        AppCommand::DiscoverPlugins {
            roots: vec![root.clone()],
        },
    );
    assert!(
        tick_until(&mut wb, Duration::from_secs(10), |wb| matches!(
            task_state(wb, discovered),
            Some(TaskState::Completed | TaskState::Failed)
        )),
        "discover_plugins 任务必须在超时前结束"
    );
    assert_eq!(task_state(&wb, discovered), Some(TaskState::Completed));

    let script_missing = wb
        .dispatch(AppCommand::EnablePlugin {
            plugin_id: "headless.probe".to_owned(),
        })
        .expect_err("缺 main.lua 的插件必须启用失败");
    let AppError::Plugin(script_words) = &script_missing else {
        panic!("读脚本失败应为 AppError::Plugin，实际: {script_missing:?}");
    };
    assert!(
        script_words.starts_with("插件文件访问失败："),
        "启用失败的整句必须是中文框架，实际 {script_words:?}"
    );
    assert!(
        !script_words.contains("io error:"),
        "英文 Display 框架又漏进了用户可见文案：{script_words:?}"
    );

    // ③ 扫描根是个普通文件：`discover_plugins` 的失败消息经任务快照直达状态栏。
    let file_as_root = root.join("not-a-directory");
    std::fs::write(&file_as_root, "这不是目录").expect("写入占位文件");
    let failed_discover = expect_pending(
        &mut wb,
        AppCommand::DiscoverPlugins {
            roots: vec![file_as_root],
        },
    );
    assert!(
        tick_until(&mut wb, Duration::from_secs(10), |wb| matches!(
            task_state(wb, failed_discover),
            Some(TaskState::Completed | TaskState::Failed)
        )),
        "discover_plugins 任务必须在超时前结束"
    );
    assert_eq!(task_state(&wb, failed_discover), Some(TaskState::Failed));
    let discover_words =
        task_message(&wb, failed_discover).expect("失败任务必须留下一句给用户的话");
    assert!(
        discover_words.starts_with("插件文件访问失败："),
        "discover_plugins 的失败消息必须是中文框架，实际 {discover_words:?}"
    );
    assert!(
        !discover_words.contains("io error:"),
        "discover_plugins 的失败消息里漏进了英文 Display 框架：{discover_words:?}"
    );

    // ④ 记在应用侧的根在两次扫描之间被换成普通文件：`reload_plugins` 是另一处 wire，
    //    它的根来自 `PluginManager::roots()` 而不是命令参数，所以必须单独钉一次。
    let recorded = expect_pending(
        &mut wb,
        AppCommand::DiscoverPlugins {
            roots: vec![root.clone()],
        },
    );
    assert!(
        tick_until(&mut wb, Duration::from_secs(10), |wb| matches!(
            task_state(wb, recorded),
            Some(TaskState::Completed | TaskState::Failed)
        )),
        "discover_plugins 任务必须在超时前结束"
    );
    assert_eq!(task_state(&wb, recorded), Some(TaskState::Completed));
    std::fs::remove_dir_all(&root).expect("收回插件根目录");
    std::fs::write(&root, "根目录已不是目录").expect("把插件根换成普通文件");

    let reload = expect_pending(&mut wb, AppCommand::ReloadPlugins);
    assert_eq!(task_kind(&wb, reload).as_deref(), Some("reload_plugins"));
    assert!(
        tick_until(&mut wb, Duration::from_secs(10), |wb| matches!(
            task_state(wb, reload),
            Some(TaskState::Completed | TaskState::Failed)
        )),
        "reload_plugins 任务必须在超时前结束"
    );
    assert_eq!(task_state(&wb, reload), Some(TaskState::Failed));
    let reload_words = task_message(&wb, reload).expect("失败任务必须留下一句给用户的话");
    assert!(
        reload_words.starts_with("插件文件访问失败："),
        "reload_plugins 的失败消息必须是中文框架，实际 {reload_words:?}"
    );
    assert!(
        !reload_words.contains("io error:"),
        "reload_plugins 的失败消息里漏进了英文 Display 框架：{reload_words:?}"
    );

    // 收尾：根目录已被换成普通文件，不留后台文件。
    let _ = std::fs::remove_file(&root);
}

/// 导出落盘失败到状态栏的那句话必须是中文（第 25 轮把 `write_exported_content` 从
/// `StorageError` 的英文 `Display` 改到 `tool_platform::storage::translate_error`）。
///
/// 载荷本身（`std::io::Error` 的原文）随机器与 locale 变，所以钉框架、不钉 OS 那半；
/// 但**框架只许出现一次**：载荷里若再混进一层中文框架，就是「存储操作失败：存储操作失败：…」。
#[test]
fn file_export_failures_reach_the_user_in_chinese() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);

    // 写不进去的目标：父目录不存在 —— 与用户「选了个已被移走的目录」是同一类失败。
    let dir = std::env::temp_dir().join(format!(
        "serial-tool-headless-export-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let target = dir.join("no-such-parent").join("terminal.csv");

    // `spawn_file_export` 由 presentation 直接调用（不经过 `dispatch`），所以这里拿到的
    // 就是 `CommandOutcome`，`expect_pending` 那条 helper 包不上；同一判定内联一份。
    let task = match wb
        .spawn_file_export(
            "export_terminal",
            "csv".to_owned(),
            FileHandle::from_native_path(&target),
            || Ok("列1,列2\n".to_owned()),
        )
        .expect("导出命令必须被接受")
    {
        CommandOutcome::Pending { task_id, .. } => task_id,
        CommandOutcome::Done => panic!("导出必须返回 Pending，而不是同步完成"),
    };
    assert_eq!(
        task_kind(&wb, task).as_deref(),
        Some("export_terminal"),
        "导出任务必须带 presentation 传进来的 kind"
    );
    assert!(
        tick_until(&mut wb, Duration::from_secs(10), |wb| matches!(
            task_state(wb, task),
            Some(TaskState::Completed | TaskState::Failed)
        )),
        "export_terminal 任务必须在超时前结束"
    );
    assert_eq!(
        task_state(&wb, task),
        Some(TaskState::Failed),
        "写到不存在的父目录必须失败"
    );

    let words = task_message(&wb, task).expect("失败任务必须留下一句给用户的话");
    assert!(
        words.starts_with("存储操作失败："),
        "导出失败的正文必须是中文框架，实际 {words:?}"
    );
    assert_eq!(
        words.matches("存储操作失败：").count(),
        1,
        "中文框架叠了两层：{words:?}"
    );
    assert!(
        !words.contains("storage operation failed"),
        "英文 Display 又漏进了用户可见文案：{words:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 回放文件**打不开**（区别于「不存在」）时，用户读到的那半句必须是中文框架。
///
/// 为什么用一个"能 stat、打不开"的路径而不是不存在的路径：`prepare_load` 第一步就
/// `metadata`，路径不存在时那里先红（那句中文是第 24 轮之前就有的
/// `获取文件大小 <路径> 失败：<OS 细节>`），永远走不到 `File::open`。
/// 这种路径由 `tool_testing::UnopenablePath` 造（Windows 用目录；Unix 用 Unix 域套接字，
/// 因为**目录在 Linux 上是可以读打开的**，第 43 轮那版"目录就是打不开"的夹具在 Linux 上
/// 直接死在测试前提上）。
///
/// 第 43 轮起框架里点名**是哪个路径**（用户在回放面板手输的就是它），所以本用例钉
/// 「框架在最前 + 整句正好等于框架与 OS 细节的拼接」—— 后者同时钉住了路径、原样带出的
/// 细节、以及**没有叠第二层框架**。kind 不在这条链上（派发侧只做 `to_string()`），
/// 它由 `crates/recorder/src/replay.rs` 的
/// `native_load_exits_keep_the_kind_of_the_underlying_os_error` 钉。
#[test]
fn replay_file_that_cannot_be_opened_reaches_the_user_in_chinese() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);

    let unopenable = tool_testing::UnopenablePath::new();
    let target = unopenable.path();
    let raw = std::fs::File::open(target)
        .expect_err("测试前提：这个路径必须打不开，否则本用例没有在测「打开失败」");

    let task = expect_pending(
        &mut wb,
        AppCommand::LoadReplay {
            file: FileHandle::from_native_path(target),
        },
    );
    assert_eq!(task_kind(&wb, task).as_deref(), Some("load_replay"));
    assert!(
        tick_until(&mut wb, Duration::from_secs(10), |wb| matches!(
            task_state(wb, task),
            Some(TaskState::Completed | TaskState::Failed)
        )),
        "load_replay 任务必须在超时前结束"
    );
    assert_eq!(
        task_state(&wb, task),
        Some(TaskState::Failed),
        "实际句串: {:?}",
        task_message(&wb, task)
    );

    let words = task_message(&wb, task).expect("失败任务必须留下一句给用户的话");
    assert!(
        words.starts_with("打开回放文件 "),
        "打开回放文件失败的正文必须是中文框架，实际 {words:?}"
    );
    assert_eq!(
        words,
        format!("打开回放文件 {} 失败：{raw}", target.display()),
        "框架之外还多了别的话（双重框架），或者路径/OS 细节被改写了"
    );
}

/// 重启交接的判定必须来自权威状态（PluginManager 的 stopping 表），不是错误文案。
///
/// 覆盖三个方向，任一处判定「说谎」都会红：
/// ① Running 期间为 false——否则 `always true` 的判定会把重启永远挂在队列上；
/// ② disable 后、线程退出前为 true，且 enable 在同窗口被拒——这就是 UI 每帧
///    重投重启请求所依据的答案；被拒也不得吞掉停机状态；
/// ③ 收割到线程真正退出后翻回 false，enable 重新可用。
///
/// 全程只走 Workbench API：不跑 egui，也不断言任何错误文本（本用例存在的意义
/// 就是「换了文案也不该坏」）。
#[test]
fn plugin_restart_handoff_relies_on_the_stopping_predicate() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus.clone());
    let plugin_logs = bus.subscribe(TopicFilter::exact(topics::LOG_SYSTEM));
    let probe = "headless.probe";

    let root = write_probe_plugin("restart");
    let discover_task = expect_pending(
        &mut wb,
        AppCommand::DiscoverPlugins {
            roots: vec![root.clone()],
        },
    );
    assert!(
        tick_until(&mut wb, Duration::from_secs(10), |wb| matches!(
            task_state(wb, discover_task),
            Some(TaskState::Completed | TaskState::Failed)
        )),
        "discover_plugins 任务必须在超时前结束"
    );
    expect_done(
        &mut wb,
        AppCommand::EnablePlugin {
            plugin_id: probe.to_owned(),
        },
    );

    // 与邻居用例同一判据：等到脚本真的在 Lua 运行时里跑起来。
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut script_started = false;
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
    assert!(script_started, "探针插件必须真实运行起来才有停机窗口可测");
    assert!(
        !wb.plugin_is_stopping(probe),
        "运行中的插件不在停机窗口里（否则重启请求会被无限挂起）"
    );

    expect_done(
        &mut wb,
        AppCommand::DisablePlugin {
            plugin_id: probe.to_owned(),
        },
    );

    // disable 只设 stop 标记、不 join：Lua 线程还在自己那一片（最多 50ms）事件里退场，
    // 所以这里观察到的是真实窗口，不是采样运气。
    assert!(
        wb.plugin_is_stopping(probe),
        "禁用后、线程退出前 plugin_is_stopping 必须为 true，重启请求才能被留到下一帧"
    );
    assert!(
        wb.dispatch(AppCommand::EnablePlugin {
            plugin_id: probe.to_owned(),
        })
        .is_err(),
        "仍在关闭时 enable 必须失败——它与 plugin_is_stopping 共用同一条判定"
    );
    assert!(
        wb.plugin_is_stopping(probe),
        "被拒的 enable 不得吞掉停机窗口：判定必须仍然为 true"
    );

    // 收割到线程真正退出：判定翻回 false，重启这才被派发。
    let mut window_closed = false;
    for _ in 0..200 {
        wb.process_plugin_lifecycle();
        if !wb.plugin_is_stopping(probe) {
            window_closed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        window_closed,
        "运行时退出后 plugin_is_stopping 必须翻回 false，否则重启永远等不完"
    );
    expect_done(
        &mut wb,
        AppCommand::EnablePlugin {
            plugin_id: probe.to_owned(),
        },
    );
    assert_eq!(
        wb.plugin_state(probe),
        Some(PluginStateView::Running),
        "窗口结束后必须能重新启用"
    );
    assert!(!wb.plugin_is_stopping(probe));

    // 收尾：不留后台线程与临时目录。
    expect_done(
        &mut wb,
        AppCommand::DisablePlugin {
            plugin_id: probe.to_owned(),
        },
    );
    // 同上：`break` 有条件但此前没人对结果负责，停机窗口没关掉时用例照样绿。
    // 这条比上面那条吃负载（要等停机窗口自己关掉）：实测一次通过用掉的迭代数是
    // 空闲 2、cpu×4 最坏 13、churn×6 最坏 34（`target/r56-budget.py`）⇒ 200 还有约 6 倍余量，
    // 真逼近预算时该改的是负载口径而不是悄悄抬数字。
    let mut harvested = false;
    for _ in 0..200 {
        wb.process_plugin_lifecycle();
        if !wb.plugin_is_stopping(probe) && wb.take_plugin_cleanup_requests().is_empty() {
            harvested = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        harvested,
        "停机窗口必须在 200×5ms 内收完（`plugin_is_stopping` 翻回 false 且清理请求取空），\
         否则本用例留下一个没退场的运行时线程；收尾时插件停在 state={:?}、剩余清理请求 {:?} \
         —— 没收割完不许静默通过",
        wb.plugin_state(probe),
        wb.take_plugin_cleanup_requests()
    );
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
//
// Task 6 之后，字节的唯一生产点是 `crate::send_plan::plan_send`（native 与 wasm 共用），
// 投递入参由它给出的 `PlannedSend` 承担：上述变异现在注入 `plan_send` 的 `bytes`
// （或 `send_transport_bytes` 解构后的同名变量）都必须让下面的用例变红。

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
/// 卡在 `accept()` 上的线程上。
///
/// 反过来，**线程自己不保证会退出**：`listener.accept()` 排在读超时与绝对 deadline
/// **之前**（两者都在它下面几行才建立），所以客户端始终没连上时，它会 parked 在
/// `accept()` 里直到进程结束，并占着这个内核分配的端口。刻意不改成
/// `set_nonblocking(true)` + 轮询到 deadline：走到那一步说明用例已经因为"对端没连上"
/// 而红了，多一个 parked 线程既不影响任何断言，也不影响其它用例（端口是 `:0` 由内核
/// 分配，不存在竞争），而轮询循环会给这个"字节级契约"的唯一观测点再加一层时序。
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

/// 周期发送的投递入口（`TransportEndpoint::send`，native 后台线程唯一用的那个）必须与
/// 一次性发送同一份判定：`send_plan::plan_send`。
///
/// 分叉前的实际差异（本用例逐条钉住）：
/// - 多行 HEX 在周期线程里是**按行各投一帧**（`input.lines()` 逐行解析），而同一段输入
///   按一次「发送」按钮只投一帧 —— 一个 4 字节包被拆成两个 2 字节包，对端固件看到的
///   帧边界不同。
/// - 非法 HEX 的文案是 `translate_error` 的「无效HEX：…」，与 `plan_send` 的
///   「HEX 解析失败：…」是两套措辞，用户在两侧、两条路径看到的不是同一句话。
/// - 纯空白文本被 `input.trim().is_empty()` 静默当成「发送成功」，而 web 会把
///   「文本 + 换行后缀」真的发出去。
#[test]
fn periodic_send_endpoint_uses_the_shared_send_plan() {
    // identify + 两条合法发送；非法 HEX 那条必须一帧都不产生。
    let (addr, observed) = spawn_loopback_ws_server(3);

    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);
    let config = NetworkSerialConfig {
        host: "127.0.0.1".to_owned(),
        port: addr.port(),
        api_key: None,
    };
    let name = config.display_name();
    expect_done(&mut wb, AppCommand::RegisterNetworkPort { config });
    let connect_task = expect_pending(
        &mut wb,
        AppCommand::Connect {
            port: PortId::new(name.clone()),
            settings: SerialSettings::default(),
        },
    );
    let connected = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        task_state(wb, connect_task) == Some(TaskState::Completed)
    });
    assert!(
        connected,
        "必须连上 loopback 服务器才有可观测的帧：{:?}",
        task_state(&wb, connect_task)
    );
    let handshaked = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        wb.query_transport().statuses.iter().any(|status| {
            status.port_name.as_deref() == Some(name.as_str()) && status.open && !status.connecting
        })
    });
    assert!(
        handshaked,
        "WebSocket 握手必须完成，否则本用例的帧断言全部退化成零断言：{:?}",
        wb.query_transport().statuses
    );

    let endpoint = wb.transport_endpoint();

    // 1) 非法 HEX：报错文案来自共用的 `SendPlanError::Display`，且一帧都不许上线。
    let invalid = endpoint
        .send(&name, "AB ZZ", true, "", false)
        .expect_err("非法 HEX 必须被拒绝");
    assert!(
        invalid.starts_with("HEX 解析失败："),
        "周期发送的 HEX 错误必须与一次性发送同一句（{invalid:?}）"
    );

    // 2) 文本：换行后缀由共用的命令构造附加，与 `do_send`/共享 sender 同一处。
    endpoint
        .send(&name, "AT", false, "\r\n", false)
        .expect("文本发送必须投递");

    // 3) 多行 HEX：整段是一次 plan，投一帧四字节。四个 0xAB 各自是独立的非法起始字节，
    //    所以 lossy 渲染恰好四个 U+FFFD；写成 "AB CD" 会得到 U+FFFD + U+036B + U+FFFD，
    //    因为 `CD AB` 是一段**合法**的两字节序列（`AB CD` 两字节也只出两个 U+FFFD）。
    endpoint
        .send(&name, "AB AB\nAB AB", true, "", true)
        .expect("严格模式多行 HEX 必须投递");

    let (frames, stopped) = drain_frames(&observed, Duration::from_secs(20));
    assert_eq!(
        stopped, None,
        "回路服务器观测中断：{stopped:?}；已收到 {frames:?}"
    );
    let parsed: Vec<(String, u64, String)> =
        frames.iter().map(|frame| json_rpc_frame(frame)).collect();
    let scripts: Vec<&str> = parsed
        .iter()
        .filter(|(method, _, _)| method != "server.connection.identify")
        .map(|(_, _, script)| script.as_str())
        .collect();
    assert_eq!(
        scripts,
        vec!["AT\r\n", "\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}"],
        "对端必须恰好收到两条发送：多行 HEX 分成两帧即「周期发送仍在按行拆分」，\
         纯空白输入多出一帧即「静默 no-op 被改成真发送」"
    );
}

/// 端口从未打开时，两条投递入口都必须说中文，且说的是同一句话。
///
/// 收敛前的实测状态：桌面端只有周期发送（`TransportEndpoint::send`）过了
/// `tool_transport::translate_error`；点一次「发送」走平台后端，`native.rs::operation_error`
/// 当时直接 `TransportError::to_string()`，**变体在能被翻译之前就被压成英文原文**，
/// 于是同一个未打开的端口在两条路上得到两种语言。
/// 那一轮只把正文换成中文，平台层的外框（`transport operation failed: `，即
/// `TransportError` 的 `Display`）还留在用户眼前；现在连外框也没了 ——
/// `Workbench::send_transport_bytes` 的串口分支改过 `tool_platform::translate_error`，
/// `Operation` 按契约原样返回，于是两条入口给出的是**逐字相同的一句**。
#[test]
fn sending_to_a_port_that_was_never_opened_reads_in_chinese_on_both_entry_points() {
    const PORT: &str = "COM_HEADLESS_CLOSED";
    const CLOSED_PORT_WORDS: &str = "串口 COM_HEADLESS_CLOSED 未打开（可能已断开，请重连）";

    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);

    // ① 一次性发送：走 `NativeTransportBackend::send` → `operation_error`。
    let task = expect_pending(
        &mut wb,
        AppCommand::SendText {
            port: PortId::new(PORT),
            text: "AT".to_owned(),
        },
    );
    assert_eq!(
        task_kind(&wb, task).as_deref(),
        Some("send_serial"),
        "未注册成网络端口的名字必须走串口后端，否则本用例测的不是 `operation_error`"
    );
    let ended = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        matches!(
            task_state(wb, task),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        ended,
        "发送任务必须在 10s 内结束，实际: {:?}",
        task_state(&wb, task)
    );
    assert_eq!(
        task_state(&wb, task),
        Some(TaskState::Failed),
        "端口从未打开，发送不得假装成功"
    );
    assert_eq!(
        task_message(&wb, task).as_deref(),
        Some(CLOSED_PORT_WORDS),
        "任务失败句串变了：正文不再是周期发送那句中文，或平台 `Display` 的英文外框又回来了"
    );

    // ② 周期发送：同一个失败，同一句中文（少了平台层的英文外框）。
    let endpoint = wb.transport_endpoint();
    assert_eq!(
        endpoint
            .send(PORT, "AT", false, "\r\n", false)
            .expect_err("端口没开，周期发送必须报错而不是静默成功"),
        CLOSED_PORT_WORDS,
        "两条入口的正文从这里起必须逐字相同 —— 它就是 `operation_error` 塞进 \
         `TransportError::Operation` 的那个载荷"
    );
}

// ── 手动重连的一次性尝试（`Workbench::start_reconnect`）─────────────────────
//
// 这里原先守着一段「3 秒预算 + 10ms 轮询」的循环，判据是 `Io(WouldBlock)`（"端口还在关闭中"）。
// 那条判据**没有任何生产者**（串口的 kind 在 `platform/src/native.rs::operation_error` 就被
// 折成字符串；`open_network_serial` 连接失败时也照旧返回 `Ok`），循环因此恒等于"一次 open"，
// 只是把失败结果压在手里最多 3 秒。判定与循环一起删掉了 —— 要重试的是**自动重连**那套
// （`crates/app/src/reconnect.rs`：退避 + 上限 + 可取消）；手动重连失败立刻把中文原因交给用户。
//
// 下面三条钉的是这次尝试的公开面：立刻放弃、文案中文化、网络端口走网络分支。

/// 打不开的口必须**立刻**放弃：这一次尝试就是全部，没有第二轮。
///
/// 把串口分支的 `return Err(...)` 改成重试（或给回那段 3 秒预算），这条用例会在 2.5s 时
/// 看到任务仍然 Running 而变红 —— 用户侧的症状就是「重连一个不存在的口也要转 3 秒」。
#[test]
fn reconnect_gives_up_at_once_on_a_port_that_cannot_be_opened() {
    const PORT: &str = "COM_HEADLESS_RECONNECT_HARD_FAIL";

    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);
    let task = expect_pending(
        &mut wb,
        AppCommand::Reconnect {
            port: PortId::new(PORT),
        },
    );
    assert_eq!(
        task_kind(&wb, task).as_deref(),
        Some("reconnect"),
        "Reconnect 必须落到重连任务本身，否则下面测的不是这次尝试"
    );

    let gave_up = tick_until(&mut wb, Duration::from_millis(2_500), |wb| {
        matches!(
            task_state(wb, task),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        gave_up,
        "硬失败必须在预算之内结束（不留任何重试等待），实际: {:?}",
        task_state(&wb, task)
    );
    assert_eq!(
        task_state(&wb, task),
        Some(TaskState::Failed),
        "端口根本打不开，重连不得假装成功"
    );
    assert_ne!(
        task_message(&wb, task).as_deref(),
        Some("重连超时（3 秒）"),
        "放弃的理由必须是端口给的那句错（那条「重连超时（3 秒）」已经不存在，这里防的是它回来）"
    );
}

/// 重连失败最终给用户看的那句话必须是翻译过的中文，不带平台 `Display` 的英文外框。
///
/// 旧的写法是 `map_err(|error| error.to_string())`：它把 `tool_platform::TransportError`
/// 整个 `Display` 出来（`transport operation failed: …`）—— 变体信息在能被翻译之前就被
/// 压成了字符串。换成 `translate_error` 之后，英文外框换成中文框架（`IO 错误：…` /
/// `串口错误：…`），正文沿用 transport 侧已经翻译过的那句。断言刻意只钉与系统 locale
/// 无关的两件事：英文外框不得出现、句子里必须有汉字 —— 框架（`IO 错误：` 等）本身就是
/// 中文写的，OS 给的那半句原文才随 locale 变。
#[test]
fn reconnect_failure_words_are_translated_before_they_reach_the_user() {
    const PORT: &str = "COM_HEADLESS_RECONNECT_WORDING";

    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);
    let task = expect_pending(
        &mut wb,
        AppCommand::Reconnect {
            port: PortId::new(PORT),
        },
    );
    let ended = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        matches!(
            task_state(wb, task),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        ended,
        "重连任务必须在 10s 内结束，实际: {:?}",
        task_state(&wb, task)
    );
    let words = task_message(&wb, task).expect("任务快照必须带着失败原文");

    assert!(
        !words.contains("transport operation failed"),
        "平台错误的英文外框又漏到用户面前了：{words:?}"
    );
    assert!(
        !words.contains("io error:"),
        "错误的 `Display` 原文又被当成用户文案了：{words:?}"
    );
    assert!(
        words.chars().any(|c| matches!(c, '\u{4E00}'..='\u{9FFF}')),
        "用户看到的那句必须带中文框架（`IO 错误：` / `串口错误：` 这类），实际: {words:?}"
    );
}

/// 已注册网络端口的重连必须走网络分支：`open_network_serial` + `Reconnected`。
///
/// 两条分支各拿各的错误类型、各造各的结果，写回落到串口分支的话，`open_serial` 会拿着
/// `127.0.0.1:9` 这种名字去枚举物理口，用户看到的是一句硬失败。
#[test]
fn network_reconnect_takes_the_network_branch() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);
    let config = NetworkSerialConfig {
        host: "127.0.0.1".to_owned(),
        port: 9,
        api_key: None,
    };
    let name = config.display_name();
    expect_done(&mut wb, AppCommand::RegisterNetworkPort { config });

    let task = expect_pending(
        &mut wb,
        AppCommand::Reconnect {
            port: PortId::new(name.clone()),
        },
    );
    assert_eq!(task_kind(&wb, task).as_deref(), Some("reconnect"));
    let ended = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        matches!(
            task_state(wb, task),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        ended,
        "网络重连必须在 10s 内结束，实际: {:?}",
        task_state(&wb, task)
    );
    assert_eq!(
        task_state(&wb, task),
        Some(TaskState::Completed),
        "网络端口 open 失败原因在 worker 线程里，重连这一层必须按 `open_network_serial` \
         的返回值收尾；落到串口分支就会 Failed（`COM` 枚举根本不认这个名字）"
    );
    // `Reconnected` 的落点：`poll_tasks` 把它写回选中端口。
    // 这条**钉不住 `Reconnected` 与 `TransportConnected` 的区别**：两者在 `poll_tasks` 里的
    // 落点逐字相同（实测把网络分支的 Ok 换成 `TransportConnected { port }`，本用例仍绿）。
    // 它钉的是「成功必须带一个会被 `poll_tasks` 落进 app 状态的结果」—— 换成 `TransportSent`
    // 这类没人处理的变体，选中端口还是空句，这里立刻变红。
    assert_eq!(
        wb.query_transport().config.port_name,
        name,
        "重连成功必须带一个被 `poll_tasks` 落进选中端口的结果，实际: {:?}",
        wb.query_transport().config.port_name
    );
}

/// 同一个端口登记第二次重连时，**前一个循环必须先被取消**（规则见
/// `task_model::cancel_deregistered_task`，四份 web 收尾与 native 共用这一份）。
///
/// 为什么这条用例必须在 native：wasm 那半没有测试运行器（全仓零 `wasm_bindgen_test`），
/// 而缺这道保护的恰恰是 web 的四份连接收尾 —— `insert` 就地覆盖前一个 task 的 id 之后，
/// 终态快照上的清理按 id 比对认不出它，只有 `Connected` / `Disconnected` 会按端口摘除，
/// 于是那个循环再也停不下来。规则收成一份之后，只有这里能驱动它，所以这份保护的红利
/// （漏改任一调用点立刻红）全靠本用例。
///
/// 驱动手段沿用本文件的「关不掉的 worker」：对端收下并读掉 WebSocket 握手后永不回话，
/// 平台后端那句 `disconnect` 就会堵满自己的超时预算 —— 前一个重连因此**真的还活着**，
/// 取消才不是一句空话（步骤①那条断言就是钉这个前提的）。
#[test]
fn reconnecting_the_same_port_twice_cancels_the_first_loop() {
    // 队列轮到它、那条 3 秒关闭超时走完，各给一次预算。依据是实测：96 个忙等进程下本用例
    // 整体 mean 9.2s / max 11.8s（那 3 秒被放大到约 3 倍），与本文件
    // `disconnecting_a_port_that_will_not_finish_closing_reports_chinese` 用的是同一个 30s。
    const BUDGET: Duration = Duration::from_secs(30);

    let (addr, parked) = parked_websocket_server();
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);
    let name = register_network_port(&mut wb, "127.0.0.1", addr.port());
    park_a_network_worker(&mut wb, &name, &parked);
    // 注销之后这个名字落到平台后端那条出口（`block_on_transport(backend.disconnect(..))`），
    // 重连循环因此会在那里堵满 3 秒 —— 网络分支的 `open_network_serial` 是立即返回的，
    // 用它就观察不到「前一个还在跑」这个前提。
    expect_done(
        &mut wb,
        AppCommand::RemoveNetworkPort {
            port: PortId::new(name.clone()),
        },
    );

    let first = expect_pending(
        &mut wb,
        AppCommand::Reconnect {
            port: PortId::new(name.clone()),
        },
    );
    let started = tick_until(&mut wb, BUDGET, |wb| {
        task_state(wb, first) == Some(TaskState::Running)
    });
    assert!(
        started,
        "第一次重连必须在 {BUDGET:?} 内跑进那条堵住的关闭里，实际: {:?}",
        task_state(&wb, first)
    );

    let second = expect_pending(
        &mut wb,
        AppCommand::Reconnect {
            port: PortId::new(name.clone()),
        },
    );
    // ① 前提仍然成立：登记第二个的这一刻，第一个还没结束（否则「取消前一个」无从谈起）。
    assert!(
        matches!(
            task_state(&wb, first),
            Some(TaskState::Pending) | Some(TaskState::Running)
        ),
        "登记第二个重连时前一个必须还活着，实际: {:?}",
        task_state(&wb, first)
    );
    // ② 第二个必须真的被登记上（它排在同一个 `serial:{端口}` 队列里，此时还是 Pending）。
    assert!(
        matches!(
            task_state(&wb, second),
            Some(TaskState::Pending) | Some(TaskState::Running)
        ),
        "第二个重连循环必须被登记，实际: {:?}",
        task_state(&wb, second)
    );

    let ended = tick_until(&mut wb, BUDGET, |wb| {
        matches!(
            task_state(wb, first),
            Some(TaskState::Completed | TaskState::Failed | TaskState::Cancelled)
        )
    });
    assert!(
        ended,
        "被取消的前一个循环必须在 {BUDGET:?} 内落终态（它堵在关闭超时里），实际: {:?}",
        task_state(&wb, first)
    );
    // ③ 这一句才是本用例的全部意义：前一个循环是**被取消**的，不是自己跑完的。
    // 把 `start_reconnect` 里那道保护换成 web 此前那份裸 `insert`，这里会读到 Failed
    // （端口根本打不开的那句超时），用例立刻红。
    assert_eq!(
        task_state(&wb, first),
        Some(TaskState::Cancelled),
        "同一端口第二次登记必须先取消前一个循环，实际: {:?} / 句串 {:?}",
        task_state(&wb, first),
        task_message(&wb, first)
    );
    // ④ 被取消的必须是前一个：新起的循环不能一起被取消。
    assert_ne!(
        task_state(&wb, second),
        Some(TaskState::Cancelled),
        "取消只能落在被顶替的前一个任务上，后来的那个循环不得被牵连"
    );

    // 收尾：等第二个循环自己结束，不给后面的用例留一个还在跑的 worker。
    let second_ended = tick_until(&mut wb, BUDGET, |wb| {
        matches!(
            task_state(wb, second),
            Some(TaskState::Completed | TaskState::Failed | TaskState::Cancelled)
        )
    });
    assert!(
        second_ended,
        "第二个循环必须自己走完那条超时，实际: {:?}",
        task_state(&wb, second)
    );
}

// ── 桌面端交给用户的那句话：九条 transport / platform 失败出口 ──────────────────
//
// `Workbench` 的后台任务闭包里曾有九处把传输/平台错误直接 `to_string()`。落点是
// `TaskSnapshot::message`（状态栏、日志、红色标签读的都是它），于是英文的
// `tool_platform::TransportError` 外框（`transport operation failed: `）或
// `tool_transport::TransportError` 原文（`port '…' is not open` / `io error: …`）
// 直接进了用户眼睛。现在两条类型各过各的中文映射器：
// `tool_transport::TransportError` → `tool_transport::translate_error`，
// `tool_platform::TransportError` → `tool_platform::translate_error`。
// 哪个站点包错了映射器**编译就不过**（两个 `translate_error` 的入参是不同名的类型），
// 下面这些用例钉的是另一件事：那句话真的到了用户手上，而且逐字就是它。
//
// 两条驱动手段，都不需要新机器：
// - **从未打开/从未连接的端口**：`TransportManager::enqueue_command` 给 `PortNotOpen`，
//   整句正文都是应用自己写的 → 发送、DTR/RTS 两条出口能逐字钉。
// - **一个关不掉的 worker**：连到一个「收下连接、读掉 WebSocket 握手请求、永不回话」的
//   loopback 对端上，网络 worker 就永久停在握手读里（那个读没有超时），于是
//   `close_port_blocking` 必定超时；而 `Io(TimedOut)` 的正文（`{端口} 正在关闭中`）
//   也是应用写的 → 串口的连接分支与关闭的两条分支同样能逐字钉。
//   代价只有各站点自己那份超时预算：`open_serial` 里是 100ms，`disconnect_port` 里是
//   3 秒（两条分支各占一条 per-port 有序队列，并发烧掉）。
//
// **三处失败出口从公开面上驱动不出来**，不是漏了：
// - `spawn_refresh_ports`：native 的 `list_known_ports` 只有
//   `serialport::available_ports()` 报错、或那个阻塞线程消失才会失败（本机实测
//   Completed）。
// - `spawn_request_port`：唯一的非兜底变体 `NoPortAvailable` 要一台一个串口都没有的
//   机器（本机实测 Completed）。
// - `connect` 的网络分支：`open_network_serial` 从不因「连不上」而失败（失败由 worker
//   以日志事件上报，见 `park_a_network_worker` 里那条 Completed 断言），它唯一的错误出口
//   是那次 100ms 的 `close_port_blocking`，而那要求「同名已经有一个关不掉的句柄」——
//   偏偏「同名已在打开」的早退按 `display_name()` **精确**匹配，规范名解析才忽略大小写，
//   所以只有主机名的大小写形态能把这两个名字区分开（`LOCALHOST:端口` 与 `localhost:端口`）。
//   实测这条路在本机走不通：`("LOCALHOST", p).to_socket_addrs()` 首选 `[::1]:p`，
//   而本节的对端只监听 IPv4 回环，于是 worker 直接被拒绝、连「停进握手读」都进不去。
//   拿解析顺序当依赖不是测试该做的事，所以这里宁可不钉。
//
// 前三句的逐字中文由 `crates/platform/tests/transport_error_wording.rs` 钉住（六个变体
// 全在那里，含 `NoPortAvailable`），下面只补一条「真失败时英文外框不得出现」的守卫，
// 并如实说明它在有串口、且 IPv6 回环优先的机器上是空跑 —— 不为它编一个假的成功路径。

/// 一个「收下连接、读走 WebSocket 握手请求、然后永远不回话」的 loopback 对端。
///
/// 只监听 **IPv4** 回环（与本文件其它用例同一形态）：主机名形态会先解析到 `[::1]`，
/// 于是「对端根本没监听」会被误读成「worker 已经退出」—— 本节抬头里那条驱动不出来的
/// 网络分支就是这个原因。
///
/// 返回的接收端收到一个 `()` 就代表对端已经进入握手读；观测走 channel 而不是 `join()`，
/// 所以调用方只按 deadline 等待，绝不阻塞在 `accept()` 上。
///
/// 之所以要真的 `accept` 并把请求读掉，而不是只 `bind` 不 `accept`：那是「worker 已经
/// 越过连接期那次 `stop` 检查」的**因果**凭据。只凭时间差就断开，会撞上「`stop` 正好
/// 落在连接与握手之间」的竞态 —— 那样 worker 直接退出，关闭反而成功，用例就不是在测
/// 超时分支了。
///
/// 观测线程**刻意不保证退出**：握着已建立的连接直到进程结束，正是「关不掉的 worker」
/// 的另一半。与本文件 `spawn_loopback_ws_server` 同一处置 —— 端口是 `:0` 由内核分配，
/// 多一个 parked 线程既不影响任何断言，也不影响其它用例。
fn parked_websocket_server() -> (SocketAddr, Receiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 loopback 监听");
    let addr = listener.local_addr().expect("内核分配的监听地址");
    let (sender, parked) = crossbeam_channel::unbounded();
    std::thread::spawn(move || {
        let Ok((mut socket, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0_u8; 256];
        if socket.read(&mut buf).is_ok() {
            let _ = sender.send(());
        }
        loop {
            std::thread::park();
        }
    });
    (addr, parked)
}

/// 注册一个网络端口（同步完成），返回它在 `Workbench` 里的名字 `host:port`。
fn register_network_port(wb: &mut Workbench, host: &str, port: u16) -> String {
    let config = NetworkSerialConfig {
        host: host.to_owned(),
        port,
        api_key: None,
    };
    let name = config.display_name();
    expect_done(wb, AppCommand::RegisterNetworkPort { config });
    name
}

/// 把一个网络端口连到「永不回话」的对端上，并等 worker 真的停进握手读。
///
/// `connect_network` 任务本身**必须成功**：`open_network_serial` 只负责把 worker 起起来，
/// 连不上是 worker 以日志事件上报的事。这条断言同时也是「`connect` 的网络分支造不出
/// 硬失败」的凭据 —— 那正是本节要把它改造成超时分支的原因。
fn park_a_network_worker(wb: &mut Workbench, name: &str, parked: &Receiver<()>) {
    let task = expect_pending(
        wb,
        AppCommand::Connect {
            port: PortId::new(name.to_owned()),
            settings: SerialSettings::default(),
        },
    );
    assert_eq!(
        task_kind(wb, task).as_deref(),
        Some("connect_network"),
        "已注册网络端口的连接不得落到 `connect_serial`，否则下面造不出「关不掉的 worker」"
    );
    let ended = tick_until(wb, Duration::from_secs(10), |wb| {
        matches!(
            task_state(wb, task),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        ended,
        "connect_network 必须在 10s 内结束，实际: {:?}",
        task_state(wb, task)
    );
    assert_eq!(
        task_state(wb, task),
        Some(TaskState::Completed),
        "对端不回话时 open 自身仍必须返回 Ok（失败由 worker 上报），否则本用例驱动的不是超时分支"
    );
    assert!(
        parked.recv_timeout(Duration::from_secs(10)).is_ok(),
        "对端必须已经收下并读掉握手请求，worker 才算停进握手读；没有这个信号，「关不掉」是假的"
    );
    assert!(
        wb.transport_status(name).connecting,
        "停在握手读里的端口应当仍是 connecting，实际: {:?}",
        wb.transport_status(name)
    );
}

/// 两个映射器的英文 `Display`：`map_err(|error| error.to_string())` 漏到用户面前的
/// 就是这几串，所以每个「本来该说中文」的失败出口都能用它兜一层。
fn leaked_english_frame(words: &str) -> Option<(&'static str, &'static str)> {
    [
        ("tool_platform 外框", "transport operation failed"),
        ("PortNotOpen 原文", "port '"),
        ("Io 原文", "io error:"),
        ("Serial 原文", "serial error:"),
        ("NoPortAvailable 原文", "no authorized serial port"),
    ]
    .into_iter()
    .find(|(_, frame)| words.contains(frame))
}

/// 这句话里有没有应用自己写的中文框架（`IO 错误：` / `串口错误：` 这类）。
fn reads_as_chinese(words: &str) -> bool {
    words.chars().any(|c| matches!(c, '\u{4E00}'..='\u{9FFF}'))
}

/// 断言任务以 Failed 收尾、给用户看的那句没有英文 `Display`，并把它交出来。
fn expect_failed_words(wb: &mut Workbench, task: TaskId, budget: Duration) -> String {
    let ended = tick_until(wb, budget, |wb| {
        matches!(
            task_state(wb, task),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        ended,
        "任务必须在 {budget:?} 内结束，实际: {:?}",
        task_state(wb, task)
    );
    assert_eq!(
        task_state(wb, task),
        Some(TaskState::Failed),
        "这条失败出口不得被静默成「成功」（措辞之外，行为也要钉）"
    );
    let words = task_message(wb, task).expect("任务快照必须带着失败原文");
    if let Some((name, frame)) = leaked_english_frame(&words) {
        panic!("英文 Display（{name} = {frame:?}）又漏到用户面前了：{words:?}");
    }
    assert!(
        reads_as_chinese(&words),
        "用户看到的那句必须带中文框架，实际: {words:?}"
    );
    words
}

/// `Workbench::connect` 的**串口**分支（平台后端那条出口）的两种失败形状。
///
/// 把该站点退回 `error.to_string()`，①②两格都会红：①是英文外框，②是外框再加上
/// 「正文不再逐字等于应用写的那句超时」。
#[test]
fn connecting_a_serial_port_reports_chinese_words_for_both_failure_shapes() {
    const PORT: &str = "COM_HEADLESS_CONNECT_WORDING";

    let (addr, parked) = parked_websocket_server();
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);

    // ① 根本打不开的口：`Operation(中文)` 原样返回，但 OS 给的那半句随 locale 变
    //    （`IO 错误：系统找不到指定的文件。` 与 `IO 错误：The system cannot find the
    //    file specified.`），所以这一格只能钉「应用写的那半句之外没有英文外框」，
    //    逐字文案由 `crates/transport` 自己的 `translate_error_covers_all_variants` 负责。
    let hard = expect_pending(
        &mut wb,
        AppCommand::Connect {
            port: PortId::new(PORT),
            settings: SerialSettings::default(),
        },
    );
    assert_eq!(
        task_kind(&wb, hard).as_deref(),
        Some("connect_serial"),
        "没注册成网络端口的名字必须走平台后端，否则本用例测的不是 `backend.connect` 那条出口"
    );
    expect_failed_words(&mut wb, hard, Duration::from_secs(10));

    // ② 同名还剩一个关不掉的旧 worker：`open_serial` 里那次 100ms 的
    //    `close_port_blocking` 超时，整句都是应用写的 → 逐字钉。
    let name = register_network_port(&mut wb, "127.0.0.1", addr.port());
    park_a_network_worker(&mut wb, &name, &parked);
    // 注销之后，这个名字在 `connect` 那里就不再是网络端口，于是落到串口分支。
    expect_done(
        &mut wb,
        AppCommand::RemoveNetworkPort {
            port: PortId::new(name.clone()),
        },
    );
    let timed_out = expect_pending(
        &mut wb,
        AppCommand::Connect {
            port: PortId::new(name.clone()),
            settings: SerialSettings::default(),
        },
    );
    assert_eq!(
        task_kind(&wb, timed_out).as_deref(),
        Some("connect_serial"),
        "注销后的同名端口必须走串口分支，否则测的是 `open_network_serial` 那条出口"
    );
    assert_eq!(
        expect_failed_words(&mut wb, timed_out, Duration::from_secs(10)),
        format!("操作超时：{name} 正在关闭中"),
        "关闭超时的正文被改写了：那是 `tool_transport::translate_error` 对 `Io(TimedOut)` 的那句话"
    );
}

/// `Workbench::disconnect_port` 的两条分支：网络分支直接调 `close_port_blocking`
/// （transport 那条出口），串口分支调平台后端的 `disconnect`（platform 那条出口）。
///
/// 两条都拿「关不掉的 worker」驱动：`.map_err(|error| error.to_string())` 在前者给出
/// `io error: …`、在后者给出 `transport operation failed: 操作超时：…`，两种都会红。
/// 顺带钉住一个反直觉的事实：**关闭一个从未打开的端口不是错误**
/// （`close_port_blocking` 对不存在的名字直接 `Ok(())`），所以那条路径平时根本没有
/// 可泄漏的文案 —— 也正因为如此，本用例才必须自己造出一个真的失败。
#[test]
fn disconnecting_a_port_that_will_not_finish_closing_reports_chinese() {
    let (addr_serial_side, parked_serial_side) = parked_websocket_server();
    let (addr_network_side, parked_network_side) = parked_websocket_server();
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);

    let never_opened = "COM_HEADLESS_DISCONNECT_NOT_OPEN";
    let network_side = register_network_port(&mut wb, "127.0.0.1", addr_network_side.port());
    park_a_network_worker(&mut wb, &network_side, &parked_network_side);
    let serial_side = register_network_port(&mut wb, "127.0.0.1", addr_serial_side.port());
    park_a_network_worker(&mut wb, &serial_side, &parked_serial_side);
    // 注销之后同名端口落到平台后端那条出口。
    expect_done(
        &mut wb,
        AppCommand::RemoveNetworkPort {
            port: PortId::new(serial_side.clone()),
        },
    );

    let network_task = expect_pending(
        &mut wb,
        AppCommand::Disconnect {
            port: PortId::new(network_side.clone()),
        },
    );
    let serial_task = expect_pending(
        &mut wb,
        AppCommand::Disconnect {
            port: PortId::new(serial_side.clone()),
        },
    );
    let not_opened_task = expect_pending(
        &mut wb,
        AppCommand::Disconnect {
            port: PortId::new(never_opened),
        },
    );

    // 两个超时任务各占一条 per-port 有序队列（`serial:{端口}`），所以两次 3 秒预算是
    // 并发烧掉的；第三格（从未打开）跟着一起收尾，不额外花时间。
    let all_ended = tick_until(&mut wb, Duration::from_secs(30), |wb| {
        [network_task, serial_task, not_opened_task]
            .iter()
            .all(|task| {
                matches!(
                    task_state(wb, *task),
                    Some(TaskState::Completed | TaskState::Failed)
                )
            })
    });
    assert!(
        all_ended,
        "三条关闭任务都必须在 30s 内结束，实际: {:?}",
        [network_task, serial_task, not_opened_task].map(|task| task_state(&wb, task))
    );
    assert_eq!(
        task_message(&wb, network_task).as_deref(),
        Some(format!("操作超时：{network_side} 正在关闭中").as_str()),
        "网络分支（`close_port_blocking`）给用户的那句变了"
    );
    assert_eq!(
        task_message(&wb, serial_task).as_deref(),
        Some(format!("操作超时：{serial_side} 正在关闭中").as_str()),
        "串口分支（平台后端 `disconnect`）给用户的那句变了"
    );
    assert_eq!(
        task_state(&wb, not_opened_task),
        Some(TaskState::Completed),
        "关闭一个从未打开的端口不是错误：这里没有可泄漏的文案，本用例的另外两格才有"
    );
}

/// `Workbench::send_transport_bytes` 的网络分支（transport 那条出口）。
/// 串口分支那条由
/// `sending_to_a_port_that_was_never_opened_reads_in_chinese_on_both_entry_points`
/// 逐字钉着，两条入口的正文从这里起是同一句。
#[test]
fn sending_over_a_network_port_that_was_never_connected_reads_in_chinese() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);
    let name = register_network_port(&mut wb, "127.0.0.1", 9);

    let task = expect_pending(
        &mut wb,
        AppCommand::SendText {
            port: PortId::new(name.clone()),
            text: "ping".to_owned(),
        },
    );
    assert_eq!(
        task_kind(&wb, task).as_deref(),
        Some("send_network"),
        "注册过的名字必须走 transport 那条投递入口，否则测的是平台后端的 `send`"
    );
    assert_eq!(
        expect_failed_words(&mut wb, task, Duration::from_secs(10)),
        format!("串口 {name} 未打开（可能已断开，请重连）"),
        "网络端口未连接时发送失败的那句正文变了（与串口分支同一句）"
    );
}

/// `Workbench::set_transport_signal`：DTR 与 RTS 两个分支共用同一条失败出口。
///
/// 顺带钉住网络端口那条**同步**拒绝 —— 它本轮之前就已经是中文，本轮一个字都不该改。
#[test]
fn toggling_dtr_or_rts_on_a_port_that_was_never_opened_reads_in_chinese() {
    const PORT: &str = "COM_HEADLESS_SIGNAL_WORDING";
    const NOT_OPEN_WORDS: &str = "串口 COM_HEADLESS_SIGNAL_WORDING 未打开（可能已断开，请重连）";

    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);

    for (signal, command) in [
        (
            "DTR",
            AppCommand::SetDtr {
                port: PortId::new(PORT),
                value: true,
            },
        ),
        (
            "RTS",
            AppCommand::SetRts {
                port: PortId::new(PORT),
                value: false,
            },
        ),
    ] {
        let task = expect_pending(&mut wb, command);
        assert_eq!(
            task_kind(&wb, task).as_deref(),
            Some("set_serial_signal"),
            "{signal} 必须落到设信号任务本身，否则测的不是 `backend.set_dtr`/`set_rts` 那条出口"
        );
        assert_eq!(
            expect_failed_words(&mut wb, task, Duration::from_secs(10)),
            NOT_OPEN_WORDS,
            "{signal} 失败给用户看的那句正文变了"
        );
    }

    let network = register_network_port(&mut wb, "127.0.0.1", 9);
    match wb.dispatch(AppCommand::SetDtr {
        port: PortId::new(network),
        value: true,
    }) {
        Err(AppError::Transport(words)) => assert_eq!(
            words, "网络端口不支持 DTR/RTS",
            "网络端口的同步拒绝本来就交给用户这句中文，本轮不许改动它"
        ),
        other => panic!("网络端口的 DTR/RTS 必须同步被拒，实际: {other:?}"),
    }
}

/// `Workbench::spawn_refresh_ports` 与 `spawn_request_port` 的两条失败出口。
///
/// **这两格在有串口的机器上是空跑的**（本机实测两个任务都 Completed），它们是
/// 「驱动不出失败」那条注释的落地：`request_port` 的 `NoPortAvailable` 需要一台一个
/// 串口都没有的机器，`list_known_ports` 需要 OS 枚举真的报错。真到了那种机器上，
/// 退回 `to_string()` 的泄漏是一整句英文（`no authorized serial port is available`、
/// `transport operation failed: …`），所以这层守卫值得留；逐字中文在
/// `crates/platform/tests/transport_error_wording.rs`。
#[test]
fn refreshing_and_requesting_ports_do_not_leak_the_platform_display_frame() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);

    for (kind, command) in [
        ("refresh_ports", AppCommand::RefreshPorts),
        ("request_port", AppCommand::RequestPort),
    ] {
        let task = expect_pending(&mut wb, command);
        assert_eq!(
            task_kind(&wb, task).as_deref(),
            Some(kind),
            "命令必须落到 {kind} 任务本身，否则下面测的不是那条出口"
        );
        let ended = tick_until(&mut wb, Duration::from_secs(10), |wb| {
            matches!(
                task_state(wb, task),
                Some(TaskState::Completed | TaskState::Failed)
            )
        });
        assert!(
            ended,
            "{kind} 任务必须在 10s 内结束，实际: {:?}",
            task_state(&wb, task)
        );
        if task_state(&wb, task) == Some(TaskState::Failed) {
            let words = task_message(&wb, task).expect("任务快照必须带着失败原文");
            if let Some((name, frame)) = leaked_english_frame(&words) {
                panic!("{kind} 把英文 Display（{name} = {frame:?}）当成了用户文案：{words:?}");
            }
            assert!(
                reads_as_chinese(&words),
                "{kind} 失败时用户看到的那句必须带中文框架，实际: {words:?}"
            );
        }
    }
}

/// 第 12 轮的目标：**发送认得两种形态，连接与重连只认一种**。
///
/// `is_network_port`（发送路径）同时接受 `display_name()`（`host:port`）与 `port_id()`
/// （`network://host:port`），而 `connect` 与 `start_reconnect` 只按 `display_name()` 匹配，
/// 于是同一个 id 点名时连接/重连/关闭静默降级到串口后端，发送虽然走对分支也会因端口表查不到
/// 这个名字而拿到 `PortNotOpen`。这不只是理论：wasm 侧的网络端口
/// 列表就是以 `port_id()` 为键的（`crates/app/src/web/serial.rs` 里 `let port = config.port_id()`），
/// 所以浏览器中点这一行的「连接」走的就是串口分支。
///
/// 本用例钉的是两件事：**认定只有一处判定**（两种形态都要认出来），且**写回的名字只有一个规范形**
/// —— `display_name()`，因为 `TransportManager` 的端口表就是用它作键的。
#[test]
fn network_ports_connect_and_reconnect_in_both_naming_forms() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);
    let config = NetworkSerialConfig {
        host: "127.0.0.1".to_owned(),
        port: 9,
        api_key: None,
    };
    let display = config.display_name();
    let network_id = config.port_id().to_string();
    expect_done(&mut wb, AppCommand::RegisterNetworkPort { config });

    // ① 连接：必须落到 `connect_network`，不得降级成 `connect_serial`。
    let connect = expect_pending(
        &mut wb,
        AppCommand::Connect {
            port: PortId::new(network_id.clone()),
            settings: SerialSettings::default(),
        },
    );
    assert_eq!(
        task_kind(&wb, connect).as_deref(),
        Some("connect_network"),
        "`{network_id}` 是注册过的网络端口，连接却静默走了串口后端"
    );
    let ended = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        matches!(
            task_state(wb, connect),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        ended,
        "connect 必须在 10s 内结束，实际: {:?}",
        task_state(&wb, connect)
    );
    assert_eq!(
        task_state(&wb, connect),
        Some(TaskState::Completed),
        "连不上是 worker 的事，`open_network_serial` 自身必须成功（与 display_name 形态同一收尾）"
    );
    // ② 规范化：无论用户用哪种形态点名，写回应用状态的必须是规范名 `host:port`。
    // `TransportManager` 的端口表以 `display_name()` 为键，把 `network://…` 留在选中端口里，
    // 下一句发送就会在 `enqueue_command` 上拿到 `PortNotOpen` —— 那就是把"连不上"换成了
    // "连上了但发不出"，不算修好。
    assert_eq!(
        wb.query_transport().config.port_name,
        display,
        "连接成功后写回的选中端口必须是规范名 `host:port`，实际: {:?}",
        wb.query_transport().config.port_name
    );

    // ③ 重连：同一个 id 不得因为形态不同而落到循环的串口分支。
    let reconnect = expect_pending(
        &mut wb,
        AppCommand::Reconnect {
            port: PortId::new(network_id.clone()),
        },
    );
    assert_eq!(
        task_kind(&wb, reconnect).as_deref(),
        Some("reconnect"),
        "重连命令必须被接受并起一个 reconnect 任务"
    );
    let ended = tick_until(&mut wb, Duration::from_secs(15), |wb| {
        matches!(
            task_state(wb, reconnect),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        ended,
        "reconnect 必须在 15s 内结束，实际: {:?}",
        task_state(&wb, reconnect)
    );
    assert_eq!(
        task_state(&wb, reconnect),
        Some(TaskState::Completed),
        "串口分支会拿着 `network://…` 去枚举物理口而 Failed，与 `display_name()` 形态那条 \
         `network_reconnect_takes_the_network_branch_of_the_loop` 的收尾不一致。实际句串: {:?}",
        task_message(&wb, reconnect)
    );
}

/// 上一条用例钉住的是 `connect` 那一处的规范形写回；这里钉**剩下三处同名判定**，
/// 因为它们各自的后果都不在 `connect` 那条断言的射程里（把任意一处改掉，只有本用例会红）：
/// - 发送不规范化 → `send_to("network://…")` 在 `enqueue_command` 上拿到 `PortNotOpen`，
///   任务 Failed、对端一帧都收不到；
/// - 关闭不规范化 → `Disconnected{network://…}` 与 `selected_port`（规范名）对不上，
///   端口已经关了而 UI 仍显示连着；
/// - 重连不规范化 → `Reconnected{network://…}` 被写进 `selected_port`，下一句发送就是
///   `PortNotOpen`（与 `connect` 那一处同一后果）。
#[test]
fn network_port_commands_in_the_id_form_share_the_canonical_port() {
    const SCRIPT: &str = "G28\n";

    // 对端等两帧：identify 与下面这一条 gcode 请求。
    let (addr, observed) = spawn_loopback_ws_server(2);
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);
    let config = NetworkSerialConfig {
        host: "127.0.0.1".to_owned(),
        port: addr.port(),
        api_key: None,
    };
    let display = config.display_name();
    let network_id = config.port_id().to_string();
    expect_done(&mut wb, AppCommand::RegisterNetworkPort { config });

    // ⓪ 先把端口真的打开（连的是活着的 loopback 服务器）。形态问题归上一条用例，
    // 这里要的是「端口表里有这个规范名」这个前提。
    let connect = expect_pending(
        &mut wb,
        AppCommand::Connect {
            port: PortId::new(display.clone()),
            settings: SerialSettings::default(),
        },
    );
    let ended = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        matches!(
            task_state(wb, connect),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        ended,
        "connect 必须在 10s 内结束，实际: {:?}",
        task_state(&wb, connect)
    );
    assert_eq!(
        task_state(&wb, connect),
        Some(TaskState::Completed),
        "连到真实 loopback 服务器必须成功，实际句串: {:?}",
        task_message(&wb, connect)
    );

    // ① 发送：`port_id()` 形态必须落到规范名那个 transport 键上。
    let send = expect_pending(
        &mut wb,
        AppCommand::SendText {
            port: PortId::new(network_id.clone()),
            text: SCRIPT.to_owned(),
        },
    );
    assert_eq!(
        task_kind(&wb, send).as_deref(),
        Some("send_network"),
        "`{network_id}` 的发送不得落到 send_serial"
    );
    let ended = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        matches!(
            task_state(wb, send),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        ended,
        "send 必须在 10s 内结束，实际: {:?}",
        task_state(&wb, send)
    );
    assert_eq!(
        task_state(&wb, send),
        Some(TaskState::Completed),
        "端口开着却发不出去，就是名字没规范化到 transport 认的那个键；句串: {:?}",
        task_message(&wb, send)
    );

    // `send_to` 是 fire-and-forget，任务 Completed 不等于字节上线，所以还要问对端。
    // 收帧必须在端口仍打开时完成：后面的关闭会让观测线程看到 Close 帧而提前收口。
    let (frames, stopped) = drain_frames(&observed, Duration::from_secs(20));
    assert_eq!(
        stopped, None,
        "回路服务器观测中断：{stopped:?}；已收到 {frames:?}"
    );
    let parsed: Vec<(String, u64, String)> =
        frames.iter().map(|frame| json_rpc_frame(frame)).collect();
    assert_eq!(
        parsed.len(),
        2,
        "对端应恰好收到 identify + 1 条 gcode，实际 {frames:?}"
    );
    assert_eq!(
        parsed[1].0, "printer.gcode.script",
        "第二条帧必须是 gcode 请求，实际: {:?}",
        parsed[1]
    );
    assert_eq!(
        parsed[1].2, SCRIPT,
        "`{network_id}` 形态的发送必须把这一帧投递到规范名 `{display}` 那个端口"
    );

    // ② 关闭：`port_id()` 形态写回的 `Disconnected` 必须与选中端口同源，才清得掉它。
    let disconnect = expect_pending(
        &mut wb,
        AppCommand::Disconnect {
            port: PortId::new(network_id.clone()),
        },
    );
    let ended = tick_until(&mut wb, Duration::from_secs(10), |wb| {
        matches!(
            task_state(wb, disconnect),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        ended,
        "disconnect 必须在 10s 内结束，实际: {:?}",
        task_state(&wb, disconnect)
    );
    assert_eq!(
        task_state(&wb, disconnect),
        Some(TaskState::Completed),
        "关闭一个开着的端口不该失败，句串: {:?}",
        task_message(&wb, disconnect)
    );
    assert_eq!(
        wb.query_transport().config.port_name,
        "",
        "端口已关闭，`selected_port` 却没被清掉 —— 它就是那种「一个端口两个名字」的残留"
    );

    // ③ 重连：写回的选中端口必须是规范名。
    let reconnect = expect_pending(
        &mut wb,
        AppCommand::Reconnect {
            port: PortId::new(network_id.clone()),
        },
    );
    let ended = tick_until(&mut wb, Duration::from_secs(15), |wb| {
        matches!(
            task_state(wb, reconnect),
            Some(TaskState::Completed | TaskState::Failed)
        )
    });
    assert!(
        ended,
        "reconnect 必须在 15s 内结束，实际: {:?}",
        task_state(&wb, reconnect)
    );
    assert_eq!(
        task_state(&wb, reconnect),
        Some(TaskState::Completed),
        "重连的收尾与 `connect` 同一处约定（连不上是 worker 的事），句串: {:?}",
        task_message(&wb, reconnect)
    );
    assert_eq!(
        wb.query_transport().config.port_name,
        display,
        "重连写回的名字必须是规范名 `host:port`，否则下一句发送就在 `enqueue_command` 上拿到 PortNotOpen"
    );
}
