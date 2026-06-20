//! 动态面板的数据模型（schema）与 JSON 解析（纯函数）。
//!
//! [`DynamicField`]/[`DynamicFieldKind`]/[`FieldOption`]/[`FieldFilter`] 描述
//! 插件通过 `ui.panel.create` 事件声明的表单字段；`parse_fields` 等函数把
//! `serde_json::Value` 解析为这些类型。解析逻辑与渲染（`dynamic.rs` 的
//! `dynamic_form_ui`）解耦，便于独立测试。

use serde_json::Value;

#[derive(Debug, Clone)]
pub(super) struct DynamicField {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) kind: DynamicFieldKind,
    pub(super) value: Value,
    pub(super) options: Vec<FieldOption>,
    pub(super) min: Option<f64>,
    pub(super) max: Option<f64>,
    pub(super) step: Option<f64>,
    // ── v0.2 新增 ──
    pub(super) rows: Option<usize>,
    pub(super) variant: Option<String>,
    pub(super) text: Option<String>,
    pub(super) filters: Vec<FieldFilter>,
    pub(super) enabled: bool,
    pub(super) visible: bool,
    // ── v0.3 新增 ──
    pub(super) action: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DynamicFieldKind {
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
    // ── v0.3 新增 ──
    Serial,
}

#[derive(Debug, Clone)]
pub(super) struct FieldOption {
    pub(super) label: String,
    pub(super) value: String,
}

#[derive(Debug, Clone)]
pub struct FieldFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

pub(super) fn parse_fields(value: Option<&Value>) -> Result<Vec<DynamicField>, String> {
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
            // kind 大小写不敏感
            let kind_raw = object
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("text")
                .to_ascii_lowercase();
            let kind = match kind_raw.as_str() {
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
                "serial" | "serial_port" | "comport" => DynamicFieldKind::Serial,
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

            // value 优先使用 value 字段，否则 default，否则按类型 fallback
            let field_value = object
                .get("value")
                .cloned()
                .or_else(|| object.get("default").cloned())
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
                value: field_value,
                options,
                min: object.get("min").and_then(Value::as_f64),
                max: object.get("max").and_then(Value::as_f64),
                step: object
                    .get("step")
                    .and_then(Value::as_f64)
                    .map(|s| s.max(f64::EPSILON)),
                rows: object
                    .get("rows")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize),
                variant: object
                    .get("variant")
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
                action: object
                    .get("action")
                    .and_then(Value::as_str)
                    .map(String::from),
            })
        })
        .collect()
}

pub(super) fn parse_filters(value: Option<&Value>) -> Result<Vec<FieldFilter>, String> {
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

pub(super) fn parse_options(value: Option<&Value>) -> Result<Vec<FieldOption>, String> {
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

pub(super) fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Null => String::new(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

pub(super) fn compact_number(value: f64) -> String {
    if value.fract() == 0.0 {
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
    use serde_json::json;

    #[test]
    fn parse_fields_empty_when_not_array() {
        // 非 array（如 None / object）返回空
        assert!(parse_fields(None).unwrap().is_empty());
        assert!(parse_fields(Some(&json!({}))).unwrap().is_empty());
    }

    #[test]
    fn parse_fields_requires_id_for_input_kinds() {
        // text 无 id → 报错
        let err = parse_fields(Some(&json!([{"kind": "text"}]))).unwrap_err();
        assert!(err.contains("requires id"));
    }

    #[test]
    fn parse_fields_separator_label_get_fallback_id() {
        let fields = parse_fields(Some(&json!([
            {"kind": "separator"},
            {"kind": "label", "text": "hi"}
        ])))
        .unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].id, "__field_0");
        assert_eq!(fields[1].id, "__field_1");
        assert_eq!(fields[0].kind, DynamicFieldKind::Separator);
        assert_eq!(fields[1].kind, DynamicFieldKind::Label);
    }

    #[test]
    fn parse_fields_kind_case_insensitive_and_aliases() {
        let fields = parse_fields(Some(&json!([
            {"id": "a", "kind": "NUMBER"},
            {"id": "b", "kind": "checkbox"},
            {"id": "c", "kind": "dropdown"},
            {"id": "d", "kind": "comport"},
            {"id": "e", "kind": "unknown-kind"}
        ])))
        .unwrap();
        assert_eq!(fields[0].kind, DynamicFieldKind::Number);
        assert_eq!(fields[1].kind, DynamicFieldKind::Boolean);
        assert_eq!(fields[2].kind, DynamicFieldKind::Select);
        assert_eq!(fields[3].kind, DynamicFieldKind::Serial);
        // 未知 kind fallback 为 Text
        assert_eq!(fields[4].kind, DynamicFieldKind::Text);
    }

    #[test]
    fn parse_fields_label_falls_back_to_title_then_id() {
        let fields =
            parse_fields(Some(&json!([{"id": "x", "title": "标题"}, {"id": "y"}]))).unwrap();
        assert_eq!(fields[0].label, "标题"); // title 优先
        assert_eq!(fields[1].label, "y"); // 无 label/title 用 id
    }

    #[test]
    fn parse_fields_value_fallback_chain() {
        // value > default > 类型 fallback
        let with_value = parse_fields(Some(&json!([{"id": "a", "value": 5}]))).unwrap();
        assert_eq!(with_value[0].value, json!(5));

        let with_default = parse_fields(Some(&json!([{"id": "a", "default": "d"}]))).unwrap();
        assert_eq!(with_default[0].value, json!("d"));

        // Progress 默认 0
        let progress = parse_fields(Some(&json!([{"id": "p", "kind": "progress"}]))).unwrap();
        assert_eq!(progress[0].value, json!(0));

        // Status 默认 idle 对象
        let status = parse_fields(Some(&json!([{"id": "s", "kind": "status"}]))).unwrap();
        assert_eq!(status[0].value, json!({"text": "空闲", "level": "idle"}));

        // Select 无 value/default → 取第一个 option
        let select = parse_fields(Some(&json!([{
            "id": "sel",
            "kind": "select",
            "options": ["a", "b"]
        }])))
        .unwrap();
        assert_eq!(select[0].value, json!("a"));
    }

    #[test]
    fn parse_fields_enabled_visible_default_true() {
        let fields = parse_fields(Some(&json!([{"id": "a"}]))).unwrap();
        assert!(fields[0].enabled);
        assert!(fields[0].visible);
    }

    #[test]
    fn parse_options_variants() {
        // String / Number / Bool / Object
        let opts =
            parse_options(Some(&json!(["x", 1, true, {"value": "k", "label": "K"}]))).unwrap();
        assert_eq!(opts.len(), 4);
        assert_eq!(opts[0].label, "x");
        assert_eq!(opts[0].value, "x");
        assert_eq!(opts[1].value, "1");
        assert_eq!(opts[2].value, "true");
        assert_eq!(opts[3].label, "K");
        assert_eq!(opts[3].value, "k");
    }

    #[test]
    fn parse_options_object_requires_value() {
        let err = parse_options(Some(&json!([{"label": "no-value"}]))).unwrap_err();
        assert!(err.contains("requires value"));
    }

    #[test]
    fn parse_options_rejects_non_array() {
        assert!(parse_options(Some(&json!("not array"))).is_err());
        assert!(parse_options(None).unwrap().is_empty());
    }

    #[test]
    fn parse_filters_extracts_name_and_extensions() {
        let filters = parse_filters(Some(&json!([
            {"name": "文本", "extensions": ["txt", "csv"]},
            {}
        ])))
        .unwrap();
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].name, "文本");
        assert_eq!(filters[0].extensions, vec!["txt", "csv"]);
        assert!(filters[1].extensions.is_empty());
    }

    #[test]
    fn parse_filters_rejects_non_array() {
        assert!(parse_filters(Some(&json!("x"))).is_err());
        assert!(parse_filters(None).unwrap().is_empty());
    }

    #[test]
    fn value_to_string_variants() {
        assert_eq!(value_to_string(&json!("hi")), "hi");
        assert_eq!(value_to_string(&json!(true)), "true");
        assert_eq!(value_to_string(&json!(42)), "42");
        assert_eq!(value_to_string(&Value::Null), "");
        assert_eq!(value_to_string(&json!([1, 2])), "[1,2]");
    }

    #[test]
    fn compact_number_trims_trailing_zeros() {
        assert_eq!(compact_number(3.0), "3");
        assert_eq!(compact_number(3.14000), "3.14");
        assert_eq!(compact_number(3.0), "3");
        // 5 位小数截断到 4 位后去尾零
        assert_eq!(compact_number(1.5), "1.5");
    }
}
