//! Pure-Rust Lua engine for the browser.
//!
//! This module is intentionally target-gated: Native keeps the mature mlua
//! runtime while the browser uses omniLua, which has no C/FFI dependency and
//! can be built for `wasm32-unknown-unknown`. Both engines expose the same
//! `PluginValue`/`PluginHostApi` protocol.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use omnilua::{
    Error as LuaError, Function, Lua, LuaError as InnerLuaError, SandboxConfig, Table, Thread,
    ThreadStatus, Value, Variadic,
};
use tool_plugin_api::{
    CoroutineId, FileHandle, LuaEngine, PluginCallResult, PluginError, PluginFunctionId,
    PluginHostApi, PluginHostRequest, PluginInstanceId, PluginLoadConfig, PluginResult,
    PluginSerialSettings, PluginUiCommand, PluginValue,
};

struct WebLuaInstance {
    lua: Lua,
    host: Rc<dyn PluginHostApi>,
    line_buffers: Rc<RefCell<BTreeMap<String, VecDeque<String>>>>,
    bus_events: VecDeque<PluginValue>,
    replay_outputs: Option<Vec<String>>,
    replay_buffers: Option<Rc<ReplayBuffers>>,
}

#[derive(Default)]
struct ReplayBuffers {
    events: RefCell<Vec<PluginValue>>,
    logs: RefCell<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct WebReplayOutput {
    pub events: Vec<PluginValue>,
    pub logs: Vec<String>,
}

/// Single-threaded browser Lua engine.
///
/// The app drives this engine from its normal repaint/event loop. Browser
/// operations are represented by host requests and are never implemented as
/// filesystem/process access inside the VM.
pub struct WebLuaEngine {
    next_instance: u64,
    instances: BTreeMap<PluginInstanceId, WebLuaInstance>,
}

impl Default for WebLuaEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WebLuaEngine {
    pub fn new() -> Self {
        Self {
            next_instance: 1,
            instances: BTreeMap::new(),
        }
    }

    fn instance(&self, id: PluginInstanceId) -> PluginResult<&WebLuaInstance> {
        self.instances
            .get(&id)
            .ok_or_else(|| PluginError::Runtime(format!("unknown plugin instance {}", id.0)))
    }

    fn instance_mut(&mut self, id: PluginInstanceId) -> PluginResult<&mut WebLuaInstance> {
        self.instances
            .get_mut(&id)
            .ok_or_else(|| PluginError::Runtime(format!("unknown plugin instance {}", id.0)))
    }
}

impl LuaEngine for WebLuaEngine {
    fn load_plugin(
        &mut self,
        source: &str,
        config: PluginLoadConfig,
        host: Rc<dyn PluginHostApi>,
    ) -> PluginResult<PluginInstanceId> {
        let (lua, _sandbox) = Lua::sandboxed(SandboxConfig {
            instruction_limit: Some(10_000_000),
            memory_limit_bytes: Some(64 * 1024 * 1024),
            check_interval: 1_000,
            remove_globals: vec![
                b"dofile".to_vec(),
                b"loadfile".to_vec(),
                b"os.execute".to_vec(),
                b"io".to_vec(),
            ],
        })
        .map_err(lua_error)?;

        install_codec_module(&lua).map_err(lua_error)?;
        let line_buffers = Rc::new(RefCell::new(BTreeMap::new()));
        install_ctx(&lua, &config, host.clone(), line_buffers.clone()).map_err(lua_error)?;
        lua.load(source)
            .set_name(config.script_name.as_bytes())
            .exec()
            .map_err(lua_error)?;

        let id = PluginInstanceId(self.next_instance);
        self.next_instance = self.next_instance.saturating_add(1);
        self.instances.insert(
            id,
            WebLuaInstance {
                lua,
                host,
                line_buffers,
                bus_events: VecDeque::new(),
                replay_outputs: None,
                replay_buffers: None,
            },
        );
        Ok(id)
    }

    fn call(
        &mut self,
        instance: PluginInstanceId,
        function: PluginFunctionId,
        args: &[PluginValue],
    ) -> PluginResult<PluginCallResult> {
        let instance = self.instance(instance)?;
        let function: Function = instance
            .lua
            .globals()
            .get(function.0.as_str())
            .map_err(lua_error)?;
        let args = args
            .iter()
            .map(|value| value_to_lua(&instance.lua, value))
            .collect::<Result<Vec<_>, _>>()
            .map_err(lua_error)?;
        let returns: Vec<Value> = function
            .call(omnilua::Variadic::from(args))
            .map_err(lua_error)?;
        Ok(PluginCallResult::Completed(
            values_to_plugin(&returns).map_err(lua_error)?,
        ))
    }

    fn resume(
        &mut self,
        _coroutine: CoroutineId,
        _value: PluginValue,
    ) -> PluginResult<PluginCallResult> {
        Err(PluginError::UnsupportedCapability(
            "coroutine resume is not wired to the Web scheduler yet".to_owned(),
        ))
    }

    fn stop(&mut self, instance: PluginInstanceId) -> PluginResult<()> {
        let instance_value = self.instances.remove(&instance).ok_or_else(|| {
            PluginError::Runtime(format!("unknown plugin instance {}", instance.0))
        })?;
        if let Ok(callback) = instance_value
            .lua
            .globals()
            .get::<_, Function>("__plugin_disable")
        {
            callback.call::<_, Value>(()).map_err(lua_error)?;
        }
        Ok(())
    }

    fn dispatch_event(
        &mut self,
        instance: PluginInstanceId,
        event: PluginValue,
    ) -> PluginResult<()> {
        let instance = self.instance_mut(instance)?;
        ingest_serial_event(instance, &event);
        if instance.bus_events.len() >= 4096 {
            instance.bus_events.pop_front();
        }
        instance.bus_events.push_back(event.clone());
        let handlers: Table = instance
            .lua
            .globals()
            .get("__plugin_bus_handlers")
            .map_err(lua_error)?;
        let topic = match &event {
            PluginValue::Object(values) => values
                .get("topic")
                .and_then(|value| match value {
                    PluginValue::String(value) => Some(value.as_str()),
                    _ => None,
                })
                .unwrap_or_default(),
            _ => "",
        };
        let event = value_to_lua(&instance.lua, &event).map_err(lua_error)?;
        for pair in handlers.raw_pairs().map_err(lua_error)? {
            let (_, value) = pair;
            let entry: Table = match value {
                Value::Table(value) => value,
                _ => continue,
            };
            let pattern: String = entry.get("topic").map_err(lua_error)?;
            if !topic_matches(&pattern, topic) {
                continue;
            }
            let callback: Function = entry.get("callback").map_err(lua_error)?;
            callback
                .call::<_, Value>(event.clone())
                .map_err(lua_error)?;
        }
        resume_ready_tasks(instance)?;
        Ok(())
    }

    fn dispatch_command(
        &mut self,
        instance: PluginInstanceId,
        command: &str,
        context: PluginValue,
    ) -> PluginResult<PluginCallResult> {
        let instance = self.instance(instance)?;
        let commands: Table = instance
            .lua
            .globals()
            .get("__plugin_commands")
            .map_err(lua_error)?;
        let callback: Function = commands.get(command).map_err(lua_error)?;
        let context = value_to_lua(&instance.lua, &context).map_err(lua_error)?;
        let result: Value = callback.call(context).map_err(lua_error)?;
        Ok(PluginCallResult::Completed(
            value_from_lua(result).map_err(lua_error)?,
        ))
    }

    fn update_settings(
        &mut self,
        instance: PluginInstanceId,
        settings: PluginValue,
    ) -> PluginResult<()> {
        let instance = self.instance(instance)?;
        let settings = value_to_lua(&instance.lua, &settings).map_err(lua_error)?;
        if let Ok(callback) = instance.lua.globals().get::<_, Function>("on_settings") {
            callback.call::<_, Value>(settings).map_err(lua_error)?;
        }
        Ok(())
    }
}

impl WebLuaEngine {
    /// Load the optional `replay.main` script from the same Lua plugin
    /// package.  Replay deliberately gets a restricted context: it can read
    /// replay events and emit derived events, but cannot touch live serial,
    /// UI, timers, or the browser host.
    pub fn load_replay_plugin(
        &mut self,
        source: &str,
        config: PluginLoadConfig,
        outputs: Vec<String>,
        host: Rc<dyn PluginHostApi>,
    ) -> PluginResult<PluginInstanceId> {
        let (lua, _sandbox) = Lua::sandboxed(SandboxConfig {
            instruction_limit: Some(10_000_000),
            memory_limit_bytes: Some(64 * 1024 * 1024),
            check_interval: 1_000,
            remove_globals: vec![
                b"dofile".to_vec(),
                b"loadfile".to_vec(),
                b"os.execute".to_vec(),
                b"io".to_vec(),
            ],
        })
        .map_err(lua_error)?;
        install_codec_module(&lua).map_err(lua_error)?;
        let buffers = Rc::new(ReplayBuffers::default());
        install_replay_ctx(
            &lua,
            &config,
            outputs.clone(),
            host.clone(),
            buffers.clone(),
        )
        .map_err(lua_error)?;
        lua.load(source)
            .set_name(config.script_name.as_bytes())
            .exec()
            .map_err(lua_error)?;

        let id = PluginInstanceId(self.next_instance);
        self.next_instance = self.next_instance.saturating_add(1);
        self.instances.insert(
            id,
            WebLuaInstance {
                lua,
                host,
                line_buffers: Rc::new(RefCell::new(BTreeMap::new())),
                bus_events: VecDeque::new(),
                replay_outputs: Some(outputs),
                replay_buffers: Some(buffers),
            },
        );
        Ok(id)
    }

    pub fn replay_begin(
        &mut self,
        instance: PluginInstanceId,
        session: PluginValue,
    ) -> PluginResult<()> {
        let instance = self.instance_mut(instance)?;
        if instance.replay_outputs.is_none() {
            return Err(PluginError::Runtime("不是 Replay Lua 实例".to_owned()));
        }
        instance
            .lua
            .globals()
            .set("__replay_current_event", Value::Nil)
            .map_err(lua_error)?;
        let session = value_to_lua(&instance.lua, &session).map_err(lua_error)?;
        if let Ok(function) = instance.lua.globals().get::<_, Function>("on_replay_begin") {
            function.call::<_, Value>(session).map_err(lua_error)?;
        }
        Ok(())
    }

    pub fn replay_event(
        &mut self,
        instance: PluginInstanceId,
        event: PluginValue,
    ) -> PluginResult<()> {
        let instance = self.instance_mut(instance)?;
        if instance.replay_outputs.is_none() {
            return Err(PluginError::Runtime("不是 Replay Lua 实例".to_owned()));
        }
        let (timestamp_ms, id) = match &event {
            PluginValue::Object(values) => (
                values
                    .get("timestamp_ms")
                    .and_then(|value| match value {
                        PluginValue::Integer(value) if *value >= 0 => Some(*value as u64),
                        _ => None,
                    })
                    .unwrap_or_default(),
                values
                    .get("id")
                    .and_then(|value| match value {
                        PluginValue::Integer(value) if *value >= 0 => Some(*value as u64),
                        _ => None,
                    })
                    .unwrap_or_default(),
            ),
            _ => (0, 0),
        };
        instance
            .lua
            .globals()
            .set("__replay_current_ts", timestamp_ms as i64)
            .map_err(lua_error)?;
        instance
            .lua
            .globals()
            .set("__replay_current_id", id as i64)
            .map_err(lua_error)?;
        let event_value = value_to_lua(&instance.lua, &event).map_err(lua_error)?;
        instance
            .lua
            .globals()
            .set("__replay_current_event", event_value.clone())
            .map_err(lua_error)?;
        if let Ok(function) = instance.lua.globals().get::<_, Function>("on_replay_event") {
            function.call::<_, Value>(event_value).map_err(lua_error)?;
        }
        Ok(())
    }

    pub fn replay_end(&mut self, instance: PluginInstanceId) -> PluginResult<WebReplayOutput> {
        let instance = self.instance_mut(instance)?;
        if instance.replay_outputs.is_none() {
            return Err(PluginError::Runtime("不是 Replay Lua 实例".to_owned()));
        }
        if let Ok(function) = instance.lua.globals().get::<_, Function>("on_replay_end") {
            function.call::<_, Value>(()).map_err(lua_error)?;
        }
        let buffers = instance
            .replay_buffers
            .as_ref()
            .expect("replay instance has buffers");
        Ok(WebReplayOutput {
            events: std::mem::take(&mut *buffers.events.borrow_mut()),
            logs: std::mem::take(&mut *buffers.logs.borrow_mut()),
        })
    }

    /// Advance browser Lua tasks without blocking the UI thread.
    pub fn tick(&mut self) -> PluginResult<()> {
        let ids = self.instances.keys().copied().collect::<Vec<_>>();
        for id in ids {
            let instance = self.instance_mut(id)?;
            process_timers(instance)?;
            resume_ready_tasks(instance)?;
        }
        Ok(())
    }
}

fn ingest_serial_event(instance: &mut WebLuaInstance, event: &PluginValue) {
    let PluginValue::Object(event) = event else {
        return;
    };
    if !matches!(
        event.get("topic"),
        Some(PluginValue::String(topic)) if topic == "transport.serial.default.rx"
    ) {
        return;
    }
    let port = event
        .get("metadata")
        .and_then(|metadata| match metadata {
            PluginValue::Object(metadata) => metadata.get("port"),
            _ => None,
        })
        .and_then(|port| match port {
            PluginValue::String(port) => Some(port.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "default".to_owned());
    let Some(payload) = event.get("payload") else {
        return;
    };
    let text = match payload {
        PluginValue::String(text) => text.clone(),
        PluginValue::Array(bytes) => {
            let bytes = bytes
                .iter()
                .filter_map(|value| match value {
                    PluginValue::Integer(value) => u8::try_from(*value).ok(),
                    _ => None,
                })
                .collect::<Vec<_>>();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        _ => return,
    };
    let mut buffers = instance.line_buffers.borrow_mut();
    let lines = buffers.entry(port).or_default();
    for line in text.split_inclusive(['\r', '\n']) {
        let line = line.trim_end_matches(['\r', '\n']);
        if !line.is_empty() {
            lines.push_back(line.to_owned());
        }
    }
}

fn resume_ready_tasks(instance: &mut WebLuaInstance) -> PluginResult<()> {
    for completion in instance.host.take_completions() {
        let tasks: Table = instance
            .lua
            .globals()
            .get("__plugin_tasks")
            .map_err(lua_error)?;
        for (_, value) in tasks.raw_pairs().map_err(lua_error)? {
            let Value::Table(state) = value else {
                continue;
            };
            let Ok(op) = state.get::<_, Table>("yield_op") else {
                continue;
            };
            if op.get::<_, String>("request_id").ok().as_deref()
                != Some(completion.request_id.as_str())
            {
                continue;
            }
            match completion.result {
                Ok(value) => state
                    .set(
                        "_host_result",
                        value_to_lua(&instance.lua, &value).map_err(lua_error)?,
                    )
                    .map_err(lua_error)?,
                Err(error) => state.set("_host_result_err", error).map_err(lua_error)?,
            }
            break;
        }
    }
    let tasks: Table = instance
        .lua
        .globals()
        .get("__plugin_tasks")
        .map_err(lua_error)?;
    let entries = tasks.raw_pairs().map_err(lua_error)?;
    for (id, value) in entries {
        let Value::String(id) = id else {
            continue;
        };
        let id = id.to_str().map_err(lua_error)?;
        let Value::Table(state) = value else {
            continue;
        };
        if state.get::<_, bool>("finished").unwrap_or(true) {
            continue;
        }
        let thread: Thread = match state.get::<_, Value>("thread") {
            Ok(Value::Thread(thread)) => thread,
            Err(_) => continue,
            Ok(_) => continue,
        };
        if thread.status().map_err(lua_error)? != ThreadStatus::Suspended {
            continue;
        }
        let op: Table = match state.get("yield_op") {
            Ok(op) => op,
            Err(_) => continue,
        };
        let kind: String = op.get("kind").map_err(lua_error)?;
        let now = instance.host.now_ms()?;
        let cancelled = state.get::<_, bool>("cancelled").unwrap_or(false);
        let deadline = op.get::<_, u64>("deadline_ms").unwrap_or_default();
        let ready = if cancelled {
            set_task_error(&state, &kind, &instance.lua, "cancelled").map_err(lua_error)?;
            true
        } else if deadline > 0 && now >= deadline {
            set_task_error(&state, &kind, &instance.lua, "timeout").map_err(lua_error)?;
            true
        } else if kind == "host" {
            !matches!(
                state.get::<_, Value>("_host_result"),
                Ok(Value::Nil) | Err(_)
            ) || !matches!(
                state.get::<_, Value>("_host_result_err"),
                Ok(Value::Nil) | Err(_)
            )
        } else {
            try_complete_wait(instance, &state, &op, &kind)?
        };
        if !ready {
            continue;
        }
        state.set("yield_op", Value::Nil).map_err(lua_error)?;
        instance
            .lua
            .globals()
            .set("__current_task_id", id.as_str())
            .map_err(lua_error)?;
        let result = thread.resume::<_, Value>(Value::Nil);
        instance
            .lua
            .globals()
            .set("__current_task_id", Value::Nil)
            .map_err(lua_error)?;
        match result {
            Ok(_) if thread.status().map_err(lua_error)? == ThreadStatus::Dead => {
                state.set("finished", true).map_err(lua_error)?;
            }
            Ok(_) => {}
            Err(error) => {
                state.set("finished", true).map_err(lua_error)?;
                state.set("error", error.to_string()).map_err(lua_error)?;
            }
        }
    }
    Ok(())
}

fn process_timers(instance: &mut WebLuaInstance) -> PluginResult<()> {
    let timers: Table = instance
        .lua
        .globals()
        .get("__plugin_timers")
        .map_err(lua_error)?;
    let now = instance.host.now_ms()?;
    let entries = timers.raw_pairs().map_err(lua_error)?;
    for (key, value) in entries {
        let Value::Integer(id) = key else {
            continue;
        };
        let Value::Table(timer) = value else {
            continue;
        };
        if now < timer.get::<_, u64>("next_ms").unwrap_or(now) {
            continue;
        }
        let callback: Function = match timer.get("callback") {
            Ok(callback) => callback,
            Err(_) => continue,
        };
        callback.call::<_, Value>(()).map_err(lua_error)?;
        if timer.get::<_, bool>("repeat").unwrap_or(false) {
            let interval = timer.get::<_, u64>("interval_ms").unwrap_or(1).max(1);
            timer
                .set("next_ms", now.saturating_add(interval))
                .map_err(lua_error)?;
        } else {
            timers.set(id, Value::Nil).map_err(lua_error)?;
        }
    }
    Ok(())
}

fn try_complete_wait(
    instance: &mut WebLuaInstance,
    state: &Table,
    op: &Table,
    kind: &str,
) -> PluginResult<bool> {
    if kind == "paused" {
        return Ok(!state.get::<_, bool>("paused").unwrap_or(false));
    }
    if kind == "bus_wait" {
        let topic = op.get::<_, String>("topic").map_err(lua_error)?;
        let Some(index) = instance.bus_events.iter().position(|event| {
            matches!(event, PluginValue::Object(values) if values.get("topic").is_some_and(|value| match value {
                PluginValue::String(value) => topic_matches(&topic, value),
                _ => false,
            }))
        }) else {
            return Ok(false);
        };
        let event = instance
            .bus_events
            .remove(index)
            .unwrap_or(PluginValue::Null);
        state
            .set(
                "_bus_result",
                value_to_lua(&instance.lua, &event).map_err(lua_error)?,
            )
            .map_err(lua_error)?;
        return Ok(true);
    }
    let port = op.get::<_, String>("port").map_err(lua_error)?;
    let mut buffers = instance.line_buffers.borrow_mut();
    let Some(lines) = buffers.get_mut(&port) else {
        return Ok(false);
    };
    if kind == "read_line" {
        let Some(line) = lines.pop_front() else {
            return Ok(false);
        };
        state
            .set(
                "_read_result",
                instance.lua.create_string(line).map_err(lua_error)?,
            )
            .map_err(lua_error)?;
        return Ok(true);
    }
    if !matches!(kind, "expect" | "write_line_and_expect") {
        return Ok(true);
    }
    let patterns = match op.get::<_, Table>("patterns") {
        Ok(patterns) => patterns,
        Err(_) => return Ok(false),
    };
    while let Some(line) = lines.pop_front() {
        let mut matched = None;
        let mut continued = false;
        for (_, value) in patterns.raw_pairs().map_err(lua_error)? {
            let Value::Table(pattern) = value else {
                continue;
            };
            let pattern_text: String = pattern.get("pattern").unwrap_or_default();
            if !match_pattern(&line, &pattern_text) {
                continue;
            }
            let action: String = pattern
                .get("action")
                .unwrap_or_else(|_| "return".to_owned());
            if action == "continue" {
                continued = true;
                break;
            }
            matched = Some((pattern, line.clone()));
            break;
        }
        if continued {
            if op
                .get::<_, bool>("continue_resets_timeout")
                .unwrap_or(false)
            {
                let timeout = op.get::<_, u64>("timeout_ms").unwrap_or_default();
                op.set(
                    "deadline_ms",
                    instance.host.now_ms().unwrap_or_default() + timeout,
                )
                .map_err(lua_error)?;
            }
            continue;
        }
        if let Some((pattern, line)) = matched {
            let result = instance.lua.create_table().map_err(lua_error)?;
            result
                .set("name", pattern.get::<_, String>("name").unwrap_or_default())
                .map_err(lua_error)?;
            result.set("line", line).map_err(lua_error)?;
            result.set("elapsed_ms", 0_i64).map_err(lua_error)?;
            state.set("_expect_result", result).map_err(lua_error)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn set_task_error(state: &Table, kind: &str, lua: &Lua, error: &str) -> omnilua::Result<()> {
    match kind {
        "read_line" => state.set("_read_result_err", lua.create_string(error)?)?,
        "expect" | "write_line_and_expect" => {
            state.set("_expect_err", lua.create_string(error)?)?
        }
        "bus_wait" => state.set("_bus_result_err", lua.create_string(error)?)?,
        "host" => state.set("_host_result_err", lua.create_string(error)?)?,
        _ => {}
    }
    Ok(())
}

fn match_pattern(line: &str, pattern: &str) -> bool {
    if let Some(pattern) = pattern.strip_prefix("re:") {
        return regex::Regex::new(pattern)
            .map(|regex| regex.is_match(line))
            .unwrap_or(false);
    }
    pattern
        .strip_prefix('^')
        .map(|pattern| line.starts_with(pattern))
        .unwrap_or_else(|| line.contains(pattern))
}

fn install_replay_ctx(
    lua: &Lua,
    config: &PluginLoadConfig,
    outputs: Vec<String>,
    host: Rc<dyn PluginHostApi>,
    buffers: Rc<ReplayBuffers>,
) -> omnilua::Result<()> {
    let ctx = lua.create_table()?;
    let plugin = lua.create_table()?;
    plugin.set("id", config.plugin_id.clone())?;
    plugin.set("name", config.plugin_name.clone())?;
    plugin.set("version", config.plugin_version.clone())?;
    ctx.set("plugin", plugin)?;

    let now_host = host.clone();
    ctx.set(
        "now_ms",
        lua.create_function(move |_, ()| now_host.now_ms().map_err(host_error))?,
    )?;
    // Replay analyzers may use the same persistent session storage as their
    // live counterpart, but no other host APIs are installed here.
    ctx.set("session", create_storage_api(lua, host)?)?;

    let replay = lua.create_table()?;
    let emit_buffers = buffers.clone();
    let emit_outputs = outputs;
    let plugin_id = config.plugin_id.clone();
    let plugin_version = config.plugin_version.clone();
    replay.set(
        "emit",
        lua.create_function(move |lua, (topic, payload): (String, Value)| {
            if !emit_outputs.is_empty()
                && !emit_outputs
                    .iter()
                    .any(|pattern| topic_matches(pattern, &topic))
            {
                return Err(lua_error_message(format!(
                    "replay.emit: topic '{topic}' 未在 manifest.replay.outputs 中声明"
                )));
            }
            let payload = value_from_lua(payload)?;
            let timestamp_ms = lua
                .globals()
                .get::<_, i64>("__replay_current_ts")
                .unwrap_or_default()
                .max(0) as u64;
            let derived_from = lua
                .globals()
                .get::<_, i64>("__replay_current_id")
                .unwrap_or_default()
                .max(0) as u64;
            let mut metadata = BTreeMap::new();
            metadata.insert("replay".to_owned(), PluginValue::Bool(true));
            metadata.insert(
                "origin".to_owned(),
                PluginValue::String("replay_derived".to_owned()),
            );
            metadata.insert(
                "category".to_owned(),
                PluginValue::String("derived".to_owned()),
            );
            metadata.insert("derived".to_owned(), PluginValue::Bool(true));
            metadata.insert(
                "plugin_id".to_owned(),
                PluginValue::String(plugin_id.clone()),
            );
            metadata.insert(
                "plugin_version".to_owned(),
                PluginValue::String(plugin_version.clone()),
            );
            metadata.insert(
                "derived_from".to_owned(),
                PluginValue::Array(vec![PluginValue::Integer(derived_from as i64)]),
            );
            metadata.insert("recordable".to_owned(), PluginValue::Bool(false));
            let mut event = BTreeMap::new();
            event.insert("id".to_owned(), PluginValue::Integer(0));
            event.insert(
                "timestamp_ms".to_owned(),
                PluginValue::Integer(timestamp_ms as i64),
            );
            event.insert("topic".to_owned(), PluginValue::String(topic));
            event.insert(
                "source".to_owned(),
                PluginValue::String(format!("replay-analyzer:{plugin_id}")),
            );
            event.insert(
                "direction".to_owned(),
                PluginValue::String("internal".to_owned()),
            );
            event.insert("payload".to_owned(), payload);
            event.insert("metadata".to_owned(), PluginValue::Object(metadata));
            emit_buffers
                .events
                .borrow_mut()
                .push(PluginValue::Object(event));
            Ok(())
        })?,
    )?;
    let log_buffers = buffers.clone();
    replay.set(
        "log",
        lua.create_function(move |_, message: String| {
            log_buffers.logs.borrow_mut().push(message);
            Ok(())
        })?,
    )?;
    replay.set(
        "current_event",
        lua.create_function(|lua, ()| lua.globals().get::<_, Value>("__replay_current_event"))?,
    )?;
    ctx.set("replay", replay)?;
    lua.globals().set("ctx", ctx)?;
    Ok(())
}

fn install_ctx(
    lua: &Lua,
    config: &PluginLoadConfig,
    host: Rc<dyn PluginHostApi>,
    line_buffers: Rc<RefCell<BTreeMap<String, VecDeque<String>>>>,
) -> omnilua::Result<()> {
    let globals = lua.globals();
    let handlers = lua.create_table()?;
    globals.set("__plugin_bus_handlers", handlers)?;
    globals.set("__plugin_tasks", lua.create_table()?)?;
    globals.set("__plugin_timers", lua.create_table()?)?;
    globals.set("__current_task_id", Value::Nil)?;
    let ctx = lua.create_table()?;
    let plugin = lua.create_table()?;
    plugin.set("id", config.plugin_id.clone())?;
    plugin.set("name", config.plugin_name.clone())?;
    plugin.set("version", config.plugin_version.clone())?;
    ctx.set("plugin", plugin)?;
    let context = value_to_lua(lua, &config.context)?;
    ctx.set("context", context)?;

    let now_host = host.clone();
    let now_fn = lua.create_function(move |_, ()| now_host.now_ms().map_err(host_error))?;
    ctx.set("now_ms", now_fn.clone())?;
    globals.set("__now_ms", now_fn)?;

    if config
        .permissions
        .contains(tool_plugin_api::PluginCapability::Log)
    {
        ctx.set("log", create_log_api(lua, host.clone())?)?;
    }
    if config
        .permissions
        .contains(tool_plugin_api::PluginCapability::Bus)
    {
        ctx.set("bus", create_bus_api(lua, host.clone())?)?;
    }
    if config
        .permissions
        .contains(tool_plugin_api::PluginCapability::Serial)
    {
        ctx.set(
            "serial",
            create_serial_api(lua, host.clone(), line_buffers.clone())?,
        )?;
    }
    if config
        .permissions
        .contains(tool_plugin_api::PluginCapability::Ui)
    {
        ctx.set("ui", create_ui_api(lua, host.clone())?)?;
    }
    if config
        .permissions
        .contains(tool_plugin_api::PluginCapability::Storage)
    {
        ctx.set("session", create_storage_api(lua, host.clone())?)?;
    }
    if config
        .permissions
        .contains(tool_plugin_api::PluginCapability::Config)
    {
        ctx.set("config", create_config_api(lua, host.clone())?)?;
    }
    if config
        .permissions
        .contains(tool_plugin_api::PluginCapability::Dialog)
    {
        ctx.set("dialog", create_dialog_api(lua, host.clone())?)?;
    }
    if config
        .permissions
        .contains(tool_plugin_api::PluginCapability::Filesystem)
    {
        ctx.set("fs", create_fs_api(lua, host.clone())?)?;
    }

    let commands = lua.create_table()?;
    globals.set("__plugin_commands", commands.clone())?;
    let command_api = lua.create_table()?;
    let register_commands = commands.clone();
    let register_host = host.clone();
    let register_plugin_id = config.plugin_id.clone();
    command_api.set(
        "register",
        lua.create_function(move |_, (id, callback): (String, Function)| {
            register_commands.set(id.clone(), callback)?;
            register_host
                .bus_publish(
                    "plugin.command.registered",
                    PluginValue::Object(
                        [
                            (
                                "plugin_id".to_owned(),
                                PluginValue::String(register_plugin_id.clone()),
                            ),
                            ("command".to_owned(), PluginValue::String(id)),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                )
                .map_err(host_error)?;
            Ok(())
        })?,
    )?;
    let unregister_commands = commands.clone();
    let unregister_host = host.clone();
    let unregister_plugin_id = config.plugin_id.clone();
    command_api.set(
        "unregister",
        lua.create_function(move |_, id: String| {
            unregister_commands.set(id.clone(), Value::Nil)?;
            unregister_host
                .bus_publish(
                    "plugin.command.unregistered",
                    PluginValue::Object(
                        [
                            (
                                "plugin_id".to_owned(),
                                PluginValue::String(unregister_plugin_id.clone()),
                            ),
                            ("command".to_owned(), PluginValue::String(id)),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                )
                .map_err(host_error)?;
            Ok(())
        })?,
    )?;
    let list_commands = commands.clone();
    command_api.set(
        "list",
        lua.create_function(move |lua, ()| {
            let result = lua.create_table()?;
            for (index, (key, _)) in list_commands.raw_pairs()?.into_iter().enumerate() {
                result.set(index + 1, key)?;
            }
            Ok(result)
        })?,
    )?;
    let execute_host = host.clone();
    let execute_plugin_id = config.plugin_id.clone();
    command_api.set(
        "execute",
        lua.create_function(move |_, values: Variadic<Value>| {
            let Some(PluginValue::String(command)) =
                values.first().cloned().map(value_from_lua).transpose()?
            else {
                return Err(lua_error_message(
                    "ctx.commands.execute expects a command id as its first argument",
                ));
            };
            let args = values
                .get(1)
                .cloned()
                .map(value_from_lua)
                .transpose()?
                .unwrap_or(PluginValue::Null);
            let payload = PluginValue::Object(
                [
                    (
                        "plugin_id".to_owned(),
                        PluginValue::String(execute_plugin_id.clone()),
                    ),
                    ("command".to_owned(), PluginValue::String(command)),
                    ("args".to_owned(), args),
                    (
                        "origin".to_owned(),
                        PluginValue::String("lua.commands.execute".to_owned()),
                    ),
                ]
                .into_iter()
                .collect(),
            );
            execute_host
                .bus_publish("plugin.command.execute", payload)
                .map_err(host_error)
        })?,
    )?;
    ctx.set("commands", command_api)?;

    globals.set("__plugin_disable", Value::Nil)?;
    let disable_callbacks = globals.clone();
    globals.set(
        "on_disable",
        lua.create_function(move |_, callback: Function| {
            disable_callbacks.set("__plugin_disable", callback)?;
            Ok(())
        })?,
    )?;
    if config
        .permissions
        .contains(tool_plugin_api::PluginCapability::Timer)
    {
        ctx.set("timer", create_timer_api(lua)?)?;
    }
    if config
        .permissions
        .contains(tool_plugin_api::PluginCapability::Task)
    {
        ctx.set("task", create_task_api(lua)?)?;
    }
    globals.set("ctx", ctx)?;
    install_async_wrappers(lua)?;
    Ok(())
}

fn create_log_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> omnilua::Result<Table> {
    let api = lua.create_table()?;
    for (name, level) in [
        ("trace", tool_plugin_api::LogLevel::Trace),
        ("debug", tool_plugin_api::LogLevel::Debug),
        ("info", tool_plugin_api::LogLevel::Info),
        ("warn", tool_plugin_api::LogLevel::Warn),
        ("error", tool_plugin_api::LogLevel::Error),
    ] {
        let host = host.clone();
        api.set(
            name,
            lua.create_function(move |_, message: Value| {
                let message = value_from_lua(message)?;
                host.log(level, &value_to_string(&message))
                    .map_err(host_error)
            })?,
        )?;
    }
    Ok(api)
}

fn create_bus_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> omnilua::Result<Table> {
    let api = lua.create_table()?;
    let handlers: Table = lua.globals().get("__plugin_bus_handlers")?;
    let on_handlers = handlers.clone();
    api.set(
        "on",
        lua.create_function(move |lua, (topic, callback): (String, Function)| {
            let id = on_handlers.len()? + 1;
            let entry = lua.create_table()?;
            entry.set("topic", topic)?;
            entry.set("callback", callback)?;
            on_handlers.set(id, entry)?;
            Ok(id as i64)
        })?,
    )?;
    let off_handlers = handlers;
    api.set(
        "off",
        lua.create_function(move |_, value: Value| {
            match value {
                Value::Integer(id) => off_handlers.set(id, Value::Nil)?,
                Value::String(topic) => {
                    let topic = topic.to_str()?.to_owned();
                    let entries = off_handlers.raw_pairs()?;
                    for (id, value) in entries {
                        let Value::Table(entry) = value else {
                            continue;
                        };
                        if entry.get::<_, String>("topic").ok().as_deref() == Some(&topic) {
                            off_handlers.set(id, Value::Nil)?;
                        }
                    }
                }
                _ => {}
            }
            Ok(())
        })?,
    )?;
    api.set(
        "__wait_begin",
        lua.create_function(|lua, (topic, timeout_ms): (String, Option<u64>)| {
            let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
            if task_id.is_empty() {
                return Err(lua_error_message("ctx.bus.wait 必须在 ctx.task 协程内调用"));
            }
            let timeout_ms = timeout_ms.unwrap_or(1_000);
            let op = lua.create_table()?;
            op.set("kind", "bus_wait")?;
            op.set("topic", topic)?;
            let now = lua
                .globals()
                .get::<_, Function>("__now_ms")?
                .call::<_, i64>(())
                .unwrap_or_default()
                .max(0) as u64;
            op.set("deadline_ms", now.saturating_add(timeout_ms))?;
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            let state: Table = tasks.get(task_id.as_str())?;
            state.set("yield_op", op.clone())?;
            Ok(op)
        })?,
    )?;
    api.set(
        "__wait_finish",
        lua.create_function(|lua, ()| {
            let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            let state: Table = tasks.get(task_id.as_str())?;
            if let Ok(value) = state.get::<_, Value>("_bus_result") {
                state.set("_bus_result", Value::Nil)?;
                state.set("_bus_result_err", Value::Nil)?;
                return Ok(value);
            }
            if let Ok(error) = state.get::<_, String>("_bus_result_err") {
                state.set("_bus_result_err", Value::Nil)?;
                return Err(lua_error_message(error));
            }
            Ok(Value::Nil)
        })?,
    )?;
    let publish_host = host.clone();
    api.set(
        "publish",
        lua.create_function(move |_, (topic, value): (String, Value)| {
            publish_host
                .bus_publish(&topic, value_from_lua(value)?)
                .map_err(host_error)
        })?,
    )?;
    let history_host = host;
    api.set(
        "history",
        lua.create_function(
            move |lua, (topic, limit): (Option<String>, Option<usize>)| {
                let value = history_host
                    .request(PluginHostRequest::BusHistory {
                        topic: topic.unwrap_or_default(),
                        limit: limit.unwrap_or(100),
                    })
                    .map_err(host_error)?;
                value_to_lua(lua, &value)
            },
        )?,
    )?;
    Ok(api)
}

fn create_serial_api(
    lua: &Lua,
    host: Rc<dyn PluginHostApi>,
    line_buffers: Rc<RefCell<BTreeMap<String, VecDeque<String>>>>,
) -> omnilua::Result<Table> {
    let api = lua.create_table()?;
    let devices_host = host.clone();
    api.set(
        "devices",
        lua.create_function(move |lua, ()| {
            let devices = devices_host.serial_devices().map_err(host_error)?;
            value_to_lua(
                lua,
                &PluginValue::from_json(
                    &serde_json::to_value(devices)
                        .map_err(|e| host_error(PluginError::Host(e.to_string())))?,
                ),
            )
        })?,
    )?;
    let request_host = host.clone();
    api.set(
        "request_device",
        lua.create_function(move |lua, ()| {
            let device = request_host.serial_request_device().map_err(host_error)?;
            value_to_lua(
                lua,
                &PluginValue::from_json(
                    &serde_json::to_value(device)
                        .map_err(|error| lua_error_message(error.to_string()))?,
                ),
            )
        })?,
    )?;
    let request_port_host = host.clone();
    api.set(
        "request_port",
        lua.create_function(move |lua, ()| {
            let device = request_port_host
                .serial_request_device()
                .map_err(host_error)?;
            value_to_lua(
                lua,
                &PluginValue::from_json(
                    &serde_json::to_value(device)
                        .map_err(|error| lua_error_message(error.to_string()))?,
                ),
            )
        })?,
    )?;
    let request_device_host = host.clone();
    api.set(
        "__request_device_begin",
        lua.create_function(move |lua, ()| {
            let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
            if task_id.is_empty() {
                return Err(lua_error_message(
                    "ctx.serial.request_device 必须在 ctx.task 协程内调用",
                ));
            }
            let response = request_device_host
                .request(PluginHostRequest::SerialRequestDevice {
                    task_id: Some(task_id.clone()),
                })
                .map_err(host_error)?;
            let PluginValue::Object(response) = response else {
                return value_to_lua(lua, &response);
            };
            let Some(PluginValue::String(request_id)) = response.get("request_id") else {
                return Ok(Value::Nil);
            };
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            let state: Table = tasks.get(task_id.as_str())?;
            let op = lua.create_table()?;
            op.set("kind", "host")?;
            op.set("request_id", request_id.clone())?;
            state.set("yield_op", op)?;
            state.set("_host_result", Value::Nil)?;
            state.set("_host_result_err", Value::Nil)?;
            Ok(Value::Nil)
        })?,
    )?;
    api.set(
        "__request_device_finish",
        lua.create_function(|lua, ()| {
            let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            let state: Table = tasks.get(task_id.as_str())?;
            if let Ok(error) = state.get::<_, String>("_host_result_err") {
                state.set("_host_result_err", Value::Nil)?;
                state.set("_host_result", Value::Nil)?;
                return Err(lua_error_message(error));
            }
            if let Ok(value) = state.get::<_, Value>("_host_result") {
                state.set("_host_result", Value::Nil)?;
                return Ok(value);
            }
            Ok(Value::Nil)
        })?,
    )?;
    let open_host = host.clone();
    api.set(
        "open",
        lua.create_function(move |lua, value: Value| {
            let options = match value {
                Value::String(port) => {
                    let options = lua.create_table()?;
                    options.set("port_id", port.to_str()?.to_owned())?;
                    options
                }
                Value::Table(options) => options,
                _ => {
                    return Err(lua_error_message(
                        "ctx.serial.open expects a port id string or options table",
                    ));
                }
            };
            let port = options
                .get::<_, String>("port_id")
                .or_else(|_| options.get::<_, String>("port"))?;
            let parity = match options
                .get::<_, String>("parity")
                .unwrap_or_else(|_| "none".to_owned())
                .to_ascii_lowercase()
                .as_str()
            {
                "odd" => tool_plugin_api::PluginParity::Odd,
                "even" => tool_plugin_api::PluginParity::Even,
                _ => tool_plugin_api::PluginParity::None,
            };
            open_host
                .serial_open(
                    &tool_platform::PortId::new(port),
                    PluginSerialSettings {
                        baud_rate: options.get::<_, u32>("baud_rate").unwrap_or(115_200),
                        data_bits: options.get::<_, u32>("data_bits").unwrap_or(8) as u8,
                        stop_bits: options.get::<_, u32>("stop_bits").unwrap_or(1) as u8,
                        parity,
                    },
                )
                .map_err(host_error)
        })?,
    )?;
    let close_host = host.clone();
    api.set(
        "close",
        lua.create_function(move |_, port: Option<String>| {
            let ports = if let Some(port) = port {
                vec![port]
            } else {
                match close_host
                    .request(PluginHostRequest::SerialOpenPorts)
                    .map_err(host_error)?
                {
                    PluginValue::Array(ports) => ports
                        .into_iter()
                        .filter_map(|port| match port {
                            PluginValue::String(port) => Some(port),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                }
            };
            for port in ports {
                close_host
                    .serial_close(&tool_platform::PortId::new(port))
                    .map_err(host_error)?;
            }
            Ok(())
        })?,
    )?;
    let close_port_host = host.clone();
    api.set(
        "close_port",
        lua.create_function(move |_, port: String| {
            close_port_host
                .serial_close(&tool_platform::PortId::new(port))
                .map_err(host_error)
        })?,
    )?;
    let open_ports_host = host.clone();
    api.set(
        "open_ports",
        lua.create_function(move |lua, ()| {
            value_to_lua(
                lua,
                &open_ports_host
                    .request(PluginHostRequest::SerialOpenPorts)
                    .map_err(host_error)?,
            )
        })?,
    )?;
    let list_host = host.clone();
    api.set(
        "list",
        lua.create_function(move |lua, ()| {
            let devices = list_host.serial_devices().map_err(host_error)?;
            let mut value = serde_json::to_value(devices)
                .map_err(|e| host_error(PluginError::Host(e.to_string())))?;
            if let serde_json::Value::Array(items) = &mut value {
                for item in items {
                    if let serde_json::Value::Object(item) = item {
                        if let Some(id) = item.get("id").cloned() {
                            item.insert("port_name".to_owned(), id);
                        }
                        if let Some(kind) = item.get("kind").cloned() {
                            item.insert("port_type".to_owned(), kind);
                        }
                    }
                }
            }
            value_to_lua(lua, &PluginValue::from_json(&value))
        })?,
    )?;
    let send_host = host.clone();
    api.set(
        "send_to",
        lua.create_function(move |_, (port, text): (String, String)| {
            send_host
                .serial_send(&tool_platform::PortId::new(port), text.as_bytes())
                .map_err(host_error)
        })?,
    )?;
    let send_hex_host = host.clone();
    api.set(
        "send_hex_to",
        lua.create_function(move |_, (port, text): (String, String)| {
            let bytes = parse_hex(&text).map_err(host_error)?;
            send_hex_host
                .serial_send(&tool_platform::PortId::new(port), &bytes)
                .map_err(host_error)
        })?,
    )?;
    let status_host = host.clone();
    api.set(
        "status_port",
        lua.create_function(move |lua, port: String| {
            let value = status_host
                .serial_status(&tool_platform::PortId::new(port))
                .map_err(host_error)?;
            value_to_lua(lua, &value)
        })?,
    )?;
    let write_host = host.clone();
    api.set(
        "write_line",
        lua.create_function(move |_, (port, line): (String, String)| {
            let mut bytes = line.into_bytes();
            if !bytes.ends_with(b"\n") {
                bytes.push(b'\n');
            }
            write_host
                .serial_send(&tool_platform::PortId::new(port), &bytes)
                .map_err(host_error)
        })?,
    )?;
    let flush_buffers = line_buffers;
    api.set(
        "flush_rx",
        lua.create_function(move |_, port: String| {
            if let Some(lines) = flush_buffers.borrow_mut().get_mut(&port) {
                lines.clear();
            }
            Ok(())
        })?,
    )?;

    let read_begin = lua.create_function(|lua, (port, opts): (String, Option<Table>)| {
        let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
        if task_id.is_empty() {
            return Err(lua_error_message(
                "ctx.serial.read_line 必须在 ctx.task 协程内调用",
            ));
        }
        let opts = opts.unwrap_or(lua.create_table()?);
        let op = lua.create_table()?;
        op.set("kind", "read_line")?;
        op.set("port", port)?;
        op.set(
            "timeout_ms",
            opts.get::<_, u64>("timeout_ms").unwrap_or(5_000),
        )?;
        op.set(
            "deadline_ms",
            lua.globals()
                .get::<_, Function>("__now_ms")?
                .call::<_, i64>(())
                .unwrap_or_default()
                .max(0) as u64
                + opts.get::<_, u64>("timeout_ms").unwrap_or(5_000),
        )?;
        let tasks: Table = lua.globals().get("__plugin_tasks")?;
        let state: Table = tasks.get(task_id.as_str())?;
        state.set("yield_op", op.clone())?;
        Ok(op)
    })?;
    api.set("__read_line_begin", read_begin)?;
    api.set(
        "__read_line_finish",
        lua.create_function(|lua, ()| {
            let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            let state: Table = tasks.get(task_id.as_str())?;
            let result = lua.create_table()?;
            if let Ok(line) = state.get::<_, String>("_read_result") {
                result.set("line", line)?;
                result.set("err", Value::Nil)?;
            } else if let Ok(error) = state.get::<_, String>("_read_result_err") {
                result.set("line", Value::Nil)?;
                result.set("err", error)?;
            } else {
                result.set("line", Value::Nil)?;
                result.set("err", "timeout")?;
            }
            state.set("_read_result", Value::Nil)?;
            state.set("_read_result_err", Value::Nil)?;
            Ok(result)
        })?,
    )?;

    api.set(
        "__expect_begin",
        lua.create_function(
            |lua, (port, pattern, timeout_ms): (String, String, Option<u64>)| {
                let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
                if task_id.is_empty() {
                    return Err(lua_error_message(
                        "ctx.serial.expect 必须在 ctx.task 协程内调用",
                    ));
                }
                let timeout_ms = timeout_ms.unwrap_or(1_000);
                let patterns = lua.create_table()?;
                let entry = lua.create_table()?;
                entry.set("name", "matched")?;
                entry.set("pattern", pattern)?;
                entry.set("action", "return")?;
                patterns.set(1, entry)?;
                let op = lua.create_table()?;
                op.set("kind", "expect")?;
                op.set("port", port)?;
                op.set("patterns", patterns)?;
                op.set("timeout_ms", timeout_ms)?;
                let now = lua
                    .globals()
                    .get::<_, Function>("__now_ms")?
                    .call::<_, i64>(())
                    .unwrap_or_default()
                    .max(0) as u64;
                op.set("deadline_ms", now.saturating_add(timeout_ms))?;
                let tasks: Table = lua.globals().get("__plugin_tasks")?;
                let state: Table = tasks.get(task_id.as_str())?;
                state.set("yield_op", op.clone())?;
                Ok(op)
            },
        )?,
    )?;
    api.set(
        "__expect_finish",
        lua.create_function(|lua, ()| {
            let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            let state: Table = tasks.get(task_id.as_str())?;
            if let Ok(result) = state.get::<_, Table>("_expect_result") {
                let line = result.get::<_, Value>("line").unwrap_or(Value::Nil);
                state.set("_expect_result", Value::Nil)?;
                state.set("_expect_err", Value::Nil)?;
                return Ok(line);
            }
            state.set("_expect_err", Value::Nil)?;
            Ok(Value::Nil)
        })?,
    )?;

    let expect_host = host;
    api.set(
        "__write_line_and_expect_begin",
        lua.create_function(
            move |lua, (port, line, opts): (String, String, Option<Table>)| {
                let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
                if task_id.is_empty() {
                    return Err(lua_error_message(
                        "ctx.serial.write_line_and_expect 必须在 ctx.task 协程内调用",
                    ));
                }
                let opts = opts.unwrap_or(lua.create_table()?);
                let timeout = opts.get::<_, u64>("timeout_ms").unwrap_or(300_000);
                let mut bytes = line.into_bytes();
                if !bytes.ends_with(b"\n") {
                    bytes.push(b'\n');
                }
                expect_host
                    .serial_send(&tool_platform::PortId::new(port.clone()), &bytes)
                    .map_err(host_error)?;
                let op = lua.create_table()?;
                op.set("kind", "write_line_and_expect")?;
                op.set("port", port)?;
                op.set("timeout_ms", timeout)?;
                op.set(
                    "deadline_ms",
                    lua.globals()
                        .get::<_, Function>("__now_ms")?
                        .call::<_, i64>(())
                        .unwrap_or_default()
                        .max(0) as u64
                        + timeout,
                )?;
                op.set(
                    "continue_resets_timeout",
                    opts.get::<_, bool>("continue_resets_timeout")
                        .unwrap_or(false),
                )?;
                op.set(
                    "patterns",
                    opts.get::<_, Table>("patterns")
                        .unwrap_or(lua.create_table()?),
                )?;
                let tasks: Table = lua.globals().get("__plugin_tasks")?;
                let state: Table = tasks.get(task_id.as_str())?;
                state.set("yield_op", op.clone())?;
                Ok(op)
            },
        )?,
    )?;
    api.set(
        "__write_line_and_expect_finish",
        lua.create_function(|lua, ()| {
            let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            let state: Table = tasks.get(task_id.as_str())?;
            let result = lua.create_table()?;
            if let Ok(value) = state.get::<_, Table>("_expect_result") {
                result.set("result", value)?;
                result.set("err", Value::Nil)?;
            } else if let Ok(error) = state.get::<_, String>("_expect_err") {
                result.set("result", Value::Nil)?;
                result.set("err", error)?;
            } else {
                result.set("result", Value::Nil)?;
                result.set("err", "timeout")?;
            }
            state.set("_expect_result", Value::Nil)?;
            state.set("_expect_err", Value::Nil)?;
            Ok(result)
        })?,
    )?;
    Ok(api)
}

fn install_async_wrappers(lua: &Lua) -> omnilua::Result<()> {
    lua.load(
        r#"
        if ctx.serial then
            local request_device = function()
                local value = ctx.serial.__request_device_begin()
                if value ~= nil then
                    return value
                end
                coroutine.yield({kind='host'})
                return ctx.serial.__request_device_finish()
            end
            ctx.serial.request_device = request_device
            ctx.serial.request_port = request_device
            ctx.serial.read_line = function(port, opts)
                local op = ctx.serial.__read_line_begin(port, opts)
                coroutine.yield(op)
                return ctx.serial.__read_line_finish()
            end
            ctx.serial.write_line_and_expect = function(port, line, opts)
                local op = ctx.serial.__write_line_and_expect_begin(port, line, opts)
                coroutine.yield(op)
                return ctx.serial.__write_line_and_expect_finish()
            end
            ctx.serial.expect_from = function(port, pattern, timeout_ms)
                local op = ctx.serial.__expect_begin(port, pattern, timeout_ms)
                coroutine.yield(op)
                return ctx.serial.__expect_finish()
            end
            ctx.serial.expect = function(pattern, timeout_ms)
                return ctx.serial.expect_from("default", pattern, timeout_ms)
            end
            ctx.serial.request = function(opts)
                local patterns = {{ name = "matched", pattern = opts.expect or "", action = "return" }}
                return ctx.serial.write_line_and_expect(opts.port, opts.tx, {
                    timeout_ms = opts.timeout_ms,
                    patterns = patterns,
                    continue_resets_timeout = opts.continue_resets_timeout,
                })
            end
        end
        if ctx.bus then
            ctx.bus.wait = function(topic, timeout_ms)
                local op = ctx.bus.__wait_begin(topic, timeout_ms)
                coroutine.yield(op)
                return ctx.bus.__wait_finish()
            end
            ctx.bus.subscribe = ctx.bus.wait
        end
        if ctx.dialog then
            ctx.dialog.open_file = function(opts)
                local value = ctx.dialog.__open_file_begin(opts)
                if value ~= nil then
                    return value
                end
                coroutine.yield({kind='host'})
                return ctx.dialog.__open_file_finish()
            end
        end
        "#,
    )
    .set_name("plugin-async-wrappers")
    .exec()
}

fn create_dialog_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> omnilua::Result<Table> {
    let api = lua.create_table()?;
    let begin_host = host.clone();
    api.set(
        "__open_file_begin",
        lua.create_function(move |lua, options: Option<Table>| {
            let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
            if task_id.is_empty() {
                return Err(lua_error_message(
                    "ctx.dialog.open_file 必须在 ctx.task 协程内调用",
                ));
            }
            let options = options.unwrap_or(lua.create_table()?);
            let title = options
                .get::<_, String>("title")
                .unwrap_or_else(|_| "选择文件".to_owned());
            let mut extensions = Vec::new();
            if let Ok(filters) = options.get::<_, Table>("filters") {
                for (_, filter) in filters.raw_pairs()? {
                    let Value::Table(filter) = filter else {
                        continue;
                    };
                    if let Ok(values) = filter.get::<_, Table>("extensions") {
                        for (_, extension) in values.raw_pairs()? {
                            if let Value::String(extension) = extension {
                                extensions.push(extension.to_str()?.to_owned());
                            }
                        }
                    }
                }
            }
            let response = begin_host
                .request(PluginHostRequest::FileOpenText {
                    task_id,
                    title,
                    extensions,
                })
                .map_err(host_error)?;
            let PluginValue::Object(response) = response else {
                return value_to_lua(lua, &response);
            };
            let Some(PluginValue::String(request_id)) = response.get("request_id") else {
                return Ok(Value::Nil);
            };
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
            let state: Table = tasks.get(task_id.as_str())?;
            let op = lua.create_table()?;
            op.set("kind", "host")?;
            op.set("request_id", request_id.clone())?;
            state.set("yield_op", op)?;
            state.set("_host_result", Value::Nil)?;
            state.set("_host_result_err", Value::Nil)?;
            Ok(Value::Nil)
        })?,
    )?;
    api.set(
        "__open_file_finish",
        lua.create_function(|lua, ()| {
            let task_id: String = lua.globals().get("__current_task_id").unwrap_or_default();
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            let state: Table = tasks.get(task_id.as_str())?;
            if let Ok(value) = state.get::<_, Value>("_host_result") {
                state.set("_host_result", Value::Nil)?;
                return Ok(value);
            }
            state.set("_host_result_err", Value::Nil)?;
            Ok(Value::Nil)
        })?,
    )?;
    Ok(api)
}

fn create_fs_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> omnilua::Result<Table> {
    let api = lua.create_table()?;
    let read_host = host.clone();
    api.set(
        "read_text",
        lua.create_function(move |lua, file: Value| {
            let file = match value_from_lua(file)? {
                PluginValue::String(file) => FileHandle::new(file),
                value => {
                    return Err(lua_error_message(format!(
                        "ctx.fs.read_text 需要文件句柄，收到 {value:?}"
                    )));
                }
            };
            let text = read_host.read_text(&file).map_err(host_error)?;
            Ok(Value::String(lua.create_string(text)?))
        })?,
    )?;

    for name in ["read_lines", "read_lines_stream"] {
        let line_host = host.clone();
        api.set(
            name,
            lua.create_function(move |lua, file: Value| {
                let file = match value_from_lua(file)? {
                    PluginValue::String(file) => FileHandle::new(file),
                    value => {
                        return Err(lua_error_message(format!(
                            "ctx.fs.{name} 需要文件句柄，收到 {value:?}"
                        )));
                    }
                };
                let text = line_host.read_text(&file).map_err(host_error)?;
                let lines = Rc::new(RefCell::new(
                    text.lines().map(ToOwned::to_owned).collect::<Vec<_>>(),
                ));
                let index = Rc::new(RefCell::new(0usize));
                let next_lines = lines.clone();
                let next_index = index.clone();
                let iterator = lua.create_function(move |lua, ()| {
                    let mut index = next_index.borrow_mut();
                    let Some(line) = next_lines.borrow().get(*index).cloned() else {
                        return Ok(Value::Nil);
                    };
                    *index += 1;
                    Ok(Value::String(lua.create_string(line)?))
                })?;
                Ok(Value::Function(iterator))
            })?,
        )?;
    }
    Ok(api)
}

fn create_ui_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> omnilua::Result<Table> {
    let api = lua.create_table()?;
    let panel_host = host.clone();
    api.set(
        "get_panel",
        lua.create_function(move |lua, panel_id: String| {
            let history = panel_host
                .request(PluginHostRequest::BusHistory {
                    topic: "ui.panel.".to_owned(),
                    limit: 100,
                })
                .map_err(host_error)?;
            let mut panel = PluginValue::Null;
            if let PluginValue::Array(events) = history {
                for event in events {
                    let PluginValue::Object(event) = event else {
                        continue;
                    };
                    let Some(PluginValue::String(topic)) = event.get("topic") else {
                        continue;
                    };
                    let Some(payload) = event.get("payload").cloned() else {
                        continue;
                    };
                    let matches = match &payload {
                        PluginValue::Object(payload) => payload
                            .get("id")
                            .or_else(|| payload.get("panel_id"))
                            .is_some_and(|value| {
                                matches!(value, PluginValue::String(value) if value == &panel_id)
                            }),
                        _ => false,
                    };
                    if !matches {
                        continue;
                    }
                    panel = if topic == "ui.panel.create" {
                        payload
                    } else {
                        PluginValue::Null
                    };
                    break;
                }
            }
            value_to_lua(lua, &panel)
        })?,
    )?;
    for name in [
        "create_chart",
        "create_form",
        "create_gauge",
        "create_attitude",
        "create_table",
        "remove_panel",
        "set_value",
        "set_values",
        "set_enabled",
        "set_visible",
        "table_set_rows",
        "table_append_rows",
        "table_remove_rows",
        "table_clear",
        "set_contribution_value",
        "set_status",
    ] {
        let host = host.clone();
        let command = name.to_owned();
        api.set(
            name,
            lua.create_function(move |_, value: Value| {
                host.ui_command(PluginUiCommand {
                    command: command.clone(),
                    payload: value_from_lua(value)?,
                })
                .map_err(host_error)
            })?,
        )?;
    }
    // The real ABI uses positional arguments for panel/field identifiers.
    // Install variadic replacements after the simple one-value fallback.
    for name in [
        "create_chart",
        "create_form",
        "create_gauge",
        "create_attitude",
        "create_table",
        "remove_panel",
        "set_value",
        "set_values",
        "set_enabled",
        "set_visible",
        "table_set_rows",
        "table_append_rows",
        "table_remove_rows",
        "table_clear",
        "set_contribution_value",
        "set_status",
    ] {
        let host = host.clone();
        let command = name.to_owned();
        api.set(
            name,
            lua.create_function(move |_, args: Variadic<Value>| {
                let args = args
                    .into_iter()
                    .map(value_from_lua)
                    .collect::<Result<Vec<_>, _>>()?;
                host.ui_command(PluginUiCommand {
                    command: command.clone(),
                    payload: ui_payload(&command, args),
                })
                .map_err(host_error)
            })?,
        )?;
    }
    Ok(api)
}

fn ui_payload(command: &str, args: Vec<PluginValue>) -> PluginValue {
    let mut object = std::collections::BTreeMap::new();
    match command {
        "remove_panel" => {
            if let Some(value) = args.first() {
                object.insert("id".to_owned(), value.clone());
            }
            PluginValue::Object(object)
        }
        "table_clear" => {
            if let Some(value) = args.first() {
                object.insert("panel_id".to_owned(), value.clone());
            }
            PluginValue::Object(object)
        }
        "set_value" | "set_enabled" | "set_visible" => {
            if let Some(value) = args.first() {
                object.insert("panel_id".to_owned(), value.clone());
            }
            if let Some(value) = args.get(1) {
                object.insert("field_id".to_owned(), value.clone());
            }
            if let Some(value) = args.get(2) {
                object.insert("value".to_owned(), value.clone());
            }
            PluginValue::Object(object)
        }
        "set_values" => {
            if let Some(value) = args.first() {
                object.insert("panel_id".to_owned(), value.clone());
            }
            if let Some(value) = args.get(1) {
                object.insert("values".to_owned(), value.clone());
            }
            PluginValue::Object(object)
        }
        "set_contribution_value" => {
            if let Some(value) = args.first() {
                object.insert("id".to_owned(), value.clone());
                object.insert(
                    "panel_id".to_owned(),
                    PluginValue::String("__contribution__".to_owned()),
                );
                object.insert("field_id".to_owned(), value.clone());
            }
            if let Some(value) = args.get(1) {
                object.insert("value".to_owned(), value.clone());
            }
            PluginValue::Object(object)
        }
        "set_status" => {
            if let Some(value) = args.first() {
                object.insert("message".to_owned(), value.clone());
            }
            PluginValue::Object(object)
        }
        _ if args.len() == 1 => args.into_iter().next().unwrap_or(PluginValue::Null),
        _ => PluginValue::Array(args),
    }
}

fn create_timer_api(lua: &Lua) -> omnilua::Result<Table> {
    let api = lua.create_table()?;
    let timers: Table = lua.globals().get("__plugin_timers")?;
    let every_timers = timers.clone();
    api.set(
        "every",
        lua.create_function(move |lua, (interval, callback): (u64, Function)| {
            let id = every_timers.len()? as i64 + 1;
            let timer = lua.create_table()?;
            timer.set("interval_ms", interval.max(1))?;
            timer.set(
                "next_ms",
                lua.globals()
                    .get::<_, Function>("__now_ms")?
                    .call::<_, i64>(())
                    .unwrap_or_default()
                    .max(0) as u64
                    + interval.max(1),
            )?;
            timer.set("repeat", true)?;
            timer.set("callback", callback)?;
            every_timers.set(id, timer)?;
            Ok(id)
        })?,
    )?;
    let after_timers = timers.clone();
    api.set(
        "after",
        lua.create_function(move |lua, (interval, callback): (u64, Function)| {
            let id = after_timers.len()? as i64 + 1;
            let timer = lua.create_table()?;
            timer.set("interval_ms", interval.max(1))?;
            timer.set(
                "next_ms",
                lua.globals()
                    .get::<_, Function>("__now_ms")?
                    .call::<_, i64>(())
                    .unwrap_or_default()
                    .max(0) as u64
                    + interval.max(1),
            )?;
            timer.set("repeat", false)?;
            timer.set("callback", callback)?;
            after_timers.set(id, timer)?;
            Ok(id)
        })?,
    )?;
    let cancel_timers = timers;
    api.set(
        "cancel",
        lua.create_function(move |_, id: i64| {
            cancel_timers.set(id, Value::Nil)?;
            Ok(())
        })?,
    )?;
    Ok(api)
}

fn create_task_api(lua: &Lua) -> omnilua::Result<Table> {
    let api = lua.create_table()?;
    let tasks: Table = lua.globals().get("__plugin_tasks")?;
    let start_tasks = tasks.clone();
    api.set(
        "start",
        lua.create_function(move |lua, (config, callback): (Table, Function)| {
            let id: String = config.get("id")?;
            if let Ok(existing) = start_tasks.get::<_, Table>(id.as_str())
                && !existing.get::<_, bool>("finished").unwrap_or(true)
            {
                return Err(InnerLuaError::runtime(format_args!(
                    "task id '{id}' is already running"
                ))
                .into());
            }
            let thread = lua.create_thread(callback)?;
            let state = lua.create_table()?;
            state.set("id", id.clone())?;
            state.set(
                "title",
                config.get::<_, String>("title").unwrap_or(id.clone()),
            )?;
            state.set("thread", Value::Thread(thread.clone()))?;
            state.set("cancelled", false)?;
            state.set("paused", false)?;
            state.set("finished", false)?;
            state.set("status", "running")?;
            state.set("yield_op", Value::Nil)?;
            state.set("progress_current", 0_i64)?;
            state.set("progress_total", 0_i64)?;
            state.set("progress_percent", Value::Nil)?;
            state.set("logs", lua.create_table()?)?;
            start_tasks.set(id.clone(), state.clone())?;

            let task = lua.create_table()?;
            task.set("id", id.clone())?;
            let status_state = state.clone();
            task.set(
                "set_status",
                lua.create_function(move |_, (_task, text): (Table, String)| {
                    status_state.set("status", text)?;
                    Ok(())
                })?,
            )?;
            let progress_state = state.clone();
            task.set(
                "set_progress",
                lua.create_function(move |_, (_task, current, total): (Table, i64, i64)| {
                    progress_state.set("progress_current", current)?;
                    progress_state.set("progress_total", total)?;
                    Ok(())
                })?,
            )?;
            let percent_state = state.clone();
            task.set(
                "set_progress_percent",
                lua.create_function(move |_, (_task, percent): (Table, f64)| {
                    percent_state.set("progress_percent", percent.clamp(0.0, 100.0))?;
                    Ok(())
                })?,
            )?;
            let log_state = state.clone();
            task.set(
                "log",
                lua.create_function(move |lua, (_task, level, message): (Table, String, String)| {
                    let entry = lua.create_table()?;
                    entry.set("level", level)?;
                    entry.set("message", message)?;
                    let logs: Table = log_state.get("logs")?;
                    logs.set(logs.len()? + 1, entry)?;
                    Ok(())
                })?,
            )?;
            let cancel_state = state.clone();
            task.set(
                "is_cancelled",
                lua.create_function(move |_, _task: Table| {
                    Ok(cancel_state.get::<_, bool>("cancelled").unwrap_or(false))
                })?,
            )?;
            let paused_state = state.clone();
            task.set(
                "is_paused",
                lua.create_function(move |_, _task: Table| {
                    Ok(paused_state.get::<_, bool>("paused").unwrap_or(false))
                })?,
            )?;
            let wait_if_paused: Function = lua
                .load(
                    "return function(task) while task:is_paused() do coroutine.yield({kind='paused'}) end end",
                )
                .eval()?;
            task.set("wait_if_paused", wait_if_paused)?;
            let sleep_ms: Function = lua
                .load(
                    "return function(task, ms) local now = __now_ms() coroutine.yield({kind='sleep', deadline_ms=now + (ms or 0)}) end",
                )
                .eval()?;
            task.set("sleep_ms", sleep_ms)?;

            lua.globals().set("__current_task_id", id.as_str())?;
            let result = thread.resume::<_, Value>(task.clone());
            lua.globals().set("__current_task_id", Value::Nil)?;
            if let Err(error) = result {
                state.set("finished", true)?;
                state.set("error", error.to_string())?;
            } else if thread.status()? == ThreadStatus::Dead {
                state.set("finished", true)?;
            }
            Ok(task)
        })?,
    )?;
    let cancel_tasks = tasks.clone();
    api.set(
        "cancel",
        lua.create_function(move |_, id: String| {
            if let Ok(state) = cancel_tasks.get::<_, Table>(id.as_str()) {
                state.set("cancelled", true)?;
                state.set("paused", false)?;
            }
            Ok(())
        })?,
    )?;
    let pause_tasks = tasks.clone();
    api.set(
        "pause",
        lua.create_function(move |_, id: String| {
            if let Ok(state) = pause_tasks.get::<_, Table>(id.as_str()) {
                state.set("paused", true)?;
            }
            Ok(())
        })?,
    )?;
    let resume_tasks = tasks.clone();
    api.set(
        "resume",
        lua.create_function(move |_, id: String| {
            if let Ok(state) = resume_tasks.get::<_, Table>(id.as_str()) {
                state.set("paused", false)?;
            }
            Ok(())
        })?,
    )?;
    let list_tasks = tasks.clone();
    api.set(
        "list",
        lua.create_function(move |lua, ()| {
            let result = lua.create_table()?;
            let mut index = 1;
            for (_, value) in list_tasks.raw_pairs()? {
                let Value::Table(state) = value else {
                    continue;
                };
                let summary = lua.create_table()?;
                for key in ["id", "title", "status", "finished", "cancelled", "paused"] {
                    summary.set(key, state.get::<_, Value>(key).unwrap_or(Value::Nil))?;
                }
                result.set(index, summary)?;
                index += 1;
            }
            Ok(result)
        })?,
    )?;
    api.set(
        "is_cancelled",
        lua.create_function(move |_, id: String| {
            Ok(tasks
                .get::<_, Table>(id.as_str())
                .ok()
                .and_then(|state| state.get::<_, bool>("cancelled").ok())
                .unwrap_or(false))
        })?,
    )?;
    Ok(api)
}

fn create_storage_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> omnilua::Result<Table> {
    let api = lua.create_table()?;
    let get_host = host.clone();
    api.set(
        "get",
        lua.create_function(move |lua, (key, default): (String, Option<Value>)| {
            let value = get_host.storage_get(&key).unwrap_or_else(|_| {
                default
                    .map(|value| value_from_lua(value).unwrap_or(PluginValue::Null))
                    .unwrap_or(PluginValue::Null)
            });
            value_to_lua(lua, &value)
        })?,
    )?;
    let set_host = host.clone();
    api.set(
        "set",
        lua.create_function(move |_, (key, value): (String, Value)| {
            set_host
                .storage_set(&key, value_from_lua(value)?)
                .map_err(host_error)
        })?,
    )?;
    let delete_host = host.clone();
    api.set(
        "delete",
        lua.create_function(move |_, key: String| {
            delete_host.storage_delete(&key).map_err(host_error)
        })?,
    )?;
    let keys_host = host.clone();
    api.set(
        "keys",
        lua.create_function(move |lua, ()| {
            value_to_lua(
                lua,
                &keys_host
                    .request(PluginHostRequest::StorageKeys)
                    .map_err(host_error)?,
            )
        })?,
    )?;
    Ok(api)
}

fn create_config_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> omnilua::Result<Table> {
    let api = lua.create_table()?;
    let get_host = host.clone();
    api.set(
        "get",
        lua.create_function(move |lua, (key, default): (String, Option<Value>)| {
            let default = default
                .map(value_from_lua)
                .transpose()?
                .unwrap_or(PluginValue::Null);
            let value = get_host.config_get(&key, default).map_err(host_error)?;
            value_to_lua(lua, &value)
        })?,
    )?;
    let set_host = host.clone();
    api.set(
        "set",
        lua.create_function(move |_, (key, value): (String, Value)| {
            set_host
                .config_set(&key, value_from_lua(value)?)
                .map_err(host_error)
        })?,
    )?;
    let remove_host = host.clone();
    api.set(
        "remove",
        lua.create_function(move |_, key: String| {
            remove_host.config_remove(&key).map_err(host_error)
        })?,
    )?;
    let keys_host = host.clone();
    api.set(
        "keys",
        lua.create_function(move |lua, ()| {
            let value = keys_host.config_keys().map_err(host_error)?;
            value_to_lua(lua, &value)
        })?,
    )?;
    let list_host = host.clone();
    api.set(
        "profile_list",
        lua.create_function(move |lua, ()| {
            let value = list_host
                .request(PluginHostRequest::ConfigProfileList)
                .map_err(host_error)?;
            value_to_lua(lua, &value)
        })?,
    )?;
    let load_host = host.clone();
    api.set(
        "profile_load",
        lua.create_function(move |lua, name: String| {
            let value = load_host
                .request(PluginHostRequest::ConfigProfileLoad { name })
                .map_err(host_error)?;
            value_to_lua(lua, &value)
        })?,
    )?;
    let save_host = host.clone();
    api.set(
        "profile_save",
        lua.create_function(move |_, (name, value): (String, Value)| {
            save_host
                .request(PluginHostRequest::ConfigProfileSave {
                    name,
                    value: value_from_lua(value)?,
                })
                .map(|_| ())
                .map_err(host_error)
        })?,
    )?;
    let delete_host = host;
    api.set(
        "profile_delete",
        lua.create_function(move |_, name: String| {
            delete_host
                .request(PluginHostRequest::ConfigProfileDelete { name })
                .map(|_| ())
                .map_err(host_error)
        })?,
    )?;
    Ok(api)
}

fn install_codec_module(lua: &Lua) -> omnilua::Result<()> {
    let package: Table = lua.globals().get("package")?;
    let preload: Table = package.get("preload")?;
    let hw = lua.create_table()?;
    let codec = lua.create_table()?;
    codec.set(
        "xor8",
        lua.create_function(|_, text: String| {
            Ok(text.bytes().fold(0_u8, |value, byte| value ^ byte) as i64)
        })?,
    )?;
    hw.set("codec", codec)?;
    let codec_module = hw.get::<_, Table>("codec")?;
    preload.set(
        "hw.codec",
        lua.create_function(move |_, _args: Variadic<Value>| Ok(codec_module.clone()))?,
    )?;
    let utils = lua.create_table()?;
    utils.set(
        "format_size",
        lua.create_function(|_, value: i64| {
            if value >= 1024 * 1024 {
                Ok(format!("{:.1} MiB", value as f64 / (1024.0 * 1024.0)))
            } else if value >= 1024 {
                Ok(format!("{:.1} KiB", value as f64 / 1024.0))
            } else {
                Ok(format!("{value} B"))
            }
        })?,
    )?;
    utils.set(
        "to_hex",
        lua.create_function(|_, text: String| {
            Ok(text
                .bytes()
                .map(|byte| format!("{byte:02X}"))
                .collect::<Vec<_>>()
                .join(" "))
        })?,
    )?;
    let utils_module = utils.clone();
    preload.set(
        "hw.utils",
        lua.create_function(move |_, _args: Variadic<Value>| Ok(utils_module.clone()))?,
    )?;
    Ok(())
}

fn value_to_lua(lua: &Lua, value: &PluginValue) -> omnilua::Result<Value> {
    Ok(match value {
        PluginValue::Null => Value::Nil,
        PluginValue::Bool(value) => Value::Boolean(*value),
        PluginValue::Integer(value) => Value::Integer(*value),
        PluginValue::Number(value) => Value::Number(*value),
        PluginValue::String(value) => Value::String(lua.create_string(value)?),
        PluginValue::Array(values) => {
            let table = lua.create_table()?;
            for (index, value) in values.iter().enumerate() {
                table.set(index + 1, value_to_lua(lua, value)?)?;
            }
            Value::Table(table)
        }
        PluginValue::Object(values) => {
            let table = lua.create_table()?;
            for (key, value) in values {
                table.set(key.as_str(), value_to_lua(lua, value)?)?;
            }
            Value::Table(table)
        }
    })
}

fn value_from_lua(value: Value) -> omnilua::Result<PluginValue> {
    Ok(match value {
        Value::Nil => PluginValue::Null,
        Value::Boolean(value) => PluginValue::Bool(value),
        Value::Integer(value) => PluginValue::Integer(value),
        Value::Number(value) => PluginValue::Number(value),
        Value::String(value) => PluginValue::String(value.to_str()?),
        Value::Table(table) => {
            let pairs = table.raw_pairs()?;
            let is_array = pairs.iter().enumerate().all(|(index, (key, _))| {
                matches!(key, Value::Integer(value) if *value == index as i64 + 1)
            });
            if is_array {
                PluginValue::Array(
                    pairs
                        .into_iter()
                        .map(|(_, value)| value_from_lua(value))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            } else {
                let mut object = std::collections::BTreeMap::new();
                for (key, value) in pairs {
                    let key = match key {
                        Value::String(key) => key.to_str()?,
                        Value::Integer(key) => key.to_string(),
                        Value::Number(key) => key.to_string(),
                        _ => return Err(lua_error_message("table key must be scalar")),
                    };
                    object.insert(key, value_from_lua(value)?);
                }
                PluginValue::Object(object)
            }
        }
        other => {
            return Err(lua_error_message(format!(
                "unsupported Lua value: {other:?}"
            )));
        }
    })
}

fn values_to_plugin(values: &[Value]) -> omnilua::Result<PluginValue> {
    match values {
        [] => Ok(PluginValue::Null),
        [value] => value_from_lua(value.clone()),
        values => Ok(PluginValue::Array(
            values
                .iter()
                .cloned()
                .map(value_from_lua)
                .collect::<Result<Vec<_>, _>>()?,
        )),
    }
}

fn value_to_string(value: &PluginValue) -> String {
    match value {
        PluginValue::String(value) => value.clone(),
        _ => value
            .to_json()
            .map(|value| value.to_string())
            .unwrap_or_default(),
    }
}

fn parse_hex(value: &str) -> PluginResult<Vec<u8>> {
    let compact = value
        .chars()
        .filter(|character| {
            !character.is_ascii_whitespace() && *character != '_' && *character != '-'
        })
        .collect::<String>();
    if compact.is_empty() || !compact.len().is_multiple_of(2) {
        return Err(PluginError::InvalidValue("invalid hex input".to_owned()));
    }
    (0..compact.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&compact[index..index + 2], 16)
                .map_err(|_| PluginError::InvalidValue("invalid hex input".to_owned()))
        })
        .collect()
}

fn topic_matches(pattern: &str, topic: &str) -> bool {
    pattern == topic
        || pattern
            .strip_suffix('*')
            .is_some_and(|prefix| topic.starts_with(prefix))
}

fn lua_error(error: LuaError) -> PluginError {
    PluginError::Runtime(error.to_string())
}

fn host_error(error: PluginError) -> LuaError {
    LuaError::from(InnerLuaError::runtime(format_args!("{error}")))
}

fn lua_error_message(message: impl AsRef<str>) -> LuaError {
    LuaError::from(InnerLuaError::runtime(format_args!("{}", message.as_ref())))
}
