use crossbeam_channel::{Receiver, Sender, bounded};
use mlua::{Function, Lua, LuaOptions, StdLib, Table, Value, VmState};
use parking_lot::Mutex as ParkingMutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;
use tool_core::topics;
use tool_core::{Event, LogLevel, Payload, topic_matches};
use tool_databus::{DataBus, TopicFilter};

pub mod api;
pub mod codec;
pub mod config;
pub mod convert;
pub mod globals;
pub mod host_services;
mod mlua_engine;
pub mod replay;
pub(crate) use crate::api::bus::create_bus_api;
pub(crate) use crate::api::commands::create_commands_api;
pub(crate) use crate::api::config::create_config_api;
pub(crate) use crate::api::dialog::create_dialog_api;
pub(crate) use crate::api::fs::create_fs_api;
pub(crate) use crate::api::log::create_log_api;
pub(crate) use crate::api::serial::create_serial_api;
pub(crate) use crate::api::storage::create_storage_api;
pub(crate) use crate::api::task::{
    call_disable, create_task_api, install_task_helpers, process_tasks,
};
pub(crate) use crate::api::test::install_test_api;
pub(crate) use crate::api::timer::create_timer_api;
pub(crate) use crate::api::ui::create_ui_api;
pub(crate) use crate::convert::{event_to_lua_table, json_to_lua_value};
use crate::globals::{
    PLUGIN_CALLBACKS, PLUGIN_COMMANDS, PLUGIN_DISABLE, PLUGIN_STORAGE, PLUGIN_TASKS, PLUGIN_TIMERS,
    TASK_CANCELLED, TASK_FINISHED, TASK_YIELD_OP, YIELD_DEADLINE_MS, YIELD_EXPECT, YIELD_KIND,
    YIELD_READ_LINE, YIELD_SLEEP, YIELD_WAIT_PAUSED, YIELD_WRITE_LINE_AND_EXPECT,
};
use crate::host_services::line_buffer_key;
pub use config::ConfigStore;
pub use mlua_engine::MluaEngine;
pub use replay::{run_replay_analyzer, run_replay_analyzer_with_cancel};
use tool_transport::{TransportManager, serial_topics};

// ── Host Services ──

pub use host_services::{
    DialogRequest, FileAccessBroker, FileFilter, LineBuffer, LineBufferMap, LuaHostServices,
};

const LUA_PLUGIN_EVENT_QUEUE_CAPACITY: usize = 4096;
const LUA_PLUGIN_INTERNAL_SERIAL_RX_CAPACITY: usize = 4096;
const LUA_PLUGIN_INTERNAL_SERIAL_RX_DRAIN_LIMIT: usize = 512;

const LUA_DEFAULT_PERMISSIONS: &[&str] =
    &["bus", "log", "serial", "ui", "storage", "timer", "testing"];

#[derive(Debug, Error)]
pub enum LuaHostError {
    #[error("script is already running")]
    AlreadyRunning,

    #[error("lua error: {0}")]
    Lua(#[from] mlua::Error),
}

pub type LuaHostResult<T> = Result<T, LuaHostError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LuaRunConfig {
    pub script_name: String,
    pub timeout_ms: u64,
    pub source: String,
    pub context: serde_json::Value,

    #[serde(default = "default_lua_permissions")]
    pub permissions: Vec<String>,
}

impl Default for LuaRunConfig {
    fn default() -> Self {
        Self {
            script_name: "scratch.lua".to_owned(),
            timeout_ms: 5_000,
            source: "lua".to_owned(),
            context: json!({}),
            permissions: default_lua_permissions(),
        }
    }
}

fn default_lua_permissions() -> Vec<String> {
    LUA_DEFAULT_PERMISSIONS
        .iter()
        .map(|permission| permission.to_string())
        .collect()
}

fn has_permission(config: &LuaRunConfig, permission: &str) -> bool {
    config
        .permissions
        .iter()
        .any(|candidate| candidate == permission)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LuaRunState {
    Idle,
    Running,
    Finished,
    Failed,
    Stopped,
}

struct LuaWorker {
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    outcome: Arc<ParkingMutex<Option<LuaRunState>>>,
    join: Option<JoinHandle<()>>,
}

pub struct LuaPluginRuntime {
    event_sender: Sender<Event>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    outcome: Arc<ParkingMutex<Option<LuaRunState>>>,
    join: Option<JoinHandle<()>>,
}

impl LuaPluginRuntime {
    pub fn on_event(&self, event: &Event) -> bool {
        if !self.alive.load(Ordering::Relaxed) {
            return false;
        }

        self.event_sender.try_send(event.clone()).is_ok()
    }

    /// 回放事件与实时事件走同一条投递策略（含 `alive` 判定），判据见 [`Self::on_event`]：
    /// 用 `try_send` 而非 `send` —— 回放期间若插件处理慢，丢弃事件比阻塞 UI 线程安全。
    pub fn on_replay_event(&self, event: &Event) -> bool {
        self.on_event(event)
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub fn outcome(&self) -> Option<LuaRunState> {
        *self.outcome.lock()
    }
}

/// 超时 join Lua 线程：防止线程卡住时阻塞 UI 线程的 Drop。
///
/// Lua 线程装有指令 hook，正常情况下几 ms 内就会响应 stop 标记；超时说明它卡在宿主之外
/// （例如阻塞在回调里），此时分离线程、让它自行结束，比无限等待安全。
fn join_with_timeout(join: JoinHandle<()>) {
    const DROP_JOIN_TIMEOUT: Duration = Duration::from_millis(500);

    let deadline = Instant::now() + DROP_JOIN_TIMEOUT;
    while Instant::now() < deadline {
        if join.is_finished() {
            let _ = join.join();
            return;
        }
        std::thread::yield_now();
    }
    // 超时：分离线程，不再等待
}

impl Drop for LuaPluginRuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);

        if let Some(join) = self.join.take() {
            join_with_timeout(join);
        }
    }
}

pub struct LuaHost {
    bus: DataBus,
    transport: TransportManager,
    worker: Option<LuaWorker>,
    last_state: LuaRunState,
}

impl LuaHost {
    pub fn new(bus: DataBus, transport: TransportManager) -> Self {
        Self {
            bus,
            transport,
            worker: None,
            last_state: LuaRunState::Idle,
        }
    }

    pub fn run_script(&mut self, source: String, config: LuaRunConfig) -> LuaHostResult<()> {
        self.reap_finished();

        if self.worker.is_some() {
            return Err(LuaHostError::AlreadyRunning);
        }

        let stop = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let outcome = Arc::new(ParkingMutex::new(None));

        let thread_stop = Arc::clone(&stop);
        let thread_finished = Arc::clone(&finished);
        let thread_outcome = Arc::clone(&outcome);

        let bus = self.bus.clone();
        let transport = self.transport.clone();

        bus.publish(Event::system_log(
            LogLevel::Info,
            &config.source,
            format!("正在运行 {}", config.script_name),
        ));

        let join = thread::spawn(move || {
            let result = run_script_blocking(
                source,
                config.clone(),
                bus.clone(),
                transport,
                Arc::clone(&thread_stop),
            );

            // 结局与日志成对产生：任何分支都不会出现"写了结局没写日志"
            let (state, level, message) = match result {
                Ok(()) => (
                    LuaRunState::Finished,
                    LogLevel::Info,
                    format!("{} 已完成", config.script_name),
                ),
                Err(error) if thread_stop.load(Ordering::Relaxed) => (
                    LuaRunState::Stopped,
                    LogLevel::Warn,
                    format!("{} 已停止：{error}", config.script_name),
                ),
                Err(error) => (
                    LuaRunState::Failed,
                    LogLevel::Error,
                    format!("{} 失败：{error}", config.script_name),
                ),
            };

            *thread_outcome.lock() = Some(state);
            bus.publish(Event::system_log(level, config.source, message));

            thread_finished.store(true, Ordering::Relaxed);
        });

        self.worker = Some(LuaWorker {
            stop,
            finished,
            outcome,
            join: Some(join),
        });

        self.last_state = LuaRunState::Running;

        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.stop.store(true, Ordering::Relaxed);

            self.bus
                .publish(Event::system_log(LogLevel::Warn, "lua", "请求停止"));
        }

        self.reap_finished();
    }

    pub fn state(&mut self) -> LuaRunState {
        self.reap_finished();
        self.last_state
    }

    fn reap_finished(&mut self) {
        let worker_done = self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.finished.load(Ordering::Relaxed));

        if worker_done && let Some(mut worker) = self.worker.take() {
            let outcome = *worker.outcome.lock();

            if let Some(join) = worker.join.take() {
                let _ = join.join();
            }

            self.last_state = outcome.unwrap_or(LuaRunState::Finished);
        }
    }
}

impl Drop for LuaHost {
    fn drop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.stop.store(true, Ordering::Relaxed);
        }

        // 超时 join（策略见 join_with_timeout）
        if let Some(mut worker) = self.worker.take()
            && let Some(join) = worker.join.take()
        {
            join_with_timeout(join);
        }
    }
}

pub fn run_plugin(
    source: String,
    config: LuaRunConfig,
    bus: DataBus,
    transport: TransportManager,
    host_services: LuaHostServices,
) -> LuaHostResult<LuaPluginRuntime> {
    let (event_sender, event_receiver) = bounded(LUA_PLUGIN_EVENT_QUEUE_CAPACITY);

    let stop = Arc::new(AtomicBool::new(false));
    let alive = Arc::new(AtomicBool::new(true));

    let thread_stop = Arc::clone(&stop);
    let thread_alive = Arc::clone(&alive);

    let mut host_services = host_services;
    host_services.stop_flag = Some(Arc::clone(&thread_stop));

    bus.publish(Event::system_log(
        LogLevel::Info,
        &config.source,
        format!("正在启动插件 {}", config.script_name),
    ));

    let thread_outcome = Arc::new(ParkingMutex::new(None));
    // 两份克隆各有用途：loop_outcome 会被 move 进事件循环，fallback_outcome 留给兜底判定
    let fallback_outcome = Arc::clone(&thread_outcome);
    let loop_outcome = Arc::clone(&thread_outcome);

    let join = thread::spawn(move || {
        plugin_event_loop(
            source,
            config.clone(),
            bus.clone(),
            transport,
            event_receiver,
            thread_stop,
            thread_alive,
            host_services,
            loop_outcome,
        );
        // plugin_event_loop 在错误路径已设置 Failed；这里做兜底
        let mut guard = fallback_outcome.lock();
        if guard.is_none() {
            *guard = Some(LuaRunState::Finished);
        }
    });

    Ok(LuaPluginRuntime {
        event_sender,
        stop,
        alive,
        outcome: thread_outcome,
        join: Some(join),
    })
}

/// mlua 在建 state 时无条件打开 `base`（`luaL_requiref(state, "_G", luaopen_base, 1)`，
/// mlua-0.11.6 `src/state/raw.rs:139`），`StdLib` 位掩码只能增删其它标准库、无法移除
/// base。Lua 5.4 的 `dofile`、`loadfile`、`load` 都在 `base_funcs` 里，因此必须显式
/// 抹掉：否则插件可读取并执行宿主文件系统上的任意 `.lua`（含其它插件源码），沙箱边界
/// 形同虚设。
///
/// `PACKAGE` 是启用的（插件要 `require("hw.codec")`），所以这里同时收紧 `require`：
/// `package.searchers` 只保留 preload 一个。mlua 的 safe 构造器已经把两个 C searcher
/// 换成报错桩（`disable_c_modules`，`src/state.rs:2286`），但 Lua 文件 searcher 仍然按
/// `package.path` 去 `luaL_loadfile` 打开磁盘，而 `package.path` 对插件可写 —— 不摘掉
/// 它，抹掉 `loadfile` 只是给同一扇门换了把锁。
///
/// `package.searchpath` 是同一个 searcher 用的解析原语（loadlib.c `searchpath`），摘掉
/// searcher 后它依然可被插件直接调用，用任意模板探测宿主文件是否存在。它是这一类漏洞的
/// 兄弟，所以一并置 nil —— 探测元数据也是信息泄露，而这里没有任何插件需要它。
///
/// Web 端 `omnilua` 走 `SandboxConfig::remove_globals`
/// （`plugin_runtime/src/web_lua.rs:87-97`），本函数让 native 与它对等，并多抹掉
/// `load` 与 `package.searchpath`。
pub(crate) fn harden_globals(lua: &Lua) -> mlua::Result<()> {
    for name in ["dofile", "loadfile", "load"] {
        lua.globals().set(name, mlua::Value::Nil)?;
    }
    // `package` 不在 = PACKAGE 没装载，`require` 也不存在，没什么可收紧；
    // `package` 在而 `searchers` 取不到就往上报错，宁可拒绝启动也不静默留着文件 searcher。
    if let Ok(package) = lua.globals().get::<Table>("package") {
        let searchers: Table = package.get("searchers")?;
        let preload_searcher = searchers.get::<Value>(1)?;
        let frozen = lua.create_table()?;
        frozen.set(1, preload_searcher)?;
        package.set("searchers", frozen)?;
        package.set("searchpath", Value::Nil)?;
    }
    Ok(())
}

/// 生产代码里**唯一**合法的 Lua VM 构造点：把 `Lua::new_with(子集)` 和 `harden_globals`
/// 焊在同一个函数里，"建了 VM 却忘了加固" 就没法再顺手写出来。计划里数了两处构造点，实际
/// 有四处插件可达的 VM（插件事件循环、阻塞式运行、`MluaEngine`、replay analyzer），各自
/// 抄一遍 stdlib 子集；其中 `replay.rs` 那一处连加固都没抄，`dofile` 至今活着。新增任何
/// 能跑插件源码的 VM，一律调用本函数，不要自己 `new_with`；`mod tests` 里的静态守卫按
/// 本函数的名字与全 crate 的出现次数把关，并把 `Lua::new()`、`Lua::default()`、
/// `unsafe_new*`、`new_with_options` 这些"绕过本函数"的拼法一并列为违规。
///
/// 返回 `mlua::Result` 而非 `LuaHostResult`：构造与加固的失败都意味着这个 VM 不可信，
/// 调用方必须一并当作启动失败处理（`plugin_event_loop` 就是这么做到的）。
pub(crate) fn sandbox_lua() -> mlua::Result<Lua> {
    let lua = Lua::new_with(
        StdLib::TABLE
            | StdLib::STRING
            | StdLib::MATH
            | StdLib::UTF8
            | StdLib::PACKAGE
            | StdLib::COROUTINE,
        LuaOptions::default(),
    )?;
    harden_globals(&lua)?;
    Ok(lua)
}

/// 插件启动阶段失败：记 `Failed` 结局、把原因发到总线、宣告线程不再存活。
///
/// 三件事必须一起做完 —— 漏写 outcome 宿主会一直等到超时，漏置 `alive` 则事件会继续
/// 塞进一条已经死掉的循环。原因文本由调用方给出，日志级别统一为 Error。
fn fail_startup(
    outcome: &ParkingMutex<Option<LuaRunState>>,
    bus: &DataBus,
    config: &LuaRunConfig,
    alive: &AtomicBool,
    reason: String,
) {
    *outcome.lock() = Some(LuaRunState::Failed);
    bus.publish(Event::system_log(LogLevel::Error, &config.source, reason));
    alive.store(false, Ordering::Relaxed);
}

// 入参就是插件事件线程的整套环境，各自被循环按不同方式借用；收进一个结构体只会
// 把这些借用变成对同一个大结构的字段借用，故保留本豁免（只贴函数，不扩到模块级）。
#[allow(clippy::too_many_arguments)]
fn plugin_event_loop(
    source: String,
    config: LuaRunConfig,
    bus: DataBus,
    transport: TransportManager,
    event_receiver: Receiver<Event>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    host_services: LuaHostServices,
    outcome: Arc<ParkingMutex<Option<LuaRunState>>>,
) {
    // 启动阶段的任何失败都要同样收尾（见 fail_startup）；这里只递上原因
    let startup_failed = |reason: String| fail_startup(&outcome, &bus, &config, &alive, reason);

    let lua = match sandbox_lua() {
        Ok(lua) => lua,
        Err(error) => {
            startup_failed(format!("创建/加固 Lua 沙箱失败：{error}"));
            return;
        }
    };

    // 安装指令 hook：防止死循环卡死禁用/退出
    let hook_stop = stop.clone();
    if let Err(e) = lua.set_hook(
        mlua::HookTriggers::new().every_nth_instruction(10_000),
        move |_lua, _debug| {
            if hook_stop.load(Ordering::Relaxed) {
                return Err(mlua::Error::RuntimeError("插件已停止".into()));
            }
            Ok(VmState::Continue)
        },
    ) {
        startup_failed(format!("设置指令 hook 失败：{e}"));
        return;
    }

    if let Err(error) = install_ctx(&lua, bus.clone(), transport, &config, &host_services) {
        startup_failed(format!("安装上下文失败：{error}"));
        return;
    }

    // 注入 task 辅助函数（必须在用户脚本之前）
    if let Err(error) = install_task_helpers(&lua) {
        startup_failed(format!("安装任务辅助函数失败：{error}"));
        return;
    }

    let serial_rx_subscription =
        if has_permission(&config, "serial") && host_services.line_buffers.is_some() {
            Some(bus.subscribe_lossy_bounded(
                TopicFilter::exact(serial_topics::SERIAL_RX),
                LUA_PLUGIN_INTERNAL_SERIAL_RX_CAPACITY,
            ))
        } else {
            None
        };

    if let Err(error) = lua.load(&source).set_name(&config.script_name).exec() {
        startup_failed(format!("脚本错误：{error}"));
        return;
    }

    if !has_active_work(&lua) {
        bus.publish(Event::system_log(
            LogLevel::Info,
            &config.source,
            "插件已完成（无回调）",
        ));
        alive.store(false, Ordering::Relaxed);
        return;
    }

    loop {
        if stop.load(Ordering::Relaxed) {
            call_disable(&lua, &bus, &config);
            break;
        }

        if let Some(ref subscription) = serial_rx_subscription {
            for event in subscription.drain_limited(LUA_PLUGIN_INTERNAL_SERIAL_RX_DRAIN_LIMIT) {
                drain_serial_rx_to_buffers(&event, &host_services, &bus);
            }
        }

        process_timers(&lua, &bus, &config);
        process_tasks(&lua, &bus, &config, &host_services);

        let wait_duration = min_wait(next_timer_wait(&lua), next_task_wait(&lua))
            .unwrap_or_else(|| Duration::from_millis(50))
            .min(Duration::from_millis(50));

        let event_result = if let Some(ref subscription) = serial_rx_subscription {
            crossbeam_channel::select! {
                recv(event_receiver) -> message => match message {
                    Ok(event) => Some(Ok(event)),
                    Err(_) => Some(Err(())),
                },
                recv(subscription.receiver_arc()) -> message => {
                    if let Ok(event) = message {
                        drain_serial_rx_to_buffers(&event, &host_services, &bus);
                        for event in subscription.drain_limited(LUA_PLUGIN_INTERNAL_SERIAL_RX_DRAIN_LIMIT) {
                            drain_serial_rx_to_buffers(&event, &host_services, &bus);
                        }
                    }
                    None
                },
                default(wait_duration) => None,
            }
        } else {
            match event_receiver.recv_timeout(wait_duration) {
                Ok(event) => Some(Ok(event)),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Some(Err(())),
            }
        };

        match event_result {
            Some(Ok(event)) => {
                if serial_rx_subscription.is_none() {
                    drain_serial_rx_to_buffers(&event, &host_services, &bus);
                }

                // 管理面事件不进插件事件循环
                if event.topic == topics::PLUGIN_COMMAND_REGISTERED
                    || event.topic == topics::PLUGIN_COMMAND_UNREGISTERED
                    || event.topic == topics::UI_CONTRIBUTION_SET_VALUE
                {
                    continue;
                }

                if handle_plugin_command_event(&lua, &bus, &config, &host_services, &event) {
                    continue;
                }

                if let Some(callback) = get_callback(&lua, &event.topic)
                    && let Ok(event_table) = lua.create_table()
                {
                    let _ = event_to_lua_table(&lua, &event_table, &event);

                    if let Err(error) = callback.call::<Value>(event_table) {
                        bus.publish(Event::system_log(
                            LogLevel::Warn,
                            &config.source,
                            format!("on_event 回调错误：{error}"),
                        ));
                    }
                }
            }
            None => {}
            Some(Err(())) => break,
        }

        if !has_active_work(&lua) && event_receiver.is_empty() {
            break;
        }
    }

    alive.store(false, Ordering::Relaxed);
}

/// 插件是否还有"活着的东西"：回调、命令、定时器、未结束的任务，任一存在即为真。
///
/// 启动后的"没有回调就直接结束"与事件循环的退出条件必须是同一套判据，否则常驻插件会
/// 在某一侧被提前收回。四个全局表读不到时一律按"没有活动"处理（宁可退出，也不会卡住
/// 宿主）；任务按 `finished` 标记计数，缺标记视为已结束。
fn has_active_work(lua: &Lua) -> bool {
    let table_active = |name: &str| -> bool {
        lua.globals()
            .get::<Table>(name)
            .is_ok_and(|table| !table.is_empty())
    };

    let has_tasks = lua.globals().get::<Table>(PLUGIN_TASKS).is_ok_and(|tasks| {
        tasks
            .pairs::<String, Table>()
            .filter_map(|pair| pair.ok())
            .any(|(_, state)| !state.get::<bool>(TASK_FINISHED).unwrap_or(true))
    });

    table_active(PLUGIN_CALLBACKS)
        || table_active(PLUGIN_COMMANDS)
        || table_active(PLUGIN_TIMERS)
        || has_tasks
}

fn handle_plugin_command_event(
    lua: &Lua,
    bus: &DataBus,
    config: &LuaRunConfig,
    host_services: &LuaHostServices,
    event: &Event,
) -> bool {
    if event.topic != topics::PLUGIN_COMMAND_EXECUTE {
        return false;
    }

    let Payload::Json(payload) = &event.payload else {
        bus.publish(Event::system_log(
            LogLevel::Warn,
            &config.source,
            "已忽略插件命令事件：payload 不是 JSON",
        ));
        return true;
    };

    if payload.get("plugin_id").and_then(serde_json::Value::as_str)
        != Some(host_services.plugin_id.as_str())
    {
        return true;
    }

    let Some(command) = payload
        .get("command")
        .and_then(serde_json::Value::as_str)
        .filter(|command| !command.trim().is_empty())
    else {
        bus.publish(Event::system_log(
            LogLevel::Warn,
            &config.source,
            "已忽略插件命令事件：缺少 command 字段",
        ));
        return true;
    };

    let commands: Table = match lua.globals().get(PLUGIN_COMMANDS) {
        Ok(commands) => commands,
        Err(error) => {
            bus.publish(Event::system_log(
                LogLevel::Warn,
                &config.source,
                format!("插件命令表不可用：{error}"),
            ));
            return true;
        }
    };

    let handler: Function = match commands.get(command) {
        Ok(handler) => handler,
        Err(_) => {
            bus.publish(Event::system_log(
                LogLevel::Debug,
                &config.source,
                format!("插件命令 '{command}' 未注册"),
            ));
            return true;
        }
    };

    match json_to_lua_value(lua, payload) {
        Ok(args) => {
            if let Err(error) = handler.call::<Value>(args) {
                bus.publish(Event::system_log(
                    LogLevel::Warn,
                    &config.source,
                    format!("插件命令 '{command}' 执行失败：{error}"),
                ));
            }
        }
        Err(error) => {
            bus.publish(Event::system_log(
                LogLevel::Warn,
                &config.source,
                format!("插件命令 '{command}' payload 转换失败：{error}"),
            ));
        }
    }

    true
}

fn get_callback(lua: &Lua, topic: &str) -> Option<Function> {
    let callbacks: Table = lua.globals().get(PLUGIN_CALLBACKS).ok()?;

    if let Ok(callback) = callbacks.get::<Function>(topic) {
        return Some(callback);
    }

    // 遍历注册的模式：显式 `*` 后缀才按前缀匹配，否则必须精确
    for (pattern, function) in callbacks.pairs::<String, Function>().flatten() {
        if topic_matches(&pattern, topic) {
            return Some(function);
        }
    }

    None
}

fn next_timer_wait(lua: &Lua) -> Option<Duration> {
    let timers: Table = lua.globals().get(PLUGIN_TIMERS).ok()?;
    let now_ms = tool_core::now_timestamp_ms();

    let mut next_trigger_at = u64::MAX;

    for (_, timer) in timers.pairs::<String, Table>().flatten() {
        let trigger_at_ms: u64 = timer.get("trigger_at_ms").unwrap_or(u64::MAX);
        next_trigger_at = next_trigger_at.min(trigger_at_ms);
    }

    wait_until_ms(next_trigger_at, now_ms)
}

fn next_task_wait(lua: &Lua) -> Option<Duration> {
    let tasks: Table = lua.globals().get(PLUGIN_TASKS).ok()?;
    let now_ms = tool_core::now_timestamp_ms();
    let mut next_wake_at = u64::MAX;

    for (_, state) in tasks.pairs::<String, Table>().flatten() {
        if state.get::<bool>(TASK_FINISHED).unwrap_or(true) {
            continue;
        }
        if state.get::<bool>(TASK_CANCELLED).unwrap_or(false) {
            return Some(Duration::ZERO);
        }
        if state.get::<bool>("paused").unwrap_or(false) {
            continue;
        }

        let Some(op) = state.get::<Option<Table>>(TASK_YIELD_OP).ok().flatten() else {
            return Some(Duration::ZERO);
        };
        let kind: String = op.get(YIELD_KIND).unwrap_or_default();
        match kind.as_str() {
            YIELD_SLEEP => {
                let wake_at_ms: u64 = state.get("wake_at_ms").unwrap_or(0);
                next_wake_at = next_wake_at.min(wake_at_ms);
            }
            YIELD_READ_LINE | YIELD_WRITE_LINE_AND_EXPECT => {
                let deadline_ms: u64 = op.get(YIELD_DEADLINE_MS).unwrap_or(0);
                if deadline_ms > 0 {
                    next_wake_at = next_wake_at.min(deadline_ms);
                }
            }
            YIELD_EXPECT => {
                let deadline_ms: u64 = op.get(YIELD_DEADLINE_MS).unwrap_or(0);
                next_wake_at = next_wake_at.min(deadline_ms);
            }
            YIELD_WAIT_PAUSED => {}
            _ => {}
        }
    }

    wait_until_ms(next_wake_at, now_ms)
}

/// 把"最早一个到点时刻"换算成本轮该等待的时长。
///
/// `u64::MAX` 是没有等待者的哨兵（返回 `None`）；时刻已到或已过返回零，让调用方立刻再跑
/// 一轮定时器/任务；其余情况等到那个时刻为止。
fn wait_until_ms(target_ms: u64, now_ms: u64) -> Option<Duration> {
    if target_ms == u64::MAX {
        return None;
    }

    if target_ms <= now_ms {
        return Some(Duration::ZERO);
    }

    Some(Duration::from_millis(target_ms - now_ms))
}

fn min_wait(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(wait), None) | (None, Some(wait)) => Some(wait),
        (None, None) => None,
    }
}

fn process_timers(lua: &Lua, bus: &DataBus, config: &LuaRunConfig) {
    let Ok(timers) = lua.globals().get::<Table>(PLUGIN_TIMERS) else {
        return;
    };

    let now_ms = tool_core::now_timestamp_ms();
    let mut expired = Vec::new();

    for (id, timer) in timers.pairs::<String, Table>().flatten() {
        let trigger_at_ms: u64 = timer.get("trigger_at_ms").unwrap_or(u64::MAX);

        if now_ms < trigger_at_ms {
            continue;
        }

        if let Ok(function) = timer.get::<Function>("callback")
            && let Err(error) = function.call::<()>(())
        {
            bus.publish(Event::system_log(
                LogLevel::Warn,
                &config.source,
                format!("定时器错误：{error}"),
            ));
        }

        let interval_ms: u64 = timer.get("interval_ms").unwrap_or(0);

        if interval_ms > 0 {
            let _ = timer.set("trigger_at_ms", now_ms + interval_ms);
        } else {
            expired.push(id);
        }
    }

    for id in expired {
        let _ = timers.set(id, Value::Nil);
    }
}

// topic_matches 统一使用 tool_core::topic_matches

// ── Line Buffer ──

fn drain_serial_rx_to_buffers(event: &Event, host_services: &LuaHostServices, bus: &DataBus) {
    if event.topic != serial_topics::SERIAL_RX {
        return;
    }
    let Some(ref line_buffers) = host_services.line_buffers else {
        return;
    };
    let port = event
        .metadata
        .as_object()
        .and_then(|m| m.get("port"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let key = line_buffer_key(&host_services.plugin_id, port);
    let data = match &event.payload {
        Payload::Bytes(b) => b.clone(),
        Payload::Text(t) => t.as_bytes().to_vec(),
        _ => return,
    };
    let stats = line_buffers.lock().entry(key).or_default().feed(&data);
    if stats.lines_dropped > 0 || stats.bytes_dropped > 0 {
        bus.publish(Event::system_log(
            LogLevel::Warn,
            &host_services.plugin_id,
            format!(
                "{port} 串口缓冲区溢出：丢弃 {} 行、{} 字节",
                stats.lines_dropped, stats.bytes_dropped
            ),
        ));
    }
}

// ── Task Coroutine 调度 ──
// 已从 api/task.rs 提取，通过 use crate::api::task::* 导入

pub fn run_script_for_test(
    source: &str,
    bus: DataBus,
    transport: TransportManager,
) -> LuaHostResult<()> {
    run_script_blocking(
        source.to_owned(),
        LuaRunConfig::default(),
        bus,
        transport,
        Arc::new(AtomicBool::new(false)),
    )
}

fn run_script_blocking(
    source: String,
    config: LuaRunConfig,
    bus: DataBus,
    transport: TransportManager,
    stop: Arc<AtomicBool>,
) -> LuaHostResult<()> {
    let lua = sandbox_lua()?;

    let test_services = LuaHostServices {
        plugin_root: None,
        plugin_id: "test".to_owned(),
        dialog_sender: None,
        file_broker: None,
        stop_flag: None,
        line_buffers: None,
        config_store: None,
        declared_panel_ids: Default::default(),
    };
    install_ctx(&lua, bus, transport, &config, &test_services)?;
    install_budget_hook(&lua, config.timeout_ms, stop)?;

    lua.load(&source).set_name(&config.script_name).exec()?;

    Ok(())
}

fn install_budget_hook(lua: &Lua, timeout_ms: u64, stop: Arc<AtomicBool>) -> mlua::Result<()> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));

    lua.set_hook(
        mlua::HookTriggers::new().every_nth_instruction(10_000),
        move |_lua, _debug| {
            if stop.load(Ordering::Relaxed) {
                return Err(mlua::Error::RuntimeError("脚本已停止".to_owned()));
            }

            if Instant::now() >= deadline {
                return Err(mlua::Error::RuntimeError("脚本超时".to_owned()));
            }

            Ok(VmState::Continue)
        },
    )
}

fn install_ctx(
    lua: &Lua,
    bus: DataBus,
    transport: TransportManager,
    config: &LuaRunConfig,
    host_services: &LuaHostServices,
) -> mlua::Result<()> {
    let ctx = lua.create_table()?;

    // 插件回调写入的几张全局表：每次装载都从空表开始，避免上一次运行的残留。
    for name in [
        PLUGIN_CALLBACKS,
        PLUGIN_COMMANDS,
        PLUGIN_TIMERS,
        PLUGIN_STORAGE,
        PLUGIN_TASKS,
    ] {
        lua.globals().set(name, lua.create_table()?)?;
    }

    if has_permission(config, "log") {
        ctx.set(
            "log",
            create_log_api(lua, bus.clone(), config.source.clone())?,
        )?;
    }

    if has_permission(config, "bus") {
        ctx.set(
            "bus",
            create_bus_api(
                lua,
                bus.clone(),
                config.source.clone(),
                host_services.stop_flag.clone(),
            )?,
        )?;
    }

    ctx.set(
        "commands",
        create_commands_api(
            lua,
            bus.clone(),
            config.source.clone(),
            host_services.plugin_id.clone(),
        )?,
    )?;

    if has_permission(config, "serial") {
        ctx.set(
            "serial",
            create_serial_api(lua, bus.clone(), transport, host_services)?,
        )?;
    }

    if has_permission(config, "ui") {
        ctx.set(
            "ui",
            create_ui_api(
                lua,
                bus.clone(),
                config.source.clone(),
                host_services.plugin_id.clone(),
                &host_services.declared_panel_ids,
            )?,
        )?;
    }

    if has_permission(config, "timer") {
        ctx.set("timer", create_timer_api(lua)?)?;
    }

    if has_permission(config, "storage") {
        ctx.set("session", create_storage_api(lua)?)?;
    }

    if has_permission(config, "dialog")
        && let Some(sender) = host_services.dialog_sender.clone()
    {
        ctx.set(
            "dialog",
            create_dialog_api(
                lua,
                sender,
                host_services.plugin_id.clone(),
                host_services.stop_flag.clone(),
            )?,
        )?;
    }

    if has_permission(config, "fs.read.user_selected")
        && let Some(broker) = host_services.file_broker.clone()
    {
        ctx.set(
            "fs",
            create_fs_api(lua, broker, host_services.plugin_id.clone())?,
        )?;
    }

    if has_permission(config, "task") {
        ctx.set(
            "task",
            create_task_api(
                lua,
                bus.clone(),
                config.source.clone(),
                host_services.plugin_id.clone(),
            )?,
        )?;
    }

    if has_permission(config, "config")
        && let Some(ref store) = host_services.config_store
    {
        ctx.set(
            "config",
            create_config_api(lua, store.clone(), host_services.plugin_id.clone())?,
        )?;
    }

    // 无 plugin_root 时（如临时脚本/测试）把 package.path 清空，阻止 require 读非预期路径
    let package_path = match &host_services.plugin_root {
        Some(root) => {
            let root_str = root.display().to_string().replace('\\', "/");
            format!("{root_str}/lib/?.lua;{root_str}/?.lua")
        }
        None => String::new(),
    };
    if let Ok(package) = lua.globals().get::<Table>("package") {
        let _ = package.set("path", package_path);
        let _ = package.set("cpath", "");
    }

    // ── 在沙箱加固前注册 codec 模块 ──
    // 沙箱会将 package.preload 替换为只读副本，所以 codec 必须在沙箱之前注册。
    if let Err(e) = codec::register_codec(lua) {
        log::warn!("failed to register hw.codec: {e}");
    }
    if let Err(e) = codec::register_utils(lua) {
        log::warn!("failed to register hw.utils: {e}");
    }

    // 沙箱加固：锁定 package.preload 为只读，防止插件注入恶意模块
    if let Ok(preload) = lua.globals().get::<Table>("package")
        && let Ok(preload_table) = preload.get::<Table>("preload")
    {
        // 将 preload 替换为冻结副本：插件的 require 可从 preload 读取，
        // 但无法写入新条目（写入被 metatable __newindex 拦截）
        let frozen = lua.create_table()?;
        for (name, loader) in preload_table.pairs::<String, Function>().flatten() {
            frozen.set(name, loader)?;
        }
        let mt = lua.create_table()?;
        mt.set(
            "__newindex",
            lua.create_function(
                |_lua, (_key, _value): (String, Value)| -> Result<(), mlua::Error> {
                    Err(mlua::Error::RuntimeError(
                        "package.preload is read-only".into(),
                    ))
                },
            )?,
        )?;
        mt.set("__metatable", "protected")?;
        let _ = frozen.set_metatable(Some(mt));
        let _ = preload.set("preload", frozen);
    }

    ctx.set(
        "now_ms",
        lua.create_function(|_lua, ()| Ok(tool_core::now_timestamp_ms()))?,
    )?;

    ctx.set("plugin", json_to_lua_value(lua, &config.context)?)?;

    lua.globals().set("ctx", &ctx)?;

    lua.globals().set(
        "on_disable",
        lua.create_function(|lua, function: Function| lua.globals().set(PLUGIN_DISABLE, function))?,
    )?;

    if has_permission(config, "testing") {
        install_test_api(lua, &ctx, bus, config)?;
    }

    Ok(())
}

const TEST_BOOTSTRAP: &str = include_str!("test_bootstrap.lua");

// ── Replay Analyzer ──

/// Replay analyzer 运行配置。
#[derive(Debug, Clone)]
pub struct LuaReplayConfig {
    pub script_name: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub subscriptions: Vec<String>,
    pub outputs: Vec<String>,
    pub context: serde_json::Value,
    pub plugin_root: Option<std::path::PathBuf>,
}

/// Replay analyzer 输出。
#[derive(Debug, Clone)]
pub struct LuaReplayOutput {
    pub events: Vec<Event>,
    pub logs: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tool_databus::{Subscription, TopicFilter};
    use tool_transport::serial_rx_event;

    /// 测试用宿主服务：只填 `plugin_id`，对话框/文件/配置/行缓冲区等宿主能力一律不接入。
    fn test_host_services(plugin_id: &str) -> LuaHostServices {
        LuaHostServices {
            plugin_root: None,
            plugin_id: plugin_id.to_owned(),
            dialog_sender: None,
            file_broker: None,
            stop_flag: None,
            line_buffers: None,
            config_store: None,
            declared_panel_ids: Default::default(),
        }
    }

    /// 同上，另接入一个空的行缓冲区映射：`ctx.serial` 的按行读取与 expect 要靠它。
    fn test_host_services_with_line_buffers(plugin_id: &str) -> LuaHostServices {
        LuaHostServices {
            line_buffers: Some(Arc::new(ParkingMutex::new(HashMap::new()))),
            ..test_host_services(plugin_id)
        }
    }

    /// 每 50ms 轮询一次日志订阅、最多等 2 秒，返回第一条命中任一 `needles` 的日志文本。
    ///
    /// 判定与用例原来的手写循环一致：不匹配的日志同样被消费掉（所以下一次轮询接着往
    /// 后读），超时返回 `None`，由各用例自己 `assert` 并给出失败消息。
    fn wait_for_log(logs: &Subscription, needles: &[&str]) -> Option<String> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut matched = None;
        while Instant::now() < deadline {
            if let Ok(event) = logs.recv_timeout(Duration::from_millis(50)) {
                let text = event.payload.text_lossy();
                if needles.iter().any(|needle| text.contains(needle)) {
                    matched = Some(text);
                    break;
                }
            }
        }
        matched
    }

    /// 一条喂给回放分析器的串口 RX 事件：主题用生产常量，避免和用例里订阅的
    /// `subscriptions` 各写一份字面量而悄悄对不上。
    fn replay_rx_event(text: &str) -> Event {
        Event::new(
            serial_topics::SERIAL_RX,
            "serial:COM2",
            tool_core::Direction::Rx,
            Payload::Text(text.to_owned()),
        )
    }

    /// 跑一遍回放分析器，失败直接 panic —— 这些用例断言的是成功路径的产物。
    fn run_replay(source: &str, config: LuaReplayConfig, inputs: &[Event]) -> LuaReplayOutput {
        crate::replay::run_replay_analyzer(source.to_owned(), config, inputs).unwrap()
    }

    /// 在一对一次性的总线/传输上跑完脚本并交回结果：沙箱用例的断言全在脚本里，
    /// 宿主侧只判断这段脚本有没有跑失败。
    fn run_in_scratch_vm(source: &str) -> LuaHostResult<()> {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        run_script_for_test(source, bus, transport)
    }

    /// 在一次性宿主上阻塞跑一段带 task 权限的脚本：这两个用例的断言全写在 Lua 里，
    /// 跑失败（含 Lua 断言失败）就 panic。
    fn run_task_script(script_name: &str, source: &str) {
        let mut permissions = default_lua_permissions();
        permissions.push("task".to_owned());

        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        run_script_blocking(
            source.to_owned(),
            LuaRunConfig {
                script_name: script_name.to_owned(),
                timeout_ms: 5_000,
                source: "test".to_owned(),
                context: json!({}),
                permissions,
            },
            bus,
            transport,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    }

    #[test]
    fn bundled_gcode_sender_lua_tests() {
        let script_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/gcode-sender/tests/main_test.lua")
            .canonicalize()
            .expect("resolve gcode-sender Lua test path");
        let source = std::fs::read_to_string(&script_path).expect("read gcode-sender Lua tests");

        let lua = Lua::new();
        let arg = lua.create_table().expect("create Lua arg table");
        arg.set(0, script_path.to_string_lossy().as_ref())
            .expect("set Lua script path");
        lua.globals()
            .set("arg", arg)
            .expect("install Lua arg table");

        lua.load(&source)
            .set_name(script_path.to_string_lossy().as_ref())
            .exec()
            .expect("gcode-sender Lua tests failed");
    }

    #[test]
    fn lua_log_reaches_databus() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let logs = bus.subscribe(TopicFilter::prefix("log."));

        run_script_for_test("ctx.log.info('hello from lua')", bus, transport).unwrap();

        let events = logs.drain();

        assert!(
            events.iter().any(|event| event.source == "lua"
                && event.payload.text_lossy().contains("hello from lua"))
        );
    }

    #[test]
    fn lua_bus_publish_accepts_tables() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::PROTOCOL_PID_SAMPLE));

        run_script_for_test(
            "ctx.bus.publish('protocol.pid.sample', { t = 1, target = 2.5, actual = 2.0 })",
            bus,
            transport,
        )
        .unwrap();

        let event = rx.drain().pop().unwrap();

        assert_eq!(event.topic, topics::PROTOCOL_PID_SAMPLE);

        assert_eq!(
            event.payload.text_lossy(),
            r#"{"actual":2.0,"t":1,"target":2.5}"#
        );
    }

    #[test]
    fn plugin_command_event_invokes_registered_handler() {
        let bus = DataBus::new();
        let logs = bus.subscribe(TopicFilter::prefix("log."));
        let transport = TransportManager::new(bus.clone());
        let host_services = test_host_services("cmd.plugin");

        let runtime = run_plugin(
            r#"
ctx.commands.register("cmd.plugin.run", function(payload)
    local context = payload.context or {}
    ctx.log.info("command:" .. tostring(context.value))
end)
"#
            .to_owned(),
            LuaRunConfig {
                script_name: "commands.lua".to_owned(),
                timeout_ms: 5_000,
                source: "plugin:cmd.plugin".to_owned(),
                context: json!({"id": "cmd.plugin"}),
                permissions: vec!["log".to_owned()],
            },
            bus.clone(),
            transport,
            host_services,
        )
        .unwrap();

        let event = Event::new(
            topics::PLUGIN_COMMAND_EXECUTE,
            "test",
            tool_core::Direction::Internal,
            Payload::Json(json!({
                "plugin_id": "cmd.plugin",
                "command": "cmd.plugin.run",
                "context": { "value": "ok" }
            })),
        );
        assert!(runtime.on_event(&event));

        let saw_command = wait_for_log(&logs, &["command:ok"]);
        assert!(
            saw_command.is_some(),
            "registered command handler was not invoked"
        );
    }

    #[test]
    fn serial_blocking_wrappers_are_exported() {
        run_in_scratch_vm(
            r#"
assert(type(ctx.serial.read_line) == "function", type(ctx.serial.read_line))
assert(type(ctx.serial.write_line_and_expect) == "function", type(ctx.serial.write_line_and_expect))
"#,
        )
        .unwrap();
    }

    #[test]
    fn serial_read_line_uses_internal_rx_subscription() {
        let bus = DataBus::new();
        let logs = bus.subscribe(TopicFilter::prefix("log."));
        let transport = TransportManager::new(bus.clone());
        let host_services = test_host_services_with_line_buffers("test-plugin");

        let _runtime = run_plugin(
            r#"
ctx.task.start({ id = "reader" }, function()
    local result = ctx.serial.read_line("COM1", { timeout_ms = 1000 })
    if result.err then
        error(result.err)
    end
    ctx.log.info("read:" .. tostring(result.line))
end)
ctx.log.info("reader-ready")
"#
            .to_owned(),
            LuaRunConfig {
                script_name: "internal-serial-rx.lua".to_owned(),
                timeout_ms: 5_000,
                source: "plugin:test-plugin".to_owned(),
                context: json!({}),
                permissions: vec!["serial".to_owned(), "task".to_owned(), "log".to_owned()],
            },
            bus.clone(),
            transport,
            host_services,
        )
        .unwrap();

        let saw_ready = wait_for_log(&logs, &["reader-ready"]);
        assert!(saw_ready.is_some(), "plugin did not start read task");

        bus.publish(serial_rx_event("serial:COM1", b"ok\n".to_vec()));

        let saw_read = wait_for_log(&logs, &["read:ok"]);
        assert!(
            saw_read.is_some(),
            "ctx.serial.read_line did not receive internal RX"
        );
    }

    #[test]
    fn serial_continue_response_can_reset_inactivity_timeout() {
        let bus = DataBus::new();
        let logs = bus.subscribe(TopicFilter::prefix("log."));
        let transport = TransportManager::new(bus.clone());
        let virtual_port = transport
            .open_virtual_serial("COM1")
            .expect("open virtual serial");
        let host_services = test_host_services_with_line_buffers("busy-plugin");

        let _runtime = run_plugin(
            r#"
ctx.task.start({ id = "sender" }, function()
    local response = ctx.serial.write_line_and_expect("COM1", "M105", {
        timeout_ms = 400,
        continue_resets_timeout = true,
        patterns = {
            { name = "busy", pattern = "busy", action = "continue" },
            { name = "ok", pattern = "^ok", action = "return" },
        },
    })
    if response.err then
        ctx.log.error("expect:" .. response.err)
    else
        ctx.log.info("expect:" .. response.result.name)
    end
end)
ctx.log.info("sender-ready")
"#
            .to_owned(),
            LuaRunConfig {
                script_name: "continue-timeout.lua".to_owned(),
                timeout_ms: 5_000,
                source: "plugin:busy-plugin".to_owned(),
                context: json!({}),
                permissions: vec!["serial".to_owned(), "task".to_owned(), "log".to_owned()],
            },
            bus,
            transport,
            host_services,
        )
        .unwrap();

        let saw_ready = wait_for_log(&logs, &["sender-ready"]);
        assert!(saw_ready.is_some(), "plugin did not start expect task");

        thread::sleep(Duration::from_millis(250));
        virtual_port.inject_rx(b"echo:busy: processing\n".to_vec());
        thread::sleep(Duration::from_millis(250));
        virtual_port.inject_rx(b"ok\n".to_vec());

        let result = wait_for_log(&logs, &["expect:"]);
        assert_eq!(result.as_deref(), Some("expect:ok"));
    }

    #[test]
    fn serial_expect_yields_in_task_and_receives_response() {
        // 验证 expect/expect_from 在 ctx.task 协程内走 yield 路径（不阻塞插件循环），
        // 且能收到后续 RX 响应。
        let bus = DataBus::new();
        let logs = bus.subscribe(TopicFilter::prefix("log."));
        let transport = TransportManager::new(bus.clone());
        let virtual_port = transport
            .open_virtual_serial("COM2")
            .expect("open virtual serial");
        let host_services = test_host_services_with_line_buffers("expect-plugin");

        let _runtime = run_plugin(
            r#"
ctx.task.start({ id = "waiter" }, function()
    local line = ctx.serial.expect_from("COM2", "READY", 1000)
    if line then
        ctx.log.info("got:" .. tostring(line))
    else
        ctx.log.error("timeout")
    end
end)
ctx.log.info("waiter-ready")
"#
            .to_owned(),
            LuaRunConfig {
                script_name: "expect-yield.lua".to_owned(),
                timeout_ms: 5_000,
                source: "plugin:expect-plugin".to_owned(),
                context: json!({}),
                permissions: vec!["serial".to_owned(), "task".to_owned(), "log".to_owned()],
            },
            bus.clone(),
            transport,
            host_services,
        )
        .unwrap();

        let saw_ready = wait_for_log(&logs, &["waiter-ready"]);
        assert!(saw_ready.is_some(), "plugin did not start expect task");

        // 发送匹配的 RX；task 协程应在 yield 后收到并记录。
        virtual_port.inject_rx(b"~ READY ~\n".to_vec());

        let result = wait_for_log(&logs, &["got:", "timeout"]);
        assert!(
            result.as_deref() == Some("got:~ READY ~"),
            "expect should yield and receive the response, got {result:?}"
        );
    }

    #[test]
    fn task_start_sets_current_task_id_on_first_resume() {
        run_task_script(
            "task-first-resume.lua",
            r#"
local task = ctx.task.start({ id = "instant" }, function()
    assert(__current_task_id == "instant", tostring(__current_task_id))
end)
assert(task.finished == true)
"#,
        );
    }

    #[test]
    fn task_sleep_yields_from_lua_wrapper_inside_pcall() {
        run_task_script(
            "task-sleep-yield.lua",
            r#"
local task = ctx.task.start({ id = "sleepy" }, function(task)
    pcall(function()
        task:sleep_ms(10)
    end)
end)
assert(task.finished == false)
"#,
        );
    }

    #[test]
    fn next_task_wait_tracks_sleep_wake_time() {
        let lua = Lua::new();
        let tasks = lua.create_table().unwrap();
        lua.globals().set(PLUGIN_TASKS, tasks.clone()).unwrap();

        let state = lua.create_table().unwrap();
        state.set(TASK_FINISHED, false).unwrap();
        state.set(TASK_CANCELLED, false).unwrap();
        state.set("paused", false).unwrap();
        state
            .set("wake_at_ms", tool_core::now_timestamp_ms() + 10)
            .unwrap();

        let op = lua.create_table().unwrap();
        op.set(YIELD_KIND, YIELD_SLEEP).unwrap();
        state.set(TASK_YIELD_OP, op).unwrap();
        tasks.set("sleepy", state).unwrap();

        let wait = next_task_wait(&lua).expect("sleeping task should set next wait");
        assert!(
            wait <= Duration::from_millis(10),
            "sleeping task wait should be <= 10ms, got {wait:?}"
        );
    }

    #[test]
    fn lua_timeout_stops_busy_loop() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());

        let result = run_script_blocking(
            "while true do end".to_owned(),
            LuaRunConfig {
                script_name: "loop.lua".to_owned(),
                timeout_ms: 20,
                source: "lua".to_owned(),
                context: json!({}),
                permissions: default_lua_permissions(),
            },
            bus,
            transport,
            Arc::new(AtomicBool::new(false)),
        );

        assert!(result.is_err());
    }

    #[test]
    fn lua_bus_wait_receives_later_event() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let publisher = bus.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));

            publisher.publish(Event::new(
                "test.ready",
                "test",
                tool_core::Direction::Internal,
                Payload::Text("ready".to_owned()),
            ));
        });

        run_script_for_test(
            "local event = ctx.bus.wait('test.ready', 500)\nassert(event.payload == 'ready')",
            bus,
            transport,
        )
        .unwrap();
    }

    #[test]
    fn lua_serial_expect_matches_rx_text() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let publisher = bus.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            publisher.publish(serial_rx_event("test", b"READY\r\n".to_vec()));
        });

        run_script_for_test(
            "local line = ctx.serial.expect('READY', 500)\nassert(line == 'READY\\r\\n')",
            bus,
            transport,
        )
        .unwrap();
    }

    // ── expect / expect_from 的三种模式形态 ────────────────────────────────
    //
    // `write_line_and_expect` 的 patterns 早就能用 `re:` 与 `^`（见
    // `docs/lua-plugin-api.md`「patterns 匹配规则」），但 `expect`/`expect_from`
    // 的匹配点当时是裸 `contains`，同一个 `"re:^ok"` 在这里被当成字面量子串。
    // 下面每个用例只钉一种形态 × 一条路径：阻塞路径与 task yield 路径是**不同的
    // 匹配点**，少任一处都不会被另一处发现。

    #[test]
    fn blocking_expect_honours_regex_prefix() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let publisher = bus.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            publisher.publish(serial_rx_event("test", b"READY\r\n".to_vec()));
        });

        run_script_for_test(
            "local line = ctx.serial.expect('re:^READY', 500)\n\
             assert(line == 'READY\\r\\n', 'expect(re:) 应命中，实得 ' .. tostring(line))",
            bus,
            transport,
        )
        .unwrap();
    }

    #[test]
    fn blocking_expect_from_honours_caret_anchor_over_paren_prefix() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let publisher = bus.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            publisher.publish(serial_rx_event("test", b"(0.00000)ok\r\n".to_vec()));
        });

        run_script_for_test(
            "local line = ctx.serial.expect_from('test', '^ok', 500)\n\
             assert(line == '(0.00000)ok\\r\\n', 'expect_from(^) 应跳过行首 (…) 后锚定，实得 ' .. tostring(line))",
            bus,
            transport,
        )
        .unwrap();
    }

    #[test]
    fn blocking_expect_caret_anchor_stays_anchored() {
        // 反面：锚定必须真的是锚定。`^ok` 不该命中 "rookie"，否则说明改动只是
        // 把模式串里的 `^` 去掉、退化成宽松子串。
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let publisher = bus.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            publisher.publish(serial_rx_event("test", b"rookie\r\n".to_vec()));
        });

        run_script_for_test(
            "local line = ctx.serial.expect('^ok', 500)\n\
             assert(line == nil, '^ok 不该命中 rookie，实得 ' .. tostring(line))",
            bus,
            transport,
        )
        .unwrap();
    }

    #[test]
    fn blocking_request_honours_regex_prefix() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        // 端口必须先开着，`request` 的发送那一支才不会报错；句柄要活着持有。
        let _virtual_port = transport
            .open_virtual_serial("COM9")
            .expect("open virtual serial");
        let publisher = bus.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            publisher.publish(serial_rx_event("COM9", b"READY\r\n".to_vec()));
        });

        run_script_for_test(
            "local line = ctx.serial.request({ port = 'COM9', tx = 'M105', expect = 're:^READY', timeout_ms = 500 })\n\
             assert(line == 'READY\\r\\n', 'request(re:) 应命中，实得 ' .. tostring(line))",
            bus,
            transport,
        )
        .unwrap();
    }

    #[test]
    fn blocking_request_caret_anchor_stays_anchored() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let _virtual_port = transport
            .open_virtual_serial("COM9")
            .expect("open virtual serial");
        let publisher = bus.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            publisher.publish(serial_rx_event("COM9", b"not ready at all\r\n".to_vec()));
        });

        run_script_for_test(
            "local line = ctx.serial.request({ port = 'COM9', tx = 'M105', expect = '^ready', timeout_ms = 500 })\n\
             assert(line == nil, '^ready 不该命中行中出现的 ready，实得 ' .. tostring(line))",
            bus,
            transport,
        )
        .unwrap();
    }
    #[test]
    fn task_yield_expect_from_honours_regex_prefix() {
        // 与 `serial_expect_yields_in_task_and_receives_response` 同一条路径，
        // 只是模式换成 `re:`：这条走 process_tasks 的 YIELD_EXPECT 分支。
        let bus = DataBus::new();
        let logs = bus.subscribe(TopicFilter::prefix("log."));
        let transport = TransportManager::new(bus.clone());
        let virtual_port = transport
            .open_virtual_serial("COM2")
            .expect("open virtual serial");
        let host_services = test_host_services_with_line_buffers("expect-regex-plugin");

        let _runtime = run_plugin(
            r#"
ctx.task.start({ id = "waiter" }, function()
    local line = ctx.serial.expect_from("COM2", "re:~ READY", 1000)
    if line then
        ctx.log.info("got:" .. tostring(line))
    else
        ctx.log.error("timeout")
    end
end)
ctx.log.info("waiter-ready")
"#
            .to_owned(),
            LuaRunConfig {
                script_name: "expect-yield-regex.lua".to_owned(),
                timeout_ms: 5_000,
                source: "plugin:expect-regex-plugin".to_owned(),
                context: json!({}),
                permissions: vec!["serial".to_owned(), "task".to_owned(), "log".to_owned()],
            },
            bus.clone(),
            transport,
            host_services,
        )
        .unwrap();

        // 必须先确认协程已挂起，否则 inject_rx 早于 yield，YIELD_EXPECT 分支跑不到。
        let saw_ready = wait_for_log(&logs, &["waiter-ready"]);
        assert!(saw_ready.is_some(), "plugin did not start expect task");

        virtual_port.inject_rx(b"~ READY ~\n".to_vec());

        let result = wait_for_log(&logs, &["got:", "timeout"]);
        assert!(
            result.as_deref() == Some("got:~ READY ~"),
            "yield 路径的 expect_from 应按 re: 命中，实得 {result:?}"
        );
    }

    #[test]
    fn lua_ui_create_chart_publishes_panel_event() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::UI_PANEL_CREATE));

        run_script_for_test(
            "ctx.ui.create_chart({ id = 'pid-chart', title = 'PID Chart', topic_prefix = 'protocol.pid.' })",
            bus,
            transport,
        )
        .unwrap();

        let event = rx.drain().pop().unwrap();

        assert_eq!(event.topic, topics::UI_PANEL_CREATE);
        assert!(event.payload.text_lossy().contains("pid-chart"));
    }

    #[test]
    fn lua_ui_create_attitude_publishes_panel_event() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::UI_PANEL_CREATE));

        run_script_for_test(
            "ctx.ui.create_attitude({ id = 'imu-attitude', title = 'IMU Attitude' })",
            bus,
            transport,
        )
        .unwrap();

        let event = rx.drain().pop().unwrap();

        assert_eq!(event.topic, topics::UI_PANEL_CREATE);
        assert!(event.payload.text_lossy().contains("imu-attitude"));
    }

    #[test]
    fn lua_test_case_publishes_passed_report() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::TEST_RESULT));

        run_script_for_test(
            "test.case('math works', function()\n  test.assert(1 + 1 == 2, 'math broke')\nend)",
            bus,
            transport,
        )
        .unwrap();

        let event = rx.drain().pop().unwrap();

        let report: serde_json::Value = match event.payload {
            Payload::Json(value) => value,
            _ => panic!(),
        };

        assert_eq!(report["cases"][0]["name"], "math works");
        assert_eq!(report["cases"][0]["status"], "passed");
    }

    #[test]
    fn lua_test_case_publishes_failed_report() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::TEST_RESULT));

        run_script_for_test(
            "test.case('fails clearly', function()\n  test.assert(false, 'expected failure')\nend)",
            bus,
            transport,
        )
        .unwrap();

        let event = rx.drain().pop().unwrap();

        let report: serde_json::Value = match event.payload {
            Payload::Json(value) => value,
            _ => panic!(),
        };

        assert_eq!(report["cases"][0]["status"], "failed");

        assert!(
            report["cases"][0]["error"]
                .as_str()
                .unwrap()
                .contains("expected failure")
        );
    }

    #[test]
    fn lua_test_case_associates_raw_packets() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::TEST_RESULT));

        let publisher = bus.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            publisher.publish(serial_rx_event("test", b"OK\r\n".to_vec()));
        });

        run_script_for_test(
            "test.case('waits for serial', function()\n  local line = ctx.serial.expect('OK', 500)\n  test.assert(line ~= nil, 'missing serial response')\nend)",
            bus,
            transport,
        )
        .unwrap();

        let event = rx.drain().pop().unwrap();

        let report: serde_json::Value = match event.payload {
            Payload::Json(value) => value,
            _ => panic!(),
        };

        assert_eq!(report["cases"][0]["status"], "passed");
        assert_eq!(
            report["cases"][0]["raw_packets"][0]["payload_text"],
            "OK\r\n"
        );
    }

    // ── replay analyzer 测试 ──

    #[test]
    fn replay_analyzer_no_serial_access() {
        let source = r#"
function on_replay_begin(session)
end
function on_replay_event(event)
    -- 尝试访问 ctx.serial 应该失败
    local ok, _ = pcall(function()
        ctx.serial.list()
    end)
    assert(not ok, "ctx.serial should not be available")
end
function on_replay_end()
end
"#;

        let config = LuaReplayConfig {
            script_name: "test.lua".to_owned(),
            plugin_id: "test".to_owned(),
            plugin_version: "1.0.0".to_owned(),
            subscriptions: vec![serial_topics::SERIAL_RX.to_owned()],
            outputs: vec![],
            context: json!({"id": "test", "name": "Test"}),
            plugin_root: None,
        };

        let output = run_replay(source, config, &[replay_rx_event("hello")]);
        // assert 失败会报 error，这里验证没有致命错误
        assert!(output.events.is_empty());
    }

    #[test]
    fn replay_analyzer_no_timer_access() {
        let source = r#"
function on_replay_begin(session)
end
function on_replay_event(event)
    local ok, _ = pcall(function()
        ctx.timer.after(10, function() end)
    end)
    assert(not ok, "ctx.timer should not be available")
end
function on_replay_end()
end
"#;

        let config = LuaReplayConfig {
            script_name: "test.lua".to_owned(),
            plugin_id: "test".to_owned(),
            plugin_version: "1.0.0".to_owned(),
            subscriptions: vec![serial_topics::SERIAL_RX.to_owned()],
            outputs: vec![],
            context: json!({"id": "test", "name": "Test"}),
            plugin_root: None,
        };

        let output = run_replay(source, config, &[replay_rx_event("hello")]);
        assert!(output.events.is_empty());
    }

    #[test]
    fn replay_analyzer_emit_has_correct_metadata() {
        let source = r#"
function on_replay_begin(session)
end
function on_replay_event(event)
    ctx.replay.emit("protocol.demo.sample", { t = 1, value = 100 })
end
function on_replay_end()
end
"#;

        let config = LuaReplayConfig {
            script_name: "test.lua".to_owned(),
            plugin_id: "demo.plugin".to_owned(),
            plugin_version: "2.0.0".to_owned(),
            subscriptions: vec![serial_topics::SERIAL_RX.to_owned()],
            outputs: vec![],
            context: json!({"id": "demo.plugin", "name": "Demo", "version": "2.0.0"}),
            plugin_root: None,
        };

        // 输入事件留在手里：下面还要用它的 timestamp_ms 作比对
        let input = replay_rx_event("test");

        let output = run_replay(source, config, std::slice::from_ref(&input));
        assert_eq!(output.events.len(), 1);

        let derived = &output.events[0];
        assert_eq!(derived.topic, "protocol.demo.sample");
        assert!(derived.is_replay());
        assert_eq!(derived.origin(), Some("replay_derived"));
        assert_eq!(derived.category(), Some("derived"));
        assert!(derived.meta_bool("derived"));
        assert_eq!(derived.meta_str("plugin_id"), Some("demo.plugin"));
        assert_eq!(derived.meta_str("plugin_version"), Some("2.0.0"));
        assert!(!derived.meta_bool("recordable"));
        // source 应包含 replay-analyzer 前缀
        assert!(derived.source.starts_with("replay-analyzer:"));
        // timestamp_ms 应该等于输入事件的时间戳
        assert_eq!(derived.timestamp_ms, input.timestamp_ms);
    }

    #[test]
    fn replay_analyzer_lifecycle() {
        let source = r#"
local phases = {}
function on_replay_begin(session)
    table.insert(phases, "begin")
    ctx.replay.log("started with " .. session.event_count .. " events")
end
function on_replay_event(event)
    table.insert(phases, "event")
    ctx.replay.emit("test.out", { phase = "event" })
end
function on_replay_end()
    table.insert(phases, "end")
    ctx.replay.emit("test.out", { phase = "end" })
end
"#;

        let config = LuaReplayConfig {
            script_name: "lifecycle.lua".to_owned(),
            plugin_id: "test.lifecycle".to_owned(),
            plugin_version: "1.0.0".to_owned(),
            subscriptions: vec![serial_topics::SERIAL_RX.to_owned()],
            outputs: vec![],
            context: json!({"id": "test.lifecycle", "name": "Lifecycle"}),
            plugin_root: None,
        };

        let inputs = [replay_rx_event("a"), replay_rx_event("b")];

        let output = run_replay(source, config, &inputs);

        // 应该有 2 个 event 阶段 + 1 个 end 阶段 = 3 个 emit
        assert_eq!(output.events.len(), 3);
        // 日志应该有 begin 消息
        assert!(
            output
                .logs
                .iter()
                .any(|l| l.contains("started with 2 events"))
        );
    }

    #[test]
    fn replay_analyzer_skips_unmatched_events() {
        let source = r#"
function on_replay_begin(session) end
function on_replay_event(event)
    ctx.replay.emit("test.out", {})
end
function on_replay_end() end
"#;

        let config = LuaReplayConfig {
            script_name: "skip.lua".to_owned(),
            plugin_id: "test.skip".to_owned(),
            plugin_version: "1.0.0".to_owned(),
            subscriptions: vec![serial_topics::SERIAL_RX.to_owned()],
            outputs: vec![],
            context: json!({"id": "test.skip", "name": "Skip"}),
            plugin_root: None,
        };

        // 只有 1 个匹配的 RX 事件，另 1 个是 TX
        let tx = Event::new(
            serial_topics::SERIAL_TX,
            "serial:COM2",
            tool_core::Direction::Tx,
            Payload::Text("tx".to_owned()),
        );

        let output = run_replay(source, config, &[replay_rx_event("rx"), tx]);
        assert_eq!(
            output.events.len(),
            1,
            "should only emit for matched RX event"
        );
    }

    #[test]
    fn sandbox_package_preload_is_read_only() {
        // 尝试写入 package.preload 应该被 metatable 拦截
        let result = run_in_scratch_vm(
            r#"
local ok, err = pcall(function()
    package.preload.evil = function() return "pwned" end
end)
assert(not ok, "package.preload must be read-only, got success")
assert(err ~= nil, "expected error message")
"#,
        );
        assert!(result.is_ok(), "sandbox test should pass: {result:?}");
    }

    #[test]
    fn sandbox_base_file_loaders_are_absent() {
        // 断言"全局不存在"，不是"调用报错了"：后者在文件缺失时与被禁不可区分。
        let result = run_in_scratch_vm(
            r#"
assert(dofile == nil, "dofile must be nil, got " .. type(dofile))
assert(loadfile == nil, "loadfile must be nil, got " .. type(loadfile))
assert(load == nil, "load must be nil, got " .. type(load))
-- base 本身不能没：pcall/assert/type 都是 base_funcs 的成员
assert(type(pcall) == "function", "pcall must survive")
assert(type(assert) == "function", "assert must survive")
"#,
        );
        assert!(result.is_ok(), "沙箱基线断言失败：{result:?}");
    }

    #[test]
    fn sandbox_cannot_require_os_or_io() {
        // PACKAGE 在沙箱构造点（sandbox_lua）上是启用的（插件要 require("hw.codec")），
        // 所以 require 是真实逃逸面。
        let result = run_in_scratch_vm(
            r#"
-- 真风险不在"require 报不报错"上：require 只有在标准库被装进 package.loaded 之后才可能拿到
-- 活模块，那是 stdlib 位集的**后果**，不是沙箱边界本身。原来的写法把后果当判据：抹掉
-- dofile/loadfile/load、放松 searcher 收紧、留不留 package.searchpath —— 它一种都看不见。
-- 判据直接查边界本身：这些库既不许是活的 global，也不许留在 package.loaded 里 —— 只抹
-- global 却留着 package.loaded 的话，require("os") 照样返回活表。
local widened = {}
for _, name in ipairs({"os", "io", "debug", "ffi"}) do
    local live_global = rawget(_G, name)
    local live_loaded = package.loaded and package.loaded[name]
    if live_global ~= nil or live_loaded ~= nil then
        widened[#widened + 1] = string.format(
            "%s: global=%s package.loaded=%s", name, type(live_global), type(live_loaded))
    end
end
assert(#widened == 0,
    "stdlib modules must be absent from the sandbox, got: " .. table.concat(widened, ", "))
-- package.path 对插件可写；只要 Lua 文件 searcher 还在，require 就会按被改写的
-- 模板去 open 宿主磁盘上的 .lua（searcher_Lua -> luaL_loadfile）。探针目录不必
-- 真的存在：searcher 会把逐个试过的文件名写进错误串，据此判定碰没碰文件系统。
package.path = "/hwbench-t2-sandbox-probe/?.lua"
local probe_ok, probe_err = pcall(require, "hwbench_t2_sandbox_probe")
assert(probe_ok == false, "require must not resolve modules from the filesystem")
assert(not tostring(probe_err):find("hwbench-t2-sandbox-probe", 1, true),
    "require still probes host paths: " .. tostring(probe_err))
-- package.searchpath 是那个 searcher 底层的解析原语，摘掉 searcher 并不拿走它：
-- 插件仍可直接调用它拿"宿主某路径下有没有这个文件"的答案。断言不存在，不报错。
assert(package.searchpath == nil,
    "package.searchpath must be nil, got " .. type(package.searchpath))
-- 收紧 searcher 不能顺手弄坏宿主自己注册进 preload 的模块。
assert(pcall(require, "hw.codec") == true,
    "require('hw.codec') must still resolve from the preload searcher")
"#,
        );
        assert!(result.is_ok(), "require 逃逸用例失败：{result:?}");
    }

    #[test]
    fn sandbox_cannot_rebind_globals_to_escape_harden() {
        // load(string) 被抹掉后，插件不得还有办法从字符串造 chunk。
        let result = run_in_scratch_vm(
            r#"
assert(load == nil, "load must be nil, got " .. type(load))
assert(loadfile == nil, "loadfile must be nil, got " .. type(loadfile))
assert(dofile == nil, "dofile must be nil, got " .. type(dofile))
-- 原来这里是一句 pcall(loadstring("return 1")) == false：loadstring 在 Lua 5.4 已被删除，
-- 调用一个 nil 全局必然报错，它挡不住任何东西（零断言）。字符串→chunk 的真判据就是上面的
-- load 那一行 —— 5.4 里从字符串造 chunk 的入口只剩 load，没有别的拼法可查。
assert(package.loadlib == nil or pcall(package.loadlib, "x.so", "y") == false)
"#,
        );
        assert!(result.is_ok(), "字符串→chunk 逃逸未被挡住：{result:?}");
    }

    /// 第三处插件可达 VM（`replay.rs` 的 replay analyzer，跑第三方 `replay.lua`）必须与
    /// 主沙箱同加固。它此前是裸 `Lua::new_with`：`dofile`/`loadfile`/`load` 全都活着，
    /// 而且 `plugin_root` 为 `None` 时 `package.path` 停在 Lua 的默认值上 —— 那个默认值是
    /// `setpath` 从进程环境里的 `LUA_PATH_5_4`/`LUA_PATH` 播种出来的（loadlib.c:288-311），
    /// 等于"这台机器恰好设过 LUA_PATH"会改变沙箱边界。收紧 searcher 把环境相关性一并抹平，
    /// 所以下面的判据只看 require 的报错里有没有文件探测的痕迹（`no file`），不引用任何
    /// 具体路径，也不碰磁盘。
    #[test]
    fn replay_sandbox_has_no_file_loaders() {
        let source = r#"
assert(dofile == nil, "replay: dofile must be nil, got " .. type(dofile))
assert(loadfile == nil, "replay: loadfile must be nil, got " .. type(loadfile))
assert(load == nil, "replay: load must be nil, got " .. type(load))
assert(package.searchpath == nil,
    "replay: package.searchpath must be nil, got " .. type(package.searchpath))
-- base 不能整个没掉：pcall/assert 还在，说明是逐抹而非断粮
assert(type(pcall) == "function", "replay: pcall must survive")
package.path = "/hwbench-t2-replay-probe/?.lua"
local probe_ok, probe_err = pcall(require, "hwbench_t2_replay_probe")
assert(probe_ok == false, "replay: require must not resolve modules from the filesystem")
assert(not tostring(probe_err):find("no file", 1, true),
    "replay: require still probes host paths: " .. tostring(probe_err))
assert(pcall(require, "hw.codec") == true,
    "replay: require('hw.codec') must still resolve from the preload searcher")
function on_replay_begin(session) end
function on_replay_event(event) end
function on_replay_end() end
"#;
        let config = LuaReplayConfig {
            script_name: "harden_probe.lua".to_owned(),
            plugin_id: "test.harden".to_owned(),
            plugin_version: "1.0.0".to_owned(),
            subscriptions: vec![serial_topics::SERIAL_RX.to_owned()],
            outputs: vec![],
            context: json!({"id": "test.harden", "name": "Harden"}),
            plugin_root: None,
        };
        let output = crate::replay::run_replay_analyzer(source.to_owned(), config, &[]);
        let output = match output {
            Ok(output) => output,
            Err(error) => panic!("replay 沙箱断言失败：{error}"),
        };
        assert!(
            output.events.is_empty(),
            "replay 沙箱用例不该产出事件：{:?}",
            output.events
        );
        assert!(
            output.logs.is_empty(),
            "replay 沙箱用例不应产生回调错误：{:?}",
            output.logs
        );
    }

    /// 四处插件可达 VM 里的最后一处 —— `plugin_event_loop`（`run_plugin` 起的常驻线程，真实
    /// 插件全都走这条）此前只靠"它和别人共用 `sandbox_lua`"这条传递论证活着，自己没有任何
    /// 行为背垫：字符串守卫一旦被同义拼法绕过（`Lua::default()` 就是第一种），红起来的只会
    /// 是别的站点的测试。这里让它自己红。
    ///
    /// 判据是线程自己写的结局（`outcome`），不是时序：顶层断言失败会走 fail-closed 分支，结局
    /// 变成 `Failed`；沙箱正常则脚本跑完、以 `Finished` 收尾。失败时再去 `log.system` 取
    /// `脚本错误：…`，把 Lua 的断言原文带进 panic 信息（线程先写结局、后发日志，所以取日志
    /// 需要一点宽限；它只影响报错文案，不影响判定）。
    #[test]
    fn plugin_event_loop_runs_lua_in_the_hardened_sandbox() {
        let bus = DataBus::new();
        let system_logs = bus.subscribe(TopicFilter::exact(topics::LOG_SYSTEM));
        let transport = TransportManager::new(bus.clone());
        let host_services = test_host_services("sandbox-probe");

        let runtime = run_plugin(
            r#"
assert(dofile == nil, "event loop: dofile must be nil, got " .. type(dofile))
assert(loadfile == nil, "event loop: loadfile must be nil, got " .. type(loadfile))
assert(load == nil, "event loop: load must be nil, got " .. type(load))
assert(package.searchpath == nil,
    "event loop: package.searchpath must be nil, got " .. type(package.searchpath))
-- base 不能整个没掉：pcall/assert 还在，说明是逐抹而非断粮
assert(type(pcall) == "function", "event loop: pcall must survive")
"#
            .to_owned(),
            LuaRunConfig {
                script_name: "harden_probe.lua".to_owned(),
                timeout_ms: 5_000,
                source: "plugin:sandbox-probe".to_owned(),
                context: json!({"id": "sandbox-probe"}),
                permissions: vec![],
            },
            bus.clone(),
            transport,
            host_services,
        )
        .expect("插件运行时应能启动");

        let mut script_errors: Vec<String> = Vec::new();
        let collect_errors = |errors: &mut Vec<String>| {
            for event in system_logs.drain() {
                let text = event.payload.text_lossy();
                if text.contains("脚本错误") {
                    errors.push(text);
                }
            }
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let outcome = loop {
            collect_errors(&mut script_errors);
            if let Some(outcome) = runtime.outcome() {
                break outcome;
            }
            assert!(
                Instant::now() < deadline,
                "插件事件循环未在 5 秒内给出结局；已收到的系统日志：{script_errors:?}"
            );
            thread::sleep(Duration::from_millis(5));
        };
        if outcome == LuaRunState::Failed {
            let grace = Instant::now() + Duration::from_millis(500);
            while script_errors.is_empty() && Instant::now() < grace {
                collect_errors(&mut script_errors);
                thread::sleep(Duration::from_millis(5));
            }
        }
        assert_eq!(
            outcome,
            LuaRunState::Finished,
            "插件事件循环沙箱断言失败，脚本错误日志：{script_errors:?}"
        );
    }

    /// 逐行扫描 Rust 源码，产出「生产代码」的 `(行号, 去掉注释与字面量后的文本)`。
    ///
    /// 静态守卫全靠它，所以扫描本身必须可信，两类误判都得挡住：
    /// - 行注释、跨行块注释、字符串与字符字面量里出现的 `Lua::new()` 不算违规；
    /// - `mod tests { ... }` 整块跳过 —— 这是唯一的豁免，且只豁免测试模块本身：
    ///   `bundled_gcode_sender_lua_tests` 需要一个不加沙箱的 VM，因为
    ///   `plugins/gcode-sender/tests/main_test.lua` 用 `dofile` 定位插件 main.lua，
    ///   `convert.rs` 的纯转换用例同理；它们是测试夹具，不是插件能碰到的运行时。
    ///   跨行的 `/* */` 与 `r#"..."#` 状态会带到下一行，判定不会因换行而丢。
    fn production_code_lines(text: &str) -> Vec<(usize, String)> {
        fn raw_string_ends_at(chars: &[char], index: usize, hashes: usize) -> bool {
            chars.get(index) == Some(&'"')
                && (1..=hashes).all(|offset| chars.get(index + offset) == Some(&'#'))
        }

        let mut lines = Vec::new();
        let mut depth = 0i32;
        let mut block_comment = 0usize;
        let mut raw_hashes: Option<usize> = None;
        let mut in_string = false;
        let mut in_test_module = false;

        for (number, raw_line) in text.lines().enumerate() {
            let chars: Vec<char> = raw_line.chars().collect();
            let mut code = String::new();
            let mut after_mod = false;
            let mut index = 0usize;

            while index < chars.len() {
                let ch = chars[index];

                if block_comment > 0 {
                    let next = chars.get(index + 1).copied();
                    if ch == '*' && next == Some('/') {
                        block_comment -= 1;
                        index += 2;
                    } else if ch == '/' && next == Some('*') {
                        block_comment += 1;
                        index += 2;
                    } else {
                        index += 1;
                    }
                    code.push(' ');
                    continue;
                }
                if let Some(hashes) = raw_hashes {
                    if raw_string_ends_at(&chars, index, hashes) {
                        raw_hashes = None;
                        index += 1 + hashes;
                    } else {
                        index += 1;
                    }
                    code.push(' ');
                    continue;
                }
                if in_string {
                    if ch == '\\' {
                        index += 2;
                    } else {
                        if ch == '"' {
                            in_string = false;
                        }
                        index += 1;
                    }
                    code.push(' ');
                    continue;
                }

                let next = chars.get(index + 1).copied();
                if ch == '/' && next == Some('/') {
                    break; // 行注释：本行剩余部分不是代码
                }
                if ch == '/' && next == Some('*') {
                    block_comment = 1;
                    index += 2;
                    code.push(' ');
                    continue;
                }
                if ch == '"' {
                    in_string = true;
                    index += 1;
                    code.push(' ');
                    continue;
                }
                if ch == '\'' {
                    // 字符字面量 vs 生命周期：只有形如 'x' / '\x' 才按字面量吃掉。
                    let closed_at = if next == Some('\\') {
                        chars
                            .get(index + 3..)
                            .and_then(|rest| rest.iter().position(|c| *c == '\''))
                            .map(|offset| index + 4 + offset)
                    } else if chars.get(index + 2) == Some(&'\'') {
                        Some(index + 3)
                    } else {
                        None
                    };
                    if let Some(end) = closed_at {
                        index = end;
                        code.push(' ');
                    } else {
                        code.push('\'');
                        index += 1;
                    }
                    continue;
                }
                if ch.is_ascii_alphabetic() || ch == '_' {
                    let start = index;
                    while index < chars.len()
                        && (chars[index].is_ascii_alphanumeric() || chars[index] == '_')
                    {
                        index += 1;
                    }
                    let token: String = chars[start..index].iter().collect();
                    // r"..."、r#"..."#、b"..."、br#"..."# —— 前缀必须紧贴引号。
                    let is_raw = matches!(token.as_str(), "r" | "br" | "rb");
                    let is_byte_string = token == "b";
                    if is_raw || is_byte_string {
                        let mut probe = index;
                        let mut hashes = 0usize;
                        while chars.get(probe) == Some(&'#') {
                            hashes += 1;
                            probe += 1;
                        }
                        if chars.get(probe) == Some(&'"') {
                            if is_byte_string && hashes == 0 {
                                in_string = true;
                            } else {
                                raw_hashes = Some(hashes);
                            }
                            index = probe + 1;
                            code.push(' ');
                            continue;
                        }
                    }
                    if token == "mod" {
                        after_mod = true;
                    } else {
                        if after_mod && token == "tests" {
                            in_test_module = true;
                        }
                        after_mod = false;
                    }
                    code.push_str(&token);
                    continue;
                }
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth <= 0 && in_test_module {
                            in_test_module = false;
                            depth = depth.max(0);
                        }
                    }
                    _ => {}
                }
                code.push(ch);
                index += 1;
            }

            if !in_test_module && !code.trim().is_empty() {
                lines.push((number + 1, code));
            }
        }
        lines
    }

    #[test]
    fn production_code_never_opens_all_stdlibs() {
        use std::path::Path;

        /// `fn <name> { … }` 在生产代码行序列里的闭区间（含首尾），靠花括号配对算出来。
        /// 规则 2/3 全靠它定位「唯一受制裁的 VM 构造点」，所以它自己也得上夹具。
        fn fn_span(lines: &[(usize, String)], name: &str) -> Option<(usize, usize)> {
            let header = format!("fn {name}");
            let start = lines.iter().find(|(_, code)| code.contains(&header))?.0;
            let mut depth = 0i32;
            let mut opened = false;
            for (number, code) in lines.iter().filter(|(number, _)| *number >= start) {
                for ch in code.chars() {
                    match ch {
                        '{' => {
                            depth += 1;
                            opened = true;
                        }
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
                if opened && depth <= 0 {
                    return Some((start, *number));
                }
            }
            None
        }

        /// 裸构造器的每一种拼法 —— 每一种都等价于"建了一台没人加固的 VM"。
        /// - `Lua::new()`：`StdLib::ALL_SAFE`，含 IO/OS（`os.execute` 可用）。
        /// - `Lua::default()`：mlua-0.11.6 `src/state.rs:170-174` 的 `impl Default for Lua`
        ///   就是 `fn default() -> Self { Lua::new() }`，所以它是上面那个 bug 的**字面同义词**；
        ///   而文本里既不含 `Lua::new()` 也不含 `Lua::new_with(`，只会撞在这一条上。
        /// - `unsafe_new`：裸词，同时兜住 `Lua::unsafe_new()` 与
        ///   `Lua::unsafe_new_with(StdLib::ALL, ..)` —— 比 ALL_SAFE 还宽，连 C 模块都放进来。
        /// - `new_with_options`：mlua 0.11.6 里还没有这个入口，它是别的版本里的第四种构造
        ///   拼法；将来升依赖时不能靠人记得补守卫。
        ///
        /// 受制裁的 `Lua::new_with(` 不在这里：它由规则 2 数次数、规则 3 查加固。
        ///
        /// 文本守卫的极限要说清楚：`let lua: Lua = Default::default();` 这种靠类型推断的写法
        /// 没有任何拼法可匹配。所以每一处插件可达的 VM 都另外配了一条行为背垫
        /// （`sandbox_*` 走 `run_script_blocking`、`plugin_event_loop_runs_lua_*`、
        /// `load_plugin_runs_lua_*`、`replay_sandbox_*`），拼法清单只是第一道网。
        const FORBIDDEN_CONSTRUCTORS: [&str; 4] = [
            "Lua::new()",
            "Lua::default(",
            "unsafe_new",
            "new_with_options",
        ];

        fn forbidden_constructor(code: &str) -> Option<&'static str> {
            FORBIDDEN_CONSTRUCTORS
                .iter()
                .find(|pattern| code.contains(**pattern))
                .copied()
        }

        fn flagged(text: &str) -> Vec<usize> {
            production_code_lines(text)
                .into_iter()
                .filter(|(_, code)| forbidden_constructor(code).is_some())
                .map(|(number, _)| number)
                .collect()
        }

        // 扫描器不可信 = 守卫零断言，所以先拿夹具自证：命中生产代码、忽略噪声、
        // 并且跳过测试模块后能恢复正常。
        assert_eq!(
            flagged(
                r#"
fn setup() {
    let lua = Lua::new();
}
"#
            ),
            vec![3],
            "扫描器漏掉了生产代码里的 Lua::new()"
        );
        assert_eq!(
            flagged(
                r#"
// 注释里提到 Lua::new() 不算违规
/* 块注释里的 Lua::new()
   跨行也一样 */
let hint = "字符串里的 Lua::new()";
mod tests {
    fn helper() { let _ = Lua::new(); }
}
"#
            ),
            Vec::<usize>::new(),
            "扫描器把注释/字符串/测试模块里的 Lua::new() 也算成了违规"
        );
        assert_eq!(
            flagged(
                r#"
mod tests {
    fn helper() { let _ = Lua::new(); }
}
let quote = '"';
let _ = Lua::new();
"#
            ),
            vec![6],
            "扫描器跳过 mod tests 后没恢复，或被 '\"' 字面量弄乱了状态"
        );
        // 同义词夹具：FORBIDDEN_CONSTRUCTORS 里每一种拼法都要真的能命中（漏一种 = 守卫对该
        // 写法形同虚设），同时受制裁的 `Lua::new_with(` 和满屏合法的 `LuaOptions::default()`
        // 不许被 `Lua::default(` 误伤。
        assert_eq!(
            flagged(
                r#"
fn plain() { let _ = Lua::new(); }
fn synonym() { let _ = Lua::default(); }
fn unsafe_plain() { let _ = unsafe { Lua::unsafe_new() }; }
fn unsafe_with() { let _ = unsafe { Lua::unsafe_new_with(StdLib::ALL, LuaOptions::default()) }; }
fn renamed_api() { let _ = Lua::new_with_options(StdLib::ALL_SAFE, LuaOptions::default()); }
fn sanctioned() { let _ = Lua::new_with(StdLib::TABLE, LuaOptions::default()); }
"#
            ),
            vec![2, 3, 4, 5, 6],
            "裸构造器的同义词拼法必须逐个点名；Lua::new_with( 与 LuaOptions::default() 不该被误伤"
        );

        // fn_span 是规则 2/3 的支点，先自证：区间恰好框住函数体，既不缩水也不外溢。
        let span_fixture = production_code_lines(
            r#"
fn helper() {
    let lua = Lua::new_with();
}
fn sandbox_lua() {
    let lua = Lua::new_with();
    harden_globals(&lua);
}
fn other() {
    let lua = Lua::new_with();
}
"#,
        );
        let span = fn_span(&span_fixture, "sandbox_lua").expect("夹具里就该找得到 sandbox_lua");
        assert_eq!(
            span,
            (5, 8),
            "fn_span 的花括号配对没框住 sandbox_lua 函数体"
        );
        let (inside_span, outside_span): (Vec<usize>, Vec<usize>) = span_fixture
            .iter()
            .filter(|(_, code)| code.contains("Lua::new_with("))
            .map(|(number, _)| *number)
            .partition(|number| *number >= span.0 && *number <= span.1);
        assert_eq!(
            inside_span,
            vec![6],
            "fn_span 把函数体内的构造点算到了区间外"
        );
        assert_eq!(
            outside_span,
            vec![3, 10],
            "fn_span 区间外溢到了相邻函数，会替漏网的 VM 打掩护"
        );

        // 三条规则共用同一份「生产代码」行集：
        // 1. 不许任何裸构造器：`Lua::new()` / `Lua::default()`（mlua 里它就是 `Lua::new()`）/
        //    `unsafe_new*` / `new_with_options` —— 全都等价于"开了一台没人加固的 VM"；
        // 2. 全 crate 只许一处 `Lua::new_with(`，且必须在 `sandbox_lua` 体内；
        // 3. `sandbox_lua` 自己必须调 `harden_globals`。
        // 规则 2 就是当初漏掉 replay.rs 的那道缝：那是一处 new_with(含 PACKAGE) 却没跟着
        // harden_globals 的 VM —— 拼法再怎么补也都看不见它，因为它没用裸构造器。把构造收敛到
        // 一处、把加固焊进那一处，"再开一个没加固的 VM" 才从"记得不记得"变成"过不过守卫"。
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut production: Vec<(String, usize, String)> = Vec::new();
        let mut checked = 0usize;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("读取 src 目录失败") {
                let path = entry.expect("目录项").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                checked += 1;
                let text = std::fs::read_to_string(&path).expect("读取源文件失败");
                let file = path.display().to_string();
                for (number, code) in production_code_lines(&text) {
                    production.push((file.clone(), number, code));
                }
            }
        }
        assert!(
            checked >= 20,
            "扫描到的 .rs 文件过少（{checked}），守卫可能失效"
        );

        let mut offenders: Vec<String> = production
            .iter()
            .filter_map(|(file, number, code)| {
                forbidden_constructor(code)
                    .map(|spelling| format!("{file}:{number} 用了 {spelling}"))
            })
            .collect();
        // read_dir 顺序由 OS 决定，报错信息按路径排序后才可复现。
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "生产代码不得用裸构造器打开未经加固的 Lua VM（拼法清单 {:?}）：{offenders:?}",
            FORBIDDEN_CONSTRUCTORS
        );

        let constructors: Vec<(String, usize)> = production
            .iter()
            .filter(|(_, _, code)| code.contains("Lua::new_with("))
            .map(|(file, number, _)| (file.clone(), *number))
            .collect();
        assert!(
            !constructors.is_empty(),
            "生产代码里一个 Lua::new_with 都没扫到，扫描器可能在空转"
        );
        assert_eq!(
            constructors.len(),
            1,
            "Lua VM 构造点必须收敛到 sandbox_lua 一处；多出来的每一处都没人保证加固过：{:?}",
            {
                let mut listed: Vec<String> = constructors
                    .iter()
                    .map(|(file, number)| format!("{file}:{number}"))
                    .collect();
                listed.sort();
                listed
            }
        );
        let (ctor_file, ctor_line) = constructors[0].clone();
        let ctor_lines: Vec<(usize, String)> = production
            .iter()
            .filter(|(file, _, _)| *file == ctor_file)
            .map(|(_, number, code)| (*number, code.clone()))
            .collect();
        let sanctioned = fn_span(&ctor_lines, "sandbox_lua").unwrap_or_else(|| {
            panic!(
                "唯一的 Lua::new_with 在 {ctor_file}:{ctor_line}，却找不到 fn sandbox_lua \
                 —— 构造点没有受制裁的落点"
            )
        });
        assert!(
            sanctioned.0 <= ctor_line && ctor_line <= sanctioned.1,
            "Lua::new_with 出现在受制裁构造点之外（{ctor_file}:{ctor_line}，sandbox_lua 覆盖 \
             {sanctioned:?}）：改成调用 sandbox_lua()，否则这个 VM 没人保证加固过"
        );
        assert!(
            ctor_lines.iter().any(|(number, code)| {
                (sanctioned.0..=sanctioned.1).contains(number) && code.contains("harden_globals(")
            }),
            "sandbox_lua 没调用 harden_globals(...)：沙箱构造点和裸 new_with 等价，全部插件 \
             VM 会同时失去加固"
        );
    }
}
