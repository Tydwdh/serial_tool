//! `ctx.timer.*` — 定时器 API（after / every / cancel）。

use mlua::{Function, Lua, Table, Value};

pub(crate) fn create_timer_api(lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set(
        "after",
        lua.create_function(|lua, (ms, callback): (u64, Function)| {
            let now_ms = tool_core::now_timestamp_ms();
            register_timer(lua, now_ms, now_ms + ms, 0_u64, callback)
        })?,
    )?;

    table.set(
        "every",
        lua.create_function(|lua, (ms, callback): (u64, Function)| {
            let now_ms = tool_core::now_timestamp_ms();
            let interval_ms = ms.max(1);
            register_timer(lua, now_ms, now_ms + interval_ms, interval_ms, callback)
        })?,
    )?;

    table.set(
        "cancel",
        lua.create_function(|lua, id: String| {
            let timers: Table = lua.globals().get(crate::globals::PLUGIN_TIMERS)?;
            timers.set(id, Value::Nil)?;
            Ok(())
        })?,
    )?;

    Ok(table)
}

/// 把一次定时登记进 PLUGIN_TIMERS，返回给插件用的定时器 id。
///
/// `now_ms` 由调用方读一次并同时用于 id 和触发时刻，避免两处时间戳不一致。
/// id 的序号用 raw_len + 1，避免同一毫秒内 ID 碰撞。
fn register_timer(
    lua: &Lua,
    now_ms: u64,
    trigger_at_ms: u64,
    interval_ms: u64,
    callback: Function,
) -> mlua::Result<String> {
    let timers: Table = lua.globals().get(crate::globals::PLUGIN_TIMERS)?;
    let id = format!("t{now_ms}-{}", timers.raw_len() + 1);

    let timer = lua.create_table()?;
    timer.set("trigger_at_ms", trigger_at_ms)?;
    timer.set("interval_ms", interval_ms)?;
    timer.set("callback", callback)?;

    timers.set(id.as_str(), timer)?;

    Ok(id)
}
