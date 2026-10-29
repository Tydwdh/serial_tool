use crate::app::WorkbenchApp;
use tool_application::AppCommand;
use tool_application::query::PluginStateView;
use tool_core::{Direction, Event, LogLevel, Payload, topics};

impl WorkbenchApp {
    pub(crate) fn publish_plugin_command_execute(
        &mut self,
        plugin_id: &str,
        command_id: &str,
        payload: &serde_json::Value,
    ) {
        let mut payload = payload.clone();
        if let Some(object) = payload.as_object_mut() {
            object.insert("plugin_id".to_owned(), serde_json::json!(plugin_id));
            object.insert("command".to_owned(), serde_json::json!(command_id));
            object.insert("origin".to_owned(), serde_json::json!("host.command"));
        }

        self.workbench.publish_event(Event::new(
            topics::PLUGIN_COMMAND_EXECUTE,
            "plugin.command",
            Direction::Internal,
            Payload::Json(payload),
        ));
    }

    /// 插件生命周期：禁用清理 + ingest + 事件处理。
    pub(super) fn tick_plugin_lifecycle(&mut self) {
        self.poll_dialog_requests();
        self.handle_file_browse_requests();

        // 初始扫描在后台完成后，按配置启用插件；此处只在 Discovered 状态
        // 尝试一次，避免扫描尚未完成时把“未找到”误报成启动错误。
        let configured = self.workbench.query_enabled_plugin_ids();
        for plugin_id in configured {
            if self.workbench.plugin_state(&plugin_id) != Some(PluginStateView::Discovered) {
                continue;
            }
            if let Err(error) = self.workbench.dispatch(AppCommand::EnablePlugin {
                plugin_id: plugin_id.clone(),
            }) {
                self.log(
                    LogLevel::Warn,
                    format!("恢复插件 {plugin_id} 失败：{error}"),
                );
            }
        }

        self.workbench.process_plugin_lifecycle();
        self.dynamic_panels.ingest(&mut self.panels);
        // 插件动态面板并入统一 PanelRegistry（内置面板同表），须在 ingest 之后。
        self.panel_registry
            .sync_dynamic_panels(&self.dynamic_panels);
        self.process_contribution_set_value();

        for plugin_id in self.workbench.take_plugin_cleanup_requests() {
            let removed = self.dynamic_panels.remove_by_plugin(&plugin_id);
            for panel_id in &removed {
                self.panels
                    .close_tab(tool_panels::PanelId::dynamic(panel_id));
            }
            self.workbench.clear_plugin_file_authorization(&plugin_id);
            let prefix = format!("{plugin_id}:");
            self.contribution_states
                .retain(|key, _| !key.starts_with(&prefix));
        }

        let terminal_started = self.perf.begin_frame();
        let terminal_ingested = self.terminal_panel.ingest_pending();
        self.perf
            .record_terminal_ingest(terminal_started, terminal_ingested);
    }

    /// 处理插件通过 ctx.ui.set_contribution_value 对 UI contribution 的状态更新。
    /// 使用专用 topic `ui.contribution.set.value`，与动态面板的 `ui.form.set_value` 隔离。
    fn process_contribution_set_value(&mut self) {
        for event in self.ui_events.drain_contribution_set_value(64) {
            let Payload::Json(payload) = event.payload else {
                continue;
            };
            // 要求 panel_id == "__contribution__" 作为哨兵，防止误消费面板事件
            if payload.get("panel_id").and_then(serde_json::Value::as_str)
                != Some("__contribution__")
            {
                continue;
            }
            let Some(contribution_id) = payload.get("field_id").and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let Some(value) = payload.get("value") else {
                continue;
            };
            // 从事件 source 提取 plugin_id（格式 "plugin:{plugin_id}"）
            let plugin_id = event
                .source
                .strip_prefix("plugin:")
                .unwrap_or(&event.source);
            self.set_contribution_value(plugin_id, contribution_id, value.clone());
        }
    }
}
