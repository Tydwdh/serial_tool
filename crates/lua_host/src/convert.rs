//! Lua ↔ Rust 类型转换工具函数。
//!
//! 纯函数集合，负责在 `mlua::Value`/`serde_json::Value`/`tool_core::Payload` 之间互转。
//! 被 `install_ctx`、各 `create_*_api`、`install_replay_ctx` 等广泛依赖。

use mlua::{Lua, Table, Value};
use serde_json::{Map, Number};
use tool_core::{Event, Payload};
use tool_transport::{SerialConfig, parse_data_bits, parse_parity, parse_stop_bits};

// ── Event → Lua ──

pub(crate) fn event_to_lua_table(lua: &Lua, table: &Table, event: &Event) -> mlua::Result<()> {
    table.set("id", event.id)?;
    table.set("timestamp_ms", event.timestamp_ms)?;
    table.set("topic", event.topic.clone())?;
    table.set("source", event.source.clone())?;
    table.set("direction", format!("{:?}", event.direction).to_lowercase())?;
    table.set("payload", payload_to_lua(lua, &event.payload)?)?;
    table.set("metadata", json_to_lua_value(lua, &event.metadata)?)?;

    Ok(())
}

// ── Payload → Lua ──

pub(crate) fn payload_to_lua(lua: &Lua, payload: &Payload) -> mlua::Result<Value> {
    Ok(match payload {
        Payload::Empty => Value::Nil,
        Payload::Bytes(bytes) => Value::String(lua.create_string(bytes)?),
        Payload::Text(text) => Value::String(lua.create_string(text)?),
        Payload::Json(value) => json_to_lua_value(lua, value)?,
    })
}

// ── Lua → SerialConfig ──

pub(crate) fn lua_value_to_serial_config(value: Value) -> mlua::Result<SerialConfig> {
    match value {
        Value::String(port_name) => Ok(SerialConfig {
            port_name: port_name.to_str()?.to_owned(),
            ..Default::default()
        }),

        Value::Table(table) => {
            let mut config = SerialConfig {
                port_name: table.get("port_name").or_else(|_| table.get("port"))?,
                ..Default::default()
            };

            if let Ok(value) = table.get::<u32>("baud_rate") {
                config.baud_rate = value;
            } else if let Ok(value) = table.get::<u32>("baud") {
                config.baud_rate = value;
            }

            if let Ok(value) = table.get::<u64>("timeout_ms") {
                config.timeout_ms = value;
            }

            if let Ok(value) = table.get::<String>("data_bits") {
                config.data_bits = parse_data_bits(&value);
            }

            if let Ok(value) = table.get::<String>("stop_bits") {
                config.stop_bits = parse_stop_bits(&value);
            }

            if let Ok(value) = table.get::<String>("parity") {
                config.parity = parse_parity(&value);
            }

            Ok(config)
        }

        other => Err(mlua::Error::RuntimeError(format!(
            "serial.open expects a string or table, got {}",
            other.type_name()
        ))),
    }
}

// ── Lua → Payload ──

pub(crate) fn lua_value_to_payload(value: Value) -> mlua::Result<Payload> {
    Ok(match value {
        Value::Nil => Payload::Empty,
        Value::Boolean(value) => Payload::Json(serde_json::Value::Bool(value)),
        Value::Integer(value) => Payload::Json(serde_json::Value::Number(value.into())),
        Value::Number(value) => Payload::Json(number_to_json(value)?),
        Value::String(value) => Payload::Text(value.to_str()?.to_owned()),
        Value::Table(value) => Payload::Json(lua_table_to_json(value)?),
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "unsupported payload: {}",
                other.type_name()
            )));
        }
    })
}

// ── Lua Table → JSON ──

pub(crate) fn lua_table_to_json(table: Table) -> mlua::Result<serde_json::Value> {
    let mut entries = Vec::new();
    let mut is_array = true;
    let mut max_index = 0_i64;

    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;

        if let Value::Integer(index) = key
            && index > 0
        {
            max_index = max_index.max(index);
            entries.push((key, lua_value_to_json(value)?));
            continue;
        }

        is_array = false;
        entries.push((key, lua_value_to_json(value)?));
    }

    if is_array && max_index as usize == entries.len() {
        entries.sort_by_key(|(key, _)| match key {
            Value::Integer(index) => *index,
            _ => 0,
        });

        return Ok(serde_json::Value::Array(
            entries.into_iter().map(|(_, value)| value).collect(),
        ));
    }

    let mut object = Map::new();

    for (key, value) in entries {
        object.insert(lua_key_to_string(key)?, value);
    }

    Ok(serde_json::Value::Object(object))
}

// ── Lua Value → JSON ──

pub(crate) fn lua_value_to_json(value: Value) -> mlua::Result<serde_json::Value> {
    Ok(match value {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(value) => serde_json::Value::Bool(value),
        Value::Integer(value) => serde_json::Value::Number(value.into()),
        Value::Number(value) => number_to_json(value)?,
        Value::String(value) => serde_json::Value::String(value.to_str()?.to_owned()),
        Value::Table(value) => lua_table_to_json(value)?,
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "unsupported value: {}",
                other.type_name()
            )));
        }
    })
}

// ── Helpers ──

pub(crate) fn lua_key_to_string(key: Value) -> mlua::Result<String> {
    Ok(match key {
        Value::String(value) => value.to_str()?.to_owned(),
        Value::Integer(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "unsupported key: {}",
                other.type_name()
            )));
        }
    })
}

pub(crate) fn number_to_json(value: f64) -> mlua::Result<serde_json::Value> {
    Number::from_f64(value)
        .map(serde_json::Value::Number)
        .ok_or_else(|| mlua::Error::RuntimeError("number is not finite".to_owned()))
}

pub(crate) fn payload_to_json(payload: Payload) -> serde_json::Value {
    match payload {
        Payload::Empty => serde_json::Value::Null,
        Payload::Bytes(bytes) => serde_json::Value::Array(
            bytes
                .into_iter()
                .map(|byte| serde_json::Value::Number(byte.into()))
                .collect(),
        ),
        Payload::Text(text) => serde_json::Value::String(text),
        Payload::Json(value) => value,
    }
}

// ── JSON → Lua ──

pub(crate) fn json_to_lua_value(lua: &Lua, value: &serde_json::Value) -> mlua::Result<Value> {
    Ok(match value {
        serde_json::Value::Null => Value::Nil,

        serde_json::Value::Bool(value) => Value::Boolean(*value),

        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Value::Integer(value)
            } else if let Some(value) = value.as_f64() {
                Value::Number(value)
            } else {
                Value::Nil
            }
        }

        serde_json::Value::String(value) => Value::String(lua.create_string(value)?),

        serde_json::Value::Array(values) => {
            let table = lua.create_table()?;

            for (index, value) in values.iter().enumerate() {
                table.set(index + 1, json_to_lua_value(lua, value)?)?;
            }

            Value::Table(table)
        }

        serde_json::Value::Object(values) => {
            let table = lua.create_table()?;

            for (key, value) in values {
                table.set(key.as_str(), json_to_lua_value(lua, value)?)?;
            }

            Value::Table(table)
        }
    })
}
