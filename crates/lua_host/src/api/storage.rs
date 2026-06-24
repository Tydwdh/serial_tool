//! `ctx.session.*` — 键值存储 API。

use mlua::{Lua, Table, Value};

pub(crate) fn create_storage_api(lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set(
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

    table.set(
        "set",
        lua.create_function(|lua, (key, value): (String, String)| {
            let storage: Table = lua.globals().get(crate::globals::PLUGIN_STORAGE)?;
            storage.set(key, value)?;
            Ok(())
        })?,
    )?;

    table.set(
        "keys",
        lua.create_function(|lua, ()| {
            let storage: Table = lua.globals().get(crate::globals::PLUGIN_STORAGE)?;

            let keys = storage
                .pairs::<String, Value>()
                .filter_map(|pair| pair.ok().map(|(key, _)| key))
                .collect::<Vec<_>>();

            Ok(keys)
        })?,
    )?;

    Ok(table)
}
