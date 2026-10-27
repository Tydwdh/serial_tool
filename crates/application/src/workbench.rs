use std::collections::{BTreeSet, HashMap};

use tool_core::LogLevel;
use tool_databus::{DataBus, TopicFilter};
use tool_extension::PluginManager;
use tool_lua_host::{DialogRequest, FileAccessBroker};
use tool_platform::native::block_on_transport;
use tool_platform::storage::{FileBlob, FileHandle, native::NativeFileService};
use tool_platform::{PortDescriptor, PortId, SerialParity, SerialSettings, TransportBackend};
use tool_recorder::{JsonlRecorder, ReplayManager};
use tool_transport::{SerialPortDescriptor, TransportManager};

use crate::command::{AppCommand, CommandOutcome};
use crate::error::AppError;
use crate::query::{NetworkPortConfig, PluginView, RecordingStatusView, ReplayStatusView};
use crate::send_plan::{self, PlannedSend};
use crate::task::{AppEvent, TaskContext, TaskId, TaskKind, TaskManager, TaskResult, TaskSnapshot};
use crate::task_model::cancel_deregistered_task;
use crate::transport::{PortStatusView, PortView, SerialConfigView, TransportView};

#[derive(Debug, Clone)]
pub struct ApplicationConfig {
    pub selected_port: Option<String>,
    pub baud_rate: String,
    pub data_bits: String,
    pub stop_bits: String,
    pub parity: String,
    pub auto_reconnect: bool,
    /// Terminal display-block idle-finalize threshold, not a protocol merge window.
    pub terminal_merge_window_ms: u64,
    pub terminal_max_entries: usize,
    pub log_max_entries: usize,
    pub recorder_path: String,
    pub network_ports: Vec<NetworkPortConfig>,
    pub port_aliases: HashMap<String, String>,
    pub port_groups: HashMap<String, String>,
    pub enabled_plugins: Vec<String>,
    pub network_proxy_url: Option<String>,
}

impl Default for ApplicationConfig {
    fn default() -> Self {
        Self {
            selected_port: None,
            baud_rate: "115200".to_owned(),
            data_bits: "8".to_owned(),
            stop_bits: "1".to_owned(),
            parity: "none".to_owned(),
            auto_reconnect: true,
            terminal_merge_window_ms: 5,
            terminal_max_entries: 50_000,
            log_max_entries: 50_000,
            recorder_path: String::new(),
            network_ports: Vec::new(),
            port_aliases: HashMap::new(),
            port_groups: HashMap::new(),
            enabled_plugins: Vec::new(),
            network_proxy_url: None,
        }
    }
}

pub struct Workbench {
    bus: DataBus,
    transport: TransportManager,
    file_service: NativeFileService,
    recorder: JsonlRecorder,
    replay: ReplayManager,
    plugin_manager: PluginManager,
    app_config: ApplicationConfig,
    ports: Vec<SerialPortDescriptor>,
    selected_port: Option<String>,
    tasks: TaskManager,
    reconnect_tasks: HashMap<String, TaskId>,
    _file_broker: std::sync::Arc<FileAccessBroker>,
    _dialog_receiver: crossbeam_channel::Receiver<DialogRequest>,
}

/// Application-owned handle for publishing an application event.
///
/// The underlying bus remains an implementation detail of `Workbench`; callers
/// only receive a cloneable publisher and cannot access transport/recorder
/// state through it.
#[derive(Clone)]
pub struct EventSink {
    bus: DataBus,
}

impl EventSink {
    pub fn publish(&self, event: tool_core::Event) {
        self.bus.publish(event);
    }
}

impl tool_databus::EventPublisher for EventSink {
    fn publish_event(&self, event: tool_core::Event) {
        self.publish(event);
    }
}

/// Application-owned transport endpoint used by long-lived presentation jobs.
///
/// It intentionally exposes only sending, not the transport manager itself.
#[derive(Clone)]
pub struct TransportEndpoint {
    transport: TransportManager,
}

/// UI 需要的应用事件订阅集合。订阅由 Application 创建，UI 只能消费已经
/// 定义好的事件流，不能自行访问 Application 的内部总线。
pub struct UiEventSubscriptions {
    file_browse: tool_databus::Subscription,
    contribution_set_value: tool_databus::Subscription,
    set_status: tool_databus::Subscription,
    serial_closed: tool_databus::Subscription,
}

impl UiEventSubscriptions {
    pub fn try_file_browse(&self) -> Option<tool_core::Event> {
        self.file_browse.try_recv()
    }

    pub fn drain_contribution_set_value(&self, limit: usize) -> Vec<tool_core::Event> {
        self.contribution_set_value.drain_limited(limit)
    }

    pub fn drain_status(&self, limit: usize) -> Vec<tool_core::Event> {
        self.set_status.drain_limited(limit)
    }

    /// 端口关闭/失联事件（`transport.serial.closed`）。
    ///
    /// 这是 UI 唯一能区分「用户关了端口」与「设备被拔/链路失效」的信号：
    /// 两者在 `status_port` 上都是 `open: false`，只有 `reason` 分得开。
    pub fn drain_serial_disconnects(&self, limit: usize) -> Vec<tool_transport::SerialClosed> {
        self.serial_closed
            .drain_limited(limit)
            .iter()
            .filter_map(tool_transport::serial_closed_from_event)
            .collect()
    }
}

impl TransportEndpoint {
    /// 周期发送等后台 job 的投递入口。签名不变，但内部不再自己判定「HEX 怎么解、
    /// 换行加在哪」：命令由 `send_plan::send_command` 构造、字节由 `send_plan::plan_send`
    /// 解码，与 `Workbench::dispatch_send`（点一次发送）同一份规则。
    ///
    /// `plan_send` 的 `is_network` 实参在本端点恒为 `false` 且不改变行为：那个标志只决定
    /// `PlannedSend::task_kind`（任务队列的标签），而本端点两条路径都是 `send_to(port_name, bytes)`，
    /// 真正的后端由 `TransportManager` 按端口名选。走哪个后端在这里判定过一次就是第二份规则。
    pub fn send(
        &self,
        port_name: &str,
        input: &str,
        hex_mode: bool,
        line_ending: &str,
        hex_strict: bool,
    ) -> Result<(), String> {
        let command = send_plan::send_command(port_name, input, hex_mode, line_ending, hex_strict);
        let plan = send_plan::plan_send(&command, false).map_err(|error| error.to_string())?;
        self.transport
            .send_to(port_name, plan.bytes)
            .map_err(|error| tool_transport::translate_error(&error))
    }
}

impl Workbench {
    pub fn new(bus: DataBus) -> Self {
        let transport = TransportManager::new(bus.clone());
        let (dialog_sender, dialog_receiver) = crossbeam_channel::unbounded::<DialogRequest>();
        let file_broker = std::sync::Arc::new(FileAccessBroker::default());
        let mut pm = PluginManager::new(bus.clone(), transport.clone());
        pm.set_host_services(dialog_sender, file_broker.clone());
        let recorder = JsonlRecorder::new(bus.clone());
        let replay = ReplayManager::new(bus.clone());
        Self {
            bus,
            transport,
            file_service: NativeFileService::new("."),
            recorder,
            replay,
            plugin_manager: pm,
            app_config: ApplicationConfig::default(),
            ports: Vec::new(),
            selected_port: None,
            tasks: TaskManager::new(),
            reconnect_tasks: HashMap::new(),
            _file_broker: file_broker,
            _dialog_receiver: dialog_receiver,
        }
    }

    pub fn with_config(mut self, config: ApplicationConfig) -> Self {
        self.selected_port = config.selected_port.clone();
        self.app_config = config;
        self
    }

    pub fn tick(&mut self, _now_secs: f64) {
        self.poll_tasks();
        self.recorder.tick_backpressure();
    }

    /// 推进唯一的 Application-owned 回放状态，并返回本帧发布的事件数。
    pub fn tick_replay(&mut self) -> usize {
        self.replay.tick()
    }

    pub fn task_snapshots(&self) -> Vec<TaskSnapshot> {
        self.tasks.snapshots()
    }

    pub fn perf_snapshot(&self) -> crate::perf::ApplicationPerfSnapshot {
        crate::perf::ApplicationPerfSnapshot {
            databus: self.bus.perf_snapshot(),
            recorder: self.recorder.stats(),
        }
    }

    pub fn has_active_task_kind(&self, kind: &str) -> bool {
        self.tasks.snapshots().iter().any(|snapshot| {
            snapshot.kind == kind
                && matches!(
                    snapshot.state,
                    crate::task::TaskState::Pending | crate::task::TaskState::Running
                )
        })
    }

    pub fn cancel_task(&mut self, task_id: TaskId) -> bool {
        self.tasks.cancel(task_id)
    }

    /// 供 Presentation 层提交已经准备好数据的文件导出任务。
    ///
    /// Presentation 只把拥有所有权的快照捕获进闭包；格式化和文件 IO 仍由
    /// Workbench 的统一 worker 执行，因此不会在 egui 事件回调里写大文件。
    pub fn spawn_file_export<F>(
        &mut self,
        kind: impl Into<String>,
        format: String,
        file: FileHandle,
        render: F,
    ) -> Result<CommandOutcome, AppError>
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        let file_service = self.file_service.clone();
        let path = file
            .native_path()
            .ok_or_else(|| AppError::Storage("Native 导出需要路径型文件句柄".to_owned()))?
            .to_path_buf();
        let task_id = self.tasks.spawn(kind, move |_context| {
            let content = render()?;
            write_exported_content(&file_service, &path, &file, &format, content)?;
            Ok(TaskResult::FileExported { file })
        });
        Ok(pending(task_id, "正在导出文件"))
    }

    /// One command plan for both roots: [`crate::command::execute`] resolves
    /// validation, routing and outcome text; this adapter performs the native
    /// effects through [`crate::command::CommandEffects`].
    pub fn dispatch(&mut self, command: AppCommand) -> Result<CommandOutcome, AppError> {
        crate::command::execute(self, command).map_err(AppError::from)
    }

    pub fn query_transport(&self) -> TransportView {
        let statuses: Vec<PortStatusView> = self
            .transport
            .status_all()
            .into_iter()
            .map(PortStatusView::from)
            .collect();
        let selected = self.selected_port.as_deref();
        let selected_status = selected.and_then(|port| {
            statuses
                .iter()
                .find(|status| status.port_name.as_deref() == Some(port))
        });
        let mut view = TransportView::new(tool_platform::TransportCapabilities::NATIVE_SERIAL);
        view.ports = self.ports.iter().cloned().map(PortView::from).collect();
        view.open_ports = self.transport.open_ports();
        let connected = selected_status
            .filter(|status| status.open)
            .and_then(|_| selected.map(|port| PortId::new(port.to_owned())));
        let connecting = selected_status.is_some_and(|status| status.connecting);
        view.statuses = statuses;
        view.connected = connected;
        view.connecting = connecting;
        view.settings = self.platform_serial_settings();
        view.config = SerialConfigView {
            port_name: self.selected_port.clone().unwrap_or_default(),
            baud_rate: view.settings.baud_rate,
            data_bits: self.app_config.data_bits.clone(),
            stop_bits: self.app_config.stop_bits.clone(),
            parity: self.app_config.parity.clone(),
        };
        view.auto_reconnect = self.app_config.auto_reconnect;
        view
    }

    pub fn query_recording(&self) -> RecordingStatusView {
        RecordingStatusView {
            stats: self.recorder.stats().into(),
            path: self
                .recorder
                .current_path()
                .map(|path| path.display().to_string()),
            mode: self.recorder.mode().into(),
        }
    }

    pub fn query_replay(&self) -> ReplayStatusView {
        let status = self.replay.status();
        ReplayStatusView {
            state: status.state.into(),
            path: status.path.map(|path| path.display().to_string()),
            total_events: status.total_events,
            cursor: status.cursor,
            speed: status.speed,
            position_ms: status.position_ms,
            duration_ms: status.duration_ms,
            policy: status.policy.into(),
            effective_policy: status.effective_policy.into(),
            has_recorded_protocol: status.has_recorded_protocol,
            analyzer_cache_entries: status.analyzer_cache_entries,
            analyzer_cache_valid: self.replay.analyzer_cache_valid(),
            analyzer_error: status.analyzer_error,
            analyzer_warning: status.analyzer_warning,
            can_play: self.replay.can_play(),
            can_seek: self.replay.can_seek(),
            block_reason: self.replay.replay_block_reason().map(Into::into),
            bookmarks: self
                .replay
                .bookmarks()
                .iter()
                .map(|bookmark| crate::query::ReplayBookmarkView {
                    position_ms: bookmark.pos_ms,
                    name: bookmark.name.clone(),
                })
                .collect(),
            load_report: status.load_report.map(Into::into),
        }
    }

    pub fn replay_raw_serial_events(&self) -> Vec<tool_core::Event> {
        self.replay.raw_serial_events()
    }

    pub fn replay_set_analyzer_cache(&mut self, events: Vec<tool_core::Event>) {
        self.replay.set_analyzer_cache(events);
    }

    pub fn replay_set_analyzer_error(&mut self, error: String) {
        self.replay.set_analyzer_error(error);
    }

    pub fn replay_set_analyzer_warning(&mut self, warning: String) {
        self.replay.set_analyzer_warning(warning);
    }

    pub fn replay_clear_analyzer_messages(&mut self) {
        self.replay.clear_analyzer_error();
    }

    pub fn replay_seek_panel_phase(&mut self, position_ms: u64) -> usize {
        self.replay.seek_panel_phase(position_ms)
    }

    pub fn replay_seek_data_phase(&mut self, position_ms: u64) -> usize {
        self.replay.seek_data_phase(position_ms)
    }

    pub fn replay_seek_cursor_panel_phase(&mut self, target_cursor: usize) -> usize {
        self.replay.seek_cursor_panel_phase(target_cursor)
    }

    pub fn replay_seek_cursor_data_phase(&mut self, target_cursor: usize) -> usize {
        self.replay.seek_cursor_data_phase(target_cursor)
    }

    pub fn replay_backward_cursor_by(&self, steps: usize) -> Option<usize> {
        self.replay.backward_cursor_by(steps)
    }

    pub fn query_plugins(&self) -> PluginView {
        PluginView {
            summaries: self
                .plugin_manager
                .summaries()
                .into_iter()
                .map(Into::into)
                .collect(),
            diagnostics: self
                .plugin_manager
                .diagnostics()
                .iter()
                .cloned()
                .map(Into::into)
                .collect(),
        }
    }

    /// Persisted plugin enablement is application state, not a UI config
    /// implementation detail.  The Native composition uses this projection
    /// during startup to turn discovered plugins into running plugins.
    pub fn query_enabled_plugin_ids(&self) -> Vec<String> {
        self.app_config.enabled_plugins.clone()
    }

    /// 发布应用事件。UI/runtime 不需要持有 DataBus 或其他业务管理器。
    pub fn publish_event(&self, event: tool_core::Event) {
        self.bus.publish(event);
    }

    pub fn event_sink(&self) -> EventSink {
        EventSink {
            bus: self.bus.clone(),
        }
    }

    pub fn subscribe_ui_events(&self) -> UiEventSubscriptions {
        UiEventSubscriptions {
            file_browse: self.bus.subscribe_lossy_bounded(
                TopicFilter::exact(tool_core::topics::UI_FORM_FILE_BROWSE),
                1024,
            ),
            contribution_set_value: self.bus.subscribe_lossy_bounded(
                TopicFilter::exact(tool_core::topics::UI_CONTRIBUTION_SET_VALUE),
                1024,
            ),
            set_status: self.bus.subscribe_lossy_bounded(
                TopicFilter::exact(tool_core::topics::UI_SET_STATUS),
                1024,
            ),
            serial_closed: self
                .bus
                .subscribe_lossy_bounded(TopicFilter::exact(tool_core::topics::SERIAL_CLOSED), 128),
        }
    }

    /// 为后台 presentation job 创建一个只具备发送能力的端点。
    pub fn transport_endpoint(&self) -> TransportEndpoint {
        TransportEndpoint {
            transport: self.transport.clone(),
        }
    }

    /// 仅用于进程退出时释放串口资源。
    pub fn shutdown_serial(&self) {
        self.transport.close_serial();
    }

    pub fn set_transport_repaint_waker(
        &self,
        waker: std::sync::Arc<dyn tool_transport::RepaintWaker>,
    ) {
        self.transport.set_repaint_waker(waker);
    }

    pub fn open_port_names(&self) -> Vec<String> {
        self.transport.open_ports()
    }

    pub fn transport_status(&self, port: &str) -> PortStatusView {
        self.transport.status_port(port).into()
    }

    pub fn serial_settings(&self) -> SerialSettings {
        self.platform_serial_settings()
    }

    pub fn reap_recording_stop(&mut self) -> Option<Result<String, String>> {
        self.recorder
            .reap_stopping()
            .map(|result| result.map(|path| path.display().to_string()))
    }

    pub fn reap_recording_error(&mut self) -> Option<String> {
        self.recorder.reap_error()
    }

    pub fn log(&self, lv: LogLevel, msg: impl Into<String>) {
        self.bus
            .publish(tool_core::Event::system_log(lv, "app", msg.into()));
    }

    pub fn plugin_state(&self, plugin_id: &str) -> Option<crate::query::PluginStateView> {
        self.plugin_manager.plugin_state(plugin_id).map(Into::into)
    }

    pub fn plugin_ids(&self) -> Vec<String> {
        self.plugin_manager.plugin_ids()
    }

    /// 插件运行时是否仍在关闭中：重启交接等下一帧的依据，来自 PluginManager 的权威状态。
    pub fn plugin_is_stopping(&mut self, plugin_id: &str) -> bool {
        self.plugin_manager.is_stopping(plugin_id)
    }

    pub fn process_plugin_lifecycle(&mut self) -> usize {
        self.plugin_manager.process_pending()
    }

    pub fn take_plugin_cleanup_requests(&mut self) -> Vec<String> {
        self.plugin_manager.take_cleanup_requests()
    }

    pub fn plugin_config_root(&self) -> String {
        self.plugin_manager.config_root().display().to_string()
    }

    pub fn plugin_settings(&self) -> Vec<(String, String, Vec<crate::query::PluginSettingView>)> {
        self.plugin_manager
            .plugin_settings()
            .into_iter()
            .map(|(plugin_id, plugin_name, settings)| {
                (
                    plugin_id,
                    plugin_name,
                    settings.into_iter().map(Into::into).collect(),
                )
            })
            .collect()
    }

    pub fn plugin_setting_value(
        &self,
        plugin_id: &str,
        key: &str,
        default: serde_json::Value,
    ) -> serde_json::Value {
        self.plugin_manager
            .config_store()
            .get(plugin_id, key, default)
    }

    pub fn plugin_setting_keys(&self, plugin_id: &str) -> Vec<String> {
        self.plugin_manager.config_store().keys(plugin_id)
    }

    pub fn set_plugin_setting(
        &self,
        plugin_id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), AppError> {
        self.plugin_manager
            .config_store()
            .set(plugin_id, key, value)
            .map_err(|error| AppError::Plugin(error.to_string()))
    }

    pub fn replay_analyzer_entries(&self) -> Vec<tool_extension::manifest::ReplayAnalyzerEntry> {
        self.plugin_manager.replay_analyzer_entries()
    }

    pub fn try_dialog_request(&self) -> Option<DialogRequest> {
        self._dialog_receiver.try_recv().ok()
    }

    pub fn authorize_plugin_file(&self, plugin_id: &str, file: FileHandle) {
        if let Some(path) = file.native_path() {
            self._file_broker.authorize(plugin_id, path.to_path_buf());
        }
    }

    pub fn clear_plugin_file_authorization(&self, plugin_id: &str) {
        self._file_broker.clear(plugin_id);
    }

    // —— 以下是 `dispatch` 各命令分支的实现：只搬位置、不改判定，顺序与 dispatch 里的
    //    match 分支一致，便于与 wasm 侧 `WebApplication::dispatch` 逐条对照。

    /// 注册（或按 `display_name()` 就地覆盖）一个网络端口配置，并补上对应的端口描述符
    /// 表项：配置表是 `is_network_port` 的判定输入，描述符列表决定端口选择器里能看到谁。
    fn register_network_port(&mut self, config: NetworkPortConfig) {
        let name = config.display_name();
        if let Some(existing) = self
            .app_config
            .network_ports
            .iter_mut()
            .find(|existing| network_config_matches(existing, &name))
        {
            *existing = config;
        } else {
            self.app_config.network_ports.push(config);
        }
        if !self.ports.iter().any(|port| port.port_name == name) {
            self.ports.push(SerialPortDescriptor {
                port_name: name,
                port_type: tool_transport::PortType::Network,
            });
        }
    }

    /// 注销网络端口。两种形态都认（判定见 `network_config_matches`），之后再按剩余配置
    /// 回收 Network 类型的描述符，避免留下无人认领的端口表项。
    fn remove_network_port(&mut self, port: PortId) {
        let port = port.to_string();
        self.app_config
            .network_ports
            .retain(|config| !network_config_matches(config, &port));
        // 例外：描述符名由本端生成（`register_network_port` / `apply_ports`），恒为规范形，
        // 不是用户点名的形态 —— 那是另一个输入域，不走 `network_config_matches`。
        self.ports.retain(|descriptor| {
            descriptor.port_type != tool_transport::PortType::Network
                || self
                    .app_config
                    .network_ports
                    .iter()
                    .any(|config| config.display_name() == descriptor.port_name)
        });
    }

    /// 刷新串口列表。同一时刻只允许一个 `refresh_ports` 任务：重复提交复用已在跑的那个，
    /// 而不是再起一份互相覆盖结果的扫描。
    fn spawn_refresh_ports(&mut self) -> CommandOutcome {
        if let Some(task_id) = self.tasks.active_task_id(TaskKind::RefreshPorts.as_str()) {
            return pending(task_id, "正在刷新串口列表");
        }
        let backend = self.transport.clone();
        let task_id = self.tasks.spawn(TaskKind::RefreshPorts, move |_context| {
            block_on_transport(backend.list_known_ports())
                .map(TaskResult::PlatformPortsRefreshed)
                .map_err(|error| tool_platform::translate_error(&error))
        });
        pending(task_id, "正在刷新串口列表")
    }

    fn spawn_request_port(&mut self) -> CommandOutcome {
        // Native has no browser permission prompt. Returning the first
        // known port keeps the command portable while preserving the
        // user-driven request semantics on Web.
        if let Some(task_id) = self.tasks.active_task_id(TaskKind::RequestPort.as_str()) {
            return pending(task_id, "正在获取串口");
        }
        let backend = self.transport.clone();
        let task_id = self.tasks.spawn(TaskKind::RequestPort, move |_context| {
            block_on_transport(backend.request_port())
                .map(|port| TaskResult::PlatformPortsRefreshed(vec![port]))
                .map_err(|error| tool_platform::translate_error(&error))
        });
        pending(task_id, "正在获取串口")
    }

    /// 把当前串口参数写回应用配置，供后续 connect/reconnect 复用。
    fn apply_serial_settings(&mut self, settings: SerialSettings) {
        self.app_config.baud_rate = settings.baud_rate.to_string();
        self.app_config.data_bits = settings.data_bits.to_string();
        self.app_config.stop_bits = settings.stop_bits.to_string();
        self.app_config.parity = match settings.parity {
            SerialParity::None => "none",
            SerialParity::Odd => "odd",
            SerialParity::Even => "even",
        }
        .to_owned();
    }

    /// 关闭端口。两条路径都落在 `serial:{port}` 这条 per-port 有序队列上，于是「关闭」
    /// 不会与该端口上仍在排队的发送任务交错；网络端口走 transport 的阻塞关闭（自带
    /// 超时），真实串口走 platform backend 的 disconnect。
    fn disconnect_port(&mut self, port_name: String) -> CommandOutcome {
        // 注册过的端口一律换成规范名 `display_name()` 再进队列（理由见 `connect`）：否则
        // `Disconnect` 与仍在排队的 `SendText` 落在两条 `serial:{port}` 队列上，上面那句
        // 顺序保证就没了。
        let canonical = self
            .find_network_port(&port_name)
            .map(NetworkPortConfig::display_name);
        let is_network = canonical.is_some();
        let port_name = canonical.unwrap_or(port_name);
        let task_port = port_name.clone();
        let task_id = if is_network {
            let transport = self.transport.clone();
            self.tasks.spawn_ordered(
                format!("serial:{task_port}"),
                TaskKind::DisconnectNetwork,
                move |_context| {
                    transport
                        .close_port_blocking(&task_port, std::time::Duration::from_secs(3))
                        .map_err(|error| tool_transport::translate_error(&error))?;
                    Ok(TaskResult::Disconnected {
                        port_name: task_port,
                    })
                },
            )
        } else {
            let backend = self.transport.clone();
            let port = PortId::new(task_port.clone());
            self.tasks.spawn_ordered(
                format!("serial:{task_port}"),
                TaskKind::DisconnectSerial,
                move |_context| {
                    block_on_transport(backend.disconnect(port.clone()))
                        .map_err(|error| tool_platform::translate_error(&error))?;
                    Ok(TaskResult::TransportDisconnected { port })
                },
            )
        };
        pending(task_id, format!("正在关闭 {port_name}"))
    }

    fn cancel_reconnect(&mut self, port_name: String) {
        // `start_reconnect` 存的键与 `status_port` 的端口表都是规范形，这里必须同源，
        // 否则 `port_id()` 形态的取消请求找不到人（理由见 `connect`）。
        let port_name = self
            .find_network_port(&port_name)
            .map(NetworkPortConfig::display_name)
            .unwrap_or(port_name);
        let previous = self.reconnect_tasks.remove(&port_name);
        cancel_deregistered_task(previous, |task_id| {
            self.tasks.cancel(task_id);
        });
        // 网络连接中也通过统一任务模型关闭，避免 CancelReconnect 在 UI 线程
        // 直接触碰 transport 生命周期。
        if self.transport.status_port(&port_name).connecting {
            let transport = self.transport.clone();
            let task_port = port_name.clone();
            self.tasks.spawn_ordered(
                format!("serial:{task_port}"),
                TaskKind::CancelReconnect,
                move |_context| {
                    transport.close_port(&task_port);
                    Ok(TaskResult::Disconnected {
                        port_name: task_port,
                    })
                },
            );
        }
    }

    fn dispatch_start_recording(
        &mut self,
        file: FileHandle,
        mode: crate::recording::RecordModeView,
    ) -> Result<CommandOutcome, AppError> {
        let path = file
            .native_path()
            .ok_or_else(|| AppError::Recording("Native 录制需要路径型文件句柄".to_owned()))?
            .to_path_buf();
        self.recorder.set_mode(mode.into());
        self.recorder
            .start(path)
            .map(|()| CommandOutcome::Done)
            .map_err(|e| AppError::Recording(e.to_string()))
    }

    /// 回放文件的解析（读完整个 JSONL）交给后台任务，UI 线程上的 `dispatch` 只投递。
    fn dispatch_load_replay(&mut self, file: FileHandle) -> Result<CommandOutcome, AppError> {
        let path = file
            .native_path()
            .ok_or_else(|| AppError::Replay("Native 回放需要路径型文件句柄".to_owned()))?
            .to_path_buf();
        let task_id = self.tasks.spawn(TaskKind::LoadReplay, move |_context| {
            ReplayManager::prepare_load(&path)
                .map(TaskResult::ReplayLoaded)
                .map_err(|error| error.to_string())
        });
        Ok(pending(task_id, "正在加载回放文件"))
    }

    fn replay_seek_by(&mut self, delta_ms: i64) {
        let cur = self.replay.status().position_ms;
        let next = if delta_ms < 0 {
            cur.saturating_sub((-delta_ms) as u64)
        } else {
            cur.saturating_add(delta_ms as u64)
        };
        self.replay.seek_ms(next);
    }

    fn replay_step(&mut self, delta: i32) {
        if delta > 0 {
            for _ in 0..delta {
                let _ = self.replay.step_forward();
            }
        } else {
            for _ in 0..(-delta) {
                self.replay.step_backward();
            }
        }
    }

    /// 用应用侧记录的插件根目录重扫一次（与 `DiscoverPlugins` 的区别只在根从哪来）。
    fn spawn_reload_plugins(&mut self) -> CommandOutcome {
        let roots = self.plugin_manager.roots();
        let task_id = self.tasks.spawn(TaskKind::ReloadPlugins, move |_context| {
            tool_extension::PluginManager::scan_roots(&roots)
                .map(TaskResult::PluginsDiscovered)
                .map_err(|error| tool_extension::translate_error(&error))
        });
        pending(task_id, "正在扫描插件")
    }

    fn spawn_discover_plugins(&mut self, roots: Vec<std::path::PathBuf>) -> CommandOutcome {
        let task_id = self
            .tasks
            .spawn(TaskKind::DiscoverPlugins, move |_context| {
                tool_extension::PluginManager::scan_roots(&roots)
                    .map(TaskResult::PluginsDiscovered)
                    .map_err(|error| tool_extension::translate_error(&error))
            });
        pending(task_id, "正在扫描插件")
    }

    fn publish_plugin_command(
        &self,
        plugin_id: String,
        command_id: String,
        context: serde_json::Value,
    ) {
        let mut payload = context;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("plugin_id".to_owned(), serde_json::json!(plugin_id));
            obj.insert("command".to_owned(), serde_json::json!(command_id));
            obj.insert("origin".to_owned(), serde_json::json!("host.command"));
        }
        self.bus.publish(tool_core::Event::new(
            tool_core::topics::PLUGIN_COMMAND_EXECUTE,
            "plugin.command",
            tool_core::Direction::Internal,
            tool_core::Payload::Json(payload),
        ));
    }

    fn apply_platform_ports(&mut self, available: Vec<PortDescriptor>) {
        let ports = available
            .into_iter()
            .map(|port| SerialPortDescriptor {
                port_name: port.id.to_string(),
                port_type: tool_transport::PortType::Unknown,
            })
            .collect();
        self.apply_ports(ports);
    }

    fn apply_ports(&mut self, available: Vec<SerialPortDescriptor>) {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for descriptor in available {
            seen.insert(descriptor.port_name.clone());
            if !self
                .ports
                .iter()
                .any(|port| port.port_name == descriptor.port_name)
            {
                self.ports.push(descriptor);
            }
        }
        self.ports.retain(|port| {
            port.port_type == tool_transport::PortType::Network || seen.contains(&port.port_name)
        });
        for network in &self.app_config.network_ports {
            let name = network.display_name();
            if !self.ports.iter().any(|port| port.port_name == name) {
                self.ports.push(SerialPortDescriptor {
                    port_name: name,
                    port_type: tool_transport::PortType::Network,
                });
            }
        }
        self.ports.sort_by(|a, b| {
            tool_transport::natural_sort_key(&a.port_name)
                .cmp(&tool_transport::natural_sort_key(&b.port_name))
        });
    }

    fn connect(
        &mut self,
        port_name: &str,
        settings: SerialSettings,
    ) -> Result<CommandOutcome, AppError> {
        if let Some(registered) = self.find_network_port(port_name).cloned() {
            // 之后一律用规范名 `display_name()`，两个理由各管一半：
            // - `TransportManager` 的端口表以它为键（`open_network_serial` 自己从配置算），
            //   把 `network://…` 留在写回的选中端口里，下一句发送就会在 `enqueue_command`
            //   上拿到 `PortNotOpen` —— 那是把「连不上」换成「连上了但发不出」。
            // - 队列键 `serial:{port}` 也必须同源，否则同一端口换个形态点名就落到另一条
            //   队列上，`spawn_ordered` 保证的「关闭不与仍在排队的发送交错」静默失效。
            let port_name = registered.display_name();
            let transport = self.transport.clone();
            let net = registered;
            let task_port = port_name.clone();
            let task_id = self.tasks.spawn_ordered(
                format!("serial:{task_port}"),
                TaskKind::ConnectNetwork,
                move |_context| {
                    transport
                        .open_network_serial(net)
                        .map(|_| TaskResult::Connected {
                            port_name: task_port,
                        })
                        .map_err(|error| tool_transport::translate_error(&error))
                },
            );
            Ok(pending(task_id, format!("正在连接 {port_name}")))
        } else {
            let task_port = port_name.to_owned();
            let backend = self.transport.clone();
            let port = PortId::new(task_port.clone());
            let task_id = self.tasks.spawn_ordered(
                format!("serial:{task_port}"),
                TaskKind::ConnectSerial,
                move |_context| {
                    block_on_transport(backend.connect(port.clone(), settings))
                        .map_err(|error| tool_platform::translate_error(&error))?;
                    Ok(TaskResult::TransportConnected { port })
                },
            );
            Ok(pending(task_id, format!("正在打开 {port_name}")))
        }
    }

    fn platform_serial_settings(&self) -> SerialSettings {
        SerialSettings {
            baud_rate: self.app_config.baud_rate.parse().unwrap_or(115_200),
            data_bits: self.app_config.data_bits.parse().unwrap_or(8),
            stop_bits: self.app_config.stop_bits.parse().unwrap_or(1),
            parity: match self.app_config.parity.as_str() {
                "odd" => SerialParity::Odd,
                "even" => SerialParity::Even,
                _ => SerialParity::None,
            },
        }
    }

    /// 按 `port_name` 取出对应的已注册网络端口配置；两种形态都认，判定见
    /// `network_config_matches`。调用方拿它做两件事：认出网络端口、以及把名字换成
    /// 规范形 `display_name()`（`TransportManager` 与 `serial:{port}` 队列都以它为键）。
    fn find_network_port(&self, port_name: &str) -> Option<&NetworkPortConfig> {
        self.app_config
            .network_ports
            .iter()
            .find(|config| network_config_matches(config, port_name))
    }

    /// 「这条命令的端口是不是已注册的网络端口」—— 结论只作为 `plan_send` 的 `is_network`
    /// 实参，本平台不再自己决定任务种类。两种形态与它为什么不会误命中串口名，见
    /// `network_config_matches`。
    fn is_network_port(&self, port_name: &str) -> bool {
        self.find_network_port(port_name).is_some()
    }

    /// 按 `plan_send` 算出的种类与字节投递。走哪个后端由 `PlannedSend::targets_network_port()`
    /// 决定（那是 `send_plan` 里被单测钉住的判定），本平台不再比一次字符串，
    /// 也不再对照一次端口名单。
    fn send_transport_bytes(
        &mut self,
        port_name: String,
        plan: PlannedSend,
    ) -> Result<CommandOutcome, AppError> {
        let to_network = plan.targets_network_port();
        let PlannedSend { task_kind, bytes } = plan;
        if to_network {
            // 一律换成规范名再进队列、再交给 transport（理由见 `connect`）。
            let port_name = self
                .find_network_port(&port_name)
                .map(NetworkPortConfig::display_name)
                .unwrap_or(port_name);
            let transport = self.transport.clone();
            let task_port = port_name.clone();
            let task_id = self.tasks.spawn_ordered(
                format!("serial:{task_port}"),
                task_kind,
                move |_context| {
                    transport
                        .send_to(&task_port, bytes.clone())
                        .map_err(|error| tool_transport::translate_error(&error))?;
                    Ok(TaskResult::TransportSent {
                        port: PortId::new(task_port),
                        bytes: bytes.len(),
                    })
                },
            );
            return Ok(pending(task_id, format!("正在发送到 {port_name}")));
        }

        let backend = self.transport.clone();
        let port = PortId::new(port_name.clone());
        let byte_count = bytes.len();
        let task_id =
            self.tasks
                .spawn_ordered(format!("serial:{port_name}"), task_kind, move |_context| {
                    block_on_transport(TransportBackend::send(&backend, port.clone(), bytes))
                        .map_err(|error| tool_platform::translate_error(&error))?;
                    Ok(TaskResult::TransportSent {
                        port,
                        bytes: byte_count,
                    })
                });
        Ok(pending(task_id, format!("正在发送到 {port_name}")))
    }

    fn set_transport_signal(
        &mut self,
        port_name: String,
        value: bool,
        dtr: bool,
    ) -> Result<CommandOutcome, AppError> {
        if self.is_network_port(&port_name) {
            return Err(AppError::Transport("网络端口不支持 DTR/RTS".to_owned()));
        }
        let backend = self.transport.clone();
        let port = PortId::new(port_name.clone());
        let task_port = port.clone();
        let task_id = self.tasks.spawn_ordered(
            format!("serial:{port_name}"),
            if dtr {
                TaskKind::SetDtr
            } else {
                TaskKind::SetRts
            },
            move |_context| {
                let result = if dtr {
                    block_on_transport(TransportBackend::set_dtr(&backend, port, value))
                } else {
                    block_on_transport(TransportBackend::set_rts(&backend, port, value))
                };
                result.map_err(|error| tool_platform::translate_error(&error))?;
                Ok(TaskResult::TransportSignalChanged { port: task_port })
            },
        );
        Ok(pending(
            task_id,
            format!("正在设置 {}", if dtr { "DTR" } else { "RTS" }),
        ))
    }

    fn start_reconnect(&mut self, port_name: String) -> Result<CommandOutcome, AppError> {
        let registered = self.find_network_port(&port_name).cloned();
        // 注册过的端口一律换成规范名（理由见 `connect`）：这里的队列键与 `reconnect_tasks`
        // 的键必须同源，否则同一端口两种形态点名会各起一个重连循环。
        let port_name = match &registered {
            Some(config) => config.display_name(),
            None => port_name,
        };
        // 登记前先取消该端口上已挂着的那个循环，规则与理由见 `cancel_deregistered_task`。
        let previous = self.reconnect_tasks.remove(&port_name);
        cancel_deregistered_task(previous, |task_id| {
            self.tasks.cancel(task_id);
        });

        let network = registered;
        let backend = self.transport.clone();
        let serial_settings = network.is_none().then(|| self.platform_serial_settings());
        let task_port = port_name.clone();
        let task_id = self.tasks.spawn_ordered(
            format!("serial:{port_name}"),
            if network.is_some() {
                TaskKind::ReconnectNetwork
            } else {
                TaskKind::ReconnectSerial
            },
            move |context: TaskContext| {
                let port = PortId::new(task_port.clone());
                // 一次尝试，不重试。原先这里是一段「3 秒预算 + 10ms 轮询」的循环，判据是
                // `Io(WouldBlock)` —— 而这条通道上**没有任何生产者**：串口的 kind 在
                // `platform/src/native.rs::operation_error` 就被折成字符串，
                // `open_network_serial` 在连接失败时也照旧返回 `Ok`。循环因此恒等于"一次 open"，
                // 只是把失败结果压在手里最多 3 秒。真要重试的是**自动重连**那一套
                // （`crates/app/src/reconnect.rs`：真退避 + 上限 + 可取消）；手动重连失败就立刻
                // 把中文原因交给用户，不在这里假装等待。
                context
                    .check_cancelled()
                    .map_err(|_| "重连已取消".to_owned())?;

                let Some(network) = network else {
                    block_on_transport(backend.disconnect(port.clone()))
                        .and_then(|()| {
                            block_on_transport(
                                backend.connect(port.clone(), serial_settings.unwrap_or_default()),
                            )
                        })
                        .map_err(|error| tool_platform::translate_error(&error))?;
                    return Ok(TaskResult::TransportConnected { port });
                };
                let transport = backend.clone();
                transport
                    .open_network_serial(network)
                    .map(|_| TaskResult::Reconnected {
                        port_name: task_port,
                    })
                    .map_err(|error| tool_transport::translate_error(&error))
            },
        );
        self.reconnect_tasks.insert(port_name.clone(), task_id);
        Ok(pending(task_id, format!("正在重连 {port_name}")))
    }

    fn poll_tasks(&mut self) {
        for event in self.tasks.drain_events() {
            match event {
                AppEvent::TaskStateChanged { .. } => {}
                AppEvent::TaskCompleted { id, result } => {
                    self.remove_reconnect_task(id);
                    match result {
                        TaskResult::PortsRefreshed(ports) => self.apply_ports(ports),
                        TaskResult::PlatformPortsRefreshed(ports) => {
                            self.apply_platform_ports(ports)
                        }
                        TaskResult::Connected { port_name }
                        | TaskResult::Reconnected { port_name } => {
                            self.selected_port = Some(port_name.clone());
                            self.app_config.selected_port = Some(port_name);
                        }
                        TaskResult::Disconnected { port_name } => {
                            if self.selected_port.as_deref() == Some(port_name.as_str()) {
                                self.selected_port = None;
                            }
                        }
                        TaskResult::TransportConnected { port } => {
                            let port_name = port.to_string();
                            self.selected_port = Some(port_name.clone());
                            self.app_config.selected_port = Some(port_name);
                        }
                        TaskResult::TransportDisconnected { port } => {
                            if self.selected_port.as_deref() == Some(port.as_str()) {
                                self.selected_port = None;
                            }
                        }
                        TaskResult::TransportSent { .. }
                        | TaskResult::TransportSignalChanged { .. } => {}
                        TaskResult::ReplayLoaded(data) => {
                            self.replay.load_prepared(data);
                        }
                        TaskResult::PluginsDiscovered(scan) => {
                            self.plugin_manager.apply_scan(scan);
                        }
                        TaskResult::FileExported { file } => {
                            self.bus.publish(tool_core::Event::system_log(
                                LogLevel::Info,
                                "app",
                                format!("已导出 {}", file.name()),
                            ));
                        }
                    }
                }
                AppEvent::TaskFailed { id, error } => {
                    self.remove_reconnect_task(id);
                    self.bus.publish(tool_core::Event::system_log(
                        LogLevel::Error,
                        "app.task",
                        format!("后台任务失败: {error}"),
                    ));
                }
                AppEvent::TaskCancelled { id } => {
                    self.remove_reconnect_task(id);
                }
            }
        }
    }

    fn remove_reconnect_task(&mut self, task_id: TaskId) {
        self.reconnect_tasks.retain(|_, id| *id != task_id);
    }
}

impl crate::command::CommandEffects for Workbench {
    fn refresh_ports(&mut self) -> Result<CommandOutcome, crate::command::CommandError> {
        Ok(self.spawn_refresh_ports())
    }

    fn request_port(&mut self) -> Result<CommandOutcome, crate::command::CommandError> {
        Ok(self.spawn_request_port())
    }

    fn register_network_port(
        &mut self,
        config: NetworkPortConfig,
    ) -> Result<(), crate::command::CommandError> {
        Workbench::register_network_port(self, config);
        Ok(())
    }

    fn remove_network_port(&mut self, port: PortId) -> Result<(), crate::command::CommandError> {
        Workbench::remove_network_port(self, port);
        Ok(())
    }

    fn connect(
        &mut self,
        port: PortId,
        settings: SerialSettings,
    ) -> Result<CommandOutcome, crate::command::CommandError> {
        Workbench::connect(self, port.as_str(), settings).map_err(Into::into)
    }

    fn apply_serial_settings(&mut self, settings: SerialSettings) {
        Workbench::apply_serial_settings(self, settings);
    }

    fn disconnect(&mut self, port: PortId) -> Result<CommandOutcome, crate::command::CommandError> {
        Ok(self.disconnect_port(port.to_string()))
    }

    fn reconnect(&mut self, port: PortId) -> Result<CommandOutcome, crate::command::CommandError> {
        self.start_reconnect(port.to_string()).map_err(Into::into)
    }

    fn cancel_reconnect(
        &mut self,
        port: PortId,
    ) -> Result<CommandOutcome, crate::command::CommandError> {
        Workbench::cancel_reconnect(self, port.to_string());
        Ok(CommandOutcome::Done)
    }

    fn cancel_task(&mut self, task_id: TaskId) -> bool {
        self.tasks.cancel(task_id)
    }

    fn is_network_port(&self, port: &PortId) -> bool {
        Workbench::is_network_port(self, port.as_str())
    }

    fn send_planned(
        &mut self,
        port: PortId,
        plan: PlannedSend,
    ) -> Result<CommandOutcome, crate::command::CommandError> {
        self.send_transport_bytes(port.to_string(), plan)
            .map_err(Into::into)
    }

    fn set_signal(
        &mut self,
        port: PortId,
        value: bool,
        dtr: bool,
    ) -> Result<CommandOutcome, crate::command::CommandError> {
        self.set_transport_signal(port.to_string(), value, dtr)
            .map_err(Into::into)
    }

    fn start_recording(
        &mut self,
        file: FileHandle,
        mode: crate::recording::RecordModeView,
    ) -> Result<CommandOutcome, crate::command::CommandError> {
        self.dispatch_start_recording(file, mode)
            .map_err(Into::into)
    }

    fn set_recording_mode(&mut self, mode: crate::recording::RecordModeView) {
        self.recorder.set_mode(mode.into());
    }

    fn stop_recording(&mut self) -> Result<CommandOutcome, crate::command::CommandError> {
        self.recorder.stop();
        Ok(CommandOutcome::Done)
    }

    fn pause_recording(&mut self) -> Result<CommandOutcome, crate::command::CommandError> {
        self.recorder.pause();
        Ok(CommandOutcome::Done)
    }

    fn resume_recording(&mut self) -> Result<CommandOutcome, crate::command::CommandError> {
        self.recorder.resume();
        Ok(CommandOutcome::Done)
    }

    fn add_recording_bookmark(&mut self, name: Option<String>) {
        self.recorder.add_bookmark(&name.unwrap_or_default());
    }

    fn load_replay(
        &mut self,
        file: FileHandle,
    ) -> Result<CommandOutcome, crate::command::CommandError> {
        self.dispatch_load_replay(file).map_err(Into::into)
    }

    fn replay_play(&mut self) -> Result<(), crate::command::CommandError> {
        let _ = self.replay.play();
        Ok(())
    }

    fn replay_pause(&mut self) -> Result<(), crate::command::CommandError> {
        self.replay.pause();
        Ok(())
    }

    fn replay_stop(&mut self) -> Result<(), crate::command::CommandError> {
        self.replay.stop();
        Ok(())
    }

    fn replay_seek(&mut self, position_ms: u64) -> Result<(), crate::command::CommandError> {
        self.replay.seek_ms(position_ms);
        Ok(())
    }

    fn replay_seek_by(&mut self, delta_ms: i64) -> Result<(), crate::command::CommandError> {
        Workbench::replay_seek_by(self, delta_ms);
        Ok(())
    }

    fn replay_step(&mut self, delta: i32) -> Result<(), crate::command::CommandError> {
        Workbench::replay_step(self, delta);
        Ok(())
    }

    fn replay_set_speed(&mut self, speed: f64) {
        self.replay.set_speed(speed);
    }

    fn replay_set_policy(
        &mut self,
        policy: crate::replay::ReplayPolicyView,
    ) -> Result<(), crate::command::CommandError> {
        self.replay.set_policy(policy.into());
        Ok(())
    }

    fn replay_add_bookmark(&mut self, name: Option<String>) {
        self.replay.add_bookmark(name);
    }

    fn replay_remove_bookmark(&mut self, position_ms: u64) {
        self.replay.remove_bookmark(position_ms);
    }

    fn enable_plugin(&mut self, plugin_id: String) -> Result<(), crate::command::CommandError> {
        self.plugin_manager
            .enable(&plugin_id)
            .map_err(|e| crate::command::CommandError::Plugin(tool_extension::translate_error(&e)))
    }

    fn disable_plugin(&mut self, plugin_id: String) -> Result<(), crate::command::CommandError> {
        self.plugin_manager
            .disable(&plugin_id)
            .map_err(|e| crate::command::CommandError::Plugin(tool_extension::translate_error(&e)))
    }

    fn reload_plugins(&mut self) -> Result<CommandOutcome, crate::command::CommandError> {
        Ok(self.spawn_reload_plugins())
    }

    fn discover_plugins(
        &mut self,
        roots: Vec<std::path::PathBuf>,
    ) -> Result<CommandOutcome, crate::command::CommandError> {
        Ok(self.spawn_discover_plugins(roots))
    }

    fn execute_plugin_command(
        &mut self,
        plugin_id: String,
        command_id: String,
        context: serde_json::Value,
    ) -> Result<(), crate::command::CommandError> {
        self.publish_plugin_command(plugin_id, command_id, context);
        Ok(())
    }

    fn set_terminal_merge_window(&mut self, ms: u64) {
        self.app_config.terminal_merge_window_ms = ms;
    }

    fn set_terminal_max_entries(&mut self, max: usize) {
        self.app_config.terminal_max_entries = max;
    }
}

impl crate::AppRuntime for Workbench {
    fn capabilities(&self) -> crate::AppCapabilities {
        crate::AppCapabilities::native()
    }

    fn query_transport(&self) -> TransportView {
        Workbench::query_transport(self)
    }

    fn query_recording(&self) -> RecordingStatusView {
        Workbench::query_recording(self)
    }

    fn query_replay(&self) -> ReplayStatusView {
        Workbench::query_replay(self)
    }

    fn query_plugins(&self) -> PluginView {
        Workbench::query_plugins(self)
    }

    fn task_snapshots(&self) -> Vec<TaskSnapshot> {
        Workbench::task_snapshots(self)
    }

    fn tick(&mut self) {
        self.tick(0.0);
    }

    fn dispatch(&mut self, command: crate::AppCommand) -> Result<crate::CommandOutcome, String> {
        Workbench::dispatch(self, command).map_err(|error| error.to_string())
    }
}

/// 导出内容落盘的唯一一处：CSV 前置 UTF-8 BOM（Excel 兼容），其余格式原样写出。
///
/// 调用方只剩 `spawn_file_export`：presentation 自己准备好内容交进来，BOM 与文件
/// 服务的接触都留在这里，也就是 worker 闭包内，不会在 egui 回调里写大文件。
/// 失败文本在这里就翻译成中文：返回的 `String` 会被 worker 闭包原样抛给任务模型，
/// 也就是 `TaskSnapshot::message`（状态栏就是它），所以不能再是 `StorageError` 的
/// 英文 `Display`。
fn write_exported_content(
    file_service: &NativeFileService,
    path: &std::path::Path,
    file: &FileHandle,
    format: &str,
    content: String,
) -> Result<(), String> {
    let mut bytes = content.into_bytes();
    if format == "csv" {
        let mut csv_bytes = b"\xEF\xBB\xBF".to_vec();
        csv_bytes.append(&mut bytes);
        bytes = csv_bytes;
    }
    file_service
        .block_on_write_path(
            path.to_path_buf(),
            FileBlob {
                name: file.name().to_owned(),
                mime: "application/octet-stream".to_owned(),
                bytes,
            },
        )
        .map_err(|error| tool_platform::storage::translate_error(&error))
}

fn pending(task_id: TaskId, message: impl Into<String>) -> CommandOutcome {
    CommandOutcome::Pending {
        task_id,
        message: message.into(),
    }
}

/// 「这个名字指的是不是这份已注册的网络端口配置」—— 全平台唯一的这一判定。
///
/// 两种形态都认：`display_name()`（`host:port`，本平台注册与 `workspace.json` 用的形态，
/// 也是 `TransportManager` 端口表的键）与 `port_id()`（`network://host:port`，wasm 侧网络行
/// 的键形态，见 `crates/app/src/web/serial.rs`）。写成一条而不是各站点抄一份，是因为它
/// 分叉过：`connect` / `start_reconnect` 只认前者，于是同一个 id 点名时连接、重连、关闭整个
/// 走错分支，而发送虽然被路由到网络分支，也会因端口表（以 `display_name()` 为键）查不到
/// 这个名字而拿到 `PortNotOpen` —— 两种形态其实谁都不好用，只是各处坏法不同；而
/// `RemoveNetworkPort` 删得掉的端口，换个名字就又当成串口。
///
/// 串口名不可能误命中：`port_id()` 恒以 `network://` 开头，而真实串口名是 `COMx` /
/// `/dev/tty*`；由 `headless.rs` 的 `network_port_ids_are_recognized_in_both_naming_forms`
/// 双向钉住（网络两形态 → `send_network`，普通串口名 → `send_serial`），
/// `network_ports_connect_and_reconnect_in_both_naming_forms` 钉住 connect / reconnect
/// 两条分支同样两形态都认。
fn network_config_matches(config: &NetworkPortConfig, port_name: &str) -> bool {
    config.display_name() == port_name
        || tool_platform::network_port_id(config).to_string() == port_name
}
