//! `ctx.bus.*` — 总线 API（publish / history / wait / subscribe / on / off）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mlua::{Function, Lua, Table, Value};
use serde_json::json;

use tool_core::{Direction, Event};
use tool_databus::{DataBus, TopicFilter};

use crate::convert::{
    event_to_lua_table, json_to_lua_value, lua_value_to_payload, payload_to_json,
};

pub(crate) fn create_bus_api(
    lua: &Lua,
    bus: DataBus,
    source: String,
    stop_flag: Option<Arc<AtomicBool>>,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    let publish_bus = bus.clone();

    table.set(
        "publish",
        lua.create_function(move |_lua, (topic, payload): (String, Value)| {
            // 阻止插件通过裸 bus 发布保留前缀，应使用 ctx.ui.* / ctx.serial.*
            if topic.starts_with("ui.")
                || topic.starts_with("transport.")
                || topic.starts_with("log.")
            {
                return Err(mlua::Error::RuntimeError(format!(
                    "bus.publish: topic '{topic}' 是保留前缀，请使用 ctx.ui.* / ctx.serial.* 等专用 API"
                )));
            }
            let payload = lua_value_to_payload(payload)?;

            publish_bus.publish(Event::new(
                topic,
                source.clone(),
                Direction::Internal,
                payload,
            ));

            Ok(())
        })?,
    )?;

    let history_bus = bus.clone();

    table.set(
        "history",
        lua.create_function(move |lua, topic_prefix: Option<String>| {
            let events = history_bus
                .history()
                .into_iter()
                .filter(|event| {
                    topic_prefix
                        .as_ref()
                        .map(|prefix| event.topic.starts_with(prefix))
                        .unwrap_or(true)
                })
                .rev()
                .take(100)
                .map(|event| {
                    json!({
                        "id": event.id,
                        "timestamp_ms": event.timestamp_ms,
                        "topic": event.topic,
                        "source": event.source,
                        "direction": format!("{:?}", event.direction).to_lowercase(),
                        "payload": payload_to_json(event.payload),
                    })
                })
                .collect::<Vec<_>>();

            json_to_lua_value(lua, &serde_json::Value::Array(events))
        })?,
    )?;

    let wait_bus = bus.clone();
    let wait_stop = stop_flag.clone();

    table.set(
        "wait",
        lua.create_function(move |lua, (topic, timeout_ms): (String, Option<u64>)| {
            wait_for_event(
                lua,
                wait_bus.clone(),
                TopicFilter::exact(topic),
                timeout_ms,
                wait_stop.clone(),
            )
        })?,
    )?;

    let subscribe_bus = bus.clone();
    let sub_stop = stop_flag;

    table.set(
        "subscribe",
        lua.create_function(
            move |lua, (topic_prefix, timeout_ms): (String, Option<u64>)| {
                wait_for_event(
                    lua,
                    subscribe_bus.clone(),
                    TopicFilter::prefix(topic_prefix),
                    timeout_ms,
                    sub_stop.clone(),
                )
            },
        )?,
    )?;

    table.set(
        "on",
        lua.create_function(move |lua, (topic, callback): (String, Function)| {
            let callbacks: Table = lua.globals().get(crate::globals::PLUGIN_CALLBACKS)?;
            callbacks.set(topic, callback)?;
            Ok(())
        })?,
    )?;

    table.set(
        "off",
        lua.create_function(move |lua, topic: String| {
            let callbacks: Table = lua.globals().get(crate::globals::PLUGIN_CALLBACKS)?;
            callbacks.set(topic, Value::Nil)?;
            Ok(())
        })?,
    )?;

    Ok(table)
}

pub(crate) fn wait_for_event(
    lua: &Lua,
    bus: DataBus,
    filter: TopicFilter,
    timeout_ms: Option<u64>,
    stop_flag: Option<Arc<AtomicBool>>,
) -> mlua::Result<Value> {
    let subscription = bus.subscribe(filter);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.unwrap_or(1_000));

    loop {
        if let Some(ref stop) = stop_flag
            && stop.load(Ordering::Relaxed)
        {
            return Ok(Value::Nil);
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(Value::Nil);
        }
        let remaining = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(50));
        match subscription.recv_timeout(remaining) {
            Ok(event) => {
                let table = lua.create_table()?;
                event_to_lua_table(lua, &table, &event)?;
                return Ok(Value::Table(table));
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return Ok(Value::Nil),
        }
    }
}
