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
use crate::manifest::{PluginManifest, PluginState, PluginSummary, ReplayAnalyzerEntry};
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
    roots: Vec<PathBuf>,
    subscription: Subscription,
    dialog_request_sender: Option<crossbeam_channel::Sender<DialogRequest>>,
    file_broker: Option<Arc<FileAccessBroker>>,
    line_buffers: LineBufferMap,
    config_store: Arc<ConfigStore>,
    dropped_events: u64,
    last_seen_manager_dropped: u64,
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
            roots: Vec::new(),
            subscription,
            dialog_request_sender: None,
            file_broker: None,
            line_buffers: Arc::new(Mutex::new(HashMap::new())),
            config_store: Arc::new(ConfigStore::new(config_root)),
            dropped_events: 0,
            last_seen_manager_dropped: 0,
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
                    self.bus.publish(Event::system_log(
                        LogLevel::Warn,
                        "extension",
                        format!("跳过损坏插件 {}: {e}", path.display()),
                    ));
                    continue;
                }
            };

            if let Err(e) = self.permission_manager.check(&manifest) {
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
            .map(|record| PluginSummary {
                id: record.manifest.id.clone(),
                name: record.manifest.name.clone(),
                version: record.manifest.version.clone(),
                runtime: record.manifest.runtime.clone(),
                state: record.state,
                permissions: record.manifest.live_permissions().to_vec(),
                contributes: record.manifest.contributes.clone(),
                path: record.root.clone(),
                last_error: record.last_error.clone(),
                has_replay_analyzer: record.manifest.has_replay_analyzer(),
                replay_subscriptions: record.manifest.replay_subscriptions().to_vec(),
                replay_outputs: record.manifest.replay_outputs().to_vec(),
            })
            .collect()
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

        let mut count = 0;

        // 按插件 manifest 声明的 subscription 做 Rust 层过滤，
        // 避免串口 RX/TX 等高频事件无意义复制到所有 Lua 插件。
        for (plugin_id, runtime) in &self.lua_runtimes {
            if !runtime.is_alive() {
                continue;
            }
            // 检查订阅：live.subscriptions + 兼容旧 contributes.subscriptions
            let wants = self.records.get(plugin_id).is_some_and(|record| {
                let live = record
                    .manifest
                    .live_subscriptions()
                    .iter()
                    .map(String::as_str);
                let legacy = record
                    .manifest
                    .contributes
                    .subscriptions
                    .iter()
                    .map(|s| s.topic.as_str());
                live.chain(legacy)
                    .any(|sub| tool_core::topic_matches(sub, &event.topic))
            });
            // ui.* / log.* 系统事件始终接收
            let is_sys = event.topic.starts_with("ui.") || event.topic.starts_with("log.");
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
        }
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
            runtime: "lua".to_owned(),
            main: "main.lua".to_owned(),
            permissions: vec!["filesystem".to_owned()],
            contributes: crate::manifest::PluginContributes::default(),
            live: None,
            replay: None,
        };

        assert!(PermissionManager::default().check(&manifest).is_err());
    }

    fn create_test_plugin(id: &str, main_lua: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "hardware-workbench-plugin-test-{}",
            tool_core::now_timestamp_ms()
        ));

        let plugin_root = root.join(id);

        fs::create_dir_all(&plugin_root).unwrap();

        fs::write(
            plugin_root.join("plugin.json"),
            format!(
                r#"{{
  "id": "{id}",
  "name": "PID Tuner",
  "version": "0.1.0",
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
