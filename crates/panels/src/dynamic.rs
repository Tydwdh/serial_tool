use crate::{AttitudePanel, ChartPanel, PanelKind, PanelManager, theme};
use egui::{Color32, ComboBox, DragValue, ProgressBar, RichText, Slider, TextEdit};
use serde_json::Value;
use std::collections::BTreeMap;
use tool_core::{Direction, Event, LogLevel, Payload, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};

pub struct DynamicPanels {
    bus: DataBus,
    subscription: Subscription,
    remove_subscription: Subscription,
    // UI 状态更新订阅
    set_value_subscription: Subscription,
    set_enabled_subscription: Subscription,
    set_visible_subscription: Subscription,
    file_browse_subscription: Subscription,
    file_selected_subscription: Subscription,
    panels: BTreeMap<String, DynamicPanel>,
    last_error: Option<String>,
}

enum DynamicPanel {
    Chart {
        title: String,
        chart: ChartPanel,
        owner_plugin_id: Option<String>,
    },
    Form {
        title: String,
        fields: Vec<DynamicField>,
        auto_apply: bool,
        owner_plugin_id: Option<String>,
    },
    Attitude {
        title: String,
        attitude: AttitudePanel,
        owner_plugin_id: Option<String>,
    },
}

struct DynamicField {
    id: String,
    label: String,
    kind: DynamicFieldKind,
    value: Value,
    options: Vec<FieldOption>,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
    // ── v0.2 新增 ──
    rows: Option<usize>,
    variant: Option<String>,
    level: Option<String>,
    text: Option<String>,
    filters: Vec<FieldFilter>,
    enabled: bool,
    visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DynamicFieldKind {
    Text,
    Number,
    Boolean,
    Select,
    Slider,
    // ── v0.2 新增 ──
    Button,
    TextArea,
    File,
    Progress,
    Status,
    Separator,
    Label,
}

#[derive(Debug, Clone)]
struct FieldOption {
    label: String,
    value: String,
}

#[derive(Debug, Clone)]
pub struct FieldFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

impl DynamicPanels {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            bus: bus.clone(),
            subscription: bus.subscribe(TopicFilter::exact(topics::UI_PANEL_CREATE)),
            remove_subscription: bus.subscribe(TopicFilter::exact(topics::UI_PANEL_REMOVE)),
            set_value_subscription: bus.subscribe(TopicFilter::exact(topics::UI_FORM_SET_VALUE)),
            set_enabled_subscription: bus
                .subscribe(TopicFilter::exact(topics::UI_FORM_SET_ENABLED)),
            set_visible_subscription: bus
                .subscribe(TopicFilter::exact(topics::UI_FORM_SET_VISIBLE)),
            file_browse_subscription: bus
                .subscribe(TopicFilter::exact(topics::UI_FORM_FILE_BROWSE)),
            file_selected_subscription: bus
                .subscribe(TopicFilter::exact(topics::UI_FORM_FILE_SELECTED)),
            panels: BTreeMap::new(),
            last_error: None,
        }
    }

    pub fn ingest(&mut self, panel_manager: &mut PanelManager) {
        for event in self.subscription.drain() {
            match self.create_from_event(event) {
                Ok(Some(id)) => panel_manager.add_tab(PanelKind::Dynamic(id)),
                Ok(None) => {}
                Err(error) => self.last_error = Some(error),
            }
        }

        for event in self.remove_subscription.drain() {
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
        for event in self.set_value_subscription.drain() {
            self.handle_field_update(event, |field, value| {
                field.value = value;
            });
        }
        for event in self.set_enabled_subscription.drain() {
            self.handle_field_update(event, |field, val| {
                if let Some(enabled) = val.as_bool() {
                    field.enabled = enabled;
                }
            });
        }
        for event in self.set_visible_subscription.drain() {
            self.handle_field_update(event, |field, val| {
                if let Some(visible) = val.as_bool() {
                    field.visible = visible;
                }
            });
        }
        // file browse 事件由 main.rs 处理
        let _ = self.file_browse_subscription.drain();

        // file selected 事件：更新字段值，并触发 form.changed（视为用户输入）
        for event in self.file_selected_subscription.drain() {
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
                    if let DynamicPanel::Form { fields, .. } = panel {
                        if let Some(field) = fields.iter_mut().find(|f| f.id == field_id) {
                            field.value = Value::String(path.to_owned());
                        }
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
    fn handle_field_update(&mut self, event: Event, apply: impl Fn(&mut DynamicField, Value)) {
        let Payload::Json(value) = event.payload else {
            return;
        };
        let panel_id = value.get("panel_id").and_then(Value::as_str).unwrap_or("");
        let field_id = value.get("field_id").and_then(Value::as_str).unwrap_or("");
        let new_value = value.get("value").cloned().unwrap_or(Value::Null);

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

    pub fn ui_body(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(panel) = self.panels.get_mut(id) else {
            ui.colored_label(theme::RED, "面板未找到");
            return;
        };

        match panel {
            DynamicPanel::Chart { chart, .. } => {
                chart.ui(ui);
            }
            DynamicPanel::Form {
                fields, auto_apply, ..
            } => {
                dynamic_form_ui(ui, &self.bus, id, fields, *auto_apply);
            }
            DynamicPanel::Attitude { attitude, .. } => {
                attitude.ui(ui);
            }
        }
    }

    pub fn title(&self, id: &str) -> Option<&str> {
        self.panels.get(id).map(|panel| match panel {
            DynamicPanel::Chart { title, .. } => title.as_str(),
            DynamicPanel::Form { title, .. } => title.as_str(),
            DynamicPanel::Attitude { title, .. } => title.as_str(),
        })
    }

    pub fn count(&self) -> usize {
        self.panels.len()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.panels.contains_key(id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.panels.remove(id).is_some()
    }

    pub fn remove_by_plugin(&mut self, plugin_id: &str) -> Vec<String> {
        let ids: Vec<String> = self
            .panels
            .iter()
            .filter(|(_, panel)| match panel {
                DynamicPanel::Chart {
                    owner_plugin_id, ..
                }
                | DynamicPanel::Form {
                    owner_plugin_id, ..
                }
                | DynamicPanel::Attitude {
                    owner_plugin_id, ..
                } => owner_plugin_id.as_deref() == Some(plugin_id),
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.panels.remove(id);
        }
        ids
    }

    pub fn panel_owner(&self, panel_id: &str) -> Option<&str> {
        self.panels.get(panel_id).and_then(|panel| match panel {
            DynamicPanel::Chart {
                owner_plugin_id, ..
            }
            | DynamicPanel::Form {
                owner_plugin_id, ..
            }
            | DynamicPanel::Attitude {
                owner_plugin_id, ..
            } => owner_plugin_id.as_deref(),
        })
    }

    pub fn clear_charts(&mut self) {
        for panel in self.panels.values_mut() {
            if let DynamicPanel::Chart { chart, .. } = panel {
                chart.clear();
            }
        }
    }

    pub fn ingest_all_pending(&mut self) -> usize {
        let mut count = 0;

        for panel in self.panels.values_mut() {
            if let DynamicPanel::Chart { chart, .. } = panel {
                count += chart.ingest_all_pending();
            }
        }

        count
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    fn create_from_event(&mut self, event: Event) -> Result<Option<String>, String> {
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

        let owner_plugin_id = object
            .get("plugin_id")
            .and_then(Value::as_str)
            .map(|s| s.to_owned());

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
            other => return Err(format!("不支持的动态面板类型 '{other}'")),
        };

        self.panels.insert(id.clone(), panel);
        self.last_error = None;

        Ok(Some(id))
    }

    fn remove_from_event(&mut self, event: Event) -> Result<Option<String>, String> {
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

        self.panels.remove(&id);
        self.last_error = None;

        Ok(Some(id))
    }
}

fn dynamic_form_ui(
    ui: &mut egui::Ui,
    bus: &DataBus,
    panel_id: &str,
    fields: &mut [DynamicField],
    auto_apply: bool,
) {
    let mut changed = false;

    // 预收集所有字段值（供 button action 使用，避免 borrow 冲突）
    let field_values: Vec<(String, Value)> = fields
        .iter()
        .map(|f| (f.id.clone(), f.value.clone()))
        .collect();

    for field in fields.iter_mut() {
        if !field.visible {
            continue;
        }

        let enabled = field.enabled;

        match field.kind {
            // ── 分隔符 ──
            DynamicFieldKind::Separator => {
                ui.separator();
            }
            // ── 标签 ──
            DynamicFieldKind::Label => {
                let text = field.text.as_deref().unwrap_or(&field.label);
                ui.label(RichText::new(text).color(theme::TEXT_SECONDARY));
            }
            // ── 按钮 ──
            DynamicFieldKind::Button => {
                let text = field.text.as_deref().unwrap_or(&field.label);
                let fill = match field.variant.as_deref() {
                    Some("primary") => theme::BLUE,
                    Some("danger") => theme::RED,
                    _ => theme::BG_TERTIARY,
                };
                let btn = egui::Button::new(RichText::new(text).color(theme::TEXT_WHITE))
                    .fill(fill)
                    .min_size(egui::vec2(80.0, 28.0));
                if ui.add_enabled(enabled, btn).clicked() {
                    let mut values = serde_json::Map::new();
                    for (id, val) in &field_values {
                        values.insert(id.clone(), val.clone());
                    }
                    bus.publish(Event::new(
                        topics::UI_FORM_ACTION,
                        format!("ui.panel:{panel_id}"),
                        Direction::Internal,
                        Payload::Json(serde_json::json!({
                            "panel_id": panel_id,
                            "field_id": field.id,
                            "kind": "button_clicked",
                            "values": values
                        })),
                    ));
                }
            }
            // ── 进度条 ──
            DynamicFieldKind::Progress => {
                if let Some(label) = field.text.as_deref() {
                    ui.label(label);
                }
                let v = field.value.as_f64().unwrap_or(0.0).clamp(0.0, 100.0);
                ui.add(ProgressBar::new((v / 100.0) as f32).text(format!("{v:.0}%")));
            }
            // ── 状态 ──
            DynamicFieldKind::Status => {
                let text = field
                    .value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let level = field
                    .value
                    .get("level")
                    .and_then(Value::as_str)
                    .unwrap_or("idle");
                let color = status_color(level);
                ui.label(RichText::new(text).color(color));
            }
            // ── TextArea ──
            DynamicFieldKind::TextArea => {
                let rows = field.rows.unwrap_or(6);
                let mut text = field.value.as_str().unwrap_or("").to_owned();
                ui.vertical(|ui| {
                    ui.label(&field.label);
                    let resp = ui.add_enabled(
                        enabled,
                        TextEdit::multiline(&mut text)
                            .desired_rows(rows)
                            .desired_width(f32::INFINITY),
                    );
                    if resp.changed() {
                        field.value = Value::String(text);
                        changed = true;
                    }
                });
                continue; // 跳过下面 horizontal 逻辑
            }
            // ── File ──
            DynamicFieldKind::File => {
                ui.horizontal(|ui| {
                    ui.label(&field.label);
                    let path = field.value.as_str().unwrap_or("").to_owned();
                    let mut display_path = path.clone();
                    let resp = ui.add_enabled(
                        enabled,
                        TextEdit::singleline(&mut display_path).desired_width(200.0),
                    );
                    if resp.changed() {
                        field.value = Value::String(display_path);
                        changed = true;
                    }
                    if ui.add_enabled(enabled, egui::Button::new("浏览")).clicked() {
                        bus.publish(Event::new(
                            topics::UI_FORM_FILE_BROWSE,
                            format!("ui.panel:{panel_id}"),
                            Direction::Internal,
                            Payload::Json(serde_json::json!({
                                "panel_id": panel_id,
                                "field_id": field.id,
                                "filters": field.filters.iter().map(|f| serde_json::json!({
                                    "name": f.name,
                                    "extensions": f.extensions,
                                })).collect::<Vec<_>>(),
                            })),
                        ));
                    }
                });
            }
            // ── 原有类型 ──
            _ => {
                ui.horizontal(|ui| {
                    ui.label(&field.label);

                    let field_changed = match field.kind {
                        DynamicFieldKind::Text => {
                            let mut text = field.value.as_str().unwrap_or("").to_owned();
                            let resp = ui.add_enabled(
                                enabled,
                                TextEdit::singleline(&mut text).desired_width(220.0),
                            );
                            if resp.changed() {
                                field.value = Value::String(text);
                                true
                            } else {
                                false
                            }
                        }

                        DynamicFieldKind::Number => {
                            let mut value = field.value.as_f64().unwrap_or_default();
                            let resp = ui.add_enabled(
                                enabled,
                                DragValue::new(&mut value)
                                    .speed(field.step.unwrap_or(1.0))
                                    .range(
                                        field.min.unwrap_or(f64::NEG_INFINITY)
                                            ..=field.max.unwrap_or(f64::INFINITY),
                                    ),
                            );
                            if resp.changed() {
                                field.value = serde_json::Number::from_f64(value)
                                    .map(Value::Number)
                                    .unwrap_or(Value::String(compact_number(value)));
                                true
                            } else {
                                false
                            }
                        }

                        DynamicFieldKind::Boolean => {
                            let mut value = field.value.as_bool().unwrap_or(false);
                            let resp = ui.add_enabled(enabled, egui::Checkbox::new(&mut value, ""));
                            if resp.changed() {
                                field.value = Value::Bool(value);
                                true
                            } else {
                                false
                            }
                        }

                        DynamicFieldKind::Select => {
                            let current = field.value.as_str().unwrap_or("").to_owned();
                            let selected_text = field
                                .options
                                .iter()
                                .find(|o| o.value == current)
                                .map(|o| o.label.clone())
                                .unwrap_or_else(|| current.clone());

                            let options = field.options.clone();
                            let mut new_value = current;
                            let mut field_changed = false;

                            ComboBox::from_id_salt((panel_id, &field.id))
                                .width(180.0)
                                .selected_text(selected_text)
                                .show_ui(ui, |ui| {
                                    for option in options {
                                        if ui
                                            .selectable_value(
                                                &mut new_value,
                                                option.value.clone(),
                                                &option.label,
                                            )
                                            .changed()
                                        {
                                            field_changed = true;
                                        }
                                    }
                                });

                            if field_changed {
                                field.value = Value::String(new_value);
                                true
                            } else {
                                false
                            }
                        }

                        DynamicFieldKind::Slider => {
                            let min = field.min.unwrap_or(0.0);
                            let max = field.max.unwrap_or(100.0);
                            let mut value = field.value.as_f64().unwrap_or(min).clamp(min, max);

                            let resp = ui.add_enabled(
                                enabled,
                                Slider::new(&mut value, min..=max)
                                    .step_by(field.step.unwrap_or(1.0))
                                    .show_value(true),
                            );

                            if resp.changed() {
                                field.value = serde_json::Number::from_f64(value)
                                    .map(Value::Number)
                                    .unwrap_or(Value::String(compact_number(value)));
                                true
                            } else {
                                false
                            }
                        }
                        _ => false, // Button, TextArea 等已在上面处理
                    };

                    changed |= field_changed;
                });
            }
        }
    }

    if auto_apply {
        ui.horizontal(|ui| {
            ui.label(RichText::new("变更会立即应用").color(theme::TEXT_SECONDARY));
        });

        if changed {
            publish_form_changed(bus, panel_id, fields);
        }
    } else if ui.button("应用").clicked() {
        publish_form_changed(bus, panel_id, fields);
    }
}

fn publish_form_changed(bus: &DataBus, panel_id: &str, fields: &[DynamicField]) {
    let mut values = serde_json::Map::new();

    for field in fields {
        values.insert(field.id.clone(), field.value.clone());
    }

    bus.publish(Event::new(
        topics::UI_FORM_CHANGED,
        format!("ui.panel:{panel_id}"),
        Direction::Internal,
        Payload::Json(serde_json::json!({
            "panel_id": panel_id,
            "values": values
        })),
    ));
}

fn status_color(level: &str) -> Color32 {
    match level {
        "running" => theme::BLUE,
        "success" => theme::GREEN,
        "warn" => theme::YELLOW,
        "error" => theme::RED,
        _ => theme::TEXT_SECONDARY, // idle
    }
}

fn parse_fields(value: Option<&Value>) -> Result<Vec<DynamicField>, String> {
    let Some(Value::Array(fields)) = value else {
        return Ok(Vec::new());
    };

    fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let object = field
                .as_object()
                .ok_or_else(|| "form field must be an object".to_owned())?;

            // 先解析 kind，display-only 类型可以不提供 id
            let kind = match object.get("kind").and_then(Value::as_str).unwrap_or("text") {
                "number" => DynamicFieldKind::Number,
                "boolean" | "bool" | "checkbox" => DynamicFieldKind::Boolean,
                "select" | "choice" | "enum" | "dropdown" => DynamicFieldKind::Select,
                "slider" | "range" => DynamicFieldKind::Slider,
                // ── v0.2 新增 ──
                "button" => DynamicFieldKind::Button,
                "textarea" => DynamicFieldKind::TextArea,
                "file" => DynamicFieldKind::File,
                "progress" => DynamicFieldKind::Progress,
                "status" => DynamicFieldKind::Status,
                "separator" => DynamicFieldKind::Separator,
                "label" => DynamicFieldKind::Label,
                _ => DynamicFieldKind::Text,
            };

            // separator 和 label 不强制要求 id，自动生成 fallback
            let id = object
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    if matches!(kind, DynamicFieldKind::Separator | DynamicFieldKind::Label) {
                        Some(format!("__field_{index}"))
                    } else {
                        None
                    }
                })
                .ok_or_else(|| "form field requires id".to_owned())?;

            let label = object
                .get("label")
                .or_else(|| object.get("title"))
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_owned();

            let options = parse_options(object.get("options"))?;
            let filters = parse_filters(object.get("filters"))?;

            let default_value = object
                .get("default")
                .cloned()
                .or_else(|| {
                    if matches!(kind, DynamicFieldKind::Progress) {
                        Some(Value::Number(0.into()))
                    } else if matches!(kind, DynamicFieldKind::Status) {
                        Some(serde_json::json!({"text": "空闲", "level": "idle"}))
                    } else if matches!(
                        kind,
                        DynamicFieldKind::Boolean
                            | DynamicFieldKind::Button
                            | DynamicFieldKind::Separator
                            | DynamicFieldKind::Label
                    ) {
                        None
                    } else {
                        options.first().map(|o| Value::String(o.value.clone()))
                    }
                })
                .unwrap_or(Value::String(String::new()));

            Ok(DynamicField {
                id,
                label,
                kind,
                value: default_value,
                options,
                min: object.get("min").and_then(Value::as_f64),
                max: object.get("max").and_then(Value::as_f64),
                step: object.get("step").and_then(Value::as_f64),
                rows: object
                    .get("rows")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize),
                variant: object
                    .get("variant")
                    .and_then(Value::as_str)
                    .map(String::from),
                level: object
                    .get("level")
                    .and_then(Value::as_str)
                    .map(String::from),
                text: object.get("text").and_then(Value::as_str).map(String::from),
                filters,
                enabled: object
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                visible: object
                    .get("visible")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            })
        })
        .collect()
}

fn parse_filters(value: Option<&Value>) -> Result<Vec<FieldFilter>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Value::Array(filters) = value else {
        return Err("filters must be an array".to_owned());
    };
    let mut result = Vec::new();
    for filter in filters {
        let obj = filter
            .as_object()
            .ok_or_else(|| "filter must be an object".to_owned())?;
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let extensions = obj
            .get("extensions")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        result.push(FieldFilter { name, extensions });
    }
    Ok(result)
}

fn parse_options(value: Option<&Value>) -> Result<Vec<FieldOption>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    let Value::Array(options) = value else {
        return Err("form field options must be an array".to_owned());
    };

    let mut result = Vec::new();

    for option in options {
        match option {
            Value::String(value) => {
                result.push(FieldOption {
                    label: value.clone(),
                    value: value.clone(),
                });
            }
            Value::Number(value) => {
                let value = value.to_string();
                result.push(FieldOption {
                    label: value.clone(),
                    value,
                });
            }
            Value::Bool(value) => {
                let value = value.to_string();
                result.push(FieldOption {
                    label: value.clone(),
                    value,
                });
            }
            Value::Object(object) => {
                let value = object
                    .get("value")
                    .map(value_to_string)
                    .ok_or_else(|| "select option requires value".to_owned())?;

                let label = object
                    .get("label")
                    .or_else(|| object.get("title"))
                    .map(value_to_string)
                    .unwrap_or_else(|| value.clone());

                result.push(FieldOption { label, value });
            }
            _ => return Err("unsupported select option".to_owned()),
        }
    }

    Ok(result)
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => String::new(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

fn compact_number(value: f64) -> String {
    if value.fract().abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.4}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_dynamic_chart_from_bus_event() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "test",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "pid-chart",
                "title": "PID Chart",
                "kind": "chart",
                "topic_prefix": "protocol.pid."
            })),
        ));

        panels.ingest(&mut manager);

        assert_eq!(panels.count(), 1);
        assert_eq!(panels.title("pid-chart"), Some("PID Chart"));
        assert!(
            manager
                .tabs
                .contains(&PanelKind::Dynamic("pid-chart".to_owned()))
        );
    }

    #[test]
    fn creates_dynamic_form_from_bus_event() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "test",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "pid-form",
                "title": "PID Form",
                "kind": "form",
                "fields": [
                    { "id": "kp", "label": "Kp", "kind": "number", "default": 1.0 },
                    {
                        "id": "mode",
                        "label": "模式",
                        "kind": "select",
                        "default": "auto",
                        "options": [
                            { "label": "自动", "value": "auto" },
                            { "label": "手动", "value": "manual" }
                        ]
                    }
                ]
            })),
        ));

        panels.ingest(&mut manager);

        assert_eq!(panels.count(), 1);
        assert_eq!(panels.title("pid-form"), Some("PID Form"));
        assert!(
            manager
                .tabs
                .contains(&PanelKind::Dynamic("pid-form".to_owned()))
        );
    }

    #[test]
    fn creates_dynamic_attitude_from_bus_event() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "test",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "imu-attitude",
                "title": "IMU Attitude",
                "kind": "attitude",
                "topic": "protocol.imu.attitude"
            })),
        ));

        panels.ingest(&mut manager);

        assert_eq!(panels.count(), 1);
        assert_eq!(panels.title("imu-attitude"), Some("IMU Attitude"));
        assert!(
            manager
                .tabs
                .contains(&PanelKind::Dynamic("imu-attitude".to_owned()))
        );
    }

    #[test]
    fn removes_dynamic_panel_from_bus_event() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "test",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "pid-chart",
                "title": "PID Chart",
                "kind": "chart"
            })),
        ));

        panels.ingest(&mut manager);

        bus.publish(Event::new(
            topics::UI_PANEL_REMOVE,
            "test",
            Direction::Internal,
            Payload::Json(serde_json::json!({ "id": "pid-chart" })),
        ));

        panels.ingest(&mut manager);

        assert_eq!(panels.count(), 0);
        assert!(
            !manager
                .tabs
                .contains(&PanelKind::Dynamic("pid-chart".to_owned()))
        );
    }

    #[test]
    fn creates_form_with_label_and_separator_without_id() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "test",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "file-tool-panel",
                "title": "文件工具",
                "kind": "form",
                "fields": [
                    { "kind": "label", "text": "请选择文件" },
                    { "kind": "separator" },
                    { "id": "file_path", "label": "文件", "kind": "file" },
                    { "id": "load", "label": "加载", "kind": "button" }
                ]
            })),
        ));

        panels.ingest(&mut manager);

        assert_eq!(panels.count(), 1);
        assert_eq!(panels.title("file-tool-panel"), Some("文件工具"));
    }
}
