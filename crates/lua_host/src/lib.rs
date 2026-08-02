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
    PLUGIN_COMMANDS, TASK_CANCELLED, TASK_FINISHED, TASK_YIELD_OP, YIELD_DEADLINE_MS, YIELD_KIND,
    YIELD_READ_LINE, YIELD_SLEEP, YIELD_WAIT_PAUSED, YIELD_WRITE_LINE_AND_EXPECT,
};
use crate::host_services::line_buffer_key;
pub use config::ConfigStore;
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

    pub fn on_replay_event(&self, event: &Event) -> bool {
        if !self.alive.load(Ordering::Relaxed) {
            return false;
        }

        // 使用 try_send 而非 send：回放期间若插件处理慢，丢弃事件比阻塞 UI 线程安全。
        // 与 on_event 保持一致行为。
        self.event_sender.try_send(event.clone()).is_ok()
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
impl Drop for LuaPluginRuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);

        // 超时 join：防止 Lua 线程卡住时阻塞 UI 线程 Drop。
        // Lua 线程有指令 hook，正常情况几 ms 内响应 stop flag；
        // 超时后分离线程，让其自行结束。
        if let Some(join) = self.join.take() {
            const DROP_JOIN_TIMEOUT: Duration = Duration::from_millis(500);
            let deadline = std::time::Instant::now() + DROP_JOIN_TIMEOUT;
            while std::time::Instant::now() < deadline {
                if join.is_finished() {
                    let _ = join.join();
                    return;
                }
                std::thread::yield_now();
            }
            // 超时：分离线程，不再等待
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
        let run_source = config.source.clone();

        bus.publish(Event::system_log(
            LogLevel::Info,
            run_source,
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

            match result {
                Ok(()) => {
                    *thread_outcome.lock() = Some(LuaRunState::Finished);

                    bus.publish(Event::system_log(
                        LogLevel::Info,
                        config.source,
                        format!("{} 已完成", config.script_name),
                    ));
                }
                Err(error) if thread_stop.load(Ordering::Relaxed) => {
                    *thread_outcome.lock() = Some(LuaRunState::Stopped);

                    bus.publish(Event::system_log(
                        LogLevel::Warn,
                        config.source,
                        format!("{} 已停止：{error}", config.script_name),
                    ));
                }
                Err(error) => {
                    *thread_outcome.lock() = Some(LuaRunState::Failed);

                    bus.publish(Event::system_log(
                        LogLevel::Error,
                        config.source,
                        format!("{} 失败：{error}", config.script_name),
                    ));
                }
            }

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
            .map(|worker| worker.finished.load(Ordering::Relaxed))
            .unwrap_or(false);

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

        // 超时 join（同 LuaPluginRuntime::drop 的策略）
        if let Some(mut worker) = self.worker.take()
            && let Some(join) = worker.join.take()
        {
            const DROP_JOIN_TIMEOUT: Duration = Duration::from_millis(500);
            let deadline = std::time::Instant::now() + DROP_JOIN_TIMEOUT;
            while std::time::Instant::now() < deadline {
                if join.is_finished() {
                    let _ = join.join();
                    return;
                }
                std::thread::yield_now();
            }
            // 超时：分离线程
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

    let plugin_source = config.source.clone();

    let mut host_services = host_services;
    host_services.stop_flag = Some(Arc::clone(&thread_stop));

    bus.publish(Event::system_log(
        LogLevel::Info,
        &plugin_source,
        format!("正在启动插件 {}", config.script_name),
    ));

    let thread_outcome = Arc::new(ParkingMutex::new(None));
    let outcome_for_thread = Arc::clone(&thread_outcome);

    let outcome_in_thread = Arc::clone(&outcome_for_thread);
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
            outcome_in_thread,
        );
        // plugin_event_loop 在错误路径已设置 Failed；这里做兜底
        {
            let mut guard = outcome_for_thread.lock();
            if guard.is_none() {
                *guard = Some(LuaRunState::Finished);
            }
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
    let lua = match Lua::new_with(
        StdLib::TABLE
            | StdLib::STRING
            | StdLib::MATH
            | StdLib::UTF8
            | StdLib::PACKAGE
            | StdLib::COROUTINE,
        LuaOptions::default(),
    ) {
        Ok(lua) => lua,
        Err(error) => {
            *outcome.lock() = Some(LuaRunState::Failed);
            bus.publish(Event::system_log(
                LogLevel::Error,
                &config.source,
                format!("创建 Lua 状态失败：{error}"),
            ));
            alive.store(false, Ordering::Relaxed);
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
        *outcome.lock() = Some(LuaRunState::Failed);
        bus.publish(Event::system_log(
            LogLevel::Error,
            &config.source,
            format!("设置指令 hook 失败：{e}"),
        ));
        alive.store(false, Ordering::Relaxed);
        return;
    }

    if let Err(error) = install_ctx(&lua, bus.clone(), transport, &config, &host_services) {
        *outcome.lock() = Some(LuaRunState::Failed);
        bus.publish(Event::system_log(
            LogLevel::Error,
            &config.source,
            format!("安装上下文失败：{error}"),
        ));
        alive.store(false, Ordering::Relaxed);
        return;
    }

    // 注入 task 辅助函数（必须在用户脚本之前）
    if let Err(error) = install_task_helpers(&lua) {
        *outcome.lock() = Some(LuaRunState::Failed);
        bus.publish(Event::system_log(
            LogLevel::Error,
            &config.source,
            format!("安装任务辅助函数失败：{error}"),
        ));
        alive.store(false, Ordering::Relaxed);
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
        *outcome.lock() = Some(LuaRunState::Failed);
        bus.publish(Event::system_log(
            LogLevel::Error,
            &config.source,
            format!("脚本错误：{error}"),
        ));
        alive.store(false, Ordering::Relaxed);
        return;
    }

    let has_callbacks = lua
        .globals()
        .get::<Table>(crate::globals::PLUGIN_CALLBACKS)
        .map(|table| !table.is_empty())
        .unwrap_or(false);

    let has_commands = lua
        .globals()
        .get::<Table>(crate::globals::PLUGIN_COMMANDS)
        .map(|table| !table.is_empty())
        .unwrap_or(false);

    let has_timers = lua
        .globals()
        .get::<Table>(crate::globals::PLUGIN_TIMERS)
        .map(|table| !table.is_empty())
        .unwrap_or(false);

    let has_tasks = lua
        .globals()
        .get::<Table>(crate::globals::PLUGIN_TASKS)
        .map(|t| {
            t.pairs::<String, Table>()
                .filter_map(|p| p.ok())
                .any(|(_, state)| !state.get::<bool>("finished").unwrap_or(true))
        })
        .unwrap_or(false);

    if !has_callbacks && !has_commands && !has_timers && !has_tasks {
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

                if let Some(callback) = get_callback(&lua, &event.topic) {
                    let event_table = lua.create_table().ok();

                    if let Some(event_table) = event_table {
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
            }
            None => {}
            Some(Err(())) => break,
        }

        let timers_empty = lua
            .globals()
            .get::<Table>(crate::globals::PLUGIN_TIMERS)
            .map(|table| table.is_empty())
            .unwrap_or(true);

        let callbacks_empty = lua
            .globals()
            .get::<Table>(crate::globals::PLUGIN_CALLBACKS)
            .map(|table| table.is_empty())
            .unwrap_or(true);

        let commands_empty = lua
            .globals()
            .get::<Table>(crate::globals::PLUGIN_COMMANDS)
            .map(|table| table.is_empty())
            .unwrap_or(true);

        let tasks_all_done = lua
            .globals()
            .get::<Table>(crate::globals::PLUGIN_TASKS)
            .map(|t| {
                t.pairs::<String, Table>()
                    .filter_map(|p| p.ok())
                    .all(|(_, state)| state.get::<bool>("finished").unwrap_or(true))
            })
            .unwrap_or(true);

        if timers_empty
            && callbacks_empty
            && commands_empty
            && tasks_all_done
            && event_receiver.is_empty()
        {
            break;
        }
    }

    alive.store(false, Ordering::Relaxed);
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
    let callbacks: Table = lua.globals().get(crate::globals::PLUGIN_CALLBACKS).ok()?;

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
    let timers: Table = lua.globals().get(crate::globals::PLUGIN_TIMERS).ok()?;
    let now_ms = tool_core::now_timestamp_ms();

    let mut next_trigger_at = u64::MAX;

    for (_, timer) in timers.pairs::<String, Table>().flatten() {
        let trigger_at_ms: u64 = timer.get("trigger_at_ms").unwrap_or(u64::MAX);
        next_trigger_at = next_trigger_at.min(trigger_at_ms);
    }

    if next_trigger_at == u64::MAX {
        return None;
    }

    if next_trigger_at <= now_ms {
        Some(Duration::from_millis(0))
    } else {
        Some(Duration::from_millis(next_trigger_at - now_ms))
    }
}

fn next_task_wait(lua: &Lua) -> Option<Duration> {
    let tasks: Table = lua.globals().get(crate::globals::PLUGIN_TASKS).ok()?;
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
            YIELD_WAIT_PAUSED => {}
            _ => {}
        }
    }

    if next_wake_at == u64::MAX {
        None
    } else if next_wake_at <= now_ms {
        Some(Duration::ZERO)
    } else {
        Some(Duration::from_millis(next_wake_at - now_ms))
    }
}

fn min_wait(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(wait), None) | (None, Some(wait)) => Some(wait),
        (None, None) => None,
    }
}

fn process_timers(lua: &Lua, bus: &DataBus, config: &LuaRunConfig) {
    let timers: Table = match lua.globals().get(crate::globals::PLUGIN_TIMERS) {
        Ok(timers) => timers,
        Err(_) => return,
    };

    let now_ms = tool_core::now_timestamp_ms();
    let mut expired = Vec::new();

    for (id, timer) in timers.pairs::<String, Table>().flatten() {
        let trigger_at_ms: u64 = timer.get("trigger_at_ms").unwrap_or(u64::MAX);

        if now_ms >= trigger_at_ms {
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
    let lua = Lua::new_with(
        StdLib::TABLE
            | StdLib::STRING
            | StdLib::MATH
            | StdLib::UTF8
            | StdLib::PACKAGE
            | StdLib::COROUTINE,
        LuaOptions::default(),
    )?;

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

    lua.globals()
        .set(crate::globals::PLUGIN_CALLBACKS, lua.create_table()?)?;
    lua.globals()
        .set(crate::globals::PLUGIN_COMMANDS, lua.create_table()?)?;
    lua.globals()
        .set(crate::globals::PLUGIN_TIMERS, lua.create_table()?)?;
    lua.globals()
        .set(crate::globals::PLUGIN_STORAGE, lua.create_table()?)?;
    lua.globals()
        .set(crate::globals::PLUGIN_TASKS, lua.create_table()?)?;

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
        let storage_api = create_storage_api(lua)?;
        ctx.set("session", storage_api)?;
    }

    if has_permission(config, "dialog")
        && let Some(sender) = host_services.dialog_sender.clone()
    {
        let stop = host_services.stop_flag.clone();
        ctx.set(
            "dialog",
            create_dialog_api(lua, sender, host_services.plugin_id.clone(), stop)?,
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

    if let Some(ref root) = host_services.plugin_root {
        let root_str = root.display().to_string().replace('\\', "/");
        let new_path = format!("{root_str}/lib/?.lua;{root_str}/?.lua");
        if let Ok(package) = lua.globals().get::<Table>("package") {
            let _ = package.set("path", new_path);
            let _ = package.set("cpath", "");
        }
    } else {
        // 无 plugin_root 时（如临时脚本/测试），移除 package.path 阻止 require 非预期路径
        if let Ok(package) = lua.globals().get::<Table>("package") {
            let _ = package.set("path", "");
            let _ = package.set("cpath", "");
        }
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
        for pair in preload_table.pairs::<String, Function>().flatten() {
            frozen.set(pair.0, pair.1)?;
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
        lua.create_function(|lua, function: Function| {
            lua.globals().set(crate::globals::PLUGIN_DISABLE, function)
        })?,
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
    use tool_databus::TopicFilter;
    use tool_transport::serial_rx_event;

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
        let host_services = LuaHostServices {
            plugin_root: None,
            plugin_id: "cmd.plugin".to_owned(),
            dialog_sender: None,
            file_broker: None,
            stop_flag: None,
            line_buffers: None,
            config_store: None,
            declared_panel_ids: Default::default(),
        };

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

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_command = false;
        while Instant::now() < deadline {
            if let Ok(event) = logs.recv_timeout(Duration::from_millis(50))
                && event.payload.text_lossy().contains("command:ok")
            {
                saw_command = true;
                break;
            }
        }
        assert!(saw_command, "registered command handler was not invoked");
    }

    #[test]
    fn serial_blocking_wrappers_are_exported() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());

        run_script_for_test(
            r#"
assert(type(ctx.serial.read_line) == "function", type(ctx.serial.read_line))
assert(type(ctx.serial.write_line_and_expect) == "function", type(ctx.serial.write_line_and_expect))
"#,
            bus,
            transport,
        )
        .unwrap();
    }

    #[test]
    fn serial_read_line_uses_internal_rx_subscription() {
        let bus = DataBus::new();
        let logs = bus.subscribe(TopicFilter::prefix("log."));
        let transport = TransportManager::new(bus.clone());
        let host_services = LuaHostServices {
            plugin_root: None,
            plugin_id: "test-plugin".to_owned(),
            dialog_sender: None,
            file_broker: None,
            stop_flag: None,
            line_buffers: Some(Arc::new(ParkingMutex::new(HashMap::new()))),
            config_store: None,
            declared_panel_ids: Default::default(),
        };

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

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_ready = false;
        while Instant::now() < deadline {
            if let Ok(event) = logs.recv_timeout(Duration::from_millis(50))
                && event.payload.text_lossy().contains("reader-ready")
            {
                saw_ready = true;
                break;
            }
        }
        assert!(saw_ready, "plugin did not start read task");

        bus.publish(serial_rx_event("serial:COM1", b"ok\n".to_vec()));

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_read = false;
        while Instant::now() < deadline {
            if let Ok(event) = logs.recv_timeout(Duration::from_millis(50))
                && event.payload.text_lossy().contains("read:ok")
            {
                saw_read = true;
                break;
            }
        }
        assert!(saw_read, "ctx.serial.read_line did not receive internal RX");
    }

    #[test]
    fn serial_continue_response_can_reset_inactivity_timeout() {
        let bus = DataBus::new();
        let logs = bus.subscribe(TopicFilter::prefix("log."));
        let transport = TransportManager::new(bus.clone());
        let virtual_port = transport
            .open_virtual_serial("COM1")
            .expect("open virtual serial");
        let host_services = LuaHostServices {
            plugin_root: None,
            plugin_id: "busy-plugin".to_owned(),
            dialog_sender: None,
            file_broker: None,
            stop_flag: None,
            line_buffers: Some(Arc::new(ParkingMutex::new(HashMap::new()))),
            config_store: None,
            declared_panel_ids: Default::default(),
        };

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

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_ready = false;
        while Instant::now() < deadline {
            if let Ok(event) = logs.recv_timeout(Duration::from_millis(50))
                && event.payload.text_lossy().contains("sender-ready")
            {
                saw_ready = true;
                break;
            }
        }
        assert!(saw_ready, "plugin did not start expect task");

        thread::sleep(Duration::from_millis(250));
        virtual_port.inject_rx(b"echo:busy: processing\n".to_vec());
        thread::sleep(Duration::from_millis(250));
        virtual_port.inject_rx(b"ok\n".to_vec());

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut result = None;
        while Instant::now() < deadline {
            if let Ok(event) = logs.recv_timeout(Duration::from_millis(50)) {
                let text = event.payload.text_lossy();
                if text.contains("expect:") {
                    result = Some(text);
                    break;
                }
            }
        }
        assert_eq!(result.as_deref(), Some("expect:ok"));
    }

    #[test]
    fn task_start_sets_current_task_id_on_first_resume() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut permissions = default_lua_permissions();
        permissions.push("task".to_owned());

        run_script_blocking(
            r#"
local task = ctx.task.start({ id = "instant" }, function()
    assert(__current_task_id == "instant", tostring(__current_task_id))
end)
assert(task.finished == true)
"#
            .to_owned(),
            LuaRunConfig {
                script_name: "task-first-resume.lua".to_owned(),
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
    fn task_sleep_yields_from_lua_wrapper_inside_pcall() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut permissions = default_lua_permissions();
        permissions.push("task".to_owned());

        run_script_blocking(
            r#"
local task = ctx.task.start({ id = "sleepy" }, function(task)
    pcall(function()
        task:sleep_ms(10)
    end)
end)
assert(task.finished == false)
"#
            .to_owned(),
            LuaRunConfig {
                script_name: "task-sleep-yield.lua".to_owned(),
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
    fn next_task_wait_tracks_sleep_wake_time() {
        let lua = Lua::new();
        let tasks = lua.create_table().unwrap();
        lua.globals()
            .set(crate::globals::PLUGIN_TASKS, tasks.clone())
            .unwrap();

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
            subscriptions: vec!["transport.serial.default.rx".to_owned()],
            outputs: vec![],
            context: json!({"id": "test", "name": "Test"}),
            plugin_root: None,
        };

        let input = Event::new(
            "transport.serial.default.rx",
            "serial:COM2",
            tool_core::Direction::Rx,
            Payload::Text("hello".to_owned()),
        );

        let output =
            crate::replay::run_replay_analyzer(source.to_owned(), config, &[input]).unwrap();
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
            subscriptions: vec!["transport.serial.default.rx".to_owned()],
            outputs: vec![],
            context: json!({"id": "test", "name": "Test"}),
            plugin_root: None,
        };

        let input = Event::new(
            "transport.serial.default.rx",
            "serial:COM2",
            tool_core::Direction::Rx,
            Payload::Text("hello".to_owned()),
        );

        let output =
            crate::replay::run_replay_analyzer(source.to_owned(), config, &[input]).unwrap();
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
            subscriptions: vec!["transport.serial.default.rx".to_owned()],
            outputs: vec![],
            context: json!({"id": "demo.plugin", "name": "Demo", "version": "2.0.0"}),
            plugin_root: None,
        };

        let input = Event::new(
            "transport.serial.default.rx",
            "serial:COM2",
            tool_core::Direction::Rx,
            Payload::Text("test".to_owned()),
        );

        let output = crate::replay::run_replay_analyzer(
            source.to_owned(),
            config,
            std::slice::from_ref(&input),
        )
        .unwrap();
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
            subscriptions: vec!["transport.serial.default.rx".to_owned()],
            outputs: vec![],
            context: json!({"id": "test.lifecycle", "name": "Lifecycle"}),
            plugin_root: None,
        };

        let input1 = Event::new(
            "transport.serial.default.rx",
            "serial:COM2",
            tool_core::Direction::Rx,
            Payload::Text("a".to_owned()),
        );
        let input2 = Event::new(
            "transport.serial.default.rx",
            "serial:COM2",
            tool_core::Direction::Rx,
            Payload::Text("b".to_owned()),
        );

        let output =
            crate::replay::run_replay_analyzer(source.to_owned(), config, &[input1, input2])
                .unwrap();

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
            subscriptions: vec!["transport.serial.default.rx".to_owned()],
            outputs: vec![],
            context: json!({"id": "test.skip", "name": "Skip"}),
            plugin_root: None,
        };

        // 只有 1 个匹配的 RX 事件，另 1 个是 TX
        let rx = Event::new(
            "transport.serial.default.rx",
            "serial:COM2",
            tool_core::Direction::Rx,
            Payload::Text("rx".to_owned()),
        );
        let tx = Event::new(
            "transport.serial.default.tx",
            "serial:COM2",
            tool_core::Direction::Tx,
            Payload::Text("tx".to_owned()),
        );

        let output =
            crate::replay::run_replay_analyzer(source.to_owned(), config, &[rx, tx]).unwrap();
        assert_eq!(
            output.events.len(),
            1,
            "should only emit for matched RX event"
        );
    }

    #[test]
    fn sandbox_package_preload_is_read_only() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());

        // 尝试写入 package.preload 应该被 metatable 拦截
        let result = run_script_for_test(
            r#"
local ok, err = pcall(function()
    package.preload.evil = function() return "pwned" end
end)
assert(not ok, "package.preload must be read-only, got success")
assert(err ~= nil, "expected error message")
"#,
            bus,
            transport,
        );
        assert!(result.is_ok(), "sandbox test should pass: {result:?}");
    }

    #[test]
    fn sandbox_dofile_not_available() {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());

        // dofile 不应在沙箱中可用 (StdLib::BASE 未启用)
        let result = run_script_for_test(
            r#"
local ok, err = pcall(function()
    dofile("secret.txt")
end)
assert(not ok, "dofile must not be available")
"#,
            bus,
            transport,
        );
        assert!(
            result.is_ok(),
            "dofile sandbox test should pass: {result:?}"
        );
    }
}
