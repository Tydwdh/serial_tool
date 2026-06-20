//! `ctx.timer.*` — 定时器 API（after / every / cancel）。

use mlua::{Function, Lua, Table, Value};

pub(crate) fn create_timer_api(lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set(
        "after",
        lua.create_function(move |lua, (ms, callback): (u64, Function)| {
            let timers: Table = lua.globals().get(crate::globals::PLUGIN_TIMERS)?;
            let now_ms = tool_core::now_timestamp_ms();
            // 使用 raw_len + 1 作为序号，避免同一毫秒内 ID 碰撞
            let seq = timers.raw_len() + 1;
            let id = format!("t{now_ms}-{seq}");

            let timer = lua.create_table()?;
            timer.set("trigger_at_ms", now_ms + ms)?;
            timer.set("interval_ms", 0_u64)?;
            timer.set("callback", callback)?;

            timers.set(id.clone(), timer)?;

            Ok(id)
        })?,
    )?;

    table.set(
        "every",
        lua.create_function(move |lua, (ms, callback): (u64, Function)| {
            let timers: Table = lua.globals().get(crate::globals::PLUGIN_TIMERS)?;
            let now_ms = tool_core::now_timestamp_ms();
            let interval_ms = ms.max(1);
            let seq = timers.raw_len() + 1;
            let id = format!("t{now_ms}-{seq}");

            let timer = lua.create_table()?;
            timer.set("trigger_at_ms", now_ms + interval_ms)?;
            timer.set("interval_ms", interval_ms)?;
            timer.set("callback", callback)?;

            timers.set(id.clone(), timer)?;

            Ok(id)
        })?,
    )?;

    table.set(
        "cancel",
        lua.create_function(move |lua, id: String| {
            let timers: Table = lua.globals().get(crate::globals::PLUGIN_TIMERS)?;
            timers.set(id, Value::Nil)?;
            Ok(())
        })?,
    )?;

    Ok(table)
}
