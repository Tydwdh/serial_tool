//! WorkbenchBackend — 硬件调试工作台后端核心。
//!
//! 封装所有非 UI 逻辑：串口管理、Lua 插件、录制回放、配置等。
//! 通过 FFI 供 Flutter 前端调用。

use crossbeam_channel::Receiver;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use tool_core::{Direction, Event, LogLevel, Payload, topics};
use tool_databus::DataBus;
use tool_extension::PluginManager;
use tool_lua_host::{DialogRequest, FileAccessBroker};
use tool_recorder::{JsonlRecorder, ReplayManager, ReplayPolicy};
use tool_transport::{
    DataBits, Parity, SerialConfig, SerialPortDescriptor, StopBits, TransportManager,
};

use crate::bridge::EventBridge;
use crate::event::BackendEvent;

/// 串口相关的状态。
pub struct SerialState {
    pub ports: Vec<SerialPortDescriptor>,
    pub selected_port: Option<String>,
    pub baud_rate: String,
    pub data_bits: String,
    pub stop_bits: String,
    pub parity: String,
    pub auto_reconnect: bool,
    pub port_aliases: HashMap<String, String>,
    pub port_groups: HashMap<String, Vec<String>>,
    pub port_profiles: HashMap<String, Value>,
}

impl Default for SerialState {
    fn default() -> Self {
        Self {
            ports: Vec::new(),
            selected_port: None,
            baud_rate: "115200".into(),
            data_bits: "8".into(),
            stop_bits: "1".into(),
            parity: "none".into(),
            auto_reconnect: false,
            port_aliases: HashMap::new(),
            port_groups: HashMap::new(),
            port_profiles: HashMap::new(),
        }
    }
}

/// 发送器状态。
pub struct SendState {
    pub history: VecDeque<String>,
    pub max_history: usize,
    pub line_ending: String,
    pub hex_mode: bool,
    pub strict_hex: bool,
    pub periodic_enabled: bool,
    pub periodic_interval_ms: u64,
}

impl Default for SendState {
    fn default() -> Self {
        Self {
            history: VecDeque::new(),
            max_history: 100,
            line_ending: "\n".into(),
            hex_mode: false,
            strict_hex: false,
            periodic_enabled: false,
            periodic_interval_ms: 1000,
        }
    }
}

/// 后端核心。
pub struct WorkbenchBackend {
    // 核心组件
    pub bus: DataBus,
    pub transport: TransportManager,
    pub plugin_manager: PluginManager,
    pub recorder: JsonlRecorder,
    pub replay: ReplayManager,
    pub event_bridge: EventBridge,

    // 状态
    pub serial: SerialState,
    pub send: SendState,
    /// Flutter Dock layout, stored as a forward-compatible JSON object.
    pub layout: Value,

    // 文件对话框
    pub dialog_receiver: Receiver<DialogRequest>,
    // Must stay alive while plugins are enabled, even though the backend does
    // not invoke it directly.
    pub _file_broker: Arc<FileAccessBroker>,

    // 应用目录
    pub app_dir: PathBuf,

    // 是否已销毁
    destroyed: bool,
}

impl WorkbenchBackend {
    /// 创建后端实例。
    pub fn new(app_dir: PathBuf) -> Self {
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let event_bridge = EventBridge::new(&bus);
        let (dialog_sender, dialog_receiver) = crossbeam_channel::unbounded::<DialogRequest>();
        let file_broker = Arc::new(FileAccessBroker::default());

        let mut plugin_manager = PluginManager::new(bus.clone(), transport.clone());
        plugin_manager.set_host_services(dialog_sender, file_broker.clone());

        // 发现插件
        let plugin_dir = app_dir.join("plugins");
        tool_marketplace::retire_old_plugin_dirs(&plugin_dir);
        if let Err(e) = plugin_manager.discover_roots([plugin_dir]) {
            bus.publish(Event::system_log(
                LogLevel::Error,
                "ext",
                format!("插件发现失败：{e}"),
            ));
        }

        let recorder = JsonlRecorder::new(bus.clone());
        let replay = ReplayManager::new(bus.clone());

        let mut backend = Self {
            bus,
            transport,
            plugin_manager,
            recorder,
            replay,
            event_bridge,
            serial: SerialState::default(),
            send: SendState::default(),
            layout: serde_json::json!({}),
            dialog_receiver,
            _file_broker: file_broker,
            app_dir,
            destroyed: false,
        };

        // Missing config is expected on first launch. A malformed existing
        // config must be visible to the user instead of silently resetting
        // their workspace to defaults.
        if backend.app_dir.join("config.json").exists()
            && let Err(error) = backend.load_config()
        {
            backend.event_bridge.push_event(BackendEvent::Notification {
                level: "warning".to_owned(),
                message: format!("配置未加载：{error}"),
            });
        }

        // 初始刷新端口
        backend.refresh_ports();

        backend.event_bridge.push_event(BackendEvent::Ready);
        backend
    }

    /// 销毁后端，关闭所有资源。
    pub fn destroy(&mut self) {
        if self.destroyed {
            return;
        }
        self.destroyed = true;
        self.transport.close_serial();
        // 禁用所有插件
        for id in self.plugin_manager.plugin_ids() {
            let _ = self.plugin_manager.disable(&id);
        }
    }

    /// 刷新串口列表。
    pub fn refresh_ports(&mut self) {
        match self.transport.list_serial_ports() {
            Ok(ports) => {
                self.serial.ports = ports.clone();
                self.event_bridge
                    .push_event(BackendEvent::PortList { ports });
            }
            Err(e) => {
                self.event_bridge.push_event(BackendEvent::Error {
                    message: format!("刷新端口失败: {e}"),
                });
            }
        }
    }

    // ── 串口操作 ──

    /// 打开选中串口。
    pub fn open_selected_port(&mut self) -> Result<(), String> {
        let port_name = self.serial.selected_port.as_ref().ok_or("未选择端口")?;
        let config = self.build_serial_config(port_name);
        self.transport
            .open_serial(config)
            .map_err(|e| e.to_string())?;
        self.event_bridge.push_event(BackendEvent::SerialOpen {
            port: port_name.clone(),
            success: true,
            error: None,
        });
        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "serial",
            format!("串口 {port_name} 已打开"),
        ));
        Ok(())
    }

    /// 打开指定串口。
    pub fn open_port(&mut self, port_name: &str) -> Result<(), String> {
        self.serial.selected_port = Some(port_name.to_owned());
        self.open_selected_port()
    }

    /// 关闭指定串口。
    pub fn close_port(&mut self, port_name: &str) {
        self.transport.close_port(port_name);
        self.event_bridge.push_event(BackendEvent::SerialClose {
            port: port_name.to_owned(),
        });
    }

    /// 发送数据到串口。
    pub fn send_data(&self, port: &str, input: &str, hex: bool) -> Result<(), String> {
        if hex {
            self.transport
                .send_hex_to(port, input)
                .map_err(|e| e.to_string())
        } else {
            self.transport
                .send_text_to(port, input)
                .map_err(|e| e.to_string())
        }
    }

    /// 构建当前串口配置。
    fn build_serial_config(&self, port_name: &str) -> SerialConfig {
        SerialConfig {
            port_name: port_name.to_owned(),
            baud_rate: self.serial.baud_rate.parse().unwrap_or(115200),
            data_bits: match self.serial.data_bits.as_str() {
                "5" => DataBits::Five,
                "6" => DataBits::Six,
                "7" => DataBits::Seven,
                _ => DataBits::Eight,
            },
            stop_bits: match self.serial.stop_bits.as_str() {
                "2" => StopBits::Two,
                _ => StopBits::One,
            },
            parity: match self.serial.parity.as_str() {
                "odd" => Parity::Odd,
                "even" => Parity::Even,
                _ => Parity::None,
            },
        }
    }

    // ── 录制 ──

    /// 开始/停止录制。
    pub fn toggle_recording(&mut self) {
        if self.recorder.is_running() {
            self.recorder.stop();
            self.event_bridge.push_event(BackendEvent::RecorderStatus {
                recording: false,
                stats: None,
            });
        } else {
            let path = self
                .app_dir
                .join("recordings")
                .join(format!("session-{}.jsonl", tool_core::now_timestamp_ms()));
            let path_str = path.to_string_lossy().to_string();
            match self.recorder.start(&path_str) {
                Ok(()) => {
                    self.event_bridge.push_event(BackendEvent::RecorderStatus {
                        recording: true,
                        stats: None,
                    });
                }
                Err(e) => {
                    self.event_bridge.push_event(BackendEvent::Error {
                        message: format!("开始录制失败: {e}"),
                    });
                }
            }
        }
    }

    // ── 命令处理 ──

    /// 处理来自前端的命令。
    pub fn handle_command(&mut self, cmd: &str, params: &Value) -> Result<Value, String> {
        match cmd {
            "refresh_ports" => {
                self.refresh_ports();
                Ok(serde_json::json!({"ok": true}))
            }
            "open_port" => {
                let port = params
                    .get("port")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 port 参数")?;
                self.open_port(port)
                    .map(|_| serde_json::json!({"ok": true}))
            }
            "close_port" => {
                let port = params
                    .get("port")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 port 参数")?;
                self.close_port(port);
                Ok(serde_json::json!({"ok": true}))
            }
            "send_data" => {
                let port = params
                    .get("port")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 port 参数")?;
                let data = params
                    .get("data")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 data 参数")?;
                let hex = params.get("hex").and_then(|v| v.as_bool()).unwrap_or(false);
                self.send_data(port, data, hex)
                    .map(|_| serde_json::json!({"ok": true}))
            }
            "set_baud_rate" => {
                let rate = params
                    .get("rate")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 rate 参数")?;
                self.serial.baud_rate = rate.to_owned();
                Ok(serde_json::json!({"ok": true}))
            }
            "set_serial_config" => {
                if let Some(rate) = params.get("baud_rate").and_then(|v| v.as_str()) {
                    rate.parse::<u32>().map_err(|_| "无效波特率")?;
                    self.serial.baud_rate = rate.to_owned();
                }
                if let Some(bits) = params.get("data_bits").and_then(|v| v.as_str()) {
                    if !matches!(bits, "5" | "6" | "7" | "8") {
                        return Err("无效数据位".to_owned());
                    }
                    self.serial.data_bits = bits.to_owned();
                }
                if let Some(bits) = params.get("stop_bits").and_then(|v| v.as_str()) {
                    if !matches!(bits, "1" | "2") {
                        return Err("无效停止位".to_owned());
                    }
                    self.serial.stop_bits = bits.to_owned();
                }
                if let Some(parity) = params.get("parity").and_then(|v| v.as_str()) {
                    if !matches!(parity, "none" | "odd" | "even") {
                        return Err("无效校验位".to_owned());
                    }
                    self.serial.parity = parity.to_owned();
                }
                if let Some(enabled) = params.get("auto_reconnect").and_then(|v| v.as_bool()) {
                    self.serial.auto_reconnect = enabled;
                }
                Ok(serde_json::json!({"ok": true}))
            }
            "set_send_config" => {
                if let Some(history) = params.get("history").and_then(Value::as_array) {
                    self.send.history = history
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|entry| !entry.is_empty())
                        .take(200)
                        .map(ToOwned::to_owned)
                        .collect();
                }
                if let Some(ending) = params.get("line_ending").and_then(Value::as_str) {
                    if !matches!(ending, "" | "\n" | "\r" | "\r\n") {
                        return Err("无效换行符".to_owned());
                    }
                    self.send.line_ending = ending.to_owned();
                }
                if let Some(hex_mode) = params.get("hex_mode").and_then(Value::as_bool) {
                    self.send.hex_mode = hex_mode;
                }
                if let Some(strict_hex) = params.get("strict_hex").and_then(Value::as_bool) {
                    self.send.strict_hex = strict_hex;
                }
                if let Some(periodic_enabled) =
                    params.get("periodic_enabled").and_then(Value::as_bool)
                {
                    self.send.periodic_enabled = periodic_enabled;
                }
                if let Some(interval) = params.get("periodic_interval_ms").and_then(Value::as_u64) {
                    if !(10..=3_600_000).contains(&interval) {
                        return Err("周期必须在 10–3600000 ms 之间".to_owned());
                    }
                    self.send.periodic_interval_ms = interval;
                }
                Ok(serde_json::json!({"ok": true}))
            }
            "set_selected_port" => {
                let port = params
                    .get("port")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 port 参数")?;
                self.serial.selected_port = Some(port.to_owned());
                Ok(serde_json::json!({"ok": true}))
            }
            "set_dtr" => {
                let port = params
                    .get("port")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 port 参数")?;
                let value = params
                    .get("value")
                    .and_then(|v| v.as_bool())
                    .ok_or("缺少 value 参数")?;
                self.set_dtr(port, value)
                    .map(|_| serde_json::json!({"ok": true}))
            }
            "set_rts" => {
                let port = params
                    .get("port")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 port 参数")?;
                let value = params
                    .get("value")
                    .and_then(|v| v.as_bool())
                    .ok_or("缺少 value 参数")?;
                self.set_rts(port, value)
                    .map(|_| serde_json::json!({"ok": true}))
            }
            "toggle_recording" => {
                self.toggle_recording();
                Ok(serde_json::json!({"ok": true}))
            }
            "replay_load" => {
                let path = params
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 path 参数")?;
                self.replay.load(path).map_err(|error| error.to_string())?;
                self.event_bridge.push_event(BackendEvent::ReplayStatus {
                    status: self.replay_status_json(),
                });
                Ok(serde_json::json!({"ok": true}))
            }
            "replay_play" => {
                if !self.replay.play() {
                    return Err("回放当前不可播放".to_owned());
                }
                self.event_bridge.push_event(BackendEvent::ReplayStatus {
                    status: self.replay_status_json(),
                });
                Ok(serde_json::json!({"ok": true}))
            }
            "replay_pause" => {
                self.replay.pause();
                self.event_bridge.push_event(BackendEvent::ReplayStatus {
                    status: self.replay_status_json(),
                });
                Ok(serde_json::json!({"ok": true}))
            }
            "replay_stop" => {
                self.replay.stop();
                self.event_bridge.push_event(BackendEvent::ReplayStatus {
                    status: self.replay_status_json(),
                });
                Ok(serde_json::json!({"ok": true}))
            }
            "replay_step_forward" => {
                self.replay.step_forward();
                self.event_bridge.push_event(BackendEvent::ReplayStatus {
                    status: self.replay_status_json(),
                });
                Ok(serde_json::json!({"ok": true}))
            }
            "replay_step_backward" => {
                self.replay.step_backward();
                self.event_bridge.push_event(BackendEvent::ReplayStatus {
                    status: self.replay_status_json(),
                });
                Ok(serde_json::json!({"ok": true}))
            }
            "replay_seek" => {
                let position_ms = params
                    .get("position_ms")
                    .and_then(|v| v.as_u64())
                    .ok_or("缺少 position_ms 参数")?;
                self.replay.seek_ms(position_ms);
                self.event_bridge.push_event(BackendEvent::ReplayStatus {
                    status: self.replay_status_json(),
                });
                Ok(serde_json::json!({"ok": true}))
            }
            "replay_set_speed" => {
                let speed = params
                    .get("speed")
                    .and_then(|v| v.as_f64())
                    .ok_or("缺少 speed 参数")?;
                self.replay.set_speed(speed);
                self.event_bridge.push_event(BackendEvent::ReplayStatus {
                    status: self.replay_status_json(),
                });
                Ok(serde_json::json!({"ok": true}))
            }
            "replay_set_policy" => {
                let policy = match params
                    .get("policy")
                    .and_then(Value::as_str)
                    .ok_or("缺少 policy 参数")?
                {
                    "auto" => ReplayPolicy::AutoPreferRecorded,
                    "exact" => ReplayPolicy::ExactRecorded,
                    "reparse" => ReplayPolicy::ReparseRaw,
                    _ => return Err("未知回放策略".to_owned()),
                };
                self.replay.set_policy(policy);
                self.event_bridge.push_event(BackendEvent::ReplayStatus {
                    status: self.replay_status_json(),
                });
                Ok(serde_json::json!({"ok": true}))
            }
            "replay_add_bookmark" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(ToOwned::to_owned);
                self.replay.add_bookmark(name);
                self.event_bridge.push_event(BackendEvent::ReplayStatus {
                    status: self.replay_status_json(),
                });
                Ok(serde_json::json!({"ok": true}))
            }
            "replay_remove_bookmark" => {
                let position_ms = params
                    .get("position_ms")
                    .and_then(Value::as_u64)
                    .ok_or("缺少 position_ms 参数")?;
                self.replay.remove_bookmark(position_ms);
                self.event_bridge.push_event(BackendEvent::ReplayStatus {
                    status: self.replay_status_json(),
                });
                Ok(serde_json::json!({"ok": true}))
            }
            "replay_list_files" => Ok(serde_json::json!({"files": self.replay_files()})),
            "replay_pick_file" => {
                let recordings = self.app_dir.join("recordings");
                let selected = rfd::FileDialog::new()
                    .set_title("选择录制文件")
                    .set_directory(recordings)
                    .add_filter("JSONL 录制文件", &["jsonl"])
                    .pick_file();
                Ok(serde_json::json!({
                    "path": selected.map(|path| path.to_string_lossy().to_string()),
                }))
            }
            "pick_terminal_export_path" => {
                let suggested_name = params
                    .get("suggested_name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .unwrap_or("terminal.log");
                let selected = rfd::FileDialog::new()
                    .set_title("导出终端记录")
                    .set_directory(self.app_dir.join("exports"))
                    .set_file_name(suggested_name)
                    .add_filter("日志文件", &["log", "txt"])
                    .save_file();
                Ok(serde_json::json!({
                    "path": selected.map(|path| path.to_string_lossy().to_string()),
                }))
            }
            "set_terminal_paused" => {
                let paused = params
                    .get("paused")
                    .and_then(|v| v.as_bool())
                    .ok_or("缺少 paused 参数")?;
                self.event_bridge.set_paused(paused);
                Ok(serde_json::json!({"ok": true}))
            }
            "set_layout" => {
                let layout = params
                    .get("layout")
                    .filter(|value| value.is_object())
                    .ok_or("layout 必须是对象")?
                    .clone();
                self.layout = layout;
                Ok(serde_json::json!({"ok": true}))
            }
            "dynamic_form_changed" => {
                let panel_id = params
                    .get("panel_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or("缺少 panel_id 参数")?;
                let values = params
                    .get("values")
                    .filter(|value| value.is_object())
                    .ok_or("values 必须是对象")?
                    .clone();
                self.bus.publish(Event::new(
                    topics::UI_FORM_CHANGED,
                    format!("flutter.panel:{panel_id}"),
                    Direction::Internal,
                    Payload::Json(serde_json::json!({
                        "panel_id": panel_id,
                        "values": values,
                    })),
                ));
                Ok(serde_json::json!({"ok": true}))
            }
            "dynamic_form_action" => {
                let panel_id = params
                    .get("panel_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or("缺少 panel_id 参数")?;
                let field_id = params
                    .get("field_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or("缺少 field_id 参数")?;
                let values = params
                    .get("values")
                    .filter(|value| value.is_object())
                    .ok_or("values 必须是对象")?
                    .clone();
                self.bus.publish(Event::new(
                    topics::UI_FORM_ACTION,
                    format!("flutter.panel:{panel_id}"),
                    Direction::Internal,
                    Payload::Json(serde_json::json!({
                        "panel_id": panel_id,
                        "field_id": field_id,
                        "kind": "button_clicked",
                        "action": params.get("action").cloned().unwrap_or(Value::Null),
                        "values": values,
                    })),
                ));
                Ok(serde_json::json!({"ok": true}))
            }
            "dynamic_form_pick_file" => {
                let plugin_id = params
                    .get("plugin_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let mut dialog = rfd::FileDialog::new().set_title(
                    params
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("选择文件"),
                );
                if let Some(filters) = params.get("filters").and_then(Value::as_array) {
                    for filter in filters {
                        let extensions = filter
                            .get("extensions")
                            .and_then(Value::as_array)
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .filter(|extension| *extension != "*")
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        if !extensions.is_empty() {
                            dialog = dialog.add_filter(
                                filter.get("name").and_then(Value::as_str).unwrap_or("文件"),
                                &extensions,
                            );
                        }
                    }
                }
                let selected = dialog.pick_file();
                if let Some(path) = &selected
                    && !plugin_id.is_empty()
                {
                    self._file_broker.authorize(plugin_id, path.clone());
                }
                Ok(serde_json::json!({
                    "path": selected.map(|path| path.to_string_lossy().to_string()),
                }))
            }
            "save_config" => self.save_config().map(|_| serde_json::json!({"ok": true})),
            "load_config" => self.load_config().map(|_| serde_json::json!({"ok": true})),
            "get_config" => Ok(self.get_config_json()),
            "enable_plugin" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 id 参数")?;
                self.plugin_manager.enable(id).map_err(|e| e.to_string())?;
                self.publish_plugin_list();
                Ok(serde_json::json!({"ok": true}))
            }
            "disable_plugin" => {
                let id = params
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("缺少 id 参数")?;
                self.plugin_manager.disable(id).map_err(|e| e.to_string())?;
                self.publish_plugin_list();
                Ok(serde_json::json!({"ok": true}))
            }
            "marketplace_fetch" => {
                let url = params
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or(tool_marketplace::DEFAULT_REGISTRY_URL);
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("创建网络运行时失败: {error}"))?;
                let registry = runtime.block_on(tool_marketplace::fetch_registry(url))?;
                serde_json::to_value(registry).map_err(|error| error.to_string())
            }
            "marketplace_install" => {
                let entry: tool_marketplace::RegistryPlugin = serde_json::from_value(
                    params.get("plugin").cloned().ok_or("缺少 plugin 参数")?,
                )
                .map_err(|error| format!("无效插件信息: {error}"))?;
                tool_marketplace::validate_plugin_id(&entry.id)?;
                let _ = self.plugin_manager.disable(&entry.id);
                let plugin_dir = self.app_dir.join("plugins");
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("创建网络运行时失败: {error}"))?;
                runtime.block_on(tool_marketplace::install_plugin(
                    &entry,
                    &plugin_dir,
                    |_, _| {},
                ))?;
                self.plugin_manager
                    .discover_roots([plugin_dir])
                    .map_err(|error| format!("重新扫描插件失败: {error}"))?;
                self.publish_plugin_list();
                Ok(serde_json::json!({"ok": true}))
            }
            "marketplace_uninstall" => {
                let id = params
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or("缺少 id 参数")?;
                tool_marketplace::validate_plugin_id(id)?;
                let _ = self.plugin_manager.disable(id);
                let target = self.app_dir.join("plugins").join(id);
                if target.exists() {
                    std::fs::remove_dir_all(&target)
                        .map_err(|error| format!("删除插件失败: {error}"))?;
                }
                self.plugin_manager
                    .discover_roots([self.app_dir.join("plugins")])
                    .map_err(|error| format!("重新扫描插件失败: {error}"))?;
                self.publish_plugin_list();
                Ok(serde_json::json!({"ok": true}))
            }
            _ => Err(format!("未知命令: {cmd}")),
        }
    }

    // ── 事件轮询 ──

    /// 处理待处理事件并返回。
    /// 事件来源：EventBridge（订阅 DataBus）+ 对话框请求。
    /// 不再重复订阅 DataBus——EventBridge 已覆盖所有主题。
    pub fn poll_events(&mut self, max_count: usize) -> Vec<BackendEvent> {
        let mut events = Vec::new();

        // 1. 从 EventBridge 取事件（已包含所有 DataBus 订阅）
        events.extend(self.event_bridge.poll(max_count));

        // 2. Advance playback after draining the previous batch. Events
        // published by replay are delivered on the next poll, keeping the FFI
        // callback bounded and deterministic.
        if self.replay.tick() > 0 {
            events.push(BackendEvent::ReplayStatus {
                status: self.replay_status_json(),
            });
        }

        // 3. 处理对话框请求
        self.drain_dialogs(&mut events);

        events
    }

    fn drain_dialogs(&self, _events: &mut Vec<BackendEvent>) {
        // Lua's ctx.dialog.open_file waits on its response channel. The old
        // Flutter bridge merely emitted a placeholder event, leaving the Lua
        // task blocked forever. Handle the native dialog here just like the
        // egui host does and authorize the selected path for that plugin.
        while let Ok(request) = self.dialog_receiver.try_recv() {
            let mut dialog = rfd::FileDialog::new().set_title(&request.title);
            for filter in &request.filters {
                if !filter.extensions.is_empty() && filter.extensions[0] != "*" {
                    dialog = dialog.add_filter(
                        &filter.name,
                        &filter
                            .extensions
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                    );
                }
            }
            let selected = dialog.pick_file();
            if let Some(path) = &selected {
                self._file_broker
                    .authorize(&request.plugin_id, path.clone());
            }
            let _ = request.response_sender.send(selected);
        }
    }

    // ── 查询方法 ──

    /// 获取串口列表（JSON）。
    pub fn get_ports_json(&self) -> Value {
        serde_json::to_value(&self.serial.ports).unwrap_or(Value::Null)
    }

    /// 获取插件状态（JSON）。
    pub fn get_plugins_json(&self) -> Value {
        serde_json::to_value(self.plugin_manager.summaries()).unwrap_or(Value::Array(Vec::new()))
    }

    fn publish_plugin_list(&self) {
        if let Value::Array(plugins) = self.get_plugins_json() {
            self.event_bridge
                .push_event(BackendEvent::PluginList { plugins });
        }
    }

    /// 获取后端状态（JSON）。
    pub fn get_status_json(&self) -> Value {
        let open_ports: Vec<String> = self
            .serial
            .ports
            .iter()
            .filter(|port| self.transport.status_port(&port.port_name).open)
            .map(|port| port.port_name.clone())
            .collect();
        serde_json::json!({
            "ports_count": self.serial.ports.len(),
            "open_ports": open_ports,
            "plugins_count": self.plugin_manager.count(),
            "recording": self.recorder.is_running(),
            "selected_port": self.serial.selected_port,
        })
    }

    pub fn replay_status_json(&self) -> Value {
        let status = self.replay.status();
        serde_json::json!({
            "state": format!("{:?}", status.state).to_lowercase(),
            "path": status.path.map(|path| path.to_string_lossy().to_string()),
            "total_events": status.total_events,
            "cursor": status.cursor,
            "speed": status.speed,
            "position_ms": status.position_ms,
            "duration_ms": status.duration_ms,
            "analyzer_error": status.analyzer_error,
            "analyzer_warning": status.analyzer_warning,
            "policy": replay_policy_name(status.policy),
            "effective_policy": replay_policy_name(status.effective_policy),
            "has_recorded_protocol": status.has_recorded_protocol,
            "analyzer_cache_entries": status.analyzer_cache_entries,
            "load_report": status.load_report.map(|report| serde_json::json!({
                "loaded": report.loaded,
                "skipped": report.skipped,
                "first_errors": report.first_errors,
            })),
            "bookmarks": self.replay.bookmarks().iter().map(|bookmark| serde_json::json!({
                "position_ms": bookmark.pos_ms,
                "name": bookmark.name,
            })).collect::<Vec<_>>(),
        })
    }

    fn replay_files(&self) -> Vec<Value> {
        let path = self.app_dir.join("recordings");
        let mut files = std::fs::read_dir(path)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let metadata = entry.metadata().ok()?;
                let path = entry.path();
                (metadata.is_file()
                    && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl")))
                    .then(|| serde_json::json!({
                        "path": path.to_string_lossy(),
                        "name": entry.file_name().to_string_lossy(),
                        "size": metadata.len(),
                        "modified_ms": metadata.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|time| time.as_millis()),
                    }))
            })
            .collect::<Vec<_>>();
        files.sort_by_key(|file| {
            std::cmp::Reverse(file.get("modified_ms").and_then(Value::as_u64).unwrap_or(0))
        });
        files.truncate(100);
        files
    }

    pub fn get_config_json(&self) -> Value {
        serde_json::json!({
            "serial": {
                "baud_rate": self.serial.baud_rate,
                "data_bits": self.serial.data_bits,
                "stop_bits": self.serial.stop_bits,
                "parity": self.serial.parity,
                "auto_reconnect": self.serial.auto_reconnect,
                "selected_port": self.serial.selected_port,
            },
            "layout": self.layout,
            "send": {
                "history": self.send.history,
                "line_ending": self.send.line_ending,
                "hex_mode": self.send.hex_mode,
                "strict_hex": self.send.strict_hex,
                "periodic_enabled": self.send.periodic_enabled,
                "periodic_interval_ms": self.send.periodic_interval_ms,
            },
        })
    }

    // ── 配置 ──

    /// 保存配置到文件。
    pub fn save_config(&self) -> Result<(), String> {
        let config = serde_json::json!({
            "serial": {
                "baud_rate": self.serial.baud_rate,
                "data_bits": self.serial.data_bits,
                "stop_bits": self.serial.stop_bits,
                "parity": self.serial.parity,
                "auto_reconnect": self.serial.auto_reconnect,
                "selected_port": self.serial.selected_port,
                "port_aliases": self.serial.port_aliases,
                "port_groups": self.serial.port_groups,
                "port_profiles": self.serial.port_profiles,
            },
            "send": {
                "history": self.send.history,
                "max_history": self.send.max_history,
                "line_ending": self.send.line_ending,
                "hex_mode": self.send.hex_mode,
                "strict_hex": self.send.strict_hex,
                "periodic_enabled": self.send.periodic_enabled,
                "periodic_interval_ms": self.send.periodic_interval_ms,
            },
            "layout": self.layout,
        });
        let path = self.app_dir.join("config.json");
        let content = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
        std::fs::write(&path, content).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 从文件加载配置。
    pub fn load_config(&mut self) -> Result<(), String> {
        let path = self.app_dir.join("config.json");
        let content = std::fs::read_to_string(&path).map_err(|e| format!("读取配置失败: {e}"))?;
        let config: Value =
            serde_json::from_str(&content).map_err(|e| format!("解析配置失败: {e}"))?;

        if let Some(serial) = config.get("serial") {
            if let Some(rate) = serial.get("baud_rate").and_then(|v| v.as_str()) {
                self.serial.baud_rate = rate.to_owned();
            }
            if let Some(bits) = serial.get("data_bits").and_then(|v| v.as_str()) {
                self.serial.data_bits = bits.to_owned();
            }
            if let Some(stop) = serial.get("stop_bits").and_then(|v| v.as_str()) {
                self.serial.stop_bits = stop.to_owned();
            }
            if let Some(parity) = serial.get("parity").and_then(|v| v.as_str()) {
                self.serial.parity = parity.to_owned();
            }
            if let Some(reconnect) = serial.get("auto_reconnect").and_then(|v| v.as_bool()) {
                self.serial.auto_reconnect = reconnect;
            }
            if let Some(port) = serial.get("selected_port").and_then(|v| v.as_str()) {
                self.serial.selected_port = Some(port.to_owned());
            }
            if let Some(value) = serial.get("port_aliases")
                && let Ok(aliases) = serde_json::from_value(value.clone())
            {
                self.serial.port_aliases = aliases;
            }
            if let Some(value) = serial.get("port_groups")
                && let Ok(groups) = serde_json::from_value(value.clone())
            {
                self.serial.port_groups = groups;
            }
            if let Some(value) = serial.get("port_profiles")
                && let Ok(profiles) = serde_json::from_value(value.clone())
            {
                self.serial.port_profiles = profiles;
            }
        }
        if let Some(send) = config.get("send") {
            if let Some(ending) = send.get("line_ending").and_then(|v| v.as_str()) {
                self.send.line_ending = ending.to_owned();
            }
            if let Some(hex) = send.get("hex_mode").and_then(|v| v.as_bool()) {
                self.send.hex_mode = hex;
            }
            if let Some(strict) = send.get("strict_hex").and_then(|v| v.as_bool()) {
                self.send.strict_hex = strict;
            }
            if let Some(interval) = send.get("periodic_interval_ms").and_then(|v| v.as_u64()) {
                self.send.periodic_interval_ms = interval;
            }
            if let Some(enabled) = send.get("periodic_enabled").and_then(|v| v.as_bool()) {
                self.send.periodic_enabled = enabled;
            }
            if let Some(max_history) = send.get("max_history").and_then(|v| v.as_u64()) {
                self.send.max_history = max_history as usize;
            }
            if let Some(history) = send.get("history").and_then(|v| v.as_array()) {
                self.send.history = history
                    .iter()
                    .filter_map(|entry| entry.as_str().map(ToOwned::to_owned))
                    .collect();
            }
        }
        if let Some(layout) = config.get("layout").filter(|value| value.is_object()) {
            self.layout = layout.clone();
        }
        Ok(())
    }

    // ── 串口控制 ──

    /// 设置 DTR。
    pub fn set_dtr(&self, port: &str, value: bool) -> Result<(), String> {
        self.transport
            .set_dtr(port, value)
            .map_err(|error| error.to_string())
    }

    /// 设置 RTS。
    pub fn set_rts(&self, port: &str, value: bool) -> Result<(), String> {
        self.transport
            .set_rts(port, value)
            .map_err(|error| error.to_string())
    }
}

impl Drop for WorkbenchBackend {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn replay_policy_name(policy: ReplayPolicy) -> &'static str {
    match policy {
        ReplayPolicy::AutoPreferRecorded => "auto",
        ReplayPolicy::ExactRecorded => "exact",
        ReplayPolicy::ReparseRaw => "reparse",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_app_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "tool-backend-{name}-{}",
            tool_core::now_timestamp_ms()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn layout_survives_config_round_trip() {
        let app_dir = temporary_app_dir("layout");
        {
            let mut backend = WorkbenchBackend::new(app_dir.clone());
            backend
                .handle_command(
                    "set_layout",
                    &serde_json::json!({
                        "layout": {
                            "bottomVisible": true,
                            "bottomSize": 420,
                            "center": {"tabs": ["terminal"], "activeIndex": 0}
                        }
                    }),
                )
                .unwrap();
            backend.save_config().unwrap();
        }
        let restored = WorkbenchBackend::new(app_dir.clone());
        assert_eq!(restored.layout["bottomVisible"], true);
        assert_eq!(restored.layout["bottomSize"], 420);
        std::fs::remove_dir_all(app_dir).unwrap();
    }

    #[test]
    fn send_config_rejects_invalid_interval() {
        let app_dir = temporary_app_dir("send-config");
        let mut backend = WorkbenchBackend::new(app_dir.clone());
        let error = backend
            .handle_command(
                "set_send_config",
                &serde_json::json!({"periodic_interval_ms": 1}),
            )
            .unwrap_err();
        assert!(error.contains("周期"));
        drop(backend);
        std::fs::remove_dir_all(app_dir).unwrap();
    }
}
