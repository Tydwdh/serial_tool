use crate::app::WorkbenchApp;
use tool_application::api::core::{Direction, Event, Payload, topics};

impl WorkbenchApp {
    /// 发布插件命令动作（模拟 UI 按钮点击）。
    pub(crate) fn publish_plugin_command_action(&mut self, plugin_id: &str, command_id: &str) {
        // 查找该插件的 UI contribution 信息以确定是否 record_send_input
        let summaries = self.workbench.query_plugins().summaries;
        let record_send_input = summaries
            .iter()
            .find(|s| s.id == plugin_id)
            .and_then(|s| {
                s.contributes
                    .ui
                    .iter()
                    .find(|ui| ui.command.as_deref() == Some(command_id))
            })
            .map(|ui| ui.record_send_input)
            .unwrap_or(false);

        if record_send_input {
            self.record_send_history(self.send.input.clone());
        }

        // Authorize file access if plugin has fs.read.user_selected permission
        let has_fs_permission = summaries
            .iter()
            .find(|s| s.id == plugin_id)
            .map(|s| s.permissions.iter().any(|p| p == "fs.read.user_selected"))
            .unwrap_or(false);

        if has_fs_permission {
            let input = self.send.input.trim();
            if !input.is_empty() && input.lines().count() == 1 {
                let path = std::path::PathBuf::from(input.trim_matches('"'));
                if path.is_file() {
                    self.workbench.authorize_plugin_file(plugin_id, path);
                }
            }
        }

        let context = serde_json::json!({
            "slot": "send.toolbar",
            "send": {
                "input": self.send.input.clone(),
                "target_port": self.send.target_port.clone(),
                "target_port_open": self.send_target_port_open(),
                "hex_mode": self.send.hex_mode,
                "line_ending": {
                    "label": self.send.line_ending.label(),
                    "suffix": self.send.line_ending.suffix(),
                },
                "periodic_enabled": self.send.periodic_enabled,
                "periodic_interval_ms": self.send.periodic_interval_ms,
            },
            "serial": {
                "selected_port": self.serial.selected_port.clone(),
                "open_ports": self.workbench.open_port_names(),
            }
        });

        let payload = serde_json::json!({
            "plugin_id": plugin_id,
            "contribution_id": command_id,
            "slot": "send.toolbar",
            "kind": "button",
            "command": command_id,
            "context": context,
        });

        self.publish_plugin_command_execute(plugin_id, command_id, &payload);
    }

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
        let configured = self.workbench.app_config().enabled_plugins.clone();
        for plugin_id in configured {
            if self.workbench.plugin_state(&plugin_id)
                == Some(tool_application::api::extension::PluginState::Discovered)
                && let Err(error) = self.workbench.enable_plugin(&plugin_id)
            {
                self.log(
                    tool_application::api::core::LogLevel::Warn,
                    format!("恢复插件 {plugin_id} 失败：{error}"),
                );
            }
        }

        self.workbench.process_plugin_lifecycle();
        // 插件命令并入统一 CommandRegistry（内置命令同表）。
        // 先 clone summaries 再重建，避免 plugin_summaries 的 &self 借用与
        // &mut self.commands 冲突。
        let summaries = self.plugin_summaries().to_vec();
        self.commands.rebuild_plugin_commands(&summaries);
        self.dynamic_panels.ingest(&mut self.panels);
        // 插件动态面板并入统一 PanelRegistry（内置面板同表），须在 ingest 之后。
        self.panel_registry
            .sync_dynamic_panels(&self.dynamic_panels);
        self.process_contribution_set_value();

        for plugin_id in self.workbench.take_plugin_cleanup_requests() {
            let removed = self.dynamic_panels.remove_by_plugin(&plugin_id);
            for id in &removed {
                self.panels.close_tab(tool_panels::PanelId::dynamic(id));
            }
            self.workbench.clear_plugin_file_authorization(&plugin_id);
            let prefix = format!("{plugin_id}:");
            self.contribution_states
                .retain(|key, _| !key.starts_with(&prefix));
        }

        let _terminal_ingested = self.terminal_panel.ingest_pending();
    }

    /// 处理插件通过 ctx.ui.set_contribution_value 对 UI contribution 的状态更新。
    /// 使用专用 topic `ui.contribution.set.value`，与动态面板的 `ui.form.set_value` 隔离。
    fn process_contribution_set_value(&mut self) {
        for event in self.ui_events.drain_contribution_set_value(64) {
            let tool_application::api::core::Payload::Json(payload) = event.payload else {
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
