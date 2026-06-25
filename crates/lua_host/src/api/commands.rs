//! `ctx.commands.*` — 插件命令注册与执行 API。

use mlua::{Function, Lua, Table, Value, Variadic};
use serde_json::json;
use tool_core::{Direction, Event, Payload, topics};
use tool_databus::DataBus;

use crate::convert::lua_value_to_json;
use crate::globals::PLUGIN_COMMANDS;

pub(crate) fn create_commands_api(
    lua: &Lua,
    bus: DataBus,
    source: String,
    plugin_id: String,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    let reg_bus = bus.clone();
    let reg_source = source.clone();
    let reg_pid = plugin_id.clone();

    table.set(
        "register",
        lua.create_function(move |lua, (command, handler): (String, Function)| {
            let commands: Table = lua.globals().get(PLUGIN_COMMANDS)?;
            commands.set(command.clone(), handler)?;

            // 通知管理面：命令已注册
            reg_bus.publish(Event::new(
                topics::PLUGIN_COMMAND_REGISTERED,
                reg_source.clone(),
                Direction::Internal,
                Payload::Json(json!({
                    "plugin_id": reg_pid,
                    "command": command,
                })),
            ));

            Ok(())
        })?,
    )?;

    let unreg_bus = bus.clone();
    let unreg_source = source.clone();
    let unreg_pid = plugin_id.clone();

    table.set(
        "unregister",
        lua.create_function(move |lua, command: String| {
            let commands: Table = lua.globals().get(PLUGIN_COMMANDS)?;
            commands.set(command.clone(), Value::Nil)?;

            // 通知管理面：命令已注销
            unreg_bus.publish(Event::new(
                topics::PLUGIN_COMMAND_UNREGISTERED,
                unreg_source.clone(),
                Direction::Internal,
                Payload::Json(json!({
                    "plugin_id": unreg_pid,
                    "command": command,
                })),
            ));

            Ok(())
        })?,
    )?;

    table.set(
        "list",
        lua.create_function(|lua, ()| {
            let commands: Table = lua.globals().get(PLUGIN_COMMANDS)?;
            let result = lua.create_table()?;
            let mut index = 0usize;
            for command in commands
                .pairs::<String, Value>()
                .flatten()
                .map(|pair| pair.0)
            {
                index += 1;
                result.set(index, command)?;
            }
            Ok(Value::Table(result))
        })?,
    )?;

    let execute_bus = bus;
    table.set(
        "execute",
        lua.create_function(move |_lua, values: Variadic<Value>| {
            let command = values
                .first()
                .and_then(|value| match value {
                    Value::String(command) => Some(command.to_string_lossy().to_string()),
                    _ => None,
                })
                .ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "ctx.commands.execute expects command id as first argument".to_owned(),
                    )
                })?;
            let args = values.get(1).cloned().unwrap_or(Value::Nil);
            let args = lua_value_to_json(args).unwrap_or(serde_json::Value::Null);

            execute_bus.publish(Event::new(
                topics::PLUGIN_COMMAND_EXECUTE,
                source.clone(),
                Direction::Internal,
                Payload::Json(json!({
                    "plugin_id": plugin_id,
                    "command": command,
                    "args": args,
                    "origin": "lua.commands.execute"
                })),
            ));

            Ok(())
        })?,
    )?;

    Ok(table)
}
