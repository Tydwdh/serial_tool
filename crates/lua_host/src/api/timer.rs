//! `ctx.timer.*` — 定时器 API（after / every / cancel）。

use mlua::{Function, Lua, Table, Value};
use std::sync::atomic::{AtomicU64, Ordering};

/// 同一毫秒内注册多个定时器时仍要拿到不同 id；Lua 表的 `raw_len()` 只数数组部分，
/// 而 PLUGIN_TIMERS 全是字符串键，所以不能用它生成序号。
static NEXT_TIMER_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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
    let sequence = NEXT_TIMER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let id = format!("t{now_ms}-{sequence}");

    let timer = lua.create_table()?;
    timer.set("trigger_at_ms", trigger_at_ms)?;
    timer.set("interval_ms", interval_ms)?;
    timer.set("callback", callback)?;

    timers.set(id.as_str(), timer)?;

    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_registered_in_the_same_millisecond_get_distinct_ids() {
        let lua = Lua::new();
        let timers = lua.create_table().unwrap();
        lua.globals()
            .set(crate::globals::PLUGIN_TIMERS, timers)
            .unwrap();
        let callback: Function = lua.load("return function() end").eval().unwrap();

        let first = register_timer(&lua, 1_000, 1_000, 0, callback.clone()).unwrap();
        let second = register_timer(&lua, 1_000, 1_000, 0, callback).unwrap();

        assert_ne!(first, second, "同一毫秒的两个定时器不能共用 id");
        let timers: Table = lua.globals().get(crate::globals::PLUGIN_TIMERS).unwrap();
        let count = timers.pairs::<String, Table>().count();
        assert_eq!(count, 2, "后注册的定时器不能覆盖前一个");
    }
}
