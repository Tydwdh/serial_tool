use std::collections::HashMap;
use std::path::PathBuf;

use tool_core::LogLevel;
use tool_databus::{DataBus, TopicFilter};
use tool_extension::PluginManager;
use tool_lua_host::{DialogRequest, FileAccessBroker};
use tool_recorder::{JsonlRecorder, ReplayManager};
use tool_transport::{NetworkSerialConfig, SerialConfig, SerialPortDescriptor, TransportManager};

use crate::command::{AppCommand, CommandOutcome};
use crate::error::AppError;
use crate::query::{PluginView, RecordingStatusView, ReplayStatusView, TransportView};
use crate::service::terminal::TerminalService;

#[derive(Debug, Clone)]
pub struct ApplicationConfig {
    pub selected_port: Option<String>,
    pub baud_rate: String,
    pub data_bits: String,
    pub stop_bits: String,
    pub parity: String,
    pub auto_reconnect: bool,
    pub terminal_merge_window_ms: u64,
    pub terminal_max_entries: usize,
    pub log_max_entries: usize,
    pub recorder_path: String,
    pub network_ports: Vec<NetworkSerialConfig>,
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

struct PendingReconnect {
    port_name: String,
    attempts: u32,
    next_try_at: f64,
}

pub struct Workbench {
    pub bus: DataBus,
    pub transport: TransportManager,
    pub recorder: JsonlRecorder,
    pub replay: ReplayManager,
    pub plugin_manager: PluginManager,
    pub terminal: TerminalService,
    app_config: ApplicationConfig,
    ports: Vec<SerialPortDescriptor>,
    selected_port: Option<String>,
    pending_reconnect: Option<PendingReconnect>,
    _file_broker: std::sync::Arc<FileAccessBroker>,
    _dialog_receiver: crossbeam_channel::Receiver<DialogRequest>,
    _file_browse_sub: tool_databus::Subscription,
    _contribution_sub: tool_databus::Subscription,
    _status_sub: tool_databus::Subscription,
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
        let file_browse_sub = bus.subscribe_lossy_bounded(
            TopicFilter::exact(tool_core::topics::UI_FORM_FILE_BROWSE),
            1024,
        );
        let contribution_sub = bus.subscribe_lossy_bounded(
            TopicFilter::exact(tool_core::topics::UI_CONTRIBUTION_SET_VALUE),
            1024,
        );
        let status_sub =
            bus.subscribe_lossy_bounded(TopicFilter::exact(tool_core::topics::UI_SET_STATUS), 1024);
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
            pending_reconnect: None,
            _file_broker: file_broker,
            _dialog_receiver: dialog_receiver,
            _file_browse_sub: file_browse_sub,
            _contribution_sub: contribution_sub,
            _status_sub: status_sub,
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

    pub fn tick(&mut self, now_secs: f64) {
        self.terminal.ingest_pending();
        self.tick_reconnect(now_secs);
    }

    pub fn dispatch(&mut self, command: AppCommand) -> Result<CommandOutcome, AppError> {
        match command {
            AppCommand::RefreshPorts => {
                self.refresh_ports();
                Ok(CommandOutcome::Done)
            }
            AppCommand::Connect { port_name } => self.connect(&port_name),
            AppCommand::Disconnect { port_name } => {
                self.transport.close_port(&port_name);
                Ok(CommandOutcome::Done)
            }
            AppCommand::Reconnect { port_name } => {
                // 阻塞式重连：先 close_port_blocking 再 open，语义与旧 commands.rs::reconnect_selected_port 一致
                self.transport
                    .close_port_blocking(&port_name, std::time::Duration::from_millis(3000))
                    .map_err(|e| AppError::Transport(e.to_string()))?;
                self.refresh_ports();
                if !self.ports.iter().any(|p| p.port_name == port_name) {
                    return Err(AppError::Transport(format!("串口 {port_name} 不存在")));
                }
                self.connect(&port_name)
            }
            AppCommand::CancelReconnect { port_name } => {
                if self
                    .pending_reconnect
                    .as_ref()
                    .is_some_and(|p| p.port_name == port_name)
                {
                    self.pending_reconnect = None;
                }
                // 网络连接中：直接 close_port
                if self.transport.status_port(&port_name).connecting {
                    self.transport.close_port(&port_name);
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
            AppCommand::LoadReplay { path } => self
                .replay
                .load(&path)
                .map(|_| CommandOutcome::Done)
                .map_err(|e| AppError::Replay(e.to_string())),
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
                self.replay.set_policy(policy);
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
            AppCommand::ReloadPlugins => self
                .plugin_manager
                .refresh()
                .map(|_| CommandOutcome::Done)
                .map_err(|e| AppError::Plugin(e.to_string())),
            AppCommand::DiscoverPlugins { roots } => self
                .plugin_manager
                .discover_roots(roots)
                .map(|_| CommandOutcome::Done)
                .map_err(|e| AppError::Plugin(e.to_string())),
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
                // 导出由 TerminalService 生成字符串，Workbench 负责落盘（无 rfd）
                let content = match format.as_str() {
                    "csv" => self.terminal.export_csv(),
                    "json" => self.terminal.export_json(),
                    _ => self.terminal.export_text(),
                };
                if format == "csv" {
                    write_utf8_csv(&path, &content)
                        .map(|()| CommandOutcome::Done)
                        .map_err(|e| AppError::InvalidState(e.to_string()))
                } else {
                    std::fs::write(&path, content)
                        .map(|()| CommandOutcome::Done)
                        .map_err(|e| AppError::InvalidState(e.to_string()))
                }
            }
            AppCommand::ExportLog { format: _, path: _ } => {
                // 日志导出仍由 LogPanel 负责（Presentation），此处占位避免 UI 直接触及 recorder
                Ok(CommandOutcome::Done)
            }
        }
    }

    pub fn query_transport(&self) -> TransportView {
        TransportView {
            ports: self.ports.clone(),
            open_ports: self.transport.open_ports(),
            statuses: self.transport.status_all(),
            config: SerialConfig {
                port_name: self.selected_port.clone().unwrap_or_default(),
                baud_rate: self.app_config.baud_rate.parse().unwrap_or(115_200),
                data_bits: tool_transport::parse_data_bits(&self.app_config.data_bits),
                stop_bits: tool_transport::parse_stop_bits(&self.app_config.stop_bits),
                parity: tool_transport::parse_parity(&self.app_config.parity),
            },
            auto_reconnect: self.app_config.auto_reconnect,
        }
    }

    pub fn query_recording(&self) -> RecordingStatusView {
        RecordingStatusView {
            stats: self.recorder.stats(),
            path: self.recorder.current_path().map(PathBuf::from),
            mode: self.recorder.mode(),
        }
    }

    pub fn query_replay(&self) -> ReplayStatusView {
        ReplayStatusView {
            status: Some(self.replay.status()),
        }
    }

    pub fn query_plugins(&self) -> PluginView {
        PluginView {
            summaries: self.plugin_manager.summaries(),
            diagnostics: self.plugin_manager.diagnostics().to_vec(),
        }
    }

    // ——— Delegation shims for WorkbenchApp migration ———
    pub fn bus(&self) -> &DataBus {
        &self.bus
    }
    pub fn bus_clone(&self) -> DataBus {
        self.bus.clone()
    }
    pub fn transport_clone(&self) -> TransportManager {
        self.transport.clone()
    }
    pub fn close_all_serial(&self) {
        self.transport.close_serial();
    }
    pub fn recorder_is_running(&self) -> bool {
        self.recorder.is_running()
    }
    pub fn recorder_is_stopping(&self) -> bool {
        self.recorder.is_stopping()
    }
    pub fn recorder_is_paused(&self) -> bool {
        self.recorder.is_paused()
    }
    pub fn recorder_mode(&self) -> tool_recorder::RecordMode {
        self.recorder.mode()
    }
    pub fn set_recorder_mode(&mut self, m: tool_recorder::RecordMode) {
        self.recorder.set_mode(m);
    }
    pub fn recorder_stats(&self) -> tool_recorder::RecorderStats {
        self.recorder.stats()
    }
    pub fn recorder_current_path(&self) -> Option<std::path::PathBuf> {
        self.recorder.current_path().map(|p| p.to_path_buf())
    }
    pub fn app_log(&self, lv: LogLevel, msg: impl Into<String>) {
        self.bus
            .publish(tool_core::Event::system_log(lv, "app", msg.into()));
    }
    pub fn plugin_summaries(&self) -> Vec<tool_extension::PluginSummary> {
        self.plugin_manager.summaries()
    }
    pub fn plugin_diagnostics(&self) -> &[tool_extension::PluginDiagnostic] {
        self.plugin_manager.diagnostics()
    }
    pub fn bus_ref(&self) -> &DataBus {
        &self.bus
    }
    pub fn file_broker(&self) -> std::sync::Arc<tool_lua_host::FileAccessBroker> {
        self._file_broker.clone()
    }
    pub fn dialog_receiver(&self) -> &crossbeam_channel::Receiver<tool_lua_host::DialogRequest> {
        &self._dialog_receiver
    }
    pub fn replay_manager(&self) -> &ReplayManager {
        &self.replay
    }
    pub fn replay_manager_mut(&mut self) -> &mut ReplayManager {
        &mut self.replay
    }
    pub fn open_ports(&self) -> Vec<String> {
        self.transport.open_ports()
    }
    pub fn status_port(&self, port: &str) -> tool_transport::TransportStatus {
        self.transport.status_port(port)
    }

    pub fn query_terminal_since(
        &self,
        since_seq: u64,
        limit: usize,
    ) -> crate::model::terminal::TerminalDelta {
        self.terminal.entries_since(since_seq, limit)
    }

    fn refresh_ports(&mut self) {
        let available = match self.transport.list_serial_ports() {
            Ok(v) => v,
            Err(_) => Vec::new(),
        };
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
            self.transport
                .open_network_serial(net)
                .map(|_| CommandOutcome::Done)
                .map_err(|e| AppError::Transport(e.to_string()))
        } else {
            let cfg = SerialConfig {
                port_name: port_name.to_owned(),
                baud_rate: self.app_config.baud_rate.parse().unwrap_or(115_200),
                data_bits: tool_transport::parse_data_bits(&self.app_config.data_bits),
                stop_bits: tool_transport::parse_stop_bits(&self.app_config.stop_bits),
                parity: tool_transport::parse_parity(&self.app_config.parity),
            };
            self.transport
                .open_serial(cfg)
                .map(|()| CommandOutcome::Done)
                .map_err(|e| AppError::Transport(e.to_string()))
        }
    }

    fn tick_reconnect(&mut self, now: f64) {
        let Some(mut pending) = self.pending_reconnect.take() else {
            return;
        };
        if now < pending.next_try_at {
            self.pending_reconnect = Some(pending);
            return;
        }
        if pending.attempts >= 10 {
            self.bus.publish(tool_core::Event::system_log(
                LogLevel::Warn,
                "transport.serial",
                format!("{} 重连已达上限", pending.port_name),
            ));
            return;
        }
        pending.attempts += 1;
        let backoff: u64 = (1u64 << pending.attempts.min(15)) * 100;
        pending.next_try_at = now + (backoff.min(30_000) as f64 / 1000.0);
        let cfg = SerialConfig {
            port_name: pending.port_name.clone(),
            baud_rate: self.app_config.baud_rate.parse().unwrap_or(115_200),
            data_bits: tool_transport::parse_data_bits(&self.app_config.data_bits),
            stop_bits: tool_transport::parse_stop_bits(&self.app_config.stop_bits),
            parity: tool_transport::parse_parity(&self.app_config.parity),
        };
        match self.transport.open_serial(cfg) {
            Ok(()) => {
                self.selected_port = Some(pending.port_name.clone());
            }
            Err(_) => {
                self.pending_reconnect = Some(pending);
            }
        }
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
