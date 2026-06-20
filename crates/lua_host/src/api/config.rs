//! `ctx.config.*` — 插件配置 API（get/set/remove/keys/profile_*）。

use std::sync::Arc;

use mlua::{Lua, Table, Value};

use crate::config::ConfigStore;
use crate::convert::{json_to_lua_value, lua_value_to_json};

pub(crate) fn create_config_api(
    lua: &Lua,
    store: Arc<ConfigStore>,
    plugin_id: String,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    // ctx.config.get(key, default)
    let store_get = store.clone();
    let pid_get = plugin_id.clone();
    table.set(
        "get",
        lua.create_function(move |lua, (key, default): (String, Value)| {
            let default_json = lua_value_to_json(default).unwrap_or(serde_json::Value::Null);
            let value = store_get.get(&pid_get, &key, default_json);
            json_to_lua_value(lua, &value)
        })?,
    )?;

    // ctx.config.set(key, value)
    let store_set = store.clone();
    let pid_set = plugin_id.clone();
    table.set(
        "set",
        lua.create_function(move |_lua, (key, value): (String, Value)| {
            let json_value = lua_value_to_json(value).unwrap_or(serde_json::Value::Null);
            store_set
                .set(&pid_set, &key, json_value)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))
        })?,
    )?;

    // ctx.config.remove(key)
    let store_remove = store.clone();
    let pid_remove = plugin_id.clone();
    table.set(
        "remove",
        lua.create_function(move |_lua, key: String| {
            store_remove
                .remove(&pid_remove, &key)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))
        })?,
    )?;

    // ctx.config.keys()
    let store_keys = store.clone();
    let pid_keys = plugin_id.clone();
    table.set(
        "keys",
        lua.create_function(move |lua, ()| {
            let keys = store_keys.keys(&pid_keys);
            let arr = lua.create_table()?;
            for (i, k) in keys.iter().enumerate() {
                arr.set(i + 1, k.as_str())?;
            }
            Ok(Value::Table(arr))
        })?,
    )?;

    // ── profile API ──

    // ctx.config.profile_list()
    let store_pl = store.clone();
    let pid_pl = plugin_id.clone();
    table.set(
        "profile_list",
        lua.create_function(move |lua, ()| {
            let names = store_pl.profile_list(&pid_pl);
            let arr = lua.create_table()?;
            for (i, name) in names.iter().enumerate() {
                arr.set(i + 1, name.as_str())?;
            }
            Ok(Value::Table(arr))
        })?,
    )?;

    // ctx.config.profile_load(name)
    let store_pload = store.clone();
    let pid_pload = plugin_id.clone();
    table.set(
        "profile_load",
        lua.create_function(move |lua, name: String| {
            match store_pload.profile_load(&pid_pload, &name) {
                Some(data) => json_to_lua_value(lua, &data),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // ctx.config.profile_save(name, data)
    let store_psave = store.clone();
    let pid_psave = plugin_id.clone();
    table.set(
        "profile_save",
        lua.create_function(move |_lua, (name, data): (String, Value)| {
            let json_data = lua_value_to_json(data).unwrap_or(serde_json::Value::Null);
            store_psave
                .profile_save(&pid_psave, &name, json_data)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))
        })?,
    )?;

    // ctx.config.profile_delete(name)
    let store_pdel = store;
    let pid_pdel = plugin_id;
    table.set(
        "profile_delete",
        lua.create_function(move |_lua, name: String| {
            store_pdel
                .profile_delete(&pid_pdel, &name)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))
        })?,
    )?;

    Ok(table)
}
