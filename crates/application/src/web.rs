//! Browser Application composition: AppCommand → TaskId → Promise worker → AppEvent.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::rc::Rc;

use tool_core::{Direction, Event, Payload};
use tool_databus::{DataBus, RingSubscription, Subscription, TopicFilter};
use tool_platform::web_network::WebNetworkTransport;
use tool_platform::web_serial::{WebSerialEvent, WebSerialTransport};
use tool_platform::{
    NetworkSerialConfig, PortDescriptor, PortId, TransportBackend, TransportCapabilities,
    serial_rx_event, serial_tx_event,
};
use wasm_bindgen_futures::spawn_local;

use crate::TransportView;
use crate::command::{AppCommand, CommandOutcome};
use crate::marketplace::{MarketplacePluginView, MarketplaceStatusView, MarketplaceView};
use crate::plugin::{PluginCommand, PluginView};
use crate::recording::{RecordModeView, RecorderStatsView, RecordingStatusView};
use crate::replay::{
    ReplayBlockReasonView, ReplayBookmarkView, ReplayLoadReportView, ReplayPolicyView,
    ReplayStateView, ReplayStatusView,
};
use crate::task_model::{TaskId, TaskSnapshot, TaskState};
use crate::updater::UpdateStatusView;
use serde::Deserialize;
use tool_recorder::{ReplayManager, ReplayPolicy, ReplayTextLoader};

pub type RepaintWaker = Rc<dyn Fn()>;

/// Web composition runtime. The alias keeps the platform implementation name
/// out of the UI boundary while preserving the existing Web Serial service.
pub type WebRuntime = WebApplication;

const WEB_UPDATE_INFO_URL: &str =
    "https://raw.githubusercontent.com/Tydwdh/serial_tool/main/update.json";

#[derive(Debug, Clone)]
pub enum WebAppEvent {
    TaskStateChanged(TaskSnapshot),
    TextLoaded {
        id: TaskId,
        kind: String,
        name: String,
        text: String,
    },
    FilesLoaded {
        id: TaskId,
        kind: String,
        files: Vec<(String, String)>,
    },
    PortsRefreshed(Vec<PortDescriptor>),
    PortRequested {
        id: TaskId,
        port: PortDescriptor,
    },
    PortAttached(PortDescriptor),
    PortDetached(PortId),
    NetworkPortAdded(PortDescriptor),
    NetworkPortRemoved(PortId),
    Connected {
        port: PortId,
    },
    Disconnected {
        port: PortId,
    },
    Sent {
        id: TaskId,
        port: PortId,
        bytes: usize,
    },
    SignalsChanged {
        port: PortId,
        signal: SignalKind,
        value: bool,
    },
    TaskFailed {
        id: TaskId,
        error: String,
    },
    TaskCancelled {
        id: TaskId,
    },
    ReplayChanged {
        rebuild: bool,
    },
    TerminalSettingsChanged {
        merge_window_ms: u64,
        max_entries: usize,
    },
    TerminalCleared,
    RecordingChanged,
    PluginsChanged,
    RecordingExportReady {
        id: TaskId,
        name: String,
        content: String,
        incomplete: bool,
    },
    MarketplaceFilesLoaded {
        id: TaskId,
        plugin_id: String,
        files: Vec<(String, String)>,
    },
    MarketplaceChanged,
    UpdateChanged,
}

#[derive(Debug, Deserialize)]
struct WebMarketplaceRegistry {
    #[serde(default = "default_marketplace_version")]
    version: u32,
    #[serde(default)]
    updated: String,
    #[serde(default)]
    plugins: Vec<WebMarketplacePlugin>,
}

#[derive(Debug, Deserialize)]
struct WebMarketplacePlugin {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    api_version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    published: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WebUpdateInfo {
    version: String,
    date: String,
    download_url: String,
    #[serde(default)]
    changelog: Vec<String>,
}

fn default_marketplace_version() -> u32 {
    1
}

fn parse_marketplace_registry(text: &str) -> Result<MarketplaceView, String> {
    let mut registry: WebMarketplaceRegistry =
        serde_json::from_str(text).map_err(|error| format!("解析插件市场失败：{error}"))?;
    registry.plugins.retain(|plugin| {
        !plugin.id.trim().is_empty()
            && !plugin.name.trim().is_empty()
            && !plugin.version.trim().is_empty()
    });
    registry.plugins.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(MarketplaceView {
        version: registry.version,
        updated: registry.updated,
        plugins: registry
            .plugins
            .into_iter()
        .map(|plugin| {
            let plugin_id = plugin.id.clone();
            MarketplacePluginView {
                id: plugin.id,
                name: plugin.name,
                version: plugin.version,
                api_version: plugin.api_version,
                description: plugin.description,
                author: plugin.author,
                homepage: plugin.homepage,
                repository: plugin.repository,
                license: plugin.license,
                category: plugin.category,
                icon: plugin.icon,
                permissions: plugin.permissions,
                size: plugin.size,
                published: plugin.published,
                // Browser and Native consume the same plugin package.  The
                // legacy registry fields are ignored here; point the Web
                // importer at the package's ordinary Lua sources instead of
                // a second browser-specific plugin bundle.
                manifest_url: Some(format!(
                    "https://raw.githubusercontent.com/Tydwdh/serial_tool/main/plugins/{}/plugin.json",
                    plugin_id
                )),
                main_url: Some(format!(
                    "https://raw.githubusercontent.com/Tydwdh/serial_tool/main/plugins/{}/main.lua",
                    plugin_id
                )),
            }})
            .collect(),
    })
}

#[derive(Debug, Clone, Copy)]
pub enum SignalKind {
    Dtr,
    Rts,
}

struct WebTaskRegistry {
    next_id: u64,
    snapshots: BTreeMap<TaskId, TaskSnapshot>,
    cancelled: BTreeSet<TaskId>,
    events: Vec<WebAppEvent>,
}

struct WebReplayLoadTask {
    id: TaskId,
    loader: ReplayTextLoader,
}

struct WebRecordingExport {
    id: TaskId,
    name: String,
    events: Vec<Event>,
    offset: usize,
    content: String,
    incomplete: bool,
}

struct WebRecordingService {
    subscription: Option<Subscription>,
    events: Vec<Event>,
    events_written: u64,
    recorded_bytes: u64,
    file_name: String,
    mode: tool_recorder::RecordMode,
    running: bool,
    paused: bool,
    pause_count: u64,
    incomplete: bool,
    stop_reason: Option<String>,
    last_error: Option<String>,
    stop_task: Option<TaskId>,
    export: Option<WebRecordingExport>,
    export_emitted: bool,
}

impl Default for WebRecordingService {
    fn default() -> Self {
        Self {
            subscription: None,
            events: Vec::new(),
            events_written: 0,
            recorded_bytes: 0,
            file_name: "hardware-workbench-session.jsonl".to_owned(),
            mode: tool_recorder::RecordMode::StandardReplay,
            running: false,
            paused: false,
            pause_count: 0,
            incomplete: false,
            stop_reason: None,
            last_error: None,
            stop_task: None,
            export: None,
            export_emitted: false,
        }
    }
}

impl WebRecordingService {
    const MAX_QUEUED_EVENTS: u64 = 100_000;
    const MAX_QUEUED_BYTES: u64 = 256 * 1024 * 1024;
    const MAX_SECONDS_BEHIND: f64 = 10.0;
    const MAX_RECORDED_EVENTS: u64 = 1_000_000;
    const MAX_RECORDED_BYTES: u64 = 256 * 1024 * 1024;

    fn event_size(event: &Event) -> u64 {
        let payload = match &event.payload {
            Payload::Empty => 0,
            Payload::Bytes(bytes) => bytes.len(),
            Payload::Text(text) => text.len(),
            Payload::Json(value) => value.to_string().len(),
        };
        (event.topic.len() + event.source.len() + event.metadata.to_string().len() + payload + 64)
            as u64
    }

    fn push_event(&mut self, event: Event) -> bool {
        let size = Self::event_size(&event);
        if self.events.len() as u64 >= Self::MAX_RECORDED_EVENTS
            || self.recorded_bytes.saturating_add(size) > Self::MAX_RECORDED_BYTES
        {
            return false;
        }
        self.recorded_bytes = self.recorded_bytes.saturating_add(size);
        self.events_written = self.events_written.saturating_add(1);
        self.events.push(event);
        true
    }

    fn queued_events(&self) -> u64 {
        self.subscription
            .as_ref()
            .map_or(0, Subscription::queued_len)
    }

    fn queued_bytes(&self) -> u64 {
        self.subscription
            .as_ref()
            .map_or(0, Subscription::queued_bytes)
    }

    fn seconds_behind(&self) -> f64 {
        self.subscription
            .as_ref()
            .map_or(0.0, |subscription| subscription.backlog().seconds_behind())
    }

    fn backlog_exceeded(&self) -> bool {
        self.queued_events() > Self::MAX_QUEUED_EVENTS
            || self.queued_bytes() > Self::MAX_QUEUED_BYTES
            || self.seconds_behind() > Self::MAX_SECONDS_BEHIND
    }

    fn status(&self) -> RecordingStatusView {
        RecordingStatusView {
            stats: RecorderStatsView {
                events_written: self.events_written,
                bytes_written: self.recorded_bytes,
                last_error: self.last_error.clone(),
                running: self.running,
                stopping: self.stop_task.is_some(),
                paused: self.paused,
                pause_count: self.pause_count,
                incomplete: self.incomplete,
                stop_reason: self.stop_reason.clone(),
                backlog_events: self.queued_events(),
                backlog_bytes: self.queued_bytes(),
                seconds_behind: self.seconds_behind(),
                ..RecorderStatsView::default()
            },
            path: Some(self.file_name.clone()),
            mode: self.mode.into(),
        }
    }
}

impl WebTaskRegistry {
    fn prune_finished(&mut self) {
        const MAX_RETAINED_SNAPSHOTS: usize = 256;
        while self.snapshots.len() > MAX_RETAINED_SNAPSHOTS {
            let Some(id) = self.snapshots.iter().find_map(|(id, snapshot)| {
                matches!(
                    snapshot.state,
                    TaskState::Completed | TaskState::Failed | TaskState::Cancelled
                )
                .then_some(*id)
            }) else {
                break;
            };
            self.snapshots.remove(&id);
        }
    }
}

impl Default for WebTaskRegistry {
    fn default() -> Self {
        Self {
            next_id: 1,
            snapshots: BTreeMap::new(),
            cancelled: BTreeSet::new(),
            events: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct WebApplication {
    bus: DataBus,
    transport: Option<WebSerialTransport>,
    network_transport: WebNetworkTransport,
    network_ports: Rc<RefCell<BTreeMap<String, NetworkSerialConfig>>>,
    tasks: Rc<RefCell<WebTaskRegistry>>,
    replay: Rc<RefCell<ReplayManager>>,
    replay_load: Rc<RefCell<Option<WebReplayLoadTask>>>,
    recording: Rc<RefCell<WebRecordingService>>,
    marketplace_view: Rc<RefCell<MarketplaceStatusView>>,
    update_view: Rc<RefCell<UpdateStatusView>>,
    plugin_view: Rc<RefCell<PluginView>>,
    plugin_commands: Rc<RefCell<Vec<PluginCommand>>>,
    plugin_events: Rc<RefCell<Option<RingSubscription>>>,
    repaint_waker: Rc<RefCell<Option<RepaintWaker>>>,
    serial_settings: Rc<RefCell<tool_platform::SerialSettings>>,
    terminal_merge_window_ms: Rc<RefCell<u64>>,
    terminal_max_entries: Rc<RefCell<usize>>,
    transport_view: Rc<RefCell<TransportView>>,
    reconnect_tasks: Rc<RefCell<BTreeMap<PortId, TaskId>>>,
}

impl WebApplication {
    pub fn new(bus: DataBus) -> Result<Self, String> {
        // Web Serial is optional. The rest of the browser composition (UI,
        // replay, plugins, marketplace and settings) must remain usable in a
        // browser that does not expose navigator.serial.
        let transport = WebSerialTransport::from_window().ok();
        let transport_capabilities = transport
            .as_ref()
            .map(|transport| transport.capabilities())
            .unwrap_or(TransportCapabilities {
                list_known_ports: false,
                request_port: false,
                connect: false,
                disconnect: false,
                send: false,
                set_dtr: false,
                set_rts: false,
            });
        let tasks = Rc::new(RefCell::new(WebTaskRegistry::default()));
        let replay = Rc::new(RefCell::new(ReplayManager::new(bus.clone())));
        let replay_load = Rc::new(RefCell::new(None));
        let recording = Rc::new(RefCell::new(WebRecordingService::default()));
        let marketplace_view = Rc::new(RefCell::new(MarketplaceStatusView::default()));
        let update_view = Rc::new(RefCell::new(UpdateStatusView::default()));
        let plugin_view = Rc::new(RefCell::new(PluginView::default()));
        let plugin_commands = Rc::new(RefCell::new(Vec::new()));
        let plugin_events = Rc::new(RefCell::new(None));
        let repaint_waker = Rc::new(RefCell::new(None));
        let terminal_merge_window_ms = Rc::new(RefCell::new(5));
        let terminal_max_entries = Rc::new(RefCell::new(50_000));
        let event_tasks = tasks.clone();
        let event_waker = repaint_waker.clone();
        if let Some(transport) = &transport {
            let _ = transport.watch_connection_events(Rc::new(move |event| {
                let app_event = match event {
                    WebSerialEvent::Connected(port) => WebAppEvent::PortAttached(port),
                    WebSerialEvent::Disconnected(port) => WebAppEvent::PortDetached(port),
                };
                event_tasks.borrow_mut().events.push(app_event);
                wake_handle(&event_waker);
            }));
        }
        Ok(Self {
            bus,
            transport,
            network_transport: WebNetworkTransport::new(),
            network_ports: Rc::new(RefCell::new(BTreeMap::new())),
            tasks,
            replay,
            replay_load,
            recording,
            marketplace_view,
            update_view,
            plugin_view,
            plugin_commands,
            plugin_events,
            repaint_waker,
            serial_settings: Rc::new(RefCell::new(tool_platform::SerialSettings::default())),
            terminal_merge_window_ms,
            terminal_max_entries,
            transport_view: Rc::new(RefCell::new(TransportView::new(transport_capabilities))),
            reconnect_tasks: Rc::new(RefCell::new(BTreeMap::new())),
        })
    }

    pub fn set_repaint_waker(&self, waker: RepaintWaker) {
        *self.repaint_waker.borrow_mut() = Some(waker);
    }

    fn wake(&self) {
        wake_handle(&self.repaint_waker);
    }

    /// Publish an application event without exposing the concrete DataBus to
    /// the browser composition root.
    pub fn publish_event(&self, event: Event) {
        self.bus.publish(event);
    }

    /// Snapshot the bounded DataBus history for the shared Lua host API.
    /// This is intentionally a query, not a DataBus handle leak.
    pub fn plugin_bus_history(&self, topic: &str, limit: usize) -> Vec<Event> {
        self.bus
            .history()
            .into_iter()
            .filter(|event| topic.is_empty() || event.topic.starts_with(topic))
            .rev()
            .take(limit.min(100))
            .collect()
    }

    pub fn perf_snapshot(&self) -> tool_databus::DataBusPerfSnapshot {
        self.bus.perf_snapshot()
    }

    /// Read-only transport state for the shared Native/Web presentation
    /// layer. Widget-only state (selection, aliases and text input) remains
    /// in the composition root; device lifecycle state does not.
    pub fn query_transport(&self) -> TransportView {
        self.transport_view.borrow().clone()
    }

    pub fn query_terminal_settings(&self) -> (u64, usize) {
        (
            *self.terminal_merge_window_ms.borrow(),
            *self.terminal_max_entries.borrow(),
        )
    }

    fn set_transport_status(&self, status: impl Into<String>) {
        self.transport_view.borrow_mut().status = status.into();
        self.wake();
    }

    fn apply_transport_event(&self, event: &WebAppEvent) {
        let mut view = self.transport_view.borrow_mut();
        match event {
            WebAppEvent::TaskStateChanged(snapshot)
                if is_transport_task_kind(&snapshot.kind)
                    && matches!(
                        snapshot.state,
                        crate::task_model::TaskState::Failed
                            | crate::task_model::TaskState::Cancelled
                    ) =>
            {
                self.reconnect_tasks
                    .borrow_mut()
                    .retain(|_, task_id| *task_id != snapshot.id);
                if matches!(
                    snapshot.kind.as_str(),
                    "connect_serial"
                        | "reconnect_serial"
                        | "connect_network"
                        | "reconnect_network"
                        | "cancel_reconnect_disconnect"
                ) {
                    view.connecting = false;
                }
                view.status = snapshot.message.clone();
            }
            WebAppEvent::PortsRefreshed(ports) => {
                view.ports = ports.clone();
                view.status = format!("已授权设备 {} 个", ports.len());
            }
            WebAppEvent::PortRequested { port, .. }
            | WebAppEvent::PortAttached(port)
            | WebAppEvent::NetworkPortAdded(port) => {
                view.upsert_port(port.clone());
                view.status = if matches!(event, WebAppEvent::PortRequested { .. }) {
                    "设备已授权，可连接".to_owned()
                } else if matches!(event, WebAppEvent::NetworkPortAdded(_)) {
                    "网络串口已添加，可连接".to_owned()
                } else {
                    "检测到已授权设备".to_owned()
                };
            }
            WebAppEvent::PortDetached(port) | WebAppEvent::NetworkPortRemoved(port) => {
                view.remove_port(port);
                view.status = if matches!(event, WebAppEvent::NetworkPortRemoved(_)) {
                    "网络串口已移除".to_owned()
                } else {
                    "设备已拔出".to_owned()
                };
            }
            WebAppEvent::Connected { port } => {
                self.reconnect_tasks.borrow_mut().remove(port);
                view.set_connected(Some(port.clone()));
                view.status = format!("已连接 {port}");
            }
            WebAppEvent::Disconnected { port } => {
                self.reconnect_tasks.borrow_mut().remove(port);
                view.set_connected(None);
                view.status = "设备已断开".to_owned();
            }
            WebAppEvent::Sent { bytes, .. } => {
                view.status = format!("发送成功（{bytes} 字节）");
            }
            WebAppEvent::SignalsChanged { signal, value, .. } => {
                view.status = format!("{signal:?} {value}");
            }
            // Non-transport task failures are intentionally ignored here so
            // replay/plugin/file errors do not overwrite a live serial
            // connection indicator in the shared top bar.
            _ => {}
        }
    }

    /// Drain the bounded event stream used by browser Lua plugins. The
    /// subscription is owned by Application so UI code cannot create or
    /// replace arbitrary DataBus subscribers.
    pub fn drain_plugin_events(&self, max: usize) -> Vec<Event> {
        let mut subscription = self.plugin_events.borrow_mut();
        if subscription.is_none() {
            *subscription = Some(self.bus.subscribe_ring_bounded(TopicFilter::All, 4_096));
        }
        subscription
            .as_ref()
            .map(|subscription| subscription.drain_limited(max))
            .unwrap_or_default()
    }

    pub fn clear_plugin_events(&self) {
        if let Some(subscription) = self.plugin_events.borrow().as_ref() {
            subscription.clear();
        }
    }

    /// Read-only replay view shared with the Native Workbench query surface.
    pub fn query_replay(&self) -> ReplayStatusView {
        let replay = self.replay.borrow();
        let status = replay.status();
        ReplayStatusView {
            state: match status.state {
                tool_recorder::ReplayState::Empty => ReplayStateView::Empty,
                tool_recorder::ReplayState::Loaded => ReplayStateView::Loaded,
                tool_recorder::ReplayState::Playing => ReplayStateView::Playing,
                tool_recorder::ReplayState::Paused => ReplayStateView::Paused,
                tool_recorder::ReplayState::Finished => ReplayStateView::Finished,
            },
            path: status.path.map(|path| path.display().to_string()),
            total_events: status.total_events,
            cursor: status.cursor,
            speed: status.speed,
            position_ms: status.position_ms,
            duration_ms: status.duration_ms,
            policy: replay_policy_view(status.policy),
            effective_policy: replay_policy_view(status.effective_policy),
            has_recorded_protocol: status.has_recorded_protocol,
            analyzer_cache_entries: status.analyzer_cache_entries,
            analyzer_cache_valid: replay.analyzer_cache_valid(),
            analyzer_error: status.analyzer_error,
            analyzer_warning: status.analyzer_warning,
            can_play: replay.can_play(),
            can_seek: replay.can_seek(),
            block_reason: replay.replay_block_reason().map(|reason| match reason {
                tool_recorder::ReplayBlockReason::NeedAnalyzer => {
                    ReplayBlockReasonView::NeedAnalyzer
                }
                tool_recorder::ReplayBlockReason::AnalyzerFailed(error) => {
                    ReplayBlockReasonView::AnalyzerFailed(error)
                }
            }),
            bookmarks: replay
                .bookmarks()
                .iter()
                .map(|bookmark| ReplayBookmarkView {
                    position_ms: bookmark.pos_ms,
                    name: bookmark.name.clone(),
                })
                .collect(),
            load_report: status.load_report.map(|report| ReplayLoadReportView {
                loaded: report.loaded,
                skipped: report.skipped,
                first_errors: report.first_errors,
            }),
        }
    }

    pub fn replay_raw_serial_events(&self) -> Vec<tool_core::Event> {
        self.replay.borrow().raw_serial_events()
    }

    pub fn replay_set_analyzer_cache(&self, events: Vec<tool_core::Event>) {
        self.replay.borrow_mut().set_analyzer_cache(events);
        self.wake();
    }

    pub fn replay_set_analyzer_error(&self, error: String) {
        self.replay.borrow_mut().set_analyzer_error(error);
        self.wake();
    }

    pub fn replay_set_analyzer_warning(&self, warning: String) {
        self.replay.borrow_mut().set_analyzer_warning(warning);
        self.wake();
    }

    pub fn replay_clear_analyzer_messages(&self) {
        self.replay.borrow_mut().clear_analyzer_error();
        self.wake();
    }

    pub fn query_recording(&self) -> RecordingStatusView {
        self.recording.borrow().status()
    }

    pub fn query_marketplace(&self) -> MarketplaceStatusView {
        self.marketplace_view.borrow().clone()
    }

    pub fn finish_marketplace_install(&self, plugin_id: &str, result: Result<(), String>) {
        let mut status = self.marketplace_view.borrow_mut();
        status.installing.retain(|id| id != plugin_id);
        if let Err(error) = result {
            status.error = Some(error);
        }
        drop(status);
        self.wake();
    }

    pub fn query_update(&self) -> UpdateStatusView {
        self.update_view.borrow().clone()
    }

    fn start_marketplace_refresh(&self, url: String) -> Result<CommandOutcome, String> {
        let url = url.trim().to_owned();
        if url.is_empty() {
            return Err("插件市场地址不能为空".to_owned());
        }
        if !url.starts_with("https://") && !url.starts_with("http://") {
            return Err("插件市场地址必须使用 http:// 或 https://".to_owned());
        }
        {
            let mut view = self.marketplace_view.borrow_mut();
            if view.refreshing {
                return Ok(CommandOutcome::Done);
            }
            view.refreshing = true;
            view.error = None;
        }
        self.wake();
        let view = self.marketplace_view.clone();
        let future = async move {
            let result = tool_platform::web_fetch::fetch_text(&url)
                .await
                .and_then(|text| parse_marketplace_registry(&text));
            Ok::<Result<MarketplaceView, String>, String>(result)
        };
        let task_id = self.spawn_result("marketplace_refresh", future, move |_, result| {
            let mut status = view.borrow_mut();
            status.refreshing = false;
            match result {
                Ok(registry) => {
                    status.registry = Some(registry);
                    status.error = None;
                }
                Err(error) => status.error = Some(error),
            }
            WebAppEvent::MarketplaceChanged
        });
        Ok(CommandOutcome::Pending {
            task_id,
            message: "正在刷新插件市场".to_owned(),
        })
    }

    fn start_marketplace_install(
        &self,
        plugin_id: String,
        manifest_url: String,
        main_url: String,
    ) -> Result<CommandOutcome, String> {
        for (label, url) in [("plugin.json", &manifest_url), ("main.lua", &main_url)] {
            if !url.starts_with("https://") && !url.starts_with("http://") {
                return Err(format!(
                    "插件 {} 的 {label} 地址必须使用 http:// 或 https://",
                    plugin_id
                ));
            }
        }
        {
            let mut status = self.marketplace_view.borrow_mut();
            if status.installing.iter().any(|id| id == &plugin_id) {
                return Ok(CommandOutcome::Done);
            }
            status.installing.push(plugin_id.clone());
            status.error = None;
        }
        self.wake();
        let view = self.marketplace_view.clone();
        let task_plugin_id = plugin_id.clone();
        let future = async move {
            let result = async {
                let manifest = tool_platform::web_fetch::fetch_text(&manifest_url).await?;
                let source = tool_platform::web_fetch::fetch_text(&main_url).await?;
                let mut files = vec![
                    ("plugin.json".to_owned(), manifest),
                    ("main.lua".to_owned(), source),
                ];
                if let Ok(manifest_json) = serde_json::from_str::<serde_json::Value>(&files[0].1)
                    && let Some(replay_main) = manifest_json
                        .get("replay")
                        .and_then(|replay| replay.get("main"))
                        .and_then(serde_json::Value::as_str)
                {
                    let replay_leaf = replay_main
                        .rsplit_once('/')
                        .map(|(_, leaf)| leaf)
                        .unwrap_or(replay_main);
                    let replay_url = main_url
                        .rsplit_once('/')
                        .map(|(base, _)| format!("{base}/{replay_leaf}"))
                        .unwrap_or_else(|| replay_leaf.to_owned());
                    let replay_source = tool_platform::web_fetch::fetch_text(&replay_url).await?;
                    files.push((replay_leaf.to_owned(), replay_source));
                }
                Ok::<_, String>(files)
            }
            .await;
            Ok::<Result<Vec<(String, String)>, String>, String>(result)
        };
        let task_id = self.spawn_result("marketplace_install", future, move |id, result| {
            let mut status = view.borrow_mut();
            match result {
                Ok(files) => WebAppEvent::MarketplaceFilesLoaded {
                    id,
                    plugin_id: task_plugin_id.clone(),
                    files,
                },
                Err(error) => {
                    status.installing.retain(|id| id != &task_plugin_id);
                    status.error = Some(format!("安装 Lua 插件失败：{error}"));
                    WebAppEvent::MarketplaceChanged
                }
            }
        });
        Ok(CommandOutcome::Pending {
            task_id,
            message: format!("正在安装 Lua 插件：{plugin_id}"),
        })
    }

    fn start_update_check(&self) -> Result<CommandOutcome, String> {
        {
            let mut status = self.update_view.borrow_mut();
            if status.checking {
                return Ok(CommandOutcome::Done);
            }
            status.checking = true;
            status.error = None;
        }
        self.wake();
        let view = self.update_view.clone();
        let future = async {
            let result = tool_platform::web_fetch::fetch_text(WEB_UPDATE_INFO_URL)
                .await
                .and_then(|text| {
                    serde_json::from_str::<WebUpdateInfo>(&text)
                        .map_err(|error| format!("解析更新信息失败：{error}"))
                });
            Ok::<Result<WebUpdateInfo, String>, String>(result)
        };
        let task_id = self.spawn_result("update_check", future, move |_, result| {
            let mut status = view.borrow_mut();
            status.checking = false;
            match result {
                Ok(info) => {
                    status.info = Some(crate::updater::UpdateInfoView {
                        version: info.version,
                        date: info.date,
                        download_url: info.download_url,
                        changelog: info.changelog,
                    });
                    status.error = None;
                }
                Err(error) => status.error = Some(error),
            }
            WebAppEvent::UpdateChanged
        });
        Ok(CommandOutcome::Pending {
            task_id,
            message: "正在检查 Web 更新".to_owned(),
        })
    }

    /// Read-only plugin projection for the shared presentation layer.
    pub fn query_plugins(&self) -> PluginView {
        self.plugin_view.borrow().clone()
    }

    /// Update the projection after the browser Lua capability has observed
    /// a manifest, load, unload or failure transition.
    pub fn set_plugins_view(&self, view: PluginView) {
        if *self.plugin_view.borrow() == view {
            return;
        }
        *self.plugin_view.borrow_mut() = view;
        self.tasks
            .borrow_mut()
            .events
            .push(WebAppEvent::PluginsChanged);
        self.wake();
    }

    /// Transfer plugin intents from the Application boundary to the Web
    /// capability. VM and host handles never cross this boundary.
    pub fn take_plugin_commands(&self) -> Vec<PluginCommand> {
        std::mem::take(&mut *self.plugin_commands.borrow_mut())
    }

    pub fn finish_recording_export(&self, id: TaskId, result: Result<(), String>) {
        let mut recording = self.recording.borrow_mut();
        if recording.stop_task != Some(id) {
            return;
        }
        recording.stop_task = None;
        recording.export = None;
        recording.export_emitted = false;
        recording.subscription = None;
        if let Err(error) = &result {
            recording.last_error = Some(error.clone());
        }
        drop(recording);
        match result {
            Ok(()) => {
                self.complete_task(
                    id,
                    if self.recording.borrow().incomplete {
                        "录制已导出（数据不完整）"
                    } else {
                        "录制已导出"
                    },
                );
            }
            Err(error) => {
                self.fail_task(id, error);
            }
        }
        self.tasks
            .borrow_mut()
            .events
            .push(WebAppEvent::RecordingChanged);
        self.wake();
    }

    pub fn serial_supported(&self) -> bool {
        self.transport.is_some()
    }

    fn serial_transport(&self) -> Result<WebSerialTransport, String> {
        self.transport
            .clone()
            .ok_or_else(|| "当前浏览器不支持 Web Serial".to_owned())
    }

    pub fn task_snapshots(&self) -> Vec<TaskSnapshot> {
        self.tasks.borrow().snapshots.values().cloned().collect()
    }

    pub fn task_is_active(&self, id: TaskId) -> bool {
        self.tasks
            .borrow()
            .snapshots
            .get(&id)
            .is_some_and(|snapshot| {
                matches!(snapshot.state, TaskState::Pending | TaskState::Running)
            })
    }

    /// Start a cooperative UI task whose work is performed over successive
    /// repaint ticks.  Browser APIs such as Blob generation and rendering a
    /// panel-owned export cannot be moved to the transport Promise executor,
    /// but they still must have the same observable lifecycle as every other
    /// application operation.
    pub fn begin_task(&self, kind: impl Into<String>, message: impl Into<String>) -> TaskId {
        let kind = kind.into();
        let message = message.into();
        let mut tasks = self.tasks.borrow_mut();
        let id = TaskId(tasks.next_id);
        tasks.next_id += 1;
        let pending = TaskSnapshot {
            id,
            kind,
            state: TaskState::Pending,
            message: message.clone(),
        };
        tasks.snapshots.insert(id, pending.clone());
        tasks.events.push(WebAppEvent::TaskStateChanged(pending));
        let running = TaskSnapshot {
            id,
            kind: tasks
                .snapshots
                .get(&id)
                .map(|snapshot| snapshot.kind.clone())
                .unwrap_or_default(),
            state: TaskState::Running,
            message,
        };
        tasks.snapshots.insert(id, running.clone());
        tasks.events.push(WebAppEvent::TaskStateChanged(running));
        drop(tasks);
        self.wake();
        id
    }

    /// Update progress for a cooperative task without exposing the task
    /// registry to the UI composition root.
    pub fn update_task(&self, id: TaskId, message: impl Into<String>) -> bool {
        let mut tasks = self.tasks.borrow_mut();
        let Some(snapshot) = tasks.snapshots.get_mut(&id) else {
            return false;
        };
        if !matches!(snapshot.state, TaskState::Pending | TaskState::Running) {
            return false;
        }
        snapshot.message = message.into();
        let snapshot = snapshot.clone();
        tasks.events.push(WebAppEvent::TaskStateChanged(snapshot));
        drop(tasks);
        self.wake();
        true
    }

    pub fn complete_task(&self, id: TaskId, message: impl Into<String>) -> bool {
        self.finish_cooperative_task(id, TaskState::Completed, message.into())
    }

    pub fn fail_task(&self, id: TaskId, error: impl Into<String>) -> bool {
        self.finish_cooperative_task(id, TaskState::Failed, error.into())
    }

    fn finish_cooperative_task(&self, id: TaskId, state: TaskState, message: String) -> bool {
        let mut tasks = self.tasks.borrow_mut();
        let Some(snapshot) = tasks.snapshots.get_mut(&id) else {
            return false;
        };
        if !matches!(snapshot.state, TaskState::Pending | TaskState::Running) {
            return false;
        }
        snapshot.state = state;
        snapshot.message = message;
        let snapshot = snapshot.clone();
        tasks.events.push(WebAppEvent::TaskStateChanged(snapshot));
        tasks.prune_finished();
        drop(tasks);
        self.wake();
        true
    }

    pub fn drain_events(&self) -> Vec<WebAppEvent> {
        let events = std::mem::take(&mut self.tasks.borrow_mut().events);
        for event in &events {
            self.apply_transport_event(event);
        }
        events
    }

    pub fn cancel_task(&self, id: TaskId) -> bool {
        let mut tasks = self.tasks.borrow_mut();
        let active = tasks.snapshots.get(&id).is_some_and(|snapshot| {
            matches!(snapshot.state, TaskState::Pending | TaskState::Running)
        });
        if !active {
            return false;
        }
        tasks.cancelled.insert(id);
        tasks.events.push(WebAppEvent::TaskCancelled { id });
        let cancelled_snapshot = tasks.snapshots.get_mut(&id).map(|snapshot| {
            snapshot.state = TaskState::Cancelled;
            snapshot.message = "任务已取消".to_owned();
            snapshot.clone()
        });
        if let Some(snapshot) = cancelled_snapshot {
            tasks.events.push(WebAppEvent::TaskStateChanged(snapshot));
        }
        tasks.prune_finished();
        drop(tasks);
        self.wake();
        true
    }

    fn spawn<T, F>(
        &self,
        kind: &str,
        future: F,
        complete: impl FnOnce(TaskId, T) -> WebAppEvent + 'static,
    ) -> TaskId
    where
        T: 'static,
        F: Future<Output = tool_platform::TransportResult<T>> + 'static,
    {
        self.spawn_result(kind, future, complete)
    }

    fn spawn_result<T, F, E>(
        &self,
        kind: &str,
        future: F,
        complete: impl FnOnce(TaskId, T) -> WebAppEvent + 'static,
    ) -> TaskId
    where
        T: 'static,
        E: ToString + 'static,
        F: Future<Output = Result<T, E>> + 'static,
    {
        let (id, tasks) = {
            let mut tasks = self.tasks.borrow_mut();
            let id = TaskId(tasks.next_id);
            tasks.next_id += 1;
            let snapshot = TaskSnapshot {
                id,
                kind: kind.to_owned(),
                state: TaskState::Pending,
                message: "等待异步操作启动".to_owned(),
            };
            tasks.snapshots.insert(id, snapshot.clone());
            tasks.events.push(WebAppEvent::TaskStateChanged(snapshot));
            (id, self.tasks.clone())
        };
        self.wake();

        {
            let mut tasks = tasks.borrow_mut();
            if let Some(snapshot) = tasks.snapshots.get_mut(&id) {
                snapshot.state = TaskState::Running;
                snapshot.message = "异步操作运行中".to_owned();
                let snapshot = snapshot.clone();
                tasks.events.push(WebAppEvent::TaskStateChanged(snapshot));
            }
        }
        self.wake();

        let repaint_waker = self.repaint_waker.clone();
        spawn_local(async move {
            let result = future.await;
            let should_wake = {
                let mut tasks = tasks.borrow_mut();
                if tasks.cancelled.remove(&id) {
                    false
                } else {
                    match result {
                        Ok(value) => {
                            let snapshot = if let Some(snapshot) = tasks.snapshots.get_mut(&id) {
                                snapshot.state = TaskState::Completed;
                                snapshot.message = "异步操作完成".to_owned();
                                Some(snapshot.clone())
                            } else {
                                None
                            };
                            if let Some(snapshot) = snapshot {
                                tasks.events.push(WebAppEvent::TaskStateChanged(snapshot));
                            }
                            tasks.events.push(complete(id, value));
                        }
                        Err(error) => {
                            let snapshot = if let Some(snapshot) = tasks.snapshots.get_mut(&id) {
                                snapshot.state = TaskState::Failed;
                                snapshot.message = error.to_string();
                                Some(snapshot.clone())
                            } else {
                                None
                            };
                            if let Some(snapshot) = snapshot {
                                tasks.events.push(WebAppEvent::TaskStateChanged(snapshot));
                            }
                            tasks.events.push(WebAppEvent::TaskFailed {
                                id,
                                error: error.to_string(),
                            });
                        }
                    }
                    tasks.prune_finished();
                    true
                }
            };
            if should_wake {
                wake_handle(&repaint_waker);
            }
        });
        id
    }

    /// Schedule browser file text loading in the same lifecycle registry as
    /// transport tasks. The picker itself must still be opened by the UI in a
    /// user gesture; only the asynchronous read and completion are owned here.
    pub fn load_text<F>(
        &self,
        kind: impl Into<String>,
        name: impl Into<String>,
        future: F,
    ) -> CommandOutcome
    where
        F: Future<Output = Result<String, String>> + 'static,
    {
        let kind = kind.into();
        let name = name.into();
        let task_kind = kind.clone();
        let task_name = name.clone();
        let task_id = self.spawn_result(task_kind.as_str(), future, move |id, text| {
            WebAppEvent::TextLoaded {
                id,
                kind: kind.clone(),
                name: name.clone(),
                text,
            }
        });
        CommandOutcome::Pending {
            task_id,
            message: format!("正在读取 {task_name}"),
        }
    }

    /// Schedule a multi-file browser read, preserving one task lifecycle for
    /// plugin imports and other user-selected file batches.
    pub fn load_files<F>(&self, kind: impl Into<String>, future: F) -> CommandOutcome
    where
        F: Future<Output = Result<Vec<(String, String)>, String>> + 'static,
    {
        let kind = kind.into();
        let task_kind = kind.clone();
        let task_id = self.spawn_result(task_kind.as_str(), future, move |id, files| {
            WebAppEvent::FilesLoaded {
                id,
                kind: kind.clone(),
                files,
            }
        });
        CommandOutcome::Pending {
            task_id,
            message: "正在读取所选文件".to_owned(),
        }
    }

    fn tick_replay_load(&self) {
        let mut completed = None;
        let mut failed = None;
        let mut progress = None;
        {
            let mut load = self.replay_load.borrow_mut();
            let Some(task) = load.as_mut() else {
                return;
            };
            let active = self
                .tasks
                .borrow()
                .snapshots
                .get(&task.id)
                .is_some_and(|snapshot| {
                    matches!(snapshot.state, TaskState::Pending | TaskState::Running)
                });
            if !active {
                load.take();
                return;
            }
            match task.loader.step(2_048) {
                Ok(Some(prepared)) => completed = Some((task.id, prepared)),
                Ok(None) => progress = Some(task.id),
                Err(error) => failed = Some((task.id, error.to_string())),
            }
        }

        if let Some((id, prepared)) = completed {
            self.replay.borrow_mut().load_prepared(prepared);
            self.replay_load.borrow_mut().take();
            self.complete_task(id, "回放文件加载完成");
            self.tasks
                .borrow_mut()
                .events
                .push(WebAppEvent::ReplayChanged { rebuild: true });
        } else if let Some((id, error)) = failed {
            self.replay_load.borrow_mut().take();
            self.fail_task(id, error);
        } else if let Some(id) = progress {
            self.update_task(id, "正在分帧解析回放文件");
        }
    }

    fn start_recording(
        &self,
        name: String,
        mode: RecordModeView,
    ) -> Result<CommandOutcome, String> {
        let mut recording = self.recording.borrow_mut();
        if recording.running || recording.stop_task.is_some() {
            return Err("录制或导出任务仍在进行".to_owned());
        }
        recording.file_name = if name.trim().is_empty() {
            "hardware-workbench-session.jsonl".to_owned()
        } else {
            name.trim().to_owned()
        };
        recording.mode = mode.into();
        recording.events.clear();
        recording.events_written = 0;
        recording.recorded_bytes = 0;
        recording.running = true;
        recording.paused = false;
        recording.pause_count = 0;
        recording.incomplete = false;
        recording.stop_reason = None;
        recording.last_error = None;
        recording.export = None;
        recording.export_emitted = false;
        recording.subscription = Some(self.bus.subscribe_lossless(TopicFilter::All));
        drop(recording);
        self.tasks
            .borrow_mut()
            .events
            .push(WebAppEvent::RecordingChanged);
        self.wake();
        Ok(CommandOutcome::Done)
    }

    fn request_recording_stop(
        &self,
        incomplete: bool,
        reason: Option<String>,
    ) -> Result<CommandOutcome, String> {
        let task_id = self.begin_task("export_recording", "正在分帧整理录制");
        let mut recording = self.recording.borrow_mut();
        if !recording.running && recording.subscription.is_none() {
            drop(recording);
            self.cancel_task(task_id);
            return Ok(CommandOutcome::Done);
        }
        recording.running = false;
        recording.paused = false;
        recording.incomplete |= incomplete;
        if reason.is_some() {
            recording.stop_reason = reason;
        }
        if recording.incomplete && recording.stop_reason.is_none() {
            recording.stop_reason = Some("录制不完整".to_owned());
        }
        recording.stop_task = Some(task_id);
        drop(recording);
        self.tasks
            .borrow_mut()
            .events
            .push(WebAppEvent::RecordingChanged);
        self.wake();
        Ok(CommandOutcome::Pending {
            task_id,
            message: "正在分帧整理录制".to_owned(),
        })
    }

    fn tick_recording(&self) {
        let mut export_ready = None;
        let mut progress_id = None;
        let mut auto_stop = None;
        {
            let mut recording = self.recording.borrow_mut();
            if recording.stop_task.is_none() && recording.running {
                if recording.backlog_exceeded() {
                    auto_stop = Some(format!(
                        "录制积压超过硬阈值（{} 个事件，{} 字节，落后 {:.1}s）",
                        recording.queued_events(),
                        recording.queued_bytes(),
                        recording.seconds_behind()
                    ));
                } else {
                    let paused = recording.paused;
                    let mode = recording.mode;
                    let mut drained = 0usize;
                    while drained < 2_000 {
                        let Some(event) = recording
                            .subscription
                            .as_ref()
                            .and_then(Subscription::try_recv)
                        else {
                            break;
                        };
                        drained += 1;
                        if !paused
                            && should_record_event(&event, mode)
                            && !recording.push_event(event)
                        {
                            auto_stop = Some(format!(
                                "浏览器录制达到硬上限（{} 个事件，{} 字节）",
                                WebRecordingService::MAX_RECORDED_EVENTS,
                                WebRecordingService::MAX_RECORDED_BYTES
                            ));
                            break;
                        }
                    }
                    if drained > 0 {
                        progress_id = recording.stop_task;
                    }
                    if recording.backlog_exceeded() {
                        auto_stop = Some(format!(
                            "录制消费速度不足（{} 个事件，{} 字节，落后 {:.1}s）",
                            recording.queued_events(),
                            recording.queued_bytes(),
                            recording.seconds_behind()
                        ));
                    }
                }
            }

            if let Some(id) = recording.stop_task {
                let active = self
                    .tasks
                    .borrow()
                    .snapshots
                    .get(&id)
                    .is_some_and(|snapshot| {
                        matches!(snapshot.state, TaskState::Pending | TaskState::Running)
                    });
                if !active {
                    recording.subscription = None;
                    recording.stop_task = None;
                    recording.export = None;
                    recording.export_emitted = false;
                    recording.events.clear();
                    recording.events_written = 0;
                    recording.recorded_bytes = 0;
                } else {
                    let mut drained = 0usize;
                    while drained < 2_000 {
                        let Some(event) = recording
                            .subscription
                            .as_ref()
                            .and_then(Subscription::try_recv)
                        else {
                            break;
                        };
                        drained += 1;
                        if should_record_event(&event, recording.mode)
                            && !recording.push_event(event)
                        {
                            recording.incomplete = true;
                            recording.stop_reason = Some(format!(
                                "浏览器录制达到硬上限（{} 个事件，{} 字节）",
                                WebRecordingService::MAX_RECORDED_EVENTS,
                                WebRecordingService::MAX_RECORDED_BYTES
                            ));
                            break;
                        }
                    }
                    if recording
                        .subscription
                        .as_ref()
                        .is_none_or(|subscription| subscription.queued_len() == 0)
                    {
                        if recording.export.is_none() && !recording.export_emitted {
                            recording.export = Some(WebRecordingExport {
                                id,
                                name: recording.file_name.clone(),
                                events: std::mem::take(&mut recording.events),
                                offset: 0,
                                content: String::new(),
                                incomplete: recording.incomplete,
                            });
                        }
                        let mut serialization_error = None;
                        if let Some(export) = recording.export.as_mut() {
                            let end = (export.offset + 512).min(export.events.len());
                            for event in &export.events[export.offset..end] {
                                match serde_json::to_string(event) {
                                    Ok(line) => {
                                        export.content.push_str(&line);
                                        export.content.push('\n');
                                    }
                                    Err(error) => {
                                        serialization_error = Some(error.to_string());
                                    }
                                }
                            }
                            export.offset = end;
                            if end >= export.events.len() {
                                let export = recording.export.take().expect("export exists");
                                recording.export_emitted = true;
                                export_ready = Some((
                                    export.id,
                                    export.name,
                                    export.content,
                                    export.incomplete,
                                ));
                            } else {
                                progress_id = Some(id);
                            }
                        }
                        if let Some(error) = serialization_error {
                            recording.last_error = Some(error);
                        }
                    } else {
                        progress_id = Some(id);
                    }
                }
            }
        }

        if let Some(reason) = auto_stop {
            let _ = self.request_recording_stop(true, Some(reason));
        }
        if let Some(id) = progress_id {
            self.update_task(id, "正在分帧导出录制");
        }
        if let Some((id, name, content, incomplete)) = export_ready {
            self.tasks
                .borrow_mut()
                .events
                .push(WebAppEvent::RecordingExportReady {
                    id,
                    name,
                    content,
                    incomplete,
                });
            self.wake();
        }
    }

    pub fn dispatch(&self, command: AppCommand) -> Result<CommandOutcome, String> {
        match command {
            AppCommand::StartRecording { file, mode } => {
                self.start_recording(file.name().to_owned(), mode)
            }
            AppCommand::SetSerialSettings { settings } => {
                *self.serial_settings.borrow_mut() = settings;
                self.transport_view.borrow_mut().settings = settings;
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::SetRecordingMode { mode } => {
                self.recording.borrow_mut().mode = mode.into();
                self.tasks
                    .borrow_mut()
                    .events
                    .push(WebAppEvent::RecordingChanged);
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::EnablePlugin { plugin_id } => {
                self.plugin_commands
                    .borrow_mut()
                    .push(PluginCommand::Enable { plugin_id });
                Ok(CommandOutcome::Done)
            }
            AppCommand::DisablePlugin { plugin_id } => {
                self.plugin_commands
                    .borrow_mut()
                    .push(PluginCommand::Disable { plugin_id });
                Ok(CommandOutcome::Done)
            }
            AppCommand::ReloadPlugins => {
                self.plugin_commands
                    .borrow_mut()
                    .push(PluginCommand::Reload);
                Ok(CommandOutcome::Done)
            }
            AppCommand::RefreshMarketplace { url } => self.start_marketplace_refresh(url),
            #[cfg(target_arch = "wasm32")]
            AppCommand::InstallMarketplacePlugin {
                plugin_id,
                manifest_url,
                main_url,
            } => self.start_marketplace_install(plugin_id, manifest_url, main_url),
            AppCommand::CheckForUpdate => self.start_update_check(),
            AppCommand::ExecutePluginCommand {
                plugin_id,
                command_id,
                context,
            } => {
                self.plugin_commands
                    .borrow_mut()
                    .push(PluginCommand::Execute {
                        plugin_id,
                        command_id,
                        context,
                    });
                Ok(CommandOutcome::Done)
            }
            AppCommand::StopRecording => self.request_recording_stop(false, None),
            AppCommand::PauseRecording => {
                let mut recording = self.recording.borrow_mut();
                if recording.running && !recording.paused {
                    recording.paused = true;
                    recording.pause_count = recording.pause_count.saturating_add(1);
                    self.tasks
                        .borrow_mut()
                        .events
                        .push(WebAppEvent::RecordingChanged);
                }
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::ResumeRecording => {
                let mut recording = self.recording.borrow_mut();
                recording.paused = false;
                self.tasks
                    .borrow_mut()
                    .events
                    .push(WebAppEvent::RecordingChanged);
                drop(recording);
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::AddBookmark { name } => {
                self.bus.publish(Event::new(
                    "recorder.bookmark",
                    "recorder",
                    Direction::Internal,
                    Payload::Text(name.unwrap_or_default()),
                ));
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::LoadReplayText { name, text } => {
                if self.replay_load.borrow().is_some() {
                    return Err("已有回放文件正在加载".to_owned());
                }
                let loader =
                    ReplayTextLoader::new(name, text).map_err(|error| error.to_string())?;
                let task_id = self.begin_task("load_replay", "正在解析回放文件");
                *self.replay_load.borrow_mut() = Some(WebReplayLoadTask {
                    id: task_id,
                    loader,
                });
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "正在分帧解析回放文件".to_owned(),
                })
            }
            AppCommand::ReplayPlay => {
                self.replay.borrow_mut().play();
                self.tasks
                    .borrow_mut()
                    .events
                    .push(WebAppEvent::ReplayChanged { rebuild: false });
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::ReplayPause => {
                self.replay.borrow_mut().pause();
                self.tasks
                    .borrow_mut()
                    .events
                    .push(WebAppEvent::ReplayChanged { rebuild: false });
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::ReplayStop => {
                self.replay.borrow_mut().stop();
                self.tasks
                    .borrow_mut()
                    .events
                    .push(WebAppEvent::ReplayChanged { rebuild: true });
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::ClearTerminal => {
                self.tasks
                    .borrow_mut()
                    .events
                    .push(WebAppEvent::TerminalCleared);
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::ReplaySeek { position_ms } => {
                self.replay.borrow_mut().seek_with_replay(position_ms);
                self.tasks
                    .borrow_mut()
                    .events
                    .push(WebAppEvent::ReplayChanged { rebuild: true });
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::ReplaySeekBy { delta_ms } => {
                let current = self.replay.borrow().status().position_ms;
                let position_ms = if delta_ms < 0 {
                    current.saturating_sub(delta_ms.unsigned_abs())
                } else {
                    current.saturating_add(delta_ms as u64)
                };
                self.replay.borrow_mut().seek_with_replay(position_ms);
                self.tasks
                    .borrow_mut()
                    .events
                    .push(WebAppEvent::ReplayChanged { rebuild: true });
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::ReplayStep { delta } => {
                let mut replay = self.replay.borrow_mut();
                if delta >= 0 {
                    for _ in 0..delta {
                        replay.step_forward();
                    }
                } else if let Some(target_cursor) =
                    replay.backward_cursor_by(delta.unsigned_abs() as usize)
                {
                    replay.seek_cursor_with_replay(target_cursor);
                }
                drop(replay);
                self.tasks
                    .borrow_mut()
                    .events
                    .push(WebAppEvent::ReplayChanged { rebuild: true });
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::AddReplayBookmark { name } => {
                self.replay.borrow_mut().add_bookmark(name);
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::RemoveReplayBookmark { position_ms } => {
                self.replay.borrow_mut().remove_bookmark(position_ms);
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::SetReplaySpeed { speed } => {
                self.replay.borrow_mut().set_speed(speed);
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::SetReplayPolicy { policy } => {
                self.replay.borrow_mut().set_policy(match policy {
                    ReplayPolicyView::AutoPreferRecorded => ReplayPolicy::AutoPreferRecorded,
                    ReplayPolicyView::ExactRecorded => ReplayPolicy::ExactRecorded,
                    ReplayPolicyView::ReparseRaw => ReplayPolicy::ReparseRaw,
                });
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::SetTerminalMergeWindow { ms } => {
                *self.terminal_merge_window_ms.borrow_mut() = ms;
                let max_entries = *self.terminal_max_entries.borrow();
                self.tasks
                    .borrow_mut()
                    .events
                    .push(WebAppEvent::TerminalSettingsChanged {
                        merge_window_ms: ms,
                        max_entries,
                    });
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::SetTerminalMaxEntries { max } => {
                *self.terminal_max_entries.borrow_mut() = max;
                let merge_window_ms = *self.terminal_merge_window_ms.borrow();
                self.tasks
                    .borrow_mut()
                    .events
                    .push(WebAppEvent::TerminalSettingsChanged {
                        merge_window_ms,
                        max_entries: max,
                    });
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::CancelTask { task_id } => {
                if self.cancel_task(task_id) {
                    Ok(CommandOutcome::Done)
                } else {
                    Err(format!("任务不存在：{task_id:?}"))
                }
            }
            AppCommand::CancelReconnect { port } => {
                if let Some(task_id) = self.reconnect_tasks.borrow_mut().remove(&port) {
                    let _ = self.cancel_task(task_id);
                }
                self.set_transport_status(format!("已取消重连 {port}"));

                // A Promise that is already inside `open()` cannot be
                // forcefully aborted by wasm-bindgen. Issue an explicit
                // disconnect as well; if the open was not completed yet, the
                // backend simply reports a harmless failed close task.
                let task_port = port.clone();
                if self.network_ports.borrow().contains_key(port.as_str()) {
                    let network = self.network_transport.clone();
                    let task_id = self.spawn_result(
                        "cancel_reconnect_disconnect",
                        async move {
                            let _ = network.disconnect(port).await;
                            Ok::<(), String>(())
                        },
                        move |_, _| WebAppEvent::Disconnected { port: task_port },
                    );
                    return Ok(CommandOutcome::Pending {
                        task_id,
                        message: "正在取消网络串口重连".to_owned(),
                    });
                }
                if let Some(transport) = self.transport.clone() {
                    let task_id = self.spawn_result(
                        "cancel_reconnect_disconnect",
                        async move {
                            let _ = transport.disconnect(port).await;
                            Ok::<(), String>(())
                        },
                        move |_, _| WebAppEvent::Disconnected { port: task_port },
                    );
                    return Ok(CommandOutcome::Pending {
                        task_id,
                        message: "正在取消串口重连".to_owned(),
                    });
                }
                Ok(CommandOutcome::Done)
            }
            AppCommand::RefreshPorts => {
                let transport = self.serial_transport()?;
                self.set_transport_status("正在刷新已授权串口");
                let network_ports = self.network_ports.clone();
                let task_id = self.spawn(
                    "refresh_ports",
                    async move {
                        let mut ports = transport.list_known_ports().await?;
                        ports.extend(
                            network_ports
                                .borrow()
                                .values()
                                .map(NetworkSerialConfig::descriptor),
                        );
                        Ok(ports)
                    },
                    |_, ports| WebAppEvent::PortsRefreshed(ports),
                );
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "正在刷新已授权串口".to_owned(),
                })
            }
            AppCommand::RegisterNetworkPort { config } => {
                let descriptor = config.descriptor();
                self.network_ports
                    .borrow_mut()
                    .insert(descriptor.id.to_string(), config);
                self.tasks
                    .borrow_mut()
                    .events
                    .push(WebAppEvent::NetworkPortAdded(descriptor));
                self.wake();
                Ok(CommandOutcome::Done)
            }
            AppCommand::RemoveNetworkPort { port } => {
                self.network_ports.borrow_mut().remove(port.as_str());
                if self.network_transport.is_connected(&port) {
                    let network = self.network_transport.clone();
                    let task_port = port.clone();
                    let task_id = self.spawn(
                        "remove_network_port",
                        async move { network.disconnect(port).await },
                        move |_, _| WebAppEvent::NetworkPortRemoved(task_port),
                    );
                    Ok(CommandOutcome::Pending {
                        task_id,
                        message: "正在关闭并移除网络串口".to_owned(),
                    })
                } else {
                    self.tasks
                        .borrow_mut()
                        .events
                        .push(WebAppEvent::NetworkPortRemoved(port));
                    self.wake();
                    Ok(CommandOutcome::Done)
                }
            }
            AppCommand::RequestPort => {
                let transport = self.serial_transport()?;
                self.set_transport_status("等待浏览器选择串口");
                // requestPort() must be created while the browser still has
                // the button's transient user activation.  The transport
                // deliberately constructs the Promise in request_port(),
                // before returning the future to this scheduler.
                let future = transport.request_port();
                let task_id = self.spawn("request_port", future, |id, port| {
                    WebAppEvent::PortRequested { id, port }
                });
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "等待浏览器选择串口".to_owned(),
                })
            }
            AppCommand::Connect { port, settings } => {
                *self.serial_settings.borrow_mut() = settings;
                if let Some(config) = self.network_ports.borrow().get(port.as_str()).cloned() {
                    return self.connect_network(port, config);
                }
                let transport = self.serial_transport()?;
                {
                    let mut view = self.transport_view.borrow_mut();
                    view.settings = settings;
                    view.connecting = true;
                    view.status = format!("正在打开串口 {port}");
                }
                let reconnect_port = port.clone();
                let task_port = port.clone();
                let bus = self.bus.clone();
                let task_events = self.tasks.clone();
                let repaint_waker = self.repaint_waker.clone();
                let task_id = self.spawn(
                    "connect_serial",
                    async move {
                        transport.connect(port.clone(), settings).await?;
                        let rx_port = port.clone();
                        let rx_bus = bus.clone();
                        let rx_waker = repaint_waker.clone();
                        let sink = Rc::new(move |bytes: Vec<u8>| {
                            rx_bus.publish(serial_rx_event(&rx_port, bytes));
                            wake_handle(&rx_waker);
                        });
                        let disconnect_waker = repaint_waker.clone();
                        let on_disconnect = Rc::new(move |port| {
                            task_events
                                .borrow_mut()
                                .events
                                .push(WebAppEvent::Disconnected { port });
                            wake_handle(&disconnect_waker);
                        });
                        transport
                            .start_receive_with_disconnect(port.clone(), sink, on_disconnect)
                            .map_err(|error| {
                                tool_platform::TransportError::Operation(error.to_string())
                            })?;
                        Ok(())
                    },
                    move |_, _| WebAppEvent::Connected { port: task_port },
                );
                self.reconnect_tasks
                    .borrow_mut()
                    .insert(reconnect_port, task_id);
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "正在打开串口".to_owned(),
                })
            }
            AppCommand::Reconnect { port } => {
                if let Some(config) = self.network_ports.borrow().get(port.as_str()).cloned() {
                    return self.reconnect_network(port, config);
                }
                let transport = self.serial_transport()?;
                let settings = *self.serial_settings.borrow();
                {
                    let mut view = self.transport_view.borrow_mut();
                    view.connecting = true;
                    view.settings = settings;
                    view.status = format!("正在重连串口 {port}");
                }
                let reconnect_port = port.clone();
                let task_port = port.clone();
                let bus = self.bus.clone();
                let task_events = self.tasks.clone();
                let repaint_waker = self.repaint_waker.clone();
                let task_id = self.spawn(
                    "reconnect_serial",
                    async move {
                        let _ = transport.disconnect(port.clone()).await;
                        transport.connect(port.clone(), settings).await?;
                        let rx_port = port.clone();
                        let rx_bus = bus.clone();
                        let rx_waker = repaint_waker.clone();
                        let sink = Rc::new(move |bytes: Vec<u8>| {
                            rx_bus.publish(serial_rx_event(&rx_port, bytes));
                            wake_handle(&rx_waker);
                        });
                        let disconnect_waker = repaint_waker.clone();
                        let on_disconnect = Rc::new(move |port| {
                            task_events
                                .borrow_mut()
                                .events
                                .push(WebAppEvent::Disconnected { port });
                            wake_handle(&disconnect_waker);
                        });
                        transport
                            .start_receive_with_disconnect(port.clone(), sink, on_disconnect)
                            .map_err(|error| {
                                tool_platform::TransportError::Operation(error.to_string())
                            })?;
                        Ok(())
                    },
                    move |_, _| WebAppEvent::Connected { port: task_port },
                );
                self.reconnect_tasks
                    .borrow_mut()
                    .insert(reconnect_port, task_id);
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "正在重连串口".to_owned(),
                })
            }
            AppCommand::Disconnect { port } => {
                self.set_transport_status(format!("正在关闭串口 {port}"));
                if self.network_ports.borrow().contains_key(port.as_str()) {
                    let network = self.network_transport.clone();
                    let task_port = port.clone();
                    let task_id = self.spawn(
                        "disconnect_network",
                        async move { network.disconnect(port).await },
                        move |_, _| WebAppEvent::Disconnected { port: task_port },
                    );
                    return Ok(CommandOutcome::Pending {
                        task_id,
                        message: "正在关闭网络串口".to_owned(),
                    });
                }
                let transport = self.serial_transport()?;
                let task_port = port.clone();
                let task_id = self.spawn(
                    "disconnect_serial",
                    async move { transport.disconnect(port).await },
                    move |_, _| WebAppEvent::Disconnected { port: task_port },
                );
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "正在关闭串口".to_owned(),
                })
            }
            AppCommand::SendText { port, text } => self.send(port, text.into_bytes()),
            AppCommand::SendHex { port, hex, strict } => {
                let bytes = if strict {
                    parse_hex_strict(&hex)?
                } else {
                    parse_hex(&hex)?
                };
                self.send(port, bytes)
            }
            AppCommand::SendRaw { port, bytes } => self.send(port, bytes),
            AppCommand::SetDtr { port, value } => {
                let transport = self.serial_transport()?;
                let task_port = port.clone();
                let task_id = self.spawn(
                    "set_dtr",
                    async move { transport.set_dtr(port, value).await },
                    move |_, _| WebAppEvent::SignalsChanged {
                        port: task_port,
                        signal: SignalKind::Dtr,
                        value,
                    },
                );
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "正在设置 DTR".to_owned(),
                })
            }
            AppCommand::SetRts { port, value } => {
                let transport = self.serial_transport()?;
                let task_port = port.clone();
                let task_id = self.spawn(
                    "set_rts",
                    async move { transport.set_rts(port, value).await },
                    move |_, _| WebAppEvent::SignalsChanged {
                        port: task_port,
                        signal: SignalKind::Rts,
                        value,
                    },
                );
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "正在设置 RTS".to_owned(),
                })
            }
            AppCommand::DiscoverPlugins { .. } => {
                Err("浏览器插件通过导入或插件市场加载".to_owned())
            }
        }
    }

    fn send(&self, port: PortId, bytes: Vec<u8>) -> Result<CommandOutcome, String> {
        self.set_transport_status(format!("正在发送到 {port}"));
        if self.network_ports.borrow().contains_key(port.as_str()) {
            let network = self.network_transport.clone();
            let task_port = port.clone();
            let bus = self.bus.clone();
            let byte_count = bytes.len();
            let task_id = self.spawn(
                "send_network",
                async move {
                    network.send(port.clone(), bytes.clone()).await?;
                    bus.publish(serial_tx_event(&port, bytes));
                    Ok(())
                },
                move |id, _| WebAppEvent::Sent {
                    id,
                    port: task_port,
                    bytes: byte_count,
                },
            );
            return Ok(CommandOutcome::Pending {
                task_id,
                message: "正在发送网络串口数据".to_owned(),
            });
        }
        let transport = self.serial_transport()?;
        let task_port = port.clone();
        let bus = self.bus.clone();
        let byte_count = bytes.len();
        let task_id = self.spawn(
            "send_serial",
            async move {
                transport.send(port.clone(), bytes.clone()).await?;
                bus.publish(serial_tx_event(&port, bytes));
                Ok(())
            },
            move |id, _| WebAppEvent::Sent {
                id,
                port: task_port,
                bytes: byte_count,
            },
        );
        Ok(CommandOutcome::Pending {
            task_id,
            message: "正在发送串口数据".to_owned(),
        })
    }

    fn connect_network(
        &self,
        port: PortId,
        config: NetworkSerialConfig,
    ) -> Result<CommandOutcome, String> {
        self.transport_view.borrow_mut().connecting = true;
        let network = self.network_transport.clone();
        let task_port = port.clone();
        let bus = self.bus.clone();
        let task_events = self.tasks.clone();
        let repaint_waker = self.repaint_waker.clone();
        let rx_port = port.clone();
        let reconnect_port = port.clone();
        let task_id = self.spawn(
            "connect_network",
            async move {
                let rx_bus = bus.clone();
                let rx_waker = repaint_waker.clone();
                let sink = Rc::new(move |bytes: Vec<u8>| {
                    rx_bus.publish(serial_rx_event(&rx_port, bytes));
                    wake_handle(&rx_waker);
                });
                let disconnect_port = port.clone();
                let disconnect_waker = repaint_waker.clone();
                let on_disconnect = Rc::new(move |_port: PortId| {
                    task_events
                        .borrow_mut()
                        .events
                        .push(WebAppEvent::Disconnected {
                            port: disconnect_port.clone(),
                        });
                    wake_handle(&disconnect_waker);
                });
                network.connect(config, sink, on_disconnect).await
            },
            move |_, _| WebAppEvent::Connected { port: task_port },
        );
        self.reconnect_tasks
            .borrow_mut()
            .insert(reconnect_port, task_id);
        Ok(CommandOutcome::Pending {
            task_id,
            message: "正在连接网络串口".to_owned(),
        })
    }

    fn reconnect_network(
        &self,
        port: PortId,
        config: NetworkSerialConfig,
    ) -> Result<CommandOutcome, String> {
        self.transport_view.borrow_mut().connecting = true;
        let network = self.network_transport.clone();
        let task_port = port.clone();
        let bus = self.bus.clone();
        let task_events = self.tasks.clone();
        let repaint_waker = self.repaint_waker.clone();
        let rx_port = port.clone();
        let reconnect_port = port.clone();
        let task_id = self.spawn(
            "reconnect_network",
            async move {
                let _ = network.disconnect(port.clone()).await;
                let rx_bus = bus.clone();
                let rx_waker = repaint_waker.clone();
                let sink = Rc::new(move |bytes: Vec<u8>| {
                    rx_bus.publish(serial_rx_event(&rx_port, bytes));
                    wake_handle(&rx_waker);
                });
                let disconnect_port = port.clone();
                let disconnect_waker = repaint_waker.clone();
                let on_disconnect = Rc::new(move |_port: PortId| {
                    task_events
                        .borrow_mut()
                        .events
                        .push(WebAppEvent::Disconnected {
                            port: disconnect_port.clone(),
                        });
                    wake_handle(&disconnect_waker);
                });
                network.connect(config, sink, on_disconnect).await
            },
            move |_, _| WebAppEvent::Connected { port: task_port },
        );
        self.reconnect_tasks
            .borrow_mut()
            .insert(reconnect_port, task_id);
        Ok(CommandOutcome::Pending {
            task_id,
            message: "正在重连网络串口".to_owned(),
        })
    }
}

fn is_transport_task_kind(kind: &str) -> bool {
    matches!(
        kind,
        "refresh_ports"
            | "request_port"
            | "connect_serial"
            | "reconnect_serial"
            | "connect_network"
            | "reconnect_network"
            | "disconnect_serial"
            | "disconnect_network"
            | "cancel_reconnect_disconnect"
            | "remove_network_port"
            | "send_serial"
            | "send_network"
            | "set_dtr"
            | "set_rts"
    )
}

impl crate::AppRuntime for WebApplication {
    fn capabilities(&self) -> crate::AppCapabilities {
        crate::AppCapabilities::web()
    }

    fn tick(&mut self) {
        self.tick_recording();
        self.tick_replay_load();
        self.replay.borrow_mut().tick();
    }

    fn dispatch(&mut self, command: crate::AppCommand) -> Result<crate::CommandOutcome, String> {
        WebApplication::dispatch(self, command)
    }
}

fn replay_policy_view(policy: ReplayPolicy) -> ReplayPolicyView {
    match policy {
        ReplayPolicy::AutoPreferRecorded => ReplayPolicyView::AutoPreferRecorded,
        ReplayPolicy::ExactRecorded => ReplayPolicyView::ExactRecorded,
        ReplayPolicy::ReparseRaw => ReplayPolicyView::ReparseRaw,
    }
}

fn should_record_event(event: &Event, mode: tool_recorder::RecordMode) -> bool {
    if event.is_replay()
        || matches!(event.origin(), Some("replay" | "replay_derived"))
        || event
            .meta_get("recordable")
            .is_some_and(|value| value == &serde_json::Value::Bool(false))
    {
        return false;
    }
    if matches!(
        event.topic.as_str(),
        "recorder.pause" | "recorder.resume" | "recorder.bookmark"
    ) {
        return true;
    }
    match mode {
        tool_recorder::RecordMode::RawSerial => event.topic.starts_with("transport.serial."),
        tool_recorder::RecordMode::StandardReplay => {
            event.topic.starts_with("transport.serial.")
                || event.topic.starts_with("protocol.")
                || event.topic == "ui.panel.create"
        }
    }
}

fn parse_hex(input: &str) -> Result<Vec<u8>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("HEX 输入为空".to_owned());
    }
    let tokens = trimmed
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',' || ch == ';')
        .filter(|token| !token.is_empty());
    let mut bytes = Vec::new();
    for token in tokens {
        let mut token = token
            .strip_prefix("0x")
            .or_else(|| token.strip_prefix("0X"))
            .unwrap_or(token)
            .chars()
            .filter(|ch| *ch != '_' && *ch != '-')
            .collect::<String>();
        if token.is_empty() {
            return Err("HEX token 为空".to_owned());
        }
        if token.len() > 2 && !token.len().is_multiple_of(2) {
            token.insert(0, '0');
        }
        if token.len() <= 2 {
            let value = if token.len() == 1 {
                format!("0{token}")
            } else {
                token
            };
            bytes.push(u8::from_str_radix(&value, 16).map_err(|_| format!("无效 HEX：{value}"))?);
        } else {
            for pair in token.as_bytes().chunks(2) {
                let value = std::str::from_utf8(pair).unwrap_or_default();
                bytes
                    .push(u8::from_str_radix(value, 16).map_err(|_| format!("无效 HEX：{value}"))?);
            }
        }
    }
    if bytes.is_empty() {
        return Err("HEX 输入为空".to_owned());
    }
    Ok(bytes)
}

fn parse_hex_strict(input: &str) -> Result<Vec<u8>, String> {
    for token in input
        .trim()
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',' || ch == ';')
        .filter(|token| !token.is_empty())
    {
        let normalized = token
            .strip_prefix("0x")
            .or_else(|| token.strip_prefix("0X"))
            .unwrap_or(token)
            .chars()
            .filter(|ch| *ch != '_' && *ch != '-')
            .collect::<String>();
        if normalized.len() != 2 || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("无效 HEX：{token}"));
        }
    }
    parse_hex(input)
}

fn wake_handle(handle: &Rc<RefCell<Option<RepaintWaker>>>) {
    let waker = handle.borrow().clone();
    if let Some(waker) = waker {
        waker();
    }
}
