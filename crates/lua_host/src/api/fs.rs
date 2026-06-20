//! `ctx.fs.*` — 文件读取 API（受 FileAccessBroker 授权保护）。

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;

use mlua::{Lua, Table, Value};
use parking_lot::Mutex;

use crate::host_services::FileAccessBroker;

pub(crate) fn create_fs_api(
    lua: &Lua,
    broker: Arc<FileAccessBroker>,
    plugin_id: String,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    let broker_read = broker.clone();
    let pid_read = plugin_id.clone();
    table.set(
        "read_text",
        lua.create_function(move |_lua, path: String| {
            let p = PathBuf::from(&path);
            if !broker_read.is_authorized(&pid_read, &p) {
                return Err(mlua::Error::RuntimeError(format!(
                    "文件未授权: {path}. 请先通过文件选择对话框选择文件。"
                )));
            }
            // 先查 metadata，避免读超大文件
            let meta = std::fs::metadata(&p)
                .map_err(|e| mlua::Error::RuntimeError(format!("无法获取文件信息: {e}")))?;
            if meta.len() > 16 * 1024 * 1024 {
                return Err(mlua::Error::RuntimeError("文件超过 16 MiB 上限".to_owned()));
            }
            let content = std::fs::read_to_string(&p)
                .map_err(|e| mlua::Error::RuntimeError(format!("读取文件失败: {e}")))?;
            Ok(content)
        })?,
    )?;

    let broker_lines = broker.clone();
    let pid_lines = plugin_id.clone();
    table.set(
        "read_lines",
        lua.create_function(move |lua, path: String| {
            let p = PathBuf::from(&path);
            if !broker_lines.is_authorized(&pid_lines, &p) {
                return Err(mlua::Error::RuntimeError(format!(
                    "文件未授权: {path}. 请先通过文件选择对话框选择文件。"
                )));
            }
            // 先查 metadata，避免读超大文件
            let meta = std::fs::metadata(&p)
                .map_err(|e| mlua::Error::RuntimeError(format!("无法获取文件信息: {e}")))?;
            if meta.len() > 16 * 1024 * 1024 {
                return Err(mlua::Error::RuntimeError("文件超过 16 MiB 上限".to_owned()));
            }
            let content = std::fs::read_to_string(&p)
                .map_err(|e| mlua::Error::RuntimeError(format!("读取文件失败: {e}")))?;
            let lines: Arc<Vec<String>> = Arc::new(content.lines().map(String::from).collect());
            let index = Arc::new(Mutex::new(0usize));
            let lines_len = lines.len();

            // 返回迭代函数：每次调用返回下一行，结束时返回 nil
            let iter_fn = lua.create_function(move |lua, ()| {
                let mut i = index.lock();
                if *i >= lines_len {
                    return Ok(Value::Nil);
                }
                let line = lines[*i].clone();
                *i += 1;
                Ok(Value::String(lua.create_string(&line)?))
            })?;
            Ok(Value::Function(iter_fn))
        })?,
    )?;

    let broker_stream = broker;
    let pid_stream = plugin_id;
    table.set(
        "read_lines_stream",
        lua.create_function(move |lua, path: String| {
            let p = PathBuf::from(&path);
            if !broker_stream.is_authorized(&pid_stream, &p) {
                return Err(mlua::Error::RuntimeError(format!(
                    "文件未授权: {path}. 请先通过文件选择对话框选择文件。"
                )));
            }

            let file = std::fs::File::open(&p)
                .map_err(|e| mlua::Error::RuntimeError(format!("读取文件失败: {e}")))?;
            let reader = std::rc::Rc::new(std::cell::RefCell::new(std::io::BufReader::new(file)));

            let iter_reader = reader.clone();
            let iter_fn = lua.create_function(move |lua, ()| {
                let mut line = String::new();
                let bytes = iter_reader
                    .borrow_mut()
                    .read_line(&mut line)
                    .map_err(|e| mlua::Error::RuntimeError(format!("读取文件失败: {e}")))?;
                if bytes == 0 {
                    return Ok(Value::Nil);
                }

                let trimmed = line
                    .trim_end_matches('\n')
                    .trim_end_matches('\r')
                    .to_owned();
                Ok(Value::String(lua.create_string(&trimmed)?))
            })?;

            Ok(Value::Function(iter_fn))
        })?,
    )?;

    Ok(table)
}
