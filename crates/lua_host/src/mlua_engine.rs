//! Native adapter for the platform-neutral Lua engine boundary.
//!
//! The mature threaded `run_plugin` path remains in use by the existing
//! extension manager.  This adapter gives new application code the same
//! `PluginValue`/`PluginHostApi` contract as the browser's pure-Rust engine;
//! no `mlua::Value` escapes this module.

use std::collections::BTreeMap;
use std::rc::Rc;

use mlua::{Function, Lua, Table, Value, Variadic};
use tool_plugin_api::{
    LuaEngine, PluginCallResult, PluginError, PluginFunctionId, PluginHostApi, PluginHostRequest,
    PluginInstanceId, PluginLoadConfig, PluginResult, PluginUiCommand, PluginValue,
};

use crate::convert::{lua_value_to_plugin_value, plugin_value_to_lua};

struct MluaInstance {
    lua: Lua,
}

/// Native single-threaded Lua adapter.
pub struct MluaEngine {
    next_instance: u64,
    instances: BTreeMap<PluginInstanceId, MluaInstance>,
}

impl Default for MluaEngine {
    fn default() -> Self {
        Self {
            next_instance: 1,
            instances: BTreeMap::new(),
        }
    }
}

impl MluaEngine {
    fn instance(&self, id: PluginInstanceId) -> PluginResult<&MluaInstance> {
        self.instances
            .get(&id)
            .ok_or_else(|| PluginError::Runtime(format!("unknown plugin instance {}", id.0)))
    }
}

impl LuaEngine for MluaEngine {
    fn load_plugin(
        &mut self,
        source: &str,
        config: PluginLoadConfig,
        host: Rc<dyn PluginHostApi>,
    ) -> PluginResult<PluginInstanceId> {
        let lua = Lua::new();
        install_ctx(&lua, &config, host.clone()).map_err(lua_error)?;
        lua.load(source)
            .set_name(&config.script_name)
            .exec()
            .map_err(lua_error)?;
        let id = PluginInstanceId(self.next_instance);
        self.next_instance = self.next_instance.saturating_add(1);
        self.instances.insert(id, MluaInstance { lua });
        Ok(id)
    }

    fn call(
        &mut self,
        instance: PluginInstanceId,
        function: PluginFunctionId,
        args: &[PluginValue],
    ) -> PluginResult<PluginCallResult> {
        let instance = self.instance(instance)?;
        let callback: Function = instance
            .lua
            .globals()
            .get(function.0.as_str())
            .map_err(lua_error)?;
        let args = args
            .iter()
            .map(|value| plugin_value_to_lua(&instance.lua, value))
            .collect::<Result<Vec<_>, _>>()
            .map_err(lua_error)?;
        let values: Variadic<Value> = callback.call(Variadic::from(args)).map_err(lua_error)?;
        Ok(PluginCallResult::Completed(
            values_to_plugin(values.into_iter()).map_err(lua_error)?,
        ))
    }

    fn resume(
        &mut self,
        _coroutine: tool_plugin_api::CoroutineId,
        _value: PluginValue,
    ) -> PluginResult<PluginCallResult> {
        Err(PluginError::UnsupportedCapability(
            "anonymous coroutine resume is managed by ctx.task".to_owned(),
        ))
    }

    fn stop(&mut self, instance: PluginInstanceId) -> PluginResult<()> {
        let instance = self
            .instances
            .remove(&instance)
            .ok_or_else(|| PluginError::Runtime("unknown plugin instance".to_owned()))?;
        if let Ok(callback) = instance.lua.globals().get::<Function>("__plugin_disable") {
            callback.call::<Value>(()).map_err(lua_error)?;
        }
        Ok(())
    }

    fn dispatch_event(
        &mut self,
        instance: PluginInstanceId,
        event: PluginValue,
    ) -> PluginResult<()> {
        let instance = self.instance(instance)?;
        let topic = object_string(&event, "topic").unwrap_or_default();
        let event = plugin_value_to_lua(&instance.lua, &event).map_err(lua_error)?;
        let handlers: Table = instance
            .lua
            .globals()
            .get("__plugin_bus_handlers")
            .map_err(lua_error)?;
        for pair in handlers.pairs::<Value, Table>() {
            let (_, entry) = pair.map_err(lua_error)?;
            let pattern: String = entry.get("topic").map_err(lua_error)?;
            if !topic_matches(&pattern, &topic) {
                continue;
            }
            let callback: Function = entry.get("callback").map_err(lua_error)?;
            callback.call::<Value>(event.clone()).map_err(lua_error)?;
        }
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
        let context = plugin_value_to_lua(&instance.lua, &context).map_err(lua_error)?;
        let result: Value = callback.call(context).map_err(lua_error)?;
        Ok(PluginCallResult::Completed(
            lua_value_to_plugin_value(result).map_err(lua_error)?,
        ))
    }

    fn update_settings(
        &mut self,
        instance: PluginInstanceId,
        settings: PluginValue,
    ) -> PluginResult<()> {
        let instance = self.instance(instance)?;
        if let Ok(callback) = instance.lua.globals().get::<Function>("on_settings") {
            callback
                .call::<Value>(plugin_value_to_lua(&instance.lua, &settings).map_err(lua_error)?)
                .map_err(lua_error)?;
        }
        Ok(())
    }
}

fn install_ctx(
    lua: &Lua,
    config: &PluginLoadConfig,
    host: Rc<dyn PluginHostApi>,
) -> mlua::Result<()> {
    let globals = lua.globals();
    globals.set("__plugin_bus_handlers", lua.create_table()?)?;
    globals.set("__plugin_commands", lua.create_table()?)?;
    globals.set("__plugin_disable", Value::Nil)?;
    let ctx = lua.create_table()?;
    let plugin = lua.create_table()?;
    plugin.set("id", config.plugin_id.clone())?;
    plugin.set("name", config.plugin_name.clone())?;
    plugin.set("version", config.plugin_version.clone())?;
    ctx.set("plugin", plugin)?;
    ctx.set("context", plugin_value_to_lua(lua, &config.context)?)?;
    let now_host = host.clone();
    ctx.set(
        "now_ms",
        lua.create_function(move |_, ()| now_host.now_ms().map_err(host_error))?,
    )?;
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
        ctx.set("serial", create_serial_api(lua, host.clone())?)?;
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
    let disable_globals = globals.clone();
    globals.set(
        "on_disable",
        lua.create_function(move |_, callback: Function| {
            disable_globals.set("__plugin_disable", callback)?;
            Ok(())
        })?,
    )?;
    let commands: Table = globals.get("__plugin_commands")?;
    let command_api = lua.create_table()?;
    let register = commands.clone();
    command_api.set(
        "register",
        lua.create_function(move |_, (id, callback): (String, Function)| {
            register.set(id, callback)?;
            Ok(())
        })?,
    )?;
    ctx.set("commands", command_api)?;
    globals.set("ctx", ctx)?;
    Ok(())
}

fn create_log_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> mlua::Result<Table> {
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
            lua.create_function(move |_, value: Value| {
                let value = lua_value_to_plugin_value(value)?;
                host.log(level, &value_to_string(&value))
                    .map_err(host_error)
            })?,
        )?;
    }
    Ok(api)
}

fn create_bus_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> mlua::Result<Table> {
    let api = lua.create_table()?;
    let handlers: Table = lua.globals().get("__plugin_bus_handlers")?;
    let on_handlers = handlers.clone();
    api.set(
        "on",
        lua.create_function(move |lua, (topic, callback): (String, Function)| {
            let entry = lua.create_table()?;
            entry.set("topic", topic)?;
            entry.set("callback", callback)?;
            let id = on_handlers.raw_len() + 1;
            on_handlers.set(id, entry)?;
            Ok(id)
        })?,
    )?;
    let off_handlers = handlers;
    api.set(
        "off",
        lua.create_function(move |_, id: i64| {
            off_handlers.set(id, Value::Nil)?;
            Ok(())
        })?,
    )?;
    let publish = host.clone();
    api.set(
        "publish",
        lua.create_function(move |_, (topic, value): (String, Value)| {
            publish
                .bus_publish(&topic, lua_value_to_plugin_value(value)?)
                .map_err(host_error)
        })?,
    )?;
    let history = host;
    api.set(
        "history",
        lua.create_function(
            move |lua, (topic, limit): (Option<String>, Option<usize>)| {
                let value = history
                    .request(PluginHostRequest::BusHistory {
                        topic: topic.unwrap_or_default(),
                        limit: limit.unwrap_or(100),
                    })
                    .map_err(host_error)?;
                plugin_value_to_lua(lua, &value)
            },
        )?,
    )?;
    Ok(api)
}

fn create_serial_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> mlua::Result<Table> {
    let api = lua.create_table()?;
    let devices = host.clone();
    api.set(
        "list",
        lua.create_function(move |lua, ()| {
            let value = PluginValue::from_json(
                &serde_json::to_value(devices.serial_devices().map_err(host_error)?)
                    .map_err(|error| mlua::Error::external(error.to_string()))?,
            );
            plugin_value_to_lua(lua, &value)
        })?,
    )?;
    let send = host.clone();
    api.set(
        "send_to",
        lua.create_function(move |_, (port, text): (String, String)| {
            send.serial_send(&tool_platform::PortId::new(port), text.as_bytes())
                .map_err(host_error)
        })?,
    )?;
    let status = host;
    api.set(
        "status_port",
        lua.create_function(move |lua, port: String| {
            plugin_value_to_lua(
                lua,
                &status
                    .serial_status(&tool_platform::PortId::new(port))
                    .map_err(host_error)?,
            )
        })?,
    )?;
    Ok(api)
}

fn create_ui_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> mlua::Result<Table> {
    let api = lua.create_table()?;
    for name in [
        "set_value",
        "set_values",
        "set_contribution_value",
        "set_status",
    ] {
        let host = host.clone();
        let command = name.to_owned();
        api.set(
            name,
            lua.create_function(move |_, args: Variadic<Value>| {
                let values = args
                    .into_iter()
                    .map(lua_value_to_plugin_value)
                    .collect::<Result<Vec<_>, _>>()?;
                host.ui_command(PluginUiCommand {
                    command: command.clone(),
                    payload: PluginValue::Array(values),
                })
                .map_err(host_error)
            })?,
        )?;
    }
    Ok(api)
}

fn create_storage_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> mlua::Result<Table> {
    let api = lua.create_table()?;
    let get = host.clone();
    api.set(
        "get",
        lua.create_function(move |lua, (key, default): (String, Option<Value>)| {
            let default = default
                .map(lua_value_to_plugin_value)
                .transpose()?
                .unwrap_or(PluginValue::Null);
            plugin_value_to_lua(lua, &get.storage_get(&key).unwrap_or(default))
        })?,
    )?;
    let set = host.clone();
    api.set(
        "set",
        lua.create_function(move |_, (key, value): (String, Value)| {
            set.storage_set(&key, lua_value_to_plugin_value(value)?)
                .map_err(host_error)
        })?,
    )?;
    let delete = host.clone();
    api.set(
        "delete",
        lua.create_function(move |_, key: String| delete.storage_delete(&key).map_err(host_error))?,
    )?;
    let keys = host;
    api.set(
        "keys",
        lua.create_function(move |lua, ()| {
            plugin_value_to_lua(
                lua,
                &keys
                    .request(PluginHostRequest::StorageKeys)
                    .map_err(host_error)?,
            )
        })?,
    )?;
    Ok(api)
}

fn create_config_api(lua: &Lua, host: Rc<dyn PluginHostApi>) -> mlua::Result<Table> {
    let api = create_storage_api(lua, host.clone())?;
    let set_host = host.clone();
    api.set(
        "set",
        lua.create_function(move |_, (key, value): (String, Value)| {
            set_host
                .config_set(&key, lua_value_to_plugin_value(value)?)
                .map_err(host_error)
        })?,
    )?;
    let get_host = host;
    api.set(
        "get",
        lua.create_function(move |lua, (key, default): (String, Option<Value>)| {
            let default = default
                .map(lua_value_to_plugin_value)
                .transpose()?
                .unwrap_or(PluginValue::Null);
            plugin_value_to_lua(
                lua,
                &get_host.config_get(&key, default).map_err(host_error)?,
            )
        })?,
    )?;
    Ok(api)
}

fn values_to_plugin(values: impl Iterator<Item = Value>) -> mlua::Result<PluginValue> {
    let values = values
        .map(lua_value_to_plugin_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(match values.as_slice() {
        [] => PluginValue::Null,
        [value] => value.clone(),
        _ => PluginValue::Array(values),
    })
}

fn object_string(value: &PluginValue, key: &str) -> Option<String> {
    match value {
        PluginValue::Object(object) => object.get(key).and_then(|value| match value {
            PluginValue::String(value) => Some(value.clone()),
            _ => None,
        }),
        _ => None,
    }
}

fn topic_matches(pattern: &str, topic: &str) -> bool {
    pattern == topic
        || pattern
            .strip_suffix('*')
            .is_some_and(|prefix| topic.starts_with(prefix))
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

fn lua_error(error: mlua::Error) -> PluginError {
    PluginError::Runtime(error.to_string())
}

fn host_error(error: PluginError) -> mlua::Error {
    mlua::Error::RuntimeError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use tool_plugin_api::{LogLevel, PluginHostRequest};

    #[derive(Default)]
    struct RecordingHost {
        logs: RefCell<Vec<String>>,
    }

    impl PluginHostApi for RecordingHost {
        fn request(&self, request: PluginHostRequest) -> PluginResult<PluginValue> {
            match request {
                PluginHostRequest::NowMs => Ok(PluginValue::Integer(123)),
                PluginHostRequest::Log { level, message } => {
                    self.logs.borrow_mut().push(format!("{level:?}:{message}"));
                    Ok(PluginValue::Null)
                }
                _ => Err(PluginError::UnsupportedCapability("test".to_owned())),
            }
        }
    }

    #[test]
    fn shared_engine_boundary_loads_and_dispatches_lua() {
        let host = Rc::new(RecordingHost::default());
        let mut engine = MluaEngine::default();
        let instance = engine
            .load_plugin(
                r#"
                    ctx.commands.register("demo.run", function(payload)
                        ctx.log.info(payload.message)
                        return { ok = true, now = ctx.now_ms() }
                    end)
                "#,
                PluginLoadConfig {
                    plugin_id: "demo".to_owned(),
                    plugin_name: "Demo".to_owned(),
                    plugin_version: "1.0.0".to_owned(),
                    script_name: "main.lua".to_owned(),
                    context: PluginValue::Null,
                    permissions: tool_plugin_api::PluginPermissions::new([
                        tool_plugin_api::PluginCapability::Log,
                    ]),
                },
                host.clone(),
            )
            .expect("plugin should load through the shared boundary");

        let result = engine
            .dispatch_command(
                instance,
                "demo.run",
                PluginValue::Object(
                    [(
                        "message".to_owned(),
                        PluginValue::String("hello".to_owned()),
                    )]
                    .into_iter()
                    .collect(),
                ),
            )
            .expect("command should dispatch");
        assert_eq!(
            result,
            PluginCallResult::Completed(PluginValue::Object(
                [
                    ("now".to_owned(), PluginValue::Integer(123)),
                    ("ok".to_owned(), PluginValue::Bool(true)),
                ]
                .into_iter()
                .collect(),
            ))
        );
        assert_eq!(
            host.logs.borrow().as_slice(),
            [format!("{:?}:hello", LogLevel::Info)].as_slice()
        );
        engine.stop(instance).expect("plugin should stop");
    }
}
