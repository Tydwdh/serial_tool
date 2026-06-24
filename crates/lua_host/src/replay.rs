//! Replay analyzer 运行器：`run_replay_analyzer` + `install_replay_ctx`。
//!
//! 从 `lib.rs` 抽出的 replay 专用逻辑，约 235 行。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mlua::{Function, Lua, LuaOptions, StdLib, Table, Value};
use parking_lot::Mutex;

use tool_core::{Direction, Event, mark_derived_event, now_timestamp_ms, topic_matches};

use crate::codec;
use crate::convert::{event_to_lua_table, json_to_lua_value, lua_value_to_payload};
use crate::{LuaHostResult, LuaReplayConfig, LuaReplayOutput, install_budget_hook};

pub fn run_replay_analyzer(
    source: String,
    config: LuaReplayConfig,
    input_events: &[Event],
) -> LuaHostResult<LuaReplayOutput> {
    run_replay_analyzer_with_cancel(
        source,
        config,
        input_events,
        Arc::new(AtomicBool::new(false)),
    )
}

pub fn run_replay_analyzer_with_cancel(
    source: String,
    config: LuaReplayConfig,
    input_events: &[Event],
    cancel: Arc<AtomicBool>,
) -> LuaHostResult<LuaReplayOutput> {
    let lua = Lua::new_with(
        StdLib::TABLE
            | StdLib::STRING
            | StdLib::MATH
            | StdLib::UTF8
            | StdLib::PACKAGE
            | StdLib::COROUTINE,
        LuaOptions::default(),
    )?;

    // 安装 budget hook：防止 analyzer 死循环或卡死；取消信号共用
    install_budget_hook(&lua, 30_000, Arc::clone(&cancel))?;

    let emitted_events = Arc::new(Mutex::new(Vec::new()));
    let logs = Arc::new(Mutex::new(Vec::new()));

    install_replay_ctx(&lua, emitted_events.clone(), logs.clone(), &config)?;

    // 加载并执行 Lua 源码
    lua.load(&source).set_name(&config.script_name).exec()?;

    // 检查取消
    if cancel.load(Ordering::Relaxed) {
        logs.lock().push("Analyzer 已取消".to_owned());
        return Ok(LuaReplayOutput {
            events: Vec::new(),
            logs: logs.lock().clone(),
        });
    }

    // 构建 session 信息
    let first_ts = input_events.first().map(|e| e.timestamp_ms).unwrap_or(0);
    let last_ts = input_events.last().map(|e| e.timestamp_ms).unwrap_or(0);
    let session = lua.create_table()?;
    session.set("start_ms", first_ts)?;
    session.set("end_ms", last_ts)?;
    session.set("event_count", input_events.len())?;

    // on_replay_begin
    if let Ok(begin_fn) = lua.globals().get::<Function>("on_replay_begin")
        && let Err(e) = begin_fn.call::<Value>(session)
    {
        logs.lock().push(format!("on_replay_begin error: {e}"));
    }

    // 遍历输入事件
    for input_event in input_events {
        if cancel.load(Ordering::Relaxed) {
            logs.lock().push("Analyzer 已取消".to_owned());
            break;
        }

        // 只处理匹配 subscriptions 的事件
        if !config
            .subscriptions
            .iter()
            .any(|sub| topic_matches(sub.as_str(), &input_event.topic))
        {
            continue;
        }

        // 设置当前输入事件信息，供 ctx.replay.emit 使用
        let current_input_ts = input_event.timestamp_ms;
        let current_input_id = input_event.id;
        lua.globals().set("__replay_current_ts", current_input_ts)?;
        lua.globals().set("__replay_current_id", current_input_id)?;

        if let Ok(callback) = lua.globals().get::<Function>("on_replay_event") {
            let event_table = lua.create_table()?;
            event_to_lua_table(&lua, &event_table, input_event)?;

            lua.globals()
                .set("__replay_current_event", event_table.clone())?;

            if let Err(e) = callback.call::<Value>(event_table) {
                logs.lock().push(format!("on_replay_event error: {e}"));
            }
        }
    }

    // on_replay_end 时清除 current 标记
    lua.globals().set("__replay_current_ts", last_ts)?;
    lua.globals().set("__replay_current_id", Value::Nil)?;

    // on_replay_end
    if let Ok(end_fn) = lua.globals().get::<Function>("on_replay_end")
        && let Err(e) = end_fn.call::<()>(())
    {
        logs.lock().push(format!("on_replay_end error: {e}"));
    }

    let events = emitted_events.lock().clone();
    let logs = logs.lock().clone();

    Ok(LuaReplayOutput { events, logs })
}

fn install_replay_ctx(
    lua: &Lua,
    emitted_events: Arc<Mutex<Vec<Event>>>,
    logs: Arc<Mutex<Vec<String>>>,
    config: &LuaReplayConfig,
) -> mlua::Result<()> {
    let ctx = lua.create_table()?;

    // ctx.plugin (只读)
    ctx.set("plugin", json_to_lua_value(lua, &config.context)?)?;

    // ctx.now_ms()
    ctx.set(
        "now_ms",
        lua.create_function(|_lua, ()| Ok(now_timestamp_ms()))?,
    )?;

    // 本地 require：只能加载插件根目录和 lib 子目录
    if let Some(ref root) = config.plugin_root {
        let root_str = root.display().to_string().replace('\\', "/");
        let new_path = format!("{root_str}/lib/?.lua;{root_str}/?.lua");
        if let Ok(package) = lua.globals().get::<Table>("package") {
            let _ = package.set("path", new_path);
            let _ = package.set("cpath", "");
        }
    }

    // ctx.session.get (只读)
    let storage = lua.create_table()?;
    storage.set(
        "get",
        lua.create_function(|lua, key: String| {
            let storage: Table = lua.globals().get(crate::globals::PLUGIN_STORAGE)?;
            let value: mlua::Result<String> = storage.get(key);
            Ok(match value {
                Ok(value) => Value::String(lua.create_string(&value)?),
                Err(_) => Value::Nil,
            })
        })?,
    )?;
    ctx.set("storage", storage)?;

    // ctx.replay
    let replay = lua.create_table()?;

    // ctx.replay.emit(topic, payload)
    let emitted = emitted_events.clone();
    let config_emit = config.clone();
    replay.set(
        "emit",
        lua.create_function(move |lua, (topic, payload): (String, Value)| {
            // 校验 topic 必须在 manifest replay.outputs 中
            if !config_emit.outputs.is_empty()
                && !config_emit.outputs.iter().any(|o| topic_matches(o, &topic))
            {
                return Err(mlua::Error::RuntimeError(format!(
                    "replay.emit: topic '{}' not in manifest replay.outputs",
                    topic
                )));
            }
            let payload = lua_value_to_payload(payload)?;
            let source = format!("replay-analyzer:{}", config_emit.plugin_id);

            // 读取当前输入事件时间戳
            let ts: u64 = lua.globals().get("__replay_current_ts").unwrap_or(0);
            let derived_from: u64 = lua.globals().get("__replay_current_id").unwrap_or(0);

            let mut event = Event::new(topic, source, Direction::Internal, payload);
            event.timestamp_ms = ts;
            mark_derived_event(
                &mut event,
                &config_emit.plugin_id,
                &config_emit.plugin_version,
                &[derived_from],
            );

            emitted.lock().push(event);
            Ok(())
        })?,
    )?;

    // ctx.replay.log(message)
    let logs_for_fn = logs.clone();
    replay.set(
        "log",
        lua.create_function(move |_lua, message: String| {
            logs_for_fn.lock().push(message);
            Ok(())
        })?,
    )?;

    // ctx.replay.current_event() — 返回当前输入事件 table
    replay.set(
        "current_event",
        lua.create_function(|lua, ()| {
            let event: Value = lua
                .globals()
                .get("__replay_current_event")
                .unwrap_or(Value::Nil);
            Ok(event)
        })?,
    )?;

    ctx.set("replay", replay)?;

    // 安装全局存储表
    lua.globals()
        .set(crate::globals::PLUGIN_STORAGE, lua.create_table()?)?;

    if let Err(e) = codec::register_codec(lua) {
        log::warn!("replay: failed to register hw.codec: {e}");
    }
    if let Err(e) = codec::register_utils(lua) {
        log::warn!("replay: failed to register hw.utils: {e}");
    }

    // 注册 ctx 全局变量
    lua.globals().set("ctx", ctx)?;

    Ok(())
}
