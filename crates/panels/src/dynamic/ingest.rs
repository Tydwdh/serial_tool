//! 动态面板事件摄入：`ingest` + 事件解析 + owner 鉴权。
//!
//! 从 `mod.rs` 抽出的事件处理逻辑。`ingest` 调度各类 UI 事件，
//! `create_from_event`/`remove_from_event` 解析创建/移除指令，
//! `is_allowed` 做跨插件 owner 校验，`handle_field_update` 处理字段更新。

use super::DynamicPanel;
use super::form_render::publish_form_changed;
use super::schema::{DynamicField, parse_fields};
use crate::{AttitudePanel, ChartPanel, GaugePanel, PanelKind, PanelManager};
use serde_json::Value;
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
            match panel {
                DynamicPanel::Form { fields, .. } => {
                    if let Some(field) = fields.iter_mut().find(|f| f.id == field_id) {
                        apply(field, new_value);
                    } else {
                        let msg =
                            format!("set field: field '{field_id}' not found in '{panel_id}'");
                        self.last_error = Some(msg.clone());
                        self.bus
                            .publish(Event::system_log(LogLevel::Warn, "ui.dynamic", msg));
                    }
                }
                DynamicPanel::Gauge { gauge, .. } if field_id == "value" => {
                    if let Some(v) = new_value.as_f64() {
                        gauge.set_value(v);
                    }
                }
                DynamicPanel::Gauge { gauge, .. } if field_id == "status" => {
                    if let Some(s) = new_value.as_str() {
                        gauge.set_status(s.to_owned());
                    }
                }
                _ => {
                    let msg = format!("set field: panel '{panel_id}' does not support set_value");
                    self.last_error = Some(msg.clone());
                    self.bus
                        .publish(Event::system_log(LogLevel::Warn, "ui.dynamic", msg));
                }
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

        let card = object.get("card").and_then(Value::as_bool).unwrap_or(false);

        let panel = match kind {
            "chart" => {
                // `topic` 精确订阅单个 topic；`topic_prefix` 订阅前缀下所有 topic。
                // 两者都未提供时回退到默认前缀。
                let chart = if let Some(topic) = object.get("topic").and_then(Value::as_str) {
                    ChartPanel::new_for_topic(&self.bus, topic)
                } else {
                    let topic_prefix = object
                        .get("topic_prefix")
                        .and_then(Value::as_str)
                        .unwrap_or("protocol.");
                    ChartPanel::new_for_topic_prefix(&self.bus, topic_prefix)
                };

                DynamicPanel::Chart {
                    title,
                    chart,
                    owner_plugin_id,
                    card,
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
                    card,
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
                    card,
                }
            }
            "gauge" => {
                let topic = object
                    .get("topic")
                    .and_then(Value::as_str)
                    .unwrap_or("protocol.gauge");

                let min = object.get("min").and_then(Value::as_f64).unwrap_or(0.0);
                let max = object.get("max").and_then(Value::as_f64).unwrap_or(100.0);
                let unit = object
                    .get("unit")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let label = object
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let zones = crate::gauge::parse_zones(object.get("zones"));

                DynamicPanel::Gauge {
                    title,
                    gauge: GaugePanel::from_config(&self.bus, topic, min, max, unit, zones, label),
                    owner_plugin_id,
                    card,
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
