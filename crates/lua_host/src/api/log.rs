//! `ctx.log.*` — 日志 API。

use mlua::{Lua, Table};
use tool_core::{Event, LogLevel};
use tool_databus::DataBus;

pub(crate) fn create_log_api(lua: &Lua, bus: DataBus, source: String) -> mlua::Result<Table> {
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
