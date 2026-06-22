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

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;
    use tool_transport::{DataBits, Parity, StopBits};

    // ── lua_value_to_json ──

    #[test]
    fn lua_nil_to_json_null() {
        let result = lua_value_to_json(Value::Nil).unwrap();
        assert_eq!(result, serde_json::Value::Null);
    }

    #[test]
    fn lua_bool_true_to_json() {
        let result = lua_value_to_json(Value::Boolean(true)).unwrap();
        assert_eq!(result, serde_json::Value::Bool(true));
    }

    #[test]
    fn lua_bool_false_to_json() {
        let result = lua_value_to_json(Value::Boolean(false)).unwrap();
        assert_eq!(result, serde_json::Value::Bool(false));
    }

    #[test]
    fn lua_integer_to_json() {
        let result = lua_value_to_json(Value::Integer(42)).unwrap();
        assert_eq!(result, serde_json::json!(42));
    }

    #[test]
    fn lua_negative_integer_to_json() {
        let result = lua_value_to_json(Value::Integer(-7)).unwrap();
        assert_eq!(result, serde_json::json!(-7));
    }

    #[test]
    fn lua_float_to_json() {
        let val = 1.234_f64;
        let result = lua_value_to_json(Value::Number(val)).unwrap();
        assert!(result.is_number());
        assert!((result.as_f64().unwrap() - val).abs() < f64::EPSILON);
    }

    #[test]
    fn lua_string_to_json() {
        let lua = Lua::new();
        let s = lua.create_string("hello").unwrap();
        let result = lua_value_to_json(Value::String(s)).unwrap();
        assert_eq!(result, serde_json::Value::String("hello".to_owned()));
    }

    #[test]
    fn lua_empty_table_to_json_array() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        // An empty Lua table with no entries is treated as an array (max_index == 0, len == 0).
        let result = lua_value_to_json(Value::Table(table)).unwrap();
        assert_eq!(result, serde_json::Value::Array(vec![]));
    }

    #[test]
    fn lua_sequential_table_to_json_array() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        table.set(1, 10).unwrap();
        table.set(2, 20).unwrap();
        table.set(3, 30).unwrap();
        let result = lua_value_to_json(Value::Table(table)).unwrap();
        assert_eq!(result, serde_json::json!([10, 20, 30]));
    }

    #[test]
    fn lua_string_key_table_to_json_object() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        table.set("name", "alice").unwrap();
        table.set("age", 30).unwrap();
        let result = lua_value_to_json(Value::Table(table)).unwrap();
        assert_eq!(result["name"], serde_json::json!("alice"));
        assert_eq!(result["age"], serde_json::json!(30));
    }

    #[test]
    fn lua_nested_table_to_json() {
        let lua = Lua::new();
        let inner = lua.create_table().unwrap();
        inner.set("x", 1).unwrap();
        inner.set("y", 2).unwrap();

        let outer = lua.create_table().unwrap();
        outer.set("inner", inner).unwrap();
        outer.set("label", "point").unwrap();

        let result = lua_value_to_json(Value::Table(outer)).unwrap();
        assert_eq!(result["label"], serde_json::json!("point"));
        assert_eq!(result["inner"]["x"], serde_json::json!(1));
        assert_eq!(result["inner"]["y"], serde_json::json!(2));
    }

    #[test]
    fn lua_mixed_keys_table_to_json_object() {
        // A table with both integer and string keys becomes an object
        // (is_array is false because string keys exist).
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        table.set(1, "first").unwrap();
        table.set("key", "value").unwrap();
        let result = lua_value_to_json(Value::Table(table)).unwrap();
        assert!(result.is_object());
        assert_eq!(result["1"], serde_json::json!("first"));
        assert_eq!(result["key"], serde_json::json!("value"));
    }

    #[test]
    fn lua_non_finite_number_returns_error() {
        let result = lua_value_to_json(Value::Number(f64::NAN));
        assert!(result.is_err());
        let result = lua_value_to_json(Value::Number(f64::INFINITY));
        assert!(result.is_err());
    }

    // ── json_to_lua_value ──

    #[test]
    fn json_null_to_lua_nil() {
        let lua = Lua::new();
        let result = json_to_lua_value(&lua, &serde_json::Value::Null).unwrap();
        assert!(result.is_nil());
    }

    #[test]
    fn json_bool_to_lua() {
        let lua = Lua::new();
        let result = json_to_lua_value(&lua, &serde_json::json!(true)).unwrap();
        assert_eq!(result, Value::Boolean(true));

        let result = json_to_lua_value(&lua, &serde_json::json!(false)).unwrap();
        assert_eq!(result, Value::Boolean(false));
    }

    #[test]
    fn json_integer_to_lua() {
        let lua = Lua::new();
        let result = json_to_lua_value(&lua, &serde_json::json!(42)).unwrap();
        assert_eq!(result, Value::Integer(42));
    }

    #[test]
    fn json_negative_integer_to_lua() {
        let lua = Lua::new();
        let result = json_to_lua_value(&lua, &serde_json::json!(-99)).unwrap();
        assert_eq!(result, Value::Integer(-99));
    }

    #[test]
    fn json_float_to_lua() {
        let lua = Lua::new();
        // A number that cannot fit in i64 should become a Lua float.
        let val = serde_json::json!(1e18_f64 * 100.0); // larger than i64 max
        let result = json_to_lua_value(&lua, &val).unwrap();
        assert!(result.is_number());
    }

    #[test]
    fn json_string_to_lua() {
        let lua = Lua::new();
        let result = json_to_lua_value(&lua, &serde_json::json!("hello")).unwrap();
        match result {
            Value::String(s) => assert_eq!(s.to_str().unwrap(), "hello"),
            _ => panic!("expected string"),
        }
    }

    #[test]
    fn json_array_to_lua_table() {
        let lua = Lua::new();
        let result = json_to_lua_value(&lua, &serde_json::json!([10, 20, 30])).unwrap();
        match result {
            Value::Table(t) => {
                assert_eq!(t.get::<i64>(1).unwrap(), 10);
                assert_eq!(t.get::<i64>(2).unwrap(), 20);
                assert_eq!(t.get::<i64>(3).unwrap(), 30);
            }
            _ => panic!("expected table"),
        }
    }

    #[test]
    fn json_object_to_lua_table() {
        let lua = Lua::new();
        let result =
            json_to_lua_value(&lua, &serde_json::json!({"name": "bob", "age": 25})).unwrap();
        match result {
            Value::Table(t) => {
                assert_eq!(t.get::<String>("name").unwrap(), "bob");
                assert_eq!(t.get::<i64>("age").unwrap(), 25);
            }
            _ => panic!("expected table"),
        }
    }

    #[test]
    fn json_nested_to_lua() {
        let lua = Lua::new();
        let json = serde_json::json!({
            "items": [1, 2, 3],
            "meta": {"tag": "test"}
        });
        let result = json_to_lua_value(&lua, &json).unwrap();
        match result {
            Value::Table(t) => {
                let items: Table = t.get("items").unwrap();
                assert_eq!(items.get::<i64>(1).unwrap(), 1);
                assert_eq!(items.get::<i64>(2).unwrap(), 2);
                assert_eq!(items.get::<i64>(3).unwrap(), 3);

                let meta: Table = t.get("meta").unwrap();
                assert_eq!(meta.get::<String>("tag").unwrap(), "test");
            }
            _ => panic!("expected table"),
        }
    }

    // ── Round-trip: JSON → Lua → JSON ──

    #[test]
    fn roundtrip_null() {
        let lua = Lua::new();
        let json = serde_json::Value::Null;
        let lua_val = json_to_lua_value(&lua, &json).unwrap();
        let back = lua_value_to_json(lua_val).unwrap();
        assert_eq!(back, serde_json::Value::Null);
    }

    #[test]
    fn roundtrip_bool() {
        let lua = Lua::new();
        for b in [true, false] {
            let json = serde_json::Value::Bool(b);
            let lua_val = json_to_lua_value(&lua, &json).unwrap();
            let back = lua_value_to_json(lua_val).unwrap();
            assert_eq!(back, json);
        }
    }

    #[test]
    fn roundtrip_integer() {
        let lua = Lua::new();
        let json = serde_json::json!(42);
        let lua_val = json_to_lua_value(&lua, &json).unwrap();
        let back = lua_value_to_json(lua_val).unwrap();
        assert_eq!(back, json);
    }

    #[test]
    fn roundtrip_string() {
        let lua = Lua::new();
        let json = serde_json::json!("hello world");
        let lua_val = json_to_lua_value(&lua, &json).unwrap();
        let back = lua_value_to_json(lua_val).unwrap();
        assert_eq!(back, json);
    }

    #[test]
    fn roundtrip_array() {
        let lua = Lua::new();
        let json = serde_json::json!([1, 2, 3]);
        let lua_val = json_to_lua_value(&lua, &json).unwrap();
        let back = lua_value_to_json(lua_val).unwrap();
        assert_eq!(back, json);
    }

    #[test]
    fn roundtrip_object() {
        let lua = Lua::new();
        let json = serde_json::json!({"x": 1, "y": 2});
        let lua_val = json_to_lua_value(&lua, &json).unwrap();
        let back = lua_value_to_json(lua_val).unwrap();
        assert_eq!(back, json);
    }

    #[test]
    fn roundtrip_nested_structure() {
        let lua = Lua::new();
        let json = serde_json::json!({
            "users": [
                {"name": "alice", "age": 30},
                {"name": "bob", "age": 25}
            ],
            "count": 2,
            "active": true
        });
        let lua_val = json_to_lua_value(&lua, &json).unwrap();
        let back = lua_value_to_json(lua_val).unwrap();
        assert_eq!(back, json);
    }

    #[test]
    fn roundtrip_empty_array() {
        let lua = Lua::new();
        let json = serde_json::json!([]);
        let lua_val = json_to_lua_value(&lua, &json).unwrap();
        let back = lua_value_to_json(lua_val).unwrap();
        assert_eq!(back, json);
    }

    #[test]
    fn roundtrip_empty_object() {
        // An empty JSON object {} becomes an empty Lua table, which round-trips
        // back as an empty array []. This is a known limitation: Lua tables have
        // no way to distinguish an empty object from an empty array.
        let lua = Lua::new();
        let json = serde_json::json!({});
        let lua_val = json_to_lua_value(&lua, &json).unwrap();
        let back = lua_value_to_json(lua_val).unwrap();
        assert_eq!(back, serde_json::json!([]));
    }

    // ── lua_value_to_serial_config ──

    #[test]
    fn serial_config_from_string() {
        let lua = Lua::new();
        let port = lua.create_string("COM3").unwrap();
        let config = lua_value_to_serial_config(Value::String(port)).unwrap();
        assert_eq!(config.port_name, "COM3");
        // Defaults should apply
        assert_eq!(config.baud_rate, 115_200);
        assert_eq!(config.timeout_ms, 1);
    }

    #[test]
    fn serial_config_from_table_full() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        table.set("port_name", "COM5").unwrap();
        table.set("baud_rate", 9600_u32).unwrap();
        table.set("timeout_ms", 100_u64).unwrap();
        table.set("data_bits", "7").unwrap();
        table.set("stop_bits", "2").unwrap();
        table.set("parity", "odd").unwrap();

        let config = lua_value_to_serial_config(Value::Table(table)).unwrap();
        assert_eq!(config.port_name, "COM5");
        assert_eq!(config.baud_rate, 9600);
        assert_eq!(config.timeout_ms, 100);
        assert_eq!(config.data_bits, DataBits::Seven);
        assert_eq!(config.stop_bits, StopBits::Two);
        assert_eq!(config.parity, Parity::Odd);
    }

    #[test]
    fn serial_config_from_table_minimal() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        table.set("port_name", "COM1").unwrap();
        // Only port_name is required; everything else should default.
        let config = lua_value_to_serial_config(Value::Table(table)).unwrap();
        assert_eq!(config.port_name, "COM1");
        assert_eq!(config.baud_rate, 115_200);
        assert_eq!(config.data_bits, DataBits::Eight);
        assert_eq!(config.stop_bits, StopBits::One);
        assert_eq!(config.parity, Parity::None);
        assert_eq!(config.timeout_ms, 1);
    }

    #[test]
    fn serial_config_from_table_with_baud_alias() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        table.set("port_name", "COM2").unwrap();
        table.set("baud", 4800_u32).unwrap();
        // "baud" is an alias for "baud_rate"
        let config = lua_value_to_serial_config(Value::Table(table)).unwrap();
        assert_eq!(config.baud_rate, 4800);
    }

    #[test]
    fn serial_config_from_table_with_port_alias() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        table.set("port", "COM4").unwrap();
        // "port" is an alias for "port_name"
        let config = lua_value_to_serial_config(Value::Table(table)).unwrap();
        assert_eq!(config.port_name, "COM4");
    }

    #[test]
    fn serial_config_from_table_invalid_data_bits_defaults() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        table.set("port_name", "COM1").unwrap();
        table.set("data_bits", "99").unwrap();
        // Invalid data_bits string should default to Eight
        let config = lua_value_to_serial_config(Value::Table(table)).unwrap();
        assert_eq!(config.data_bits, DataBits::Eight);
    }

    #[test]
    fn serial_config_from_table_invalid_parity_defaults() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        table.set("port_name", "COM1").unwrap();
        table.set("parity", "invalid").unwrap();
        // Invalid parity string should default to None
        let config = lua_value_to_serial_config(Value::Table(table)).unwrap();
        assert_eq!(config.parity, Parity::None);
    }

    #[test]
    fn serial_config_from_invalid_type_returns_error() {
        let result = lua_value_to_serial_config(Value::Integer(42));
        assert!(result.is_err());
        match result.unwrap_err() {
            mlua::Error::RuntimeError(msg) => {
                assert!(msg.contains("expects a string or table"));
            }
            other => panic!("expected RuntimeError, got {:?}", other),
        }
    }

    #[test]
    fn serial_config_from_table_missing_port_name_returns_error() {
        let lua = Lua::new();
        let table = lua.create_table().unwrap();
        table.set("baud_rate", 9600_u32).unwrap();
        // port_name is required; missing it should error
        let result = lua_value_to_serial_config(Value::Table(table));
        assert!(result.is_err());
    }

    // ── number_to_json ──

    #[test]
    fn number_to_json_finite() {
        let result = number_to_json(1.5).unwrap();
        assert_eq!(result, serde_json::json!(1.5));
    }

    #[test]
    fn number_to_json_nan_returns_error() {
        let result = number_to_json(f64::NAN);
        assert!(result.is_err());
    }

    #[test]
    fn number_to_json_infinity_returns_error() {
        let result = number_to_json(f64::INFINITY);
        assert!(result.is_err());
    }

    // ── lua_key_to_string ──

    #[test]
    fn lua_string_key_to_string() {
        let lua = Lua::new();
        let s = lua.create_string("key").unwrap();
        let result = lua_key_to_string(Value::String(s)).unwrap();
        assert_eq!(result, "key");
    }

    #[test]
    fn lua_integer_key_to_string() {
        let result = lua_key_to_string(Value::Integer(3)).unwrap();
        assert_eq!(result, "3");
    }

    #[test]
    fn lua_number_key_to_string() {
        let result = lua_key_to_string(Value::Number(1.5)).unwrap();
        assert_eq!(result, "1.5");
    }

    #[test]
    fn lua_invalid_key_type_returns_error() {
        let result = lua_key_to_string(Value::Boolean(true));
        assert!(result.is_err());
    }

    // ── lua_value_to_payload ──

    #[test]
    fn lua_nil_to_payload_empty() {
        let result = lua_value_to_payload(Value::Nil).unwrap();
        assert_eq!(result, Payload::Empty);
    }

    #[test]
    fn lua_bool_to_payload_json() {
        let result = lua_value_to_payload(Value::Boolean(true)).unwrap();
        assert_eq!(result, Payload::Json(serde_json::Value::Bool(true)));
    }

    #[test]
    fn lua_integer_to_payload_json() {
        let result = lua_value_to_payload(Value::Integer(7)).unwrap();
        assert_eq!(result, Payload::Json(serde_json::json!(7)));
    }

    #[test]
    fn lua_string_to_payload_text() {
        let lua = Lua::new();
        let s = lua.create_string("hello").unwrap();
        let result = lua_value_to_payload(Value::String(s)).unwrap();
        assert_eq!(result, Payload::Text("hello".to_owned()));
    }

    #[test]
    fn lua_function_to_payload_returns_error() {
        let lua = Lua::new();
        let func = lua.create_function(|_, ()| Ok(())).unwrap();
        let result = lua_value_to_payload(Value::Function(func));
        assert!(result.is_err());
    }

    // ── payload_to_json ──

    #[test]
    fn payload_empty_to_json_null() {
        let result = payload_to_json(Payload::Empty);
        assert_eq!(result, serde_json::Value::Null);
    }

    #[test]
    fn payload_text_to_json_string() {
        let result = payload_to_json(Payload::Text("hi".to_owned()));
        assert_eq!(result, serde_json::json!("hi"));
    }

    #[test]
    fn payload_bytes_to_json_array() {
        let result = payload_to_json(Payload::Bytes(vec![1, 2, 3]));
        assert_eq!(result, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn payload_json_passthrough() {
        let json = serde_json::json!({"key": 42});
        let result = payload_to_json(Payload::Json(json.clone()));
        assert_eq!(result, json);
    }
}
