//! 动态表单渲染：`dynamic_form_ui` + 辅助函数。
//!
//! 从 `mod.rs` 抽出的纯 UI 渲染逻辑。

use super::schema::{DynamicField, DynamicFieldKind, FieldOption, compact_number};
use crate::design;
use crate::dynamic::PortItem;
use crate::theme;
use egui::{Color32, ComboBox, DragValue, ProgressBar, RichText, Slider, TextEdit};
use serde_json::Value;
use tool_core::{Direction, Event, Payload, topics};
use tool_databus::EventPublisher;

/// 渲染插件声明的动态表单。
///
/// 用户输入不回传给调用方，而是发布到总线上：`auto_apply` 为真时每次改动立刻
/// 发 `ui.form.changed`，否则等「应用」按钮；按钮点击单独发 `ui.form.action`。
///
/// 这里只按 `kind` 分发，各字段类型的渲染在同文件下方的私有 `fn` 里；分发顺序
/// 就是字段遍历与控件注册顺序（egui 的命中优先级与 id salt 都依赖它）。
pub fn dynamic_form_ui<P: EventPublisher>(
    ui: &mut egui::Ui,
    bus: &P,
    panel_id: &str,
    fields: &mut [DynamicField],
    auto_apply: bool,
    ports: &[PortItem],
) {
    let mut changed = false;

    // 预收集输入型字段值（供 button action 使用），排除 display-only 字段
    let field_values: Vec<(String, Value)> = fields
        .iter()
        .filter(|f| !is_display_only(f.kind))
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
            DynamicFieldKind::Label => render_label(ui, field),
            // ── 按钮 ──
            DynamicFieldKind::Button => {
                render_button(ui, bus, panel_id, field, &field_values, enabled);
            }
            // ── 进度条 ──
            DynamicFieldKind::Progress => render_progress(ui, field),
            // ── 状态 ──
            DynamicFieldKind::Status => render_status(ui, field),
            // ── TextArea ──
            DynamicFieldKind::TextArea => {
                // TextArea 自带整行布局，不走下面「label 左 / 控件右」的横向逻辑
                changed |= render_text_area(ui, field, enabled);
            }
            // ── File ──
            DynamicFieldKind::File => render_file(ui, bus, panel_id, field, enabled),
            // ── 可编辑控件：label 在左、控件在右，同处一行 ──
            _ => {
                changed |= render_input_row(ui, field, panel_id, ports, enabled);
            }
        }
    }

    // auto_apply 下改动即提交；否则渲染「应用」按钮，由点击决定提交时机
    let submit = if auto_apply {
        changed
    } else {
        ui.button("应用").clicked()
    };
    if submit {
        publish_form_changed(bus, panel_id, fields);
    }
}

/// 标签字段：只读展示。
fn render_label(ui: &mut egui::Ui, field: &DynamicField) {
    ui.label(RichText::new(label_text(field)).color(theme::text_secondary()));
}

/// 按钮字段：不改写表单值；点击时把 action 连同预收集的输入值发到总线。
fn render_button(
    ui: &mut egui::Ui,
    bus: &dyn EventPublisher,
    panel_id: &str,
    field: &DynamicField,
    field_values: &[(String, Value)],
    enabled: bool,
) {
    let text = button_text(field);
    let fill = match field.variant.as_deref() {
        Some("primary") => theme::blue(),
        Some("danger") => theme::red(),
        _ => theme::bg_tertiary(),
    };
    let btn = egui::Button::new(RichText::new(text).color(theme::text_white()))
        .fill(fill)
        .min_size(egui::vec2(80.0, 28.0));
    if !ui.add_enabled(enabled, btn).clicked() {
        return;
    }
    let values: serde_json::Map<String, Value> = field_values.iter().cloned().collect();
    bus.publish_event(Event::new(
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

/// 进度条字段：只读展示。
fn render_progress(ui: &mut egui::Ui, field: &DynamicField) {
    if let Some(label) = field.text.as_deref() {
        ui.label(label);
    }
    let v = progress_percent(field);
    ui.add(ProgressBar::new((v / 100.0) as f32).text(format!("{v:.0}%")));
}

/// 状态字段：只读展示。
fn render_status(ui: &mut egui::Ui, field: &DynamicField) {
    let (text, level) = status_text_and_level(field);
    ui.label(RichText::new(text).color(status_color(level)));
}

/// 多行文本：label 在上、输入框在下，独占一行。返回本次交互是否改值。
fn render_text_area(ui: &mut egui::Ui, field: &mut DynamicField, enabled: bool) -> bool {
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
        let changed = resp.changed();
        if changed {
            field.value = Value::String(text);
        }
        changed
    })
    .inner
}

/// 文件字段：只读路径展示 + 「浏览」按钮，选中结果由总线回调写回。
fn render_file(
    ui: &mut egui::Ui,
    bus: &dyn EventPublisher,
    panel_id: &str,
    field: &DynamicField,
    enabled: bool,
) {
    let path = field.value.as_str().unwrap_or("").to_owned();
    let row = design::FieldRow::new(&field.label)
        .label_width(120.0)
        .min_value_width(180.0)
        .min_actions_width(72.0)
        .actions(1)
        .show(
            ui,
            |ui| {
                let mut status = file_browse_status(&path);
                ui.add_enabled(
                    false,
                    TextEdit::singleline(&mut status).desired_width(ui.available_width()),
                )
                .on_hover_text(if path.is_empty() {
                    String::new()
                } else {
                    format!("完整路径: {path}")
                });
            },
            |ui| ui.add_enabled(enabled, egui::Button::new("浏览")).clicked(),
        );
    if row.actions {
        bus.publish_event(Event::new(
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
}

/// 可编辑控件的统一行布局：label 在左、控件在右。返回本次交互是否改值。
fn render_input_row(
    ui: &mut egui::Ui,
    field: &mut DynamicField,
    panel_id: &str,
    ports: &[PortItem],
    enabled: bool,
) -> bool {
    // 标签先克隆：值闭包要可变借用 field，不能同时保留 &field.label。
    let label = field.label.clone();
    design::FieldRow::new(&label)
        .label_width(120.0)
        .min_value_width(220.0)
        .show_value(ui, |ui| {
            match field.kind {
                DynamicFieldKind::Text => render_text_field(ui, field, enabled),
                DynamicFieldKind::Number => render_number_field(ui, field, enabled),
                DynamicFieldKind::Boolean => render_boolean_field(ui, field, enabled),
                DynamicFieldKind::Select => render_select_field(ui, field, panel_id),
                DynamicFieldKind::Slider => render_slider_field(ui, field, enabled),
                DynamicFieldKind::Serial => render_serial_field(ui, field, panel_id, ports),
                _ => false, // Button, TextArea 等已在上面处理
            }
        })
        .value
}

/// 单行文本。
fn render_text_field(ui: &mut egui::Ui, field: &mut DynamicField, enabled: bool) -> bool {
    let mut text = field.value.as_str().unwrap_or("").to_owned();
    let resp = ui.add_enabled(
        enabled,
        TextEdit::singleline(&mut text).desired_width(ui.available_width().min(220.0)),
    );
    let changed = resp.changed();
    if changed {
        field.value = Value::String(text);
    }
    changed
}

/// 数字：拖拽调节，范围与步进取自清单的 min/max/step。
fn render_number_field(ui: &mut egui::Ui, field: &mut DynamicField, enabled: bool) -> bool {
    let mut value = field.value.as_f64().unwrap_or_default();
    let resp = ui.add_enabled(
        enabled,
        DragValue::new(&mut value)
            .speed(field.step.unwrap_or(1.0))
            .range(field.min.unwrap_or(f64::NEG_INFINITY)..=field.max.unwrap_or(f64::INFINITY)),
    );
    let changed = resp.changed();
    if changed {
        field.value = json_number(value);
    }
    changed
}

/// 开关。
fn render_boolean_field(ui: &mut egui::Ui, field: &mut DynamicField, enabled: bool) -> bool {
    let mut value = field.value.as_bool().unwrap_or(false);
    let resp = ui.add_enabled(enabled, egui::Checkbox::new(&mut value, ""));
    let changed = resp.changed();
    if changed {
        field.value = Value::Bool(value);
    }
    changed
}

/// 下拉选择：选项文字取自清单的 options。
///
/// 与提取前一致：`field.enabled` 不参与下拉框的可用状态。
fn render_select_field(ui: &mut egui::Ui, field: &mut DynamicField, panel_id: &str) -> bool {
    let current = field.value.as_str().unwrap_or("").to_owned();
    let selected_text = selected_option_text(&field.options, &current);

    let options = field.options.clone();
    let mut new_value = current;
    let mut field_changed = false;

    ComboBox::from_id_salt((panel_id, &field.id))
        .width(ui.available_width().min(180.0))
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            for option in options {
                if ui
                    .selectable_value(&mut new_value, option.value.clone(), &option.label)
                    .changed()
                {
                    field_changed = true;
                }
            }
        });

    if field_changed {
        field.value = Value::String(new_value);
    }
    field_changed
}

/// 滑块：min/max 缺省按 0~100 处理，当前值先夹进范围。
fn render_slider_field(ui: &mut egui::Ui, field: &mut DynamicField, enabled: bool) -> bool {
    let min = field.min.unwrap_or(0.0);
    let max = field.max.unwrap_or(100.0);
    let mut value = field.value.as_f64().unwrap_or(min).clamp(min, max);

    ui.spacing_mut().slider_width = ui.available_width().min(180.0);
    let resp = ui.add_enabled(
        enabled,
        Slider::new(&mut value, min..=max)
            .step_by(field.step.unwrap_or(1.0))
            .show_value(true),
    );

    let changed = resp.changed();
    if changed {
        field.value = json_number(value);
    }
    changed
}

/// 串口下拉：候选来自当前可用串口列表。
///
/// 与提取前一致：`field.enabled` 不参与下拉框的可用状态。
fn render_serial_field(
    ui: &mut egui::Ui,
    field: &mut DynamicField,
    panel_id: &str,
    ports: &[PortItem],
) -> bool {
    let current = field.value.as_str().unwrap_or("").to_owned();
    // 未选中端口时，按有没有可用串口给占位文案
    let placeholder = if ports.is_empty() {
        "无可用串口"
    } else {
        "请选择串口"
    };
    let selected_text = ports
        .iter()
        .find(|p| p.port_name == current)
        .map(|p| p.port_name.clone())
        .unwrap_or_else(|| placeholder.to_owned());

    let mut new_value = current;
    let mut field_changed = false;

    egui::ComboBox::from_id_salt((panel_id, &field.id))
        .width(ui.available_width().min(180.0))
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
                            port.port_name.clone(),
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
    }
    field_changed
}

/// 标签显示文本：优先用 set_value 设置的运行时文本，否则回退到静态 text / label。
fn label_text(field: &DynamicField) -> &str {
    field
        .value
        .as_str()
        .filter(|s| !s.is_empty())
        .or(field.text.as_deref())
        .unwrap_or(&field.label)
}

/// 按钮文字：允许 set_value 动态覆盖按钮文字，空字符串视为未设置。
///
/// 与标签不同，这里按 `trim()` 判空：纯空白不算「已设置」。
fn button_text(field: &DynamicField) -> &str {
    field
        .value
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .or(field.text.as_deref().filter(|s| !s.trim().is_empty()))
        .unwrap_or(&field.label)
}

/// 进度条百分比（0~100）：兼容 number（百分比）和 {current, total}。
fn progress_percent(field: &DynamicField) -> f64 {
    match (
        field.value.get("current").and_then(Value::as_f64),
        field.value.get("total").and_then(Value::as_f64),
    ) {
        (Some(current), Some(total)) if total > 0.0 => (current / total * 100.0).clamp(0.0, 100.0),
        (Some(_), Some(_)) => 0.0,
        _ => field.value.as_f64().unwrap_or(0.0).clamp(0.0, 100.0),
    }
}

/// 状态文本与级别：兼容 string 和 {text, level}。
fn status_text_and_level(field: &DynamicField) -> (&str, &str) {
    let value = &field.value;
    if value.is_string() {
        (value.as_str().unwrap_or(""), "running")
    } else {
        (
            value.get("text").and_then(Value::as_str).unwrap_or(""),
            value.get("level").and_then(Value::as_str).unwrap_or("idle"),
        )
    }
}

/// 下拉框显示选中项的标签文字；当前值不在选项里时原样显示该值。
fn selected_option_text(options: &[FieldOption], current: &str) -> String {
    options
        .iter()
        .find(|o| o.value == current)
        .map(|o| o.label.clone())
        .unwrap_or_else(|| current.to_owned())
}

/// 文件字段的只读展示文本：路径按宽度截断，完整路径放 hover 提示。
fn file_browse_status(path: &str) -> String {
    if path.is_empty() {
        "未选择".to_owned()
    } else {
        crate::ellipsize_tail(path, 40)
    }
}

/// 数值字段的写回值：正常数存 JSON number；NaN/Infinity 不是合法 JSON 数值，
/// 退化成 `compact_number` 的字符串表示。
fn json_number(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::String(compact_number(value)))
}

/// 只读展示型字段：既不出现在 `form.changed` 的 values 里，也不参与按钮取值。
fn is_display_only(kind: DynamicFieldKind) -> bool {
    matches!(
        kind,
        DynamicFieldKind::Separator
            | DynamicFieldKind::Label
            | DynamicFieldKind::Progress
            | DynamicFieldKind::Status
            | DynamicFieldKind::Button
    )
}

/// 把输入型字段的当前值打包成 `ui.form.changed` 事件发出去。
pub(super) fn publish_form_changed<P: EventPublisher>(
    bus: &P,
    panel_id: &str,
    fields: &[DynamicField],
) {
    let mut values = serde_json::Map::new();

    for field in fields {
        if is_display_only(field.kind) {
            continue;
        }
        values.insert(field.id.clone(), field.value.clone());
    }

    bus.publish_event(Event::new(
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dynamic_form_text_row_fits_narrow_panel() {
        let bus = tool_databus::DataBus::new();
        let mut fields = vec![
            DynamicField {
                id: "name".to_owned(),
                label: "名称".to_owned(),
                kind: DynamicFieldKind::Text,
                value: json!("hello"),
                options: Vec::new(),
                min: None,
                max: None,
                step: None,
                rows: None,
                variant: None,
                text: None,
                filters: Vec::new(),
                enabled: true,
                visible: true,
                action: None,
            },
            DynamicField {
                id: "path".to_owned(),
                label: "文件".to_owned(),
                kind: DynamicFieldKind::File,
                value: json!(""),
                options: Vec::new(),
                min: None,
                max: None,
                step: None,
                rows: None,
                variant: None,
                text: None,
                filters: Vec::new(),
                enabled: true,
                visible: true,
                action: None,
            },
        ];
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(240.0, 200.0),
            )),
            ..Default::default()
        };
        let mut right = 0.0;
        let _ = ctx.run_ui(raw, |ui| {
            dynamic_form_ui(ui, &bus, "panel", &mut fields, true, &[]);
            right = ui.min_rect().right();
        });
        assert!(right <= 240.5, "dynamic form row overflowed: {right}");
    }
}
