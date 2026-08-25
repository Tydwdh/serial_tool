use std::collections::HashMap;
use std::path::PathBuf;

use tool_core::LogLevel;
use tool_databus::{DataBus, TopicFilter};
use tool_extension::PluginManager;
use tool_lua_host::{DialogRequest, FileAccessBroker};
use tool_recorder::{JsonlRecorder, ReplayManager};
use tool_transport::{SerialConfig, SerialPortDescriptor, TransportManager};

use crate::command::{AppCommand, CommandOutcome};
use crate::error::AppError;
use crate::query::{
    NetworkPortConfig, PluginView, RecordModeView, RecordingStatusView, ReplayStatusView,
    TransportStatusView, TransportView,
};
use crate::service::terminal::TerminalService;
use crate::task::{AppEvent, TaskContext, TaskId, TaskManager, TaskResult, TaskSnapshot};

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
    recorder: JsonlRecorder,
    replay: ReplayManager,
    plugin_manager: PluginManager,
    terminal: TerminalService,
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
}

impl TransportEndpoint {
    pub fn send(
        &self,
        port_name: &str,
        input: &str,
        hex_mode: bool,
        line_ending: &str,
        hex_strict: bool,
    ) -> Result<(), String> {
        tool_transport::send_impl_to(
            port_name,
            input,
            hex_mode,
            line_ending,
            hex_strict,
            &self.transport,
        )
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
        let terminal = TerminalService::new(bus.clone());
        Self {
            bus,
            transport,
            recorder,
            replay,
            plugin_manager: pm,
            terminal,
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
        self.terminal
            .set_merge_window_ms(config.terminal_merge_window_ms);
        self.terminal.set_max_entries(config.terminal_max_entries);
        self.selected_port = config.selected_port.clone();
        self.app_config = config;
        self
    }

    pub fn app_config(&self) -> &ApplicationConfig {
        &self.app_config
    }

    pub fn set_network_ports(&mut self, ports: Vec<NetworkPortConfig>) {
        self.app_config.network_ports = ports;
    }

    pub fn set_serial_parameters(
        &mut self,
        baud_rate: String,
        data_bits: String,
        stop_bits: String,
        parity: String,
    ) {
        self.app_config.baud_rate = baud_rate;
        self.app_config.data_bits = data_bits;
        self.app_config.stop_bits = stop_bits;
        self.app_config.parity = parity;
    }

    pub fn tick(&mut self, _now_secs: f64) {
        self.poll_tasks();
        self.terminal.ingest_pending();
        self.recorder.tick_backpressure();
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
        path: PathBuf,
        render: F,
    ) -> CommandOutcome
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        let task_id = self.tasks.spawn(kind, move |_context| {
            let content = render()?;
            if format == "csv" {
                write_utf8_csv(&path, &content).map_err(|error| error.to_string())?;
            } else {
                std::fs::write(&path, content).map_err(|error| error.to_string())?;
            }
            Ok(TaskResult::FileExported { path })
        });
        pending(task_id, "正在导出文件")
    }

    pub fn dispatch(&mut self, command: AppCommand) -> Result<CommandOutcome, AppError> {
        match command {
            AppCommand::RefreshPorts => {
                if let Some(task_id) = self.tasks.active_task_id("refresh_ports") {
                    return Ok(pending(task_id, "正在刷新串口列表"));
                }
                let transport = self.transport.clone();
                let task_id = self.tasks.spawn("refresh_ports", move |_context| {
                    transport
                        .list_serial_ports()
                        .map(TaskResult::PortsRefreshed)
                        .map_err(|error| error.to_string())
                });
                Ok(pending(task_id, "正在刷新串口列表"))
            }
            AppCommand::Connect { port_name } => self.connect(&port_name),
            AppCommand::Disconnect { port_name } => {
                let transport = self.transport.clone();
                let task_port = port_name.clone();
                let task_id = self.tasks.spawn("disconnect", move |_context| {
                    transport.close_port(&task_port);
                    Ok(TaskResult::Disconnected {
                        port_name: task_port,
                    })
                });
                Ok(pending(task_id, format!("正在关闭 {port_name}")))
            }
            AppCommand::Reconnect { port_name } => self.start_reconnect(port_name),
            AppCommand::CancelReconnect { port_name } => {
                if let Some(task_id) = self.reconnect_tasks.remove(&port_name) {
                    self.tasks.cancel(task_id);
                }
                // 网络连接中也通过统一任务模型关闭，避免 CancelReconnect 在 UI 线程
                // 直接触碰 transport 生命周期。
                if self.transport.status_port(&port_name).connecting {
                    let transport = self.transport.clone();
                    let task_port = port_name.clone();
                    self.tasks.spawn("cancel_reconnect", move |_context| {
                        transport.close_port(&task_port);
                        Ok(TaskResult::Disconnected {
                            port_name: task_port,
                        })
                    });
                }
                Ok(CommandOutcome::Done)
            }
            AppCommand::CancelTask { task_id } => {
                if !self.tasks.cancel(task_id) {
                    return Err(AppError::InvalidState(format!(
                        "后台任务 {:?} 不存在或已经结束",
                        task_id
                    )));
                }
                Ok(CommandOutcome::Done)
            }
            AppCommand::SendText { port_name, text } => self
                .transport
                .send_text_to(&port_name, &text)
                .map(|()| CommandOutcome::Done)
                .map_err(|e| AppError::Transport(e.to_string())),
            AppCommand::SendHex { port_name, hex } => self
                .transport
                .send_hex_to(&port_name, &hex)
                .map(|()| CommandOutcome::Done)
                .map_err(|e| AppError::Transport(e.to_string())),
            AppCommand::SendRaw { port_name, bytes } => self
                .transport
                .send_to(&port_name, bytes)
                .map(|()| CommandOutcome::Done)
                .map_err(|e| AppError::Transport(e.to_string())),
            AppCommand::SetDtr { port_name, value } => self
                .transport
                .set_dtr(&port_name, value)
                .map(|()| CommandOutcome::Done)
                .map_err(|e| AppError::Transport(e.to_string())),
            AppCommand::SetRts { port_name, value } => self
                .transport
                .set_rts(&port_name, value)
                .map(|()| CommandOutcome::Done)
                .map_err(|e| AppError::Transport(e.to_string())),
            AppCommand::StartRecording { path } => self
                .recorder
                .start(&path)
                .map(|()| CommandOutcome::Done)
                .map_err(|e| AppError::Recording(e.to_string())),
            AppCommand::StopRecording => {
                self.recorder.stop();
                Ok(CommandOutcome::Done)
            }
            AppCommand::PauseRecording => {
                self.recorder.pause();
                Ok(CommandOutcome::Done)
            }
            AppCommand::ResumeRecording => {
                self.recorder.resume();
                Ok(CommandOutcome::Done)
            }
            AppCommand::AddBookmark { name } => {
                let n = name.unwrap_or_default();
                self.recorder.add_bookmark(&n);
                Ok(CommandOutcome::Done)
            }
            AppCommand::LoadReplay { path } => {
                let task_id = self.tasks.spawn("load_replay", move |_context| {
                    ReplayManager::prepare_load(&path)
                        .map(TaskResult::ReplayLoaded)
                        .map_err(|error| error.to_string())
                });
                Ok(pending(task_id, "正在加载回放文件"))
            }
            AppCommand::ReplayPlay => {
                let _ = self.replay.play();
                Ok(CommandOutcome::Done)
            }
            AppCommand::ReplayPause => {
                self.replay.pause();
                Ok(CommandOutcome::Done)
            }
            AppCommand::ReplayStop => {
                self.replay.stop();
                Ok(CommandOutcome::Done)
            }
            AppCommand::ReplaySeek { position_ms } => {
                self.replay.seek_ms(position_ms);
                Ok(CommandOutcome::Done)
            }
            AppCommand::ReplaySeekBy { delta_ms } => {
                let cur = self.replay.status().position_ms;
                let next = if delta_ms < 0 {
                    cur.saturating_sub((-delta_ms) as u64)
                } else {
                    cur.saturating_add(delta_ms as u64)
                };
                self.replay.seek_ms(next);
                Ok(CommandOutcome::Done)
            }
            AppCommand::ReplayStep { delta } => {
                if delta > 0 {
                    for _ in 0..delta {
                        let _ = self.replay.step_forward();
                    }
                } else {
                    for _ in 0..(-delta) {
                        self.replay.step_backward();
                    }
                }
                Ok(CommandOutcome::Done)
            }
            AppCommand::SetReplaySpeed { speed } => {
                self.replay.set_speed(speed);
                Ok(CommandOutcome::Done)
            }
            AppCommand::SetReplayPolicy { policy } => {
                self.replay.set_policy(policy.into());
                Ok(CommandOutcome::Done)
            }
            AppCommand::EnablePlugin { plugin_id } => self
                .plugin_manager
                .enable(&plugin_id)
                .map(|()| CommandOutcome::Done)
                .map_err(|e| AppError::Plugin(e.to_string())),
            AppCommand::DisablePlugin { plugin_id } => self
                .plugin_manager
                .disable(&plugin_id)
                .map(|()| CommandOutcome::Done)
                .map_err(|e| AppError::Plugin(e.to_string())),
            AppCommand::ReloadPlugins => {
                let roots = self.plugin_manager.roots();
                let task_id = self.tasks.spawn("reload_plugins", move |_context| {
                    tool_extension::PluginManager::scan_roots(&roots)
                        .map(TaskResult::PluginsDiscovered)
                        .map_err(|error| error.to_string())
                });
                Ok(pending(task_id, "正在扫描插件"))
            }
            AppCommand::DiscoverPlugins { roots } => {
                let task_id = self.tasks.spawn("discover_plugins", move |_context| {
                    tool_extension::PluginManager::scan_roots(&roots)
                        .map(TaskResult::PluginsDiscovered)
                        .map_err(|error| error.to_string())
                });
                Ok(pending(task_id, "正在扫描插件"))
            }
            AppCommand::ExecutePluginCommand {
                plugin_id,
                command_id,
                context,
            } => {
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
                Ok(CommandOutcome::Done)
            }
            AppCommand::ClearTerminal => {
                self.terminal.clear();
                Ok(CommandOutcome::Done)
            }
            AppCommand::SetTerminalMergeWindow { ms } => {
                self.terminal.set_merge_window_ms(ms);
                self.app_config.terminal_merge_window_ms = ms;
                Ok(CommandOutcome::Done)
            }
            AppCommand::SetTerminalMaxEntries { max } => {
                self.terminal.set_max_entries(max);
                self.app_config.terminal_max_entries = max;
                Ok(CommandOutcome::Done)
            }
            AppCommand::ExportTerminal { format, path } => {
                let export_job = self.terminal.export_job();
                let task_path = path.clone();
                let task_format = format.clone();
                let task_id = self.tasks.spawn("export_terminal", move |_context| {
                    let content = export_job.render(&task_format);
                    if task_format == "csv" {
                        write_utf8_csv(&task_path, &content)
                    } else {
                        std::fs::write(&task_path, content)
                    }
                    .map_err(|error| error.to_string())?;
                    Ok(TaskResult::FileExported { path: task_path })
                });
                Ok(pending(task_id, "正在导出终端数据"))
            }
            AppCommand::ExportLog { format: _, path: _ } => {
                // 日志导出仍由 LogPanel 负责（Presentation），此处占位避免 UI 直接触及 recorder
                Ok(CommandOutcome::Done)
            }
        }
    }

    pub fn query_transport(&self) -> TransportView {
        TransportView {
            ports: self.ports.clone().into_iter().map(Into::into).collect(),
            open_ports: self.transport.open_ports(),
            statuses: self
                .transport
                .status_all()
                .into_iter()
                .map(Into::into)
                .collect(),
            config: crate::query::SerialConfigView {
                port_name: self.selected_port.clone().unwrap_or_default(),
                baud_rate: self.app_config.baud_rate.parse().unwrap_or(115_200),
                data_bits: self.app_config.data_bits.clone(),
                stop_bits: self.app_config.stop_bits.clone(),
                parity: self.app_config.parity.clone(),
            },
            auto_reconnect: self.app_config.auto_reconnect,
        }
    }

    pub fn query_recording(&self) -> RecordingStatusView {
        RecordingStatusView {
            stats: self.recorder.stats().into(),
            path: self.recorder.current_path().map(PathBuf::from),
            mode: self.recorder.mode().into(),
        }
    }

    pub fn query_replay(&self) -> ReplayStatusView {
        let status = self.replay.status();
        ReplayStatusView {
            state: status.state.into(),
            path: status.path,
            total_events: status.total_events,
            cursor: status.cursor,
            speed: status.speed,
            position_ms: status.position_ms,
            duration_ms: status.duration_ms,
            policy: status.policy.into(),
            effective_policy: status.effective_policy.into(),
            has_recorded_protocol: status.has_recorded_protocol,
            analyzer_cache_entries: status.analyzer_cache_entries,
            analyzer_error: status.analyzer_error,
            analyzer_warning: status.analyzer_warning,
            load_report: status.load_report.map(Into::into),
        }
    }

    pub fn query_plugins(&self) -> PluginView {
        PluginView {
            summaries: self.plugin_manager.summaries(),
            diagnostics: self.plugin_manager.diagnostics().to_vec(),
        }
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

    pub fn transport_status(&self, port: &str) -> TransportStatusView {
        self.transport.status_port(port).into()
    }

    pub fn set_dtr(&self, port: &str, value: bool) -> Result<(), AppError> {
        self.transport
            .set_dtr(port, value)
            .map_err(|error| AppError::Transport(error.to_string()))
    }

    pub fn set_rts(&self, port: &str, value: bool) -> Result<(), AppError> {
        self.transport
            .set_rts(port, value)
            .map_err(|error| AppError::Transport(error.to_string()))
    }

    pub fn send_input(
        &self,
        port: &str,
        input: &str,
        hex_mode: bool,
        line_ending: &str,
        hex_strict: bool,
    ) -> Result<(), AppError> {
        tool_transport::send_impl_to(
            port,
            input,
            hex_mode,
            line_ending,
            hex_strict,
            &self.transport,
        )
        .map_err(|error| AppError::Transport(tool_transport::translate_error(&error)))
    }

    pub fn recording_is_running(&self) -> bool {
        self.recorder.is_running()
    }

    pub fn recording_is_stopping(&self) -> bool {
        self.recorder.is_stopping()
    }

    pub fn recording_is_paused(&self) -> bool {
        self.recorder.is_paused()
    }

    pub fn recording_mode(&self) -> RecordModeView {
        self.recorder.mode().into()
    }

    pub fn set_recording_mode(&mut self, mode: RecordModeView) {
        self.recorder.set_mode(mode.into());
    }

    pub fn start_recording(&mut self, path: impl AsRef<std::path::Path>) -> Result<(), AppError> {
        self.recorder
            .start(path)
            .map_err(|error| AppError::Recording(error.to_string()))
    }

    pub fn stop_recording(&mut self) {
        self.recorder.stop();
    }

    pub fn pause_recording(&mut self) {
        self.recorder.pause();
    }

    pub fn resume_recording(&mut self) {
        self.recorder.resume();
    }

    pub fn recording_current_path(&self) -> Option<std::path::PathBuf> {
        self.recorder.current_path().map(|p| p.to_path_buf())
    }

    pub fn reap_recording_stop(&mut self) -> Option<Result<std::path::PathBuf, String>> {
        self.recorder.reap_stopping()
    }

    pub fn reap_recording_error(&mut self) -> Option<String> {
        self.recorder.reap_error()
    }

    pub fn log(&self, lv: LogLevel, msg: impl Into<String>) {
        self.bus
            .publish(tool_core::Event::system_log(lv, "app", msg.into()));
    }

    pub fn plugin_state(&self, plugin_id: &str) -> Option<tool_extension::PluginState> {
        self.plugin_manager.plugin_state(plugin_id)
    }

    pub fn plugin_ids(&self) -> Vec<String> {
        self.plugin_manager.plugin_ids()
    }

    pub fn enable_plugin(&mut self, plugin_id: &str) -> Result<(), AppError> {
        self.plugin_manager
            .enable(plugin_id)
            .map_err(|error| AppError::Plugin(error.to_string()))
    }

    pub fn disable_plugin(&mut self, plugin_id: &str) -> Result<(), AppError> {
        self.plugin_manager
            .disable(plugin_id)
            .map_err(|error| AppError::Plugin(error.to_string()))
    }

    pub fn process_plugin_lifecycle(&mut self) -> usize {
        self.plugin_manager.process_pending()
    }

    pub fn take_plugin_cleanup_requests(&mut self) -> Vec<String> {
        self.plugin_manager.take_cleanup_requests()
    }

    pub fn plugin_config_root(&self) -> std::path::PathBuf {
        self.plugin_manager.config_root().to_path_buf()
    }

    pub fn plugin_settings(
        &self,
    ) -> Vec<(String, String, Vec<tool_extension::manifest::PluginSetting>)> {
        self.plugin_manager.plugin_settings()
    }

    pub fn plugin_config_store(&self) -> std::sync::Arc<tool_lua_host::ConfigStore> {
        self.plugin_manager.config_store().clone()
    }

    pub fn replay_analyzer_entries(&self) -> Vec<tool_extension::manifest::ReplayAnalyzerEntry> {
        self.plugin_manager.replay_analyzer_entries()
    }

    pub fn try_dialog_request(&self) -> Option<DialogRequest> {
        self._dialog_receiver.try_recv().ok()
    }

    pub fn authorize_plugin_file(&self, plugin_id: &str, path: PathBuf) {
        self._file_broker.authorize(plugin_id, path);
    }

    pub fn clear_plugin_file_authorization(&self, plugin_id: &str) {
        self._file_broker.clear(plugin_id);
    }

    pub fn query_terminal_since(
        &self,
        since_seq: u64,
        limit: usize,
    ) -> crate::model::terminal::TerminalDelta {
        self.terminal.entries_since(since_seq, limit)
    }

    fn apply_ports(&mut self, available: Vec<SerialPortDescriptor>) {
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for p in available {
            seen.insert(p.port_name.clone());
            if !self.ports.iter().any(|x| x.port_name == p.port_name) {
                self.ports.push(p);
            }
        }
        self.ports.retain(|p| {
            p.port_type == tool_transport::PortType::Network || seen.contains(&p.port_name)
        });
        for net in &self.app_config.network_ports {
            let name = net.display_name();
            if !self.ports.iter().any(|p| p.port_name == name) {
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

    fn connect(&mut self, port_name: &str) -> Result<CommandOutcome, AppError> {
        if let Some(net) = self
            .app_config
            .network_ports
            .iter()
            .find(|n| n.display_name() == port_name)
            .cloned()
        {
            let transport = self.transport.clone();
            let net: tool_transport::NetworkSerialConfig = net.into();
            let task_port = port_name.to_owned();
            let task_id = self.tasks.spawn("connect_network", move |_context| {
                transport
                    .open_network_serial(net)
                    .map(|_| TaskResult::Connected {
                        port_name: task_port,
                    })
                    .map_err(|error| error.to_string())
            });
            Ok(pending(task_id, format!("正在连接 {port_name}")))
        } else {
            let transport = self.transport.clone();
            let task_port = port_name.to_owned();
            let cfg = self.serial_config(port_name);
            let task_id = self.tasks.spawn("connect_serial", move |_context| {
                transport
                    .open_serial(cfg)
                    .map(|()| TaskResult::Connected {
                        port_name: task_port,
                    })
                    .map_err(|error| error.to_string())
            });
            Ok(pending(task_id, format!("正在打开 {port_name}")))
        }
    }

    fn serial_config(&self, port_name: &str) -> SerialConfig {
        SerialConfig {
            port_name: port_name.to_owned(),
            baud_rate: self.app_config.baud_rate.parse().unwrap_or(115_200),
            data_bits: tool_transport::parse_data_bits(&self.app_config.data_bits),
            stop_bits: tool_transport::parse_stop_bits(&self.app_config.stop_bits),
            parity: tool_transport::parse_parity(&self.app_config.parity),
        }
    }

    fn start_reconnect(&mut self, port_name: String) -> Result<CommandOutcome, AppError> {
        if let Some(previous) = self.reconnect_tasks.remove(&port_name) {
            self.tasks.cancel(previous);
        }

        let transport = self.transport.clone();
        let network = self
            .app_config
            .network_ports
            .iter()
            .find(|config| config.display_name() == port_name)
            .cloned()
            .map(Into::into);
        let serial_config = (!network.is_some()).then(|| self.serial_config(&port_name));
        let task_port = port_name.clone();
        let task_id = self.tasks.spawn("reconnect", move |context: TaskContext| {
            transport.close_port(&task_port);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);

            loop {
                context
                    .check_cancelled()
                    .map_err(|_| "重连已取消".to_owned())?;
                let result = if let Some(network) = network.clone() {
                    transport.open_network_serial(network).map(|_| ())
                } else {
                    transport.open_serial(serial_config.clone().expect("serial config"))
                };

                match result {
                    Ok(()) => {
                        return Ok(TaskResult::Reconnected {
                            port_name: task_port,
                        });
                    }
                    Err(error) => {
                        let retryable = matches!(
                            &error,
                            tool_transport::TransportError::Io(io_error)
                                if io_error.kind() == std::io::ErrorKind::WouldBlock
                        );
                        if !retryable {
                            return Err(error.to_string());
                        }
                    }
                }

                if std::time::Instant::now() >= deadline {
                    return Err("重连超时（3 秒）".to_owned());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
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
                        TaskResult::ReplayLoaded(data) => {
                            self.replay.load_prepared(data);
                        }
                        TaskResult::PluginsDiscovered(scan) => {
                            self.plugin_manager.apply_scan(scan);
                        }
                        TaskResult::FileExported { path } => {
                            self.bus.publish(tool_core::Event::system_log(
                                LogLevel::Info,
                                "app",
                                format!("已导出 {}", path.display()),
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

fn pending(task_id: TaskId, message: impl Into<String>) -> CommandOutcome {
    CommandOutcome::Pending {
        task_id,
        message: message.into(),
    }
}

fn write_utf8_csv(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let file = std::fs::File::create(path)?;
    let mut w = std::io::BufWriter::new(file);
    w.write_all(b"\xEF\xBB\xBF")?;
    w.write_all(content.as_bytes())?;
    w.flush()
}
