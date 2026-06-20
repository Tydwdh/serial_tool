//! `ctx.dialog.*` — 文件对话框 API。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mlua::{Lua, Table, Value};

use crate::host_services::{DialogRequest, FileFilter};

pub(crate) fn create_dialog_api(
    lua: &Lua,
    dialog_sender: crossbeam_channel::Sender<DialogRequest>,
    plugin_id: String,
    stop_flag: Option<Arc<AtomicBool>>,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set(
        "open_file",
        lua.create_function(move |cb_lua, config: Value| {
            let obj = match config {
                Value::Table(t) => t,
                _ => return Ok(Value::Nil),
            };

            let title: String = obj.get("title").unwrap_or_else(|_| "选择文件".to_owned());
            let filters: Vec<FileFilter> = parse_lua_filters(&obj)?;

            let (response_sender, response_receiver) =
                crossbeam_channel::bounded::<Option<PathBuf>>(1);

            match dialog_sender.try_send(DialogRequest {
                plugin_id: plugin_id.clone(),
                title,
                filters,
                response_sender,
            }) {
                Ok(()) => {}
                Err(crossbeam_channel::TrySendError::Full(_)) => {
                    return Err(mlua::Error::RuntimeError(
                        "dialog: too many pending requests".into(),
                    ));
                }
                Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                    return Err(mlua::Error::RuntimeError(
                        "dialog: host not available".into(),
                    ));
                }
            }

            // 100ms 轮询，支持插件停止时及时返回
            let result = loop {
                if let Some(ref stop) = stop_flag
                    && stop.load(Ordering::Relaxed)
                {
                    break None;
                }
                match response_receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(Some(path)) => {
                        break Some(path.display().to_string());
                    }
                    Ok(None) => break None,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break None,
                }
            };

            match result {
                Some(path_str) => {
                    let s = cb_lua.create_string(&path_str)?;
                    Ok(Value::String(s))
                }
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    Ok(table)
}

pub(crate) fn parse_lua_filters(obj: &Table) -> mlua::Result<Vec<FileFilter>> {
    let filters_table: Option<Table> = obj.get("filters").ok();
    let Some(filters_table) = filters_table else {
        return Ok(vec![FileFilter {
            name: "所有文件".to_owned(),
            extensions: vec!["*".to_owned()],
        }]);
    };

    let mut result = Vec::new();
    for pair in filters_table.pairs::<Value, Value>() {
        let (_, value) = pair?;
        if let Value::Table(ft) = value {
            let name: String = ft.get("name").unwrap_or_default();
            let exts: Vec<String> = ft
                .get::<Table>("extensions")
                .map(|t| {
                    t.sequence_values::<String>()
                        .filter_map(|v| v.ok())
                        .collect()
                })
                .unwrap_or_default();
            result.push(FileFilter {
                name,
                extensions: exts,
            });
        }
    }
    if result.is_empty() {
        result.push(FileFilter {
            name: "所有文件".to_owned(),
            extensions: vec!["*".to_owned()],
        });
    }
    Ok(result)
}
