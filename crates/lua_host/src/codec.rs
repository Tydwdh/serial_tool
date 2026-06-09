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
