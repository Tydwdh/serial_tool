//! `ctx.ui.*` — 动态面板创建与状态更新 API。

use mlua::{Lua, Table, Value};
use serde_json::Map;
use std::collections::BTreeSet;

use tool_core::{Direction, Event, Payload, topics};
use tool_databus::DataBus;

use crate::convert::{json_to_lua_value, lua_value_to_json};

pub(crate) fn create_ui_api(
    lua: &Lua,
    bus: DataBus,
    source: String,
    plugin_id: String,
    declared_panel_ids: &BTreeSet<String>,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    // 一个功能域一个 register：键名与注册顺序都保持拆分前不变。
    register_panel_create_apis(lua, &table, &bus, &source, &plugin_id, declared_panel_ids)?;
    register_remove_panel_api(lua, &table, &bus, &source, declared_panel_ids)?;
    register_set_values_api(lua, &table, &bus, &source)?;
    register_table_row_apis(lua, &table, &bus, &source)?;
    register_get_panel_api(lua, &table, &bus)?;
    register_form_field_apis(lua, &table, &bus, &source)?;
    register_contribution_value_api(lua, &table, &bus, &source)?;
    register_set_status_api(lua, &table, &bus, &source)?;

    Ok(table)
}

/// ── 面板创建 ──：`create_chart` / `create_form` / `create_attitude` /
/// `create_gauge` / `create_table`，各自补上 kind 与 defaults 后发创建事件。
fn register_panel_create_apis(
    lua: &Lua,
    ui: &Table,
    bus: &DataBus,
    source: &str,
    plugin_id: &str,
    declared_panel_ids: &BTreeSet<String>,
) -> mlua::Result<()> {
    for (name, kind) in [
        ("create_chart", "chart"),
        ("create_form", "form"),
        ("create_attitude", "attitude"),
        ("create_gauge", "gauge"),
        ("create_table", "table"),
    ] {
        let bus = bus.clone();
        let source = source.to_owned();
        let pid = plugin_id.to_owned();
        let static_ids = declared_panel_ids.clone();

        ui.set(
            name,
            lua.create_function(move |_lua, config: Value| {
                let mut config = ensure_json_object(lua_value_to_json(config)?, name)?;
                if let Some(id) = config.get("id").and_then(serde_json::Value::as_str)
                    && static_ids.contains(id)
                {
                    return Err(host_managed_panel_error(id));
                }

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

    Ok(())
}

/// ── 面板销毁 ──：`remove_panel(panel_id)`，宿主托管面板拒绝移除。
fn register_remove_panel_api(
    lua: &Lua,
    ui: &Table,
    bus: &DataBus,
    source: &str,
    declared_panel_ids: &BTreeSet<String>,
) -> mlua::Result<()> {
    let bus_for_remove = bus.clone();
    let source_for_remove = source.to_owned();
    let static_ids_for_remove = declared_panel_ids.clone();

    ui.set(
        "remove_panel",
        lua.create_function(move |_lua, panel_id: String| {
            if static_ids_for_remove.contains(&panel_id) {
                return Err(host_managed_panel_error(&panel_id));
            }
            bus_for_remove.publish(Event::new(
                topics::UI_PANEL_REMOVE,
                source_for_remove.clone(),
                Direction::Internal,
                Payload::Json(serde_json::json!({ "id": panel_id })),
            ));

            Ok(())
        })?,
    )?;

    Ok(())
}

/// ── 表单批量赋值 ──：`ctx.ui.set_values(panel_id, { field = value, ... })`：单事件批量更新。
fn register_set_values_api(lua: &Lua, ui: &Table, bus: &DataBus, source: &str) -> mlua::Result<()> {
    let bus_values = bus.clone();
    let src_values = source.to_owned();
    ui.set(
        "set_values",
        lua.create_function(move |_lua, (panel_id, values): (String, Value)| {
            let values = ensure_json_object(lua_value_to_json(values)?, "set_values")?;
            bus_values.publish(Event::new(
                topics::UI_PANEL_SET_VALUES,
                src_values.clone(),
                Direction::Internal,
                Payload::Json(serde_json::json!({
                    "panel_id": panel_id,
                    "values": values,
                })),
            ));
            Ok(())
        })?,
    )?;

    Ok(())
}

/// ── 表格行操作 ──：`table_set_rows` / `table_append_rows` / `table_remove_rows` / `table_clear`。
fn register_table_row_apis(lua: &Lua, ui: &Table, bus: &DataBus, source: &str) -> mlua::Result<()> {
    for (name, topic) in [
        ("table_set_rows", topics::UI_TABLE_SET_ROWS),
        ("table_append_rows", topics::UI_TABLE_APPEND_ROWS),
        ("table_remove_rows", topics::UI_TABLE_REMOVE_ROWS),
    ] {
        let op_bus = bus.clone();
        let op_source = source.to_owned();
        ui.set(
            name,
            lua.create_function(move |_lua, (panel_id, rows): (String, Value)| {
                op_bus.publish(Event::new(
                    topic,
                    op_source.clone(),
                    Direction::Internal,
                    Payload::Json(serde_json::json!({
                        "panel_id": panel_id,
                        "rows": lua_value_to_json(rows).unwrap_or(serde_json::Value::Null),
                    })),
                ));
                Ok(())
            })?,
        )?;
    }

    let clear_bus = bus.clone();
    let clear_source = source.to_owned();
    ui.set(
        "table_clear",
        lua.create_function(move |_lua, panel_id: String| {
            clear_bus.publish(Event::new(
                topics::UI_TABLE_CLEAR,
                clear_source.clone(),
                Direction::Internal,
                Payload::Json(serde_json::json!({ "panel_id": panel_id })),
            ));
            Ok(())
        })?,
    )?;

    Ok(())
}

/// ── 面板查询 ──：`get_panel(panel_id)` 返回面板创建时的配置。
fn register_get_panel_api(lua: &Lua, ui: &Table, bus: &DataBus) -> mlua::Result<()> {
    let bus_for_get = bus.clone();

    ui.set(
        "get_panel",
        lua.create_function(move |lua, panel_id: String| {
            let panel = find_panel_config(&bus_for_get, &panel_id);
            json_to_lua_value(lua, &panel)
        })?,
    )?;

    Ok(())
}

/// 倒查事件历史，取该面板最近一次 create 事件的配置；没有则给 `Null`。
fn find_panel_config(bus: &DataBus, panel_id: &str) -> serde_json::Value {
    bus.history()
        .into_iter()
        .rev()
        .find(|event| {
            let is_panel_event =
                event.topic == topics::UI_PANEL_CREATE || event.topic == topics::UI_PANEL_REMOVE;
            // create 事件里面板 id 叫 id，remove 事件里叫 panel_id。
            let event_panel_id = match &event.payload {
                Payload::Json(value) => value
                    .get("id")
                    .or_else(|| value.get("panel_id"))
                    .and_then(|value| value.as_str()),
                _ => None,
            };

            is_panel_event && event_panel_id == Some(panel_id)
        })
        .and_then(|event| match event.payload {
            Payload::Json(value) if event.topic == topics::UI_PANEL_CREATE => Some(value),
            _ => None,
        })
        .unwrap_or(serde_json::Value::Null)
}

/// ── 表单字段 ──：`ctx.ui.set_value(panel_id, field_id, value)`，
/// 以及只有 Lua 名与 topic 不同、参数与载荷形状一致的 `set_enabled` / `set_visible`。
fn register_form_field_apis(
    lua: &Lua,
    ui: &Table,
    bus: &DataBus,
    source: &str,
) -> mlua::Result<()> {
    let bus_set = bus.clone();
    let src_set = source.to_owned();
    ui.set(
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

    // ctx.ui.set_enabled / set_visible：只有 Lua 名与 topic 不同，参数与载荷形状一致。
    for (name, topic) in [
        ("set_enabled", topics::UI_FORM_SET_ENABLED),
        ("set_visible", topics::UI_FORM_SET_VISIBLE),
    ] {
        let op_bus = bus.clone();
        let op_source = source.to_owned();
        ui.set(
            name,
            lua.create_function(
                move |_lua, (panel_id, field_id, value): (String, String, bool)| {
                    op_bus.publish(Event::new(
                        topic,
                        op_source.clone(),
                        Direction::Internal,
                        Payload::Json(serde_json::json!({
                            "panel_id": panel_id,
                            "field_id": field_id,
                            "value": value,
                        })),
                    ));
                    Ok(())
                },
            )?,
        )?;
    }

    Ok(())
}

/// ── Contribution 状态 ──：`ctx.ui.set_contribution_value(contribution_id, value)`
/// 用于更新 toggle / progress / label 等 UI contribution 的运行时状态。
fn register_contribution_value_api(
    lua: &Lua,
    ui: &Table,
    bus: &DataBus,
    source: &str,
) -> mlua::Result<()> {
    let bus_scv = bus.clone();
    let src_scv = source.to_owned();
    ui.set(
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

    Ok(())
}

/// ── 状态栏 ──：`ctx.ui.set_status(text)` — 向状态栏推送一条通知（Info 级别）
fn register_set_status_api(lua: &Lua, ui: &Table, bus: &DataBus, source: &str) -> mlua::Result<()> {
    let bus_status = bus.clone();
    let src_status = source.to_owned();
    ui.set(
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

    Ok(())
}

/// 宿主托管面板（在 `plugin.json` 中声明）的报错文案；create 与 remove 两条路径
/// 共用一处定义，避免插件可见的错误信息漂移。
fn host_managed_panel_error(panel_id: &str) -> mlua::Error {
    mlua::Error::RuntimeError(format!(
        "panel '{panel_id}' is declared in plugin.json and is managed by the host"
    ))
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
