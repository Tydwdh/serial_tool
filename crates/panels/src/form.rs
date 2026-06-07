use crate::theme;
use serde_json::json;
use tool_core::{Direction, Event, Payload, topics};
use tool_databus::DataBus;

pub struct FormPanel {
    bus: DataBus,
    kp: String,
    ki: String,
    kd: String,
    command: String,
    enabled: bool,
    last_error: Option<String>,
}

impl FormPanel {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            bus: bus.clone(),
            kp: "1.0".to_owned(),
            ki: "0.0".to_owned(),
            kd: "0.0".to_owned(),
            command: "SET_PID".to_owned(),
            enabled: true,
            last_error: None,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("PID 参数");
        ui.horizontal(|ui| {
            ui.label("Kp");
            ui.text_edit_singleline(&mut self.kp);
            ui.label("Ki");
            ui.text_edit_singleline(&mut self.ki);
            ui.label("Kd");
            ui.text_edit_singleline(&mut self.kd);
        });
        ui.horizontal(|ui| {
            ui.label("命令");
            ui.text_edit_singleline(&mut self.command);
            ui.checkbox(&mut self.enabled, "启用");
        });
        ui.horizontal(|ui| {
            if ui.button("应用").clicked() {
                self.publish();
            }
        });

        if let Some(error) = &self.last_error {
            ui.colored_label(theme::RED, error);
        }
    }

    fn publish(&mut self) {
        let kp = match self.kp.parse::<f64>() {
            Ok(value) => value,
            Err(error) => {
                self.last_error = Some(format!("Kp: {error}"));
                return;
            }
        };
        let ki = match self.ki.parse::<f64>() {
            Ok(value) => value,
            Err(error) => {
                self.last_error = Some(format!("Ki: {error}"));
                return;
            }
        };
        let kd = match self.kd.parse::<f64>() {
            Ok(value) => value,
            Err(error) => {
                self.last_error = Some(format!("Kd: {error}"));
                return;
            }
        };

        self.bus.publish(Event::new(
            topics::UI_FORM_CHANGED,
            "ui.form.pid",
            Direction::Internal,
            Payload::Json(json!({
                "command": self.command.clone(),
                "enabled": self.enabled,
                "kp": kp,
                "ki": ki,
                "kd": kd
            })),
        ));
        self.last_error = None;
    }
}
