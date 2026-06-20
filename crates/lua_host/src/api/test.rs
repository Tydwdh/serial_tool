//! `ctx.test.*` — 测试框架 API（case / assert / raw_packets / latest_event_id / publish_report）。

use mlua::{Lua, Table, Value};
use tool_core::{Direction, Event, Payload};
use tool_databus::DataBus;
use tool_testing::TestPacketLog;
use tool_transport::serial_topics;

use crate::convert::{json_to_lua_value, lua_value_to_json};

pub(crate) fn install_test_api(
    lua: &Lua,
    ctx: &Table,
    bus: DataBus,
    config: &crate::LuaRunConfig,
) -> mlua::Result<()> {
    let host = lua.create_table()?;

    let bus_for_latest = bus.clone();
    let bus_for_packets = bus.clone();
    let bus_for_publish = bus.clone();

    let run_started_ms = tool_core::now_timestamp_ms();
    let run_id = format!("{}-{run_started_ms}", config.script_name);
    let source = config.source.clone();

    host.set("run_id", run_id.clone())?;
    host.set("source", config.source.clone())?;
    host.set("script_name", config.script_name.clone())?;
    host.set("run_started_ms", run_started_ms)?;

    host.set(
        "now_ms",
        lua.create_function(|_lua, ()| Ok(tool_core::now_timestamp_ms()))?,
    )?;

    host.set(
        "latest_event_id",
        lua.create_function(move |_lua, ()| {
            Ok(bus_for_latest
                .history()
                .into_iter()
                .map(|event| event.id)
                .max()
                .unwrap_or_default())
        })?,
    )?;

    host.set(
        "raw_packets_since",
        lua.create_function(move |lua, start_id: u64| {
            let packets = bus_for_packets
                .history()
                .into_iter()
                .filter(|event| {
                    event.id > start_id
                        && matches!(
                            event.topic.as_str(),
                            serial_topics::SERIAL_RX | serial_topics::SERIAL_TX
                        )
                })
                .map(test_packet_from_event)
                .collect::<Vec<_>>();

            json_to_lua_value(
                lua,
                &serde_json::to_value(packets).map_err(mlua::Error::external)?,
            )
        })?,
    )?;

    host.set(
        "publish_report",
        lua.create_function(move |_lua, report: Value| {
            bus_for_publish.publish(Event::new(
                tool_core::topics::TEST_RESULT,
                source.clone(),
                Direction::Internal,
                Payload::Json(lua_value_to_json(report)?),
            ));

            Ok(())
        })?,
    )?;

    lua.globals().set("__test_host", host)?;

    lua.load(crate::TEST_BOOTSTRAP)
        .set_name("test-bootstrap")
        .exec()?;

    let test: Table = lua.globals().get("test")?;
    ctx.set("test", test)?;

    Ok(())
}

pub(crate) fn test_packet_from_event(event: Event) -> TestPacketLog {
    let payload_text = event.payload.text_lossy();

    let payload_hex = event
        .payload
        .as_bytes()
        .map(|bytes| {
            bytes
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    TestPacketLog {
        id: event.id,
        timestamp_ms: event.timestamp_ms,
        topic: event.topic,
        direction: event.direction,
        payload_text,
        payload_hex,
    }
}
