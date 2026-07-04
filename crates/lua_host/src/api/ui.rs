//! `ctx.ui.*` — UI 面板 API（create_chart/form/attitude/gauge + remove + set_value + set_enabled + set_visible）。

use mlua::{Lua, Table, Value};
use serde_json::Map;

use tool_core::{Direction, Event, Payload, topics};
use tool_databus::DataBus;

use crate::convert::{json_to_lua_value, lua_value_to_json};

pub(crate) fn create_ui_api(
    lua: &Lua,
    bus: DataBus,
    source: String,
    plugin_id: String,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    for (name, kind) in [
        ("create_chart", "chart"),
        ("create_form", "form"),
        ("create_attitude", "attitude"),
        ("create_gauge", "gauge"),
    ] {
        let bus = bus.clone();
        let source = source.clone();
        let pid = plugin_id.clone();

        table.set(
            name,
            lua.create_function(move |_lua, config: Value| {
                let mut config = ensure_json_object(lua_value_to_json(config)?, name)?;

                config.insert(
                    "kind".to_owned(),
                    serde_json::Value::String(kind.to_owned()),
                );

                config.insert(
                    "plugin_id".to_owned(),
                    serde_json::Value::String(pid.clone()),
                );

                ensure_panel_defaults(&mut config, kind)?;

                bus.publish(Event::new(
                    topics::UI_PANEL_CREATE,
                    source.clone(),
                    Direction::Internal,
                    Payload::Json(serde_json::Value::Object(config)),
                ));

                Ok(())
            })?,
        )?;
    }

    let bus_for_remove = bus.clone();
    let source_for_remove = source.clone();

    table.set(
        "remove_panel",
        lua.create_function(move |_lua, panel_id: String| {
            bus_for_remove.publish(Event::new(
                topics::UI_PANEL_REMOVE,
                source_for_remove.clone(),
                Direction::Internal,
                Payload::Json(serde_json::json!({ "id": panel_id })),
            ));

            Ok(())
        })?,
    )?;

    let bus_for_get = bus.clone();

    table.set(
        "get_panel",
        lua.create_function(move |lua, panel_id: String| {
            let panel = bus_for_get
                .history()
                .into_iter()
                .rev()
                .find(|event| {
                    event.topic == topics::UI_PANEL_CREATE
                        && match &event.payload {
                            Payload::Json(value) => {
                                value.get("id").and_then(|value| value.as_str()) == Some(&panel_id)
                            }
                            _ => false,
                        }
                })
                .and_then(|event| match event.payload {
                    Payload::Json(value) => Some(value),
                    _ => None,
                })
                .unwrap_or(serde_json::Value::Null);

            json_to_lua_value(lua, &panel)
        })?,
    )?;

    // ctx.ui.set_value(panel_id, field_id, value)
    let bus_set = bus.clone();
    let src_set = source.clone();
    table.set(
        "set_value",
        lua.create_function(
            move |_lua, (panel_id, field_id, value): (String, String, Value)| {
                bus_set.publish(Event::new(
                    topics::UI_FORM_SET_VALUE,
                    src_set.clone(),
                    Direction::Internal,
                    Payload::Json(serde_json::json!({
                        "panel_id": panel_id,
                        "field_id": field_id,
                        "value": lua_value_to_json(value).unwrap_or(serde_json::Value::Null),
                    })),
                ));
                Ok(())
            },
        )?,
    )?;

    let bus_enabled = bus.clone();
    let src_enabled = source.clone();
    table.set(
        "set_enabled",
        lua.create_function(
            move |_lua, (panel_id, field_id, enabled): (String, String, bool)| {
                bus_enabled.publish(Event::new(
                    topics::UI_FORM_SET_ENABLED,
                    src_enabled.clone(),
                    Direction::Internal,
                    Payload::Json(serde_json::json!({
                        "panel_id": panel_id,
                        "field_id": field_id,
                        "value": enabled,
                    })),
                ));
                Ok(())
            },
        )?,
    )?;

    let bus_visible = bus.clone();
    let src_visible = source.clone();
    table.set(
        "set_visible",
        lua.create_function(
            move |_lua, (panel_id, field_id, visible): (String, String, bool)| {
                bus_visible.publish(Event::new(
                    topics::UI_FORM_SET_VISIBLE,
                    src_visible.clone(),
                    Direction::Internal,
                    Payload::Json(serde_json::json!({
                        "panel_id": panel_id,
                        "field_id": field_id,
                        "value": visible,
                    })),
                ));
                Ok(())
            },
        )?,
    )?;

    // ctx.ui.set_contribution_value(contribution_id, value)
    // 用于更新 toggle / progress / label 等 UI contribution 的运行时状态。
    let bus_scv = bus.clone();
    let src_scv = source.clone();
    table.set(
        "set_contribution_value",
        lua.create_function(move |_lua, (contribution_id, value): (String, Value)| {
            bus_scv.publish(Event::new(
                topics::UI_CONTRIBUTION_SET_VALUE,
                src_scv.clone(),
                Direction::Internal,
                Payload::Json(serde_json::json!({
                    "panel_id": "__contribution__",
                    "field_id": contribution_id,
                    "value": lua_value_to_json(value).unwrap_or(serde_json::Value::Null),
                })),
            ));
            Ok(())
        })?,
    )?;

    // ctx.ui.set_status(text) — 向状态栏推送一条通知（Info 级别）
    let bus_status = bus.clone();
    let src_status = source.clone();
    table.set(
        "set_status",
        lua.create_function(move |_lua, message: String| {
            bus_status.publish(Event::new(
                topics::UI_SET_STATUS,
                src_status.clone(),
                Direction::Internal,
                Payload::Json(serde_json::json!({ "message": message })),
            ));
            Ok(())
        })?,
    )?;

    Ok(table)
}

pub(crate) fn ensure_json_object(
    value: serde_json::Value,
    function_name: &str,
) -> mlua::Result<Map<String, serde_json::Value>> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| mlua::Error::RuntimeError(format!("ctx.ui.{function_name} expects a table")))
}

pub(crate) fn ensure_panel_defaults(
    config: &mut Map<String, serde_json::Value>,
    fallback_kind: &str,
) -> mlua::Result<()> {
    if !config.contains_key("id") {
        return Err(mlua::Error::RuntimeError(
            "panel config requires id".to_owned(),
        ));
    }

    if !config.contains_key("title") {
        let title = config
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or(fallback_kind)
            .to_owned();

        config.insert("title".to_owned(), serde_json::Value::String(title));
    }

    if fallback_kind == "chart"
        && !config.contains_key("topic_prefix")
        && !config.contains_key("topic")
    {
        config.insert(
            "topic_prefix".to_owned(),
            serde_json::Value::String("protocol.".to_owned()),
        );
    }

    if fallback_kind == "form" && !config.contains_key("fields") {
        config.insert("fields".to_owned(), serde_json::Value::Array(Vec::new()));
    }

    if fallback_kind == "attitude" && !config.contains_key("topic") {
        config.insert(
            "topic".to_owned(),
            serde_json::Value::String(tool_core::topics::PROTOCOL_IMU_ATTITUDE.to_owned()),
        );
    }

    if fallback_kind == "gauge" && !config.contains_key("topic") {
        config.insert(
            "topic".to_owned(),
            serde_json::Value::String("protocol.gauge".to_owned()),
        );
    }

    Ok(())
}
