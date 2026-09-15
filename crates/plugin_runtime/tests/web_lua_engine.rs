//! Browser Lua VM contract tests.
//!
//! The browser cannot run `mlua` (C/FFI), so it uses the pure Rust VM in
//! `tool_plugin_runtime::web_lua`. That module used to be `wasm32`-gated, which
//! meant no automated test ever executed it. These tests run the same VM on the
//! host target and pin the behavior the browser depends on: the shared
//! `LuaEngine` protocol, the sandbox, capability gating, host request mapping
//! and replay analysis.

use std::cell::RefCell;
use std::rc::Rc;

use tool_plugin_api::{
    LogLevel, LuaEngine, PluginCallResult, PluginCapability, PluginError, PluginFunctionId,
    PluginHostApi, PluginHostRequest, PluginInstanceId, PluginLoadConfig, PluginPermissions,
    PluginValue,
};
use tool_plugin_runtime::WebLuaEngine;

/// Host double that answers the requests the VM maps onto `ctx.*`.
#[derive(Default)]
struct RecordingHost {
    logs: RefCell<Vec<String>>,
    published: RefCell<Vec<(String, PluginValue)>>,
    storage: RefCell<std::collections::BTreeMap<String, PluginValue>>,
}

impl RecordingHost {
    fn logs(&self) -> Vec<String> {
        self.logs.borrow().clone()
    }
}

impl PluginHostApi for RecordingHost {
    fn request(&self, request: PluginHostRequest) -> Result<PluginValue, PluginError> {
        match request {
            PluginHostRequest::NowMs => Ok(PluginValue::Integer(1_700)),
            PluginHostRequest::Log { level, message } => {
                self.logs.borrow_mut().push(format!("{level:?}:{message}"));
                Ok(PluginValue::Null)
            }
            PluginHostRequest::BusPublish { topic, value } => {
                self.published.borrow_mut().push((topic, value));
                Ok(PluginValue::Null)
            }
            PluginHostRequest::StorageGet { key } => Ok(self
                .storage
                .borrow()
                .get(&key)
                .cloned()
                .unwrap_or(PluginValue::Null)),
            PluginHostRequest::StorageSet { key, value } => {
                self.storage.borrow_mut().insert(key, value);
                Ok(PluginValue::Null)
            }
            PluginHostRequest::StorageDelete { key } => {
                self.storage.borrow_mut().remove(&key);
                Ok(PluginValue::Null)
            }
            PluginHostRequest::StorageKeys => Ok(PluginValue::Array(
                self.storage
                    .borrow()
                    .keys()
                    .cloned()
                    .map(PluginValue::String)
                    .collect(),
            )),
            _ => Err(PluginError::UnsupportedCapability("test host".to_owned())),
        }
    }
}

fn load_config(permissions: &[PluginCapability]) -> PluginLoadConfig {
    PluginLoadConfig {
        plugin_id: "test.plugin".to_owned(),
        plugin_name: "Test Plugin".to_owned(),
        plugin_version: "1.0.0".to_owned(),
        script_name: "main.lua".to_owned(),
        context: PluginValue::Null,
        permissions: PluginPermissions::new(permissions.to_vec()),
    }
}

fn load(
    engine: &mut WebLuaEngine,
    host: &Rc<RecordingHost>,
    source: &str,
    permissions: &[PluginCapability],
) -> PluginInstanceId {
    engine
        .load_plugin(source, load_config(permissions), host.clone())
        .expect("plugin should load through the shared engine boundary")
}

fn completed(result: PluginCallResult) -> PluginValue {
    match result {
        PluginCallResult::Completed(value) => value,
        other => panic!("expected a synchronous result, got {other:?}"),
    }
}

fn object(fields: impl IntoIterator<Item = (&'static str, PluginValue)>) -> PluginValue {
    PluginValue::Object(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn serial_rx_event(id: i64, payload: &str, port: &str) -> PluginValue {
    object([
        ("id", PluginValue::Integer(id)),
        ("timestamp_ms", PluginValue::Integer(id * 10)),
        (
            "topic",
            PluginValue::String("transport.serial.default.rx".to_owned()),
        ),
        ("payload", PluginValue::String(payload.to_owned())),
        (
            "metadata",
            object([("port", PluginValue::String(port.to_owned()))]),
        ),
    ])
}

#[test]
fn shared_engine_boundary_loads_and_dispatches_lua() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();
    let instance = load(
        &mut engine,
        &host,
        r#"
            ctx.commands.register("demo.run", function(payload)
                ctx.log.info(payload.message)
                return { ok = true, now = ctx.now_ms() }
            end)
        "#,
        &[PluginCapability::Log],
    );

    let result = engine
        .dispatch_command(
            instance,
            "demo.run",
            object([("message", PluginValue::String("hello".to_owned()))]),
        )
        .expect("registered command should be dispatchable");

    assert_eq!(host.logs(), vec!["Info:hello".to_owned()]);

    let PluginValue::Object(fields) = completed(result) else {
        panic!("command should return a table");
    };
    assert_eq!(fields.get("ok"), Some(&PluginValue::Bool(true)));
    assert_eq!(fields.get("now"), Some(&PluginValue::Integer(1_700)));
}

#[test]
fn top_level_plugin_body_runs_on_load() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();
    load(
        &mut engine,
        &host,
        r#"ctx.log.info("started " .. ctx.plugin.id)"#,
        &[PluginCapability::Log],
    );

    assert_eq!(host.logs(), vec!["Info:started test.plugin".to_owned()]);
}

#[test]
fn disabling_the_instance_runs_on_disable_once() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();
    let instance = load(
        &mut engine,
        &host,
        r#"
            on_disable(function()
                ctx.log.info("stopped")
            end)
        "#,
        &[PluginCapability::Log],
    );

    engine.stop(instance).expect("stop should run on_disable");
    assert_eq!(host.logs(), vec!["Info:stopped".to_owned()]);

    // 实例已移除：再次 stop 必须报错而不是静默成功。
    assert!(engine.stop(instance).is_err());
}

#[test]
fn bus_events_reach_the_matching_handler_only() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();
    let instance = load(
        &mut engine,
        &host,
        r#"
            ctx.bus.on("transport.serial.*", function(event)
                ctx.log.info("serial " .. event.topic)
            end)
            ctx.bus.on("protocol.imu.*", function(event)
                ctx.log.info("imu " .. event.topic)
            end)
        "#,
        &[PluginCapability::Bus, PluginCapability::Log],
    );

    engine
        .dispatch_event(
            instance,
            object([(
                "topic",
                PluginValue::String("transport.serial.default.rx".to_owned()),
            )]),
        )
        .expect("wildcard handler should receive the event");

    assert_eq!(
        host.logs(),
        vec!["Info:serial transport.serial.default.rx".to_owned()],
        "only the matching topic pattern may run"
    );
}

#[test]
fn bus_publish_reaches_the_host() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();
    load(
        &mut engine,
        &host,
        r#"ctx.bus.publish("plugin.derived.sample", { value = 42 })"#,
        &[PluginCapability::Bus],
    );

    let published = host.published.borrow();
    let (topic, payload) = published
        .iter()
        .find(|(topic, _)| topic == "plugin.derived.sample")
        .expect("publish should reach the host");
    assert_eq!(topic, "plugin.derived.sample");
    let PluginValue::Object(fields) = payload else {
        panic!("published payload should be a table");
    };
    assert_eq!(fields.get("value"), Some(&PluginValue::Integer(42)));
}

#[test]
fn session_storage_round_trips_through_the_host() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();
    load(
        &mut engine,
        &host,
        r#"
            ctx.session.set("mode", "fast")
            ctx.log.info("read " .. tostring(ctx.session.get("mode", "unset")))
            ctx.log.info("missing " .. tostring(ctx.session.get("nope", "unset")))
        "#,
        &[PluginCapability::Storage, PluginCapability::Log],
    );

    assert_eq!(
        host.logs(),
        vec!["Info:read fast".to_owned(), "Info:missing unset".to_owned()]
    );
    assert_eq!(
        host.storage.borrow().get("mode"),
        Some(&PluginValue::String("fast".to_owned()))
    );
}

#[test]
fn capabilities_are_denied_when_not_declared() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();

    // 未声明 log 权限：ctx.log 不存在，脚本在加载阶段就失败，不能落到宿主。
    let result = engine.load_plugin(
        r#"ctx.log.info("should not be reachable")"#,
        load_config(&[]),
        host.clone(),
    );

    assert!(result.is_err(), "undeclared capability must fail the load");
    assert!(
        host.logs().is_empty(),
        "undeclared capability must not reach the host"
    );
}

#[test]
fn sandbox_removes_file_and_process_access() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();

    for source in [
        r#"io.open("/etc/passwd")"#,
        r#"os.execute("echo hi")"#,
        r#"dofile("other.lua")"#,
        r#"loadfile("other.lua")"#,
    ] {
        let result = engine.load_plugin(source, load_config(&[]), host.clone());
        assert!(
            result.is_err(),
            "sandbox must reject `{source}` instead of executing it"
        );
    }
}

#[test]
fn instruction_limit_stops_runaway_plugin() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();

    let started = std::time::Instant::now();
    let result = engine.load_plugin(r#"while true do end"#, load_config(&[]), host.clone());

    assert!(result.is_err(), "infinite loop must be interrupted");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "instruction hook must stop the loop instead of hanging the UI thread"
    );
}

#[test]
fn unknown_function_id_is_reported_as_error() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();
    let instance = load(&mut engine, &host, "local x = 1", &[]);

    let result = engine.call(
        instance,
        PluginFunctionId("missing_function".to_owned()),
        &[],
    );
    assert!(result.is_err(), "missing global must not be reported as ok");
}

#[test]
fn bundled_replay_analyzer_derives_events_from_recorded_lines() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();
    let instance = engine
        .load_replay_plugin(
            include_str!("../../../plugins/template.serial-chart/replay.lua"),
            load_config(&[]),
            vec!["protocol.template.sample".to_owned()],
            host.clone(),
        )
        .expect("bundled replay analyzer should load in the browser VM");

    engine
        .replay_begin(instance, object([("event_count", PluginValue::Integer(2))]))
        .expect("on_replay_begin should run");

    for (id, line) in [
        (1, r#"{"seq":1,"value":12.3,"target":50.0}"#),
        (3, r#"{"seq":3,"value":13.5,"target":51.0}"#),
    ] {
        engine
            .replay_event(instance, serial_rx_event(id, &format!("{line}\n"), "COM1"))
            .expect("on_replay_event should run");
    }

    let output = engine
        .replay_end(instance)
        .expect("on_replay_end should run");

    assert_eq!(output.events.len(), 2, "one derived sample per packet");
    let PluginValue::Object(first) = &output.events[0] else {
        panic!("derived event should be a table");
    };
    assert_eq!(
        first.get("topic"),
        Some(&PluginValue::String("protocol.template.sample".to_owned()))
    );
    let PluginValue::Object(payload) = first.get("payload").expect("payload") else {
        panic!("derived payload should be a table");
    };
    // seq=1 之后直接跳到 seq=3：丢失计数必须被最上层脚本算出来。
    assert_eq!(payload.get("t"), Some(&PluginValue::Integer(1)));
    assert_eq!(payload.get("received"), Some(&PluginValue::Integer(1)));
    assert_eq!(payload.get("lost_total"), Some(&PluginValue::Integer(0)));

    let PluginValue::Object(second) = &output.events[1] else {
        panic!("derived event should be a table");
    };
    let PluginValue::Object(second_payload) = second.get("payload").expect("payload") else {
        panic!("derived payload should be a table");
    };
    assert_eq!(second_payload.get("t"), Some(&PluginValue::Integer(3)));
    assert_eq!(
        second_payload.get("lost_total"),
        Some(&PluginValue::Integer(1))
    );

    assert_eq!(output.logs.len(), 2, "replay begin/end logs are collected");
    assert!(output.logs[0].contains("events=2"));
    assert!(output.logs[1].contains("received=2"));
    assert!(output.logs[1].contains("lost=1"));
}

#[test]
fn replay_emit_rejects_topics_missing_from_the_manifest() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();
    let instance = engine
        .load_replay_plugin(
            r#"
                function on_replay_event(event)
                    ctx.replay.emit("protocol.undeclared", { value = 1 })
                end
            "#,
            load_config(&[]),
            vec!["protocol.template.sample".to_owned()],
            host.clone(),
        )
        .expect("replay plugin should load");

    let result = engine.replay_event(instance, serial_rx_event(1, "x\n", "COM1"));
    assert!(
        result.is_err(),
        "undeclared replay output must be rejected instead of silently emitted"
    );
}

#[test]
fn live_plugin_cannot_use_replay_api() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();
    let instance = load(&mut engine, &host, "local x = 1", &[]);

    assert!(
        engine.replay_end(instance).is_err(),
        "live instances must be rejected by the replay API"
    );
}

#[test]
fn log_levels_map_to_the_host_protocol() {
    let host = Rc::new(RecordingHost::default());
    let mut engine = WebLuaEngine::new();
    load(
        &mut engine,
        &host,
        r#"
            ctx.log.trace("t")
            ctx.log.debug("d")
            ctx.log.info("i")
            ctx.log.warn("w")
            ctx.log.error("e")
        "#,
        &[PluginCapability::Log],
    );

    assert_eq!(
        host.logs(),
        vec![
            format!("{:?}:t", LogLevel::Trace),
            format!("{:?}:d", LogLevel::Debug),
            format!("{:?}:i", LogLevel::Info),
            format!("{:?}:w", LogLevel::Warn),
            format!("{:?}:e", LogLevel::Error),
        ]
    );
}
