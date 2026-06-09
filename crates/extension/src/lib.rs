use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tool_core::{Direction, Event, LogLevel, Payload, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};
use tool_lua_host::{LuaPluginRuntime, LuaRunConfig, run_plugin};
use tool_transport::TransportManager;
use tool_wasm_host::{WasmPluginConfig, WasmPluginRuntime};

pub mod host_services;
use host_services::{DialogRequest, FileAccessBroker};

const MAX_PLUGIN_EVENTS_PER_FRAME: usize = 500;

#[derive(Debug, Error)]
pub enum ExtensionError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("manifest parse error: {0}")]
    Manifest(#[from] serde_json::Error),

    #[error("plugin '{0}' was not found")]
    NotFound(String),

    #[error("plugin '{0}' is already enabled")]
    AlreadyEnabled(String),

    #[error("unsupported runtime '{0}'")]
    UnsupportedRuntime(String),

    #[error("permission '{permission}' is not allowed for plugin '{plugin_id}'")]
    PermissionDenied {
        plugin_id: String,
        permission: String,
    },

    #[error("lua error: {0}")]
    Lua(#[from] tool_lua_host::LuaHostError),

    #[error("wasm error: {0}")]
    Wasm(#[from] tool_wasm_host::WasmHostError),
}

pub type ExtensionResult<T> = Result<T, ExtensionError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub runtime: String,
    /// 默认入口（live.replay 不存在时使用）
    pub main: String,

    #[serde(default)]
    pub permissions: Vec<String>,

    #[serde(default)]
    pub contributes: PluginContributes,

    /// 实时插件配置（可选，不填时回退到顶层 main/permissions）
    #[serde(default)]
    pub live: Option<LiveConfig>,

    /// 回放解析器配置（可选）
    #[serde(default)]
    pub replay: Option<ReplayConfig>,
}

impl PluginManifest {
    pub fn live_main(&self) -> &str {
        self.live
            .as_ref()
            .and_then(|l| l.main.as_deref())
            .unwrap_or(&self.main)
    }

    pub fn live_permissions(&self) -> &[String] {
        self.live
            .as_ref()
            .and_then(|l| l.permissions.as_ref())
            .unwrap_or(&self.permissions)
    }

    pub fn has_replay_analyzer(&self) -> bool {
        self.replay.is_some()
    }

    pub fn replay_main(&self) -> Option<&str> {
        self.replay.as_ref().map(|r| r.main.as_str())
    }

    pub fn replay_permissions(&self) -> &[String] {
        self.replay
            .as_ref()
            .map(|r| r.permissions.as_slice())
            .unwrap_or(&[])
    }

    pub fn replay_subscriptions(&self) -> &[String] {
        self.replay
            .as_ref()
            .map(|r| r.subscriptions.as_slice())
            .unwrap_or(&[])
    }

    pub fn replay_outputs(&self) -> &[String] {
        self.replay
            .as_ref()
            .map(|r| r.outputs.as_slice())
            .unwrap_or(&[])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveConfig {
    #[serde(default)]
    pub main: Option<String>,

    #[serde(default)]
    pub permissions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayConfig {
    pub main: String,

    #[serde(default)]
    pub subscriptions: Vec<String>,

    #[serde(default)]
    pub outputs: Vec<String>,

    #[serde(default)]
    pub permissions: Vec<String>,
}

/// 已发现插件中 replay analyzer 的元信息。
/// 不需要插件处于 enabled 状态。
#[derive(Debug, Clone)]
pub struct ReplayAnalyzerEntry {
    pub plugin_id: String,
    pub manifest: PluginManifest,
    pub root: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginContributes {
    #[serde(default)]
    pub commands: Vec<PluginCommand>,

    #[serde(default)]
    pub panels: Vec<PluginPanelContribution>,

    #[serde(default)]
    pub settings: Vec<PluginSetting>,

    #[serde(default)]
    pub subscriptions: Vec<PluginSubscription>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCommand {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginPanelContribution {
    pub id: String,
    pub title: String,

    #[serde(default)]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginSetting {
    pub id: String,
    pub title: String,

    #[serde(default)]
    pub default: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginSubscription {
    pub topic: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PluginState {
    Discovered,
    Enabled,
    Running,
    Finished,
    Failed,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub runtime: String,
    pub state: PluginState,
    pub permissions: Vec<String>,
    pub contributes: PluginContributes,
    pub path: PathBuf,
    pub last_error: Option<String>,

    // ── replay analyzer ──
    pub has_replay_analyzer: bool,
    pub replay_subscriptions: Vec<String>,
    pub replay_outputs: Vec<String>,
}

struct PluginRecord {
    manifest: PluginManifest,
    root: PathBuf,
    state: PluginState,
    last_error: Option<String>,
}

/// Replay analyzer 只允许 log 和 storage 权限。
const REPLAY_ALLOWED_PERMISSIONS: &[&str] = &["log", "storage"];

pub struct PermissionManager {
    allowed: BTreeSet<String>,
}

impl PermissionManager {
    pub fn new(allowed: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowed: allowed.into_iter().map(Into::into).collect(),
        }
    }

    pub fn check(&self, manifest: &PluginManifest) -> ExtensionResult<()> {
        // 检查 live 权限（用 self.allowed）
        for permission in manifest.live_permissions() {
            if !self.allowed.contains(permission) {
                return Err(ExtensionError::PermissionDenied {
                    plugin_id: manifest.id.clone(),
                    permission: permission.clone(),
                });
            }
        }

        // 检查 replay 权限（只允许 log / storage）
        for permission in manifest.replay_permissions() {
            if !REPLAY_ALLOWED_PERMISSIONS.contains(&permission.as_str()) {
                return Err(ExtensionError::PermissionDenied {
                    plugin_id: manifest.id.clone(),
                    permission: permission.clone(),
                });
            }
        }

        Ok(())
    }
}

impl Default for PermissionManager {
    fn default() -> Self {
        Self::new([
            "bus",
            "log",
            "serial",
            "ui",
            "storage",
            "timer",
            "testing",
            "dialog",
            "fs.read.user_selected",
        ])
    }
}

pub struct PluginManager {
    bus: DataBus,
    transport: TransportManager,
    permission_manager: PermissionManager,
    records: BTreeMap<String, PluginRecord>,
    lua_runtimes: HashMap<String, LuaPluginRuntime>,
    wasm_runtimes: HashMap<String, WasmPluginRuntime>,
    roots: Vec<PathBuf>,
    subscription: Subscription,
    dialog_request_sender: Option<crossbeam_channel::Sender<DialogRequest>>,
    file_broker: Option<Arc<FileAccessBroker>>,
}

impl PluginManager {
    pub fn new(bus: DataBus, transport: TransportManager) -> Self {
        let subscription = bus.subscribe(TopicFilter::All);

        Self {
            bus,
            transport,
            permission_manager: PermissionManager::default(),
            records: BTreeMap::new(),
            lua_runtimes: HashMap::new(),
            wasm_runtimes: HashMap::new(),
            roots: Vec::new(),
            subscription,
            dialog_request_sender: None,
            file_broker: None,
        }
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

            let manifest = load_manifest(&manifest_path)?;
            self.permission_manager.check(&manifest)?;

            let id = manifest.id.clone();

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

        if self.lua_runtimes.contains_key(plugin_id) || self.wasm_runtimes.contains_key(plugin_id) {
            return Err(ExtensionError::AlreadyEnabled(plugin_id.to_owned()));
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
            "wasm" | "wasmtime" => self.enable_wasm(plugin_id),
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

    fn enable_wasm(&mut self, plugin_id: &str) -> ExtensionResult<()> {
        let record = self
            .records
            .get_mut(plugin_id)
            .ok_or_else(|| ExtensionError::NotFound(plugin_id.to_owned()))?;

        let mut config = WasmPluginConfig::new(
            &record.manifest.id,
            &record.manifest.name,
            record.root.join(&record.manifest.main),
        );

        config.permissions = record.manifest.permissions.clone();

        config.initial_subscriptions = record
            .manifest
            .contributes
            .subscriptions
            .iter()
            .map(|subscription| subscription.topic.clone())
            .collect();

        let runtime = match WasmPluginRuntime::load(self.bus.clone(), config) {
            Ok(runtime) => runtime,
            Err(error) => {
                record.state = PluginState::Failed;
                record.last_error = Some(error.to_string());
                return Err(error.into());
            }
        };

        if let Err(error) = runtime.activate() {
            record.state = PluginState::Failed;
            record.last_error = Some(error.to_string());
            return Err(error.into());
        }

        record.state = PluginState::Enabled;
        record.last_error = None;

        self.wasm_runtimes.insert(plugin_id.to_owned(), runtime);

        self.bus.publish(Event::system_log(
            LogLevel::Info,
            format!("plugin:{plugin_id}"),
            "wasm plugin enabled",
        ));

        Ok(())
    }

    pub fn disable(&mut self, plugin_id: &str) -> ExtensionResult<()> {
        if !self.records.contains_key(plugin_id) {
            return Err(ExtensionError::NotFound(plugin_id.to_owned()));
        }

        if let Some(runtime) = self.lua_runtimes.remove(plugin_id) {
            runtime.stop();
        }

        if let Some(runtime) = self.wasm_runtimes.remove(plugin_id)
            && let Err(error) = runtime.deactivate()
            && let Some(record) = self.records.get_mut(plugin_id)
        {
            record.last_error = Some(error.to_string());
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

        if let Some(record) = self.records.get_mut(plugin_id) {
            record.state = PluginState::Disabled;
        }

        self.bus.publish(Event::system_log(
            LogLevel::Info,
            format!("plugin:{plugin_id}"),
            "plugin disabled",
        ));

        Ok(())
    }

    pub fn summaries(&mut self) -> Vec<PluginSummary> {
        self.update_runtime_states();

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

        let ids = self
            .wasm_runtimes
            .iter()
            .filter(|(id, runtime)| {
                runtime.is_subscribed(&event.topic) && event.source != format!("wasm:{id}")
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();

        let mut count = 0;

        for id in ids {
            let result = self
                .wasm_runtimes
                .get(&id)
                .map(|runtime| runtime.on_event(event));

            match result {
                Some(Ok(true)) => count += 1,
                Some(Ok(false)) | None => {}
                Some(Err(error)) => {
                    let error_text = error.to_string();

                    if let Some(record) = self.records.get_mut(&id) {
                        record.state = PluginState::Failed;
                        record.last_error = Some(error_text.clone());
                    }

                    self.bus.publish(Event::system_log(
                        LogLevel::Warn,
                        format!("plugin:{id}"),
                        error_text,
                    ));
                }
            }
        }

        for runtime in self.lua_runtimes.values() {
            if runtime.is_alive() && runtime.on_event(event) {
                count += 1;
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
            self.lua_runtimes.remove(id);

            if let Some(record) = self.records.get_mut(id) {
                record.state = PluginState::Finished;
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

#[cfg(test)]
mod tests {
    use super::*;
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
            contributes: PluginContributes::default(),
            live: None,
            replay: None,
        };

        assert!(PermissionManager::default().check(&manifest).is_err());
    }

    #[test]
    fn old_manifest_without_live_replay_is_compatible() {
        let json = r#"{
          "id": "demo.test",
          "name": "Test",
          "version": "1.0.0",
          "runtime": "lua",
          "main": "main.lua",
          "permissions": ["bus", "log", "ui"]
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.live_main(), "main.lua");
        assert_eq!(manifest.live_permissions().len(), 3);
        assert!(!manifest.has_replay_analyzer());
        assert!(manifest.replay_main().is_none());
    }

    #[test]
    fn new_manifest_with_live_and_replay() {
        let json = r#"{
          "id": "demo.test",
          "name": "Test",
          "version": "1.0.0",
          "runtime": "lua",
          "main": "main.lua",
          "permissions": ["bus"],
          "live": {
            "main": "live.lua",
            "permissions": ["bus", "log", "serial", "ui"]
          },
          "replay": {
            "main": "replay.lua",
            "subscriptions": ["transport.serial.default.rx"],
            "outputs": ["protocol.demo.sample"],
            "permissions": ["log", "storage"]
          }
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.live_main(), "live.lua");
        assert_eq!(manifest.live_permissions().len(), 4);
        assert!(manifest.has_replay_analyzer());
        assert_eq!(manifest.replay_main(), Some("replay.lua"));
        assert_eq!(manifest.replay_subscriptions().len(), 1);
        assert_eq!(manifest.replay_outputs().len(), 1);
        assert_eq!(manifest.replay_permissions().len(), 2);
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
fn is_replay_event(event: &Event) -> bool {
    event.is_replay()
}
