//! Browser composition root.
//!
//! Native-only services are replaced by browser
//! capabilities where the platform has a real equivalent: Web Serial,
//! local settings, Blob export, lossless in-memory recording and JSONL replay.

use crate::bootstrap;
use crate::panel_registry::{BuiltinPanel, PanelKind, PanelRegistry};
use crate::shared_keymap::{
    BUILTIN_KEYMAP_COMMANDS, CMD_CLEAR_TERMINAL, CMD_COMMAND_PALETTE, CMD_OPEN_PORT,
    CMD_RECONNECT_PORT, CMD_REFRESH_PORTS, CMD_SEND, CMD_TOGGLE_BOTTOM_PANEL,
    CMD_TOGGLE_RIGHT_DOCK, KeyBinding, Keymap,
};
use crate::shared_settings::{SETTINGS_NAV_ITEMS, settings_nav_button};
use crate::shared_shell::{AppShellHost, DockHost};
use crate::web::plugins::plugin_setting_view;
use crate::web::serial::{
    accept_web_attached_port, apply_web_refreshed_ports, forget_web_detached_port,
    forget_web_network_port, handle_web_reconnect_task_cancelled, handle_web_reconnect_task_failed,
    select_authorized_port, select_web_port_state, settings_label, web_connected_port,
    web_port_display_name, web_reconnect_chip_ui, web_serial_status_label, web_top_bar_port_items,
    web_top_bar_serial_command,
};
use crate::web_plugin_host::{WebPluginData, WebPluginDataStore, WebPluginHost};
use crate::workbench_app::WorkbenchApp;
use eframe::egui;
use egui::FontFamily;
use egui_material_icons::icons::{
    ICON_FIBER_MANUAL_RECORD, ICON_FOLDER, ICON_INFO, ICON_KEYBOARD, ICON_NETWORK_CHECK,
    ICON_PALETTE, ICON_RESTART_ALT, ICON_SEARCH, ICON_STOP, ICON_TUNE,
};
use js_sys::Array;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use tool_application::plugin::{
    PluginCommandView, PluginContributesView, PluginPanelContributionView, PluginStateView,
    PluginSummaryView, PluginUiContributionView,
};
use tool_application::replay::ReplayPolicyView;
use tool_application::send_plan;
use tool_application::web::{SignalKind, WebAppEvent, WebRuntime};
use tool_application::{AppCommand, AppRuntime, CommandOutcome, TaskId};
use tool_core::Event;
// 行尾符表已收进 `tool-core`；`WebSettings` 仍以 `WebLineEnding` 这个名字持久化
// 裸变体名（浏览器设置），合并前的表与 `tool_core` 那份逐字节相同。
use tool_core::LineEnding as WebLineEnding;
use tool_core::{Direction, Payload, topic_matches, topics};
use tool_databus::DataBus;
use tool_panels::{
    ChartPanel, DataSettingsView, KeymapAction, KeymapEntry, LogExportCursor, LogPanel,
    PANEL_DEVICES, PanelId, PanelManager, PluginsPanel, RecordingAction, RecordingMode,
    RecordingView, ReplayPanel, ReplayPolicyOption, SerialPanel, SerialTopBarView, StatusBarAction,
    StatusBarView, StatusSignalView, TerminalExportCursor, TerminalExportFormat, TerminalPanel,
    copy_text_with_feedback, data_settings_ui, design, keymap_ui, recording_ui,
    status_bar_contents_ui, theme,
};
use tool_panels::{
    SendAction, SendPortItem, SendToolbarButton, SendView,
    record_history as record_shared_send_history, send_layout_for_width,
    sender_ui as shared_sender_ui,
};
use tool_platform::storage::{SettingsStore, web::WebSettingsStore};
use tool_platform::{NetworkSerialConfig, PortDescriptor, PortId, SerialSettings};
use tool_plugin_api::{PluginInstanceId, PluginSerialDevice};
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::{JsFuture, spawn_local};

// 浏览器侧 composition root 按功能域拆分：插件（Lua 宿主与贡献点）与回放
// （JSONL 加载与 Web Replay Analyzer）各自一个子模块，其余仍是本文件的组合根。
// 串口/端口/自动重连域同样独立成 serial.rs；全部类型定义仍留在本文件。
mod plugins;
mod replay;
mod serial;

const NOTO_SANS_SC: &[u8] = include_bytes!("../../../../assets/NotoSansSC-VF.ttf");
const JETBRAINS_MONO: &[u8] =
    include_bytes!("../../../../assets/JetBrainsMonoNerdFontMono-Regular.ttf");
// Bump when the shared default Dock geometry changes so an older browser
// localStorage layout cannot silently keep the pre-parity split ratios.
const WEB_LAYOUT_VERSION: u8 = 6;
const WEB_MAX_SEND_HISTORY: usize = 200;
const WEB_PLUGIN_API_VERSION: &str = "1";
const WEB_MARKETPLACE_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/Tydwdh/serial_tool/main/plugin-marketplace/registry.json";
const REPOSITORY_URL: &str = env!("CARGO_PKG_REPOSITORY");

/// 两步确认按钮在 egui 临时记忆里的 salt，与 `ui/settings_panel.rs` 的同名常量一致。
const LAYOUT_RESET_CONFIRM_ID: &str = "reset_workspace_layout_confirm";
const RESTORE_DEFAULTS_CONFIRM_ID: &str = "restore_all_defaults_confirm";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebNotificationLevel {
    Info,
    Warn,
    Error,
}

impl WebNotificationLevel {
    fn ttl_seconds(self) -> f64 {
        match self {
            Self::Info => 1.5,
            Self::Warn => 2.0,
            Self::Error => 4.0,
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            Self::Info => theme::text_secondary(),
            Self::Warn => theme::yellow(),
            Self::Error => theme::red(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WebNotification {
    pub(crate) source: String,
    pub(crate) level: WebNotificationLevel,
    pub(crate) text: String,
    pub(crate) expires_at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WebSettings {
    #[serde(default)]
    layout_version: u8,
    #[serde(default)]
    theme: theme::AppTheme,
    #[serde(default)]
    custom_theme_source: Option<String>,
    #[serde(default)]
    serial: SerialSettings,
    #[serde(default)]
    network_ports: Vec<NetworkSerialConfig>,
    #[serde(default)]
    port_aliases: BTreeMap<String, String>,
    #[serde(default)]
    port_groups: BTreeMap<String, String>,
    #[serde(default)]
    port_profiles: BTreeMap<String, SerialSettings>,
    #[serde(default = "default_auto_reconnect")]
    auto_reconnect: bool,
    #[serde(default)]
    top_bar_serial_collapsed: bool,
    #[serde(default)]
    tx_hex: bool,
    #[serde(default)]
    send_line_ending: WebLineEnding,
    #[serde(default = "default_hex_strict")]
    send_hex_strict: bool,
    #[serde(default)]
    send_history: Vec<String>,
    #[serde(default = "default_periodic_interval_ms")]
    send_periodic_interval_ms: String,
    #[serde(default = "default_record_file_name")]
    record_file_name: String,
    #[serde(default)]
    record_mode: WebRecordMode,
    #[serde(default)]
    replay_policy: ReplayPolicyOption,
    #[serde(default)]
    keymap: Keymap,
    /// 命令面板最近使用顺序（命令 id）：键空间与 native 那份同名字段一致，
    /// 两端各存各的存储，但同一个 id 才认得同一份历史。
    #[serde(default)]
    command_usage_order: Vec<String>,
    #[serde(default = "default_terminal_merge_window_ms")]
    terminal_merge_window_ms: u64,
    #[serde(default = "default_terminal_max_entries")]
    terminal_max_entries: usize,
    #[serde(default = "default_terminal_max_entries")]
    log_max_entries: usize,
    #[serde(default = "default_font_size")]
    font_size: f32,
    #[serde(default = "PanelManager::default_workspace")]
    panels: PanelManager,
    #[serde(default)]
    web_plugins: Vec<WebPluginPersisted>,
    #[serde(default = "default_web_marketplace_url")]
    marketplace_url: String,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            layout_version: WEB_LAYOUT_VERSION,
            theme: theme::AppTheme::default(),
            custom_theme_source: None,
            serial: SerialSettings::default(),
            network_ports: Vec::new(),
            port_aliases: BTreeMap::new(),
            port_groups: BTreeMap::new(),
            port_profiles: BTreeMap::new(),
            auto_reconnect: default_auto_reconnect(),
            top_bar_serial_collapsed: false,
            tx_hex: false,
            send_line_ending: WebLineEnding::None,
            send_hex_strict: true,
            send_history: Vec::new(),
            send_periodic_interval_ms: default_periodic_interval_ms(),
            record_file_name: default_record_file_name(),
            record_mode: WebRecordMode::StandardReplay,
            replay_policy: ReplayPolicyOption::AutoPreferRecorded,
            keymap: Keymap::default(),
            command_usage_order: Vec::new(),
            terminal_merge_window_ms: default_terminal_merge_window_ms(),
            terminal_max_entries: default_terminal_max_entries(),
            log_max_entries: default_terminal_max_entries(),
            font_size: default_font_size(),
            panels: PanelManager::default_workspace(),
            web_plugins: Vec::new(),
            marketplace_url: default_web_marketplace_url(),
        }
    }
}

pub(crate) struct WebSerialState {
    ports: Vec<PortDescriptor>,
    selected_port: Option<PortId>,
    connected: Option<PortId>,
    auto_reconnect: bool,
    top_bar_serial_collapsed: bool,
    reconnect: Option<WebReconnectState>,
    send_input: String,
    tx_hex: bool,
    line_ending: WebLineEnding,
    hex_strict: bool,
    send_error: Option<String>,
    send_history: Vec<String>,
    history_search: String,
    history_index: Option<usize>,
    saved_input: String,
    periodic_enabled: bool,
    periodic_interval_ms: String,
    periodic_next_at: Option<f64>,
    periodic_send_count: u64,
    dtr: bool,
    rts: bool,
    settings: SerialSettings,
    network_ports: Vec<NetworkSerialConfig>,
    port_aliases: BTreeMap<String, String>,
    port_groups: BTreeMap<String, String>,
    port_profiles: BTreeMap<String, SerialSettings>,
    network_host: String,
    network_port: String,
    network_api_key: String,
    status: String,
    refresh_on_page_entry: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) enum WebRecordMode {
    #[default]
    StandardReplay,
    RawSerial,
}

struct WebReconnectState {
    port: PortId,
    vendor_id: Option<u16>,
    product_id: Option<u16>,
    attempts: u32,
    next_attempt_at: f64,
    task_id: Option<TaskId>,
}

impl WebReconnectState {
    fn matches(&self, port: &PortDescriptor) -> bool {
        self.port == port.id
            || (self.vendor_id.is_some()
                && self.vendor_id == port.vendor_id
                && self.product_id == port.product_id)
    }
}

fn default_record_file_name() -> String {
    "hardware-workbench-session.jsonl".to_owned()
}

fn default_auto_reconnect() -> bool {
    true
}

fn default_terminal_merge_window_ms() -> u64 {
    5
}

fn default_terminal_max_entries() -> usize {
    50_000
}

fn default_font_size() -> f32 {
    13.0
}

fn default_hex_strict() -> bool {
    true
}

fn default_periodic_interval_ms() -> String {
    "1000".to_owned()
}

fn default_web_marketplace_url() -> String {
    WEB_MARKETPLACE_REGISTRY_URL.to_owned()
}

fn web_now_seconds() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map(|performance| performance.now() / 1000.0)
        .unwrap_or(0.0)
}

fn record_web_send_history(serial: &mut WebSerialState, text: String) {
    if text.trim().is_empty() {
        return;
    }
    if serial.send_history.first() == Some(&text) {
        return;
    }
    serial.send_history.retain(|item| item != &text);
    serial.send_history.insert(0, text);
    serial.send_history.truncate(WEB_MAX_SEND_HISTORY);
}

/// 设备消失后登记待重连状态；vid/pid 取自拔出前最后一次看到的端口描述符，
/// 用于在端口名变化时仍能认出同一台物理设备。
fn web_reconnect_state(port: PortId, descriptor: Option<PortDescriptor>) -> WebReconnectState {
    let (vendor_id, product_id) = descriptor
        .map(|item| (item.vendor_id, item.product_id))
        .unwrap_or((None, None));
    WebReconnectState {
        port,
        vendor_id,
        product_id,
        attempts: 0,
        next_attempt_at: 0.0,
        task_id: None,
    }
}

/// wasm 侧的 HEX 预检：判定与真正发送时**同源**——`WebApplication::validate_hex` 调的
/// 就是 native `dispatch` 用的 `tool_core::{parse_hex, parse_hex_strict}`。
///
/// 这里原本是本文件手抄的第三份判定：它的严格分支只按空白分词、既不认 `,`/`;` 分隔，
/// 也只剥一层 `0x` 前缀，于是 `"0A,BB"`、`"0x0xAB"` 一类输入在 web 被拒、在 native 放行。
/// 收敛后 web 与 native 判定完全一致（即 web 在这几例上变得更宽松）。
fn web_hex_error(runtime: Option<&WebRuntime>, input: &str, strict: bool) -> Option<String> {
    match runtime {
        Some(runtime) => runtime.validate_hex(input, strict).err(),
        None => Some("当前浏览器不支持 Web Serial".to_owned()),
    }
}

/// 创建一个 `type=file` 的 input 元素（不插入 DOM）。
///
/// accept/multiple 由各入口自己设置；任一步失败都返回 None，让调用点保留各自的提示文案。
fn web_file_input(document: &web_sys::Document) -> Option<web_sys::HtmlInputElement> {
    let input = document
        .create_element("input")
        .ok()
        .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())?;
    input.set_type("file");
    Some(input)
}

/// 从 change 事件里取回触发它的 input 元素。
fn web_file_input_from_event(event: &web_sys::Event) -> Option<web_sys::HtmlInputElement> {
    event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
}

/// 挂上 change 监听并立即打开文件选择器。
///
/// 闭包必须泄漏：文件内容只在异步的 change 事件里到达一次，届时栈上的 `Closure` 已不存在。
/// 返回 false 表示监听器没挂上，此时不会打开选择器，由调用点提示；闭包随之正常回收。
fn web_open_file_picker(
    input: &web_sys::HtmlInputElement,
    on_change: impl FnMut(web_sys::Event) + 'static,
) -> bool {
    let callback: Box<dyn FnMut(web_sys::Event)> = Box::new(on_change);
    let closure = Closure::wrap(callback);
    if input
        .add_event_listener_with_callback("change", closure.as_ref().unchecked_ref())
        .is_err()
    {
        return false;
    }
    closure.forget();
    input.click();
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct WebPluginContributes {
    #[serde(default)]
    pub(crate) panels: Vec<serde_json::Value>,
    #[serde(default)]
    pub(crate) commands: Vec<WebPluginCommand>,
    #[serde(default)]
    pub(crate) settings: Vec<WebPluginSetting>,
    #[serde(default)]
    pub(crate) ui: Vec<WebPluginUiContribution>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WebPluginCommand {
    pub(crate) id: String,
    pub(crate) title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WebPluginSetting {
    pub(crate) id: String,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) default: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) options: Vec<serde_json::Value>,
    #[serde(default)]
    pub(crate) min: Option<f64>,
    #[serde(default)]
    pub(crate) max: Option<f64>,
    #[serde(default)]
    pub(crate) step: Option<f64>,
    #[serde(default)]
    pub(crate) rows: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WebPluginUiContribution {
    pub(crate) id: String,
    pub(crate) slot: String,
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) command: Option<String>,
    #[serde(default)]
    pub(crate) tooltip: Option<String>,
    #[serde(default)]
    pub(crate) order: i32,
    #[serde(default = "default_true")]
    pub(crate) enabled: bool,
    #[serde(default = "default_true")]
    pub(crate) visible: bool,
    #[serde(default)]
    pub(crate) record_send_input: bool,
    #[serde(default)]
    pub(crate) default: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WebPluginManifest {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) version: String,
    #[serde(default = "default_web_plugin_api_version")]
    pub(crate) api_version: String,
    pub(crate) runtime: String,
    pub(crate) main: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) author: Option<String>,
    #[serde(default)]
    pub(crate) homepage: Option<String>,
    #[serde(default)]
    pub(crate) repository: Option<String>,
    #[serde(default)]
    pub(crate) license: Option<String>,
    #[serde(default)]
    pub(crate) category: Option<String>,
    #[serde(default)]
    pub(crate) icon: Option<String>,
    #[serde(default)]
    pub(crate) permissions: Vec<String>,
    #[serde(default)]
    pub(crate) live: Option<WebPluginLiveConfig>,
    #[serde(default)]
    pub(crate) replay: Option<WebPluginReplayConfig>,
    #[serde(default)]
    pub(crate) contributes: WebPluginContributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct WebPluginLiveConfig {
    #[serde(default)]
    pub(crate) main: Option<String>,
    #[serde(default)]
    pub(crate) permissions: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) subscriptions: Vec<String>,
}

impl WebPluginManifest {
    fn live_main(&self) -> &str {
        self.live
            .as_ref()
            .and_then(|live| live.main.as_deref())
            .unwrap_or(&self.main)
    }

    fn live_permissions(&self) -> &[String] {
        self.live
            .as_ref()
            .and_then(|live| live.permissions.as_deref())
            .unwrap_or(&self.permissions)
    }

    fn live_subscriptions(&self) -> &[String] {
        self.live
            .as_ref()
            .map(|live| live.subscriptions.as_slice())
            .unwrap_or(&[])
    }

    /// 清单是否声明了某项权限。沿用 `live_permissions()` 的回落语义：
    /// 没有 live 段时按顶层 permissions 判定。
    fn has_permission(&self, permission: &str) -> bool {
        self.live_permissions()
            .iter()
            .any(|name| name == permission)
    }

    /// 事件是否要投递给该插件。未声明 subscriptions 时收全量；`ui.`、`log.`
    /// 与插件命令主题无条件投递，因为贡献点渲染和命令执行都靠它们驱动。
    fn receives_event(&self, event: &Event) -> bool {
        self.live_subscriptions().is_empty()
            || self
                .live_subscriptions()
                .iter()
                .any(|pattern| topic_matches(pattern, &event.topic))
            || event.topic.starts_with("ui.")
            || event.topic.starts_with("log.")
            || event.topic == topics::PLUGIN_COMMAND_EXECUTE
    }
}

/// Browser replay analyzer metadata. Live plugin execution uses the same Lua
/// source as Native; replay callbacks remain a separately scheduled phase.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct WebPluginReplayConfig {
    #[serde(default = "default_web_replay_main")]
    pub(crate) main: String,
    #[serde(default)]
    pub(crate) subscriptions: Vec<String>,
    #[serde(default)]
    pub(crate) outputs: Vec<String>,
}

fn default_web_replay_main() -> String {
    "replay.lua".to_owned()
}

fn default_web_plugin_api_version() -> String {
    WEB_PLUGIN_API_VERSION.to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WebPluginPersisted {
    manifest: WebPluginManifest,
    source: String,
    #[serde(default)]
    replay_source: Option<String>,
    enabled: bool,
    #[serde(default)]
    settings: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    storage: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    profiles: BTreeMap<String, serde_json::Value>,
}

struct WebPluginRecord {
    persisted: WebPluginPersisted,
    host: Option<Rc<WebPluginHost>>,
    lua_instance: Option<PluginInstanceId>,
    replay_instance: Option<PluginInstanceId>,
    load_task: Option<TaskId>,
    loading: bool,
    error: Option<String>,
    panels_published: bool,
}

impl WebPluginRecord {
    /// 新安装或从持久化设置恢复的插件：只带清单与源码，运行期状态一律为空。
    fn new(persisted: WebPluginPersisted) -> Self {
        Self {
            persisted,
            host: None,
            lua_instance: None,
            replay_instance: None,
            load_task: None,
            loading: false,
            error: None,
            panels_published: false,
        }
    }
}

type PendingLuaFileRequests = Rc<RefCell<BTreeMap<TaskId, (Rc<WebPluginHost>, String)>>>;
type PendingLuaSerialRequests = Rc<RefCell<BTreeMap<TaskId, (Rc<WebPluginHost>, String)>>>;

pub(crate) struct WebPluginState {
    records: Vec<WebPluginRecord>,
    /// Runtime values published by `ctx.ui.set_contribution_value`.
    ///
    /// Keep these in the browser composition root just like Native keeps
    /// them in `WorkbenchApp`; they are presentation state, not persisted
    /// plugin settings. The qualified key prevents two plugins from
    /// accidentally updating the same contribution id.
    contribution_states: BTreeMap<String, serde_json::Value>,
    pending_lua_file_requests: PendingLuaFileRequests,
    pending_lua_serial_requests: PendingLuaSerialRequests,
}

struct WebPaletteCommand {
    id: String,
    title: String,
}

pub(crate) enum WebExportJob {
    Terminal(TerminalExportCursor),
    Logs(LogExportCursor),
}

pub(crate) struct WebExportState {
    task_id: TaskId,
    stem: &'static str,
    format: TerminalExportFormat,
    job: WebExportJob,
    offset: usize,
    content: String,
}

impl Default for WebPluginState {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            contribution_states: BTreeMap::new(),
            pending_lua_file_requests: Rc::new(RefCell::new(BTreeMap::new())),
            pending_lua_serial_requests: Rc::new(RefCell::new(BTreeMap::new())),
        }
    }
}

impl WebPluginState {
    fn restore(&mut self, persisted: Vec<WebPluginPersisted>) {
        self.contribution_states.clear();
        self.pending_lua_file_requests.borrow_mut().clear();
        self.pending_lua_serial_requests.borrow_mut().clear();
        self.records = persisted.into_iter().map(WebPluginRecord::new).collect();
    }

    fn persisted(&self) -> Vec<WebPluginPersisted> {
        self.records
            .iter()
            .map(|record| record.persisted.clone())
            .collect()
    }

    fn set_contribution_value(
        &mut self,
        plugin_id: &str,
        contribution_id: &str,
        value: serde_json::Value,
    ) {
        self.contribution_states
            .insert(format!("{plugin_id}:{contribution_id}"), value);
    }

    fn contribution_value(
        &self,
        plugin_id: &str,
        contribution_id: &str,
    ) -> Option<&serde_json::Value> {
        self.contribution_states
            .get(&format!("{plugin_id}:{contribution_id}"))
    }

    fn clear_contribution_values(&mut self, plugin_id: &str) {
        let prefix = format!("{plugin_id}:");
        self.contribution_states
            .retain(|key, _| !key.starts_with(&prefix));
    }

    /// Convert browser plugin manifests into the same Application DTO consumed
    /// by Native plugin presentation code. Runtime-only Lua handles deliberately
    /// stay out of this view.
    pub(crate) fn summaries(&self) -> Vec<PluginSummaryView> {
        self.records
            .iter()
            .map(|record| {
                let manifest = &record.persisted.manifest;
                let contributes = PluginContributesView {
                    commands: manifest
                        .contributes
                        .commands
                        .iter()
                        .map(|command| PluginCommandView {
                            id: command.id.clone(),
                            title: command.title.clone(),
                        })
                        .collect(),
                    ui: manifest
                        .contributes
                        .ui
                        .iter()
                        .map(web_plugin_ui_contribution_view)
                        .collect(),
                    panels: manifest
                        .contributes
                        .panels
                        .iter()
                        .filter_map(web_plugin_panel_contribution_view)
                        .collect(),
                    settings: manifest
                        .contributes
                        .settings
                        .iter()
                        .map(plugin_setting_view)
                        .collect(),
                };
                PluginSummaryView {
                    id: manifest.id.clone(),
                    name: manifest.name.clone(),
                    version: manifest.version.clone(),
                    api_version: manifest.api_version.clone(),
                    runtime: manifest.runtime.clone(),
                    state: web_plugin_record_state(record),
                    permissions: manifest.live_permissions().to_vec(),
                    contributes,
                    path: format!("browser://plugin/{}", manifest.id),
                    last_error: record.error.clone(),
                    description: manifest.description.clone(),
                    author: manifest.author.clone(),
                    homepage: manifest.homepage.clone(),
                    repository: manifest.repository.clone(),
                    license: manifest.license.clone(),
                    category: manifest.category.clone(),
                    icon: manifest.icon.clone(),
                    has_replay_analyzer: manifest.replay.is_some(),
                    replay_subscriptions: manifest
                        .replay
                        .as_ref()
                        .map(|replay| replay.subscriptions.clone())
                        .unwrap_or_default(),
                    replay_outputs: manifest
                        .replay
                        .as_ref()
                        .map(|replay| replay.outputs.clone())
                        .unwrap_or_default(),
                    registered_commands: manifest
                        .contributes
                        .commands
                        .iter()
                        .map(|command| command.id.clone())
                        .collect(),
                    missing_commands: Vec::new(),
                    undeclared_commands: Vec::new(),
                }
            })
            .collect()
    }
}

#[derive(Default)]
pub(crate) struct WebReplayAnalyzerState {
    running: bool,
    task_id: Option<TaskId>,
    plugin_indices: Vec<usize>,
    input_events: Vec<Event>,
    next_plugin: usize,
    input_indices: Vec<usize>,
    input_offset: usize,
    derived_events: Vec<Event>,
    logs: Vec<String>,
    error: Option<String>,
}

impl Default for WebSerialState {
    fn default() -> Self {
        Self {
            ports: Vec::new(),
            selected_port: None,
            connected: None,
            auto_reconnect: default_auto_reconnect(),
            top_bar_serial_collapsed: false,
            reconnect: None,
            send_input: String::new(),
            tx_hex: false,
            line_ending: WebLineEnding::None,
            hex_strict: true,
            send_error: None,
            send_history: Vec::new(),
            history_search: String::new(),
            history_index: None,
            saved_input: String::new(),
            periodic_enabled: false,
            periodic_interval_ms: default_periodic_interval_ms(),
            periodic_next_at: None,
            periodic_send_count: 0,
            dtr: true,
            rts: true,
            settings: SerialSettings::default(),
            network_ports: Vec::new(),
            port_aliases: BTreeMap::new(),
            port_groups: BTreeMap::new(),
            port_profiles: BTreeMap::new(),
            network_host: String::new(),
            network_port: "7125".to_owned(),
            network_api_key: String::new(),
            status: String::new(),
            refresh_on_page_entry: true,
        }
    }
}

impl WorkbenchApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let selected_theme = theme::AppTheme::default();
        apply_web_theme(&cc.egui_ctx, selected_theme);
        setup_web_fonts(cc);

        let bus = DataBus::new();
        let runtime = WebRuntime::new(bus.clone()).ok().inspect(|runtime| {
            let ctx = cc.egui_ctx.clone();
            runtime.set_repaint_waker(Rc::new(move || ctx.request_repaint()));
        });
        let serial = Rc::new(RefCell::new(WebSerialState::default()));
        let plugin_data: WebPluginDataStore =
            Rc::new(RefCell::new(BTreeMap::<String, WebPluginData>::new()));
        let settings_store = WebSettingsStore::from_window("hardware-workbench").ok();
        let settings_load = Rc::new(RefCell::new(None));
        if let Some(store) = settings_store.clone() {
            let loaded = settings_load.clone();
            let ctx = cc.egui_ctx.clone();
            spawn_local(async move {
                let settings = match store.load("settings.json".to_owned()).await {
                    Ok(Some(bytes)) => serde_json::from_slice::<WebSettings>(&bytes).ok(),
                    _ => match store.load("theme.json".to_owned()).await {
                        Ok(Some(bytes)) => serde_json::from_slice::<theme::AppTheme>(&bytes)
                            .ok()
                            .map(|theme| WebSettings {
                                theme,
                                ..WebSettings::default()
                            }),
                        _ => None,
                    },
                };
                if let Some(settings) = settings {
                    *loaded.borrow_mut() = Some(settings);
                    ctx.request_repaint();
                }
            });
        }
        let serial_status = if runtime.as_ref().is_some_and(WebRuntime::serial_supported) {
            ""
        } else {
            "当前浏览器不支持 Web Serial"
        };
        serial.borrow_mut().status = serial_status.to_owned();
        let app = Self {
            terminal_panel: TerminalPanel::new(&bus),
            chart_panel: ChartPanel::new(&bus),
            bottom_log_panel: LogPanel::new(&bus),
            panels: PanelManager::default_workspace(),
            panel_registry: PanelRegistry::web(),
            layout_dirty: false,
            ui_theme: selected_theme,
            theme_source: None,
            runtime,
            dynamic_panels: tool_panels::DynamicPanels::new(&bus),
            serial,
            settings_store,
            settings_load,
            recording_file_name: default_record_file_name(),
            recording_mode: WebRecordMode::StandardReplay,
            replay_panel: ReplayPanel::new(),
            replay_analyzer: WebReplayAnalyzerState::default(),
            plugins: WebPluginState::default(),
            plugin_data,
            web_lua: tool_plugin_runtime::WebLuaEngine::new(),
            plugins_panel: PluginsPanel::new(),
            marketplace_url: default_web_marketplace_url(),
            keymap: Keymap::default(),
            key_recording: None,
            command_palette_open: false,
            command_palette_query: String::new(),
            command_palette_selected: None,
            command_usage_order: Vec::new(),
            web_export: None,
            perf: crate::web_perf::WebPerfDiagnostics::default(),
            web_notifications: Vec::new(),
        };
        app.sync_web_plugin_view();
        app
    }

    fn poll_loaded_settings(&mut self, ctx: &egui::Context) {
        let Some(settings) = self.settings_load.borrow_mut().take() else {
            return;
        };
        self.ui_theme = settings.theme;
        self.theme_source = settings.custom_theme_source;
        if let Some(source) = self.theme_source.as_deref() {
            if theme::load_theme_text(source, "浏览器自定义主题").is_ok() {
                self.ui_theme = theme::AppTheme::Custom;
            } else {
                self.theme_source = None;
                self.ui_theme = theme::AppTheme::default();
            }
        } else if self.ui_theme == theme::AppTheme::Custom {
            self.ui_theme = theme::AppTheme::default();
        }
        {
            let mut serial = self.serial.borrow_mut();
            serial.settings = settings.serial;
            serial.network_ports = settings.network_ports.clone();
            serial.port_aliases = settings.port_aliases.clone();
            serial.port_groups = settings.port_groups.clone();
            serial.port_profiles = settings.port_profiles.clone();
            serial.auto_reconnect = settings.auto_reconnect;
            serial.top_bar_serial_collapsed = settings.top_bar_serial_collapsed;
            serial.tx_hex = settings.tx_hex;
            serial.line_ending = settings.send_line_ending;
            serial.hex_strict = settings.send_hex_strict;
            serial.send_history = settings
                .send_history
                .into_iter()
                .filter(|item| !item.trim().is_empty())
                .take(WEB_MAX_SEND_HISTORY)
                .collect();
            serial.periodic_interval_ms = settings.send_periodic_interval_ms;
        }
        for config in &settings.network_ports {
            self.dispatch_without_status(AppCommand::RegisterNetworkPort {
                config: config.clone(),
            });
        }
        self.recording_file_name = settings.record_file_name;
        self.recording_mode = settings.record_mode;
        self.dispatch_without_status(AppCommand::SetReplayPolicy {
            policy: replay_policy_view_option(settings.replay_policy),
        });
        self.keymap = settings.keymap;
        self.key_recording = None;
        self.command_usage_order = settings.command_usage_order;
        self.command_palette_open = false;
        self.command_palette_query.clear();
        self.command_palette_selected = None;
        if let Some(runtime) = self.runtime.as_ref() {
            let _ = runtime.dispatch(AppCommand::SetTerminalMergeWindow {
                ms: settings.terminal_merge_window_ms,
            });
            let _ = runtime.dispatch(AppCommand::SetTerminalMaxEntries {
                max: settings.terminal_max_entries,
            });
        } else {
            self.terminal_panel.merge_window_ms = settings.terminal_merge_window_ms;
            self.terminal_panel
                .set_max_entries(settings.terminal_max_entries);
        }
        self.bottom_log_panel
            .set_max_entries(settings.log_max_entries);
        self.terminal_panel.font_size = settings.font_size;
        self.bottom_log_panel.font_size = settings.font_size;
        self.plugins.restore(settings.web_plugins);
        {
            let mut data = self.plugin_data.borrow_mut();
            data.clear();
            for record in &self.plugins.records {
                data.insert(
                    record.persisted.manifest.id.clone(),
                    WebPluginData {
                        settings: record.persisted.settings.clone(),
                        storage: record.persisted.storage.clone(),
                        profiles: record.persisted.profiles.clone(),
                    },
                );
            }
        }
        self.marketplace_url = settings.marketplace_url;
        for index in 0..self.plugins.records.len() {
            if self.plugins.records[index].persisted.enabled {
                self.load_web_plugin(index);
            }
        }
        self.sync_web_plugin_view();
        let panels = if settings.layout_version == WEB_LAYOUT_VERSION {
            settings.panels
        } else {
            // The earlier WebApp used a separate sidebar/bottom layout. Do not
            // migrate that tree; restart from the shared application default.
            PanelManager::default_workspace()
        };
        self.panels = panels;
        apply_web_theme(ctx, self.ui_theme);
    }

    fn persist_settings(&self) {
        let Some(store) = self.settings_store.clone() else {
            return;
        };
        let (terminal_merge_window_ms, terminal_max_entries) = self
            .runtime
            .as_ref()
            .map(WebRuntime::query_terminal_settings)
            .unwrap_or((
                self.terminal_panel.merge_window_ms,
                self.terminal_panel.max_entries,
            ));
        let mut plugin_records = self.plugins.persisted();
        for record in &mut plugin_records {
            if let Some(data) = self.plugin_data.borrow().get(&record.manifest.id) {
                record.settings = data.settings.clone();
                record.storage = data.storage.clone();
            }
        }
        let settings = WebSettings {
            layout_version: WEB_LAYOUT_VERSION,
            theme: self.ui_theme,
            custom_theme_source: self.theme_source.clone(),
            serial: self.serial.borrow().settings,
            network_ports: self.serial.borrow().network_ports.clone(),
            port_aliases: self.serial.borrow().port_aliases.clone(),
            port_groups: self.serial.borrow().port_groups.clone(),
            port_profiles: self.serial.borrow().port_profiles.clone(),
            auto_reconnect: self.serial.borrow().auto_reconnect,
            top_bar_serial_collapsed: self.serial.borrow().top_bar_serial_collapsed,
            tx_hex: self.serial.borrow().tx_hex,
            send_line_ending: self.serial.borrow().line_ending,
            send_hex_strict: self.serial.borrow().hex_strict,
            send_history: self.serial.borrow().send_history.clone(),
            send_periodic_interval_ms: self.serial.borrow().periodic_interval_ms.clone(),
            record_file_name: self.recording_file_name.clone(),
            record_mode: self.recording_mode,
            replay_policy: self
                .runtime
                .as_ref()
                .map(WebRuntime::query_replay)
                .map(|status| replay_policy_option_view(status.policy))
                .unwrap_or_default(),
            keymap: self.keymap.clone(),
            command_usage_order: self.command_usage_order.clone(),
            terminal_merge_window_ms,
            terminal_max_entries,
            log_max_entries: self.bottom_log_panel.max_entries,
            font_size: self.terminal_panel.font_size,
            panels: self.panels.clone(),
            web_plugins: plugin_records,
            marketplace_url: self.marketplace_url.clone(),
        };
        let Ok(value) = serde_json::to_vec(&settings) else {
            return;
        };
        spawn_local(async move {
            let _ = store.save("settings.json".to_owned(), value).await;
        });
    }

    fn push_web_notification(
        &mut self,
        source: impl Into<String>,
        level: WebNotificationLevel,
        text: impl Into<String>,
    ) {
        let source = source.into();
        let notification = WebNotification {
            source: source.clone(),
            level,
            text: text.into(),
            expires_at: web_now_seconds() + level.ttl_seconds(),
        };
        if let Some(existing) = self
            .web_notifications
            .iter_mut()
            .find(|item| item.source == source)
        {
            *existing = notification;
        } else {
            self.web_notifications.push(notification);
        }
        const MAX_WEB_NOTIFICATIONS: usize = 32;
        if self.web_notifications.len() > MAX_WEB_NOTIFICATIONS {
            let excess = self.web_notifications.len() - MAX_WEB_NOTIFICATIONS;
            self.web_notifications.drain(0..excess);
        }
    }

    fn current_web_notifications(&mut self) -> Vec<WebNotification> {
        let now = web_now_seconds();
        self.web_notifications.retain(|item| item.expires_at > now);
        self.web_notifications.clone()
    }

    /// 状态栏每帧检查一次：缓冲满导致的丢事件是持续性故障，只推送一条短通知，
    /// 并把面板自己的截断标志复位，避免同一次截断反复刷屏。
    fn push_web_overflow_notifications(&mut self) {
        let now_ms = tool_core::now_timestamp_ms();
        let terminal_dropped = self.terminal_panel.take_dropped_events(now_ms);
        if terminal_dropped > 0 {
            self.push_web_notification(
                "terminal-data-loss",
                WebNotificationLevel::Error,
                format!("接收区缓冲已满，丢失 {terminal_dropped} 条最旧事件"),
            );
        }
        let log_dropped = self.bottom_log_panel.take_dropped_events(now_ms);
        if log_dropped > 0 {
            self.push_web_notification(
                "log-data-loss",
                WebNotificationLevel::Warn,
                format!("日志缓冲已满，丢失 {log_dropped} 条最旧事件"),
            );
        }
        if self.terminal_panel.truncated {
            self.push_web_notification(
                "terminal",
                WebNotificationLevel::Warn,
                format!(
                    "终端已截断，仅保留最近 {} 条",
                    self.terminal_panel.max_entries
                ),
            );
            self.terminal_panel.truncated = false;
        }
        if self.bottom_log_panel.truncated {
            self.push_web_notification(
                "log",
                WebNotificationLevel::Warn,
                format!(
                    "日志已截断，仅保留最近 {} 条",
                    self.bottom_log_panel.max_entries
                ),
            );
            self.bottom_log_panel.truncated = false;
        }
    }

    fn start_web_recording(&mut self) {
        let Some(runtime) = self.runtime.as_ref() else {
            self.serial.borrow_mut().status = "异步任务运行时不可用".to_owned();
            return;
        };
        let mode = web_recording_mode_view(self.recording_mode);
        match runtime.dispatch(AppCommand::StartRecording {
            file: tool_platform::storage::FileHandle::named(self.recording_file_name.clone()),
            mode,
        }) {
            Ok(CommandOutcome::Done) => self.serial.borrow_mut().status = "录制中".to_owned(),
            Ok(CommandOutcome::Pending { message, .. }) => {
                self.serial.borrow_mut().status = message
            }
            Err(error) => self.serial.borrow_mut().status = format!("录制失败：{error}"),
        }
    }

    fn stop_web_recording(&mut self, _incomplete: bool, _reason: Option<String>) {
        let Some(runtime) = self.runtime.as_ref() else {
            return;
        };
        let outcome = runtime.dispatch(AppCommand::StopRecording);
        match outcome {
            Ok(CommandOutcome::Pending { message, .. }) => {
                self.serial.borrow_mut().status = message
            }
            Ok(CommandOutcome::Done) => {}
            Err(error) => self.serial.borrow_mut().status = format!("停止录制失败：{error}"),
        }
    }

    fn request_web_theme_file(&mut self, _ctx: &egui::Context) {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            self.serial.borrow_mut().status = "浏览器文档不可用".to_owned();
            return;
        };
        let Some(input) = web_file_input(&document) else {
            self.serial.borrow_mut().status = "无法创建主题文件选择器".to_owned();
            return;
        };
        input.set_accept(".json,application/json");
        let Some(runtime) = self.runtime.clone() else {
            self.serial.borrow_mut().status = "当前浏览器没有可用的异步任务运行时".to_owned();
            return;
        };
        let opened = web_open_file_picker(&input, move |event: web_sys::Event| {
            let Some(input) = web_file_input_from_event(&event) else {
                return;
            };
            let Some(file) = input.files().and_then(|files| files.get(0)) else {
                return;
            };
            let name = file.name();
            let future = async move {
                JsFuture::from(file.text())
                    .await
                    .map_err(|error| format!("读取主题文件失败：{error:?}"))
                    .and_then(|value| {
                        value
                            .as_string()
                            .ok_or_else(|| "主题文件不是有效文本".to_owned())
                    })
            };
            let _ = runtime.load_text("theme_import", name, future);
        });
        if !opened {
            self.serial.borrow_mut().status = "无法监听主题文件选择事件".to_owned();
        }
    }

    fn request_web_update_check(&mut self, _ctx: &egui::Context) {
        self.dispatch_command_to_status(AppCommand::CheckForUpdate);
    }

    /// 浏览器导入的 JSON 主题落地：只有解析成功才替换 `theme_source`，
    /// 失败时保留原主题，仅回写一条状态文案。
    fn apply_web_theme_import(&mut self, source: String, ctx: &egui::Context) {
        match theme::load_theme_text(&source, "浏览器自定义主题") {
            Ok(name) => {
                self.theme_source = Some(source);
                self.ui_theme = theme::AppTheme::Custom;
                apply_web_theme(ctx, self.ui_theme);
                self.persist_settings();
                self.serial.borrow_mut().status = format!("已应用自定义主题：{name}");
            }
            Err(error) => {
                self.serial.borrow_mut().status = format!("主题加载失败：{error}");
            }
        }
    }

    /// 浏览器每帧的事件泵：一次取空本帧事件队列，逐条交给 `handle_web_event`。
    ///
    /// 这里不做限量、也不许提前退出：队列是一次性取走的，运行时不会替我们留着没处理的
    /// 那条，跳过任何一条就等于丢掉它的 UI 状态与回执（录制导出、Lua 请求回执都是单次
    /// 状态机推进，错过补不回来）。事件顺序即产生顺序，逐条按序处理，不要重排。
    fn poll_web_events(&mut self, ctx: &egui::Context) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        for event in runtime.drain_events() {
            self.handle_web_event(event, ctx);
        }
    }

    /// 单条事件落地。三段顺序就是原来循环体的顺序：
    /// ① 持有串口的 `RefCell` 借用，就地改 UI 状态，同时把需要 `&mut self` 的收尾动作攒进局部变量；
    /// ② `drop(serial)` 释放借用；
    /// ③ 按「主题 → 插件安装 → 市场插件 → Lua 文件回执 → Lua 串口回执」补做收尾。
    ///
    /// ②→③ 的先后不能改：③ 里的方法会在部分路径上再次 `borrow_mut` 串口，借用没释放就当场 panic。
    /// ③ 内部的顺序也不能改：它和 ① 各条 arm 都会写 `serial.status`，后写的覆盖先写的。
    fn handle_web_event(&mut self, event: WebAppEvent, ctx: &egui::Context) {
        let mut plugin_lua_file_resolution = None;
        let mut plugin_lua_serial_resolution = None;
        let mut plugin_files = None;
        let mut marketplace_files = None;
        let mut theme_import = None;
        let mut serial = self.serial.borrow_mut();
        match event {
            WebAppEvent::TaskStateChanged(snapshot) if snapshot.kind == "refresh_ports" => {}
            WebAppEvent::TaskStateChanged(snapshot) => {
                serial.status = snapshot.message;
            }
            WebAppEvent::RecordingChanged
            | WebAppEvent::PluginsChanged
            | WebAppEvent::MarketplaceChanged
            | WebAppEvent::UpdateChanged => {}
            WebAppEvent::RecordingExportReady {
                id,
                name,
                content,
                incomplete,
            } => {
                finish_web_recording_export(
                    self.runtime.as_ref(),
                    &mut serial,
                    id,
                    name,
                    content,
                    incomplete,
                );
            }
            WebAppEvent::ReplayChanged { rebuild } => {
                if rebuild {
                    self.terminal_panel.clear();
                    self.bottom_log_panel.clear();
                    self.dynamic_panels.clear_charts();
                }
                self.replay_panel.set_load_pending(false);
                serial.status = "回放状态已更新".to_owned();
            }
            WebAppEvent::TerminalCleared => {
                self.terminal_panel.clear();
                serial.status = "终端已清空".to_owned();
            }
            WebAppEvent::TextLoaded {
                kind, name, text, ..
            } if kind == "replay_load" => {
                self.replay_panel.path = name.clone();
                if let Some(runtime) = self.runtime.as_ref() {
                    self.terminal_panel.clear();
                    self.bottom_log_panel.clear();
                    self.dynamic_panels.clear_charts();
                    dispatch_web_replay_load(
                        runtime,
                        &mut self.replay_panel,
                        &mut serial,
                        name,
                        text,
                    );
                }
            }
            WebAppEvent::TextLoaded {
                kind, name, text, ..
            } if kind == "theme_import" => {
                theme_import = Some((name, text));
                serial.status = "主题文件读取完成，正在应用".to_owned();
            }
            WebAppEvent::TextLoaded { id, kind, text, .. } if kind == "lua_host_file" => {
                plugin_lua_file_resolution = Some((id, true, text));
                serial.status = "Lua 插件文件读取完成".to_owned();
            }
            WebAppEvent::TextLoaded { kind, .. } => {
                serial.status = format!("异步任务完成：{kind}");
            }
            WebAppEvent::FilesLoaded { kind, files, .. } if kind == "plugin_import" => {
                serial.status = "插件文件读取完成，正在安装".to_owned();
                plugin_files = Some(files);
            }
            WebAppEvent::MarketplaceFilesLoaded {
                plugin_id, files, ..
            } => {
                serial.status = "市场插件文件读取完成，正在安装".to_owned();
                marketplace_files = Some((plugin_id, files));
            }
            WebAppEvent::FilesLoaded { kind, .. } => {
                serial.status = format!("异步任务完成：{kind}");
            }
            WebAppEvent::PortsRefreshed(ports) => {
                apply_web_refreshed_ports(&mut serial, ports);
            }
            WebAppEvent::PortRequested { id, port } => {
                plugin_lua_serial_resolution =
                    Some((id, Ok(PluginSerialDevice::from(port.clone()))));
                select_authorized_port(&mut serial, port);
                serial.status = "设备已授权，可连接".to_owned();
            }
            WebAppEvent::PortAttached(port) => {
                accept_web_attached_port(&mut serial, port);
            }
            WebAppEvent::PortDetached(port) => {
                forget_web_detached_port(&mut serial, port);
            }
            WebAppEvent::NetworkPortAdded(port) => {
                select_authorized_port(&mut serial, port);
                serial.status = "网络串口已添加，可连接".to_owned();
            }
            WebAppEvent::NetworkPortRemoved(port) => {
                forget_web_network_port(&mut serial, port);
            }
            WebAppEvent::Connected { port } => {
                apply_web_connected(&mut serial, port);
            }
            WebAppEvent::Disconnected { port, manual } => {
                apply_web_disconnected(&mut serial, port, manual);
            }
            WebAppEvent::Sent { bytes, .. } => {
                serial.status = format!("发送已提交（{bytes} 字节）");
            }
            WebAppEvent::SignalsChanged { signal, value, .. } => {
                apply_web_signals(&mut serial, signal, value);
            }
            WebAppEvent::TerminalSettingsChanged {
                merge_window_ms,
                max_entries,
            } => {
                self.terminal_panel.merge_window_ms = merge_window_ms;
                self.terminal_panel.set_max_entries(max_entries);
            }
            WebAppEvent::TaskFailed { id, error } => {
                (plugin_lua_file_resolution, plugin_lua_serial_resolution) =
                    take_web_lua_resolutions(&self.plugins, id, &error, &error);
                if !handle_web_reconnect_task_failed(&mut serial, id, &error) {
                    serial.status = format!("操作失败：{error}");
                }
            }
            WebAppEvent::TaskCancelled { id } => {
                (plugin_lua_file_resolution, plugin_lua_serial_resolution) =
                    take_web_lua_resolutions(
                        &self.plugins,
                        id,
                        "Lua 插件文件读取已取消",
                        "Lua 插件串口授权已取消",
                    );
                if !handle_web_reconnect_task_cancelled(&mut serial, id) {
                    serial.status = "操作已取消".to_owned();
                }
            }
        }
        drop(serial);
        if let Some((_name, source)) = theme_import {
            self.apply_web_theme_import(source, ctx);
        }
        if let Some(files) = plugin_files {
            self.apply_web_plugin_files(files);
        }
        if let Some((plugin_id, files)) = marketplace_files {
            self.apply_web_marketplace_files(plugin_id, files);
        }
        if let Some((id, ok, result)) = plugin_lua_file_resolution {
            self.resolve_web_lua_file_request(id, ok, &result);
        }
        if let Some((id, result)) = plugin_lua_serial_resolution {
            self.resolve_web_lua_serial_request(id, result);
        }
    }

    fn poll_web_dynamic_file_requests(&mut self, ctx: &egui::Context) {
        let requests = self.dynamic_panels.drain_file_browse_requests();
        let Some(runtime) = self.runtime.as_ref() else {
            return;
        };
        let runtime = runtime.clone();
        for event in requests {
            let Payload::Json(value) = event.payload else {
                continue;
            };
            let Some(panel_id) = value.get("panel_id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(field_id) = value.get("field_id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(document) = web_sys::window().and_then(|window| window.document()) else {
                continue;
            };
            let Some(input) = web_file_input(&document) else {
                continue;
            };
            let panel_id = panel_id.to_owned();
            let field_id = field_id.to_owned();
            let runtime = runtime.clone();
            let repaint = ctx.clone();
            web_open_file_picker(&input, move |event: web_sys::Event| {
                let Some(input) = web_file_input_from_event(&event) else {
                    return;
                };
                let Some(file) = input.files().and_then(|files| files.get(0)) else {
                    return;
                };
                let path = file.name();
                let runtime = runtime.clone();
                let repaint = repaint.clone();
                let panel_id = panel_id.clone();
                let field_id = field_id.clone();
                spawn_local(async move {
                    let (text, error) = match JsFuture::from(file.text()).await {
                        Ok(value) => (value.as_string(), None),
                        Err(error) => (None, Some(format!("读取文件失败：{error:?}"))),
                    };
                    runtime.publish_event(Event::new(
                        topics::UI_FORM_FILE_SELECTED,
                        "ui",
                        Direction::Internal,
                        Payload::Json(serde_json::json!({
                            "panel_id": panel_id,
                            "field_id": field_id,
                            "path": path,
                            "text": text,
                            "error": error,
                        })),
                    ));
                    repaint.request_repaint();
                });
            });
        }
    }

    fn tick_web_periodic_send(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        let application_connected = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.query_transport().connected);
        let mut command = None;
        let mut history = None;
        let mut persist = false;
        {
            let mut serial = self.serial.borrow_mut();
            if !serial.periodic_enabled {
                return;
            }

            let interval_ms = match serial.periodic_interval_ms.trim().parse::<f64>() {
                Ok(value) if value > 0.0 => value,
                _ => {
                    serial.periodic_enabled = false;
                    serial.periodic_next_at = None;
                    serial.send_error = Some("周期发送间隔必须 > 0ms".to_owned());
                    persist = true;
                    0.0
                }
            };
            if interval_ms <= 0.0 {
                // Persist after releasing the RefCell borrow below.
            } else if application_connected.is_none() {
                serial.periodic_enabled = false;
                serial.periodic_next_at = None;
                serial.send_error = Some("周期发送已停止：目标串口未打开".to_owned());
                persist = true;
            } else if serial.send_input.is_empty() {
                serial.periodic_enabled = false;
                serial.periodic_next_at = None;
                serial.send_error = Some("周期发送已停止：输入为空".to_owned());
                persist = true;
            } else {
                let interval = interval_ms / 1000.0;
                let next_at = serial.periodic_next_at.unwrap_or(now + interval);
                serial.periodic_next_at = Some(next_at);
                if now >= next_at {
                    let port = application_connected
                        .clone()
                        .expect("application transport was checked");
                    command = Some(send_plan::send_command(
                        port.as_str(),
                        &serial.send_input,
                        serial.tx_hex,
                        serial.line_ending.suffix(),
                        serial.hex_strict,
                    ));
                    history = Some(serial.send_input.clone());
                    serial.periodic_send_count = serial.periodic_send_count.saturating_add(1);
                    serial.periodic_next_at = Some(now + interval);
                }
            }
        }

        if let Some(command) = command {
            self.dispatch_serial(command, ctx);
        }
        if let Some(text) = history {
            record_web_send_history(&mut self.serial.borrow_mut(), text);
            persist = true;
        }
        if persist {
            self.persist_settings();
        }
        if self.serial.borrow().periodic_enabled {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// 尽力把命令交给 Application 运行时：与 `dispatch_serial` 不同，它不写状态栏、
    /// 也不请求重绘，运行时缺位或命令报错时一律静默忽略。用于那些"同步刷新共享
    /// 状态、结果由后续事件回灌"的设置类命令。
    fn dispatch_without_status(&self, command: AppCommand) {
        if let Some(runtime) = self.runtime.as_ref() {
            let _ = runtime.dispatch(command);
        }
    }

    /// 把命令结果原样写进状态栏：Pending 用运行时给的文案，Done 保持不动，Err 写错误
    /// 本身（不套 `dispatch_serial` 的"操作失败："前缀，市场与更新面板依赖原文展示）。
    /// 运行时缺位时什么都不做。
    fn dispatch_command_to_status(&self, command: AppCommand) {
        let Some(runtime) = self.runtime.as_ref() else {
            return;
        };
        match runtime.dispatch(command) {
            Ok(CommandOutcome::Pending { message, .. }) => {
                self.serial.borrow_mut().status = message;
            }
            Ok(CommandOutcome::Done) => {}
            Err(error) => self.serial.borrow_mut().status = error,
        }
    }

    fn poll_web_key_recording(&mut self, ctx: &egui::Context) {
        let Some(command_id) = self.key_recording.clone() else {
            return;
        };
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.key_recording = None;
            return;
        }
        let captured = ctx.input(|input| {
            input.events.iter().find_map(|event| {
                let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                else {
                    return None;
                };
                if matches!(
                    key,
                    egui::Key::ControlLeft
                        | egui::Key::ControlRight
                        | egui::Key::ShiftLeft
                        | egui::Key::ShiftRight
                        | egui::Key::AltLeft
                        | egui::Key::AltRight
                ) {
                    return None;
                }
                Some((
                    format!("{key:?}"),
                    modifiers.command,
                    modifiers.shift,
                    modifiers.alt,
                ))
            })
        });
        let Some((key, ctrl, shift, alt)) = captured else {
            return;
        };

        let binding = KeyBinding::new(key, ctrl, shift, alt);
        self.keymap.remove_binding_everywhere(&binding);
        let mut bindings = self.keymap.get_bindings(&command_id);
        bindings.retain(|candidate| candidate != &binding);
        bindings.push(binding.clone());
        self.keymap.set_bindings(&command_id, bindings);
        self.key_recording = None;
        self.persist_settings();
        self.serial.borrow_mut().status = format!(
            "{} 快捷键已更新为 {}",
            web_keymap_title(&command_id),
            binding.display()
        );
        ctx.request_repaint();
    }

    fn dispatch_web_keymap(&mut self, ctx: &egui::Context) {
        if self.key_recording.is_some() {
            return;
        }
        let keymap = self.keymap.clone();
        let command_id = ctx.input(|input| {
            keymap.bindings.iter().find_map(|(command_id, bindings)| {
                bindings
                    .iter()
                    .any(|binding| {
                        input.events.iter().any(|event| {
                            let egui::Event::Key {
                                key,
                                pressed: true,
                                modifiers,
                                ..
                            } = event
                            else {
                                return false;
                            };
                            format!("{key:?}") == binding.key
                                && modifiers.command == binding.ctrl
                                && modifiers.shift == binding.shift
                                && modifiers.alt == binding.alt
                        })
                    })
                    .then_some(command_id.clone())
            })
        });
        let Some(command_id) = command_id else {
            return;
        };

        self.execute_web_command(&command_id, ctx);
    }

    fn execute_web_command(&mut self, command_id: &str, ctx: &egui::Context) {
        match command_id {
            CMD_SEND => self.send_web_current(ctx),
            CMD_REFRESH_PORTS => self.dispatch_serial(AppCommand::RefreshPorts, ctx),
            CMD_OPEN_PORT => {
                let (connected, selected, settings) = {
                    let serial = self.serial.borrow();
                    (
                        self.runtime
                            .as_ref()
                            .and_then(|runtime| runtime.query_transport().connected)
                            .or_else(|| serial.connected.clone()),
                        serial.selected_port.clone(),
                        serial.settings,
                    )
                };
                let command = connected
                    .map(|port| AppCommand::Disconnect { port })
                    .or_else(|| selected.map(|port| AppCommand::Connect { port, settings }));
                if let Some(command) = command {
                    self.dispatch_serial(command, ctx);
                }
            }
            CMD_RECONNECT_PORT => {
                let port = self
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.query_transport().connected)
                    .or_else(|| self.serial.borrow().connected.clone());
                if let Some(port) = port {
                    self.dispatch_serial(AppCommand::Reconnect { port }, ctx);
                }
            }
            CMD_CLEAR_TERMINAL => {
                self.dispatch_serial(AppCommand::ClearTerminal, ctx);
            }
            CMD_TOGGLE_BOTTOM_PANEL => {
                let visible = self.panels.bottom_visible();
                self.panels.set_bottom_visible(!visible);
                self.layout_dirty = true;
            }
            CMD_TOGGLE_RIGHT_DOCK => {
                let visible = self.panels.right_visible();
                self.panels.set_right_visible(!visible);
                self.layout_dirty = true;
            }
            crate::shared_keymap::CMD_START_RECORDING => {
                let running = self
                    .runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.query_recording().stats.running);
                if running {
                    self.stop_web_recording(false, None);
                } else {
                    self.start_web_recording();
                }
            }
            crate::shared_keymap::CMD_ADD_BOOKMARK => {
                let position_ms = self.web_replay_status().position_ms;
                self.dispatch_web_replay(
                    AppCommand::AddReplayBookmark {
                        name: Some(format!("书签 {position_ms}")),
                    },
                    false,
                );
                self.serial.borrow_mut().status = format!("已添加回放书签：{position_ms} ms");
            }
            CMD_COMMAND_PALETTE => {
                self.command_palette_open = true;
                self.command_palette_query.clear();
                self.command_palette_selected = None;
            }
            _ => {
                let Some((plugin_id, plugin_command_id)) = command_id.split_once(':') else {
                    return;
                };
                self.execute_web_plugin_command(
                    plugin_id.to_owned(),
                    plugin_command_id.to_owned(),
                    serde_json::json!({
                        "source": "web_command_palette",
                        "command": plugin_command_id,
                    }),
                );
                self.poll_web_lua_host_requests();
            }
        }
    }

    fn send_web_current(&mut self, ctx: &egui::Context) {
        let (command, history) = {
            let mut serial = self.serial.borrow_mut();
            let connected = self
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.query_transport().connected)
                .or_else(|| serial.connected.clone());
            let Some(port) = connected else {
                serial.send_error = Some("请先连接串口".to_owned());
                return;
            };
            let input = serial.send_input.trim().to_owned();
            if input.is_empty() {
                serial.send_error = Some("发送内容不能为空".to_owned());
                return;
            }
            if serial.tx_hex
                && let Some(error) = web_hex_error(self.runtime.as_ref(), &input, serial.hex_strict)
            {
                serial.send_error = Some(error);
                return;
            }
            // 载荷规则与 native 同一处：文本追加换行后缀、HEX 不追加。
            // 上面的 `web_hex_error` 只是提前给红字提示，不决定投递内容。
            let command = send_plan::send_command(
                port.as_str(),
                &serial.send_input,
                serial.tx_hex,
                serial.line_ending.suffix(),
                serial.hex_strict,
            );
            (command, serial.send_input.clone())
        };
        record_web_send_history(&mut self.serial.borrow_mut(), history);
        self.serial.borrow_mut().send_error = None;
        self.dispatch_serial(command, ctx);
        self.persist_settings();
    }
}

// ── poll_web_events 的分段实现 ──
//
// 下面每个私有函数对应 `handle_web_event` 里的一条 match arm，**自上而下就是 arm 的顺序**。
// 拆出来只是为了让那条 `match` 读起来像一张分发表，不改变任何判定顺序与文案。
//
// 它们都在串口的 `RefCell` 借用仍被持有时被调用，所以参数只收具体字段的借用
// （`&mut WebSerialState`、`&mut ReplayPanel`……），不收 `&mut self`——整块 `&mut self`
// 会和 `self.serial` 的借用在借用检查器里打架。需要 `&mut self` 的收尾动作（主题导入、
// 插件安装、Lua 回执）留在 `handle_web_event` 的 `drop(serial)` 之后，别往这里搬。

/// 录制导出的文本已就绪，三步：浏览器落盘 → 把同一个结果回执给运行时 → 按结果写状态文案。
///
/// 回执传的是 `result.clone()`，原件留给文案，所以运行时与界面看到的必然是同一次落盘。
/// 运行时还没建好（`runtime` 为 `None`）时只落盘、不回执，文案照写。
fn finish_web_recording_export(
    runtime: Option<&WebRuntime>,
    serial: &mut WebSerialState,
    id: TaskId,
    name: String,
    content: String,
    incomplete: bool,
) {
    let result = download_text_file(&name, "application/x-ndjson", content);
    if let Some(runtime) = runtime {
        runtime.finish_recording_export(id, result.clone());
    }
    serial.status = match result {
        Ok(()) if incomplete => "录制已停止并导出（数据不完整）".to_owned(),
        Ok(()) => "录制已停止并导出".to_owned(),
        Err(error) => format!("录制导出失败：{error}"),
    };
}

/// 回放 JSONL 文本读好了：派发后台解析命令，按 Pending / Done / Err 三条路径回填面板与文案。
///
/// 只有 `Pending` 才点亮面板的「加载中」，关掉它的是之后的 `ReplayChanged` 事件；
/// `Done` 不动 `load_pending`，只有 `Err` 才把错误文本挂到面板上。
/// 三处数据视图的清空留在调用方那条 arm 里、且早于本函数：先清旧回放，再提交新文件。
fn dispatch_web_replay_load(
    runtime: &WebRuntime,
    replay_panel: &mut ReplayPanel,
    serial: &mut WebSerialState,
    name: String,
    text: String,
) {
    match runtime.dispatch(AppCommand::LoadReplayText { name, text }) {
        Ok(CommandOutcome::Pending { .. }) => {
            replay_panel.set_load_pending(true);
            serial.status = "回放文件读取完成，正在后台解析".to_owned();
        }
        Ok(_) => serial.status = "回放文件已提交加载".to_owned(),
        Err(error) => {
            replay_panel.message = Some(error.clone());
            serial.status = format!("回放加载失败：{error}");
        }
    }
}

/// 连接成功：丢掉待重连记录、把选中项跟到这台端口，最后落一条带串口参数的文案。
///
/// 带参数的文案必须排在最后：`select_web_port_state` 会按端口档位改掉 `serial.settings`，
/// 先算文案就会显示连接前的旧波特率。
fn apply_web_connected(serial: &mut WebSerialState, port: PortId) {
    serial.reconnect = None;
    select_web_port_state(serial, Some(port.clone()));
    serial.connected = Some(port.clone());
    serial.status = format!("已连接 {port} @ {}", settings_label(serial.settings));
}

/// 断开：只有断的确实是当前连接端口才清 `connected`；非手动断开（设备自己掉的）且开了
/// 自动重连时，拿列表里的描述符挂上重连记录——设备重插可能改名，靠
/// `WebReconnectState::matches` 才认得出还是同一台机器。
/// 状态文案无条件写，与是否挂上重连记录无关。
fn apply_web_disconnected(serial: &mut WebSerialState, port: PortId, manual: bool) {
    if serial.connected.as_ref() == Some(&port) {
        serial.connected = None;
        if serial.auto_reconnect && !manual {
            let descriptor = serial.ports.iter().find(|item| item.id == port).cloned();
            serial.reconnect = Some(web_reconnect_state(port.clone(), descriptor));
        }
    }
    serial.status = "设备已断开".to_owned();
}

/// 信号线回写：Dtr / Rts 各点亮一个布尔，随后统一落一条 `{:?} 已更新` 文案。
/// `match` 覆盖 `SignalKind` 的全部变体，运行时新增一根线时这里会编译不过，提醒补赋值。
fn apply_web_signals(serial: &mut WebSerialState, signal: SignalKind, value: bool) {
    match signal {
        SignalKind::Dtr => serial.dtr = value,
        SignalKind::Rts => serial.rts = value,
    }
    serial.status = format!("{signal:?} 已更新");
}

/// Lua 宿主两类挂起请求的迟到终态：`(文件读取回执, 串口授权回执)`。
type WebLuaTaskResolutions = (
    Option<(TaskId, bool, String)>,
    Option<(TaskId, Result<PluginSerialDevice, String>)>,
);

/// 任务失败与任务取消这两条终态都可能属于 Lua 宿主挂起的请求：文件读取与串口授权各查一张表，
/// 命中哪张就把这条终态转成对应回执，交给 `handle_web_event` 在串口借用释放后送回宿主。
///
/// 两张表都要查，命中一张也不能提前返回（一个任务 id 只会落在其中一张里，但查之前无从判断）。
/// 未命中的那张表不产出回执、也不克隆消息文本，所以 `file_message` / `serial_message`
/// 只在真需要回执时才变成 `String`。
fn take_web_lua_resolutions(
    plugins: &WebPluginState,
    id: TaskId,
    file_message: &str,
    serial_message: &str,
) -> WebLuaTaskResolutions {
    let file = plugins
        .pending_lua_file_requests
        .borrow()
        .contains_key(&id)
        .then(|| (id, false, file_message.to_owned()));
    let lua_serial = plugins
        .pending_lua_serial_requests
        .borrow()
        .contains_key(&id)
        .then(|| (id, Err(serial_message.to_owned())));
    (file, lua_serial)
}

impl DockHost for WorkbenchApp {
    fn panels(&mut self) -> &mut PanelManager {
        &mut self.panels
    }

    fn panels_ref(&self) -> &PanelManager {
        &self.panels
    }

    fn panel_registry(&self) -> &PanelRegistry {
        &self.panel_registry
    }

    fn render_panel(&mut self, ui: &mut egui::Ui, id: &PanelId) {
        self.panels.active_tab = id.clone();
        if !self.panel_registry.is_available(id) {
            ui.vertical_centered(|ui| {
                ui.add_space(32.0);
                ui.heading(self.panel_registry.title(id));
                ui.label("当前运行环境没有提供这个面板所需的平台能力");
                ui.small("这不是布局占位；请检查浏览器权限、设备能力或插件运行状态。");
            });
            return;
        }
        match self.panel_registry.kind_for(id) {
            Some(PanelKind::Builtin(BuiltinPanel::Devices)) => self.serial_ui(ui),
            Some(PanelKind::Builtin(BuiltinPanel::Replay)) => {
                egui::ScrollArea::vertical()
                    .id_salt("scroll-replay")
                    .show(ui, |ui| self.web_replay_panel_ui(ui));
            }
            Some(PanelKind::Builtin(BuiltinPanel::Plugins)) => {
                egui::ScrollArea::vertical()
                    .id_salt("scroll-plugins")
                    .show(ui, |ui| self.web_plugins_ui(ui));
            }
            Some(PanelKind::Builtin(BuiltinPanel::Chart)) => {
                let started = self.perf.begin_frame();
                self.chart_panel.ui(ui);
                self.perf.record_chart_render(started);
            }
            Some(PanelKind::Builtin(BuiltinPanel::Terminal)) => {
                let started = self.perf.begin_frame();
                let port_aliases: HashMap<String, String> = self
                    .serial
                    .borrow()
                    .port_aliases
                    .clone()
                    .into_iter()
                    .collect();
                self.terminal_panel.set_port_aliases(&port_aliases);
                self.terminal_panel.ui(ui);
                self.perf.record_terminal_render(started);
            }
            Some(PanelKind::Builtin(BuiltinPanel::Sender)) => self.shared_sender_ui(ui),
            Some(PanelKind::Builtin(BuiltinPanel::Logs)) => {
                let started = self.perf.begin_frame();
                self.bottom_log_panel.ui(ui);
                self.perf.record_log_render(started);
            }
            Some(PanelKind::Builtin(BuiltinPanel::Settings)) => {
                egui::ScrollArea::vertical()
                    .id_salt("scroll-settings")
                    .show(ui, |ui| self.settings_ui(ui));
            }
            Some(PanelKind::Dynamic { suffix }) => {
                if self.dynamic_panels.contains(&suffix) {
                    let started = self.perf.begin_frame();
                    let _ = egui::ScrollArea::vertical()
                        .show(ui, |ui| self.dynamic_panels.ui_body(ui, &suffix));
                    self.perf.record_chart_render(started);
                } else {
                    ui.colored_label(theme::red(), format!("动态面板不存在：{suffix}"));
                }
            }
            _ => {
                ui.colored_label(theme::red(), format!("面板不存在：{id}"));
            }
        }
    }

    fn mark_layout_dirty(&mut self) {
        self.layout_dirty = true;
    }
}

impl AppShellHost for WorkbenchApp {
    fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        let recording_status = self.runtime.as_ref().map(WebRuntime::query_recording);
        let recording_running = recording_status
            .as_ref()
            .is_some_and(|status| status.stats.running);
        let transport_view = self.runtime.as_ref().map(WebRuntime::query_transport);
        let (
            connected,
            connecting,
            selected,
            ports,
            port_aliases,
            settings,
            recording,
            collapsed,
            reconnect,
        ) = {
            let serial = self.serial.borrow();
            (
                web_connected_port(transport_view.as_ref(), &serial),
                transport_view.as_ref().is_some_and(|view| view.connecting),
                serial.selected_port.clone(),
                transport_view
                    .as_ref()
                    .map(|view| view.ports.clone())
                    .unwrap_or_else(|| serial.ports.clone()),
                serial.port_aliases.clone(),
                transport_view
                    .as_ref()
                    .map(|view| view.settings)
                    .unwrap_or(serial.settings),
                recording_running,
                serial.top_bar_serial_collapsed,
                serial.reconnect.as_ref().map(|pending| {
                    (
                        pending.port.to_string(),
                        pending.attempts,
                        pending.next_attempt_at,
                    )
                }),
            )
        };
        let selected_id = selected.as_ref().map(ToString::to_string);
        let connected_id = connected.as_ref().map(ToString::to_string);
        let serial_ports = web_top_bar_port_items(
            &ports,
            connected_id.as_deref(),
            connecting,
            selected.as_ref(),
            reconnect.as_ref().map(|(port, _, _)| port.as_str()),
        );
        let mut selected_port = selected_id;
        let mut collapsed_state = collapsed;
        let mut serial_actions = Vec::new();
        let mut cancel_reconnect = false;
        let reconnect_port = reconnect
            .as_ref()
            .map(|(port, _, _)| PortId::new(port.clone()));
        ui.horizontal_wrapped(|ui| {
            let mut view = SerialTopBarView {
                ports: &serial_ports,
                aliases: &port_aliases,
                selected: &mut selected_port,
                connected: connected_id.as_deref(),
                connecting,
                collapsed: &mut collapsed_state,
                settings,
                show_request_port: self
                    .runtime
                    .as_ref()
                    .is_some_and(WebRuntime::serial_supported),
            };
            serial_actions = SerialPanel::top_bar_contents_ui(ui, &mut view);

            if let Some((port, attempts, next_attempt_at)) = reconnect
                && web_reconnect_chip_ui(ui, &port, attempts, next_attempt_at)
            {
                cancel_reconnect = true;
            }
            ui.separator();
            self.web_ui_contribution_slot(ui, "top_bar.left");
            let record_button = if recording {
                design::button(ui, ICON_STOP, "停止录制", design::ButtonKind::Danger)
            } else {
                design::button(
                    ui,
                    ICON_FIBER_MANUAL_RECORD,
                    "开始录制",
                    design::ButtonKind::Ghost,
                )
            };
            if record_button.clicked() {
                if recording {
                    self.stop_web_recording(false, None);
                } else {
                    self.start_web_recording();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.web_ui_contribution_slot(ui, "top_bar.right");
            });
        });
        if collapsed_state != collapsed {
            self.serial.borrow_mut().top_bar_serial_collapsed = collapsed_state;
            self.persist_settings();
        }
        if cancel_reconnect {
            self.cancel_web_reconnect(reconnect_port, ui.ctx());
        }
        if selected_port.as_deref() != selected.as_ref().map(PortId::as_str) {
            self.select_web_port(selected_port.clone().map(PortId::new));
        }
        if let Some(command) = serial_actions
            .into_iter()
            .next()
            .map(|action| web_top_bar_serial_command(action, settings))
        {
            self.dispatch_serial(command, ui.ctx());
        }
    }

    fn render_status_bar(&mut self, ui: &mut egui::Ui) {
        let recording_status = self.runtime.as_ref().map(WebRuntime::query_recording);
        let recording_stats = recording_status.as_ref().map(|status| &status.stats);
        let recording_running = recording_stats.is_some_and(|stats| stats.running);
        let recording_paused = recording_stats.is_some_and(|stats| stats.paused);
        let recording_events = recording_stats.map_or(0, |stats| stats.events_written);
        let recording_bytes = recording_stats.map_or(0, |stats| stats.bytes_written);
        let recording_error = recording_stats.and_then(|stats| stats.last_error.clone());
        let update_status = self
            .runtime
            .as_ref()
            .map(WebRuntime::query_update)
            .unwrap_or_default();
        let transport_view = self.runtime.as_ref().map(WebRuntime::query_transport);
        let (
            connected,
            connecting,
            selected,
            ports,
            port_aliases,
            settings,
            dtr,
            rts,
            status,
            recording,
            paused,
            events,
            bytes,
            recorder_error,
        ) = {
            let serial = self.serial.borrow();
            (
                web_connected_port(transport_view.as_ref(), &serial),
                transport_view.as_ref().is_some_and(|view| view.connecting),
                serial.selected_port.clone(),
                transport_view
                    .as_ref()
                    .map(|view| view.ports.clone())
                    .unwrap_or_else(|| serial.ports.clone()),
                serial.port_aliases.clone(),
                transport_view
                    .as_ref()
                    .map(|view| view.settings)
                    .unwrap_or(serial.settings),
                serial.dtr,
                serial.rts,
                transport_view
                    .as_ref()
                    .filter(|view| !view.status.is_empty())
                    .map(|view| view.status.clone())
                    .unwrap_or_else(|| serial.status.clone()),
                recording_running,
                recording_paused,
                recording_events,
                recording_bytes,
                recording_error,
            )
        };
        let port_label = connected
            .as_ref()
            .or(selected.as_ref())
            .and_then(|id| ports.iter().find(|port| &port.id == id))
            .map(|port| web_port_display_name(port, &port_aliases))
            .unwrap_or_else(|| "串口已关闭".to_owned());
        self.push_web_overflow_notifications();
        let notifications = self.current_web_notifications();
        let status_view = StatusBarView {
            serial_color: if connecting {
                theme::yellow()
            } else if connected.is_some() {
                theme::green()
            } else {
                theme::text_secondary()
            },
            serial_label: web_serial_status_label(
                &port_label,
                connecting,
                connected.is_some(),
                settings,
            ),
            recording_color: if recording {
                theme::red()
            } else {
                theme::text_dimmed()
            },
            recording_label: web_recording_status_label(recording, paused, events, bytes),
            signals: connected.is_some().then_some(StatusSignalView { dtr, rts }),
        };
        let mut signal_action = None;
        ui.horizontal(|ui| {
            signal_action = status_bar_contents_ui(ui, &status_view).into_iter().next();
            self.web_ui_contribution_slot(ui, "status_bar.left");
            web_notification_status_ui(ui, &notifications);
            self.web_status_bar_right_ui(ui, &update_status, &status, recorder_error.as_deref());
        });
        if let Some(action) = signal_action
            && let Some(port) = connected
        {
            let command = match action {
                StatusBarAction::SetDtr { value } => AppCommand::SetDtr { port, value },
                StatusBarAction::SetRts { value } => AppCommand::SetRts { port, value },
            };
            self.dispatch_serial(command, ui.ctx());
        }
    }
}

impl eframe::App for WorkbenchApp {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        theme::bg_primary().to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let frame_started = self.perf.begin_frame();
        self.poll_loaded_settings(ui.ctx());
        self.poll_web_key_recording(ui.ctx());
        self.dispatch_web_keymap(ui.ctx());
        self.poll_web_export(ui.ctx());
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.tick();
        }
        if let Err(error) = self.web_lua.tick() {
            self.serial.borrow_mut().status = format!("Lua 任务调度失败：{error}");
        }
        self.poll_web_events(ui.ctx());
        self.tick_web_reconnect(ui.ctx());
        self.tick_web_periodic_send(ui.ctx());
        let plugin_started = self.perf.begin_frame();
        self.poll_web_plugins(ui.ctx());
        self.perf.record_plugin_callback(plugin_started);
        self.poll_web_lua_host_requests();
        self.poll_web_dynamic_file_requests(ui.ctx());
        let terminal_started = self.perf.begin_frame();
        let terminal_ingested = self.terminal_panel.ingest_pending();
        self.perf
            .record_terminal_ingest(terminal_started, terminal_ingested);
        let dynamic_ports = self
            .serial
            .borrow()
            .ports
            .iter()
            .map(|port| tool_panels::PortItem {
                port_name: port.id.to_string(),
            })
            .collect::<Vec<_>>();
        self.dynamic_panels.set_ports(&dynamic_ports);
        self.dynamic_panels.ingest(&mut self.panels);
        self.dynamic_panels.ingest_all_pending();
        self.panel_registry
            .sync_dynamic_panels(&self.dynamic_panels);
        if !self
            .panels
            .is_panel_visible(&PanelId::builtin(PANEL_DEVICES))
        {
            self.serial.borrow_mut().refresh_on_page_entry = true;
        }
        crate::shared_shell::show_shell(self, ui);
        let log_started = self.perf.begin_frame();
        let log_ingested = self.bottom_log_panel.ingest_pending();
        self.perf.record_log_ingest(log_started, log_ingested);
        self.web_command_palette_ui(ui.ctx());
        if let Some(format) = self.terminal_panel.take_export_request()
            && self.web_export.is_none()
            && let Some(runtime) = self.runtime.as_ref()
        {
            let task_id = runtime.begin_task("export_terminal", "正在准备导出终端");
            self.web_export = Some(WebExportState {
                task_id,
                stem: "terminal",
                format,
                job: WebExportJob::Terminal(self.terminal_panel.begin_export_cursor()),
                offset: 0,
                content: String::new(),
            });
        }
        if let Some(format) = self.bottom_log_panel.take_export_request()
            && self.web_export.is_none()
            && let Some(runtime) = self.runtime.as_ref()
        {
            let task_id = runtime.begin_task("export_log", "正在准备导出日志");
            self.web_export = Some(WebExportState {
                task_id,
                stem: "logs",
                format,
                job: WebExportJob::Logs(self.bottom_log_panel.begin_export_cursor()),
                offset: 0,
                content: String::new(),
            });
        }
        if self.layout_dirty {
            self.layout_dirty = false;
            self.persist_settings();
        }
        let recording_status = self.runtime.as_ref().map(WebRuntime::query_recording);
        let recording_stats = recording_status.as_ref().map(|status| &status.stats);
        let recorder = crate::web_perf::WebRecorderPerf {
            running: recording_stats.is_some_and(|stats| stats.running),
            queued_events: recording_stats.map_or(0, |stats| stats.backlog_events),
            queued_bytes: recording_stats.map_or(0, |stats| stats.backlog_bytes),
            seconds_behind: recording_stats.map_or(0.0, |stats| stats.seconds_behind),
            recorded_events: recording_stats.map_or(0, |stats| stats.events_written),
            recorded_bytes: recording_stats.map_or(0, |stats| stats.bytes_written),
            write_bytes_per_sec: 0,
            incomplete: recording_stats.is_some_and(|stats| stats.incomplete),
        };
        let bus_snapshot = self.runtime.as_ref().map(WebRuntime::perf_snapshot);
        self.perf.end_frame(frame_started, bus_snapshot, recorder);
    }
}

impl WorkbenchApp {
    fn web_keymap_commands(&self) -> Vec<WebPaletteCommand> {
        let mut commands = BUILTIN_KEYMAP_COMMANDS
            .iter()
            .map(|command| WebPaletteCommand {
                id: command.id.to_owned(),
                title: command.title.to_owned(),
            })
            .collect::<Vec<_>>();

        let plugin_view = self
            .runtime
            .as_ref()
            .map(WebRuntime::query_plugins)
            .unwrap_or_default();
        for summary in plugin_view.summaries {
            if matches!(summary.state, PluginStateView::Disabled) {
                continue;
            }
            let plugin_id = summary.id;
            let plugin_name = summary.name;
            commands.extend(summary.contributes.commands.into_iter().map(|command| {
                WebPaletteCommand {
                    id: format!("{plugin_id}:{}", command.id),
                    title: format!("{plugin_name}: {}", command.title),
                }
            }));
        }
        commands
    }

    fn web_palette_commands(&self) -> Vec<WebPaletteCommand> {
        let mut commands = self
            .web_keymap_commands()
            .into_iter()
            .filter(|command| command.id != CMD_COMMAND_PALETTE)
            .collect::<Vec<_>>();
        let running_plugin_ids = self
            .runtime
            .as_ref()
            .map(WebRuntime::query_plugins)
            .map(|view| {
                view.summaries
                    .into_iter()
                    .filter(|summary| summary.state == PluginStateView::Running)
                    .map(|summary| summary.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        commands.retain(|command| {
            !command.id.contains(':')
                || running_plugin_ids
                    .iter()
                    .any(|plugin_id| command.id.starts_with(&format!("{plugin_id}:")))
        });
        commands
    }

    /// 面板里可见的命令条目：先按标题过滤查询词，再按最近使用顺序排序。
    fn web_palette_entries(&self, query: &tool_panels::SearchQuery) -> Vec<WebPaletteCommand> {
        let mut entries = self
            .web_palette_commands()
            .into_iter()
            .filter(|command| query.is_empty() || query.matches(&command.title))
            .collect::<Vec<_>>();
        entries.sort_by_key(|command| {
            self.command_usage_order
                .iter()
                .position(|id| id == &command.id)
                .unwrap_or(usize::MAX)
        });
        entries
    }

    /// 让选中项与当前条目数对齐，并处理上下键；`count` 为 0 时不碰键盘，
    /// 避免对空列表取模。
    fn sync_web_palette_selection(&mut self, ctx: &egui::Context, count: usize) {
        if count == 0 {
            self.command_palette_selected = None;
            return;
        }
        if self
            .command_palette_selected
            .is_none_or(|index| index >= count)
        {
            self.command_palette_selected = Some(0);
        }
        if ctx.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
            let current = self.command_palette_selected.unwrap_or(0);
            self.command_palette_selected = Some((current + 1) % count);
        }
        if ctx.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
            let current = self.command_palette_selected.unwrap_or(0);
            self.command_palette_selected =
                Some(if current == 0 { count - 1 } else { current - 1 });
        }
    }

    fn web_command_palette_ui(&mut self, ctx: &egui::Context) {
        if !self.command_palette_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.command_palette_open = false;
            return;
        }

        let query = tool_panels::SearchQuery::new(&self.command_palette_query, false);
        let entries = self.web_palette_entries(&query);
        self.sync_web_palette_selection(ctx, entries.len());

        let mut open = true;
        let mut selected_command = None;
        let window = egui::Window::new("命令面板")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -100.0])
            .default_width(470.0)
            .frame(design::elevated_card())
            .show(ctx, |ui| {
                let response = ui
                    .horizontal(|ui| {
                        ui.label(design::icon_only(
                            ICON_SEARCH,
                            theme::text_secondary(),
                            19.0,
                        ));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.command_palette_query)
                                .hint_text("搜索命令…")
                                .desired_width(f32::INFINITY)
                                .frame(egui::Frame::NONE),
                        )
                        .on_hover_text(
                            tool_panels::SearchQuery::hover_hint(
                                query.used_invalid_regex_fallback(),
                            ),
                        )
                    })
                    .inner;
                if !response.has_focus() {
                    response.request_focus();
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        if entries.is_empty() {
                            ui.colored_label(theme::text_dimmed(), "无匹配命令");
                        }
                        for (index, command) in entries.iter().enumerate() {
                            let shortcut = self
                                .keymap
                                .get_bindings(&command.id)
                                .first()
                                .map(KeyBinding::display)
                                .unwrap_or_default();
                            let selected = self.command_palette_selected == Some(index);
                            let label = if shortcut.is_empty() {
                                command.title.to_owned()
                            } else {
                                format!("{}    {}", command.title, shortcut)
                            };
                            if ui.selectable_label(selected, label).clicked() {
                                selected_command = Some(command.id.clone());
                            }
                        }
                    });
            });

        if let Some(inner) = window
            && ctx.input(|input| {
                input.pointer.any_click()
                    && input
                        .pointer
                        .hover_pos()
                        .is_some_and(|position| !inner.response.rect.contains(position))
            })
        {
            open = false;
        }
        if selected_command.is_none()
            && ctx.input(|input| input.key_pressed(egui::Key::Enter))
            && let Some(index) = self.command_palette_selected
            && let Some(command) = entries.get(index)
        {
            selected_command = Some(command.id.clone());
        }
        if let Some(command_id) = selected_command {
            self.command_usage_order.retain(|id| id != &command_id);
            self.command_usage_order.insert(0, command_id.clone());
            self.command_palette_open = false;
            self.command_palette_query.clear();
            self.command_palette_selected = None;
            self.persist_settings();
            self.execute_web_command(&command_id, ctx);
        } else {
            self.command_palette_open = open;
        }
    }

    fn poll_web_export(&mut self, ctx: &egui::Context) {
        const EXPORT_SCAN_BATCH: usize = 512;
        if let (Some(runtime), Some(state)) = (self.runtime.as_ref(), self.web_export.as_ref())
            && !runtime.task_is_active(state.task_id)
        {
            self.web_export = None;
            self.serial.borrow_mut().status = "导出任务已取消".to_owned();
            return;
        }
        let Some(state) = self.web_export.as_mut() else {
            return;
        };
        let format = state.format;
        let (chunk, done, exported) = match &mut state.job {
            WebExportJob::Terminal(cursor) => {
                self.terminal_panel
                    .export_cursor_chunk(cursor, format, EXPORT_SCAN_BATCH)
            }
            WebExportJob::Logs(cursor) => {
                self.bottom_log_panel
                    .export_cursor_chunk(cursor, format, EXPORT_SCAN_BATCH)
            }
        };
        state.content.push_str(&chunk);
        state.offset += exported;
        if done {
            let Some(state) = self.web_export.take() else {
                return;
            };
            let result = download_export(state.stem, state.format, state.content);
            if let Some(runtime) = self.runtime.as_ref() {
                match result {
                    Ok(()) => {
                        runtime.complete_task(state.task_id, "导出完成");
                    }
                    Err(error) => {
                        runtime.fail_task(state.task_id, error.clone());
                        self.serial.borrow_mut().status = format!("导出失败：{error}");
                    }
                }
            }
        } else {
            let target = if state.stem == "terminal" {
                "终端"
            } else {
                "日志"
            };
            let progress = format!("正在导出 {target}：已写入 {} 条", state.offset);
            if let Some(runtime) = self.runtime.as_ref() {
                runtime.update_task(state.task_id, progress.clone());
            }
            self.serial.borrow_mut().status = progress;
            ctx.request_repaint();
        }
    }

    /// 状态栏右半区（右对齐）：更新提示、右侧贡献点、运行状态文本与录制错误。
    fn web_status_bar_right_ui(
        &mut self,
        ui: &mut egui::Ui,
        update_status: &tool_application::updater::UpdateStatusView,
        status: &str,
        recorder_error: Option<&str>,
    ) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if update_status.checking {
                ui.spinner();
                ui.small("检查更新…");
            } else if let Some(info) = update_status.info.as_ref()
                && web_version_is_newer(&info.version, env!("CARGO_PKG_VERSION"))
            {
                if ui.small_button(format!("更新 v{}", info.version)).clicked() {
                    open_web_url(&info.download_url);
                }
            } else if let Some(error) = update_status.error.as_deref() {
                ui.colored_label(theme::yellow(), "更新检查失败")
                    .on_hover_text(error);
            }
            self.web_ui_contribution_slot(ui, "status_bar.right");
            ui.label(status);
            if let Some(error) = recorder_error {
                ui.colored_label(theme::red(), "录制错误")
                    .on_hover_text(error);
            }
        });
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let nav_id = ui.id().with("settings_category");
        let mut category = ui
            .ctx()
            .data_mut(|data| data.get_persisted::<usize>(nav_id))
            .unwrap_or(0)
            .min(4);
        design::elevated_card().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (index, icon, label) in SETTINGS_NAV_ITEMS {
                    if settings_nav_button(ui, category == index, icon, label).clicked() {
                        category = index;
                    }
                }
            });
        });
        ui.ctx()
            .data_mut(|data| data.insert_persisted(nav_id, category));
        ui.add_space(8.0);

        if category == 0 {
            self.render_workspace_settings_card(ui);
            self.render_appearance_settings_card(ui);
        }

        if category == 1 {
            ui.add_space(8.0);
            design::card().show(ui, |ui| {
                web_card_header(ui, ICON_NETWORK_CHECK, "网络");
                ui.label("浏览器网络请求遵循当前浏览器的代理和安全策略。");
                ui.label("串口权限与自动重连在“串口”面板中配置。");
            });
            self.render_data_settings_card(ui);
        }

        if category == 2 {
            self.render_keymap_editor(ui);
        }

        if category == 3 {
            ui.add_space(8.0);
            self.web_plugin_settings_ui(ui);
        }

        if category == 4 {
            let update_status = self
                .runtime
                .as_ref()
                .map(WebRuntime::query_update)
                .unwrap_or_default();
            ui.add_space(8.0);
            self.render_restore_defaults_row(ui);
            self.render_about_settings_card(ui, &update_status);
        }
    }

    fn render_workspace_settings_card(&mut self, ui: &mut egui::Ui) {
        design::card().show(ui, |ui| {
            web_card_header(ui, ICON_FOLDER, "工作区");
            ui.label("布局、主题和数据偏好会保存在当前浏览器的本地存储中。");
            ui.label("浏览器构建使用内嵌字体，不依赖本地文件系统。");
            ui.horizontal_wrapped(|ui| {
                ui.label("工作区布局");
                // 两步确认与 native 同一份规则（design::confirm_*）、同一个 salt。
                let armed = design::confirm_armed(ui, LAYOUT_RESET_CONFIRM_ID);
                let label = if armed {
                    "确认恢复布局"
                } else {
                    "恢复默认布局"
                };
                let color = if armed {
                    theme::orange()
                } else {
                    theme::text_primary()
                };
                let response = ui.button(egui::RichText::new(label).color(color));
                let clicked = response.clicked();
                if armed {
                    response
                        .on_hover_text("再次点击恢复默认布局，3 秒内有效（Esc 或点击别处取消）");
                } else {
                    response.on_hover_text("仅重置面板位置，不修改主题、串口和插件状态");
                }
                if design::confirm_click(ui, LAYOUT_RESET_CONFIRM_ID, clicked) {
                    self.panels.reset_tiles_layout();
                    self.layout_dirty = true;
                    self.push_web_notification(
                        "settings",
                        WebNotificationLevel::Info,
                        "已恢复默认布局，运行中的插件面板已保留",
                    );
                }
            });
        });
    }

    fn render_appearance_settings_card(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        design::card().show(ui, |ui| {
            web_card_header(ui, ICON_PALETTE, "外观");
            ui.horizontal_wrapped(|ui| {
                ui.label("界面主题");
                egui::ComboBox::from_id_salt("app-theme")
                    .selected_text(self.ui_theme.label())
                    .show_ui(ui, |ui| {
                        for candidate in theme::AppTheme::ALL {
                            if ui
                                .selectable_label(self.ui_theme == candidate, candidate.label())
                                .clicked()
                            {
                                self.ui_theme = candidate;
                                self.theme_source = None;
                                apply_web_theme(ui.ctx(), candidate);
                                self.persist_settings();
                            }
                        }
                        if self.theme_source.is_some()
                            && ui
                                .selectable_label(
                                    self.ui_theme == theme::AppTheme::Custom,
                                    theme::AppTheme::Custom.label(),
                                )
                                .clicked()
                        {
                            self.ui_theme = theme::AppTheme::Custom;
                            apply_web_theme(ui.ctx(), self.ui_theme);
                            self.persist_settings();
                        }
                    });
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("导入 JSON 主题").clicked() {
                    self.request_web_theme_file(ui.ctx());
                }
                if self.theme_source.is_some()
                    && ui
                        .small_button("清除自定义主题")
                        .on_hover_text("切换回内置主题并删除浏览器中保存的主题文本")
                        .clicked()
                {
                    self.theme_source = None;
                    self.ui_theme = theme::AppTheme::default();
                    apply_web_theme(ui.ctx(), self.ui_theme);
                    self.persist_settings();
                }
                if self.ui_theme == theme::AppTheme::Custom {
                    ui.small("当前主题来自浏览器导入的 JSON 文件");
                }
            });
            let mut font_size = self.terminal_panel.font_size;
            if ui
                .add(
                    egui::Slider::new(&mut font_size, 10.0..=24.0)
                        .step_by(1.0)
                        .text("等宽字体大小")
                        .suffix("px"),
                )
                .changed()
            {
                self.terminal_panel.font_size = font_size;
                self.bottom_log_panel.font_size = font_size;
                self.persist_settings();
            }
            ui.add_space(4.0);
            let mut bottom_visible = self.panels.bottom_visible();
            if ui.checkbox(&mut bottom_visible, "显示底部面板").changed() {
                self.panels.set_bottom_visible(bottom_visible);
                self.persist_settings();
            }
        });
    }

    fn render_data_settings_card(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        design::card().show(ui, |ui| {
            web_card_header(ui, ICON_TUNE, "数据");
            let (changed, terminal_max_entries, log_max_entries) = {
                let mut view = DataSettingsView {
                    merge_window_ms: &mut self.terminal_panel.merge_window_ms,
                    terminal_max_entries: &mut self.terminal_panel.max_entries,
                    log_max_entries: &mut self.bottom_log_panel.max_entries,
                };
                let changed = data_settings_ui(ui, &mut view);
                (changed, *view.terminal_max_entries, *view.log_max_entries)
            };
            if changed {
                self.terminal_panel.set_max_entries(terminal_max_entries);
                self.bottom_log_panel.set_max_entries(log_max_entries);
                self.dispatch_without_status(AppCommand::SetTerminalMergeWindow {
                    ms: self.terminal_panel.merge_window_ms,
                });
                self.dispatch_without_status(AppCommand::SetTerminalMaxEntries {
                    max: terminal_max_entries,
                });
                self.persist_settings();
            }
        });
    }

    fn render_keymap_editor(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        design::card().show(ui, |ui| {
            web_card_header(ui, ICON_KEYBOARD, "快捷键");
            let commands = self.web_keymap_commands();
            let entries = commands
                .iter()
                .map(|command| KeymapEntry {
                    id: command.id.clone(),
                    title: command.title.clone(),
                    bindings: self
                        .keymap
                        .get_bindings(&command.id)
                        .iter()
                        .map(KeyBinding::display)
                        .collect::<Vec<_>>()
                        .join(", "),
                    recording: self.key_recording.as_deref() == Some(command.id.as_str()),
                })
                .collect::<Vec<_>>();
            for action in keymap_ui(ui, &entries) {
                match action {
                    KeymapAction::Record(command_id) => {
                        self.key_recording = Some(command_id);
                    }
                    KeymapAction::Clear(command_id) => {
                        self.keymap.set_bindings(&command_id, Vec::new());
                        self.persist_settings();
                    }
                    KeymapAction::RestoreDefaults => {
                        self.keymap = Keymap::default();
                        self.key_recording = None;
                        self.persist_settings();
                    }
                }
            }
        });
    }

    fn render_restore_defaults_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            // 与 native 同一个两步确认规则：一次点击清空全部浏览器端配置不可撤销。
            let armed = design::confirm_armed(ui, RESTORE_DEFAULTS_CONFIRM_ID);
            let label = if armed {
                "确认恢复默认设置"
            } else {
                "恢复所有默认设置"
            };
            let response = design::button(ui, ICON_RESTART_ALT, label, design::ButtonKind::Danger);
            let clicked = response.clicked();
            if armed {
                response.on_hover_text("再次点击恢复默认，3 秒内有效（Esc 或点击别处取消）");
            }
            if design::confirm_click(ui, RESTORE_DEFAULTS_CONFIRM_ID, clicked) {
                self.reset_web_settings(ui.ctx());
            }
        });
        ui.add_space(8.0);
    }

    fn render_about_settings_card(
        &mut self,
        ui: &mut egui::Ui,
        update_status: &tool_application::updater::UpdateStatusView,
    ) {
        design::card().show(ui, |ui| {
            web_card_header(ui, ICON_INFO, "关于");
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("硬件调试工作台 v{}", env!("CARGO_PKG_VERSION")));
                ui.hyperlink_to(REPOSITORY_URL, REPOSITORY_URL)
                    .on_hover_text("打开项目仓库");
                if ui.small_button("复制版本号").clicked() {
                    copy_text_with_feedback(
                        ui,
                        format!("v{}", env!("CARGO_PKG_VERSION")),
                        format!("已复制 v{}", env!("CARGO_PKG_VERSION")),
                    );
                }
            });
            ui.label("浏览器版本使用 Web Serial、localStorage 和 Blob 下载能力。");
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!update_status.checking, egui::Button::new("检查 Web 更新"))
                    .clicked()
                {
                    self.request_web_update_check(ui.ctx());
                }
                if update_status.checking {
                    ui.label("正在检查…");
                }
            });
            if let Some(error) = &update_status.error {
                ui.colored_label(theme::red(), error);
            }
            if let Some(info) = &update_status.info {
                let current = env!("CARGO_PKG_VERSION");
                if web_version_is_newer(&info.version, current) {
                    ui.label(format!("发现新版本 v{}（{}）", info.version, info.date));
                    if ui.button("打开下载页").clicked() {
                        open_web_url(&info.download_url);
                    }
                    for item in &info.changelog {
                        ui.small(format!("• {item}"));
                    }
                } else {
                    ui.label(format!("已是最新版本（v{current}）"));
                }
            }
        });
    }

    fn reset_web_settings(&mut self, ctx: &egui::Context) {
        let network_ports = {
            let mut serial = self.serial.borrow_mut();
            let network_ports = serial
                .network_ports
                .iter()
                .map(NetworkSerialConfig::port_id)
                .collect::<Vec<_>>();
            serial.selected_port = None;
            serial.settings = SerialSettings::default();
            serial.network_ports.clear();
            serial.port_aliases.clear();
            serial.port_groups.clear();
            serial.port_profiles.clear();
            serial.auto_reconnect = default_auto_reconnect();
            serial.top_bar_serial_collapsed = false;
            serial.reconnect = None;
            serial.tx_hex = false;
            serial.line_ending = WebLineEnding::None;
            serial.hex_strict = true;
            serial.send_history.clear();
            serial.periodic_interval_ms = default_periodic_interval_ms();
            network_ports
        };
        for port in network_ports {
            self.dispatch_without_status(AppCommand::RemoveNetworkPort { port });
        }
        self.panels.reset_tiles_layout();
        self.layout_dirty = true;
        self.ui_theme = theme::AppTheme::default();
        self.theme_source = None;
        self.keymap = Keymap::default();
        self.command_usage_order.clear();
        self.command_palette_open = false;
        self.command_palette_query.clear();
        self.command_palette_selected = None;
        self.terminal_panel.merge_window_ms = default_terminal_merge_window_ms();
        self.terminal_panel
            .set_max_entries(default_terminal_max_entries());
        self.dispatch_without_status(AppCommand::SetTerminalMergeWindow {
            ms: default_terminal_merge_window_ms(),
        });
        self.dispatch_without_status(AppCommand::SetTerminalMaxEntries {
            max: default_terminal_max_entries(),
        });
        self.bottom_log_panel
            .set_max_entries(default_terminal_max_entries());
        self.terminal_panel.font_size = default_font_size();
        self.bottom_log_panel.font_size = default_font_size();
        apply_web_theme(ctx, self.ui_theme);
        self.persist_settings();
        self.push_web_notification("settings", WebNotificationLevel::Warn, "已恢复默认设置");
    }

    fn web_send_toolbar_buttons(&self) -> Vec<SendToolbarButton> {
        let mut buttons = self
            .plugins
            .summaries()
            .into_iter()
            .filter(|summary| summary.state == PluginStateView::Running)
            .flat_map(|summary| {
                let plugin_id = summary.id;
                let commands = summary.contributes.commands;
                summary
                    .contributes
                    .ui
                    .into_iter()
                    .filter(|item| {
                        item.slot == "send.toolbar"
                            && item.visible
                            && matches!(
                                item.kind.to_ascii_lowercase().as_str(),
                                "button" | "small_button" | ""
                            )
                    })
                    .map(move |item| {
                        let contribution_id = item.id;
                        let title = item.title.or_else(|| {
                            item.command.as_ref().and_then(|command_id| {
                                commands
                                    .iter()
                                    .find(|command| command.id == *command_id)
                                    .map(|command| command.title.clone())
                            })
                        });
                        SendToolbarButton {
                            plugin_id: plugin_id.clone(),
                            contribution_id: contribution_id.clone(),
                            title: title.unwrap_or(contribution_id),
                            tooltip: item.tooltip,
                            order: item.order,
                            enabled: item.enabled,
                        }
                    })
            })
            .collect::<Vec<_>>();
        buttons.sort_by(|left, right| {
            left.order
                .cmp(&right.order)
                .then_with(|| left.title.cmp(&right.title))
        });
        buttons
    }

    fn activate_web_send_toolbar_button(&mut self, plugin_id: &str, contribution_id: &str) {
        let Some((command, record_send_input)) = self
            .plugins
            .records
            .iter()
            .find(|record| record.persisted.manifest.id == plugin_id)
            .and_then(|record| {
                (record.persisted.enabled && record.lua_instance.is_some()).then(|| {
                    record
                        .persisted
                        .manifest
                        .contributes
                        .ui
                        .iter()
                        .find(|item| {
                            item.id == contribution_id
                                && item.slot == "send.toolbar"
                                && item.visible
                                && matches!(
                                    item.kind.to_ascii_lowercase().as_str(),
                                    "button" | "small_button" | ""
                                )
                        })
                        .map(|item| (item.command.clone(), item.record_send_input))
                })
            })
            .flatten()
        else {
            return;
        };

        if record_send_input {
            let input = self.serial.borrow().send_input.clone();
            record_shared_send_history(
                &mut self.serial.borrow_mut().send_history,
                input,
                WEB_MAX_SEND_HISTORY,
            );
        }

        let context = self.web_contribution_command_context("send.toolbar", contribution_id);
        if let Some(command) = command {
            // Plugin commands receive an action payload.  The actual UI
            // state is intentionally nested under `context`; this is the
            // same shape Native uses and is what shared Lua plugins read.
            self.execute_web_plugin_command(
                plugin_id.to_owned(),
                command,
                serde_json::json!({
                    "plugin_id": plugin_id,
                    "contribution_id": contribution_id,
                    "slot": "send.toolbar",
                    "kind": "button",
                    "context": context,
                }),
            );
            self.poll_web_lua_host_requests();
        }
    }

    /// 发送面板提交：先记下当前输入用作历史，再派发命令，最后写回共享历史。
    fn send_web_with_history(&mut self, command: AppCommand, ctx: &egui::Context) {
        let history = self.serial.borrow().send_input.clone();
        self.dispatch_serial(command, ctx);
        record_shared_send_history(
            &mut self.serial.borrow_mut().send_history,
            history,
            WEB_MAX_SEND_HISTORY,
        );
    }

    /// Render the same rich sender used by the Native composition root.
    ///
    /// Only the final action dispatch remains platform-specific.  Keeping the
    /// state adapter here (rather than duplicating widgets) means browser and
    /// desktop get identical mode switching, history navigation, validation,
    /// periodic controls and signal controls.
    fn shared_sender_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let toolbar_buttons = self.web_send_toolbar_buttons();
        let transport = self.runtime.as_ref().map(WebRuntime::query_transport);
        let (ports, connected) = {
            let serial = self.serial.borrow();
            let ports = transport
                .as_ref()
                .map(|view| &view.ports)
                .unwrap_or(&serial.ports)
                .iter()
                .map(|port| SendPortItem {
                    id: port.id.to_string(),
                    label: web_port_display_name(port, &serial.port_aliases),
                })
                .collect::<Vec<_>>();
            let connected = web_connected_port(transport.as_ref(), &serial);
            (ports, connected)
        };

        // `Ref` 临时量随本语句结束释放，紧接着的块才能 `borrow_mut()`。
        let mut line_ending = self.serial.borrow().line_ending;
        let (actions, selected_target) = {
            let mut serial = self.serial.borrow_mut();
            let state = &mut *serial;
            let WebSerialState {
                selected_port,
                send_input,
                tx_hex,
                hex_strict,
                send_error,
                send_history,
                history_search,
                history_index,
                saved_input,
                periodic_enabled,
                periodic_interval_ms,
                periodic_send_count,
                dtr,
                rts,
                ..
            } = state;
            let mut target_port = selected_port.as_ref().map(ToString::to_string);
            let target_open = selected_port
                .as_ref()
                .is_some_and(|port| connected.as_ref() == Some(port));
            let mut view = SendView {
                ports: &ports,
                target_port: &mut target_port,
                target_open,
                input: send_input,
                hex_mode: tx_hex,
                hex_strict,
                line_ending: &mut line_ending,
                error: send_error,
                history: send_history,
                history_search,
                history_index,
                saved_input,
                periodic_enabled,
                periodic_interval_ms,
                periodic_send_count,
                dtr,
                rts,
                toolbar_buttons: &toolbar_buttons,
                max_history: WEB_MAX_SEND_HISTORY,
                layout: send_layout_for_width(ui.available_width()),
            };
            let actions = shared_sender_ui(ui, &mut view);
            *selected_port = target_port.as_deref().map(PortId::new);
            (actions, target_port)
        };

        let selected_target = selected_target.map(PortId::new);
        if selected_target != self.serial.borrow().selected_port {
            self.select_web_port(selected_target);
        }

        {
            let mut serial = self.serial.borrow_mut();
            serial.line_ending = line_ending;
        }

        for action in actions {
            self.dispatch_send_action(action, &ctx);
        }
        self.web_ui_contribution_slot(ui, "send.toolbar");
        self.persist_settings();
    }

    /// 发送面板的一条动作落到命令上：文本/HEX 走带历史的发送入口，
    /// 流控信号直接派发，工具栏按钮交给贡献点激活。
    fn dispatch_send_action(&mut self, action: SendAction, ctx: &egui::Context) {
        match action {
            SendAction::SendText { port, text } => {
                self.send_web_with_history(
                    AppCommand::SendText {
                        port: PortId::new(port),
                        text,
                    },
                    ctx,
                );
            }
            SendAction::SendHex { port, hex, strict } => {
                self.send_web_with_history(
                    AppCommand::SendHex {
                        port: PortId::new(port),
                        hex,
                        strict,
                    },
                    ctx,
                );
            }
            SendAction::SetDtr { port, value } => {
                self.dispatch_serial(
                    AppCommand::SetDtr {
                        port: PortId::new(port),
                        value,
                    },
                    ctx,
                );
            }
            SendAction::SetRts { port, value } => {
                self.dispatch_serial(
                    AppCommand::SetRts {
                        port: PortId::new(port),
                        value,
                    },
                    ctx,
                );
            }
            SendAction::ActivateToolbar {
                plugin_id,
                contribution_id,
            } => {
                self.activate_web_send_toolbar_button(&plugin_id, &contribution_id);
            }
        }
    }

    fn web_recording_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let previous_mode = self.recording_mode;
        let mut mode = match self.recording_mode {
            WebRecordMode::StandardReplay => RecordingMode::StandardReplay,
            WebRecordMode::RawSerial => RecordingMode::RawSerial,
        };
        let status = self.runtime.as_ref().map(WebRuntime::query_recording);
        let stats = status.as_ref().map(|status| &status.stats);
        let running = stats.is_some_and(|stats| stats.running);
        let paused = stats.is_some_and(|stats| stats.paused);
        let stopping = stats.is_some_and(|stats| stats.stopping);
        let events = stats.map_or(0, |stats| stats.events_written);
        let bytes = stats.map_or(0, |stats| stats.bytes_written);
        let backlog_events = stats.map_or(0, |stats| stats.backlog_events);
        let backlog_bytes = stats.map_or(0, |stats| stats.backlog_bytes);
        let last_error = stats.and_then(|stats| stats.last_error.as_deref());
        let current_path = (running || stopping)
            .then(|| status.as_ref().and_then(|status| status.path.as_deref()))
            .flatten();
        let actions = {
            let mut view = RecordingView {
                file_name: &mut self.recording_file_name,
                mode: &mut mode,
                running,
                stopping,
                paused,
                events_written: events,
                bytes_written: Some(bytes),
                flush_elapsed_ms: None,
                backlog_events: running.then_some(backlog_events),
                backlog_bytes: running.then_some(backlog_bytes),
                current_path,
                last_error,
                show_browse: false,
            };
            recording_ui(ui, &mut view)
        };
        self.recording_mode = match mode {
            RecordingMode::StandardReplay => WebRecordMode::StandardReplay,
            RecordingMode::RawSerial => WebRecordMode::RawSerial,
        };
        if previous_mode != self.recording_mode {
            self.dispatch_without_status(AppCommand::SetRecordingMode {
                mode: web_recording_mode_view(self.recording_mode),
            });
            self.persist_settings();
        }
        for action in actions {
            match action {
                RecordingAction::Browse => {}
                RecordingAction::StartStop => {
                    if running {
                        self.stop_web_recording(false, None);
                    } else {
                        self.start_web_recording();
                    }
                }
                RecordingAction::PauseResume => {
                    let command = if paused {
                        AppCommand::ResumeRecording
                    } else {
                        AppCommand::PauseRecording
                    };
                    self.dispatch_without_status(command);
                }
            }
        }
    }
}

fn download_export(
    stem: &str,
    format: TerminalExportFormat,
    content: String,
) -> Result<(), String> {
    let (extension, mime) = match format {
        TerminalExportFormat::Txt => ("txt", "text/plain"),
        TerminalExportFormat::Csv => ("csv", "text/csv"),
        TerminalExportFormat::Json => ("json", "application/json"),
    };
    download_text_file(&format!("{stem}.{extension}"), mime, content)
}

fn download_text_file(file_name: &str, mime: &str, content: String) -> Result<(), String> {
    let parts = Array::new();
    parts.push(&JsValue::from_str(&content));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let blob = match web_sys::Blob::new_with_str_sequence_and_options(&parts, &options) {
        Ok(blob) => blob,
        Err(error) => return Err(format!("创建 Blob 失败：{error:?}")),
    };
    let url = match web_sys::Url::create_object_url_with_blob(&blob) {
        Ok(url) => url,
        Err(error) => return Err(format!("创建下载 URL 失败：{error:?}")),
    };
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        let _ = web_sys::Url::revoke_object_url(&url);
        return Err("浏览器文档不可用".to_owned());
    };
    let Some(anchor) = document
        .create_element("a")
        .ok()
        .and_then(|element| element.dyn_into::<web_sys::HtmlAnchorElement>().ok())
    else {
        let _ = web_sys::Url::revoke_object_url(&url);
        return Err("创建下载链接失败".to_owned());
    };
    anchor.set_href(&url);
    anchor.set_download(file_name);
    anchor.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

fn replay_policy_view_option(policy: ReplayPolicyOption) -> ReplayPolicyView {
    match policy {
        ReplayPolicyOption::AutoPreferRecorded => ReplayPolicyView::AutoPreferRecorded,
        ReplayPolicyOption::ExactRecorded => ReplayPolicyView::ExactRecorded,
        ReplayPolicyOption::ReparseRaw => ReplayPolicyView::ReparseRaw,
    }
}

/// 浏览器录制模式 → Application 的共享 DTO；启动录制与切换模式必须走同一个映射。
fn web_recording_mode_view(mode: WebRecordMode) -> tool_application::recording::RecordModeView {
    match mode {
        WebRecordMode::StandardReplay => {
            tool_application::recording::RecordModeView::StandardReplay
        }
        WebRecordMode::RawSerial => tool_application::recording::RecordModeView::RawSerial,
    }
}

/// UI 贡献点 → 共享 DTO；`default` 缺失时按 JSON null 交给 Lua 侧读。
fn web_plugin_ui_contribution_view(
    contribution: &WebPluginUiContribution,
) -> PluginUiContributionView {
    PluginUiContributionView {
        id: contribution.id.clone(),
        slot: contribution.slot.clone(),
        kind: contribution.kind.clone(),
        title: contribution.title.clone(),
        command: contribution.command.clone(),
        tooltip: contribution.tooltip.clone(),
        order: contribution.order,
        enabled: contribution.enabled,
        visible: contribution.visible,
        record_send_input: contribution.record_send_input,
        default: contribution
            .default
            .clone()
            .unwrap_or(serde_json::Value::Null),
    }
}

/// 面板贡献是自由 JSON：只认带 `id` 的对象，其余条目整条跳过。
fn web_plugin_panel_contribution_view(
    panel: &serde_json::Value,
) -> Option<PluginPanelContributionView> {
    let object = panel.as_object()?;
    let id = object.get("id")?.as_str()?.to_owned();
    let title = object
        .get("title")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&id)
        .to_owned();
    let kind = object
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("dynamic")
        .to_owned();
    let config = object
        .get("config")
        .and_then(serde_json::Value::as_object)
        .map(|config| {
            config
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    Some(PluginPanelContributionView {
        id,
        title,
        kind,
        config,
    })
}

/// 插件在浏览器侧看到的状态：错误优先，其次才是启用开关与 Lua 实例是否已在。
fn web_plugin_record_state(record: &WebPluginRecord) -> PluginStateView {
    if record.error.is_some() {
        PluginStateView::Failed
    } else if record.persisted.enabled && record.lua_instance.is_some() {
        PluginStateView::Running
    } else if record.persisted.enabled {
        PluginStateView::Enabled
    } else {
        PluginStateView::Disabled
    }
}

fn replay_policy_option_view(policy: ReplayPolicyView) -> ReplayPolicyOption {
    match policy {
        ReplayPolicyView::AutoPreferRecorded => ReplayPolicyOption::AutoPreferRecorded,
        ReplayPolicyView::ExactRecorded => ReplayPolicyOption::ExactRecorded,
        ReplayPolicyView::ReparseRaw => ReplayPolicyOption::ReparseRaw,
    }
}

fn web_keymap_title(command_id: &str) -> &'static str {
    BUILTIN_KEYMAP_COMMANDS
        .iter()
        .find(|command| command.id == command_id)
        .map_or("命令", |command| command.title)
}

/// 状态栏录制标签：MB 按 1024 进制换算，与录制面板的统计口径一致。
fn web_recording_status_label(recording: bool, paused: bool, events: u64, bytes: u64) -> String {
    if !recording {
        return "未录制".to_owned();
    }
    let megabytes = bytes as f64 / 1024.0 / 1024.0;
    if paused {
        format!("录制已暂停 {events} 条 {megabytes:.1}MB")
    } else {
        format!("录制中 {events} 条 {megabytes:.1}MB")
    }
}

/// 状态栏的通知区：最新一条直接内联显示，多于一条时再给一个可折叠的「通知 N 条」
/// 列表窗口，展开状态持久化在 egui memory 里，跨帧保留用户的开合选择。
fn web_notification_status_ui(ui: &mut egui::Ui, notifications: &[WebNotification]) {
    let Some(notification) = notifications.first() else {
        return;
    };
    ui.separator();
    ui.label(egui::RichText::new(notification.text.clone()).color(notification.level.color()))
        .on_hover_text(&notification.text);
    if notifications.len() <= 1 {
        return;
    }
    let overflow_id = ui.id().with("web_notification_overflow");
    let overflow_response = ui.small_button(format!("通知 {} 条", notifications.len()));
    let mut overflow_open = ui.ctx().memory_mut(|memory| {
        memory
            .data
            .get_persisted::<bool>(overflow_id)
            .unwrap_or(false)
    });
    if overflow_response.clicked() {
        overflow_open = !overflow_open;
        ui.ctx().memory_mut(|memory| {
            memory.data.insert_persisted(overflow_id, overflow_open);
        });
    }
    if overflow_open {
        egui::Window::new("通知列表")
            .id(overflow_id)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 100.0])
            .auto_sized()
            .show(ui.ctx(), |ui| {
                ui.set_min_width(320.0);
                ui.set_max_height(300.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for item in notifications {
                        ui.label(
                            egui::RichText::new(&item.text)
                                .color(item.level.color())
                                .small(),
                        );
                    }
                });
            });
    }
}

/// 卡片开头的公共三段：撑满可用宽度、分节头、分隔线——与 Native 设置页同形。
/// 各入口仍自己打开 `design::card()`，这里只收这一段公共前缀。
fn web_card_header(ui: &mut egui::Ui, icon: egui_material_icons::MaterialIcon, title: &str) {
    ui.set_min_width(ui.available_width());
    design::section_header(ui, icon, title);
    ui.separator();
}

fn web_version_is_newer(remote: &str, local: &str) -> bool {
    let parse = |version: &str| {
        version
            .split('.')
            .map(|part| part.parse::<u32>().unwrap_or_default())
            .collect::<Vec<_>>()
    };
    let remote = parse(remote);
    let local = parse(local);
    for index in 0..remote.len().max(local.len()) {
        match (
            remote.get(index).copied().unwrap_or_default(),
            local.get(index).copied().unwrap_or_default(),
        ) {
            (remote, local) if remote > local => return true,
            (remote, local) if remote < local => return false,
            _ => {}
        }
    }
    false
}

fn open_web_url(url: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.open_with_url(url);
    }
}

fn setup_web_fonts(cc: &eframe::CreationContext<'_>) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "zh".to_owned(),
        egui::FontData::from_static(NOTO_SANS_SC).into(),
    );
    fonts.font_data.insert(
        "jetbrains".to_owned(),
        egui::FontData::from_static(JETBRAINS_MONO).into(),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "zh".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "jetbrains".to_owned());
    // 等宽族也必须带上中文回退：JetBrains Mono 没有 CJK 字形，终端/日志按等宽渲染，
    // 缺字形就变成豆腐块。native 的 `bootstrap::setup_fonts` 排的是 ["jetbrains", "zh", …]。
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(1, "zh".to_owned());
    cc.egui_ctx.set_fonts(fonts);
    egui_material_icons::initialize(&cc.egui_ctx);
}

fn apply_web_theme(ctx: &egui::Context, selected_theme: theme::AppTheme) {
    // Native loads the selected theme file during its bootstrap path. Web has
    // no filesystem, so it must initialize the bundled palette explicitly;
    // otherwise theme color accessors fall back to Color32::default(), which
    // makes both the background and text black.
    if selected_theme != theme::AppTheme::Custom
        && let Err(error) = theme::load_builtin_theme(selected_theme, std::path::Path::new(""))
    {
        web_sys::console::warn_1(&JsValue::from_str(&format!(
            "failed to load bundled Web theme: {error}"
        )));
    }
    bootstrap::apply_theme(ctx, selected_theme);
}

/// Start the browser application in an existing canvas.
///
/// The HTML/JS host owns the canvas and calls this function after the page is
/// ready, which keeps the Rust composition root independent of the hosting
/// framework.
#[wasm_bindgen]
pub async fn start(canvas_id: String) -> Result<(), JsValue> {
    let window =
        eframe::web_sys::window().ok_or_else(|| JsValue::from_str("window unavailable"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("document unavailable"))?;
    let element = document
        .get_element_by_id(&canvas_id)
        .ok_or_else(|| JsValue::from_str("canvas element not found"))?;
    let canvas = element
        .dyn_into::<eframe::web_sys::HtmlCanvasElement>()
        .map_err(|_| JsValue::from_str("element is not a canvas"))?;

    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|cc| Ok(Box::new(WorkbenchApp::new(cc)))),
        )
        .await
}

/// Trunk loads the generated wasm module after the canvas has been parsed.
/// Starting from this hook keeps the host page free of generated wasm-bindgen
/// module names while retaining `start(canvas_id)` for embedding scenarios.
#[wasm_bindgen(start)]
pub fn bootstrap() {
    spawn_local(async {
        if let Err(error) = start("hardware-workbench".to_owned()).await {
            web_sys::console::error_1(&error);
        }
    });
}
