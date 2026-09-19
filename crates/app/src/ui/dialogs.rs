use crate::app::WorkbenchApp;
use serde_json::Value;
use tool_core::{Direction, Event, LogLevel, Payload};

impl WorkbenchApp {
    /// 处理 Lua ctx.dialog.open_file 请求。每帧最多处理一个。
    pub(crate) fn poll_dialog_requests(&mut self) {
        let Some(request) = self.workbench.try_dialog_request() else {
            return;
        };
        let dialog = rfd::FileDialog::new().set_title(&request.title);
        let result = add_file_filters(dialog, &request.filters).pick_file();
        if let Some(path) = &result {
            self.workbench.authorize_plugin_file(
                &request.plugin_id,
                tool_platform::storage::FileHandle::from_native_path(path.clone()),
            );
        }
        let _ = request.response_sender.send(result);
    }

    /// 处理 ui.form.file_browse 请求。每帧最多处理一个，避免连续弹多个模态对话框。
    pub(crate) fn handle_file_browse_requests(&mut self) {
        let Some(event) = self.ui_events.try_file_browse() else {
            return;
        };
        if let Payload::Json(value) = event.payload {
            let panel_id = value.get("panel_id").and_then(Value::as_str).unwrap_or("");
            let field_id = value.get("field_id").and_then(Value::as_str).unwrap_or("");
            let filters: Vec<tool_lua_host::FileFilter> = value
                .get("filters")
                .and_then(Value::as_array)
                .map(|filter_values| {
                    filter_values
                        .iter()
                        .map(|filter_value| tool_lua_host::FileFilter {
                            name: filter_value
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                            extensions: filter_value
                                .get("extensions")
                                .and_then(Value::as_array)
                                .map(|extensions| {
                                    extensions
                                        .iter()
                                        .filter_map(|v| v.as_str().map(String::from))
                                        .collect()
                                })
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            let dialog = rfd::FileDialog::new().set_title("选择文件");
            let result = add_file_filters(dialog, &filters).pick_file();

            if let Some(selected_path) = &result {
                if let Some(owner) = self.dynamic_panels.panel_owner(panel_id) {
                    self.workbench.authorize_plugin_file(
                        owner,
                        tool_platform::storage::FileHandle::from_native_path(selected_path.clone()),
                    );
                } else {
                    self.log(
                        LogLevel::Warn,
                        format!("file 字段 {panel_id}/{field_id} 没有 owner plugin，跳过授权"),
                    );
                }

                self.workbench.publish_event(Event::new(
                    tool_core::topics::UI_FORM_FILE_SELECTED,
                    "ui",
                    Direction::Internal,
                    Payload::Json(serde_json::json!({
                        "panel_id": panel_id,
                        "field_id": field_id,
                        "path": selected_path.display().to_string(),
                    })),
                ));
            }
        }
    }
}

/// 把 `FileFilter` 列表挂到 rfd 对话框上；扩展名为空或以 `*` 打头的条目不生成过滤器。
fn add_file_filters(
    mut dialog: rfd::FileDialog,
    filters: &[tool_lua_host::FileFilter],
) -> rfd::FileDialog {
    for filter in filters {
        if filter.extensions.is_empty() || filter.extensions[0] == "*" {
            continue;
        }
        let extensions: Vec<&str> = filter.extensions.iter().map(|ext| ext.as_str()).collect();
        dialog = dialog.add_filter(&filter.name, &extensions);
    }
    dialog
}
