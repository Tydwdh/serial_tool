//! 动态面板事件摄入：`ingest` + 事件解析 + owner 鉴权。
//!
//! 从 `mod.rs` 抽出的事件处理逻辑。`ingest` 调度各类 UI 事件，
//! `create_from_event`/`remove_from_event` 解析创建/移除指令，
//! `is_allowed` 做跨插件 owner 校验，`handle_field_update` 处理字段更新。

use super::DynamicPanel;
use super::form_render::publish_form_changed;
use super::schema::{DynamicField, parse_fields};
use crate::{
    AttitudePanel, ChartPanel, DataTablePanel, GaugePanel, MAX_INGEST_PER_FRAME, PanelId,
    PanelManager,
};
use serde_json::Value;
use tool_core::{Event, LogLevel, Payload, topics};
use tool_databus::{DataBus, Subscription};

/// 取出用于鉴权的事件来源。
///
/// 回放事件本身带的是 `replay:` 前缀来源，真正的发起方记录在 metadata 的
/// `original_source` 里，鉴权必须按原始来源判定。返回 `String` 而不是 `&str`
/// 是因为调用点随后会把 `event.payload` move 走。
fn event_source_for_owner(event: &Event) -> String {
    if event.source.starts_with("replay:") {
        event
            .meta_str("original_source")
            .unwrap_or(&event.source)
            .to_owned()
    } else {
        event.source.clone()
    }
}

/// 字段更新失败的上报：写 `last_error`（面板显示用）并发一条 `ui.dynamic` 系统日志。
///
/// 写成自由函数而非 `&mut self` 方法：调用点仍持有 `self.panels` 的可变借用，
/// 只借用 `last_error` 与 `bus` 两个字段才不冲突。
fn report_field_error(last_error: &mut Option<String>, bus: &DataBus, msg: String) {
    bus.publish(Event::system_log(LogLevel::Warn, "ui.dynamic", msg.clone()));
    *last_error = Some(msg);
}

/// 每个摄入周期从订阅里最多取这么多事件——上限与其余面板一致，
/// 避免一次摄入把整帧 UI 时间吃掉。
fn drain_for_frame(subscription: &Subscription) -> Vec<Event> {
    subscription.drain_limited(MAX_INGEST_PER_FRAME)
}

impl super::DynamicPanels {
    /// 每帧调度一轮动态面板事件：创建/移除、字段状态更新、表格行操作。
    pub fn ingest(&mut self, panel_manager: &mut PanelManager) {
        for event in drain_for_frame(&self.subscription) {
            match self.create_from_event(event) {
                // 插件主动创建面板时立即显示并聚焦；同一插件的多个面板自动分组。
                Ok(Some(id)) => {
                    let owner = self.panel_owner(&id).map(str::to_owned);
                    let pane = PanelId::dynamic(&id);
                    if let Some(owner) = owner {
                        panel_manager.open_plugin_tab(pane, &owner);
                    } else {
                        panel_manager.open_tab(pane);
                    }
                }
                Ok(None) => {}
                Err(error) => self.last_error = Some(error),
            }
        }

        for event in drain_for_frame(&self.remove_subscription) {
            match self.remove_from_event(event) {
                Ok(Some(id)) => {
                    self.panels.remove(&id);
                    panel_manager.close_tab(PanelId::dynamic(&id));
                }
                Ok(None) => {}
                Err(error) => self.last_error = Some(error),
            }
        }

        // UI 状态更新事件
        for event in drain_for_frame(&self.set_value_subscription) {
            self.handle_field_update(event, |field, value| {
                field.value = value;
            });
        }
        for event in drain_for_frame(&self.set_values_subscription) {
            self.handle_values_update(event);
        }
        for event in drain_for_frame(&self.set_enabled_subscription) {
            self.handle_field_update(event, |field, val| {
                if let Some(enabled) = val.as_bool() {
                    field.enabled = enabled;
                }
            });
        }
        for event in drain_for_frame(&self.set_visible_subscription) {
            self.handle_field_update(event, |field, val| {
                if let Some(visible) = val.as_bool() {
                    field.visible = visible;
                }
            });
        }
        // file browse 事件由 composition root 处理；Web 用浏览器文件选择器，
        // Native 使用 Workbench 的桌面文件服务。

        // file selected 事件：更新字段值，并触发 form.changed（视为用户输入）
        // 只接受来自 ui/app 的事件，防止插件伪造文件选择结果
        for event in drain_for_frame(&self.file_selected_subscription) {
            if event.source != "ui" && event.source != "app" {
                continue;
            }
            let Payload::Json(val) = event.payload else {
                continue;
            };
            let panel_id = val.get("panel_id").and_then(Value::as_str).unwrap_or("");
            let field_id = val.get("field_id").and_then(Value::as_str).unwrap_or("");
            let path = val.get("path").and_then(Value::as_str).unwrap_or("");
            if let Some(DynamicPanel::Form {
                fields, auto_apply, ..
            }) = self.panels.get_mut(panel_id)
            {
                if let Some(field) = fields.iter_mut().find(|f| f.id == field_id) {
                    field.value = Value::String(path.to_owned());
                }
                if *auto_apply {
                    publish_form_changed(&self.bus, panel_id, fields.as_slice());
                }
            }
        }

        for event in drain_for_frame(&self.table_set_rows_subscription) {
            self.handle_table_rows(event, TableOperation::Set);
        }
        for event in drain_for_frame(&self.table_append_rows_subscription) {
            self.handle_table_rows(event, TableOperation::Append);
        }
        for event in drain_for_frame(&self.table_remove_rows_subscription) {
            self.handle_table_rows(event, TableOperation::Remove);
        }
        for event in drain_for_frame(&self.table_clear_subscription) {
            self.handle_table_rows(event, TableOperation::Clear);
        }
    }

    /// owner 鉴权：判断 `source` 是否允许修改 `panel_id` 指向的面板。
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
            // 有 owner 的插件面板：只允许同 owner 修改
            Some(owner_id) => source == format!("plugin:{owner_id}"),
        }
    }

    /// 处理通用字段更新事件（set_value / set_enabled / set_visible）
    fn handle_field_update(&mut self, event: Event, apply: impl Fn(&mut DynamicField, Value)) {
        let source = event_source_for_owner(&event);
        let Payload::Json(value) = event.payload else {
            return;
        };
        let panel_id = value.get("panel_id").and_then(Value::as_str).unwrap_or("");
        let field_id = value.get("field_id").and_then(Value::as_str).unwrap_or("");
        let new_value = value.get("value").cloned().unwrap_or(Value::Null);

        if !self.is_allowed(panel_id, &source) {
            self.bus.publish(Event::system_log(
                LogLevel::Warn,
                "ui.dynamic",
                format!(
                    "set field '{panel_id}.{field_id}' rejected: source '{source}' not allowed"
                ),
            ));
            return;
        }

        let Some(panel) = self.panels.get_mut(panel_id) else {
            let msg = format!("set field: panel '{panel_id}' not found");
            report_field_error(&mut self.last_error, &self.bus, msg);
            return;
        };
        match panel {
            DynamicPanel::Form { fields, .. } => {
                if let Some(field) = fields.iter_mut().find(|f| f.id == field_id) {
                    apply(field, new_value);
                } else {
                    let msg = format!("set field: field '{field_id}' not found in '{panel_id}'");
                    report_field_error(&mut self.last_error, &self.bus, msg);
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
                report_field_error(&mut self.last_error, &self.bus, msg);
            }
        }
    }

    fn handle_values_update(&mut self, event: Event) {
        let source = event_source_for_owner(&event);
        let Payload::Json(value) = event.payload else {
            return;
        };
        let panel_id = value.get("panel_id").and_then(Value::as_str).unwrap_or("");
        let Some(values) = value.get("values").and_then(Value::as_object) else {
            return;
        };
        if !self.is_allowed(panel_id, &source) {
            self.bus.publish(Event::system_log(
                LogLevel::Warn,
                "ui.dynamic",
                format!("set values for '{panel_id}' rejected: source '{source}' not allowed"),
            ));
            return;
        }
        if let Some(panel) = self.panels.get_mut(panel_id) {
            match panel {
                DynamicPanel::Form { fields, .. } => {
                    for field in fields {
                        if let Some(value) = values.get(&field.id) {
                            field.value = value.clone();
                        }
                    }
                }
                DynamicPanel::Gauge { gauge, .. } => {
                    if let Some(value) = values.get("value").and_then(Value::as_f64) {
                        gauge.set_value(value);
                    }
                    if let Some(status) = values.get("status").and_then(Value::as_str) {
                        gauge.set_status(status.to_owned());
                    }
                }
                DynamicPanel::Table { table, .. } => {
                    if let Some(rows) = values.get("rows") {
                        table.set_rows(rows.clone());
                    }
                }
                _ => {}
            }
        }
    }

    fn handle_table_rows(&mut self, event: Event, operation: TableOperation) {
        let source = event_source_for_owner(&event);
        let Payload::Json(value) = event.payload else {
            return;
        };
        let panel_id = value.get("panel_id").and_then(Value::as_str).unwrap_or("");
        if !self.is_allowed(panel_id, &source) {
            return;
        }
        let Some(DynamicPanel::Table { table, .. }) = self.panels.get_mut(panel_id) else {
            return;
        };
        let rows = value.get("rows").cloned().unwrap_or(Value::Array(vec![]));
        match operation {
            TableOperation::Set => table.set_rows(rows),
            TableOperation::Append => table.append_rows(rows),
            TableOperation::Remove => table.remove_rows(rows),
            TableOperation::Clear => table.clear(),
        }
    }

    fn create_from_event(&mut self, event: Event) -> Result<Option<String>, String> {
        let source = event_source_for_owner(&event);
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

        // owner 从 event.source 推导，不信任 payload 中的 plugin_id。
        // 冲突检查发生在面板构建之后，届时 String 版的 owner 已经被 move 进面板，
        // 所以这里另外留一份指向 `source` 的借用供它比较。
        let new_owner = source.strip_prefix("plugin:");
        let owner_plugin_id: Option<String> = new_owner.map(str::to_owned);

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
            "table" => DynamicPanel::Table {
                title,
                table: DataTablePanel::from_config(object)?,
                owner_plugin_id,
                card,
            },
            other => return Err(format!("不支持的动态面板类型 '{other}'")),
        };

        // 冲突检查：已有面板不能被不同 owner 覆盖，无 owner 面板不能被插件覆盖
        if self.panels.contains_key(&id) {
            let existing_owner = self.panel_owner(&id);
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
        let source = event_source_for_owner(&event);
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
                format!("remove panel '{id}' rejected: source '{source}' not allowed"),
            ));
            return Ok(None);
        }

        self.panels.remove(&id);
        self.last_error = None;

        Ok(Some(id))
    }
}

#[derive(Clone, Copy)]
enum TableOperation {
    Set,
    Append,
    Remove,
    Clear,
}
