//! ctx.task.* — Task API（start/cancel/pause/resume/list）+ task coroutine 调度。

use crate::LuaRunConfig;
use crate::api::serial::match_pat;
use crate::globals::{
    CURRENT_TASK_ID, PLUGIN_DISABLE, PLUGIN_TASKS, TASK_CANCELLED, TASK_FINISHED, TASK_YIELD_OP,
    YIELD_DEADLINE_MS, YIELD_KIND, YIELD_PORT, YIELD_READ_LINE, YIELD_SLEEP, YIELD_WAIT_PAUSED,
    YIELD_WRITE_LINE_AND_EXPECT,
};
use crate::host_services::{LuaHostServices, line_buffer_key};
use mlua::{Function, Lua, Table, Thread, Value};
use tool_core::{Event, LogLevel};
use tool_databus::DataBus;

const MAX_TASK_RESUMES_PER_TICK: usize = 50;

/// 注入 task 辅助函数（供 coroutine 内调用 yield）
pub(crate) fn install_task_helpers(lua: &Lua) -> mlua::Result<()> {
    lua.load(
        r#"
    -- 内部 yield 辅助：所有阻塞操作都通过纯 Lua wrapper yield。
    function __task_yield(yield_op)
        return coroutine.yield(yield_op)
    end
"#,
    )
    .set_name("task-helpers")
    .exec()?;
    Ok(())
}

/// 每帧恢复可运行的 task coroutine。
pub(crate) fn process_tasks(
    lua: &Lua,
    _bus: &DataBus,
    _config: &LuaRunConfig,
    host_services: &LuaHostServices,
) {
    let tasks: Table = match lua.globals().get(PLUGIN_TASKS) {
        Ok(t) => t,
        Err(_) => return,
    };

    let now_ms = tool_core::now_timestamp_ms();
    let mut resume_count = 0usize;

    // 先收集需要恢复的 task id
    let mut ready_ids: Vec<String> = Vec::new();
    for (id, state) in tasks.pairs::<String, Table>().flatten() {
        if state.get::<bool>(TASK_FINISHED).unwrap_or(true) {
            continue;
        }

        // ── cancelled 优先：打断 sleep/read_line/expect/paused 等一切等待 ──
        let cancelled: bool = state.get(TASK_CANCELLED).unwrap_or(false);
        if cancelled {
            let yield_op: Option<Table> = state.get(TASK_YIELD_OP).ok();
            if let Some(ref op) = yield_op {
                let kind: String = op.get(YIELD_KIND).unwrap_or_default();
                match kind.as_str() {
                    YIELD_READ_LINE => {
                        let _ = state.set("_read_result", Value::Nil);
                        let _ = state.set(
                            "_read_result_err",
                            lua.create_string("cancelled")
                                .map(Value::String)
                                .unwrap_or(Value::Nil),
                        );
                    }
                    YIELD_WRITE_LINE_AND_EXPECT => {
                        let _ = state.set("_expect_result", Value::Nil);
                        let _ = state.set(
                            "_expect_err",
                            lua.create_string("cancelled")
                                .map(Value::String)
                                .unwrap_or(Value::Nil),
                        );
                    }
                    _ => {
                        // sleep / wait_paused / unknown: 直接恢复
                    }
                }
            }
            ready_ids.push(id);
            continue;
        }

        // ── 非 cancelled：正常调度 ──
        let paused: bool = state.get("paused").unwrap_or(false);
        if paused {
            continue;
        }

        let yield_op: Option<Table> = state.get(TASK_YIELD_OP).ok();
        if let Some(ref op) = yield_op {
            let kind: String = op.get(YIELD_KIND).unwrap_or_default();
            match kind.as_str() {
                YIELD_SLEEP => {
                    let wake_at_ms: u64 = state.get("wake_at_ms").unwrap_or(0);
                    if now_ms < wake_at_ms {
                        continue;
                    }
                }
                YIELD_WAIT_PAUSED => {
                    continue;
                }
                YIELD_READ_LINE => {
                    let port: String = op.get(YIELD_PORT).unwrap_or_default();
                    let deadline_ms: u64 = op.get(YIELD_DEADLINE_MS).unwrap_or(0);
                    if deadline_ms > 0 && now_ms > deadline_ms {
                        let _ = state.set("_read_result", Value::Nil);
                        let _ = state.set(
                            "_read_result_err",
                            lua.create_string("timeout")
                                .map(Value::String)
                                .unwrap_or(Value::Nil),
                        );
                    } else if let Some(ref map) = host_services.line_buffers {
                        let key = line_buffer_key(&host_services.plugin_id, &port);
                        let mut map_lock = map.lock();
                        if let Some(buffer) = map_lock.get_mut(&key) {
                            if let Some(line) = buffer.next_line() {
                                let _ = state.set(
                                    "_read_result",
                                    lua.create_string(&line)
                                        .map(Value::String)
                                        .unwrap_or(Value::Nil),
                                );
                                let _ = state.set("_read_result_err", Value::Nil);
                            } else {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
                }
                YIELD_WRITE_LINE_AND_EXPECT => {
                    let port: String = op.get(YIELD_PORT).unwrap_or_default();
                    let deadline_ms: u64 = op.get(YIELD_DEADLINE_MS).unwrap_or(0);
                    if deadline_ms > 0 && now_ms > deadline_ms {
                        let _ = state.set("_expect_result", Value::Nil);
                        let _ = state.set(
                            "_expect_err",
                            lua.create_string("timeout")
                                .map(Value::String)
                                .unwrap_or(Value::Nil),
                        );
                    } else if let Some(ref map) = host_services.line_buffers {
                        let key = line_buffer_key(&host_services.plugin_id, &port);
                        // 在锁内只收集行，释放锁后再做 Lua 匹配（避免死锁）
                        let lines: Vec<String> = {
                            let mut map_lock = map.lock();
                            if let Some(buffer) = map_lock.get_mut(&key) {
                                let mut buf = Vec::new();
                                while let Some(line) = buffer.next_line() {
                                    buf.push(line);
                                    // 最多取 64 行避免无限循环
                                    if buf.len() >= 64 {
                                        break;
                                    }
                                }
                                buf
                            } else {
                                continue;
                            }
                        };
                        // 遍历匹配。未命中的候选行属于本次 expect 的无关输出，扫描后
                        // 必须消费；否则 64 行以上的噪声会被反复回灌并永久挡住后续 ACK。
                        // 命中 return 时，只保留命中行之后尚未检查的尾部。
                        let mut matched = None;
                        let mut matched_through = None;
                        for (i, line) in lines.iter().enumerate() {
                            let patterns: Option<Table> = op.get("patterns").ok();
                            if let Some(ref pts) = patterns {
                                for pair in pts.pairs::<Value, Table>().flatten() {
                                    let p: Table = pair.1;
                                    let pat: String = p.get("pattern").unwrap_or_default();
                                    let action: String =
                                        p.get("action").unwrap_or_else(|_| "return".to_owned());
                                    let pname: String = p.get("name").unwrap_or_default();
                                    let hit = match_pat(line, &pat);
                                    if hit {
                                        if action == "continue" {
                                            let _ = state
                                                .set("status", format!("设备忙: {pname}: {line}"));
                                            break;
                                        }
                                        matched = Some((
                                            p.get::<String>("name").unwrap_or_default(),
                                            line.clone(),
                                        ));
                                        matched_through = Some(i + 1);
                                        break;
                                    }
                                }
                            }
                            if matched.is_some() {
                                break;
                            }
                        }
                        {
                            let mut map_lock = map.lock();
                            if let Some(buffer) = map_lock.get_mut(&key) {
                                buffer.finish_expect_scan(lines, matched_through);
                            }
                        }
                        if let Some((name, line)) = matched {
                            let result = lua.create_table().ok();
                            if let Some(ref r) = result {
                                let _ = r.set("name", name.as_str());
                                let _ = r.set("line", line.as_str());
                                let _ = r.set("elapsed_ms", 0_u64);
                            }
                            let _ = state.set(
                                "_expect_result",
                                result.map(Value::Table).unwrap_or(Value::Nil),
                            );
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
                }
                _ => {
                    continue;
                }
            }
        }
        ready_ids.push(id);
    }

    for id in &ready_ids {
        if resume_count >= MAX_TASK_RESUMES_PER_TICK {
            break;
        }

        let state: Table = match tasks.get(id.as_str()) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let thread: Thread = match state.get("thread") {
            Ok(t) => t,
            Err(_) => continue,
        };

        // 清除 yield_op 标记
        let _ = state.set(TASK_YIELD_OP, Value::Nil);

        // 设置当前 task id，供 read_line 等函数使用
        let _ = lua.globals().set(CURRENT_TASK_ID, id.as_str());

        // resume coroutine
        match thread.resume::<Value>(()) {
            Ok(_values) => {
                // coroutine 可能 yield 了或返回了，检查状态
                match thread.status() {
                    mlua::ThreadStatus::Resumable => {
                        // coroutine yielded，yield_op 已在 Lua 侧设置
                    }
                    _ => {
                        // Finished 或 Error — 标记完成
                        let _ = state.set(TASK_FINISHED, true);
                    }
                }
            }
            Err(e) => {
                // CoroutineUnresumable — 记录错误并标记完成
                let _ = state.set(TASK_FINISHED, true);
                let _ = state.set("last_error", lua.create_string(e.to_string()).ok());
                _bus.publish(Event::system_log(
                    LogLevel::Error,
                    &_config.source,
                    format!("任务 '{id}' 失败：{e}"),
                ));
            }
        }

        resume_count += 1;
    }

    // 清除当前 task id
    let _ = lua.globals().set(CURRENT_TASK_ID, Value::Nil);
}

/// 从 task 对象获取 state table
fn get_state_for_task(lua: &Lua, task: &Table) -> mlua::Result<Table> {
    let id: String = task.get("id")?;
    let tasks: Table = lua.globals().get(PLUGIN_TASKS)?;
    tasks.get(id.as_str())
}

/// 注入 task 对象方法，每个方法通过 task.id 查找 __plugin_tasks 中的实际 state
fn create_task_methods_table(lua: &Lua) -> mlua::Result<Table> {
    let tbl = lua.create_table()?;

    // task:is_cancelled() → bool
    tbl.set(
        "is_cancelled",
        lua.create_function(|lua, task: Table| {
            let state = get_state_for_task(lua, &task)?;
            Ok(state.get::<bool>(TASK_CANCELLED).unwrap_or(false))
        })?,
    )?;

    // task:is_paused() → bool
    tbl.set(
        "is_paused",
        lua.create_function(|lua, task: Table| {
            let state = get_state_for_task(lua, &task)?;
            Ok(state.get::<bool>("paused").unwrap_or(false))
        })?,
    )?;

    // task:sleep_ms(ms) 的 Rust 半边：设置状态并返回 yield op。
    tbl.set(
        "__sleep_ms_begin",
        lua.create_function(|lua, (task, ms): (Table, u64)| {
            let state = get_state_for_task(lua, &task)?;
            let _ = state.set("wake_at_ms", tool_core::now_timestamp_ms() + ms);
            let op = lua.create_table()?;
            op.set(YIELD_KIND, YIELD_SLEEP)?;
            op.set("ms", ms)?;
            let _ = state.set(TASK_YIELD_OP, op.clone());
            Ok(Value::Table(op))
        })?,
    )?;

    // task:wait_if_paused() 的 Rust 半边：暂停时返回 yield op。
    tbl.set(
        "__wait_if_paused_begin",
        lua.create_function(|lua, task: Table| {
            let state = get_state_for_task(lua, &task)?;
            if !state.get::<bool>("paused").unwrap_or(false) {
                return Ok(Value::Nil);
            }
            let op = lua.create_table()?;
            op.set(YIELD_KIND, YIELD_WAIT_PAUSED)?;
            let _ = state.set(TASK_YIELD_OP, op.clone());
            Ok(Value::Table(op))
        })?,
    )?;

    // task:set_progress(current, total)
    tbl.set(
        "set_progress",
        lua.create_function(|lua, (task, current, total): (Table, u64, u64)| {
            let state = get_state_for_task(lua, &task)?;
            let _ = state.set("progress_current", current);
            let _ = state.set("progress_total", total);
            Ok(())
        })?,
    )?;

    // task:set_progress_percent(percent)
    tbl.set(
        "set_progress_percent",
        lua.create_function(|lua, (task, percent): (Table, f64)| {
            let state = get_state_for_task(lua, &task)?;
            let _ = state.set("progress_percent", percent.clamp(0.0, 100.0));
            Ok(())
        })?,
    )?;

    // task:set_status(text)
    tbl.set(
        "set_status",
        lua.create_function(|lua, (task, text): (Table, String)| {
            let state = get_state_for_task(lua, &task)?;
            let _ = state.set("status", text);
            Ok(())
        })?,
    )?;

    // task:log(level, message)
    tbl.set(
        "log",
        lua.create_function(|lua, (task, level, message): (Table, String, String)| {
            let state = get_state_for_task(lua, &task)?;
            let logs: Table = state.get("logs").unwrap_or_else(|_| {
                lua.create_table().unwrap_or_else(|e| {
                    log::error!("task:log create_table failed: {e}; log entries will be lost");
                    // 降级：复用 globals 表作为临时容器（日志会丢失，但任务不会崩溃）
                    lua.globals()
                })
            });
            let idx = logs.raw_len() + 1;
            let entry = lua.create_table()?;
            entry.set("level", level)?;
            entry.set("message", message)?;
            entry.set("timestamp_ms", tool_core::now_timestamp_ms())?;
            logs.set(idx, entry)?;
            let _ = state.set("logs", logs);
            Ok(())
        })?,
    )?;

    install_task_method_wrappers(lua, &tbl)?;

    Ok(tbl)
}

fn install_task_method_wrappers(lua: &Lua, methods: &Table) -> mlua::Result<()> {
    let install: Function = lua
        .load(
            r#"
return function(methods)
    methods.sleep_ms = function(task, ms)
        local op = methods.__sleep_ms_begin(task, ms)
        if op ~= nil then
            return coroutine.yield(op)
        end
    end

    methods.wait_if_paused = function(task)
        local op = methods.__wait_if_paused_begin(task)
        if op ~= nil then
            return coroutine.yield(op)
        end
    end
end
"#,
        )
        .set_name("task-method-wrappers")
        .eval()?;
    install.call::<()>(methods.clone())
}

/// ctx.task API
pub(crate) fn create_task_api(
    lua: &Lua,
    bus: DataBus,
    source: String,
    plugin_id: String,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    // ctx.task.start(config, fn)
    let bus_start = bus.clone();
    let src_start = source.clone();
    let pid_start = plugin_id.clone();
    let methods = create_task_methods_table(lua)?;
    table.set(
        "start",
        lua.create_function(move |lua, (config, func): (Table, Function)| {
            let id: String = config.get("id")?;
            let title: String = config.get("title").unwrap_or_else(|_| id.clone());
            let cancellable: bool = config.get("cancellable").unwrap_or(true);
            let pausable: bool = config.get("pausable").unwrap_or(true);

            // 创建 coroutine
            let thread = lua.create_thread(func)?;

            // 创建 task 内部状态表
            let state = lua.create_table()?;
            state.set("id", id.clone())?;
            state.set("title", title.clone())?;
            state.set("thread", &thread)?;
            state.set(TASK_CANCELLED, false)?;
            state.set("paused", false)?;
            state.set("cancellable", cancellable)?;
            state.set("pausable", pausable)?;
            state.set("progress_current", 0_u64)?;
            state.set("progress_total", 0_u64)?;
            state.set("progress_percent", Value::Nil)?;
            state.set("status", "running")?;
            state.set("wake_at_ms", 0_u64)?;
            state.set(TASK_YIELD_OP, Value::Nil)?;
            state.set(TASK_FINISHED, false)?;
            state.set("error", Value::Nil)?;
            state.set("logs", lua.create_table()?)?;

            // 存入全局任务表 — 检查同 ID 是否已有未完成任务
            let tasks: Table = lua.globals().get(PLUGIN_TASKS)?;
            if let Ok(existing) = tasks.get::<Table>(id.as_str()) {
                let finished: bool = existing.get("finished").unwrap_or(true);
                if !finished {
                    return Err(mlua::Error::external(format!(
                        "task id '{id}' is already running"
                    )));
                }
            }
            tasks.set(id.clone(), &state)?;

            // 创建用户可见的 task 对象
            let task_obj = lua.create_table()?;
            task_obj.set("id", id.clone())?;

            // __index: 先查方法表，再查 state
            let mt = lua.create_table()?;
            let m_ref = methods.clone();
            mt.set(
                "__index",
                lua.create_function(move |lua, (tbl, key): (Table, String)| {
                    // 先查方法
                    if let Ok(v) = m_ref.get::<Value>(key.as_str())
                        && !v.is_nil()
                    {
                        return Ok(v);
                    }
                    // 再查 state
                    let task_id: String = tbl.get("id")?;
                    let tasks: Table = lua.globals().get(PLUGIN_TASKS)?;
                    if let Ok(s) = tasks.get::<Table>(task_id.as_str())
                        && let Ok(v) = s.get::<Value>(key.as_str())
                    {
                        return Ok(v);
                    }
                    Ok(Value::Nil)
                })?,
            )?;
            let _ = task_obj.set_metatable(Some(mt));

            bus_start.publish(Event::system_log(
                LogLevel::Info,
                &src_start,
                format!("[插件:{}] 任务 {} 已启动", pid_start, id),
            ));

            // 首次 resume：把 task_obj 传给 function(task)
            // 如果 task 立即 yield（如 sleep），resume 返回 yield 值
            // coroutine 的 yield_op / wake_at_ms 已在 sleep_ms 等函数中设置
            let _ = lua.globals().set(CURRENT_TASK_ID, id.as_str());
            let resume_result = thread.resume::<Value>(task_obj.clone());
            let _ = lua.globals().set(CURRENT_TASK_ID, Value::Nil);

            match resume_result {
                Ok(_) => {
                    // coroutine 正常返回（未 yield，函数执行完毕）
                    match thread.status() {
                        mlua::ThreadStatus::Resumable => {
                            // yielded — yield_op 已由 Lua 侧设置
                        }
                        _ => {
                            let _ = state.set(TASK_FINISHED, true);
                        }
                    }
                }
                Err(_) => {
                    let _ = state.set(TASK_FINISHED, true);
                }
            }

            Ok(Value::Table(task_obj))
        })?,
    )?;

    // ctx.task.cancel(id)
    let tasks_ref = bus.clone();
    let src_cancel = source.clone();
    table.set(
        "cancel",
        lua.create_function(move |lua, id: String| {
            let tasks: Table = lua.globals().get(PLUGIN_TASKS)?;
            if let Ok(state) = tasks.get::<Table>(id.as_str()) {
                let _ = state.set(TASK_CANCELLED, true);
                let _ = state.set("paused", false);
                tasks_ref.publish(Event::system_log(
                    LogLevel::Info,
                    &src_cancel,
                    format!("任务 {} 已取消", id),
                ));
            }
            Ok(())
        })?,
    )?;

    // ctx.task.pause(id)
    table.set(
        "pause",
        lua.create_function(move |lua, id: String| {
            let tasks: Table = lua.globals().get(PLUGIN_TASKS)?;
            if let Ok(state) = tasks.get::<Table>(id.as_str()) {
                let pausable: bool = state.get("pausable").unwrap_or(false);
                if pausable {
                    let _ = state.set("paused", true);
                }
            }
            Ok(())
        })?,
    )?;

    // ctx.task.resume(id)
    table.set(
        "resume",
        lua.create_function(move |lua, id: String| {
            let tasks: Table = lua.globals().get(PLUGIN_TASKS)?;
            if let Ok(state) = tasks.get::<Table>(id.as_str()) {
                let _ = state.set("paused", false);
            }
            Ok(())
        })?,
    )?;

    // ctx.task.list() → 返回所有 task 摘要
    table.set(
        "list",
        lua.create_function(move |lua, ()| {
            let tasks: Table = lua.globals().get(PLUGIN_TASKS)?;
            let result = lua.create_table()?;
            let mut idx = 0_u32;
            for (_id, state) in tasks.pairs::<String, Table>().flatten() {
                idx += 1;
                let summary = lua.create_table()?;
                summary.set("id", state.get::<String>("id").unwrap_or_default())?;
                summary.set("title", state.get::<String>("title").unwrap_or_default())?;
                summary.set(
                    TASK_CANCELLED,
                    state.get::<bool>(TASK_CANCELLED).unwrap_or(false),
                )?;
                summary.set("paused", state.get::<bool>("paused").unwrap_or(false))?;
                summary.set(
                    TASK_FINISHED,
                    state.get::<bool>(TASK_FINISHED).unwrap_or(false),
                )?;
                summary.set(
                    "progress_current",
                    state.get::<u64>("progress_current").unwrap_or(0),
                )?;
                summary.set(
                    "progress_total",
                    state.get::<u64>("progress_total").unwrap_or(0),
                )?;
                summary.set(
                    "progress_percent",
                    state.get::<f64>("progress_percent").unwrap_or(0.0),
                )?;
                summary.set("status", state.get::<String>("status").unwrap_or_default())?;
                summary.set("error", state.get::<String>("error").unwrap_or_default())?;
                result.set(idx, summary)?;
            }
            Ok(Value::Table(result))
        })?,
    )?;

    Ok(table)
}

/// 取消所有 task（插件 disable 时调用），唤醒所有 waiting task 让它们检测取消
pub(crate) fn cancel_all_tasks(lua: &Lua, bus: &DataBus, config: &LuaRunConfig) {
    let tasks: Table = match lua.globals().get(PLUGIN_TASKS) {
        Ok(t) => t,
        Err(_) => return,
    };

    let mut task_ids: Vec<String> = Vec::new();
    for (id, state) in tasks.pairs::<String, Table>().flatten() {
        let _ = state.set(TASK_CANCELLED, true);
        let _ = state.set("paused", false);
        task_ids.push(id);
    }

    // 唤醒所有 task coroutine 让它们发现 cancelled
    for _ in 0..MAX_TASK_RESUMES_PER_TICK {
        let mut any_resumed = false;
        for id in &task_ids {
            let state: Table = match tasks.get(id.as_str()) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if state.get::<bool>(TASK_FINISHED).unwrap_or(true) {
                continue;
            }
            let thread: Thread = match state.get("thread") {
                Ok(t) => t,
                Err(_) => continue,
            };
            let _ = state.set(TASK_YIELD_OP, Value::Nil);
            // 强制 resume，忽略结果，标记为 finished
            let _ = thread.resume::<Value>(());
            let _ = state.set(TASK_FINISHED, true);
            any_resumed = true;
        }
        if !any_resumed {
            break;
        }
    }

    bus.publish(Event::system_log(
        LogLevel::Info,
        &config.source,
        format!("已取消 {} 个任务", task_ids.len()),
    ));
}

pub(crate) fn call_disable(lua: &Lua, bus: &DataBus, config: &LuaRunConfig) {
    // 先取消所有 task，让它们检测 cancelled 并退出
    cancel_all_tasks(lua, bus, config);

    if let Ok(function) = lua.globals().get::<Function>(PLUGIN_DISABLE)
        && let Err(error) = function.call::<()>(())
    {
        bus.publish(Event::system_log(
            LogLevel::Warn,
            &config.source,
            format!("on_disable 回调错误：{error}"),
        ));
    }
}
