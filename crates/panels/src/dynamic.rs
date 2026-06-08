use crate::{AttitudePanel, ChartPanel, PanelKind, PanelManager, theme};
use egui::{ComboBox, DragValue, Slider, TextEdit};
use serde_json::Value;
use std::collections::BTreeMap;
use tool_core::{Direction, Event, Payload, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};

pub struct DynamicPanels {
    bus: DataBus,
    subscription: Subscription,
    remove_subscription: Subscription,
    panels: BTreeMap<String, DynamicPanel>,
    last_error: Option<String>,
}

enum DynamicPanel {
    Chart {
        title: String,
        chart: ChartPanel,
    },
    Form {
        title: String,
        fields: Vec<DynamicField>,
        auto_apply: bool,
    },
    Attitude {
        title: String,
        attitude: AttitudePanel,
    },
}

struct DynamicField {
    id: String,
    label: String,
    kind: DynamicFieldKind,
    value: String,
    options: Vec<FieldOption>,
    min: Option<f64>,
    max: Option<f64>,
    step: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DynamicFieldKind {
    Text,
    Number,
    Boolean,
    Select,
    Slider,
}

#[derive(Debug, Clone)]
struct FieldOption {
    label: String,
    value: String,
}

impl DynamicPanels {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            bus: bus.clone(),
            subscription: bus.subscribe(TopicFilter::exact(topics::UI_PANEL_CREATE)),
            remove_subscription: bus.subscribe(TopicFilter::exact(topics::UI_PANEL_REMOVE)),
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

    for field in fields.iter_mut() {
        ui.horizontal(|ui| {
            ui.label(&field.label);

            let field_changed = match field.kind {
                DynamicFieldKind::Text => ui
                    .add(TextEdit::singleline(&mut field.value).desired_width(220.0))
                    .changed(),

                DynamicFieldKind::Number => {
                    let mut value = field.value.parse::<f64>().unwrap_or_default();
                    let response = ui.add(
                        DragValue::new(&mut value)
                            .speed(field.step.unwrap_or(1.0))
                            .range(
                                field.min.unwrap_or(f64::NEG_INFINITY)
                                    ..=field.max.unwrap_or(f64::INFINITY),
                            ),
                    );

                    if response.changed() {
                        field.value = compact_number(value);
                        true
                    } else {
                        false
                    }
                }

                DynamicFieldKind::Boolean => {
                    let mut value = parse_bool(&field.value);
                    let response = ui.checkbox(&mut value, "");

                    if response.changed() {
                        field.value = value.to_string();
                        true
                    } else {
                        false
                    }
                }

                DynamicFieldKind::Select => {
                    let selected_text = field
                        .options
                        .iter()
                        .find(|option| option.value == field.value)
                        .map(|option| option.label.clone())
                        .unwrap_or_else(|| field.value.clone());

                    let options = field.options.clone();
                    let mut field_changed = false;

                    ComboBox::from_id_salt((panel_id, field.id.as_str()))
                        .width(180.0)
                        .selected_text(selected_text)
                        .show_ui(ui, |ui| {
                            for option in options {
                                if ui
                                    .selectable_value(&mut field.value, option.value, option.label)
                                    .changed()
                                {
                                    field_changed = true;
                                }
                            }
                        });

                    field_changed
                }

                DynamicFieldKind::Slider => {
                    let min = field.min.unwrap_or(0.0);
                    let max = field.max.unwrap_or(100.0);
                    let mut value = field.value.parse::<f64>().unwrap_or(min).clamp(min, max);

                    let response = ui.add(
                        Slider::new(&mut value, min..=max)
                            .step_by(field.step.unwrap_or(1.0))
                            .show_value(true),
                    );

                    if response.changed() {
                        field.value = compact_number(value);
                        true
                    } else {
                        false
                    }
                }
            };

            changed |= field_changed;
        });
    }

    if auto_apply {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("变更会立即应用").color(theme::TEXT_SECONDARY));
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
        values.insert(field.id.clone(), field_value_to_json(field));
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

fn field_value_to_json(field: &DynamicField) -> Value {
    match field.kind {
        DynamicFieldKind::Number | DynamicFieldKind::Slider => field
            .value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(field.value.clone())),

        DynamicFieldKind::Boolean => Value::Bool(parse_bool(&field.value)),

        DynamicFieldKind::Text | DynamicFieldKind::Select => Value::String(field.value.clone()),
    }
}

fn parse_fields(value: Option<&Value>) -> Result<Vec<DynamicField>, String> {
    let Some(Value::Array(fields)) = value else {
        return Ok(Vec::new());
    };

    fields
        .iter()
        .map(|field| {
            let object = field
                .as_object()
                .ok_or_else(|| "form field must be an object".to_owned())?;

            let id = object
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| "form field requires id".to_owned())?
                .to_owned();

            let label = object
                .get("label")
                .or_else(|| object.get("title"))
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_owned();

            let kind = match object.get("kind").and_then(Value::as_str).unwrap_or("text") {
                "number" => DynamicFieldKind::Number,
                "boolean" | "bool" | "checkbox" => DynamicFieldKind::Boolean,
                "select" | "choice" | "enum" | "dropdown" => DynamicFieldKind::Select,
                "slider" | "range" => DynamicFieldKind::Slider,
                _ => DynamicFieldKind::Text,
            };

            let options = parse_options(object.get("options"))?;

            let default_value = object
                .get("default")
                .map(value_to_string)
                .or_else(|| options.first().map(|option| option.value.clone()))
                .unwrap_or_default();

            Ok(DynamicField {
                id,
                label,
                kind,
                value: default_value,
                options,
                min: object.get("min").and_then(Value::as_f64),
                max: object.get("max").and_then(Value::as_f64),
                step: object.get("step").and_then(Value::as_f64),
            })
        })
        .collect()
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
}
