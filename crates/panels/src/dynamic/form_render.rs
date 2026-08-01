//! 动态表单渲染：`dynamic_form_ui` + 辅助函数。
//!
//! 从 `mod.rs` 抽出的纯 UI 渲染逻辑，约 400 行。

use super::schema::{DynamicField, DynamicFieldKind, compact_number};
use crate::theme;
use egui::{Color32, ComboBox, DragValue, ProgressBar, RichText, Slider, TextEdit};
use serde_json::Value;
use tool_core::{Direction, Event, Payload, topics};
use tool_databus::DataBus;
use tool_transport::SerialPortDescriptor;

pub fn dynamic_form_ui(
    ui: &mut egui::Ui,
    bus: &DataBus,
    panel_id: &str,
    fields: &mut [DynamicField],
    auto_apply: bool,
    ports: &[SerialPortDescriptor],
) {
    let mut changed = false;

    // 预收集输入型字段值（供 button action 使用），排除 display-only 字段
    let field_values: Vec<(String, Value)> = fields
        .iter()
        .filter(|f| {
            !matches!(
                f.kind,
                DynamicFieldKind::Separator
                    | DynamicFieldKind::Label
                    | DynamicFieldKind::Progress
                    | DynamicFieldKind::Status
                    | DynamicFieldKind::Button
            )
        })
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
                // 优先用 set_value 设置的运行时文本，否则回退到静态 text / label
                let text = field
                    .value
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .or(field.text.as_deref())
                    .unwrap_or(&field.label);
                ui.label(RichText::new(text).color(theme::text_secondary()));
            }
            // ── 按钮 ──
            DynamicFieldKind::Button => {
                // 允许 set_value 动态覆盖按钮文字，空字符串视为未设置
                let text = field
                    .value
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .or(field.text.as_deref().filter(|s| !s.trim().is_empty()))
                    .unwrap_or(&field.label);
                let fill = match field.variant.as_deref() {
                    Some("primary") => theme::blue(),
                    Some("danger") => theme::red(),
                    _ => theme::bg_tertiary(),
                };
                let btn = egui::Button::new(RichText::new(text).color(theme::text_white()))
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
                            "action": field.action,
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
                // 兼容 number（百分比）和 {current, total}
                let v = if field.value.is_number() {
                    field.value.as_f64().unwrap_or(0.0).clamp(0.0, 100.0)
                } else if let (Some(c), Some(t)) = (
                    field.value.get("current").and_then(Value::as_f64),
                    field.value.get("total").and_then(Value::as_f64),
                ) {
                    if t > 0.0 {
                        (c / t * 100.0).clamp(0.0, 100.0)
                    } else {
                        0.0
                    }
                } else {
                    field.value.as_f64().unwrap_or(0.0).clamp(0.0, 100.0)
                };
                ui.add(ProgressBar::new((v / 100.0) as f32).text(format!("{v:.0}%")));
            }
            // ── 状态 ──
            DynamicFieldKind::Status => {
                // 兼容 string 和 {text, level}
                let (text, level) = if field.value.is_string() {
                    (
                        field.value.as_str().unwrap_or("").to_owned(),
                        "running".to_owned(),
                    )
                } else {
                    (
                        field
                            .value
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        field
                            .value
                            .get("level")
                            .and_then(Value::as_str)
                            .unwrap_or("idle")
                            .to_owned(),
                    )
                };
                let color = status_color(&level);
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
                    let status = if path.is_empty() {
                        "未选择".to_owned()
                    } else {
                        crate::ellipsize_tail(&path, 40)
                    };
                    ui.add_enabled(
                        false,
                        TextEdit::singleline(&mut status.clone()).desired_width(200.0),
                    )
                    .on_hover_text(if path.is_empty() {
                        String::new()
                    } else {
                        format!("完整路径: {path}")
                    });
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
                        DynamicFieldKind::Serial => {
                            let current = field.value.as_str().unwrap_or("").to_owned();
                            let selected_text = ports
                                .iter()
                                .find(|p| p.port_name == current)
                                .map(|p| format!("{}  {}", p.port_name, p.port_type))
                                .unwrap_or_else(|| {
                                    if ports.is_empty() {
                                        "无可用串口".to_owned()
                                    } else {
                                        "请选择串口".to_owned()
                                    }
                                });

                            let mut new_value = current;
                            let mut field_changed = false;

                            egui::ComboBox::from_id_salt((panel_id, &field.id))
                                .width(180.0)
                                .selected_text(selected_text)
                                .show_ui(ui, |ui| {
                                    if ports.is_empty() {
                                        ui.add_enabled(false, egui::Label::new("无可用串口"));
                                    } else {
                                        for port in ports {
                                            if ui
                                                .selectable_value(
                                                    &mut new_value,
                                                    port.port_name.clone(),
                                                    format!(
                                                        "{}  {}",
                                                        port.port_name, port.port_type
                                                    ),
                                                )
                                                .changed()
                                            {
                                                field_changed = true;
                                            }
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
                        _ => false, // Button, TextArea 等已在上面处理
                    };

                    changed |= field_changed;
                });
            }
        }
    }

    if auto_apply {
        if changed {
            publish_form_changed(bus, panel_id, fields);
        }
    } else if ui.button("应用").clicked() {
        publish_form_changed(bus, panel_id, fields);
    }
}

pub(super) fn publish_form_changed(bus: &DataBus, panel_id: &str, fields: &[DynamicField]) {
    let mut values = serde_json::Map::new();

    for field in fields {
        // 只包含输入型字段，排除 display-only
        if matches!(
            field.kind,
            DynamicFieldKind::Separator
                | DynamicFieldKind::Label
                | DynamicFieldKind::Progress
                | DynamicFieldKind::Status
                | DynamicFieldKind::Button
        ) {
            continue;
        }
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

pub(super) fn status_color(level: &str) -> Color32 {
    match level {
        "running" => theme::blue(),
        "success" => theme::green(),
        "warn" => theme::yellow(),
        "error" => theme::red(),
        _ => theme::text_secondary(), // idle
    }
}
