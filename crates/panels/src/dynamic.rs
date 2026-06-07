use crate::{AttitudePanel, ChartPanel, PanelKind, PanelManager, theme};
use egui::TextEdit;
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
}

enum DynamicFieldKind {
    Text,
    Number,
    Boolean,
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
            DynamicPanel::Form { fields, .. } => {
                dynamic_form_ui(ui, &self.bus, id, fields);
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
            "form" => DynamicPanel::Form {
                title,
                fields: parse_fields(object.get("fields"))?,
            },
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

fn dynamic_form_ui(ui: &mut egui::Ui, bus: &DataBus, panel_id: &str, fields: &mut [DynamicField]) {
    for field in fields.iter_mut() {
        ui.horizontal(|ui| {
            ui.label(&field.label);
            match field.kind {
                DynamicFieldKind::Text => {
                    ui.add(TextEdit::singleline(&mut field.value).desired_width(180.0));
                }
                DynamicFieldKind::Number => {
                    ui.add(TextEdit::singleline(&mut field.value).desired_width(96.0));
                }
                DynamicFieldKind::Boolean => {
                    let mut value = matches!(field.value.as_str(), "true" | "1" | "yes");
                    if ui.checkbox(&mut value, "").changed() {
                        field.value = value.to_string();
                    }
                }
            }
        });
    }

    if ui.button("应用").clicked() {
        let mut values = serde_json::Map::new();
        for field in fields {
            let value = match field.kind {
                DynamicFieldKind::Number => field
                    .value
                    .parse::<f64>()
                    .ok()
                    .and_then(serde_json::Number::from_f64)
                    .map(Value::Number)
                    .unwrap_or_else(|| Value::String(field.value.clone())),
                DynamicFieldKind::Boolean => {
                    Value::Bool(matches!(field.value.as_str(), "true" | "1" | "yes"))
                }
                DynamicFieldKind::Text => Value::String(field.value.clone()),
            };
            values.insert(field.id.clone(), value);
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
                "boolean" | "bool" => DynamicFieldKind::Boolean,
                _ => DynamicFieldKind::Text,
            };
            let value = object
                .get("default")
                .map(|value| match value {
                    Value::String(value) => value.clone(),
                    Value::Bool(value) => value.to_string(),
                    Value::Number(value) => value.to_string(),
                    _ => String::new(),
                })
                .unwrap_or_default();

            Ok(DynamicField {
                id,
                label,
                kind,
                value,
            })
        })
        .collect()
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
                    { "id": "kp", "label": "Kp", "kind": "number", "default": 1.0 }
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
