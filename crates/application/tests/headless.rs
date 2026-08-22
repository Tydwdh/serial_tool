use tool_application::{AppCommand, Workbench};
use tool_databus::DataBus;

#[test]
fn headless_workbench_can_dispatch_and_query() {
    let bus = DataBus::new();
    let mut wb = Workbench::new(bus);

    // RefreshPorts should be executable without egui.
    wb.dispatch(AppCommand::RefreshPorts).expect("refresh");

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
    let err = wb.dispatch(AppCommand::Connect {
        port_name: "COM_NOT_EXIST_999".into(),
    });
    assert!(err.is_err());

    // Tick must be callable headless.
    wb.tick(0.0);
    wb.tick(1.0);
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
