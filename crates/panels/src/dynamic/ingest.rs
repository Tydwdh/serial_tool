//! 动态面板事件摄入：`ingest` + 事件解析 + owner 鉴权。
//!
//! 从 `mod.rs` 抽出的事件处理逻辑。`ingest` 调度各类 UI 事件，
//! `create_from_event`/`remove_from_event` 解析创建/移除指令，
//! `is_allowed` 做跨插件 owner 校验，`handle_field_update` 处理字段更新。

use super::form_render::publish_form_changed;
use super::schema::{DynamicField, parse_fields};
use super::{DynamicPanel, LogEntry};
use crate::{AttitudePanel, ChartPanel, PanelKind, PanelManager};
use serde_json::Value;
use std::collections::VecDeque;
use tool_core::{Event, LogLevel, Payload, topics};

fn event_source_for_owner(event: &Event) -> &str {
    if event.source.starts_with("replay:") {
        event.meta_str("original_source").unwrap_or(&event.source)
    } else {
        &event.source
    }
}

impl super::DynamicPanels {
    pub fn ingest(&mut self, panel_manager: &mut PanelManager) {
        for event in self.subscription.drain_limited(500) {
            match self.create_from_event(event) {
                Ok(Some(id)) => panel_manager.add_tab(PanelKind::Dynamic(id)),
                Ok(None) => {}
                Err(error) => self.last_error = Some(error),
            }
        }

        for event in self.remove_subscription.drain_limited(500) {
            match self.remove_from_event(event) {
                Ok(Some(id)) => {
                    self.panels.remove(&id);
                    panel_manager.close_tab(PanelKind::Dynamic(id));
                }
                Ok(None) => {}
                Err(error) => self.last_error = Some(error),
            }
        }

        // UI 状态更新事件
        for event in self.set_value_subscription.drain_limited(500) {
            self.handle_field_update(event, |field, value| {
                field.value = value;
            });
        }
        for event in self.set_enabled_subscription.drain_limited(500) {
            self.handle_field_update(event, |field, val| {
                if let Some(enabled) = val.as_bool() {
                    field.enabled = enabled;
                }
            });
        }
        for event in self.set_visible_subscription.drain_limited(500) {
            self.handle_field_update(event, |field, val| {
                if let Some(visible) = val.as_bool() {
                    field.visible = visible;
                }
            });
        }
        // log append 事件
        for event in self.log_append_subscription.drain_limited(500) {
            let owner_source = event_source_for_owner(&event).to_owned();
            if let Payload::Json(val) = event.payload {
                let panel_id = val.get("panel_id").and_then(Value::as_str).unwrap_or("");
                let level_str = val.get("level").and_then(Value::as_str).unwrap_or("info");
                let msg = val.get("message").and_then(Value::as_str).unwrap_or("");
                let level = LogLevel::parse_name(level_str).unwrap_or(LogLevel::Info);
                // 用事件真实来源校验 owner，不信任 payload 里的 plugin_id
                let actual_plugin_id = owner_source
                    .strip_prefix("plugin:")
                    .unwrap_or(owner_source.as_str());
                if let Some(DynamicPanel::Log {
                    entries,
                    max_entries,
                    owner_plugin_id,
                    ..
                }) = self.panels.get_mut(panel_id)
                {
                    // owner 校验：拒绝非 owner 插件的写入
                    if let Some(owner) = owner_plugin_id.as_deref()
                        && owner != actual_plugin_id
                    {
                        continue;
                    }
                    entries.push_back(LogEntry {
                        timestamp_ms: tool_core::now_timestamp_ms(),
                        level,
                        message: msg.to_owned(),
                    });
                    while entries.len() > *max_entries {
                        entries.pop_front();
                    }
                }
            }
        }

        // file browse 事件由 main.rs 处理
        let _ = self.file_browse_subscription.drain_limited(500);

        // file selected 事件：更新字段值，并触发 form.changed（视为用户输入）
        // 只接受来自 ui/app 的事件，防止插件伪造文件选择结果
        for event in self.file_selected_subscription.drain_limited(500) {
            if event.source != "ui" && event.source != "app" {
                continue;
            }
            if let Payload::Json(val) = event.payload {
                let panel_id = val.get("panel_id").and_then(Value::as_str).unwrap_or("");
                let field_id = val.get("field_id").and_then(Value::as_str).unwrap_or("");
                let path = val.get("path").and_then(Value::as_str).unwrap_or("");
                if let Some(panel) = self.panels.get_mut(panel_id) {
                    let auto = matches!(
                        panel,
                        DynamicPanel::Form {
                            auto_apply: true,
                            ..
                        }
                    );
                    if let DynamicPanel::Form { fields, .. } = panel
                        && let Some(field) = fields.iter_mut().find(|f| f.id == field_id)
                    {
                        field.value = Value::String(path.to_owned());
                    }
                    if auto {
                        let panel_id = panel_id.to_owned();
                        if let Some(DynamicPanel::Form { fields, .. }) = self.panels.get(&panel_id)
                        {
                            publish_form_changed(&self.bus, &panel_id, fields);
                        }
                    }
                }
            }
        }
    }

    /// 处理通用字段更新事件（set_value / set_enabled / set_visible）
    fn is_allowed(&self, panel_id: &str, source: &str) -> bool {
        let owner = self.panel_owner(panel_id);
        match owner {
            None => {
                // 面板不存在或为无 owner 的系统面板：
                // - 不存在时允许任何来源的 remove（清理已失效面板不阻塞）
                // - 系统面板禁止插件修改
                if !self.panels.contains_key(panel_id) {
                    return true;
                }
                !source.starts_with("plugin:")
            }
            Some(owner_id) => {
                // 有 owner 的插件面板：只允许同 owner 修改
                let expected = format!("plugin:{owner_id}");
                source == expected
            }
        }
    }

    fn handle_field_update(&mut self, event: Event, apply: impl Fn(&mut DynamicField, Value)) {
        let source = event_source_for_owner(&event).to_owned();
        let Payload::Json(value) = event.payload else {
            return;
        };
        let panel_id = value.get("panel_id").and_then(Value::as_str).unwrap_or("");
        let field_id = value.get("field_id").and_then(Value::as_str).unwrap_or("");
        let new_value = value.get("value").cloned().unwrap_or(Value::Null);

        // owner 校验
        if !self.is_allowed(panel_id, &source) {
            self.bus.publish(Event::system_log(
                LogLevel::Warn,
                "ui.dynamic",
                format!(
                    "set field '{panel_id}.{field_id}' rejected: source '{}' not allowed",
                    source
                ),
            ));
            return;
        }

        if let Some(panel) = self.panels.get_mut(panel_id) {
            if let DynamicPanel::Form { fields, .. } = panel {
                if let Some(field) = fields.iter_mut().find(|f| f.id == field_id) {
                    apply(field, new_value);
                } else {
                    let msg = format!("set field: field '{field_id}' not found in '{panel_id}'");
                    self.last_error = Some(msg.clone());
                    self.bus
                        .publish(Event::system_log(LogLevel::Warn, "ui.dynamic", msg));
                }
            } else {
                let msg = format!("set field: panel '{panel_id}' is not a form");
                self.last_error = Some(msg.clone());
                self.bus
                    .publish(Event::system_log(LogLevel::Warn, "ui.dynamic", msg));
            }
        } else {
            let msg = format!("set field: panel '{panel_id}' not found");
            self.last_error = Some(msg.clone());
            self.bus
                .publish(Event::system_log(LogLevel::Warn, "ui.dynamic", msg));
        }
    }

    fn create_from_event(&mut self, event: Event) -> Result<Option<String>, String> {
        let source = event_source_for_owner(&event).to_owned();
        let Payload::Json(value) = event.payload else {
            return Ok(None);
        };

        let object = value
            .as_object()
            .ok_or_else(|| "ui.panel.create payload must be an object".to_owned())?;

        let id = object
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "ui.panel.create requires id".to_owned())?
            .to_owned();

        let title = object
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_owned();

        let kind = object
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("chart");

        // owner 从 event.source 推导，不信任 payload 中的 plugin_id
        let owner_plugin_id: Option<String> = source.strip_prefix("plugin:").map(|s| s.to_owned());
        let owner_for_check = owner_plugin_id.clone();

        let panel = match kind {
            "chart" => {
                let topic_prefix = object
                    .get("topic_prefix")
                    .or_else(|| object.get("topic"))
                    .and_then(Value::as_str)
                    .unwrap_or("protocol.");

                DynamicPanel::Chart {
                    title,
                    chart: ChartPanel::new_for_topic_prefix(&self.bus, topic_prefix),
                    owner_plugin_id,
                }
            }
            "form" => {
                let auto_apply = object
                    .get("auto_apply")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);

                DynamicPanel::Form {
                    title,
                    fields: parse_fields(object.get("fields"))?,
                    auto_apply,
                    owner_plugin_id,
                }
            }
            "attitude" | "attitude3d" => {
                let topic = object
                    .get("topic")
                    .and_then(Value::as_str)
                    .unwrap_or(topics::PROTOCOL_IMU_ATTITUDE);

                DynamicPanel::Attitude {
                    title,
                    attitude: AttitudePanel::new_for_topic(&self.bus, topic),
                    owner_plugin_id,
                }
            }
            "log" => {
                let max_entries = object
                    .get("max_entries")
                    .and_then(Value::as_u64)
                    .unwrap_or(5000) as usize;
                // 保证最小值为 10，防止 max_entries=0 导致日志面板无用
                let max_entries = max_entries.max(10);
                DynamicPanel::Log {
                    title,
                    entries: VecDeque::new(),
                    max_entries,
                    owner_plugin_id,
                }
            }
            other => return Err(format!("不支持的动态面板类型 '{other}'")),
        };

        // 冲突检查：已有面板不能被不同 owner 覆盖，无 owner 面板不能被插件覆盖
        if self.panels.contains_key(&id) {
            let existing_owner = self.panel_owner(&id);
            let new_owner = owner_for_check.as_deref();
            match existing_owner {
                Some(existing) if existing != new_owner.unwrap_or("") => {
                    return Err(format!(
                        "panel id '{id}' already owned by '{existing}', cannot be overwritten"
                    ));
                }
                None if new_owner.is_some() => {
                    return Err(format!(
                        "panel id '{id}' is a system panel, cannot be overwritten by plugin"
                    ));
                }
                _ => {}
            }
        }

        self.panels.insert(id.clone(), panel);
        self.last_error = None;

        Ok(Some(id))
    }

    fn remove_from_event(&mut self, event: Event) -> Result<Option<String>, String> {
        let source = event_source_for_owner(&event).to_owned();
        let id = match event.payload {
            Payload::Json(value) => value
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| "ui.panel.remove requires id".to_owned())?
                .to_owned(),
            Payload::Text(text) => text.trim().to_owned(),
            Payload::Bytes(_) | Payload::Empty => return Ok(None),
        };

        if id.is_empty() {
            return Err("ui.panel.remove requires id".to_owned());
        }

        // owner 校验：不允许跨插件删除面板
        if !self.is_allowed(&id, &source) {
            self.bus.publish(Event::system_log(
                LogLevel::Warn,
                "ui.dynamic",
                format!(
                    "remove panel '{id}' rejected: source '{}' not allowed",
                    source
                ),
            ));
            return Ok(None);
        }

        self.panels.remove(&id);
        self.last_error = None;

        Ok(Some(id))
    }
}
