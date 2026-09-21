//! Platform capability contract tests.
//!
//! `tool-platform` is the boundary Application and the UI both depend on, so
//! its identifiers, capability flags and native storage behaviour are pinned
//! here. The `wasm32` implementations are exercised by the browser build; these
//! tests cover the platform-neutral values plus the native storage capability.

use futures_executor::block_on;
use tempfile::tempdir;
use tool_platform::storage::native::{NativeFileService, NativeSettingsStore};
use tool_platform::storage::{FileBlob, FileHandle, FileId, FileService, SettingsStore};
use tool_platform::{
    NetworkSerialConfig, PortId, PortKind, SerialParity, SerialSettings, TransportCapabilities,
    serial_rx_event, serial_tx_event,
};

#[test]
fn serial_settings_defaults_match_the_documented_line_parameters() {
    let settings = SerialSettings::default();
    assert_eq!(settings.baud_rate, 115_200);
    assert_eq!(settings.data_bits, 8);
    assert_eq!(settings.stop_bits, 1);
    assert_eq!(settings.parity, SerialParity::None);
}

#[test]
fn serial_settings_round_trip_through_json_with_snake_case_parity() {
    let settings = SerialSettings {
        baud_rate: 1_000_000,
        data_bits: 8,
        stop_bits: 2,
        parity: SerialParity::Even,
    };

    let json = serde_json::to_value(settings).expect("serialize");
    assert_eq!(json["baud_rate"], 1_000_000);
    assert_eq!(json["parity"], "even");

    let restored: SerialSettings = serde_json::from_value(json).expect("deserialize");
    assert_eq!(restored, settings);
}

#[test]
fn network_port_identity_cannot_collide_with_a_serial_port() {
    let config = NetworkSerialConfig {
        host: "nexus.local".to_owned(),
        port: 7125,
        api_key: None,
    };

    assert_eq!(config.display_name(), "nexus.local:7125");
    assert_eq!(config.port_id(), PortId::new("network://nexus.local:7125"));

    let descriptor = config.descriptor();
    assert_eq!(descriptor.kind, PortKind::Network);
    assert_eq!(descriptor.label, "nexus.local:7125");
    assert!(descriptor.authorized, "配置过的网络端点不需要额外授权");

    // 浏览器串口端口用不透明 id；前缀保证两者不会撞成同一个 PortId。
    assert_ne!(descriptor.id, PortId::new(config.display_name()));
}

#[test]
fn network_serial_config_defaults_port_and_api_key() {
    let config = NetworkSerialConfig::default();
    assert_eq!(config.port, 7125);
    assert!(config.api_key.is_none());

    let restored: NetworkSerialConfig =
        serde_json::from_str(r#"{"host":"printer.local","port":80}"#).expect("deserialize");
    assert!(restored.api_key.is_none(), "api_key 缺省时必须可反序列化");
}

#[test]
fn network_capabilities_do_not_advertise_line_signals() {
    // 三套能力放在同一处比较：断言的是「哪一端开了哪些位」，不是单个位的绝对值。
    let native = TransportCapabilities::NATIVE_SERIAL;
    let web = TransportCapabilities::WEB_SERIAL;
    let network = TransportCapabilities::WEB_NETWORK;

    assert!(!network.set_dtr);
    assert!(!network.set_rts);
    assert!(network.connect);
    assert!(network.send);
    assert!(!network.request_port, "网络端点不需要浏览器设备授权");
    assert!(
        !network.list_known_ports,
        "网络端点由用户配置，不是浏览器枚举出来的"
    );

    assert!(native.set_dtr && native.set_rts && native.request_port);
    assert!(web.set_dtr && web.send && web.connect);
}

#[test]
fn port_id_is_a_stable_ordered_identifier() {
    let a = PortId::new("serial:COM1");
    let b = PortId::new("serial:COM2");
    assert!(a < b);
    assert_eq!(a.to_string(), "serial:COM1");
    assert_eq!(a.as_str(), "serial:COM1");
    assert_eq!(
        serde_json::to_string(&a).expect("serialize"),
        "\"serial:COM1\""
    );
    assert_eq!(
        serde_json::from_str::<PortId>("\"serial:COM1\"").expect("deserialize"),
        a
    );
}

#[test]
fn rx_and_tx_events_carry_topic_direction_and_port_metadata() {
    let port = PortId::new("serial:COM3");

    let rx = serial_rx_event(&port, b"hello".to_vec());
    assert_eq!(rx.topic, tool_core::topics::SERIAL_RX);
    assert_eq!(rx.source, "serial:serial:COM3");
    assert_eq!(rx.direction, tool_core::Direction::Rx);
    assert_eq!(rx.metadata["port"], "serial:COM3");
    match rx.payload {
        tool_core::Payload::Bytes(bytes) => assert_eq!(bytes, b"hello"),
        other => panic!("RX payload must stay raw bytes, got {other:?}"),
    }

    let tx = serial_tx_event(&port, b"ping".to_vec());
    assert_eq!(tx.topic, tool_core::topics::SERIAL_TX);
    assert_eq!(tx.direction, tool_core::Direction::Tx);
}

#[test]
fn file_handle_named_and_path_flavors_stay_distinguishable() {
    let named = FileHandle::named("session.log");
    assert_eq!(named.name(), "session.log");
    assert_eq!(named.id().as_str(), "session.log");
    assert!(
        named.native_path().is_none(),
        "name-only handle must not pretend to have a path"
    );

    let from_path = FileHandle::from_native_path(std::env::temp_dir().join("session.log"));
    assert!(from_path.native_path().is_some());
    assert_eq!(from_path.name(), from_path.id().as_str());
}

#[test]
fn native_settings_store_round_trips_and_removes() {
    let root = tempdir().expect("temp dir");
    let store = NativeSettingsStore::new(root.path());
    let load = || store.load_blocking("workspace.json").expect("load");

    assert_eq!(load(), None, "未写入的 key 必须返回 None 而不是错误");

    store
        .save_blocking("workspace.json", b"{\"a\":1}".to_vec())
        .expect("save");
    assert_eq!(load(), Some(b"{\"a\":1}".to_vec()));

    // 覆盖写入：Windows 的 rename 不能替换已存在的目标，能力层必须自己兜住。
    store
        .save_blocking("workspace.json", b"{\"a\":2}".to_vec())
        .expect("overwrite");
    assert_eq!(load(), Some(b"{\"a\":2}".to_vec()));

    block_on(store.remove("workspace.json".to_owned())).expect("remove");
    assert_eq!(load(), None);
    // 再次删除已不存在的 key 也要成功（幂等）。
    block_on(store.remove("workspace.json".to_owned())).expect("idempotent");
}

#[test]
fn native_file_service_round_trips_nested_ids() {
    let root = tempdir().expect("temp dir");
    let service = NativeFileService::new(root.path());
    let id = FileId::new("exports/2026/session.csv");

    block_on(service.write(
        id.clone(),
        FileBlob {
            name: "session.csv".to_owned(),
            mime: "text/csv".to_owned(),
            bytes: b"a,b\n1,2\n".to_vec(),
        },
    ))
    .expect("write");

    let blob = block_on(service.read(id)).expect("read");
    assert_eq!(blob.bytes, b"a,b\n1,2\n");
    assert!(root.path().join("exports/2026/session.csv").is_file());
}

#[test]
fn native_file_service_rejects_ids_that_escape_the_root() {
    let root = tempdir().expect("temp dir");
    let service = NativeFileService::new(root.path());
    let outside = root.path().parent().expect("parent").join("outside.txt");

    for escaping in [
        "../outside.txt".to_owned(),
        "exports/../../outside.txt".to_owned(),
        std::env::current_dir()
            .expect("cwd")
            .join("absolute.txt")
            .display()
            .to_string(),
    ] {
        let id = FileId::new(escaping.as_str());
        let write = block_on(service.write(
            id.clone(),
            FileBlob {
                name: "x".to_owned(),
                mime: "text/plain".to_owned(),
                bytes: b"x".to_vec(),
            },
        ));
        assert!(
            write.is_err(),
            "`{escaping}` 必须被拒绝，不能写到存储根目录之外"
        );
        assert!(
            block_on(service.read(id)).is_err(),
            "`{escaping}` 的读路径同样必须被拒绝"
        );
    }

    assert!(!outside.exists(), "被拒绝的写入不得在根目录外留下文件");
}
