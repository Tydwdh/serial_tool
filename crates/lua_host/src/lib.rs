use crossbeam_channel::{Receiver, Sender, bounded};
use mlua::{Function, Lua, LuaOptions, StdLib, Table, Value, VmState};
use parking_lot::Mutex as ParkingMutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;
use tool_core::{Direction, Event, LogLevel, Payload, topics};
use tool_databus::{DataBus, TopicFilter};
use tool_testing::TestPacketLog;
use tool_transport::{SerialConfig, TransportManager};

const LUA_PLUGIN_EVENT_QUEUE_CAPACITY: usize = 4096;

const LUA_DEFAULT_PERMISSIONS: &[&str] =
    &["bus", "log", "serial", "ui", "storage", "timer", "testing"];

#[derive(Debug, Error)]
pub enum LuaHostError {
    #[error("script is already running")]
    AlreadyRunning,

    #[error("lua error: {0}")]
    Lua(#[from] mlua::Error),
}

pub type LuaHostResult<T> = Result<T, LuaHostError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LuaRunConfig {
    pub script_name: String,
    pub timeout_ms: u64,
    pub source: String,
    pub context: serde_json::Value,

    #[serde(default = "default_lua_permissions")]
    pub permissions: Vec<String>,
}

impl Default for LuaRunConfig {
    fn default() -> Self {
        Self {
            script_name: "scratch.lua".to_owned(),
            timeout_ms: 5_000,
            source: "lua".to_owned(),
            context: json!({}),
            permissions: default_lua_permissions(),
        }
    }
}

fn default_lua_permissions() -> Vec<String> {
    LUA_DEFAULT_PERMISSIONS
        .iter()
        .map(|permission| permission.to_string())
        .collect()
}

fn has_permission(config: &LuaRunConfig, permission: &str) -> bool {
    config
        .permissions
        .iter()
        .any(|candidate| candidate == permission)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LuaRunState {
    Idle,
    Running,
    Finished,
    Failed,
    Stopped,
}

struct LuaWorker {
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    outcome: Arc<ParkingMutex<Option<LuaRunState>>>,
    join: Option<JoinHandle<()>>,
}

pub struct LuaPluginRuntime {
    event_sender: Sender<Event>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl LuaPluginRuntime {
    pub fn on_event(&self, event: &Event) -> bool {
        if !self.alive.load(Ordering::Relaxed) {
            return false;
        }

        self.event_sender.try_send(event.clone()).is_ok()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

impl Drop for LuaPluginRuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);

        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub struct LuaHost {
    bus: DataBus,
    transport: TransportManager,
    worker: Option<LuaWorker>,
    last_state: LuaRunState,
}

impl LuaHost {
    pub fn new(bus: DataBus, transport: TransportManager) -> Self {
        Self {
            bus,
            transport,
            worker: None,
            last_state: LuaRunState::Idle,
        }
    }

    pub fn run_script(&mut self, source: String, config: LuaRunConfig) -> LuaHostResult<()> {
        self.reap_finished();

        if self.worker.is_some() {
            return Err(LuaHostError::AlreadyRunning);
        }

        let stop = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let outcome = Arc::new(ParkingMutex::new(None));

        let thread_stop = Arc::clone(&stop);
        let thread_finished = Arc::clone(&finished);
        let thread_outcome = Arc::clone(&outcome);

        let bus = self.bus.clone();
        let transport = self.transport.clone();
        let run_source = config.source.clone();

        bus.publish(Event::system_log(
            LogLevel::Info,
            run_source,
            format!("running {}", config.script_name),
        ));

        let join = thread::spawn(move || {
            let result = run_script_blocking(
                source,
                config.clone(),
                bus.clone(),
                transport,
                Arc::clone(&thread_stop),
            );

            match result {
                Ok(()) => {
                    *thread_outcome.lock() = Some(LuaRunState::Finished);

                    bus.publish(Event::system_log(
                        LogLevel::Info,
                        config.source,
                        format!("{} finished", config.script_name),
                    ));
                }
                Err(error) if thread_stop.load(Ordering::Relaxed) => {
                    *thread_outcome.lock() = Some(LuaRunState::Stopped);

                    bus.publish(Event::system_log(
                        LogLevel::Warn,
                        config.source,
                        format!("{} stopped: {error}", config.script_name),
                    ));
                }
                Err(error) => {
                    *thread_outcome.lock() = Some(LuaRunState::Failed);

                    bus.publish(Event::system_log(
                        LogLevel::Error,
                        config.source,
                        format!("{} failed: {error}", config.script_name),
                    ));
                }
            }

            thread_finished.store(true, Ordering::Relaxed);
        });

        self.worker = Some(LuaWorker {
            stop,
            finished,
            outcome,
            join: Some(join),
        });

        self.last_state = LuaRunState::Running;

        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.stop.store(true, Ordering::Relaxed);

            self.bus
                .publish(Event::system_log(LogLevel::Warn, "lua", "stop requested"));
        }

        self.reap_finished();
    }

    pub fn state(&mut self) -> LuaRunState {
        self.reap_finished();
        self.last_state
    }

    fn reap_finished(&mut self) {
        let worker_done = self
            .worker
            .as_ref()
            .map(|worker| worker.finished.load(Ordering::Relaxed))
            .unwrap_or(false);

        if worker_done && let Some(mut worker) = self.worker.take() {
            let outcome = *worker.outcome.lock();

            if let Some(join) = worker.join.take() {
                let _ = join.join();
            }

            self.last_state = outcome.unwrap_or(LuaRunState::Finished);
        }
    }
}

impl Drop for LuaHost {
    fn drop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.stop.store(true, Ordering::Relaxed);
        }

        if let Some(mut worker) = self.worker.take()
            && let Some(join) = worker.join.take()
        {
            let _ = join.join();
        }
    }
}

pub fn run_plugin(
    source: String,
    config: LuaRunConfig,
    bus: DataBus,
    transport: TransportManager,
) -> LuaHostResult<LuaPluginRuntime> {
    let (event_sender, event_receiver) = bounded(LUA_PLUGIN_EVENT_QUEUE_CAPACITY);

    let stop = Arc::new(AtomicBool::new(false));
    let alive = Arc::new(AtomicBool::new(true));

    let thread_stop = Arc::clone(&stop);
    let thread_alive = Arc::clone(&alive);

    let plugin_source = config.source.clone();

    bus.publish(Event::system_log(
        LogLevel::Info,
        &plugin_source,
        format!("starting plugin {}", config.script_name),
    ));

    let join = thread::spawn(move || {
        plugin_event_loop(
            source,
            config,
            bus,
            transport,
            event_receiver,
            thread_stop,
            thread_alive,
        );
    });

    Ok(LuaPluginRuntime {
        event_sender,
        stop,
        alive,
        join: Some(join),
    })
}

fn plugin_event_loop(
    source: String,
    config: LuaRunConfig,
    bus: DataBus,
    transport: TransportManager,
    event_receiver: Receiver<Event>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
) {
    let lua = match Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    ) {
        Ok(lua) => lua,
        Err(error) => {
            bus.publish(Event::system_log(
                LogLevel::Error,
                &config.source,
                format!("failed to create lua: {error}"),
            ));

            alive.store(false, Ordering::Relaxed);
            return;
        }
    };

    if let Err(error) = install_ctx(&lua, bus.clone(), transport, &config) {
        bus.publish(Event::system_log(
            LogLevel::Error,
            &config.source,
            format!("failed to install ctx: {error}"),
        ));

        alive.store(false, Ordering::Relaxed);
        return;
    }

    if let Err(error) = lua.load(&source).set_name(&config.script_name).exec() {
        bus.publish(Event::system_log(
            LogLevel::Error,
            &config.source,
            format!("script error: {error}"),
        ));

        alive.store(false, Ordering::Relaxed);
        return;
    }

    let has_callbacks = lua
        .globals()
        .get::<Table>("__plugin_callbacks")
        .map(|table| !table.is_empty())
        .unwrap_or(false);

    let has_timers = lua
        .globals()
        .get::<Table>("__plugin_timers")
        .map(|table| !table.is_empty())
        .unwrap_or(false);

    if !has_callbacks && !has_timers {
        bus.publish(Event::system_log(
            LogLevel::Info,
            &config.source,
            "plugin finished (no callbacks)",
        ));

        alive.store(false, Ordering::Relaxed);
        return;
    }

    loop {
        if stop.load(Ordering::Relaxed) {
            call_disable(&lua, &bus, &config);
            break;
        }

        match event_receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                if let Some(callback) = get_callback(&lua, &event.topic) {
                    let event_table = lua.create_table().ok();

                    if let Some(event_table) = event_table {
                        let _ = event_to_lua_table(&lua, &event_table, &event);

                        if let Err(error) = callback.call::<Value>(event_table) {
                            bus.publish(Event::system_log(
                                LogLevel::Warn,
                                &config.source,
                                format!("on_event error: {error}"),
                            ));
                        }
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }

        process_timers(&lua, &bus, &config);

        let timers_empty = lua
            .globals()
            .get::<Table>("__plugin_timers")
            .map(|table| table.is_empty())
            .unwrap_or(true);

        let callbacks_empty = lua
            .globals()
            .get::<Table>("__plugin_callbacks")
            .map(|table| table.is_empty())
            .unwrap_or(true);

        if timers_empty && callbacks_empty && event_receiver.is_empty() {
            break;
        }
    }

    alive.store(false, Ordering::Relaxed);
}

fn get_callback(lua: &Lua, topic: &str) -> Option<Function> {
    let callbacks: Table = lua.globals().get("__plugin_callbacks").ok()?;

    if let Ok(callback) = callbacks.get::<Function>(topic) {
        return Some(callback);
    }

    for (prefix, function) in callbacks.pairs::<String, Function>().flatten() {
        if topic.starts_with(&prefix) {
            return Some(function);
        }
    }

    None
}

fn process_timers(lua: &Lua, bus: &DataBus, config: &LuaRunConfig) {
    let timers: Table = match lua.globals().get("__plugin_timers") {
        Ok(timers) => timers,
        Err(_) => return,
    };

    let now_ms = tool_core::now_timestamp_ms();
    let mut expired = Vec::new();

    for (id, timer) in timers.pairs::<String, Table>().flatten() {
        let trigger_at_ms: u64 = timer.get("trigger_at_ms").unwrap_or(u64::MAX);

        if now_ms >= trigger_at_ms {
            if let Ok(function) = timer.get::<Function>("callback")
                && let Err(error) = function.call::<()>(())
            {
                bus.publish(Event::system_log(
                    LogLevel::Warn,
                    &config.source,
                    format!("timer error: {error}"),
                ));
            }

            let interval_ms: u64 = timer.get("interval_ms").unwrap_or(0);

            if interval_ms > 0 {
                let _ = timer.set("trigger_at_ms", now_ms + interval_ms);
            } else {
                expired.push(id);
            }
        }
    }

    for id in expired {
        let _ = timers.set(id, Value::Nil);
    }
}

fn call_disable(lua: &Lua, bus: &DataBus, config: &LuaRunConfig) {
    if let Ok(function) = lua.globals().get::<Function>("__plugin_disable")
        && let Err(error) = function.call::<()>(())
    {
        bus.publish(Event::system_log(
            LogLevel::Warn,
            &config.source,
            format!("on_disable error: {error}"),
        ));
    }
}

pub fn run_script_for_test(
    source: &str,
    bus: DataBus,
    transport: TransportManager,
) -> LuaHostResult<()> {
    run_script_blocking(
        source.to_owned(),
        LuaRunConfig::default(),
        bus,
        transport,
        Arc::new(AtomicBool::new(false)),
    )
}

fn run_script_blocking(
    source: String,
    config: LuaRunConfig,
    bus: DataBus,
    transport: TransportManager,
    stop: Arc<AtomicBool>,
) -> LuaHostResult<()> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )?;

    install_ctx(&lua, bus, transport, &config)?;
    install_budget_hook(&lua, config.timeout_ms, stop)?;

    lua.load(&source).set_name(&config.script_name).exec()?;

    Ok(())
}

fn install_budget_hook(lua: &Lua, timeout_ms: u64, stop: Arc<AtomicBool>) -> mlua::Result<()> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));

    lua.set_hook(
        mlua::HookTriggers::new().every_nth_instruction(10_000),
        move |_lua, _debug| {
            if stop.load(Ordering::Relaxed) {
                return Err(mlua::Error::RuntimeError("script stopped".to_owned()));
            }

            if Instant::now() >= deadline {
                return Err(mlua::Error::RuntimeError("script timeout".to_owned()));
            }

            Ok(VmState::Continue)
        },
    )
}

fn install_ctx(
    lua: &Lua,
    bus: DataBus,
    transport: TransportManager,
    config: &LuaRunConfig,
) -> mlua::Result<()> {
    let ctx = lua.create_table()?;

    lua.globals()
        .set("__plugin_callbacks", lua.create_table()?)?;
    lua.globals().set("__plugin_timers", lua.create_table()?)?;
    lua.globals().set("__plugin_storage", lua.create_table()?)?;

    if has_permission(config, "log") {
        ctx.set(
            "log",
            create_log_api(lua, bus.clone(), config.source.clone())?,
        )?;
    }

    if has_permission(config, "bus") {
        ctx.set(
            "bus",
            create_bus_api(lua, bus.clone(), config.source.clone())?,
        )?;
    }

    if has_permission(config, "serial") {
        ctx.set("serial", create_serial_api(lua, bus.clone(), transport)?)?;
    }

    if has_permission(config, "ui") {
        ctx.set(
            "ui",
            create_ui_api(lua, bus.clone(), config.source.clone())?,
        )?;
    }

    if has_permission(config, "timer") {
        ctx.set("timer", create_timer_api(lua)?)?;
    }

    if has_permission(config, "storage") {
        ctx.set("storage", create_storage_api(lua)?)?;
    }

    ctx.set(
        "now_ms",
        lua.create_function(|_lua, ()| Ok(tool_core::now_timestamp_ms()))?,
    )?;

    ctx.set("plugin", json_to_lua_value(lua, &config.context)?)?;

    lua.globals().set("ctx", &ctx)?;

    lua.globals().set(
        "on_disable",
        lua.create_function(|lua, function: Function| {
            lua.globals().set("__plugin_disable", function)
        })?,
    )?;

    if has_permission(config, "testing") {
        install_test_api(lua, &ctx, bus, config)?;
    }

    Ok(())
}

fn create_log_api(lua: &Lua, bus: DataBus, source: String) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    for (name, level) in [
        ("trace", LogLevel::Trace),
        ("debug", LogLevel::Debug),
        ("info", LogLevel::Info),
        ("warn", LogLevel::Warn),
        ("error", LogLevel::Error),
    ] {
        let bus = bus.clone();
        let source = source.clone();

        table.set(
            name,
            lua.create_function(move |_lua, message: String| {
                bus.publish(Event::system_log(level, source.clone(), message));
                Ok(())
            })?,
        )?;
    }

    Ok(table)
}

fn create_bus_api(lua: &Lua, bus: DataBus, source: String) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    let publish_bus = bus.clone();

    table.set(
        "publish",
        lua.create_function(move |_lua, (topic, payload): (String, Value)| {
            let payload = lua_value_to_payload(payload)?;

            publish_bus.publish(Event::new(
                topic,
                source.clone(),
                Direction::Internal,
                payload,
            ));

            Ok(())
        })?,
    )?;

    let history_bus = bus.clone();

    table.set(
        "history",
        lua.create_function(move |lua, topic_prefix: Option<String>| {
            let events = history_bus
                .history()
                .into_iter()
                .filter(|event| {
                    topic_prefix
                        .as_ref()
                        .map(|prefix| event.topic.starts_with(prefix))
                        .unwrap_or(true)
                })
                .rev()
                .take(100)
                .map(|event| {
                    json!({
                        "id": event.id,
                        "timestamp_ms": event.timestamp_ms,
                        "topic": event.topic,
                        "source": event.source,
                        "direction": format!("{:?}", event.direction).to_lowercase(),
                        "payload": payload_to_json(event.payload),
                    })
                })
                .collect::<Vec<_>>();

            json_to_lua_value(lua, &serde_json::Value::Array(events))
        })?,
    )?;

    let wait_bus = bus.clone();

    table.set(
        "wait",
        lua.create_function(move |lua, (topic, timeout_ms): (String, Option<u64>)| {
            wait_for_event(lua, wait_bus.clone(), TopicFilter::exact(topic), timeout_ms)
        })?,
    )?;

    let subscribe_bus = bus.clone();

    table.set(
        "subscribe",
        lua.create_function(
            move |lua, (topic_prefix, timeout_ms): (String, Option<u64>)| {
                wait_for_event(
                    lua,
                    subscribe_bus.clone(),
                    TopicFilter::prefix(topic_prefix),
                    timeout_ms,
                )
            },
        )?,
    )?;

    table.set(
        "on",
        lua.create_function(move |lua, (topic, callback): (String, Function)| {
            let callbacks: Table = lua.globals().get("__plugin_callbacks")?;
            callbacks.set(topic, callback)?;
            Ok(())
        })?,
    )?;

    table.set(
        "off",
        lua.create_function(move |lua, topic: String| {
            let callbacks: Table = lua.globals().get("__plugin_callbacks")?;
            callbacks.set(topic, Value::Nil)?;
            Ok(())
        })?,
    )?;

    Ok(table)
}

fn create_serial_api(lua: &Lua, bus: DataBus, transport: TransportManager) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    let transport_for_list = transport.clone();

    table.set(
        "list",
        lua.create_function(move |lua, ()| {
            let ports = transport_for_list
                .list_serial_ports()
                .map_err(mlua::Error::external)?
                .into_iter()
                .map(|port| json!({ "port_name": port.port_name, "port_type": port.port_type }))
                .collect::<Vec<_>>();

            json_to_lua_value(lua, &serde_json::Value::Array(ports))
        })?,
    )?;

    let transport_for_open = transport.clone();

    table.set(
        "open",
        lua.create_function(move |_lua, config: Value| {
            transport_for_open
                .open_serial(lua_value_to_serial_config(config)?)
                .map_err(mlua::Error::external)
        })?,
    )?;

    let transport_for_close = transport.clone();

    table.set(
        "close",
        lua.create_function(move |_lua, ()| {
            transport_for_close.close_serial();
            Ok(())
        })?,
    )?;

    let transport_for_close_port = transport.clone();

    table.set(
        "close_port",
        lua.create_function(move |_lua, port: String| {
            transport_for_close_port.close_port(&port);
            Ok(())
        })?,
    )?;

    let transport_for_send = transport.clone();

    table.set(
        "send",
        lua.create_function(move |_lua, text: String| {
            transport_for_send
                .send_text(&text)
                .map_err(mlua::Error::external)
        })?,
    )?;

    let transport_for_send_to = transport.clone();

    table.set(
        "send_to",
        lua.create_function(move |_lua, (port, text): (String, String)| {
            transport_for_send_to
                .send_text_to(&port, &text)
                .map_err(mlua::Error::external)
        })?,
    )?;

    let transport_for_send_hex = transport.clone();

    table.set(
        "send_hex",
        lua.create_function(move |_lua, text: String| {
            transport_for_send_hex
                .send_hex(&text)
                .map_err(mlua::Error::external)
        })?,
    )?;

    let transport_for_send_hex_to = transport.clone();

    table.set(
        "send_hex_to",
        lua.create_function(move |_lua, (port, text): (String, String)| {
            transport_for_send_hex_to
                .send_hex_to(&port, &text)
                .map_err(mlua::Error::external)
        })?,
    )?;

    let transport_for_status = transport.clone();

    table.set(
        "status",
        lua.create_function(move |lua, ()| {
            let status = transport_for_status.status();

            json_to_lua_value(
                lua,
                &json!({
                    "open": status.open,
                    "port_name": status.port_name,
                    "baud_rate": status.baud_rate,
                }),
            )
        })?,
    )?;

    let transport_for_status_port = transport.clone();

    table.set(
        "status_port",
        lua.create_function(move |lua, port: String| {
            let status = transport_for_status_port.status_port(&port);

            json_to_lua_value(
                lua,
                &json!({
                    "open": status.open,
                    "port_name": status.port_name,
                    "baud_rate": status.baud_rate,
                }),
            )
        })?,
    )?;

    let transport_for_open_ports = transport.clone();

    table.set(
        "open_ports",
        lua.create_function(move |_lua, ()| Ok(transport_for_open_ports.open_ports()))?,
    )?;

    let expect_bus = bus.clone();

    table.set(
        "expect",
        lua.create_function(move |lua, (pattern, timeout_ms): (String, Option<u64>)| {
            let subscription = expect_bus.subscribe(TopicFilter::exact(topics::SERIAL_RX));
            let deadline = Instant::now() + Duration::from_millis(timeout_ms.unwrap_or(1_000));

            loop {
                let now = Instant::now();

                if now >= deadline {
                    return Ok(Value::Nil);
                }

                let remaining = deadline.saturating_duration_since(now);

                match subscription.recv_timeout(remaining.min(Duration::from_millis(50))) {
                    Ok(event) => {
                        let text = event.payload.text_lossy();

                        if text.contains(&pattern) {
                            return Ok(Value::String(lua.create_string(&text)?));
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        return Ok(Value::Nil);
                    }
                }
            }
        })?,
    )?;

    Ok(table)
}

fn create_storage_api(lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set(
        "get",
        lua.create_function(|lua, key: String| {
            let storage: Table = lua.globals().get("__plugin_storage")?;

            let value: mlua::Result<String> = storage.get(key);

            Ok(match value {
                Ok(value) => Value::String(lua.create_string(&value)?),
                Err(_) => Value::Nil,
            })
        })?,
    )?;

    table.set(
        "set",
        lua.create_function(|lua, (key, value): (String, String)| {
            let storage: Table = lua.globals().get("__plugin_storage")?;
            storage.set(key, value)?;
            Ok(())
        })?,
    )?;

    table.set(
        "keys",
        lua.create_function(|lua, ()| {
            let storage: Table = lua.globals().get("__plugin_storage")?;

            let keys = storage
                .pairs::<String, Value>()
                .filter_map(|pair| pair.ok().map(|(key, _)| key))
                .collect::<Vec<_>>();

            Ok(keys)
        })?,
    )?;

    Ok(table)
}

fn create_timer_api(lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set(
        "after",
        lua.create_function(move |lua, (ms, callback): (u64, Function)| {
            let timers: Table = lua.globals().get("__plugin_timers")?;
            let id = format!("t{}", tool_core::now_timestamp_ms());

            let timer = lua.create_table()?;
            timer.set("trigger_at_ms", tool_core::now_timestamp_ms() + ms)?;
            timer.set("interval_ms", 0_u64)?;
            timer.set("callback", callback)?;

            timers.set(id.clone(), timer)?;

            Ok(id)
        })?,
    )?;

    table.set(
        "every",
        lua.create_function(move |lua, (ms, callback): (u64, Function)| {
            let timers: Table = lua.globals().get("__plugin_timers")?;
            let id = format!("t{}", tool_core::now_timestamp_ms());

            let timer = lua.create_table()?;
            timer.set("trigger_at_ms", tool_core::now_timestamp_ms() + ms)?;
            timer.set("interval_ms", ms)?;
            timer.set("callback", callback)?;

            timers.set(id.clone(), timer)?;

            Ok(id)
        })?,
    )?;

    table.set(
        "cancel",
        lua.create_function(move |lua, id: String| {
            let timers: Table = lua.globals().get("__plugin_timers")?;
            timers.set(id, Value::Nil)?;
            Ok(())
        })?,
    )?;

    Ok(table)
}

fn create_ui_api(lua: &Lua, bus: DataBus, source: String) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    for (name, kind) in [
        ("create_chart", "chart"),
        ("create_form", "form"),
        ("create_attitude", "attitude"),
    ] {
        let bus = bus.clone();
        let source = source.clone();

        table.set(
            name,
            lua.create_function(move |_lua, config: Value| {
                let mut config = ensure_json_object(lua_value_to_json(config)?, name)?;

                config.insert(
                    "kind".to_owned(),
                    serde_json::Value::String(kind.to_owned()),
                );

                ensure_panel_defaults(&mut config, kind)?;

                bus.publish(Event::new(
                    topics::UI_PANEL_CREATE,
                    source.clone(),
                    Direction::Internal,
                    Payload::Json(serde_json::Value::Object(config)),
                ));

                Ok(())
            })?,
        )?;
    }

    let bus_for_remove = bus.clone();
    let source_for_remove = source.clone();

    table.set(
        "remove_panel",
        lua.create_function(move |_lua, panel_id: String| {
            bus_for_remove.publish(Event::new(
                topics::UI_PANEL_REMOVE,
                source_for_remove.clone(),
                Direction::Internal,
                Payload::Json(json!({ "id": panel_id })),
            ));

            Ok(())
        })?,
    )?;

    let bus_for_get = bus;

    table.set(
        "get_panel",
        lua.create_function(move |lua, panel_id: String| {
            let panel = bus_for_get
                .history()
                .into_iter()
                .rev()
                .find(|event| {
                    event.topic == topics::UI_PANEL_CREATE
                        && match &event.payload {
                            Payload::Json(value) => {
                                value.get("id").and_then(|value| value.as_str()) == Some(&panel_id)
                            }
                            _ => false,
                        }
                })
                .and_then(|event| match event.payload {
                    Payload::Json(value) => Some(value),
                    _ => None,
                })
                .unwrap_or(serde_json::Value::Null);

            json_to_lua_value(lua, &panel)
        })?,
    )?;

    Ok(table)
}

fn install_test_api(
    lua: &Lua,
    ctx: &Table,
    bus: DataBus,
    config: &LuaRunConfig,
) -> mlua::Result<()> {
    let host = lua.create_table()?;

    let bus_for_latest = bus.clone();
    let bus_for_packets = bus.clone();
    let bus_for_publish = bus.clone();

    let run_started_ms = tool_core::now_timestamp_ms();
    let run_id = format!("{}-{run_started_ms}", config.script_name);
    let source = config.source.clone();

    host.set("run_id", run_id.clone())?;
    host.set("source", config.source.clone())?;
    host.set("script_name", config.script_name.clone())?;
    host.set("run_started_ms", run_started_ms)?;

    host.set(
        "now_ms",
        lua.create_function(|_lua, ()| Ok(tool_core::now_timestamp_ms()))?,
    )?;

    host.set(
        "latest_event_id",
        lua.create_function(move |_lua, ()| {
            Ok(bus_for_latest
                .history()
                .into_iter()
                .map(|event| event.id)
                .max()
                .unwrap_or_default())
        })?,
    )?;

    host.set(
        "raw_packets_since",
        lua.create_function(move |lua, start_id: u64| {
            let packets = bus_for_packets
                .history()
                .into_iter()
                .filter(|event| {
                    event.id > start_id
                        && matches!(event.topic.as_str(), topics::SERIAL_RX | topics::SERIAL_TX)
                })
                .map(test_packet_from_event)
                .collect::<Vec<_>>();

            json_to_lua_value(
                lua,
                &serde_json::to_value(packets).map_err(mlua::Error::external)?,
            )
        })?,
    )?;

    host.set(
        "publish_report",
        lua.create_function(move |_lua, report: Value| {
            bus_for_publish.publish(Event::new(
                topics::TEST_RESULT,
                source.clone(),
                Direction::Internal,
                Payload::Json(lua_value_to_json(report)?),
            ));

            Ok(())
        })?,
    )?;

    lua.globals().set("__test_host", host)?;

    lua.load(TEST_BOOTSTRAP).set_name("test-bootstrap").exec()?;

    let test: Table = lua.globals().get("test")?;
    ctx.set("test", test)?;

    Ok(())
}

fn test_packet_from_event(event: Event) -> TestPacketLog {
    let payload_text = event.payload.text_lossy();

    let payload_hex = event
        .payload
        .as_bytes()
        .map(|bytes| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    TestPacketLog {
        id: event.id,
        timestamp_ms: event.timestamp_ms,
        topic: event.topic,
        direction: format!("{:?}", event.direction).to_lowercase(),
        payload_text,
        payload_hex,
    }
}

fn event_to_lua_table(lua: &Lua, table: &Table, event: &Event) -> mlua::Result<()> {
    table.set("id", event.id)?;
    table.set("timestamp_ms", event.timestamp_ms)?;
    table.set("topic", event.topic.clone())?;
    table.set("source", event.source.clone())?;
    table.set("direction", format!("{:?}", event.direction).to_lowercase())?;
    table.set("payload", payload_to_lua(lua, &event.payload)?)?;
    table.set("metadata", json_to_lua_value(lua, &event.metadata)?)?;

    Ok(())
}

fn payload_to_lua(lua: &Lua, payload: &Payload) -> mlua::Result<Value> {
    Ok(match payload {
        Payload::Empty => Value::Nil,
        Payload::Bytes(bytes) => Value::String(lua.create_string(bytes)?),
        Payload::Text(text) => Value::String(lua.create_string(text)?),
        Payload::Json(value) => json_to_lua_value(lua, value)?,
    })
}

fn lua_value_to_serial_config(value: Value) -> mlua::Result<SerialConfig> {
    match value {
        Value::String(port_name) => Ok(SerialConfig {
            port_name: port_name.to_str()?.to_owned(),
            ..Default::default()
        }),

        Value::Table(table) => {
            let mut config = SerialConfig {
                port_name: table.get("port_name").or_else(|_| table.get("port"))?,
                ..Default::default()
            };

            if let Ok(value) = table.get::<u32>("baud_rate") {
                config.baud_rate = value;
            } else if let Ok(value) = table.get::<u32>("baud") {
                config.baud_rate = value;
            }

            if let Ok(value) = table.get::<u64>("timeout_ms") {
                config.timeout_ms = value;
            }

            if let Ok(value) = table.get::<String>("data_bits") {
                config.data_bits = parse_data_bits(&value);
            }

            if let Ok(value) = table.get::<String>("stop_bits") {
                config.stop_bits = parse_stop_bits(&value);
            }

            if let Ok(value) = table.get::<String>("parity") {
                config.parity = parse_parity(&value);
            }

            Ok(config)
        }

        other => Err(mlua::Error::RuntimeError(format!(
            "serial.open expects a string or table, got {}",
            other.type_name()
        ))),
    }
}

fn parse_data_bits(value: &str) -> tool_transport::DataBits {
    match value {
        "5" => tool_transport::DataBits::Five,
        "6" => tool_transport::DataBits::Six,
        "7" => tool_transport::DataBits::Seven,
        _ => tool_transport::DataBits::Eight,
    }
}

fn parse_stop_bits(value: &str) -> tool_transport::StopBits {
    match value {
        "2" => tool_transport::StopBits::Two,
        _ => tool_transport::StopBits::One,
    }
}

fn parse_parity(value: &str) -> tool_transport::Parity {
    match value {
        "odd" => tool_transport::Parity::Odd,
        "even" => tool_transport::Parity::Even,
        _ => tool_transport::Parity::None,
    }
}

fn lua_value_to_payload(value: Value) -> mlua::Result<Payload> {
    Ok(match value {
        Value::Nil => Payload::Empty,
        Value::Boolean(value) => Payload::Json(serde_json::Value::Bool(value)),
        Value::Integer(value) => Payload::Json(serde_json::Value::Number(value.into())),
        Value::Number(value) => Payload::Json(number_to_json(value)?),
        Value::String(value) => Payload::Text(value.to_str()?.to_owned()),
        Value::Table(value) => Payload::Json(lua_table_to_json(value)?),
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "unsupported payload: {}",
                other.type_name()
            )));
        }
    })
}

fn lua_table_to_json(table: Table) -> mlua::Result<serde_json::Value> {
    let mut entries = Vec::new();
    let mut is_array = true;
    let mut max_index = 0_i64;

    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;

        if let Value::Integer(index) = key
            && index > 0
        {
            max_index = max_index.max(index);
            entries.push((key, lua_value_to_json(value)?));
            continue;
        }

        is_array = false;
        entries.push((key, lua_value_to_json(value)?));
    }

    if is_array && max_index as usize == entries.len() {
        entries.sort_by_key(|(key, _)| match key {
            Value::Integer(index) => *index,
            _ => 0,
        });

        return Ok(serde_json::Value::Array(
            entries.into_iter().map(|(_, value)| value).collect(),
        ));
    }

    let mut object = Map::new();

    for (key, value) in entries {
        object.insert(lua_key_to_string(key)?, value);
    }

    Ok(serde_json::Value::Object(object))
}

fn lua_value_to_json(value: Value) -> mlua::Result<serde_json::Value> {
    Ok(match value {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(value) => serde_json::Value::Bool(value),
        Value::Integer(value) => serde_json::Value::Number(value.into()),
        Value::Number(value) => number_to_json(value)?,
        Value::String(value) => serde_json::Value::String(value.to_str()?.to_owned()),
        Value::Table(value) => lua_table_to_json(value)?,
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "unsupported value: {}",
                other.type_name()
            )));
        }
    })
}

fn lua_key_to_string(key: Value) -> mlua::Result<String> {
    Ok(match key {
        Value::String(value) => value.to_str()?.to_owned(),
        Value::Integer(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "unsupported key: {}",
                other.type_name()
            )));
        }
    })
}

fn number_to_json(value: f64) -> mlua::Result<serde_json::Value> {
    Number::from_f64(value)
        .map(serde_json::Value::Number)
        .ok_or_else(|| mlua::Error::RuntimeError("number is not finite".to_owned()))
}

fn payload_to_json(payload: Payload) -> serde_json::Value {
    match payload {
        Payload::Empty => serde_json::Value::Null,
        Payload::Bytes(bytes) => serde_json::Value::Array(
            bytes
                .into_iter()
                .map(|byte| serde_json::Value::Number(byte.into()))
                .collect(),
        ),
        Payload::Text(text) => serde_json::Value::String(text),
        Payload::Json(value) => value,
    }
}

fn json_to_lua_value(lua: &Lua, value: &serde_json::Value) -> mlua::Result<Value> {
    Ok(match value {
        serde_json::Value::Null => Value::Nil,

        serde_json::Value::Bool(value) => Value::Boolean(*value),

        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Value::Integer(value)
            } else if let Some(value) = value.as_f64() {
                Value::Number(value)
            } else {
                Value::Nil
            }
        }

        serde_json::Value::String(value) => Value::String(lua.create_string(value)?),

        serde_json::Value::Array(values) => {
            let table = lua.create_table()?;

            for (index, value) in values.iter().enumerate() {
                table.set(index + 1, json_to_lua_value(lua, value)?)?;
            }

            Value::Table(table)
        }

        serde_json::Value::Object(values) => {
            let table = lua.create_table()?;

            for (key, value) in values {
                table.set(key.as_str(), json_to_lua_value(lua, value)?)?;
            }

            Value::Table(table)
        }
    })
}

fn ensure_json_object(
    value: serde_json::Value,
    function_name: &str,
) -> mlua::Result<Map<String, serde_json::Value>> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| mlua::Error::RuntimeError(format!("ctx.ui.{function_name} expects a table")))
}

fn ensure_panel_defaults(
    config: &mut Map<String, serde_json::Value>,
    fallback_kind: &str,
) -> mlua::Result<()> {
    if !config.contains_key("id") {
        return Err(mlua::Error::RuntimeError(
            "panel config requires id".to_owned(),
        ));
    }

    if !config.contains_key("title") {
        let title = config
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or(fallback_kind)
            .to_owned();

        config.insert("title".to_owned(), serde_json::Value::String(title));
    }

    if fallback_kind == "chart" && !config.contains_key("topic_prefix") {
        config.insert(
            "topic_prefix".to_owned(),
            serde_json::Value::String("protocol.".to_owned()),
        );
    }

    if fallback_kind == "form" && !config.contains_key("fields") {
        config.insert("fields".to_owned(), serde_json::Value::Array(Vec::new()));
    }

    if fallback_kind == "attitude" && !config.contains_key("topic") {
        config.insert(
            "topic".to_owned(),
            serde_json::Value::String(topics::PROTOCOL_IMU_ATTITUDE.to_owned()),
        );
    }

    Ok(())
}

fn wait_for_event(
    lua: &Lua,
    bus: DataBus,
    filter: TopicFilter,
    timeout_ms: Option<u64>,
) -> mlua::Result<Value> {
    let subscription = bus.subscribe(filter);

    match subscription.recv_timeout(Duration::from_millis(timeout_ms.unwrap_or(1_000))) {
        Ok(event) => {
            let table = lua.create_table()?;
            event_to_lua_table(lua, &table, &event)?;
            Ok(Value::Table(table))
        }
        Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(Value::Nil),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Ok(Value::Nil),
    }
}

const TEST_BOOTSTRAP: &str = r#"
local host = __test_host
local test = {
  _cases = {},
  _before_each = nil,
  _after_each = nil,
  _timeout_ms = 5000,
  _current = nil
}

function test.timeout(ms)
  test._timeout_ms = ms
end

function test.before_each(fn)
  test._before_each = fn
end

function test.after_each(fn)
  test._after_each = fn
end

function test.log(message)
  local text = tostring(message)
  if test._current then
    table.insert(test._current.logs, text)
  end
  if ctx.log then
    ctx.log.info(text)
  end
end

function test.assert(condition, message)
  if test._current then
    test._current.assertions = test._current.assertions + 1
  end
  if not condition then
    error(message or "assertion failed", 2)
  end
end

function test.expect(topic, timeout_ms)
  return ctx.bus.wait(topic, timeout_ms or test._timeout_ms)
end

local function publish_report()
  host.publish_report({
    run_id = host.run_id,
    source = host.source,
    script_name = host.script_name,
    started_ms = host.run_started_ms,
    finished_ms = host.now_ms(),
    cases = test._cases
  })
end

function test.case(name, fn)
  local start_event_id = host.latest_event_id()
  local started = host.now_ms()
  local case = {
    name = name,
    status = "passed",
    duration_ms = 0,
    logs = {},
    assertions = 0,
    error = nil,
    raw_packets = {}
  }

  test._current = case
  local ok, err = pcall(function()
    if test._before_each then test._before_each() end
    fn()
  end)

  local after_ok, after_err = true, nil
  if test._after_each then
    after_ok, after_err = pcall(test._after_each)
  end

  case.duration_ms = host.now_ms() - started
  case.raw_packets = host.raw_packets_since(start_event_id)

  if not ok then
    case.status = "failed"
    case.error = tostring(err)
  elseif not after_ok then
    case.status = "failed"
    case.error = tostring(after_err)
  end

  test._current = nil
  table.insert(test._cases, case)
  publish_report()
end

_G.test = test
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use tool_databus::TopicFilter;

    #[test]
    fn lua_log_reaches_databus() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let logs = bus.subscribe(TopicFilter::prefix("log."));

        run_script_for_test("ctx.log.info('hello from lua')", bus, transport).unwrap();

        let events = logs.drain();

        assert!(
            events.iter().any(|event| event.source == "lua"
                && event.payload.text_lossy().contains("hello from lua"))
        );
    }

    #[test]
    fn lua_bus_publish_accepts_tables() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::PROTOCOL_PID_SAMPLE));

        run_script_for_test(
            "ctx.bus.publish('protocol.pid.sample', { t = 1, target = 2.5, actual = 2.0 })",
            bus,
            transport,
        )
        .unwrap();

        let event = rx.drain().pop().unwrap();

        assert_eq!(event.topic, topics::PROTOCOL_PID_SAMPLE);

        assert_eq!(
            event.payload.text_lossy(),
            r#"{"actual":2.0,"t":1,"target":2.5}"#
        );
    }

    #[test]
    fn lua_timeout_stops_busy_loop() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());

        let result = run_script_blocking(
            "while true do end".to_owned(),
            LuaRunConfig {
                script_name: "loop.lua".to_owned(),
                timeout_ms: 20,
                source: "lua".to_owned(),
                context: json!({}),
                permissions: default_lua_permissions(),
            },
            bus,
            transport,
            Arc::new(AtomicBool::new(false)),
        );

        assert!(result.is_err());
    }

    #[test]
    fn lua_bus_wait_receives_later_event() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let publisher = bus.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));

            publisher.publish(Event::new(
                "test.ready",
                "test",
                Direction::Internal,
                Payload::Text("ready".to_owned()),
            ));
        });

        run_script_for_test(
            "local event = ctx.bus.wait('test.ready', 500)\nassert(event.payload == 'ready')",
            bus,
            transport,
        )
        .unwrap();
    }

    #[test]
    fn lua_serial_expect_matches_rx_text() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let publisher = bus.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            publisher.publish(Event::serial_rx("test", b"READY\r\n".to_vec()));
        });

        run_script_for_test(
            "local line = ctx.serial.expect('READY', 500)\nassert(line == 'READY\\r\\n')",
            bus,
            transport,
        )
        .unwrap();
    }

    #[test]
    fn lua_ui_create_chart_publishes_panel_event() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::UI_PANEL_CREATE));

        run_script_for_test(
            "ctx.ui.create_chart({ id = 'pid-chart', title = 'PID Chart', topic_prefix = 'protocol.pid.' })",
            bus,
            transport,
        )
        .unwrap();

        let event = rx.drain().pop().unwrap();

        assert_eq!(event.topic, topics::UI_PANEL_CREATE);
        assert!(event.payload.text_lossy().contains("pid-chart"));
    }

    #[test]
    fn lua_ui_create_attitude_publishes_panel_event() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::UI_PANEL_CREATE));

        run_script_for_test(
            "ctx.ui.create_attitude({ id = 'imu-attitude', title = 'IMU Attitude' })",
            bus,
            transport,
        )
        .unwrap();

        let event = rx.drain().pop().unwrap();

        assert_eq!(event.topic, topics::UI_PANEL_CREATE);
        assert!(event.payload.text_lossy().contains("imu-attitude"));
    }

    #[test]
    fn lua_test_case_publishes_passed_report() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::TEST_RESULT));

        run_script_for_test(
            "test.case('math works', function()\n  test.assert(1 + 1 == 2, 'math broke')\nend)",
            bus,
            transport,
        )
        .unwrap();

        let event = rx.drain().pop().unwrap();

        let report: serde_json::Value = match event.payload {
            Payload::Json(value) => value,
            _ => panic!(),
        };

        assert_eq!(report["cases"][0]["name"], "math works");
        assert_eq!(report["cases"][0]["status"], "passed");
    }

    #[test]
    fn lua_test_case_publishes_failed_report() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::TEST_RESULT));

        run_script_for_test(
            "test.case('fails clearly', function()\n  test.assert(false, 'expected failure')\nend)",
            bus,
            transport,
        )
        .unwrap();

        let event = rx.drain().pop().unwrap();

        let report: serde_json::Value = match event.payload {
            Payload::Json(value) => value,
            _ => panic!(),
        };

        assert_eq!(report["cases"][0]["status"], "failed");

        assert!(
            report["cases"][0]["error"]
                .as_str()
                .unwrap()
                .contains("expected failure")
        );
    }

    #[test]
    fn lua_test_case_associates_raw_packets() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::TEST_RESULT));

        let publisher = bus.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            publisher.publish(Event::serial_rx("test", b"OK\r\n".to_vec()));
        });

        run_script_for_test(
            "test.case('waits for serial', function()\n  local line = ctx.serial.expect('OK', 500)\n  test.assert(line ~= nil, 'missing serial response')\nend)",
            bus,
            transport,
        )
        .unwrap();

        let event = rx.drain().pop().unwrap();

        let report: serde_json::Value = match event.payload {
            Payload::Json(value) => value,
            _ => panic!(),
        };

        assert_eq!(report["cases"][0]["status"], "passed");
        assert_eq!(
            report["cases"][0]["raw_packets"][0]["payload_text"],
            "OK\r\n"
        );
    }
}
