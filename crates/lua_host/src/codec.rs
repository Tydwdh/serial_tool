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
}
