//! 两端 Lua 插件 API 的差分测试：同一份脚本分别喂给 native(mlua) 与 web(omnilua)。
//!
//! 为什么要这一份：插件作者只写一个 `main.lua`，同一个 `ctx.*` 调用却由两份独立
//! 实现的注册面服务（`crates/lua_host/src/api/*` 与 `crates/plugin_runtime/src/web_lua.rs`）。
//! 两边各写一遍的面历史上反复出事，而在此之前没有任何一条用例把"同一份脚本在两端
//! 得出同样的结果"钉住。这里只钉**实测确实一致**的那部分；两端不一致的项一律不写成
//! 用例（那会把分叉固化成契约），它们记在交付报告里等产品拍板。
//!
//! 判定方式是"脚本自己算出期望值"：每条片段把算出的标量发到 `READBACK_TOPIC`，
//! Rust 侧只读那一条主题并比对常量。片段不碰串口、时间戳、墙钟，所以结果只取决于
//! 注册面本身。
//!
//! 故意不进这张表的面：`ctx.task.*` 要有人驱动协程调度器，`ctx.config` / `ctx.dialog`
//! / `ctx.fs` 在 native 的一次性测试服务下压根不会被安装（那些宿主服务全是 `None`），
//! 喂个假应答只能让两端"一致地什么也没做"。这类要么补真宿主、要么就是分叉，不该由
//! 差分表代答。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use tool_databus::{DataBus, TopicFilter};
use tool_lua_host::run_script_for_test;
use tool_plugin_api::{
    LuaEngine, PluginCapability, PluginError, PluginHostApi, PluginHostRequest, PluginLoadConfig,
    PluginPermissions, PluginValue,
};
use tool_plugin_runtime::WebLuaEngine;
use tool_transport::TransportManager;

/// 脚本把算出来的值发布到这条主题；宿主侧只读它，其余事件一律忽略。
///
/// 刻意不带 `ui.` / `transport.` / `log.` 前缀 —— native 的 `ctx.bus.publish`
/// 会拒绝这三个保留前缀，换成它们的话片段会在两端走出不同的错误路径。
const READBACK_TOPIC: &str = "parity.lua_api.readback";

/// 一条差分用例：`(用例名, 脚本, 两端都必须得出的值)`。
type Case = (&'static str, &'static str, &'static str);

/// native 侧的 `run_script_for_test` 走 `LuaRunConfig::default()`，权限就是
/// `bus/log/serial/ui/storage/timer/testing`；web 侧必须授予同一批能力，
/// 否则两端安装的 `ctx.*` 域不同，比的就不是同一条面了。
const WEB_CAPABILITIES: &[PluginCapability] = &[
    PluginCapability::Bus,
    PluginCapability::Log,
    PluginCapability::Serial,
    PluginCapability::Storage,
    PluginCapability::Timer,
    PluginCapability::Ui,
];

/// 把脚本要发的主题名注入进去，避免这条字面量在 Rust 与每条 Lua 片段里各写一份。
fn script_with_topic(body: &str) -> String {
    format!("local TOPIC = {READBACK_TOPIC:?}\n{body}")
}

/// 跑 native(mlua) 引擎，交出脚本在 `READBACK_TOPIC` 上发布的那一个值。
fn native_readback(body: &str, case: &str) -> String {
    let bus = DataBus::new();
    let transport = TransportManager::new(bus.clone());
    let readback = bus.subscribe(TopicFilter::exact(READBACK_TOPIC));

    run_script_for_test(&script_with_topic(body), bus, transport)
        .unwrap_or_else(|error| panic!("用例 {case}：native 引擎跑脚本失败：{error}"));

    let events = readback.drain();
    let [event] = events.as_slice() else {
        panic!(
            "用例 {case}：native 引擎应在 {READBACK_TOPIC} 上发布恰好一条事件，实际 {} 条",
            events.len()
        );
    };
    event.payload.text_lossy()
}

/// 跑 web(omnilua) 引擎：脚本在 `load_plugin` 里作为插件正文顶层执行，
/// 宿主替身记录下的 `ctx.bus.publish` 就是要比对的值。
fn web_readback(body: &str, case: &str) -> String {
    let host = Rc::new(ParityHost::default());
    let mut engine = WebLuaEngine::new();
    engine
        .load_plugin(
            &script_with_topic(body),
            ParityHost::load_config(),
            host.clone(),
        )
        .unwrap_or_else(|error| panic!("用例 {case}：web 引擎跑脚本失败：{error}"));

    let published = host.published.borrow();
    let matching = published
        .iter()
        .filter(|(topic, _)| topic == READBACK_TOPIC)
        .collect::<Vec<_>>();
    let [(_, value)] = matching.as_slice() else {
        panic!(
            "用例 {case}：web 引擎应在 {READBACK_TOPIC} 上发布恰好一条事件，实际 {} 条（全部发布：{published:?}）",
            matching.len()
        );
    };
    match value {
        PluginValue::String(text) => text.clone(),
        other => panic!("用例 {case}：web 引擎发布的不是字符串，而是 {other:?}"),
    }
}

/// 差分判定：native 与 web 得出同一个值，且那个值就是用例声明的期望值。
///
/// 两个条件都要，缺一不可 —— 只比 `native == web` 的话，两端同时跑偏也会绿；
/// 只比期望值的话，两端各自跑偏到不同值时只会红一次，说不清是谁偏的。
///
/// 失败一次性收集完再报：差分测试要的产物是"哪几条分叉了"的整张表，第一条就 panic
/// 的话，每次改动都得重跑一轮才知道还有没有下一条。
fn assert_parity(cases: &[Case]) {
    let mut failures = Vec::new();
    for (case, body, expected) in cases {
        let native = native_readback(body, case);
        let web = web_readback(body, case);
        if native != web {
            failures.push(format!(
                "用例 {case} 两端不一致：native 得出 {native:?}，web 得出 {web:?}"
            ));
        } else if native != *expected {
            failures.push(format!(
                "用例 {case} 两端一致，但得出的不是声明的期望值：{native:?} != {expected:?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} 条差分用例没有交出同一个值：\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn bus_api_scripts_agree_on_native_and_web() {
    assert_parity(&[
        (
            "bus_publish_forwards_computed_string",
            r#"ctx.bus.publish(TOPIC, string.rep("ab", 3))"#,
            "ababab",
        ),
        (
            "bus_exposes_the_same_function_names",
            r#"ctx.bus.publish(TOPIC, table.concat({
                type(ctx.bus.publish), type(ctx.bus.history), type(ctx.bus.on),
                type(ctx.bus.off), type(ctx.bus.wait), type(ctx.bus.subscribe)
            }))"#,
            "functionfunctionfunctionfunctionfunctionfunction",
        ),
    ]);
}

#[test]
fn session_api_scripts_agree_on_native_and_web() {
    assert_parity(&[
        (
            "session_round_trips_a_stored_value",
            r#"ctx.session.set("k", string.rep("z", 4))
            ctx.bus.publish(TOPIC, tostring(#ctx.session.get("k")))"#,
            "4",
        ),
        (
            "session_missing_key_is_nil",
            r#"ctx.bus.publish(TOPIC, tostring(ctx.session.get("absent")))"#,
            "nil",
        ),
        (
            "session_keys_counts_written_entries",
            r#"ctx.session.set("a", "1")
            ctx.session.set("b", "2")
            ctx.bus.publish(TOPIC, tostring(#ctx.session.keys()))"#,
            "2",
        ),
    ]);
}

#[test]
fn command_api_scripts_agree_on_native_and_web() {
    assert_parity(&[
        (
            "commands_list_counts_registrations",
            r#"ctx.commands.register("a", function() end)
            ctx.commands.register("b", function() end)
            ctx.bus.publish(TOPIC, tostring(#ctx.commands.list()))"#,
            "2",
        ),
        (
            "commands_unregister_shrinks_list",
            r#"ctx.commands.register("a", function() end)
            ctx.commands.register("b", function() end)
            ctx.commands.unregister("a")
            ctx.bus.publish(TOPIC, tostring(#ctx.commands.list()))"#,
            "1",
        ),
        (
            "commands_execute_without_id_is_an_error",
            r#"ctx.bus.publish(TOPIC, tostring(pcall(ctx.commands.execute)))"#,
            "false",
        ),
    ]);
}

#[test]
fn other_plugin_facing_surfaces_agree_on_native_and_web() {
    assert_parity(&[
        (
            "codec_module_require_and_xor8_fold",
            r#"local codec = require("hw.codec")
            ctx.bus.publish(TOPIC, string.format(
                "%d/%d", codec.xor8("AB"), codec.xor8("ABCDE")))"#,
            "3/65",
        ),
        (
            "ui_exposes_the_same_function_names",
            r#"ctx.bus.publish(TOPIC, table.concat({
                type(ctx.ui.create_chart), type(ctx.ui.create_form), type(ctx.ui.create_gauge),
                type(ctx.ui.create_attitude), type(ctx.ui.create_table), type(ctx.ui.remove_panel),
                type(ctx.ui.set_value), type(ctx.ui.set_values), type(ctx.ui.set_enabled),
                type(ctx.ui.set_visible), type(ctx.ui.table_set_rows), type(ctx.ui.table_append_rows),
                type(ctx.ui.table_remove_rows), type(ctx.ui.table_clear),
                type(ctx.ui.set_contribution_value), type(ctx.ui.set_status), type(ctx.ui.get_panel)
            }))"#,
            "functionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunction",
        ),
        (
            "log_timer_command_expose_the_same_function_names",
            r#"ctx.bus.publish(TOPIC, table.concat({
                type(ctx.log.trace), type(ctx.log.debug), type(ctx.log.info),
                type(ctx.log.warn), type(ctx.log.error),
                type(ctx.timer.after), type(ctx.timer.every), type(ctx.timer.cancel),
                type(ctx.commands.register), type(ctx.commands.unregister),
                type(ctx.commands.list), type(ctx.commands.execute)
            }))"#,
            "functionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunction",
        ),
        (
            "serial_exposes_the_same_function_names",
            r#"ctx.bus.publish(TOPIC, table.concat({
                type(ctx.serial.list), type(ctx.serial.devices), type(ctx.serial.open),
                type(ctx.serial.close), type(ctx.serial.open_ports), type(ctx.serial.close_port),
                type(ctx.serial.send_to), type(ctx.serial.send_hex_to),
                type(ctx.serial.status_port), type(ctx.serial.write_line),
                type(ctx.serial.flush_rx), type(ctx.serial.read_line),
                type(ctx.serial.expect), type(ctx.serial.expect_from),
                type(ctx.serial.request), type(ctx.serial.write_line_and_expect)
            }))"#,
            "functionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunctionfunction",
        ),
    ]);
}

/// 宿主替身：把 `ctx.bus.publish` 记下来当差分读数，会话存储照现有实现接，
/// 时钟给一个常量（差分片段不许读墙钟，给常量是让"读了墙钟"立刻可见），其余报"不支持"。
///
/// 故意不给 config/dialog/fs 等应答：那些域在两端要么需要真实宿主、要么在
/// native 的一次性测试服务下根本不存在，喂一个假应答只会把差异盖住。
#[derive(Default)]
struct ParityHost {
    published: RefCell<Vec<(String, PluginValue)>>,
    storage: RefCell<BTreeMap<String, PluginValue>>,
}

impl ParityHost {
    fn load_config() -> PluginLoadConfig {
        PluginLoadConfig {
            plugin_id: "parity.lua-api".to_owned(),
            plugin_name: "Lua API Parity".to_owned(),
            plugin_version: "1.0.0".to_owned(),
            script_name: "parity.lua".to_owned(),
            context: PluginValue::Null,
            permissions: PluginPermissions::new(WEB_CAPABILITIES.iter().copied()),
        }
    }
}

impl PluginHostApi for ParityHost {
    fn request(&self, request: PluginHostRequest) -> Result<PluginValue, PluginError> {
        match request {
            PluginHostRequest::NowMs => Ok(PluginValue::Integer(1_700)),
            PluginHostRequest::Log { .. } => Ok(PluginValue::Null),
            PluginHostRequest::BusPublish { topic, value } => {
                self.published.borrow_mut().push((topic, value));
                Ok(PluginValue::Null)
            }
            PluginHostRequest::StorageGet { key } => Ok(self
                .storage
                .borrow()
                .get(&key)
                .cloned()
                .unwrap_or(PluginValue::Null)),
            PluginHostRequest::StorageSet { key, value } => {
                self.storage.borrow_mut().insert(key, value);
                Ok(PluginValue::Null)
            }
            PluginHostRequest::StorageDelete { key } => {
                self.storage.borrow_mut().remove(&key);
                Ok(PluginValue::Null)
            }
            PluginHostRequest::StorageKeys => Ok(PluginValue::Array(
                self.storage
                    .borrow()
                    .keys()
                    .cloned()
                    .map(PluginValue::String)
                    .collect(),
            )),
            other => Err(PluginError::UnsupportedCapability(format!(
                "parity 宿主替身不支持 {other:?}"
            ))),
        }
    }
}
