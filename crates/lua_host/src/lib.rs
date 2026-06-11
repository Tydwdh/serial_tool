use crossbeam_channel::{Receiver, Sender, bounded};
use mlua::{Function, Lua, LuaOptions, StdLib, Table, Thread, Value, VmState};
use parking_lot::Mutex as ParkingMutex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;
use tool_core::{Direction, Event, LogLevel, Payload, topic_matches, topics};
use tool_databus::{DataBus, TopicFilter};

pub mod codec;
pub mod config;
pub use config::ConfigStore;
use tool_testing::TestPacketLog;
use tool_transport::{SerialConfig, TransportManager};

// ── Host Services ──

#[derive(Debug, Clone)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

pub struct DialogRequest {
    pub plugin_id: String,
    pub title: String,
    pub filters: Vec<FileFilter>,
    pub response_sender: crossbeam_channel::Sender<Option<PathBuf>>,
}

/// 跨组件共享的文件访问授权管理器。
#[derive(Debug, Default)]
pub struct FileAccessBroker {
    authorized: parking_lot::Mutex<HashMap<String, HashSet<PathBuf>>>,
}

fn canonical_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl FileAccessBroker {
    pub fn authorize(&self, plugin_id: &str, path: PathBuf) {
        let canonical = canonical_path(&path);
        self.authorized
            .lock()
            .entry(plugin_id.to_owned())
            .or_default()
            .insert(canonical);
    }

    pub fn is_authorized(&self, plugin_id: &str, path: &Path) -> bool {
        let canonical = canonical_path(path);
        self.authorized
            .lock()
            .get(plugin_id)
            .map(|paths| paths.contains(&canonical))
            .unwrap_or(false)
    }

    pub fn clear(&self, plugin_id: &str) {
        self.authorized.lock().remove(plugin_id);
    }
}

/// 按 plugin_id + port_name 隔离的行缓冲区
#[derive(Debug, Clone)]
pub struct LineBuffer {
    pub lines: VecDeque<String>,
    raw: Vec<u8>,
    pub max_buffer_bytes: usize,
    pub max_line_bytes: usize,
}

impl Default for LineBuffer {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            raw: Vec::new(),
            max_buffer_bytes: 256 * 1024,
            max_line_bytes: 16 * 1024,
        }
    }
}

impl LineBuffer {
    /// 喂入原始字节，拆分完整行。超出容量时丢弃最老的行。
    fn feed(&mut self, data: &[u8]) {
        // 防止无换行长流撑爆 raw buffer
        if self.raw.len() + data.len() > self.max_buffer_bytes {
            // 先尝试丢弃已解析的完整行
            while self.raw.len() + data.len() > self.max_buffer_bytes && !self.lines.is_empty() {
                self.lines.pop_front();
            }
            // 如果仍然超限，截断 raw 头部
            if self.raw.len() + data.len() > self.max_buffer_bytes {
                let excess = self.raw.len() + data.len() - self.max_buffer_bytes;
                let drain_pos = excess.min(self.raw.len());
                self.raw.drain(..drain_pos);
            }
        }
        self.raw.extend_from_slice(data);

        // 按 \n 拆行
        while let Some(pos) = self.raw.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = self.raw.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes).to_string();
            let trimmed = line
                .trim_end_matches('\r')
                .trim_end_matches('\n')
                .to_owned();
            if trimmed.len() <= self.max_line_bytes {
                self.lines.push_back(trimmed);
            } else {
                // UTF-8 安全截断：使用 chars() 迭代器
                let truncated: String = trimmed.chars().take(self.max_line_bytes).collect();
                self.lines.push_back(truncated);
            }
        }
    }

    fn next_line(&mut self) -> Option<String> {
        self.lines.pop_front()
    }
}

/// 跨组件共享的行缓冲区映射。
pub type LineBufferMap = Arc<ParkingMutex<HashMap<String, LineBuffer>>>;

fn line_buffer_key(plugin_id: &str, port_name: &str) -> String {
    format!("{plugin_id}:{port_name}")
}

/// 传递给 Lua runtime 的宿主服务。
pub struct LuaHostServices {
    pub plugin_root: Option<PathBuf>,
    pub plugin_id: String,
    pub dialog_sender: Option<crossbeam_channel::Sender<DialogRequest>>,
    pub file_broker: Option<Arc<FileAccessBroker>>,
    pub stop_flag: Option<Arc<AtomicBool>>,
    pub line_buffers: Option<LineBufferMap>,
    pub config_store: Option<Arc<ConfigStore>>,
}

const LUA_PLUGIN_EVENT_QUEUE_CAPACITY: usize = 4096;

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

        self.event_sender.send(event.clone()).is_ok()
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

        if let Some(join) = self.join.take() {
            let _ = join.join();
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
            format!("running {}", config.script_name),
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
                        format!("{} finished", config.script_name),
                    ));
                }
                Err(error) if thread_stop.load(Ordering::Relaxed) => {
                    *thread_outcome.lock() = Some(LuaRunState::Stopped);

                    bus.publish(Event::system_log(
                        LogLevel::Warn,
                        config.source,
                        format!("{} stopped: {error}", config.script_name),
                    ));
                }
                Err(error) => {
                    *thread_outcome.lock() = Some(LuaRunState::Failed);

                    bus.publish(Event::system_log(
                        LogLevel::Error,
                        config.source,
                        format!("{} failed: {error}", config.script_name),
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
                .publish(Event::system_log(LogLevel::Warn, "lua", "stop requested"));
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

        if let Some(mut worker) = self.worker.take()
            && let Some(join) = worker.join.take()
        {
            let _ = join.join();
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
        format!("starting plugin {}", config.script_name),
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
        if outcome_for_thread.lock().is_none() {
            *outcome_for_thread.lock() = Some(LuaRunState::Finished);
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
                format!("failed to create lua: {error}"),
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
                return Err(mlua::Error::RuntimeError("plugin stopped".into()));
            }
            Ok(VmState::Continue)
        },
    ) {
        *outcome.lock() = Some(LuaRunState::Failed);
        bus.publish(Event::system_log(
            LogLevel::Error,
            &config.source,
            format!("failed to set hook: {e}"),
        ));
        alive.store(false, Ordering::Relaxed);
        return;
    }

    if let Err(error) = install_ctx(&lua, bus.clone(), transport, &config, &host_services) {
        *outcome.lock() = Some(LuaRunState::Failed);
        bus.publish(Event::system_log(
            LogLevel::Error,
            &config.source,
            format!("failed to install ctx: {error}"),
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
            format!("failed to install task helpers: {error}"),
        ));
        alive.store(false, Ordering::Relaxed);
        return;
    }

    if let Err(error) = lua.load(&source).set_name(&config.script_name).exec() {
        *outcome.lock() = Some(LuaRunState::Failed);
        bus.publish(Event::system_log(
            LogLevel::Error,
            &config.source,
            format!("script error: {error}"),
        ));
        alive.store(false, Ordering::Relaxed);
        return;
    }

    let has_callbacks = lua
        .globals()
        .get::<Table>("__plugin_callbacks")
        .map(|table| !table.is_empty())
        .unwrap_or(false);

    let has_timers = lua
        .globals()
        .get::<Table>("__plugin_timers")
        .map(|table| !table.is_empty())
        .unwrap_or(false);

    let has_tasks = lua
        .globals()
        .get::<Table>("__plugin_tasks")
        .map(|t| {
            t.pairs::<String, Table>()
                .filter_map(|p| p.ok())
                .any(|(_, state)| !state.get::<bool>("finished").unwrap_or(true))
        })
        .unwrap_or(false);

    if !has_callbacks && !has_timers && !has_tasks {
        bus.publish(Event::system_log(
            LogLevel::Info,
            &config.source,
            "plugin finished (no callbacks)",
        ));
        alive.store(false, Ordering::Relaxed);
        return;
    }

    loop {
        if stop.load(Ordering::Relaxed) {
            call_disable(&lua, &bus, &config);
            break;
        }

        process_timers(&lua, &bus, &config);
        process_tasks(&lua, &bus, &config, &host_services);

        let wait_duration = next_timer_wait(&lua).unwrap_or_else(|| Duration::from_millis(50));

        match event_receiver.recv_timeout(wait_duration.min(Duration::from_millis(50))) {
            Ok(event) => {
                // 将 serial_rx 数据喂入行缓冲区
                drain_serial_rx_to_buffers(&event, &host_services);

                if let Some(callback) = get_callback(&lua, &event.topic) {
                    let event_table = lua.create_table().ok();

                    if let Some(event_table) = event_table {
                        let _ = event_to_lua_table(&lua, &event_table, &event);

                        if let Err(error) = callback.call::<Value>(event_table) {
                            bus.publish(Event::system_log(
                                LogLevel::Warn,
                                &config.source,
                                format!("on_event error: {error}"),
                            ));
                        }
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }

        let timers_empty = lua
            .globals()
            .get::<Table>("__plugin_timers")
            .map(|table| table.is_empty())
            .unwrap_or(true);

        let callbacks_empty = lua
            .globals()
            .get::<Table>("__plugin_callbacks")
            .map(|table| table.is_empty())
            .unwrap_or(true);

        let tasks_all_done = lua
            .globals()
            .get::<Table>("__plugin_tasks")
            .map(|t| {
                t.pairs::<String, Table>()
                    .filter_map(|p| p.ok())
                    .all(|(_, state)| state.get::<bool>("finished").unwrap_or(true))
            })
            .unwrap_or(true);

        if timers_empty && callbacks_empty && tasks_all_done && event_receiver.is_empty() {
            break;
        }
    }

    alive.store(false, Ordering::Relaxed);
}
fn get_callback(lua: &Lua, topic: &str) -> Option<Function> {
    let callbacks: Table = lua.globals().get("__plugin_callbacks").ok()?;

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
    let timers: Table = lua.globals().get("__plugin_timers").ok()?;
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
fn process_timers(lua: &Lua, bus: &DataBus, config: &LuaRunConfig) {
    let timers: Table = match lua.globals().get("__plugin_timers") {
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
                    format!("timer error: {error}"),
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

/// 简单 Lua pattern 匹配：支持 ^ 锚点和子串匹配。
fn match_pat(line: &str, pat: &str) -> bool {
    if let Some(suffix) = pat.strip_prefix('^') {
        line.starts_with(suffix)
    } else {
        line.contains(pat)
    }
}

// ── Line Buffer ──

fn drain_serial_rx_to_buffers(event: &Event, host_services: &LuaHostServices) {
    if event.topic != topics::SERIAL_RX {
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
    line_buffers.lock().entry(key).or_default().feed(&data);
}

// ── Task Coroutine 调度 ──

const MAX_TASK_RESUMES_PER_TICK: usize = 50;

/// 注入 task 辅助函数（供 coroutine 内调用 yield）
fn install_task_helpers(lua: &Lua) -> mlua::Result<()> {
    lua.load(
        r#"
    -- 内部 yield 辅助：所有阻塞操作都通过这个函数 yield
    function __task_yield(yield_op)
        coroutine.yield(yield_op)
    end
"#,
    )
    .set_name("task-helpers")
    .exec()?;
    Ok(())
}

/// 每帧恢复可运行的 task coroutine。
fn process_tasks(
    lua: &Lua,
    _bus: &DataBus,
    _config: &LuaRunConfig,
    host_services: &LuaHostServices,
) {
    let tasks: Table = match lua.globals().get("__plugin_tasks") {
        Ok(t) => t,
        Err(_) => return,
    };

    let now_ms = tool_core::now_timestamp_ms();
    let mut resume_count = 0usize;

    // 先收集需要恢复的 task id
    let mut ready_ids: Vec<String> = Vec::new();
    for pair in tasks.pairs::<String, Table>() {
        if let Ok((id, state)) = pair {
            if state.get::<bool>("finished").unwrap_or(true) {
                continue;
            }

            // ── cancelled 优先：打断 sleep/read_line/expect/paused 等一切等待 ──
            let cancelled: bool = state.get("cancelled").unwrap_or(false);
            if cancelled {
                let yield_op: Option<Table> = state.get("yield_op").ok();
                if let Some(ref op) = yield_op {
                    let kind: String = op.get("kind").unwrap_or_default();
                    match kind.as_str() {
                        "read_line" => {
                            let _ = state.set("_read_result", Value::Nil);
                            let _ = state.set(
                                "_read_result_err",
                                lua.create_string("cancelled")
                                    .map(Value::String)
                                    .unwrap_or(Value::Nil),
                            );
                        }
                        "write_line_and_expect" => {
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

            let yield_op: Option<Table> = state.get("yield_op").ok();
            if let Some(ref op) = yield_op {
                let kind: String = op.get("kind").unwrap_or_default();
                match kind.as_str() {
                    "sleep" => {
                        let wake_at_ms: u64 = state.get("wake_at_ms").unwrap_or(0);
                        if now_ms < wake_at_ms {
                            continue;
                        }
                    }
                    "wait_paused" => {
                        continue;
                    }
                    "read_line" => {
                        let port: String = op.get("port").unwrap_or_default();
                        let deadline_ms: u64 = op.get("deadline_ms").unwrap_or(0);
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
                    "write_line_and_expect" => {
                        let port: String = op.get("port").unwrap_or_default();
                        let deadline_ms: u64 = op.get("deadline_ms").unwrap_or(0);
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
                            let mut map_lock = map.lock();
                            if let Some(buffer) = map_lock.get_mut(&key) {
                                let mut matched = None;
                                while let Some(line) = buffer.next_line() {
                                    let patterns: Option<Table> = op.get("patterns").ok();
                                    if let Some(ref pts) = patterns {
                                        for pair in pts.pairs::<Value, Table>().flatten() {
                                            let p: Table = pair.1;
                                            let pat: String = p.get("pattern").unwrap_or_default();
                                            let action: String = p
                                                .get("action")
                                                .unwrap_or_else(|_| "return".to_owned());
                                            let pname: String = p.get("name").unwrap_or_default();
                                            let hit = match_pat(&line, &pat);
                                            if hit {
                                                if action == "continue" {
                                                    let _ = state.set(
                                                        "status",
                                                        format!("设备忙: {pname}: {line}"),
                                                    );
                                                    break;
                                                }
                                                matched = Some((
                                                    p.get::<String>("name").unwrap_or_default(),
                                                    line.clone(),
                                                ));
                                                break;
                                            }
                                        }
                                    }
                                    if matched.is_some() {
                                        break;
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
        let _ = state.set("yield_op", Value::Nil);

        // 设置当前 task id，供 read_line 等函数使用
        let _ = lua.globals().set("__current_task_id", id.as_str());

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
                        let _ = state.set("finished", true);
                    }
                }
            }
            Err(e) => {
                // CoroutineUnresumable — 记录错误并标记完成
                let _ = state.set("finished", true);
                let _ = state.set("last_error", lua.create_string(&e.to_string()).ok());
                _bus.publish(Event::system_log(
                    LogLevel::Error,
                    &_config.source,
                    format!("task '{id}' failed: {e}"),
                ));
            }
        }

        resume_count += 1;
    }

    // 清除当前 task id
    let _ = lua.globals().set("__current_task_id", Value::Nil);
}

/// 从 task 对象获取 state table
fn get_state_for_task(lua: &Lua, task: &Table) -> mlua::Result<Table> {
    let id: String = task.get("id")?;
    let tasks: Table = lua.globals().get("__plugin_tasks")?;
    tasks.get(id.as_str())
}

/// 注入 task 对象方法，每个方法通过 task.id 查找 __plugin_tasks 中的实际 state
fn create_task_methods_table(lua: &Lua) -> mlua::Result<Table> {
    let tbl = lua.create_table()?;

    // task:is_cancelled() → bool
    tbl.set(
        "is_cancelled",
        lua.create_function(|lua, task: Table| {
            let state = get_state_for_task(&lua, &task)?;
            Ok(state.get::<bool>("cancelled").unwrap_or(false))
        })?,
    )?;

    // task:is_paused() → bool
    tbl.set(
        "is_paused",
        lua.create_function(|lua, task: Table| {
            let state = get_state_for_task(&lua, &task)?;
            Ok(state.get::<bool>("paused").unwrap_or(false))
        })?,
    )?;

    // task:sleep_ms(ms) — yield {kind="sleep", ms=N}
    tbl.set(
        "sleep_ms",
        lua.create_function(|lua, (task, ms): (Table, u64)| {
            let state = get_state_for_task(&lua, &task)?;
            let _ = state.set("wake_at_ms", tool_core::now_timestamp_ms() + ms);
            let op = lua.create_table()?;
            op.set("kind", "sleep")?;
            op.set("ms", ms)?;
            let _ = state.set("yield_op", op.clone());
            let yield_fn: Function = lua.globals().get("__task_yield")?;
            yield_fn.call::<Value>(op)?;
            Ok(())
        })?,
    )?;

    // task:wait_if_paused() — yield {kind="wait_paused"}
    tbl.set(
        "wait_if_paused",
        lua.create_function(|lua, task: Table| {
            let state = get_state_for_task(&lua, &task)?;
            if !state.get::<bool>("paused").unwrap_or(false) {
                return Ok(());
            }
            let op = lua.create_table()?;
            op.set("kind", "wait_paused")?;
            let _ = state.set("yield_op", op.clone());
            let yield_fn: Function = lua.globals().get("__task_yield")?;
            yield_fn.call::<Value>(op)?;
            Ok(())
        })?,
    )?;

    // task:set_progress(current, total)
    tbl.set(
        "set_progress",
        lua.create_function(|lua, (task, current, total): (Table, u64, u64)| {
            let state = get_state_for_task(&lua, &task)?;
            let _ = state.set("progress_current", current);
            let _ = state.set("progress_total", total);
            Ok(())
        })?,
    )?;

    // task:set_progress_percent(percent)
    tbl.set(
        "set_progress_percent",
        lua.create_function(|lua, (task, percent): (Table, f64)| {
            let state = get_state_for_task(&lua, &task)?;
            let _ = state.set("progress_percent", percent.clamp(0.0, 100.0));
            Ok(())
        })?,
    )?;

    // task:set_status(text)
    tbl.set(
        "set_status",
        lua.create_function(|lua, (task, text): (Table, String)| {
            let state = get_state_for_task(&lua, &task)?;
            let _ = state.set("status", text);
            Ok(())
        })?,
    )?;

    // task:log(level, message)
    tbl.set(
        "log",
        lua.create_function(|lua, (task, level, message): (Table, String, String)| {
            let state = get_state_for_task(&lua, &task)?;
            let logs: Table = state
                .get("logs")
                .unwrap_or_else(|_| lua.create_table().unwrap());
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

    Ok(tbl)
}

/// ctx.task API
fn create_task_api(
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
            state.set("cancelled", false)?;
            state.set("paused", false)?;
            state.set("cancellable", cancellable)?;
            state.set("pausable", pausable)?;
            state.set("progress_current", 0_u64)?;
            state.set("progress_total", 0_u64)?;
            state.set("progress_percent", Value::Nil)?;
            state.set("status", "running")?;
            state.set("wake_at_ms", 0_u64)?;
            state.set("yield_op", Value::Nil)?;
            state.set("finished", false)?;
            state.set("error", Value::Nil)?;
            state.set("logs", lua.create_table()?)?;

            // 存入全局任务表
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
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
                    let tasks: Table = lua.globals().get("__plugin_tasks")?;
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
                format!("[plugin:{}] task {} started", pid_start, id),
            ));

            // 首次 resume：把 task_obj 传给 function(task)
            // 如果 task 立即 yield（如 sleep），resume 返回 yield 值
            // coroutine 的 yield_op / wake_at_ms 已在 sleep_ms 等函数中设置
            match thread.resume::<Value>(task_obj.clone()) {
                Ok(_) => {
                    // coroutine 正常返回（未 yield，函数执行完毕）
                    match thread.status() {
                        mlua::ThreadStatus::Resumable => {
                            // yielded — yield_op 已由 Lua 侧设置
                        }
                        _ => {
                            let _ = state.set("finished", true);
                        }
                    }
                }
                Err(_) => {
                    let _ = state.set("finished", true);
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
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            if let Ok(state) = tasks.get::<Table>(id.as_str()) {
                let _ = state.set("cancelled", true);
                let _ = state.set("paused", false);
                tasks_ref.publish(Event::system_log(
                    LogLevel::Info,
                    &src_cancel,
                    format!("task {} cancelled", id),
                ));
            }
            Ok(())
        })?,
    )?;

    // ctx.task.pause(id)
    table.set(
        "pause",
        lua.create_function(move |lua, id: String| {
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
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
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
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
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            let result = lua.create_table()?;
            let mut idx = 0_u32;
            for pair in tasks.pairs::<String, Table>() {
                if let Ok((_id, state)) = pair {
                    idx += 1;
                    let summary = lua.create_table()?;
                    summary.set("id", state.get::<String>("id").unwrap_or_default())?;
                    summary.set("title", state.get::<String>("title").unwrap_or_default())?;
                    summary.set("cancelled", state.get::<bool>("cancelled").unwrap_or(false))?;
                    summary.set("paused", state.get::<bool>("paused").unwrap_or(false))?;
                    summary.set("finished", state.get::<bool>("finished").unwrap_or(false))?;
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
            }
            Ok(Value::Table(result))
        })?,
    )?;

    Ok(table)
}

/// 取消所有 task（插件 disable 时调用），唤醒所有 waiting task 让它们检测取消
fn cancel_all_tasks(lua: &Lua, bus: &DataBus, config: &LuaRunConfig) {
    let tasks: Table = match lua.globals().get("__plugin_tasks") {
        Ok(t) => t,
        Err(_) => return,
    };

    let mut task_ids: Vec<String> = Vec::new();
    for pair in tasks.pairs::<String, Table>() {
        if let Ok((id, state)) = pair {
            let _ = state.set("cancelled", true);
            let _ = state.set("paused", false);
            task_ids.push(id);
        }
    }

    // 唤醒所有 task coroutine 让它们发现 cancelled
    for _ in 0..MAX_TASK_RESUMES_PER_TICK {
        let mut any_resumed = false;
        for id in &task_ids {
            let state: Table = match tasks.get(id.as_str()) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if state.get::<bool>("finished").unwrap_or(true) {
                continue;
            }
            let thread: Thread = match state.get("thread") {
                Ok(t) => t,
                Err(_) => continue,
            };
            let _ = state.set("yield_op", Value::Nil);
            // 强制 resume，忽略结果，标记为 finished
            let _ = thread.resume::<Value>(());
            let _ = state.set("finished", true);
            any_resumed = true;
        }
        if !any_resumed {
            break;
        }
    }

    bus.publish(Event::system_log(
        LogLevel::Info,
        &config.source,
        format!("cancelled {} task(s)", task_ids.len()),
    ));
}

fn call_disable(lua: &Lua, bus: &DataBus, config: &LuaRunConfig) {
    // 先取消所有 task，让它们检测 cancelled 并退出
    cancel_all_tasks(lua, bus, config);

    if let Ok(function) = lua.globals().get::<Function>("__plugin_disable")
        && let Err(error) = function.call::<()>(())
    {
        bus.publish(Event::system_log(
            LogLevel::Warn,
            &config.source,
            format!("on_disable error: {error}"),
        ));
    }
}

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
                return Err(mlua::Error::RuntimeError("script stopped".to_owned()));
            }

            if Instant::now() >= deadline {
                return Err(mlua::Error::RuntimeError("script timeout".to_owned()));
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
        .set("__plugin_callbacks", lua.create_table()?)?;
    lua.globals().set("__plugin_timers", lua.create_table()?)?;
    lua.globals().set("__plugin_storage", lua.create_table()?)?;
    lua.globals().set("__plugin_tasks", lua.create_table()?)?;

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
            )?,
        )?;
    }

    if has_permission(config, "timer") {
        ctx.set("timer", create_timer_api(lua)?)?;
    }

    if has_permission(config, "storage") {
        let storage_api = create_storage_api(lua)?;
        ctx.set("session", storage_api.clone())?;
        // 向后兼容旧名称
        ctx.set("storage", storage_api)?;
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
    }

    ctx.set(
        "now_ms",
        lua.create_function(|_lua, ()| Ok(tool_core::now_timestamp_ms()))?,
    )?;

    ctx.set("plugin", json_to_lua_value(lua, &config.context)?)?;

    let _ = codec::register_codec(lua);

    lua.globals().set("ctx", &ctx)?;

    lua.globals().set(
        "on_disable",
        lua.create_function(|lua, function: Function| {
            lua.globals().set("__plugin_disable", function)
        })?,
    )?;

    if has_permission(config, "testing") {
        install_test_api(lua, &ctx, bus, config)?;
    }

    Ok(())
}

fn create_log_api(lua: &Lua, bus: DataBus, source: String) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    for (name, level) in [
        ("trace", LogLevel::Trace),
        ("debug", LogLevel::Debug),
        ("info", LogLevel::Info),
        ("warn", LogLevel::Warn),
        ("error", LogLevel::Error),
    ] {
        let bus = bus.clone();
        let source = source.clone();

        table.set(
            name,
            lua.create_function(move |_lua, message: String| {
                bus.publish(Event::system_log(level, source.clone(), message));
                Ok(())
            })?,
        )?;
    }

    Ok(table)
}

fn create_bus_api(
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
            let callbacks: Table = lua.globals().get("__plugin_callbacks")?;
            callbacks.set(topic, callback)?;
            Ok(())
        })?,
    )?;

    table.set(
        "off",
        lua.create_function(move |lua, topic: String| {
            let callbacks: Table = lua.globals().get("__plugin_callbacks")?;
            callbacks.set(topic, Value::Nil)?;
            Ok(())
        })?,
    )?;

    Ok(table)
}

fn create_serial_api(
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

    let transport_for_send = transport.clone();

    table.set(
        "send",
        lua.create_function(move |_lua, text: String| {
            let open = transport_for_send.open_ports();
            if open.len() > 1 {
                return Err(mlua::Error::RuntimeError(format!(
                    "ctx.serial.send 在多串口打开时语义不明确（已打开 {} 个端口），请使用 send_to(port, text)",
                    open.len()
                )));
            }
            transport_for_send
                .send_text(&text)
                .map_err(mlua::Error::external)
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

    let transport_for_send_hex = transport.clone();

    table.set(
        "send_hex",
        lua.create_function(move |_lua, text: String| {
            let open = transport_for_send_hex.open_ports();
            if open.len() > 1 {
                return Err(mlua::Error::RuntimeError(format!(
                    "ctx.serial.send_hex 在多串口打开时语义不明确（已打开 {} 个端口），请使用 send_hex_to(port, hex)",
                    open.len()
                )));
            }
            transport_for_send_hex
                .send_hex(&text)
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

    let transport_for_status = transport.clone();

    table.set(
        "status",
        lua.create_function(move |lua, ()| {
            let status = transport_for_status.status();

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
                let subscription = expect_from_bus.subscribe(TopicFilter::exact(topics::SERIAL_RX));
                let deadline = Instant::now() + Duration::from_millis(timeout_ms.unwrap_or(1_000));

                loop {
                    if let Some(ref stop) = expect_from_stop {
                        if stop.load(Ordering::Relaxed) {
                            return Ok(Value::Nil);
                        }
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
                            let event_port = event
                                .metadata
                                .get("port")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if event_port != port {
                                continue;
                            }
                            let text = event.payload.text_lossy();
                            if text.contains(&pattern) {
                                return Ok(Value::String(lua.create_string(&text)?));
                            }
                        }
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                            return Ok(Value::Nil);
                        }
                    }
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
            let subscription = expect_bus.subscribe(TopicFilter::exact(topics::SERIAL_RX));
            let deadline = Instant::now() + Duration::from_millis(timeout_ms.unwrap_or(1_000));

            loop {
                if let Some(ref stop) = expect_stop {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(Value::Nil);
                    }
                }

                let now = Instant::now();

                if now >= deadline {
                    return Ok(Value::Nil);
                }

                let remaining = deadline.saturating_duration_since(now);

                match subscription.recv_timeout(remaining.min(Duration::from_millis(50))) {
                    Ok(event) => {
                        let text = event.payload.text_lossy();

                        if text.contains(&pattern) {
                            return Ok(Value::String(lua.create_string(&text)?));
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        return Ok(Value::Nil);
                    }
                }
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
            let subscription = rq_bus.subscribe(TopicFilter::exact(topics::SERIAL_RX));
            let deadline = Instant::now() + Duration::from_millis(timeout_ms);

            // 2. 发送
            rq_transport
                .send_text_to(&port, &tx)
                .map_err(mlua::Error::external)?;

            // 3. 匹配响应
            loop {
                if let Some(ref stop) = rq_stop {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(Value::Nil);
                    }
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
                        let event_port = event
                            .metadata
                            .get("port")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if event_port != port {
                            continue;
                        }
                        let text = event.payload.text_lossy();
                        if text.contains(&expect) {
                            return Ok(Value::String(lua.create_string(&text)?));
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        return Ok(Value::Nil);
                    }
                }
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

    // ctx.serial.read_line(port_name, opts)
    // 只能在 task coroutine 内调用（coroutine.yield）
    let lb_read = host_services.line_buffers.clone();
    let pid_read = host_services.plugin_id.clone();
    let transport_read = transport.clone();
    table.set(
        "read_line",
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
            if let Some(ref map) = lb_read {
                if let Some(line) = map.lock().get_mut(&key).and_then(|b| b.next_line()) {
                    let result = lua.create_table()?;
                    result.set("line", lua.create_string(&line)?)?;
                    result.set("err", Value::Nil)?;
                    return Ok(Value::Table(result));
                }
            }

            // 无数据，yield 等待
            let task_id: String = lua
                .globals()
                .get::<String>("__current_task_id")
                .unwrap_or_default();
            if task_id.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "ctx.serial.read_line 必须在 ctx.task 协程内调用".into(),
                ));
            }

            let op = lua.create_table()?;
            op.set("kind", "read_line")?;
            op.set("port", port)?;
            op.set("delimiter", delimiter)?;
            op.set("timeout_ms", timeout_ms)?;
            op.set("deadline_ms", tool_core::now_timestamp_ms() + timeout_ms)?;

            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            if let Ok(state) = tasks.get::<Table>(task_id.as_str()) {
                let _ = state.set("yield_op", op.clone());
            }

            let yield_fn: Function = lua.globals().get("__task_yield")?;
            yield_fn.call::<Value>(op)?;

            // 恢复后，从 state 中读取结果。返回 table { line = ..., err = ... }
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
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

    // ctx.serial.write_line_and_expect(port, line, opts)
    // 只能在 task coroutine 内调用
    let lb_expect = host_services.line_buffers.clone();
    let pid_expect = host_services.plugin_id.clone();
    let transport_expect = transport;
    table.set(
        "write_line_and_expect",
        lua.create_function(move |lua, (port, line, opts): (String, String, Table)| {
            let port = transport_expect
                .canonical_open_port_name(&port)
                .unwrap_or(port);
            let timeout_ms: u64 = opts.get("timeout_ms").unwrap_or(300_000);
            let delimiter: String = opts.get("delimiter").unwrap_or_else(|_| "\n".to_owned());
            if delimiter != "\n" {
                return Err(mlua::Error::RuntimeError(
                    "v0.3 只支持 delimiter=\"\\n\"".into(),
                ));
            }
            let patterns: Table = opts.get("patterns").unwrap_or_else(|_| {
                let t = lua.create_table().unwrap();
                let entry = lua.create_table().unwrap();
                entry.set("pattern", "^ok").unwrap();
                entry.set("action", "return").unwrap();
                t.set(1, entry).unwrap();
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
                    while let Some(candidate) = buffer.next_line() {
                        for pair in patterns.pairs::<Value, Table>().flatten() {
                            let p: Table = pair.1;
                            let pat: String = p.get("pattern").unwrap_or_default();
                            let action: String =
                                p.get("action").unwrap_or_else(|_| "return".to_owned());
                            if match_pat(&candidate, &pat) {
                                if action == "continue" {
                                    // 更新 task status 让用户看到设备忙碌
                                    let tid: String = lua
                                        .globals()
                                        .get::<String>("__current_task_id")
                                        .unwrap_or_default();
                                    if !tid.is_empty() {
                                        let tasks: Table = lua.globals().get("__plugin_tasks")?;
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
                                let r = lua.create_table()?;
                                r.set("name", p.get::<String>("name").unwrap_or_default())?;
                                r.set("line", candidate)?;
                                r.set("elapsed_ms", 0_u64)?;
                                let wrapper = lua.create_table()?;
                                wrapper.set("result", r)?;
                                wrapper.set("err", Value::Nil)?;
                                return Ok(Value::Table(wrapper));
                            }
                        }
                    }
                }
            }

            // 无匹配，yield
            let task_id: String = lua
                .globals()
                .get::<String>("__current_task_id")
                .unwrap_or_default();
            if task_id.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "ctx.serial.write_line_and_expect 必须在 ctx.task 协程内调用".into(),
                ));
            }

            // 构造 yield_op（包含 deadline_ms 供 process_tasks 判断超时）
            let yield_data = lua.create_table()?;
            yield_data.set("kind", "write_line_and_expect")?;
            yield_data.set("port", port.as_str())?;
            yield_data.set("delimiter", delimiter.as_str())?;
            yield_data.set("timeout_ms", timeout_ms)?;
            yield_data.set("deadline_ms", tool_core::now_timestamp_ms() + timeout_ms)?;
            let _ = yield_data.set("patterns", patterns);

            let tasks: Table = lua.globals().get("__plugin_tasks")?;
            if let Ok(state) = tasks.get::<Table>(task_id.as_str()) {
                let _ = state.set("yield_op", yield_data.clone());
            }

            let yield_fn: Function = lua.globals().get("__task_yield")?;
            yield_fn.call::<Value>(yield_data)?;

            // 恢复后读取结果。返回 table { result = {name,line,elapsed_ms}, err = ... }
            let tasks: Table = lua.globals().get("__plugin_tasks")?;
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

    Ok(table)
}

fn create_storage_api(lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set(
        "get",
        lua.create_function(|lua, key: String| {
            let storage: Table = lua.globals().get("__plugin_storage")?;

            let value: mlua::Result<String> = storage.get(key);

            Ok(match value {
                Ok(value) => Value::String(lua.create_string(&value)?),
                Err(_) => Value::Nil,
            })
        })?,
    )?;

    table.set(
        "set",
        lua.create_function(|lua, (key, value): (String, String)| {
            let storage: Table = lua.globals().get("__plugin_storage")?;
            storage.set(key, value)?;
            Ok(())
        })?,
    )?;

    table.set(
        "keys",
        lua.create_function(|lua, ()| {
            let storage: Table = lua.globals().get("__plugin_storage")?;

            let keys = storage
                .pairs::<String, Value>()
                .filter_map(|pair| pair.ok().map(|(key, _)| key))
                .collect::<Vec<_>>();

            Ok(keys)
        })?,
    )?;

    Ok(table)
}

fn create_timer_api(lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set(
        "after",
        lua.create_function(move |lua, (ms, callback): (u64, Function)| {
            let timers: Table = lua.globals().get("__plugin_timers")?;
            let now_ms = tool_core::now_timestamp_ms();
            let id = format!("t{now_ms}-{}", timers.raw_len());

            let timer = lua.create_table()?;
            timer.set("trigger_at_ms", now_ms + ms)?;
            timer.set("interval_ms", 0_u64)?;
            timer.set("callback", callback)?;

            timers.set(id.clone(), timer)?;

            Ok(id)
        })?,
    )?;

    table.set(
        "every",
        lua.create_function(move |lua, (ms, callback): (u64, Function)| {
            let timers: Table = lua.globals().get("__plugin_timers")?;
            let now_ms = tool_core::now_timestamp_ms();
            let interval_ms = ms.max(1);
            let id = format!("t{now_ms}-{}", timers.raw_len());

            let timer = lua.create_table()?;
            timer.set("trigger_at_ms", now_ms + interval_ms)?;
            timer.set("interval_ms", interval_ms)?;
            timer.set("callback", callback)?;

            timers.set(id.clone(), timer)?;

            Ok(id)
        })?,
    )?;

    table.set(
        "cancel",
        lua.create_function(move |lua, id: String| {
            let timers: Table = lua.globals().get("__plugin_timers")?;
            timers.set(id, Value::Nil)?;
            Ok(())
        })?,
    )?;

    Ok(table)
}
fn create_dialog_api(
    lua: &Lua,
    dialog_sender: crossbeam_channel::Sender<DialogRequest>,
    plugin_id: String,
    stop_flag: Option<Arc<AtomicBool>>,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set(
        "open_file",
        lua.create_function(move |cb_lua, config: Value| {
            let obj = match config {
                Value::Table(t) => t,
                _ => return Ok(Value::Nil),
            };

            let title: String = obj.get("title").unwrap_or_else(|_| "选择文件".to_owned());
            let filters: Vec<FileFilter> = parse_lua_filters(&obj)?;

            let (response_sender, response_receiver) =
                crossbeam_channel::bounded::<Option<PathBuf>>(1);

            let _ = dialog_sender.send(DialogRequest {
                plugin_id: plugin_id.clone(),
                title,
                filters,
                response_sender,
            });

            // 100ms 轮询，支持插件停止时及时返回
            let result = loop {
                if let Some(ref stop) = stop_flag {
                    if stop.load(Ordering::Relaxed) {
                        break None;
                    }
                }
                match response_receiver.recv_timeout(Duration::from_millis(100)) {
                    Ok(Some(path)) => {
                        break Some(path.display().to_string());
                    }
                    Ok(None) => break None,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break None,
                }
            };

            match result {
                Some(path_str) => {
                    let s = cb_lua.create_string(&path_str)?;
                    Ok(Value::String(s))
                }
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    Ok(table)
}

fn parse_lua_filters(obj: &Table) -> mlua::Result<Vec<FileFilter>> {
    let filters_table: Option<Table> = obj.get("filters").ok();
    let Some(filters_table) = filters_table else {
        return Ok(vec![FileFilter {
            name: "所有文件".to_owned(),
            extensions: vec!["*".to_owned()],
        }]);
    };

    let mut result = Vec::new();
    for pair in filters_table.pairs::<Value, Value>() {
        let (_, value) = pair?;
        if let Value::Table(ft) = value {
            let name: String = ft.get("name").unwrap_or_default();
            let exts: Vec<String> = ft
                .get::<Table>("extensions")
                .map(|t| {
                    t.sequence_values::<String>()
                        .filter_map(|v| v.ok())
                        .collect()
                })
                .unwrap_or_default();
            result.push(FileFilter {
                name,
                extensions: exts,
            });
        }
    }
    if result.is_empty() {
        result.push(FileFilter {
            name: "所有文件".to_owned(),
            extensions: vec!["*".to_owned()],
        });
    }
    Ok(result)
}

fn create_fs_api(
    lua: &Lua,
    broker: Arc<FileAccessBroker>,
    plugin_id: String,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    let broker_read = broker.clone();
    let pid_read = plugin_id.clone();
    table.set(
        "read_text",
        lua.create_function(move |_lua, path: String| {
            let p = PathBuf::from(&path);
            if !broker_read.is_authorized(&pid_read, &p) {
                return Err(mlua::Error::RuntimeError(format!(
                    "文件未授权: {path}. 请先通过文件选择对话框选择文件。"
                )));
            }
            let content = std::fs::read_to_string(&p)
                .map_err(|e| mlua::Error::RuntimeError(format!("读取文件失败: {e}")))?;
            // 16 MiB 上限
            if content.len() > 16 * 1024 * 1024 {
                return Err(mlua::Error::RuntimeError("文件超过 16 MiB 上限".to_owned()));
            }
            Ok(content)
        })?,
    )?;

    let broker_lines = broker;
    let pid_lines = plugin_id;
    table.set(
        "read_lines",
        lua.create_function(move |lua, path: String| {
            let p = PathBuf::from(&path);
            if !broker_lines.is_authorized(&pid_lines, &p) {
                return Err(mlua::Error::RuntimeError(format!(
                    "文件未授权: {path}. 请先通过文件选择对话框选择文件。"
                )));
            }
            let content = std::fs::read_to_string(&p)
                .map_err(|e| mlua::Error::RuntimeError(format!("读取文件失败: {e}")))?;
            if content.len() > 16 * 1024 * 1024 {
                return Err(mlua::Error::RuntimeError("文件超过 16 MiB 上限".to_owned()));
            }
            let lines: Arc<Vec<String>> = Arc::new(content.lines().map(String::from).collect());
            let index = Arc::new(ParkingMutex::new(0usize));
            let lines_len = lines.len();

            // 返回迭代函数：每次调用返回下一行，结束时返回 nil
            let iter_fn = lua.create_function(move |lua, ()| {
                let mut i = index.lock();
                if *i >= lines_len {
                    return Ok(Value::Nil);
                }
                let line = lines[*i].clone();
                *i += 1;
                Ok(Value::String(lua.create_string(&line)?))
            })?;
            Ok(Value::Function(iter_fn))
        })?,
    )?;

    Ok(table)
}

fn create_config_api(lua: &Lua, store: Arc<ConfigStore>, plugin_id: String) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    // ctx.config.get(key, default)
    let store_get = store.clone();
    let pid_get = plugin_id.clone();
    table.set(
        "get",
        lua.create_function(move |lua, (key, default): (String, Value)| {
            let default_json = lua_value_to_json(default).unwrap_or(serde_json::Value::Null);
            let value = store_get.get(&pid_get, &key, default_json);
            json_to_lua_value(&lua, &value)
        })?,
    )?;

    // ctx.config.set(key, value)
    let store_set = store.clone();
    let pid_set = plugin_id.clone();
    table.set(
        "set",
        lua.create_function(move |_lua, (key, value): (String, Value)| {
            let json_value = lua_value_to_json(value).unwrap_or(serde_json::Value::Null);
            store_set
                .set(&pid_set, &key, json_value)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))
        })?,
    )?;

    // ctx.config.remove(key)
    let store_remove = store.clone();
    let pid_remove = plugin_id.clone();
    table.set(
        "remove",
        lua.create_function(move |_lua, key: String| {
            store_remove
                .remove(&pid_remove, &key)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))
        })?,
    )?;

    // ctx.config.keys()
    let store_keys = store.clone();
    let pid_keys = plugin_id.clone();
    table.set(
        "keys",
        lua.create_function(move |lua, ()| {
            let keys = store_keys.keys(&pid_keys);
            let arr = lua.create_table()?;
            for (i, k) in keys.iter().enumerate() {
                arr.set(i + 1, k.as_str())?;
            }
            Ok(Value::Table(arr))
        })?,
    )?;

    // ── profile API ──

    // ctx.config.profile_list()
    let store_pl = store.clone();
    let pid_pl = plugin_id.clone();
    table.set(
        "profile_list",
        lua.create_function(move |lua, ()| {
            let names = store_pl.profile_list(&pid_pl);
            let arr = lua.create_table()?;
            for (i, name) in names.iter().enumerate() {
                arr.set(i + 1, name.as_str())?;
            }
            Ok(Value::Table(arr))
        })?,
    )?;

    // ctx.config.profile_load(name)
    let store_pload = store.clone();
    let pid_pload = plugin_id.clone();
    table.set(
        "profile_load",
        lua.create_function(move |lua, name: String| {
            match store_pload.profile_load(&pid_pload, &name) {
                Some(data) => json_to_lua_value(&lua, &data),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // ctx.config.profile_save(name, data)
    let store_psave = store.clone();
    let pid_psave = plugin_id.clone();
    table.set(
        "profile_save",
        lua.create_function(move |_lua, (name, data): (String, Value)| {
            let json_data = lua_value_to_json(data).unwrap_or(serde_json::Value::Null);
            store_psave
                .profile_save(&pid_psave, &name, json_data)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))
        })?,
    )?;

    // ctx.config.profile_delete(name)
    let store_pdel = store;
    let pid_pdel = plugin_id;
    table.set(
        "profile_delete",
        lua.create_function(move |_lua, name: String| {
            store_pdel
                .profile_delete(&pid_pdel, &name)
                .map_err(|e| mlua::Error::RuntimeError(e.to_string()))
        })?,
    )?;

    Ok(table)
}

fn create_ui_api(
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
        ("create_log", "log"),
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
                Payload::Json(json!({ "id": panel_id })),
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

    // ctx.ui.log_append(panel_id, { level = "info", message = "..." })
    let bus_log = bus.clone();
    let src_log = source.clone();
    let pid_log = plugin_id.clone();
    table.set(
        "log_append",
        lua.create_function(move |_lua, (panel_id, entry): (String, Table)| {
            let level: String = entry.get("level").unwrap_or_else(|_| "info".to_owned());
            let message: String = entry.get("message").unwrap_or_default();
            bus_log.publish(Event::new(
                topics::UI_LOG_APPEND,
                src_log.clone(),
                Direction::Internal,
                Payload::Json(serde_json::json!({
                    "panel_id": panel_id,
                    "level": level,
                    "message": message,
                    "plugin_id": pid_log,
                })),
            ));
            Ok(())
        })?,
    )?;

    let bus_visible = bus;
    let src_visible = source;
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

    Ok(table)
}

fn install_test_api(
    lua: &Lua,
    ctx: &Table,
    bus: DataBus,
    config: &LuaRunConfig,
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
                        && matches!(event.topic.as_str(), topics::SERIAL_RX | topics::SERIAL_TX)
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
                topics::TEST_RESULT,
                source.clone(),
                Direction::Internal,
                Payload::Json(lua_value_to_json(report)?),
            ));

            Ok(())
        })?,
    )?;

    lua.globals().set("__test_host", host)?;

    lua.load(TEST_BOOTSTRAP).set_name("test-bootstrap").exec()?;

    let test: Table = lua.globals().get("test")?;
    ctx.set("test", test)?;

    Ok(())
}

fn test_packet_from_event(event: Event) -> TestPacketLog {
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
        direction: format!("{:?}", event.direction).to_lowercase(),
        payload_text,
        payload_hex,
    }
}

fn event_to_lua_table(lua: &Lua, table: &Table, event: &Event) -> mlua::Result<()> {
    table.set("id", event.id)?;
    table.set("timestamp_ms", event.timestamp_ms)?;
    table.set("topic", event.topic.clone())?;
    table.set("source", event.source.clone())?;
    table.set("direction", format!("{:?}", event.direction).to_lowercase())?;
    table.set("payload", payload_to_lua(lua, &event.payload)?)?;
    table.set("metadata", json_to_lua_value(lua, &event.metadata)?)?;

    Ok(())
}

fn payload_to_lua(lua: &Lua, payload: &Payload) -> mlua::Result<Value> {
    Ok(match payload {
        Payload::Empty => Value::Nil,
        Payload::Bytes(bytes) => Value::String(lua.create_string(bytes)?),
        Payload::Text(text) => Value::String(lua.create_string(text)?),
        Payload::Json(value) => json_to_lua_value(lua, value)?,
    })
}

fn lua_value_to_serial_config(value: Value) -> mlua::Result<SerialConfig> {
    match value {
        Value::String(port_name) => Ok(SerialConfig {
            port_name: port_name.to_str()?.to_owned(),
            ..Default::default()
        }),

        Value::Table(table) => {
            let mut config = SerialConfig {
                port_name: table.get("port_name").or_else(|_| table.get("port"))?,
                ..Default::default()
            };

            if let Ok(value) = table.get::<u32>("baud_rate") {
                config.baud_rate = value;
            } else if let Ok(value) = table.get::<u32>("baud") {
                config.baud_rate = value;
            }

            if let Ok(value) = table.get::<u64>("timeout_ms") {
                config.timeout_ms = value;
            }

            if let Ok(value) = table.get::<String>("data_bits") {
                config.data_bits = parse_data_bits(&value);
            }

            if let Ok(value) = table.get::<String>("stop_bits") {
                config.stop_bits = parse_stop_bits(&value);
            }

            if let Ok(value) = table.get::<String>("parity") {
                config.parity = parse_parity(&value);
            }

            Ok(config)
        }

        other => Err(mlua::Error::RuntimeError(format!(
            "serial.open expects a string or table, got {}",
            other.type_name()
        ))),
    }
}

fn parse_data_bits(value: &str) -> tool_transport::DataBits {
    match value {
        "5" => tool_transport::DataBits::Five,
        "6" => tool_transport::DataBits::Six,
        "7" => tool_transport::DataBits::Seven,
        _ => tool_transport::DataBits::Eight,
    }
}

fn parse_stop_bits(value: &str) -> tool_transport::StopBits {
    match value {
        "2" => tool_transport::StopBits::Two,
        _ => tool_transport::StopBits::One,
    }
}

fn parse_parity(value: &str) -> tool_transport::Parity {
    match value {
        "odd" => tool_transport::Parity::Odd,
        "even" => tool_transport::Parity::Even,
        _ => tool_transport::Parity::None,
    }
}

fn lua_value_to_payload(value: Value) -> mlua::Result<Payload> {
    Ok(match value {
        Value::Nil => Payload::Empty,
        Value::Boolean(value) => Payload::Json(serde_json::Value::Bool(value)),
        Value::Integer(value) => Payload::Json(serde_json::Value::Number(value.into())),
        Value::Number(value) => Payload::Json(number_to_json(value)?),
        Value::String(value) => Payload::Text(value.to_str()?.to_owned()),
        Value::Table(value) => Payload::Json(lua_table_to_json(value)?),
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "unsupported payload: {}",
                other.type_name()
            )));
        }
    })
}

fn lua_table_to_json(table: Table) -> mlua::Result<serde_json::Value> {
    let mut entries = Vec::new();
    let mut is_array = true;
    let mut max_index = 0_i64;

    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;

        if let Value::Integer(index) = key
            && index > 0
        {
            max_index = max_index.max(index);
            entries.push((key, lua_value_to_json(value)?));
            continue;
        }

        is_array = false;
        entries.push((key, lua_value_to_json(value)?));
    }

    if is_array && max_index as usize == entries.len() {
        entries.sort_by_key(|(key, _)| match key {
            Value::Integer(index) => *index,
            _ => 0,
        });

        return Ok(serde_json::Value::Array(
            entries.into_iter().map(|(_, value)| value).collect(),
        ));
    }

    let mut object = Map::new();

    for (key, value) in entries {
        object.insert(lua_key_to_string(key)?, value);
    }

    Ok(serde_json::Value::Object(object))
}

fn lua_value_to_json(value: Value) -> mlua::Result<serde_json::Value> {
    Ok(match value {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(value) => serde_json::Value::Bool(value),
        Value::Integer(value) => serde_json::Value::Number(value.into()),
        Value::Number(value) => number_to_json(value)?,
        Value::String(value) => serde_json::Value::String(value.to_str()?.to_owned()),
        Value::Table(value) => lua_table_to_json(value)?,
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "unsupported value: {}",
                other.type_name()
            )));
        }
    })
}

fn lua_key_to_string(key: Value) -> mlua::Result<String> {
    Ok(match key {
        Value::String(value) => value.to_str()?.to_owned(),
        Value::Integer(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "unsupported key: {}",
                other.type_name()
            )));
        }
    })
}

fn number_to_json(value: f64) -> mlua::Result<serde_json::Value> {
    Number::from_f64(value)
        .map(serde_json::Value::Number)
        .ok_or_else(|| mlua::Error::RuntimeError("number is not finite".to_owned()))
}

fn payload_to_json(payload: Payload) -> serde_json::Value {
    match payload {
        Payload::Empty => serde_json::Value::Null,
        Payload::Bytes(bytes) => serde_json::Value::Array(
            bytes
                .into_iter()
                .map(|byte| serde_json::Value::Number(byte.into()))
                .collect(),
        ),
        Payload::Text(text) => serde_json::Value::String(text),
        Payload::Json(value) => value,
    }
}

fn json_to_lua_value(lua: &Lua, value: &serde_json::Value) -> mlua::Result<Value> {
    Ok(match value {
        serde_json::Value::Null => Value::Nil,

        serde_json::Value::Bool(value) => Value::Boolean(*value),

        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Value::Integer(value)
            } else if let Some(value) = value.as_f64() {
                Value::Number(value)
            } else {
                Value::Nil
            }
        }

        serde_json::Value::String(value) => Value::String(lua.create_string(value)?),

        serde_json::Value::Array(values) => {
            let table = lua.create_table()?;

            for (index, value) in values.iter().enumerate() {
                table.set(index + 1, json_to_lua_value(lua, value)?)?;
            }

            Value::Table(table)
        }

        serde_json::Value::Object(values) => {
            let table = lua.create_table()?;

            for (key, value) in values {
                table.set(key.as_str(), json_to_lua_value(lua, value)?)?;
            }

            Value::Table(table)
        }
    })
}

fn ensure_json_object(
    value: serde_json::Value,
    function_name: &str,
) -> mlua::Result<Map<String, serde_json::Value>> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| mlua::Error::RuntimeError(format!("ctx.ui.{function_name} expects a table")))
}

fn ensure_panel_defaults(
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

    if fallback_kind == "chart" && !config.contains_key("topic_prefix") {
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
            serde_json::Value::String(topics::PROTOCOL_IMU_ATTITUDE.to_owned()),
        );
    }

    Ok(())
}

fn wait_for_event(
    lua: &Lua,
    bus: DataBus,
    filter: TopicFilter,
    timeout_ms: Option<u64>,
    stop_flag: Option<Arc<AtomicBool>>,
) -> mlua::Result<Value> {
    let subscription = bus.subscribe(filter);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.unwrap_or(1_000));

    loop {
        if let Some(ref stop) = stop_flag {
            if stop.load(Ordering::Relaxed) {
                return Ok(Value::Nil);
            }
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

const TEST_BOOTSTRAP: &str = r#"
local host = __test_host
local test = {
  _cases = {},
  _before_each = nil,
  _after_each = nil,
  _timeout_ms = 5000,
  _current = nil
}

function test.timeout(ms)
  test._timeout_ms = ms
end

function test.before_each(fn)
  test._before_each = fn
end

function test.after_each(fn)
  test._after_each = fn
end

function test.log(message)
  local text = tostring(message)
  if test._current then
    table.insert(test._current.logs, text)
  end
  if ctx.log then
    ctx.log.info(text)
  end
end

function test.assert(condition, message)
  if test._current then
    test._current.assertions = test._current.assertions + 1
  end
  if not condition then
    error(message or "assertion failed", 2)
  end
end

function test.expect(topic, timeout_ms)
  return ctx.bus.wait(topic, timeout_ms or test._timeout_ms)
end

local function publish_report()
  host.publish_report({
    run_id = host.run_id,
    source = host.source,
    script_name = host.script_name,
    started_ms = host.run_started_ms,
    finished_ms = host.now_ms(),
    cases = test._cases
  })
end

function test.case(name, fn)
  local start_event_id = host.latest_event_id()
  local started = host.now_ms()
  local case = {
    name = name,
    status = "passed",
    duration_ms = 0,
    logs = {},
    assertions = 0,
    error = nil,
    raw_packets = {}
  }

  test._current = case
  local ok, err = pcall(function()
    if test._before_each then test._before_each() end
    fn()
  end)

  local after_ok, after_err = true, nil
  if test._after_each then
    after_ok, after_err = pcall(test._after_each)
  end

  case.duration_ms = host.now_ms() - started
  case.raw_packets = host.raw_packets_since(start_event_id)

  if not ok then
    case.status = "failed"
    case.error = tostring(err)
  elseif not after_ok then
    case.status = "failed"
    case.error = tostring(after_err)
  end

  test._current = nil
  table.insert(test._cases, case)
  publish_report()
end

_G.test = test
"#;

// ── Replay Analyzer ──

/// Replay analyzer 运行配置。
#[derive(Debug, Clone)]
pub struct LuaReplayConfig {
    pub script_name: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub subscriptions: Vec<String>,
    pub context: serde_json::Value,
    pub plugin_root: Option<PathBuf>,
}

/// Replay analyzer 输出。
#[derive(Debug, Clone)]
pub struct LuaReplayOutput {
    pub events: Vec<Event>,
    pub logs: Vec<String>,
}

/// 运行 Lua replay analyzer。
///
/// 创建一个受限 Lua 环境，只提供 `ctx.plugin`、`ctx.storage.get`、
/// `ctx.replay.emit`、`ctx.replay.log`、`ctx.now_ms` 以及
/// `ctx.replay.current_event`。
///
/// 不提供 `ctx.serial`、`ctx.timer`、`ctx.ui`、`ctx.bus.publish`、
/// `ctx.bus.on`。
///
/// 执行流程：
/// 1. `on_replay_begin(session_info)`
/// 2. 对每个匹配 subscriptions 的 input_event 调用 `on_replay_event(event)`
/// 3. `on_replay_end()`
pub fn run_replay_analyzer(
    source: String,
    config: LuaReplayConfig,
    input_events: &[Event],
) -> LuaHostResult<LuaReplayOutput> {
    let lua = Lua::new_with(
        StdLib::TABLE
            | StdLib::STRING
            | StdLib::MATH
            | StdLib::UTF8
            | StdLib::PACKAGE
            | StdLib::COROUTINE,
        LuaOptions::default(),
    )?;

    // 安装 budget hook：防止 analyzer 死循环或卡死
    let replay_stop = Arc::new(AtomicBool::new(false));
    install_budget_hook(&lua, 30_000, replay_stop.clone())?;

    let emitted_events = Arc::new(ParkingMutex::new(Vec::new()));
    let logs = Arc::new(ParkingMutex::new(Vec::new()));

    install_replay_ctx(&lua, emitted_events.clone(), logs.clone(), &config)?;

    // 加载并执行 Lua 源码
    lua.load(&source).set_name(&config.script_name).exec()?;

    // 构建 session 信息
    let first_ts = input_events.first().map(|e| e.timestamp_ms).unwrap_or(0);
    let last_ts = input_events.last().map(|e| e.timestamp_ms).unwrap_or(0);
    let session = lua.create_table()?;
    session.set("start_ms", first_ts)?;
    session.set("end_ms", last_ts)?;
    session.set("event_count", input_events.len())?;

    // on_replay_begin
    if let Ok(begin_fn) = lua.globals().get::<Function>("on_replay_begin")
        && let Err(e) = begin_fn.call::<Value>(session)
    {
        logs.lock().push(format!("on_replay_begin error: {e}"));
    }

    // 遍历输入事件
    for input_event in input_events {
        // 只处理匹配 subscriptions 的事件
        // 使用与实时插件一致的 topic_matches 语义（* 前缀，无 * 精确）
        if !config
            .subscriptions
            .iter()
            .any(|sub| topic_matches(sub.as_str(), &input_event.topic))
        {
            continue;
        }

        // 设置当前输入事件信息，供 ctx.replay.emit 使用
        let current_input_ts = input_event.timestamp_ms;
        let current_input_id = input_event.id;
        lua.globals().set("__replay_current_ts", current_input_ts)?;
        lua.globals().set("__replay_current_id", current_input_id)?;

        if let Ok(callback) = lua.globals().get::<Function>("on_replay_event") {
            let event_table = lua.create_table()?;
            event_to_lua_table(&lua, &event_table, input_event)?;

            // 设置当前事件，供 ctx.replay.current_event() 使用
            lua.globals()
                .set("__replay_current_event", event_table.clone())?;

            if let Err(e) = callback.call::<Value>(event_table) {
                logs.lock().push(format!("on_replay_event error: {e}"));
            }
        }
    }

    // on_replay_end 时清除 current 标记，emit 使用最后一个输入事件时间戳
    lua.globals().set("__replay_current_ts", last_ts)?;
    lua.globals().set("__replay_current_id", Value::Nil)?;

    // on_replay_end
    if let Ok(end_fn) = lua.globals().get::<Function>("on_replay_end")
        && let Err(e) = end_fn.call::<()>(())
    {
        logs.lock().push(format!("on_replay_end error: {e}"));
    }

    let events = emitted_events.lock().clone();
    let logs = logs.lock().clone();

    Ok(LuaReplayOutput { events, logs })
}

fn install_replay_ctx(
    lua: &Lua,
    emitted_events: Arc<ParkingMutex<Vec<Event>>>,
    logs: Arc<ParkingMutex<Vec<String>>>,
    config: &LuaReplayConfig,
) -> mlua::Result<()> {
    let ctx = lua.create_table()?;

    // ctx.plugin (只读)
    ctx.set("plugin", json_to_lua_value(lua, &config.context)?)?;

    // ctx.now_ms()
    ctx.set(
        "now_ms",
        lua.create_function(|_lua, ()| Ok(tool_core::now_timestamp_ms()))?,
    )?;

    // 本地 require：只能加载插件根目录和 lib 子目录
    if let Some(ref root) = config.plugin_root {
        let root_str = root.display().to_string().replace('\\', "/");
        let new_path = format!("{root_str}/lib/?.lua;{root_str}/?.lua");
        if let Ok(package) = lua.globals().get::<Table>("package") {
            let _ = package.set("path", new_path);
            let _ = package.set("cpath", "");
        }
    }

    // ctx.storage.get (只读)
    let storage = lua.create_table()?;
    storage.set(
        "get",
        lua.create_function(|lua, key: String| {
            let storage: Table = lua.globals().get("__plugin_storage")?;
            let value: mlua::Result<String> = storage.get(key);
            Ok(match value {
                Ok(value) => Value::String(lua.create_string(&value)?),
                Err(_) => Value::Nil,
            })
        })?,
    )?;
    ctx.set("storage", storage)?;

    // ctx.replay
    let replay = lua.create_table()?;

    // ctx.replay.emit(topic, payload)
    // 使用当前输入事件的时间戳（由外层在 Lua globals 中设置）
    let emitted = emitted_events.clone();
    let config_emit = config.clone();
    replay.set(
        "emit",
        lua.create_function(move |lua, (topic, payload): (String, Value)| {
            let payload = lua_value_to_payload(payload)?;
            let source = format!("replay-analyzer:{}", config_emit.plugin_id);

            // 读取当前输入事件时间戳
            let ts: u64 = lua.globals().get("__replay_current_ts").unwrap_or(0);
            let derived_from: u64 = lua.globals().get("__replay_current_id").unwrap_or(0);

            let mut event = Event::new(topic, source, Direction::Internal, payload);
            event.timestamp_ms = ts;
            tool_core::mark_derived_event(
                &mut event,
                &config_emit.plugin_id,
                &config_emit.plugin_version,
                &[derived_from],
            );

            emitted.lock().push(event);
            Ok(())
        })?,
    )?;

    // ctx.replay.log(message)
    let logs_for_fn = logs.clone();
    replay.set(
        "log",
        lua.create_function(move |_lua, message: String| {
            logs_for_fn.lock().push(message);
            Ok(())
        })?,
    )?;

    // ctx.replay.current_event() — 返回当前输入事件 table
    replay.set(
        "current_event",
        lua.create_function(|lua, ()| {
            let event: Value = lua
                .globals()
                .get("__replay_current_event")
                .unwrap_or(Value::Nil);
            Ok(event)
        })?,
    )?;

    ctx.set("replay", replay)?;

    // 安装全局存储表
    lua.globals().set("__plugin_storage", lua.create_table()?)?;

    let _ = codec::register_codec(lua);

    // 注册 ctx 全局变量
    lua.globals().set("ctx", ctx)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_databus::TopicFilter;

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
                Direction::Internal,
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
            publisher.publish(Event::serial_rx("test", b"READY\r\n".to_vec()));
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
            publisher.publish(Event::serial_rx("test", b"OK\r\n".to_vec()));
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
            context: json!({"id": "test", "name": "Test"}),
            plugin_root: None,
        };

        let input = Event::new(
            "transport.serial.default.rx",
            "serial:COM2",
            Direction::Rx,
            Payload::Text("hello".to_owned()),
        );

        let output = run_replay_analyzer(source.to_owned(), config, &[input]).unwrap();
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
            context: json!({"id": "test", "name": "Test"}),
            plugin_root: None,
        };

        let input = Event::new(
            "transport.serial.default.rx",
            "serial:COM2",
            Direction::Rx,
            Payload::Text("hello".to_owned()),
        );

        let output = run_replay_analyzer(source.to_owned(), config, &[input]).unwrap();
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
            context: json!({"id": "demo.plugin", "name": "Demo", "version": "2.0.0"}),
            plugin_root: None,
        };

        let input = Event::new(
            "transport.serial.default.rx",
            "serial:COM2",
            Direction::Rx,
            Payload::Text("test".to_owned()),
        );

        let output = run_replay_analyzer(source.to_owned(), config, &[input.clone()]).unwrap();
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
            context: json!({"id": "test.lifecycle", "name": "Lifecycle"}),
            plugin_root: None,
        };

        let input1 = Event::new(
            "transport.serial.default.rx",
            "serial:COM2",
            Direction::Rx,
            Payload::Text("a".to_owned()),
        );
        let input2 = Event::new(
            "transport.serial.default.rx",
            "serial:COM2",
            Direction::Rx,
            Payload::Text("b".to_owned()),
        );

        let output = run_replay_analyzer(source.to_owned(), config, &[input1, input2]).unwrap();

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
            context: json!({"id": "test.skip", "name": "Skip"}),
            plugin_root: None,
        };

        // 只有 1 个匹配的 RX 事件，另 1 个是 TX
        let rx = Event::new(
            "transport.serial.default.rx",
            "serial:COM2",
            Direction::Rx,
            Payload::Text("rx".to_owned()),
        );
        let tx = Event::new(
            "transport.serial.default.tx",
            "serial:COM2",
            Direction::Tx,
            Payload::Text("tx".to_owned()),
        );

        let output = run_replay_analyzer(source.to_owned(), config, &[rx, tx]).unwrap();
        assert_eq!(
            output.events.len(),
            1,
            "should only emit for matched RX event"
        );
    }
}
