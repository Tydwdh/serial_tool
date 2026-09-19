//! `ctx.fs.*` — 文件读取 API（受 FileAccessBroker 授权保护）。

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mlua::{Lua, Table, Value};
use parking_lot::Mutex;

use crate::host_services::FileAccessBroker;

const MAX_TEXT_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// 授权校验：未授权时的错误要引导用户回到文件对话框重选。
fn ensure_authorized(
    broker: &FileAccessBroker,
    plugin_id: &str,
    path: &str,
) -> mlua::Result<PathBuf> {
    let p = PathBuf::from(path);
    if !broker.is_authorized(plugin_id, &p) {
        return Err(mlua::Error::RuntimeError(format!(
            "文件未授权: {path}. 请先通过文件选择对话框选择文件。"
        )));
    }
    Ok(p)
}

/// 整本读取文本：先看 metadata 再读，避免超大文件把内存打满。
fn read_capped_text(path: &Path) -> mlua::Result<String> {
    let meta = std::fs::metadata(path)
        .map_err(|e| mlua::Error::RuntimeError(format!("无法获取文件信息: {e}")))?;
    if meta.len() > MAX_TEXT_FILE_BYTES {
        return Err(mlua::Error::RuntimeError("文件超过 16 MiB 上限".to_owned()));
    }
    std::fs::read_to_string(path)
        .map_err(|e| mlua::Error::RuntimeError(format!("读取文件失败: {e}")))
}

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
            let p = ensure_authorized(&broker_read, &pid_read, &path)?;
            read_capped_text(&p)
        })?,
    )?;

    let broker_lines = broker.clone();
    let pid_lines = plugin_id.clone();
    table.set(
        "read_lines",
        lua.create_function(move |lua, path: String| {
            let p = ensure_authorized(&broker_lines, &pid_lines, &path)?;
            let content = read_capped_text(&p)?;
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
            let p = ensure_authorized(&broker_stream, &pid_stream, &path)?;

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
