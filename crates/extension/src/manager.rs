//! 插件管理器：发现、启用、禁用、事件分发。

use parking_lot::Mutex;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tool_core::{Direction, Event, LogLevel, Payload, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};
use tool_lua_host::{ConfigStore, LineBufferMap, LuaPluginRuntime, LuaRunConfig, run_plugin};
use tool_transport::TransportManager;

use crate::host_services::{DialogRequest, FileAccessBroker};
use crate::manifest::{
    PluginDiagnostic, PluginDiagnosticSeverity, PluginManifest, PluginState, PluginSummary,
    ReplayAnalyzerEntry, SUPPORTED_PLUGIN_API_VERSIONS,
};
use crate::permission::PermissionManager;
use crate::{ExtensionError, ExtensionResult};

const MAX_PLUGIN_EVENTS_PER_FRAME: usize = 500;

struct PluginRecord {
    manifest: PluginManifest,
    root: PathBuf,
    state: PluginState,
    last_error: Option<String>,
}

pub struct PluginManager {
    bus: DataBus,
    transport: TransportManager,
    permission_manager: PermissionManager,
    records: BTreeMap<String, PluginRecord>,
    lua_runtimes: HashMap<String, LuaPluginRuntime>,
    stopping_plugins: Vec<(String, LuaPluginRuntime)>,
    /// 运行时注册的命令：plugin_id → 命令 ID 列表
    registered_commands: HashMap<String, Vec<String>>,
    roots: Vec<PathBuf>,
    subscription: Subscription,
    dialog_request_sender: Option<crossbeam_channel::Sender<DialogRequest>>,
    file_broker: Option<Arc<FileAccessBroker>>,
    line_buffers: LineBufferMap,
    config_store: Arc<ConfigStore>,
    dropped_events: u64,
    last_seen_manager_dropped: u64,
    diagnostics: Vec<PluginDiagnostic>,
}

impl PluginManager {
    pub fn new(bus: DataBus, transport: TransportManager) -> Self {
        let subscription = bus.subscribe_lossy_bounded(TopicFilter::All, 32_768);
        let config_root = dirs_next::config_dir()
            .map(|d| d.join("HardwareWorkbench").join("plugin-config"))
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("plugin-config")
            });

        Self {
            bus,
            transport,
            permission_manager: PermissionManager::default(),
            records: BTreeMap::new(),
            lua_runtimes: HashMap::new(),
            stopping_plugins: Vec::new(),
            registered_commands: HashMap::new(),
            roots: Vec::new(),
            subscription,
            dialog_request_sender: None,
            file_broker: None,
            line_buffers: Arc::new(Mutex::new(HashMap::new())),
            config_store: Arc::new(ConfigStore::new(config_root)),
            dropped_events: 0,
            last_seen_manager_dropped: 0,
            diagnostics: Vec::new(),
        }
    }

    pub fn set_workspace(&self, workspace: &Path) {
        // ConfigStore 在构造时已设置 root，后续可通过此方法通知。
        // 当前版本 ConfigStore 不动态迁移，新路径仅用于日志。
        let config_root = workspace.join("plugin-config");
        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "extension",
            format!("config workspace: {}", config_root.display()),
        ));
    }

    pub fn config_root(&self) -> &Path {
        self.config_store.root()
    }

    /// 获取 ConfigStore 引用，供设置面板读取/写入插件配置。
    pub fn config_store(&self) -> &Arc<ConfigStore> {
        &self.config_store
    }

    /// 返回所有已启用插件的设置定义：(plugin_id, plugin_name, settings)
    pub fn plugin_settings(&self) -> Vec<(String, String, Vec<crate::manifest::PluginSetting>)> {
        self.records
            .iter()
            .filter(|(_, record)| {
                !record.manifest.contributes.settings.is_empty()
                    && matches!(
                        record.state,
                        PluginState::Enabled | PluginState::Running | PluginState::Finished
                    )
            })
            .map(|(id, record)| {
                (
                    id.clone(),
                    record.manifest.name.clone(),
                    record.manifest.contributes.settings.clone(),
                )
            })
            .collect()
    }

    pub fn set_host_services(
        &mut self,
        dialog_sender: crossbeam_channel::Sender<DialogRequest>,
        broker: Arc<FileAccessBroker>,
    ) {
        self.dialog_request_sender = Some(dialog_sender);
        self.file_broker = Some(broker);
    }

    pub fn discover_roots(
        &mut self,
        roots: impl IntoIterator<Item = PathBuf>,
    ) -> ExtensionResult<usize> {
        self.roots = roots.into_iter().collect();
        self.refresh()
    }

    pub fn refresh(&mut self) -> ExtensionResult<usize> {
        self.diagnostics.clear();
        // 清理：移除所有之前从某个 root 发现但现已不存在的插件
        self.records.retain(|_, record| record.root.exists());

        let roots = self.roots.clone();
        let mut count = 0;

        for root in roots {
            count += self.discover_root(&root)?;
        }

        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "extension",
            format!("discovered {count} plugin(s)"),
        ));

        Ok(count)
    }

    pub fn discover_root(&mut self, root: &Path) -> ExtensionResult<usize> {
        if !root.exists() {
            return Ok(0);
        }

        let mut count = 0;

        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let manifest_path = path.join("plugin.json");

            if !manifest_path.exists() {
                continue;
            }

            let manifest = match load_manifest(&manifest_path) {
                Ok(m) => m,
                Err(e) => {
                    self.push_diagnostic(
                        PluginDiagnosticSeverity::Error,
                        "manifest_parse_error",
                        None,
                        path.clone(),
                        format!("manifest 解析失败: {e}"),
                    );
                    self.bus.publish(Event::system_log(
                        LogLevel::Warn,
                        "extension",
                        format!("跳过损坏插件 {}: {e}", path.display()),
                    ));
                    continue;
                }
            };

            if !manifest.api_version_supported() {
                self.push_diagnostic(
                    PluginDiagnosticSeverity::Warning,
                    "unsupported_api_version",
                    Some(manifest.id.clone()),
                    path.clone(),
                    format!(
                        "api_version '{}' 不受支持，当前支持 {}",
                        manifest.api_version,
                        SUPPORTED_PLUGIN_API_VERSIONS.join(", ")
                    ),
                );
                self.bus.publish(Event::system_log(
                    LogLevel::Warn,
                    "extension",
                    format!(
                        "跳过不兼容插件 {} ({}) : api_version '{}' 不受支持，当前支持 {}",
                        manifest.id,
                        path.display(),
                        manifest.api_version,
                        SUPPORTED_PLUGIN_API_VERSIONS.join(", ")
                    ),
                ));
                continue;
            }

            if let Err(errors) = manifest.validate() {
                self.push_diagnostic(
                    PluginDiagnosticSeverity::Error,
                    "manifest_validation_error",
                    Some(manifest.id.clone()),
                    path.clone(),
                    errors.join("; "),
                );
                self.bus.publish(Event::system_log(
                    LogLevel::Warn,
                    "extension",
                    format!(
                        "跳过无效插件 {} ({}) : {}",
                        manifest.id,
                        path.display(),
                        errors.join("; ")
                    ),
                ));
                continue;
            }

            if let Err(e) = self.permission_manager.check(&manifest) {
                self.push_diagnostic(
                    PluginDiagnosticSeverity::Error,
                    "permission_denied",
                    Some(manifest.id.clone()),
                    path.clone(),
                    e.to_string(),
                );
                self.bus.publish(Event::system_log(
                    LogLevel::Warn,
                    "extension",
                    format!("跳过无权限插件 {} ({}) : {e}", manifest.id, path.display()),
                ));
                continue;
            }

            let id = manifest.id.clone();

            // 重复 ID 检测：非同一目录时处理
            if let Some(existing) = self.records.get(&id)
                && existing.root != path
            {
                let is_running =
                    matches!(existing.state, PluginState::Running | PluginState::Enabled);
                self.bus.publish(Event::system_log(
                    LogLevel::Warn,
                    "extension",
                    format!(
                        "插件 ID 冲突: '{id}' 同时存在于 {} 和 {}",
                        existing.root.display(),
                        path.display()
                    ),
                ));
                if is_running {
                    self.push_diagnostic(
                        PluginDiagnosticSeverity::Warning,
                        "duplicate_plugin_id",
                        Some(id.clone()),
                        path.clone(),
                        format!(
                            "插件 ID 冲突，{} 已在运行，跳过 {}",
                            existing.root.display(),
                            path.display()
                        ),
                    );
                    self.bus.publish(Event::system_log(
                        LogLevel::Warn,
                        "extension",
                        format!(
                            "插件 '{id}' 已有实例在运行中，跳过 {} 的覆盖",
                            path.display()
                        ),
                    ));
                    continue;
                }
            }

            let existing_state = self
                .records
                .get(&id)
                .map(|record| record.state)
                .unwrap_or(PluginState::Discovered);

            self.records.insert(
                id,
                PluginRecord {
                    manifest,
                    root: path,
                    state: existing_state,
                    last_error: None,
                },
            );

            count += 1;
        }

        Ok(count)
    }

    pub fn enable(&mut self, plugin_id: &str) -> ExtensionResult<()> {
        self.update_runtime_states();
        // 收割已停止的插件，避免 restart 时误判为 Stopping
        self.reap_stopping_plugins();

        if self.lua_runtimes.contains_key(plugin_id) {
            return Err(ExtensionError::AlreadyEnabled(plugin_id.to_owned()));
        }

        // 检查是否正在关闭中，防止同一插件同时存在两个运行时。
        if self.stopping_plugins.iter().any(|(id, _)| id == plugin_id) {
            return Err(ExtensionError::Stopping(plugin_id.to_owned()));
        }

        let runtime = self
            .records
            .get(plugin_id)
            .ok_or_else(|| ExtensionError::NotFound(plugin_id.to_owned()))?
            .manifest
            .runtime
            .clone();

        match runtime.as_str() {
            "lua" => self.enable_lua(plugin_id),
            _ => Err(ExtensionError::UnsupportedRuntime(runtime)),
        }
    }

    fn enable_lua(&mut self, plugin_id: &str) -> ExtensionResult<()> {
        let record = self
            .records
            .get_mut(plugin_id)
            .ok_or_else(|| ExtensionError::NotFound(plugin_id.to_owned()))?;

        let main = record.manifest.live_main().to_owned();
        let script_path = record.root.join(&main);
        let script = fs::read_to_string(&script_path)?;
        let context = manifest_context(&record.manifest, &record.root);

        let host_services = tool_lua_host::LuaHostServices {
            plugin_root: Some(record.root.clone()),
            plugin_id: record.manifest.id.clone(),
            dialog_sender: self.dialog_request_sender.clone(),
            file_broker: self.file_broker.clone(),
            stop_flag: None,
            line_buffers: Some(self.line_buffers.clone()),
            config_store: Some(self.config_store.clone()),
        };

        let runtime = run_plugin(
            script,
            LuaRunConfig {
                script_name: format!("plugin:{}:{}", record.manifest.id, main),
                timeout_ms: 0,
                source: format!("plugin:{}", record.manifest.id),
                context,
                permissions: record.manifest.live_permissions().to_vec(),
            },
            self.bus.clone(),
            self.transport.clone(),
            host_services,
        )?;

        record.state = PluginState::Running;
        record.last_error = None;

        self.lua_runtimes.insert(plugin_id.to_owned(), runtime);

        self.bus.publish(Event::system_log(
            LogLevel::Info,
            format!("plugin:{plugin_id}"),
            "plugin enabled",
        ));

        Ok(())
    }

    pub fn disable(&mut self, plugin_id: &str) -> ExtensionResult<()> {
        if !self.records.contains_key(plugin_id) {
            return Err(ExtensionError::NotFound(plugin_id.to_owned()));
        }

        // 先标记为 Disabled：防止 Lua 侧在 stop 窗口内重新创建面板
        if let Some(record) = self.records.get_mut(plugin_id)
            && matches!(record.state, PluginState::Running | PluginState::Enabled)
        {
            record.state = PluginState::Disabled;
        }

        // 异步停止：设 stop 后移入 stopping_plugins，不 join（避免卡 UI）
        if let Some(runtime) = self.lua_runtimes.remove(plugin_id) {
            runtime.stop();
            self.stopping_plugins.push((plugin_id.to_owned(), runtime));
        }

        // 清除该插件的运行时注册命令
        self.registered_commands.remove(plugin_id);

        let panel_ids: Vec<String> = self
            .records
            .get(plugin_id)
            .map(|record| {
                record
                    .manifest
                    .contributes
                    .panels
                    .iter()
                    .map(|panel| panel.id.clone())
                    .collect()
            })
            .unwrap_or_default();

        let source = format!("plugin:{plugin_id}");

        for panel_id in panel_ids {
            self.bus.publish(Event::new(
                topics::UI_PANEL_REMOVE,
                source.clone(),
                Direction::Internal,
                Payload::Json(serde_json::json!({ "id": panel_id })),
            ));
        }

        self.bus.publish(Event::system_log(
            LogLevel::Info,
            format!("plugin:{plugin_id}"),
            "plugin stopping...",
        ));

        Ok(())
    }

    /// 收割已停止的插件线程。在 process_pending 中每帧调用。
    fn reap_stopping_plugins(&mut self) {
        self.stopping_plugins.retain(|(id, runtime)| {
            if runtime.is_alive() {
                return true;
            }
            self.bus.publish(Event::system_log(
                LogLevel::Info,
                format!("plugin:{id}"),
                "plugin stopped",
            ));
            false // remove — Drop will join the finished thread
        });
    }

    /// 清理已完成的运行时并更新插件状态。
    /// 调用后 `summaries()` 返回的 state 将反映最新运行时状态。
    pub fn reap_finished(&mut self) {
        self.update_runtime_states();
    }

    pub fn summaries(&self) -> Vec<PluginSummary> {
        self.records
            .values()
            .map(|record| {
                // 只在 Running 状态时做命令对账；未启用时 registered 一定为空，
                // 不应把所有声明命令都标为 missing。
                let (registered, missing, undeclared) =
                    if matches!(record.state, PluginState::Running) {
                        let reg = self
                            .registered_commands
                            .get(&record.manifest.id)
                            .cloned()
                            .unwrap_or_default();

                        let declared: Vec<String> = record
                            .manifest
                            .contributes
                            .commands
                            .iter()
                            .map(|c| c.id.clone())
                            .collect();

                        let declared_set: std::collections::HashSet<&str> =
                            declared.iter().map(String::as_str).collect();
                        let registered_set: std::collections::HashSet<&str> =
                            reg.iter().map(String::as_str).collect();

                        let miss: Vec<String> = declared
                            .iter()
                            .filter(|id| !registered_set.contains(id.as_str()))
                            .cloned()
                            .collect();

                        let undec: Vec<String> = reg
                            .iter()
                            .filter(|id| !declared_set.contains(id.as_str()))
                            .cloned()
                            .collect();

                        (reg, miss, undec)
                    } else {
                        (Vec::new(), Vec::new(), Vec::new())
                    };

                PluginSummary {
                    id: record.manifest.id.clone(),
                    name: record.manifest.name.clone(),
                    version: record.manifest.version.clone(),
                    api_version: record.manifest.api_version.clone(),
                    runtime: record.manifest.runtime.clone(),
                    state: record.state,
                    permissions: record.manifest.live_permissions().to_vec(),
                    contributes: record.manifest.contributes.clone(),
                    path: record.root.clone(),
                    last_error: record.last_error.clone(),
                    has_replay_analyzer: record.manifest.has_replay_analyzer(),
                    replay_subscriptions: record.manifest.replay_subscriptions().to_vec(),
                    replay_outputs: record.manifest.replay_outputs().to_vec(),
                    registered_commands: registered,
                    missing_commands: missing,
                    undeclared_commands: undeclared,
                }
            })
            .collect()
    }

    pub fn diagnostics(&self) -> &[PluginDiagnostic] {
        &self.diagnostics
    }

    /// 列出所有已发现插件中有 replay analyzer 的配置。
    /// 不需要插件处于 enabled 状态。
    pub fn replay_analyzer_entries(&self) -> Vec<ReplayAnalyzerEntry> {
        self.records
            .iter()
            .filter(|(_, r)| r.manifest.has_replay_analyzer())
            .map(|(id, r)| ReplayAnalyzerEntry {
                plugin_id: id.clone(),
                manifest: r.manifest.clone(),
                root: r.root.clone(),
            })
            .collect()
    }

    pub fn count(&self) -> usize {
        self.records.len()
    }

    pub fn process_pending(&mut self) -> usize {
        self.reap_stopping_plugins();

        // 暴露 PluginManager 入口队列丢包
        let manager_dropped = self.subscription.dropped_count();
        if manager_dropped > self.last_seen_manager_dropped {
            self.last_seen_manager_dropped = manager_dropped;
            self.bus.publish(Event::system_log(
                LogLevel::Warn,
                "extension",
                format!("插件事件队列溢出，已丢弃 {manager_dropped} 条，插件可能丢失事件"),
            ));
        }

        let mut count = 0;

        for _ in 0..MAX_PLUGIN_EVENTS_PER_FRAME {
            let Some(event) = self.subscription.try_recv() else {
                break;
            };

            count += self.process_event(&event);
        }

        count
    }

    pub fn process_event(&mut self, event: &Event) -> usize {
        // 关键：回放事件不再送进实时插件。
        // 否则 replay RX 会被 Lua 插件再次解析，重新发布 protocol.demo.sample，
        // 和录制文件里原有的 protocol.demo.sample 混在一起。
        if is_replay_event(event) {
            return 0;
        }

        // ── 管理面事件：命令注册/注销 ──
        if event.topic == topics::PLUGIN_COMMAND_REGISTERED {
            self.handle_command_registered(event);
            return 0;
        }
        if event.topic == topics::PLUGIN_COMMAND_UNREGISTERED {
            self.handle_command_unregistered(event);
            return 0;
        }

        // ui.contribution.set_value 是宿主侧事件，不进 Lua
        if event.topic == topics::UI_CONTRIBUTION_SET_VALUE {
            return 0;
        }

        // 命令执行前置检测：命令是否已注册
        if event.topic == topics::PLUGIN_COMMAND_EXECUTE {
            self.check_command_registered(event);
        }

        // 设置面板变更自动持久化
        if event.topic == topics::UI_FORM_CHANGED {
            self.persist_settings_change(event);
        }

        let mut count = 0;

        // 按插件 manifest 声明的 subscription 做 Rust 层过滤，
        // 避免串口 RX/TX 等高频事件无意义复制到所有 Lua 插件。
        for (plugin_id, runtime) in &self.lua_runtimes {
            if !runtime.is_alive() {
                continue;
            }
            // 检查订阅：live.subscriptions
            let wants = self.records.get(plugin_id).is_some_and(|record| {
                record
                    .manifest
                    .live_subscriptions()
                    .iter()
                    .any(|sub| tool_core::topic_matches(sub, &event.topic))
            });
            // ui.* / log.* / plugin.command.execute 系统事件始终接收
            let is_sys = event.topic.starts_with("ui.")
                || event.topic.starts_with("log.")
                || event.topic == topics::PLUGIN_COMMAND_EXECUTE;
            if !is_sys && !wants {
                continue;
            }
            if runtime.on_event(event) {
                count += 1;
            } else {
                self.dropped_events += 1;
                if self.dropped_events.is_multiple_of(1000) {
                    self.bus.publish(Event::system_log(
                        LogLevel::Warn,
                        "extension",
                        format!("dropped {} Lua events (queue full)", self.dropped_events),
                    ));
                }
            }
        }

        count
    }

    fn update_runtime_states(&mut self) {
        let ids: Vec<String> = self.lua_runtimes.keys().cloned().collect();
        let mut finished = Vec::new();

        for id in &ids {
            if let Some(runtime) = self.lua_runtimes.get(id)
                && !runtime.is_alive()
            {
                finished.push(id.clone());
            }
        }

        for id in &finished {
            let runtime = self.lua_runtimes.remove(id.as_str());

            if let Some(record) = self.records.get_mut(id.as_str()) {
                match runtime.and_then(|r| r.outcome()) {
                    None => {
                        record.state = PluginState::Finished;
                    }
                    Some(tool_lua_host::LuaRunState::Failed) => {
                        record.state = PluginState::Failed;
                        record.last_error = Some("plugin script failed with error".into());
                    }
                    Some(tool_lua_host::LuaRunState::Stopped) => {
                        record.state = PluginState::Disabled;
                    }
                    Some(tool_lua_host::LuaRunState::Finished)
                    | Some(tool_lua_host::LuaRunState::Idle) => {
                        record.state = PluginState::Finished;
                    }
                    Some(tool_lua_host::LuaRunState::Running) => {
                        // 不应该走到这里：alive=false 但 outcome=Running
                        record.state = PluginState::Finished;
                    }
                }
            }

            // 运行时结束后清除注册命令
            self.registered_commands.remove(id.as_str());
        }
    }

    fn handle_command_registered(&mut self, event: &Event) {
        let Payload::Json(payload) = &event.payload else {
            return;
        };
        let Some(plugin_id) = payload.get("plugin_id").and_then(serde_json::Value::as_str) else {
            return;
        };
        let Some(command) = payload.get("command").and_then(serde_json::Value::as_str) else {
            return;
        };

        let commands = self
            .registered_commands
            .entry(plugin_id.to_owned())
            .or_default();

        // 去重：同一命令重复 register 不重复记录
        if !commands.iter().any(|c| c == command) {
            commands.push(command.to_owned());
        }

        // 命令注册成功后清除对应的 command_not_found 诊断
        self.diagnostics.retain(|d| {
            !(d.code == "command_not_found"
                && d.plugin_id.as_deref() == Some(plugin_id)
                && d.message.contains(command))
        });
    }

    fn handle_command_unregistered(&mut self, event: &Event) {
        let Payload::Json(payload) = &event.payload else {
            return;
        };
        let Some(plugin_id) = payload.get("plugin_id").and_then(serde_json::Value::as_str) else {
            return;
        };
        let Some(command) = payload.get("command").and_then(serde_json::Value::as_str) else {
            return;
        };

        if let Some(commands) = self.registered_commands.get_mut(plugin_id) {
            commands.retain(|c| c != command);
        }
    }

    fn check_command_registered(&mut self, event: &Event) {
        let Payload::Json(payload) = &event.payload else {
            return;
        };
        let Some(plugin_id) = payload.get("plugin_id").and_then(serde_json::Value::as_str) else {
            return;
        };
        let Some(command) = payload.get("command").and_then(serde_json::Value::as_str) else {
            return;
        };

        // 只对 Running 状态的插件做诊断；未启用的插件没有 registered_commands 是正常的
        let is_running = self
            .records
            .get(plugin_id)
            .is_some_and(|r| matches!(r.state, PluginState::Running));
        if !is_running {
            return;
        }

        // 检查命令是否在 registered_commands 中
        let is_registered = self
            .registered_commands
            .get(plugin_id)
            .is_some_and(|cmds| cmds.iter().any(|c| c == command));

        if !is_registered {
            // 去重：如果已存在相同 plugin_id + command 的诊断，不重复添加
            let already_diagnosed = self.diagnostics.iter().any(|d| {
                d.code == "command_not_found"
                    && d.plugin_id.as_deref() == Some(plugin_id)
                    && d.message.contains(command)
            });
            if !already_diagnosed {
                let path = self
                    .records
                    .get(plugin_id)
                    .map(|r| r.root.clone())
                    .unwrap_or_default();
                self.push_diagnostic(
                    PluginDiagnosticSeverity::Warning,
                    "command_not_found",
                    Some(plugin_id.to_owned()),
                    path,
                    format!("命令 '{command}' 未注册，点击或快捷键触发无效"),
                );
            }
        }
    }

    /// 设置面板变更时自动持久化到 ConfigStore。
    fn persist_settings_change(&mut self, event: &Event) {
        let Payload::Json(payload) = &event.payload else {
            return;
        };

        let Some(panel_id) = payload.get("panel_id").and_then(serde_json::Value::as_str) else {
            return;
        };

        // 只处理设置面板（panel_id 以 .settings 结尾）
        let Some(plugin_id) = panel_id.strip_suffix(".settings") else {
            return;
        };

        let Some(values) = payload.get("values").and_then(serde_json::Value::as_object) else {
            return;
        };

        for (key, value) in values {
            let _ = self.config_store.set(plugin_id, key, value.clone());
        }
    }

    fn push_diagnostic(
        &mut self,
        severity: PluginDiagnosticSeverity,
        code: &'static str,
        plugin_id: Option<String>,
        path: PathBuf,
        message: String,
    ) {
        self.diagnostics.push(PluginDiagnostic::new(
            severity, code, plugin_id, path, message,
        ));
    }
}

fn load_manifest(path: &Path) -> ExtensionResult<PluginManifest> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

fn manifest_context(manifest: &PluginManifest, root: &Path) -> serde_json::Value {
    json!({
        "id": manifest.id,
        "name": manifest.name,
        "version": manifest.version,
        "api_version": manifest.api_version,
        "runtime": manifest.runtime,
        "root": root.display().to_string(),
        "permissions": manifest.permissions,
        "contributes": manifest.contributes
    })
}

fn is_replay_event(event: &Event) -> bool {
    event.is_replay()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::PermissionManager;
    use tool_core::topics;
    use tool_databus::TopicFilter;

    #[test]
    fn discovers_manifest_and_enables_lua_plugin() {
        let root = create_test_plugin(
            "builtin.pid-tuner",
            r#"ctx.log.info('activated ' .. ctx.plugin.id)
ctx.bus.publish('protocol.pid.sample', { t = 1, target = 50, actual = 43, output = 0.71 })"#,
        );

        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let rx = bus.subscribe(TopicFilter::exact(topics::PROTOCOL_PID_SAMPLE));

        let mut manager = PluginManager::new(bus, transport);

        assert_eq!(manager.discover_roots([root.clone()]).unwrap(), 1);

        manager.enable("builtin.pid-tuner").unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));

        assert_eq!(manager.count(), 1);
        assert_eq!(rx.drain().len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unknown_permission() {
        let manifest = PluginManifest {
            id: "bad".to_owned(),
            name: "Bad".to_owned(),
            version: "0.1.0".to_owned(),
            api_version: crate::manifest::CURRENT_PLUGIN_API_VERSION.to_owned(),
            runtime: "lua".to_owned(),
            main: "main.lua".to_owned(),
            permissions: vec!["filesystem".to_owned()],
            contributes: crate::manifest::PluginContributes::default(),
            live: None,
            replay: None,
        };

        assert!(PermissionManager::default().check(&manifest).is_err());
    }

    #[test]
    fn skips_unsupported_plugin_api_version() {
        let root = create_test_plugin_with_api_version("future.plugin", "99.0");

        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut manager = PluginManager::new(bus, transport);

        assert_eq!(manager.discover_roots([root.clone()]).unwrap(), 0);
        assert_eq!(manager.count(), 0);
        let diagnostics = manager.diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, PluginDiagnosticSeverity::Warning);
        assert_eq!(diagnostics[0].code, "unsupported_api_version");
        assert_eq!(diagnostics[0].plugin_id.as_deref(), Some("future.plugin"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn records_manifest_parse_diagnostic() {
        let root = create_broken_plugin("broken.plugin", "{ not json");

        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut manager = PluginManager::new(bus, transport);

        assert_eq!(manager.discover_roots([root.clone()]).unwrap(), 0);
        assert_eq!(manager.count(), 0);
        let diagnostics = manager.diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, PluginDiagnosticSeverity::Error);
        assert_eq!(diagnostics[0].code, "manifest_parse_error");
        assert_eq!(diagnostics[0].plugin_id, None);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skips_invalid_manifest_with_diagnostic() {
        let root = create_custom_plugin(
            "bad.ui",
            r#"{
  "id": "bad.ui",
  "name": "Bad UI",
  "version": "0.1.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["bus"],
  "contributes": {
    "ui": [
      { "id": "bad.ui.button", "slot": "send.toolbar", "command": "bad.ui.missing" }
    ]
  }
}"#,
            "ctx.log.info('bad ui')",
        );

        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut manager = PluginManager::new(bus, transport);

        assert_eq!(manager.discover_roots([root.clone()]).unwrap(), 0);
        assert_eq!(manager.count(), 0);
        let diagnostics = manager.diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, PluginDiagnosticSeverity::Error);
        assert_eq!(diagnostics[0].code, "manifest_validation_error");
        assert_eq!(diagnostics[0].plugin_id.as_deref(), Some("bad.ui"));
        assert!(diagnostics[0].message.contains("bad.ui.missing"));

        let _ = fs::remove_dir_all(root);
    }

    // ── 命令追踪测试 ──

    #[test]
    fn tracks_registered_commands_after_enable() {
        let root = create_custom_plugin(
            "cmd.tracker",
            r#"{
  "id": "cmd.tracker",
  "name": "Command Tracker",
  "version": "0.1.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["log"],
  "contributes": {
    "commands": [
      { "id": "cmd.tracker.run", "title": "Run" },
      { "id": "cmd.tracker.stop", "title": "Stop" }
    ]
  }
}"#,
            r#"
ctx.commands.register("cmd.tracker.run", function() end)
ctx.commands.register("cmd.tracker.stop", function() end)
ctx.log.info("registered")
"#,
        );

        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut manager = PluginManager::new(bus, transport);

        assert_eq!(manager.discover_roots([root.clone()]).unwrap(), 1);
        manager.enable("cmd.tracker").unwrap();

        // 等待 Lua 线程执行 + registered 事件到达 + process_pending 处理
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            manager.process_pending();
        }

        let summaries = manager.summaries();
        let s = summaries.iter().find(|s| s.id == "cmd.tracker").unwrap();
        assert!(
            s.registered_commands
                .contains(&"cmd.tracker.run".to_owned()),
            "registered_commands should contain cmd.tracker.run, got {:?}",
            s.registered_commands
        );
        assert!(
            s.registered_commands
                .contains(&"cmd.tracker.stop".to_owned()),
            "registered_commands should contain cmd.tracker.stop, got {:?}",
            s.registered_commands
        );
        // 声明且已注册 → missing 为空
        assert!(
            s.missing_commands.is_empty(),
            "missing_commands should be empty, got {:?}",
            s.missing_commands
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn detects_missing_declared_commands() {
        // manifest 声明了命令，但 Lua 代码不注册
        let root = create_custom_plugin(
            "cmd.missing",
            r#"{
  "id": "cmd.missing",
  "name": "Missing Commands",
  "version": "0.1.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["log"],
  "contributes": {
    "commands": [
      { "id": "cmd.missing.apply", "title": "Apply" }
    ]
  }
}"#,
            r#"ctx.log.info("no command registered")"#,
        );

        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut manager = PluginManager::new(bus, transport);

        assert_eq!(manager.discover_roots([root.clone()]).unwrap(), 1);
        manager.enable("cmd.missing").unwrap();

        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            manager.process_pending();
        }

        let summaries = manager.summaries();
        let s = summaries.iter().find(|s| s.id == "cmd.missing").unwrap();
        assert!(
            s.missing_commands.contains(&"cmd.missing.apply".to_owned()),
            "missing_commands should contain cmd.missing.apply, got {:?}",
            s.missing_commands
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn detects_undeclared_dynamic_commands() {
        // Lua 代码注册了 manifest 未声明的命令
        let root = create_custom_plugin(
            "cmd.dynamic",
            r#"{
  "id": "cmd.dynamic",
  "name": "Dynamic Commands",
  "version": "0.1.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["log"],
  "contributes": {}
}"#,
            r#"
ctx.commands.register("cmd.dynamic.secret", function() end)
ctx.log.info("registered dynamic")
"#,
        );

        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut manager = PluginManager::new(bus, transport);

        assert_eq!(manager.discover_roots([root.clone()]).unwrap(), 1);
        manager.enable("cmd.dynamic").unwrap();

        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            manager.process_pending();
        }

        let summaries = manager.summaries();
        let s = summaries.iter().find(|s| s.id == "cmd.dynamic").unwrap();
        assert!(
            s.undeclared_commands
                .contains(&"cmd.dynamic.secret".to_owned()),
            "undeclared_commands should contain cmd.dynamic.secret, got {:?}",
            s.undeclared_commands
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn clears_registered_commands_on_disable() {
        let root = create_custom_plugin(
            "cmd.cleanup",
            r#"{
  "id": "cmd.cleanup",
  "name": "Cleanup Test",
  "version": "0.1.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["log"],
  "contributes": {
    "commands": [
      { "id": "cmd.cleanup.run", "title": "Run" }
    ]
  }
}"#,
            r#"
ctx.commands.register("cmd.cleanup.run", function() end)
ctx.log.info("registered")
"#,
        );

        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut manager = PluginManager::new(bus, transport);

        assert_eq!(manager.discover_roots([root.clone()]).unwrap(), 1);
        manager.enable("cmd.cleanup").unwrap();

        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            manager.process_pending();
        }

        // 确认注册了
        assert!(manager.registered_commands.contains_key("cmd.cleanup"));

        manager.disable("cmd.cleanup").unwrap();

        // disable 后应清除
        assert!(
            !manager.registered_commands.contains_key("cmd.cleanup"),
            "registered_commands should be cleared after disable"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn diagnoses_command_not_found_on_execute() {
        let root = create_custom_plugin(
            "cmd.notfound",
            r#"{
  "id": "cmd.notfound",
  "name": "Not Found Test",
  "version": "0.1.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["log"],
  "contributes": {
    "commands": [
      { "id": "cmd.notfound.run", "title": "Run" }
    ]
  }
}"#,
            r#"ctx.log.info("no command registered")"#,
        );

        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut manager = PluginManager::new(bus, transport);

        assert_eq!(manager.discover_roots([root.clone()]).unwrap(), 1);
        manager.enable("cmd.notfound").unwrap();

        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            manager.process_pending();
        }

        // 模拟触发一个未注册的命令
        let event = Event::new(
            topics::PLUGIN_COMMAND_EXECUTE,
            "test",
            Direction::Internal,
            Payload::Json(json!({
                "plugin_id": "cmd.notfound",
                "command": "cmd.notfound.run",
                "origin": "test"
            })),
        );
        manager.process_event(&event);

        let diagnostics = manager.diagnostics();
        let not_found = diagnostics.iter().find(|d| {
            d.code == "command_not_found" && d.plugin_id.as_deref() == Some("cmd.notfound")
        });
        assert!(
            not_found.is_some(),
            "should have command_not_found diagnostic, got {:?}",
            diagnostics
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn command_dedup_on_repeated_register() {
        let root = create_custom_plugin(
            "cmd.dedup",
            r#"{
  "id": "cmd.dedup",
  "name": "Dedup Test",
  "version": "0.1.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["log"],
  "contributes": {
    "commands": [
      { "id": "cmd.dedup.run", "title": "Run" }
    ]
  }
}"#,
            r#"
ctx.commands.register("cmd.dedup.run", function() end)
-- 重复注册同一命令
ctx.commands.register("cmd.dedup.run", function() end)
ctx.log.info("registered twice")
"#,
        );

        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut manager = PluginManager::new(bus, transport);

        assert_eq!(manager.discover_roots([root.clone()]).unwrap(), 1);
        manager.enable("cmd.dedup").unwrap();

        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            manager.process_pending();
        }

        let summaries = manager.summaries();
        let s = summaries.iter().find(|s| s.id == "cmd.dedup").unwrap();
        // 重复注册不应重复计数
        let count = s
            .registered_commands
            .iter()
            .filter(|c| c.as_str() == "cmd.dedup.run")
            .count();
        assert_eq!(count, 1, "duplicate register should not duplicate entries");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn command_not_found_cleared_on_register() {
        let root = create_custom_plugin(
            "cmd.late",
            r#"{
  "id": "cmd.late",
  "name": "Late Register",
  "version": "0.1.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["log"],
  "contributes": {
    "commands": [
      { "id": "cmd.late.run", "title": "Run" }
    ]
  }
}"#,
            r#"
-- 先不注册，等宿主发一个 execute 后再注册
ctx.log.info("started")
"#,
        );

        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut manager = PluginManager::new(bus, transport);

        assert_eq!(manager.discover_roots([root.clone()]).unwrap(), 1);
        manager.enable("cmd.late").unwrap();

        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            manager.process_pending();
        }

        // 模拟触发未注册命令 → 产生诊断
        let exec_event = Event::new(
            topics::PLUGIN_COMMAND_EXECUTE,
            "test",
            Direction::Internal,
            Payload::Json(json!({
                "plugin_id": "cmd.late",
                "command": "cmd.late.run",
                "origin": "test"
            })),
        );
        manager.process_event(&exec_event);
        assert!(
            manager
                .diagnostics()
                .iter()
                .any(|d| d.code == "command_not_found"),
            "should have command_not_found before register"
        );

        // 模拟注册事件 → 应清除诊断
        let reg_event = Event::new(
            topics::PLUGIN_COMMAND_REGISTERED,
            "plugin:cmd.late",
            Direction::Internal,
            Payload::Json(json!({
                "plugin_id": "cmd.late",
                "command": "cmd.late.run"
            })),
        );
        manager.process_event(&reg_event);
        assert!(
            !manager
                .diagnostics()
                .iter()
                .any(|d| d.code == "command_not_found"),
            "command_not_found should be cleared after register"
        );

        let _ = fs::remove_dir_all(root);
    }

    fn create_test_plugin(id: &str, main_lua: &str) -> PathBuf {
        create_test_plugin_inner(id, None, main_lua)
    }

    fn create_test_plugin_with_api_version(id: &str, api_version: &str) -> PathBuf {
        create_test_plugin_inner(id, Some(api_version), "ctx.log.info('future')")
    }

    fn create_broken_plugin(id: &str, manifest_text: &str) -> PathBuf {
        create_custom_plugin(id, manifest_text, "")
    }

    fn create_custom_plugin(id: &str, manifest_text: &str, main_lua: &str) -> PathBuf {
        let safe_id = id.replace(['.', ':', '\\', '/'], "-");
        let root = std::env::temp_dir().join(format!(
            "hardware-workbench-plugin-test-custom-{}-{}",
            safe_id,
            tool_core::now_timestamp_ms()
        ));
        let plugin_root = root.join(id);

        fs::create_dir_all(&plugin_root).unwrap();
        fs::write(plugin_root.join("plugin.json"), manifest_text).unwrap();
        fs::write(plugin_root.join("main.lua"), main_lua).unwrap();

        root
    }

    fn create_test_plugin_inner(id: &str, api_version: Option<&str>, main_lua: &str) -> PathBuf {
        let safe_id = id.replace(['.', ':', '\\', '/'], "-");
        let root = std::env::temp_dir().join(format!(
            "hardware-workbench-plugin-test-{}-{}",
            safe_id,
            tool_core::now_timestamp_ms()
        ));

        let plugin_root = root.join(id);

        fs::create_dir_all(&plugin_root).unwrap();

        let api_version_line = api_version
            .map(|version| format!(r#"  "api_version": "{version}","#))
            .unwrap_or_default();
        fs::write(
            plugin_root.join("plugin.json"),
            format!(
                r#"{{
  "id": "{id}",
  "name": "PID Tuner",
  "version": "0.1.0",
{api_version_line}
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["bus", "log", "serial", "ui"],
  "contributes": {{
    "commands": [{{ "id": "{id}.apply", "title": "Apply PID" }}],
    "panels": [{{ "id": "{id}.chart", "title": "PID Chart", "kind": "chart" }}]
  }}
}}"#
            ),
        )
        .unwrap();

        fs::write(plugin_root.join("main.lua"), main_lua).unwrap();

        root
    }
}
