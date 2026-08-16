//! ctx.serial.* — 串口 API（list/open/close/send/expect/read_line/write_line_and_expect）。

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use mlua::{Lua, Table, Value};
use serde_json::json;
use tool_databus::{DataBus, TopicFilter};
use tool_transport::{TransportManager, serial_topics};

use crate::convert::{json_to_lua_value, lua_value_to_serial_config};
use crate::globals::{
    CURRENT_TASK_ID, EXPECT_ACTION, EXPECT_PATTERN, PLUGIN_TASKS, TASK_YIELD_OP,
    YIELD_CONTINUE_RESETS_TIMEOUT, YIELD_DEADLINE_MS, YIELD_KIND, YIELD_PORT, YIELD_READ_LINE,
    YIELD_TIMEOUT_MS, YIELD_WRITE_LINE_AND_EXPECT,
};
use crate::host_services::{LuaHostServices, line_buffer_key};

/// 简单 Lua pattern 匹配：支持 `^` 锚点和子串匹配。
///
/// 响应模式匹配。三种形态：
///
/// - `re:<regex>`：正则匹配（Rust regex 语法）。同样先跳过行首可选的 `(...)`
///   前缀再做匹配；正则内自行用 `^`/`$` 锚定。编译结果按模式字符串缓存。
/// - `^xxx`：行首匹配。部分固件（如某些 Marlin 分支）会在每行响应前加一个
///   `(数字)` 调试/时间戳前缀（如 `(0.00000)ok*43`）。为兼容这类固件，`^` 锚定
///   会先跳过行首一个可选的 `(...)` 前缀（以 `(` 开头、`)` 结尾、内部无换行），
///   再做 starts_with。行首无 `(` 时行为不变，向后兼容。
/// - 无前缀：子串匹配（contains）。
pub(crate) fn match_pat(line: &str, pat: &str) -> bool {
    if let Some(regex) = pat.strip_prefix("re:") {
        let after_prefix = strip_leading_paren_prefix(line);
        return compile_cached_regex(regex).is_some_and(|re| re.is_match(after_prefix));
    }
    if let Some(suffix) = pat.strip_prefix('^') {
        // 跳过行首可选的 (...) 前缀，兼容带 (0.00000) 前缀的固件响应。
        let after_prefix = strip_leading_paren_prefix(line);
        after_prefix.starts_with(suffix)
    } else {
        line.contains(pat)
    }
}

/// 按模式字符串缓存编译结果；非法正则返回 None（并记录一次警告）。
fn compile_cached_regex(pattern: &str) -> Option<&'static regex::Regex> {
    use std::sync::{Mutex, OnceLock};

    static CACHE: OnceLock<
        Mutex<std::collections::HashMap<String, Option<&'static regex::Regex>>>,
    > = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut map = cache.lock().expect("regex cache poisoned");
    if let Some(entry) = map.get(pattern) {
        return *entry;
    }
    let compiled = match regex::Regex::new(pattern) {
        Ok(re) => {
            // Box::leak 返回 &'static mut，显式收窄为 &'static 共享引用后缓存。
            let leaked: &'static regex::Regex = Box::leak(Box::new(re));
            Some(leaked)
        }
        Err(e) => {
            log::warn!("serial expect: invalid regex pattern {pattern:?}: {e}");
            None
        }
    };
    map.insert(pattern.to_owned(), compiled);
    *map.get(pattern).expect("inserted entry exists")
}

/// 若 `line` 以 `(...)` 开头（`(` 到第一个 `)`，内部无换行），返回去掉该前缀
/// 后的剩余部分；否则原样返回。
fn strip_leading_paren_prefix(line: &str) -> &str {
    if line.starts_with('(')
        && let Some(close) = line.find(')')
    {
        // 前缀内不能跨行（find 到的 ) 必须在同一行内，line 本身已是单行）
        &line[close + 1..]
    } else {
        line
    }
}

/// 超时轮询串口事件直到匹配成功或超时。
/// 返回 `Some(text)` 匹配成功，`None` 表示超时/停止/断开。
fn poll_until_match(
    subscription: &tool_databus::Subscription,
    stop_flag: &Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    deadline: Instant,
    mut match_fn: impl FnMut(&tool_core::Event) -> Option<String>,
) -> Option<String> {
    loop {
        if let Some(stop) = stop_flag
            && stop.load(Ordering::Relaxed)
        {
            return None;
        }
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let remaining = deadline
            .saturating_duration_since(now)
            .min(Duration::from_millis(50));
        match subscription.recv_timeout(remaining) {
            Ok(event) => {
                if let Some(result) = match_fn(&event) {
                    return Some(result);
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return None;
            }
        }
    }
}

pub(crate) fn create_serial_api(
    lua: &Lua,
    bus: DataBus,
    transport: TransportManager,
    host_services: &LuaHostServices,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    let transport_for_list = transport.clone();

    table.set(
        "list",
        lua.create_function(move |lua, ()| {
            let ports = transport_for_list
                .list_serial_ports()
                .map_err(mlua::Error::external)?
                .into_iter()
                .map(|port| json!({ "port_name": port.port_name, "port_type": port.port_type }))
                .collect::<Vec<_>>();

            json_to_lua_value(lua, &serde_json::Value::Array(ports))
        })?,
    )?;

    let transport_for_open = transport.clone();

    table.set(
        "open",
        lua.create_function(move |_lua, config: Value| {
            transport_for_open
                .open_serial(lua_value_to_serial_config(config)?)
                .map_err(mlua::Error::external)
        })?,
    )?;

    let transport_for_close = transport.clone();

    table.set(
        "close",
        lua.create_function(move |_lua, ()| {
            transport_for_close.close_serial();
            Ok(())
        })?,
    )?;

    let transport_for_close_port = transport.clone();

    table.set(
        "close_port",
        lua.create_function(move |_lua, port: String| {
            transport_for_close_port.close_port(&port);
            Ok(())
        })?,
    )?;

    let transport_for_send_to = transport.clone();

    table.set(
        "send_to",
        lua.create_function(move |_lua, (port, text): (String, String)| {
            transport_for_send_to
                .send_text_to(&port, &text)
                .map_err(mlua::Error::external)
        })?,
    )?;

    let transport_for_send_hex_to = transport.clone();

    table.set(
        "send_hex_to",
        lua.create_function(move |_lua, (port, text): (String, String)| {
            transport_for_send_hex_to
                .send_hex_to(&port, &text)
                .map_err(mlua::Error::external)
        })?,
    )?;

    let transport_for_status_port = transport.clone();

    table.set(
        "status_port",
        lua.create_function(move |lua, port: String| {
            let status = transport_for_status_port.status_port(&port);

            json_to_lua_value(
                lua,
                &json!({
                    "open": status.open,
                    "port_name": status.port_name,
                    "baud_rate": status.baud_rate,
                }),
            )
        })?,
    )?;

    let transport_for_open_ports = transport.clone();

    table.set(
        "open_ports",
        lua.create_function(move |_lua, ()| Ok(transport_for_open_ports.open_ports()))?,
    )?;

    // ctx.serial.expect_from(port, pattern, timeout_ms) — 端口级 API
    let expect_from_bus = bus.clone();
    let expect_from_stop = host_services.stop_flag.clone();
    let expect_from_transport = transport.clone();
    table.set(
        "expect_from",
        lua.create_function(
            move |lua, (port, pattern, timeout_ms): (String, String, Option<u64>)| {
                let port = expect_from_transport
                    .canonical_open_port_name(&port)
                    .unwrap_or(port);
                let subscription =
                    expect_from_bus.subscribe(TopicFilter::exact(serial_topics::SERIAL_RX));
                let deadline = Instant::now() + Duration::from_millis(timeout_ms.unwrap_or(1_000));

                let result =
                    poll_until_match(&subscription, &expect_from_stop, deadline, |event| {
                        let event_port = event
                            .metadata
                            .get("port")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if event_port != port {
                            return None;
                        }
                        let text = event.payload.text_lossy();
                        if text.contains(&pattern) {
                            Some(text)
                        } else {
                            None
                        }
                    });
                match result {
                    Some(text) => Ok(Value::String(lua.create_string(&text)?)),
                    None => Ok(Value::Nil),
                }
            },
        )?,
    )?;

    let expect_bus = bus.clone();

    let expect_transport = transport.clone();
    let expect_stop = host_services.stop_flag.clone();
    table.set(
        "expect",
        lua.create_function(move |lua, (pattern, timeout_ms): (String, Option<u64>)| {
            // 多串口打开时拒绝不带端口的 expect，避免匹配错误端口
            let open_ports = expect_transport.open_ports();
            if open_ports.len() > 1 {
                return Err(mlua::Error::RuntimeError(
                    "多个串口已打开，请使用 ctx.serial.expect_from(port, pattern) 或 ctx.serial.request()".into(),
                ));
            }
            let subscription = expect_bus.subscribe(TopicFilter::exact(serial_topics::SERIAL_RX));
            let deadline = Instant::now() + Duration::from_millis(timeout_ms.unwrap_or(1_000));

            let result = poll_until_match(&subscription, &expect_stop, deadline, |event| {
                let text = event.payload.text_lossy();
                if text.contains(&pattern) { Some(text) } else { None }
            });
            match result {
                Some(text) => Ok(Value::String(lua.create_string(&text)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // ctx.serial.request({ port, tx, expect, timeout_ms })
    // 正确顺序：先注册 subscriber，再发送，再匹配响应（避免竞态）
    let rq_bus = bus.clone();
    let rq_transport = transport.clone();
    let rq_stop = host_services.stop_flag.clone();
    table.set(
        "request",
        lua.create_function(move |lua, opts: Table| {
            let port: String = opts.get("port")?;
            let tx: String = opts.get("tx")?;
            let expect: String = opts.get("expect")?;
            let timeout_ms: u64 = opts.get("timeout_ms").unwrap_or(1_000);

            // 1. 先注册 subscriber
            let subscription = rq_bus.subscribe(TopicFilter::exact(serial_topics::SERIAL_RX));
            let deadline = Instant::now() + Duration::from_millis(timeout_ms);

            // 2. 发送
            rq_transport
                .send_text_to(&port, &tx)
                .map_err(mlua::Error::external)?;

            // 3. 匹配响应
            let result = poll_until_match(&subscription, &rq_stop, deadline, |event| {
                let event_port = event
                    .metadata
                    .get("port")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if event_port != port {
                    return None;
                }
                let text = event.payload.text_lossy();
                if text.contains(&expect) {
                    Some(text)
                } else {
                    None
                }
            });
            match result {
                Some(text) => Ok(Value::String(lua.create_string(&text)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // ── 行缓冲区操作 ──

    // ctx.serial.flush_rx(port_name)
    let lb_flush = host_services.line_buffers.clone();
    let pid_flush = host_services.plugin_id.clone();
    let transport_flush = transport.clone();
    table.set(
        "flush_rx",
        lua.create_function(move |_lua, port_name: String| {
            let port_name = transport_flush
                .canonical_open_port_name(&port_name)
                .unwrap_or(port_name);
            if let Some(ref map) = lb_flush {
                let key = line_buffer_key(&pid_flush, &port_name);
                map.lock().remove(&key);
            }
            Ok(())
        })?,
    )?;

    // ctx.serial.write_line(port_name, line)
    let transport_wl = transport.clone();
    table.set(
        "write_line",
        lua.create_function(move |_lua, (port, line): (String, String)| {
            let port = transport_wl.canonical_open_port_name(&port).unwrap_or(port);
            let text = if line.ends_with('\n') {
                line
            } else {
                format!("{line}\n")
            };
            transport_wl
                .send_text_to(&port, &text)
                .map_err(mlua::Error::external)
        })?,
    )?;

    // ctx.serial.read_line(port_name, opts) 的 Rust begin 半边。
    let lb_read = host_services.line_buffers.clone();
    let pid_read = host_services.plugin_id.clone();
    let transport_read = transport.clone();
    table.set(
        "__read_line_begin",
        lua.create_function(move |lua, (port, opts): (String, Table)| {
            let port = transport_read
                .canonical_open_port_name(&port)
                .unwrap_or(port);
            let timeout_ms: u64 = opts.get("timeout_ms").unwrap_or(5_000);
            let delimiter: String = opts.get("delimiter").unwrap_or_else(|_| "\n".to_owned());
            if delimiter != "\n" {
                return Err(mlua::Error::RuntimeError(
                    "v0.3 只支持 delimiter=\"\\n\"".into(),
                ));
            }

            // 先检查行缓冲区是否已有数据
            let key = line_buffer_key(&pid_read, &port);
            if let Some(ref map) = lb_read
                && let Some(line) = map.lock().get_mut(&key).and_then(|b| b.next_line())
            {
                let result = lua.create_table()?;
                result.set("line", lua.create_string(&line)?)?;
                result.set("err", Value::Nil)?;
                let ready = lua.create_table()?;
                ready.set("__ready", true)?;
                ready.set("value", result)?;
                return Ok(Value::Table(ready));
            }

            // 无数据，返回 yield op，由 Lua wrapper 执行 coroutine.yield。
            let task_id: String = lua
                .globals()
                .get::<String>(CURRENT_TASK_ID)
                .unwrap_or_default();
            if task_id.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "ctx.serial.read_line 必须在 ctx.task 协程内调用".into(),
                ));
            }

            let op = lua.create_table()?;
            op.set(YIELD_KIND, YIELD_READ_LINE)?;
            op.set(YIELD_PORT, port)?;
            op.set("delimiter", delimiter)?;
            op.set(YIELD_TIMEOUT_MS, timeout_ms)?;
            op.set(
                YIELD_DEADLINE_MS,
                tool_core::now_timestamp_ms() + timeout_ms,
            )?;

            let tasks: Table = lua.globals().get(PLUGIN_TASKS)?;
            if let Ok(state) = tasks.get::<Table>(task_id.as_str()) {
                let _ = state.set(TASK_YIELD_OP, op.clone());
            }

            Ok(Value::Table(op))
        })?,
    )?;

    // ctx.serial.read_line(port_name, opts) 的 Rust finish 半边。
    table.set(
        "__read_line_finish",
        lua.create_function(move |lua, ()| {
            let task_id: String = lua
                .globals()
                .get::<String>(CURRENT_TASK_ID)
                .unwrap_or_default();
            if task_id.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "ctx.serial.read_line 必须在 ctx.task 协程内调用".into(),
                ));
            }
            // 恢复后，从 state 中读取结果。返回 table { line = ..., err = ... }
            let tasks: Table = lua.globals().get(PLUGIN_TASKS)?;
            if let Ok(state) = tasks.get::<Table>(task_id.as_str()) {
                let line: Option<String> = state.get("_read_result").ok();
                let err: Option<String> = state.get("_read_result_err").ok();
                let _ = state.set("_read_result", Value::Nil);
                let _ = state.set("_read_result_err", Value::Nil);
                let result = lua.create_table()?;
                if let Some(l) = line {
                    result.set("line", lua.create_string(&l)?)?;
                    result.set("err", Value::Nil)?;
                    return Ok(Value::Table(result));
                }
                if let Some(e) = err {
                    result.set("line", Value::Nil)?;
                    result.set("err", lua.create_string(&e)?)?;
                    return Ok(Value::Table(result));
                }
            }
            // 默认：超时
            let result = lua.create_table()?;
            result.set("line", Value::Nil)?;
            result.set("err", lua.create_string("timeout")?)?;
            Ok(Value::Table(result))
        })?,
    )?;

    // ctx.serial.write_line_and_expect(port, line, opts) 的 Rust begin 半边。
    let lb_expect = host_services.line_buffers.clone();
    let pid_expect = host_services.plugin_id.clone();
    let transport_expect = transport;
    table.set(
        "__write_line_and_expect_begin",
        lua.create_function(move |lua, (port, line, opts): (String, String, Table)| {
            let port = transport_expect
                .canonical_open_port_name(&port)
                .unwrap_or(port);
            let timeout_ms: u64 = opts.get("timeout_ms").unwrap_or(300_000);
            let continue_resets_timeout: bool =
                opts.get("continue_resets_timeout").unwrap_or(false);
            let delimiter: String = opts.get("delimiter").unwrap_or_else(|_| "\n".to_owned());
            if delimiter != "\n" {
                return Err(mlua::Error::RuntimeError(
                    "v0.3 只支持 delimiter=\"\\n\"".into(),
                ));
            }
            let patterns: Table = opts.get("patterns").unwrap_or_else(|_| {
                // 构建默认 expect pattern 表。
                let t = match lua.create_table() {
                    Ok(t) => t,
                    Err(e) => {
                        log::error!("serial expect create_table failed: {e}");
                        return lua.globals();
                    }
                };
                let entry = match lua.create_table() {
                    Ok(e) => e,
                    Err(e) => {
                        log::error!("serial expect entry create_table failed: {e}");
                        return t;
                    }
                };
                let _ = entry.set(EXPECT_PATTERN, "^ok").map_err(|e| {
                    log::error!("entry.set EXPECT_PATTERN failed: {e}");
                });
                let _ = entry.set(EXPECT_ACTION, "return").map_err(|e| {
                    log::error!("entry.set EXPECT_ACTION failed: {e}");
                });
                let _ = t.set(1, entry).map_err(|e| {
                    log::error!("t.set entry failed: {e}");
                });
                t
            });

            // 发送前清空旧缓冲，避免 stale ok/error 误匹配当前命令
            let flush_before: bool = opts.get("flush_before_send").unwrap_or(true);
            if flush_before {
                let key = line_buffer_key(&pid_expect, &port);
                if let Some(ref map) = lb_expect {
                    map.lock().remove(&key);
                }
            }

            // 发送
            let text = if line.ends_with('\n') {
                line
            } else {
                format!("{line}\n")
            };
            transport_expect
                .send_text_to(&port, &text)
                .map_err(mlua::Error::external)?;

            // 先检查缓冲区是否有立即匹配
            let key = line_buffer_key(&pid_expect, &port);
            if let Some(ref map) = lb_expect {
                let mut map_lock = map.lock();
                if let Some(buffer) = map_lock.get_mut(&key) {
                    // drain 出最多 64 行做立即匹配。未命中行会被消费，避免同一批
                    // 噪声被反复检查并挡住后续真正的 ok/ack。
                    let mut candidates: Vec<String> = Vec::new();
                    while let Some(candidate) = buffer.next_line() {
                        candidates.push(candidate);
                        if candidates.len() >= 64 {
                            break;
                        }
                    }
                    let mut matched: Option<(Table, String)> = None;
                    let mut matched_through = None;
                    for (i, candidate) in candidates.iter().enumerate() {
                        for pair in patterns.pairs::<Value, Table>().flatten() {
                            let p: Table = pair.1;
                            let pat: String = p.get(EXPECT_PATTERN).unwrap_or_default();
                            let action: String =
                                p.get(EXPECT_ACTION).unwrap_or_else(|_| "return".to_owned());
                            if match_pat(candidate, &pat) {
                                if action == "continue" {
                                    // 更新 task status 让用户看到设备忙碌
                                    let tid: String = lua
                                        .globals()
                                        .get::<String>(CURRENT_TASK_ID)
                                        .unwrap_or_default();
                                    if !tid.is_empty() {
                                        let tasks: Table = lua.globals().get(PLUGIN_TASKS)?;
                                        if let Ok(s) = tasks.get::<Table>(tid.as_str()) {
                                            let pname: String = p.get("name").unwrap_or_default();
                                            let _ = s.set(
                                                "status",
                                                format!("设备忙: {pname}: {candidate}"),
                                            );
                                        }
                                    }
                                    break;
                                }
                                matched = Some((p, candidate.clone()));
                                matched_through = Some(i + 1);
                                break;
                            }
                        }
                        if matched.is_some() {
                            break;
                        }
                    }
                    buffer.finish_expect_scan(candidates, matched_through);
                    if let Some((p, candidate)) = matched {
                        let r = lua.create_table()?;
                        r.set("name", p.get::<String>("name").unwrap_or_default())?;
                        r.set("line", candidate)?;
                        r.set("elapsed_ms", 0_u64)?;
                        let wrapper = lua.create_table()?;
                        wrapper.set("result", r)?;
                        wrapper.set("err", Value::Nil)?;
                        let ready = lua.create_table()?;
                        ready.set("__ready", true)?;
                        ready.set("value", wrapper)?;
                        return Ok(Value::Table(ready));
                    }
                }
            }

            // 无匹配，返回 yield op，由 Lua wrapper 执行 coroutine.yield。
            let task_id: String = lua
                .globals()
                .get::<String>(CURRENT_TASK_ID)
                .unwrap_or_default();
            if task_id.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "ctx.serial.write_line_and_expect 必须在 ctx.task 协程内调用".into(),
                ));
            }

            // 构造 yield_op（包含 deadline_ms 供 process_tasks 判断超时）
            let yield_data = lua.create_table()?;
            yield_data.set(YIELD_KIND, YIELD_WRITE_LINE_AND_EXPECT)?;
            yield_data.set(YIELD_PORT, port.as_str())?;
            yield_data.set("delimiter", delimiter.as_str())?;
            yield_data.set(YIELD_TIMEOUT_MS, timeout_ms)?;
            yield_data.set(YIELD_CONTINUE_RESETS_TIMEOUT, continue_resets_timeout)?;
            yield_data.set(
                YIELD_DEADLINE_MS,
                tool_core::now_timestamp_ms() + timeout_ms,
            )?;
            let _ = yield_data.set("patterns", patterns);

            let tasks: Table = lua.globals().get(PLUGIN_TASKS)?;
            if let Ok(state) = tasks.get::<Table>(task_id.as_str()) {
                let _ = state.set(TASK_YIELD_OP, yield_data.clone());
            }

            Ok(Value::Table(yield_data))
        })?,
    )?;

    // ctx.serial.write_line_and_expect(port, line, opts) 的 Rust finish 半边。
    table.set(
        "__write_line_and_expect_finish",
        lua.create_function(move |lua, ()| {
            let task_id: String = lua
                .globals()
                .get::<String>(CURRENT_TASK_ID)
                .unwrap_or_default();
            if task_id.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "ctx.serial.write_line_and_expect 必须在 ctx.task 协程内调用".into(),
                ));
            }
            // 恢复后读取结果。返回 table { result = {name,line,elapsed_ms}, err = ... }
            let tasks: Table = lua.globals().get(PLUGIN_TASKS)?;
            if let Ok(state) = tasks.get::<Table>(task_id.as_str()) {
                let matched: Option<Table> = state.get("_expect_result").ok();
                let err: Option<String> = state.get("_expect_err").ok();
                let _ = state.set("_expect_result", Value::Nil);
                let _ = state.set("_expect_err", Value::Nil);
                let wrapper = lua.create_table()?;
                if let Some(t) = matched {
                    wrapper.set("result", t)?;
                    wrapper.set("err", Value::Nil)?;
                    return Ok(Value::Table(wrapper));
                }
                if let Some(e) = err {
                    wrapper.set("result", Value::Nil)?;
                    wrapper.set("err", lua.create_string(&e)?)?;
                    return Ok(Value::Table(wrapper));
                }
            }
            let wrapper = lua.create_table()?;
            wrapper.set("result", Value::Nil)?;
            wrapper.set("err", lua.create_string("timeout")?)?;
            Ok(Value::Table(wrapper))
        })?,
    )?;

    install_serial_blocking_wrappers(lua, &table)?;

    Ok(table)
}

fn install_serial_blocking_wrappers(lua: &Lua, serial: &Table) -> mlua::Result<()> {
    let install: mlua::Function = lua
        .load(
            r#"
return function(serial)
    serial.read_line = function(port, opts)
        local op = serial.__read_line_begin(port, opts or {})
        if op and op.__ready then
            return op.value
        end
        coroutine.yield(op)
        return serial.__read_line_finish()
    end

    serial.write_line_and_expect = function(port, line, opts)
        local op = serial.__write_line_and_expect_begin(port, line, opts or {})
        if op and op.__ready then
            return op.value
        end
        coroutine.yield(op)
        return serial.__write_line_and_expect_finish()
    end
end
"#,
        )
        .set_name("serial-blocking-wrappers")
        .eval()?;
    install.call::<()>(serial.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_pat_anchor_matches_plain_line() {
        // 行首无 (...) 前缀：行为与原 starts_with 一致
        assert!(match_pat("ok*43", "^ok"));
        assert!(match_pat("ok", "^ok"));
        assert!(!match_pat("rookie", "^ok"));
        assert!(!match_pat("Bed OK", "^ok"));
    }

    #[test]
    fn match_pat_anchor_skips_paren_prefix() {
        // 部分 Marlin 分支给每行加 (数字) 前缀，^ 锚定应跳过它
        assert!(match_pat("(0.00000)ok*43", "^ok"));
        assert!(match_pat("(2.00000)Resend: N5", "^Resend:"));
        assert!(match_pat("(1.00000)Error:Printer halted", "^Error"));
        assert!(match_pat("(0.00000)OK", "^OK"));
    }

    #[test]
    fn match_pat_anchor_with_paren_prefix_still_precise() {
        // 跳过前缀后仍需精确 starts_with，避免误匹配
        assert!(!match_pat("(0.00000)rookie", "^ok"));
        assert!(!match_pat("(0.00000)Bed OK", "^ok"));
    }

    #[test]
    fn match_pat_substring_unaffected() {
        // 无 ^ 的 pattern 走 contains，不受前缀跳过影响
        assert!(match_pat("(0.00000)echo:busy: processing", "busy"));
        assert!(!match_pat("(0.00000)ok", "resend"));
    }

    #[test]
    fn match_pat_paren_prefix_no_close_is_left_as_is() {
        // 行首 ( 但无 )：不当作前缀，原样 starts_with（必然不匹配 ^ok）
        assert!(!match_pat("(unclosed ok", "^ok"));
    }

    #[test]
    fn match_pat_regex_prefix() {
        // re: 前缀走正则（Rust regex 语法），自动跳过 (...) 前缀后匹配
        assert!(match_pat("done", "re:^done$"));
        assert!(!match_pat("DONE", "re:^done$")); // regex 默认区分大小写
        assert!(match_pat("(0.00000)done", "re:^done$"));
        assert!(match_pat("M105 ok 12.3", "re:\\d+\\.\\d+"));
        assert!(!match_pat("M105 ok 12.3", "re:^error"));
        assert!(!match_pat("rookie ok", "re:^ok"));
    }

    #[test]
    fn match_pat_regex_skips_paren_prefix() {
        assert!(match_pat("(1.234)ok", "re:^ok"));
        assert!(!match_pat("(1.234)rookie", "re:^ok"));
        assert!(match_pat("(1.234)Error:Line Number", "re:^Error"));
    }

    #[test]
    fn match_pat_invalid_regex_returns_false() {
        // 非法正则不 panic，按不匹配处理
        assert!(!match_pat("anything", "re:["));
    }

    #[test]
    fn match_pat_regex_ignore_case_flag() {
        // (?i) 内联标志可用
        assert!(match_pat("DONE", "re:(?i)^done$"));
    }
}
