use tool_application::{AppCommand, Workbench};
use tool_databus::DataBus;

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
        port_name: "COM_NOT_EXIST_999".into(),
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
