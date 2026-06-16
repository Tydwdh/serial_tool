use mlua::{Lua, Table, Value};

/// 注册 hw.codec 到 package.preload，live 和 replay 均可 require("hw.codec")。
pub fn register_codec(lua: &Lua) -> mlua::Result<()> {
    let package: Table = lua.globals().get("package")?;
    let preload: Table = package.get("preload")?;
    preload.set("hw.codec", lua.create_function(create_codec_table)?)?;
    Ok(())
}

fn create_codec_table(lua: &Lua, (): ()) -> mlua::Result<Value> {
    let tbl = lua.create_table()?;

    tbl.set(
        "to_hex",
        lua.create_function(|_, bytes: mlua::String| {
            let hex: String = bytes
                .as_bytes()
                .iter()
                .map(|b| format!("{:02X}", b))
                .collect();
            Ok(hex)
        })?,
    )?;

    tbl.set(
        "from_hex",
        lua.create_function(|lua, hex: mlua::String| {
            let hex = hex.to_str()?.replace(' ', "");
            if hex.len() % 2 != 0 {
                return Err(mlua::Error::RuntimeError(
                    "hex string must have even length".into(),
                ));
            }
            let bytes: Vec<u8> = (0..hex.len())
                .step_by(2)
                .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                .collect();
            if bytes.len() * 2 != hex.len() {
                return Err(mlua::Error::RuntimeError("invalid hex string".into()));
            }
            Ok(Value::String(lua.create_string(&bytes)?))
        })?,
    )?;

    tbl.set(
        "xor8",
        lua.create_function(|_, text: mlua::String| {
            let xor: u8 = text.as_bytes().iter().fold(0, |acc, &b| acc ^ b);
            Ok(xor)
        })?,
    )?;

    tbl.set(
        "crc16_modbus",
        lua.create_function(|_, bytes: mlua::String| {
            let mut crc: u16 = 0xFFFF;
            for byte in bytes.as_bytes().iter() {
                crc ^= *byte as u16;
                for _ in 0..8 {
                    if crc & 0x0001 != 0 {
                        crc = (crc >> 1) ^ 0xA001;
                    } else {
                        crc >>= 1;
                    }
                }
            }
            Ok(crc)
        })?,
    )?;

    tbl.set(
        "trim_line",
        lua.create_function(|_, line: mlua::String| {
            let s = line
                .to_str()?
                .trim_end_matches('\r')
                .trim_end_matches('\n')
                .to_owned();
            Ok(s)
        })?,
    )?;

    tbl.set(
        "split_lines",
        lua.create_function(|lua, text: mlua::String| {
            let lines: Vec<String> = text
                .to_str()?
                .lines()
                .map(|l| l.trim_end_matches('\r').to_owned())
                .collect();
            let arr = lua.create_table()?;
            for (i, line) in lines.iter().enumerate() {
                arr.set(i + 1, line.as_str())?;
            }
            Ok(Value::Table(arr))
        })?,
    )?;

    Ok(Value::Table(tbl))
}

/// 注册 hw.utils 到 package.preload。提供常用工具函数。
pub fn register_utils(lua: &Lua) -> mlua::Result<()> {
    let package: Table = lua.globals().get("package")?;
    let preload: Table = package.get("preload")?;
    preload.set("hw.utils", lua.create_function(create_utils_table)?)?;
    Ok(())
}

fn create_utils_table(lua: &Lua, (): ()) -> mlua::Result<Value> {
    let tbl = lua.create_table()?;

    // hw.utils.split(str, sep) → array of strings
    tbl.set(
        "split",
        lua.create_function(|lua, (s, sep): (mlua::String, mlua::String)| {
            let s = s.to_str()?.to_owned();
            let sep = sep.to_str()?.to_owned();
            let parts: Vec<&str> = s.split(&sep).collect();
            let arr = lua.create_table()?;
            for (i, part) in parts.iter().enumerate() {
                arr.set(i + 1, *part)?;
            }
            Ok(Value::Table(arr))
        })?,
    )?;

    // hw.utils.join(arr, sep) → string
    tbl.set(
        "join",
        lua.create_function(|_lua, (arr, sep): (Table, mlua::String)| {
            let mut parts: Vec<String> = Vec::new();
            for i in 1..=arr.raw_len() {
                if let Ok(v) = arr.get::<mlua::String>(i) {
                    parts.push(v.to_str()?.to_owned());
                }
            }
            Ok(parts.join(sep.to_str()?.as_ref()))
        })?,
    )?;

    // hw.utils.parse_number(s) → number or nil
    tbl.set(
        "parse_number",
        lua.create_function(|_lua, s: mlua::String| {
            let s = s.to_str()?.trim().to_owned();
            if let Ok(i) = s.parse::<i64>() {
                Ok(Value::Integer(i))
            } else if let Ok(f) = s.parse::<f64>() {
                Ok(Value::Number(f))
            } else {
                Ok(Value::Nil)
            }
        })?,
    )?;

    // hw.utils.table_keys(t) → array of keys
    tbl.set(
        "table_keys",
        lua.create_function(|lua, t: Table| {
            let arr = lua.create_table()?;
            let mut i = 0;
            for (key, _) in t.pairs::<Value, Value>().flatten() {
                i += 1;
                arr.set(i, key)?;
            }
            Ok(Value::Table(arr))
        })?,
    )?;

    // hw.utils.starts_with(s, prefix) → bool
    tbl.set(
        "starts_with",
        lua.create_function(|_lua, (s, prefix): (mlua::String, mlua::String)| {
            Ok(s.to_str()?.as_ref().starts_with(prefix.to_str()?.as_ref()))
        })?,
    )?;

    // hw.utils.ends_with(s, suffix) → bool
    tbl.set(
        "ends_with",
        lua.create_function(|_lua, (s, suffix): (mlua::String, mlua::String)| {
            Ok(s.to_str()?.as_ref().ends_with(suffix.to_str()?.as_ref()))
        })?,
    )?;

    // hw.utils.format_size(bytes) → human-readable string
    tbl.set(
        "format_size",
        lua.create_function(|_lua, bytes: u64| {
            let units = ["B", "KB", "MB", "GB"];
            let mut size = bytes as f64;
            let mut unit_idx = 0;
            while size >= 1024.0 && unit_idx < units.len() - 1 {
                size /= 1024.0;
                unit_idx += 1;
            }
            if unit_idx == 0 {
                Ok(format!("{} {}", bytes, units[unit_idx]))
            } else {
                Ok(format!("{:.1} {}", size, units[unit_idx]))
            }
        })?,
    )?;

    Ok(Value::Table(tbl))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::{Lua, LuaOptions, StdLib};

    fn setup() -> Lua {
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::PACKAGE,
            LuaOptions::default(),
        )
        .unwrap();
        register_codec(&lua).unwrap();
        register_utils(&lua).unwrap();
        lua
    }

    #[test]
    fn to_hex_encodes_correctly() {
        let lua = setup();
        lua.load("local c = require('hw.codec'); assert(c.to_hex('\\x00\\xFF\\xAB') == '00FFAB')")
            .exec()
            .unwrap();
    }

    #[test]
    fn from_hex_decodes_correctly() {
        let lua = setup();
        lua.load(
            "local c = require('hw.codec'); assert(c.from_hex('00FFAB') == '\\x00\\xFF\\xAB')",
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn from_hex_rejects_odd_length() {
        let lua = setup();
        let result = lua
            .load("local c = require('hw.codec'); c.from_hex('ABC')")
            .exec();
        assert!(result.is_err());
    }

    #[test]
    fn from_hex_rejects_invalid_chars() {
        let lua = setup();
        let result = lua
            .load("local c = require('hw.codec'); c.from_hex('ZZ')")
            .exec();
        assert!(result.is_err());
    }

    #[test]
    fn xor8_computes_correctly() {
        let lua = setup();
        // XOR of 'A'(65) ^ 'B'(66) ^ 'C'(67) = 64
        lua.load("local c = require('hw.codec'); assert(c.xor8('ABC') == 64)")
            .exec()
            .unwrap();
    }

    #[test]
    fn crc16_modbus_known_value() {
        let lua = setup();
        // CRC-16 Modbus for empty input should be 0xFFFF
        lua.load("local c = require('hw.codec'); assert(c.crc16_modbus('') == 0xFFFF)")
            .exec()
            .unwrap();
    }

    #[test]
    fn trim_line_strips_newline() {
        let lua = setup();
        lua.load("local c = require('hw.codec'); assert(c.trim_line('hello\\n') == 'hello')")
            .exec()
            .unwrap();
    }

    #[test]
    fn trim_line_keeps_normal_text() {
        let lua = setup();
        lua.load("local c = require('hw.codec'); assert(c.trim_line('hello') == 'hello')")
            .exec()
            .unwrap();
    }

    #[test]
    fn split_lines_works() {
        let lua = setup();
        lua.load(
            r#"
            local c = require('hw.codec')
            local lines = c.split_lines("a\nb\nc")
            assert(#lines == 3)
            assert(lines[1] == 'a')
            assert(lines[2] == 'b')
            assert(lines[3] == 'c')
        "#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn split_lines_strips_cr() {
        let lua = setup();
        lua.load(
            r#"
            local c = require('hw.codec')
            local lines = c.split_lines("a\r\nb\r\n")
            assert(#lines == 2)
            assert(lines[1] == 'a')
            assert(lines[2] == 'b')
        "#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn utils_split_works() {
        let lua = setup();
        lua.load(
            r#"local u = require('hw.utils'); local parts = u.split('a,b,c', ','); assert(#parts == 3 and parts[1] == 'a' and parts[3] == 'c')"#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn utils_parse_number() {
        let lua = setup();
        lua.load("local u = require('hw.utils'); assert(u.parse_number('42') == 42); assert(u.parse_number('3.14') > 3.13)")
            .exec()
            .unwrap();
    }

    #[test]
    fn utils_starts_with() {
        let lua = setup();
        lua.load("local u = require('hw.utils'); assert(u.starts_with('hello', 'hel')); assert(not u.starts_with('hello', 'lo'))")
            .exec()
            .unwrap();
    }

    #[test]
    fn utils_join_works() {
        let lua = setup();
        lua.load("local u = require('hw.utils'); assert(u.join({'a','b','c'}, ',') == 'a,b,c')")
            .exec()
            .unwrap();
    }

    #[test]
    fn utils_format_size() {
        let lua = setup();
        lua.load("local u = require('hw.utils'); assert(u.format_size(1024) == '1.0 KB')")
            .exec()
            .unwrap();
    }
}
