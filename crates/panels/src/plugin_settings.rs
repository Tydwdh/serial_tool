//! Shared plugin settings presentation.
//!
//! The Native and Web runtimes own different persistence and Lua update
//! mechanisms, but the manifest-driven settings form is the same UI.  Keeping
//! the JSON editing here prevents the browser from growing a second form
//! renderer with subtly different defaults, ranges, and option handling.

use egui::{ComboBox, DragValue, Slider, TextEdit};
use serde_json::Value;
use std::collections::BTreeMap;
use tool_application::plugin::PluginSettingView;

use crate::design;
use egui_material_icons::icons::ICON_APPS;

/// Mutable settings state supplied by an application composition root.
pub struct PluginSettingsView<'a> {
    pub plugin_id: &'a str,
    pub plugin_name: &'a str,
    pub settings: &'a [PluginSettingView],
    pub ports: &'a [String],
    pub values: &'a mut BTreeMap<String, Value>,
}

/// Render one manifest's settings in the same card on every platform.
///
/// The return value is true when at least one value changed.  Persistence and
/// runtime notification remain the responsibility of the caller.
pub fn plugin_settings_ui(ui: &mut egui::Ui, view: &mut PluginSettingsView<'_>) -> bool {
    if view.settings.is_empty() {
        return false;
    }

    let mut changed = false;
    design::card().show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        design::section_header(ui, ICON_APPS, format!("{} 设置", view.plugin_name));
        ui.separator();

        for setting in view.settings {
            let mut value = view
                .values
                .remove(&setting.id)
                .unwrap_or_else(|| setting.default.clone());
            let field_changed =
                plugin_setting_field_ui(ui, view.plugin_id, setting, view.ports, &mut value);
            if let Some(description) = setting.description.as_deref() {
                ui.small(description);
            }
            view.values.insert(setting.id.clone(), value);
            changed |= field_changed;
        }
    });
    changed
}

fn plugin_setting_field_ui(
    ui: &mut egui::Ui,
    plugin_id: &str,
    setting: &PluginSettingView,
    ports: &[String],
    value: &mut Value,
) -> bool {
    match setting.kind.as_str() {
        "boolean" | "bool" | "checkbox" => {
            let mut checked = value.as_bool().unwrap_or(false);
            let changed = ui.checkbox(&mut checked, &setting.title).changed();
            if changed {
                *value = Value::Bool(checked);
            }
            changed
        }
        "number" => {
            let min = setting.min.unwrap_or(f64::NEG_INFINITY);
            let max = setting.max.unwrap_or(f64::INFINITY).max(min);
            let mut number = value.as_f64().unwrap_or_default().clamp(min, max);
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(&setting.title);
                let mut drag = DragValue::new(&mut number);
                if let Some(step) = setting.step.filter(|step| *step > 0.0) {
                    drag = drag.speed(step);
                }
                if min.is_finite() && max.is_finite() {
                    drag = drag.range(min..=max);
                }
                changed = ui.add(drag).changed();
            });
            if changed {
                *value = serde_json::json!(number.clamp(min, max));
            }
            changed
        }
        "slider" | "range" => {
            let min = setting.min.unwrap_or(0.0);
            let max = setting.max.unwrap_or(100.0).max(min);
            let mut number = value.as_f64().unwrap_or(min).clamp(min, max);
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(&setting.title);
                let mut slider = Slider::new(&mut number, min..=max);
                if let Some(step) = setting.step.filter(|step| *step > 0.0) {
                    slider = slider.step_by(step);
                }
                changed = ui.add(slider).changed();
            });
            if changed {
                *value = serde_json::json!(number);
            }
            changed
        }
        "select" | "choice" | "enum" | "dropdown" => {
            let mut selected = value.clone();
            let selected_text = setting
                .options
                .iter()
                .find(|option| option_value(option) == selected)
                .map(option_label)
                .unwrap_or_else(|| option_label(&selected));
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(&setting.title);
                ComboBox::from_id_salt(("plugin-setting", plugin_id, &setting.id))
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        for option in &setting.options {
                            let option_value = option_value(option);
                            if ui
                                .selectable_value(&mut selected, option_value, option_label(option))
                                .changed()
                            {
                                changed = true;
                            }
                        }
                    });
            });
            if changed {
                *value = selected;
            }
            changed
        }
        "serial" | "serial_port" | "comport" => {
            let mut selected = value.as_str().unwrap_or_default().to_owned();
            let selected_text = if let Some(port) = ports.iter().find(|port| *port == &selected) {
                port.clone()
            } else if ports.is_empty() {
                "无可用串口".to_owned()
            } else {
                "请选择串口".to_owned()
            };
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(&setting.title);
                ComboBox::from_id_salt(("plugin-setting", plugin_id, &setting.id))
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        if ports.is_empty() {
                            ui.add_enabled(false, egui::Label::new("无可用串口"));
                        } else {
                            for port in ports {
                                if ui
                                    .selectable_value(&mut selected, port.clone(), port)
                                    .changed()
                                {
                                    changed = true;
                                }
                            }
                        }
                    });
            });
            if changed {
                *value = Value::String(selected);
            }
            changed
        }
        "textarea" => {
            let mut text = value.as_str().unwrap_or_default().to_owned();
            ui.label(&setting.title);
            let changed = ui
                .add(
                    TextEdit::multiline(&mut text)
                        .desired_rows(setting.rows.unwrap_or(4).clamp(2, 20))
                        .desired_width(ui.available_width()),
                )
                .changed();
            if changed {
                *value = Value::String(text);
            }
            changed
        }
        _ => {
            let mut text = value.as_str().unwrap_or_default().to_owned();
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(&setting.title);
                changed = ui.add(TextEdit::singleline(&mut text)).changed();
            });
            if changed {
                *value = Value::String(text);
            }
            changed
        }
    }
}

fn option_value(option: &Value) -> Value {
    option
        .get("value")
        .cloned()
        .unwrap_or_else(|| option.clone())
}

fn option_label(option: &Value) -> String {
    if let Some(label) = option.get("label").and_then(Value::as_str) {
        return label.to_owned();
    }
    if let Some(title) = option.get("title").and_then(Value::as_str) {
        return title.to_owned();
    }
    if let Some(text) = option.as_str() {
        return text.to_owned();
    }
    option_value(option).to_string()
}

#[cfg(test)]
mod tests {
    use super::{option_label, option_value};
    use serde_json::json;

    #[test]
    fn option_helpers_support_manifest_objects_and_scalars() {
        let object = json!({"value": "fast", "label": "快速"});
        assert_eq!(option_value(&object), json!("fast"));
        assert_eq!(option_label(&object), "快速");
        assert_eq!(option_value(&json!("slow")), json!("slow"));
        assert_eq!(option_label(&json!("slow")), "slow");
    }
}
