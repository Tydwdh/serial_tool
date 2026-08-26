//! Browser composition root.
//!
//! Browser composition root. Native-only services are replaced by browser
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
use crate::web_plugin_host::{WebPluginData, WebPluginDataStore, WebPluginHost};
use crate::workbench_app::WorkbenchApp;
use eframe::egui;
use egui::FontFamily;
use egui_material_icons::icons::{
    ICON_APPS, ICON_CABLE, ICON_CANCEL, ICON_FIBER_MANUAL_RECORD, ICON_FOLDER, ICON_INFO,
    ICON_KEYBOARD, ICON_LINK_OFF, ICON_NETWORK_CHECK, ICON_PALETTE, ICON_POWER_SETTINGS_NEW,
    ICON_REFRESH, ICON_SEARCH, ICON_STOP, ICON_TUNE,
};
use js_sys::Array;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use tool_application::plugin::{
    PluginCommandView, PluginContributesView, PluginPanelContributionView, PluginSettingView,
    PluginStateView, PluginSummaryView, PluginUiContributionView, PluginView,
};
use tool_application::replay::{ReplayPolicyView, ReplayStateView, ReplayStatusView};
use tool_application::web::{WebAppEvent, WebRuntime};
use tool_application::{AppCommand, AppRuntime, CommandOutcome, TaskId};
use tool_core::Event;
use tool_core::{Direction, Payload, topic_matches, topics};
use tool_databus::DataBus;
use tool_panels::{
    ChartPanel, DataSettingsView, LogExportCursor, LogPanel, PanelId, PanelManager, PluginsPanel,
    RecordingAction, RecordingMode, RecordingView, ReplayPanel, ReplayPolicyOption, SerialAction,
    SerialPanel, SerialPortItem, SerialPortMetadata, SerialView, TerminalExportCursor,
    TerminalExportFormat, TerminalPanel, data_settings_ui, design, recording_ui, theme,
};
use tool_panels::{
    SendAction, SendLayout, SendLineEnding, SendPortItem, SendView,
    record_history as record_shared_send_history, sender_ui as shared_sender_ui,
};
use tool_platform::storage::{SettingsStore, web::WebSettingsStore};
use tool_platform::{
    NetworkSerialConfig, PortDescriptor, PortId, PortKind, SerialParity, SerialSettings,
};
use tool_plugin_api::{
    LuaEngine, PluginCallResult, PluginHostApi, PluginHostCompletion, PluginHostPendingRequest,
    PluginInstanceId, PluginLoadConfig, PluginPermissions, PluginSerialDevice, PluginValue,
};
use tool_plugin_runtime::WebReplayOutput;
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::{JsFuture, spawn_local};

const NOTO_SANS_SC: &[u8] = include_bytes!("../../../assets/NotoSansSC-VF.ttf");
const JETBRAINS_MONO: &[u8] =
    include_bytes!("../../../assets/JetBrainsMonoNerdFontMono-Regular.ttf");
// Bump when the shared default Dock geometry changes so an older browser
// localStorage layout cannot silently keep the pre-parity split ratios.
const WEB_LAYOUT_VERSION: u8 = 6;
const WEB_MAX_SEND_HISTORY: usize = 200;
const WEB_PLUGIN_API_VERSION: &str = "1";
const WEB_MARKETPLACE_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/Tydwdh/serial_tool/main/plugin-marketplace/registry.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebNotificationLevel {
    #[allow(dead_code)]
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) enum WebLineEnding {
    #[default]
    None,
    Lf,
    Cr,
    Crlf,
}

impl WebLineEnding {
    fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::Lf => "LF",
            Self::Cr => "CR",
            Self::Crlf => "CRLF",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Lf => "\n",
            Self::Cr => "\r",
            Self::Crlf => "\r\n",
        }
    }
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

fn web_reconnect_delay(attempts: u32) -> f64 {
    (0.5 * 2_f64.powi(attempts.min(4) as i32)).min(8.0)
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

fn select_web_port_state(serial: &mut WebSerialState, selected: Option<PortId>) {
    if serial.selected_port == selected {
        return;
    }
    if let Some(previous) = serial.selected_port.clone() {
        serial
            .port_profiles
            .insert(previous.to_string(), serial.settings);
    }
    serial.selected_port = selected.clone();
    if let Some(port) = selected
        && let Some(settings) = serial.port_profiles.get(port.as_str()).copied()
    {
        serial.settings = settings;
    }
}

fn web_hex_error(input: &str, strict: bool) -> Option<String> {
    if strict {
        for token in input.split_whitespace() {
            let token = token
                .strip_prefix("0x")
                .or_else(|| token.strip_prefix("0X"))
                .unwrap_or(token);
            if token.len() != 2 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Some(format!("无效 HEX：{token}"));
            }
        }
    }
    web_parse_hex(input)
        .err()
        .map(|error| format!("HEX 解析失败：{error}"))
}

fn web_parse_hex(input: &str) -> Result<(), String> {
    let tokens = input
        .trim()
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',' || ch == ';')
        .filter(|token| !token.is_empty());
    let mut found = false;
    for token in tokens {
        found = true;
        let mut token = token
            .strip_prefix("0x")
            .or_else(|| token.strip_prefix("0X"))
            .unwrap_or(token)
            .chars()
            .filter(|ch| *ch != '_' && *ch != '-')
            .collect::<String>();
        if token.is_empty() {
            return Err("空 HEX token".to_owned());
        }
        if token.len() > 2 && !token.len().is_multiple_of(2) {
            token.insert(0, '0');
        }
        if token.len() <= 2 {
            if !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(format!("无效 HEX：{token}"));
            }
        } else {
            for pair in token.as_bytes().chunks(2) {
                if pair.len() != 2 || !pair.iter().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(format!("无效 HEX：{token}"));
                }
            }
        }
    }
    if found {
        Ok(())
    } else {
        Err("输入为空".to_owned())
    }
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
        self.records = persisted
            .into_iter()
            .map(|persisted| WebPluginRecord {
                persisted,
                host: None,
                lua_instance: None,
                replay_instance: None,
                load_task: None,
                loading: false,
                error: None,
                panels_published: false,
            })
            .collect();
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
                let state = if record.error.is_some() {
                    PluginStateView::Failed
                } else if record.persisted.enabled && record.lua_instance.is_some() {
                    PluginStateView::Running
                } else if record.persisted.enabled {
                    PluginStateView::Enabled
                } else {
                    PluginStateView::Disabled
                };
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
                        .map(|contribution| PluginUiContributionView {
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
                        })
                        .collect(),
                    panels: manifest
                        .contributes
                        .panels
                        .iter()
                        .filter_map(|panel| {
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
                        })
                        .collect(),
                    settings: manifest
                        .contributes
                        .settings
                        .iter()
                        .map(|setting| PluginSettingView {
                            id: setting.id.clone(),
                            title: setting.title.clone(),
                            kind: setting.kind.clone(),
                            default: setting.default.clone().unwrap_or(serde_json::Value::Null),
                            options: setting.options.clone(),
                            min: setting.min,
                            max: setting.max,
                            step: setting.step,
                            rows: setting.rows,
                            description: setting.description.clone(),
                        })
                        .collect(),
                };
                PluginSummaryView {
                    id: manifest.id.clone(),
                    name: manifest.name.clone(),
                    version: manifest.version.clone(),
                    api_version: manifest.api_version.clone(),
                    runtime: manifest.runtime.clone(),
                    state,
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
            status: "Web Serial：点击“刷新”读取已授权设备".to_owned(),
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
            "Web Serial：点击“刷新”读取已授权设备"
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

    fn sync_web_plugin_view(&self) {
        let Some(runtime) = self.runtime.as_ref() else {
            return;
        };
        runtime.set_plugins_view(PluginView {
            summaries: self.plugins.summaries(),
            diagnostics: Vec::new(),
        });
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
        if let Some(runtime) = self.runtime.as_ref() {
            for config in &settings.network_ports {
                let _ = runtime.dispatch(AppCommand::RegisterNetworkPort {
                    config: config.clone(),
                });
            }
        }
        self.recording_file_name = settings.record_file_name;
        self.recording_mode = settings.record_mode;
        if let Some(runtime) = self.runtime.as_ref() {
            let _ = runtime.dispatch(AppCommand::SetReplayPolicy {
                policy: replay_policy_view_option(settings.replay_policy),
            });
        }
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

    fn start_web_recording(&mut self) {
        let Some(runtime) = self.runtime.as_ref() else {
            self.serial.borrow_mut().status = "异步任务运行时不可用".to_owned();
            return;
        };
        let mode = match self.recording_mode {
            WebRecordMode::StandardReplay => {
                tool_application::recording::RecordModeView::StandardReplay
            }
            WebRecordMode::RawSerial => tool_application::recording::RecordModeView::RawSerial,
        };
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

    #[cfg(any())]
    fn poll_web_recording(&mut self, ctx: &egui::Context) {
        if !self.recording.running {
            self.poll_web_recording_export(ctx);
            return;
        }
        if self.recording.backlog_exceeded() {
            let reason = format!(
                "录制积压超过安全阈值（{} 个事件，{} 字节，落后 {:.1}s）",
                self.recording.queued_events(),
                self.recording.queued_bytes(),
                self.recording.seconds_behind()
            );
            self.stop_web_recording(true, Some(reason));
            return;
        }

        let paused = self.recording.paused;
        let mode = self.recording.mode;
        let mut drained = 0usize;
        let mut pending = Vec::new();
        if let Some(subscription) = self.recording.subscription.as_ref() {
            while drained < 2_000 {
                let Some(event) = subscription.try_recv() else {
                    break;
                };
                drained += 1;
                pending.push(event);
            }
        }
        let mut recorded_limit_hit = false;
        if !paused {
            for event in pending {
                if should_record_web_event(&event, mode)
                    && !self.recording.push_recorded_event(event)
                {
                    recorded_limit_hit = true;
                    break;
                }
            }
        }
        if recorded_limit_hit {
            let reason = self.recording.recorded_limit_message();
            self.stop_web_recording(true, Some(reason));
            return;
        }
        if self.recording.backlog_exceeded() {
            let reason = format!(
                "录制消费速度不足（{} 个事件，{} 字节，落后 {:.1}s）",
                self.recording.queued_events(),
                self.recording.queued_bytes(),
                self.recording.seconds_behind()
            );
            self.stop_web_recording(true, Some(reason));
        }

        self.poll_web_recording_export(ctx);
    }

    #[cfg(any())]
    fn poll_web_recording_export(&mut self, ctx: &egui::Context) {
        const EXPORT_BATCH: usize = 512;
        if let Some(task_id) = self.recording.export_task
            && let Some(runtime) = self.runtime.as_ref()
            && !runtime.task_is_active(task_id)
        {
            self.recording.export = None;
            self.recording.export_task = None;
            self.recording.export_started_at = None;
            self.serial.borrow_mut().status = "录制导出任务已取消".to_owned();
            return;
        }
        let Some(export) = self.recording.export.as_mut() else {
            return;
        };
        let start = export.offset;
        let end = (start + EXPORT_BATCH).min(export.events.len());
        for event in &export.events[start..end] {
            match serde_json::to_string(event) {
                Ok(line) => {
                    export.content.push_str(&line);
                    export.content.push('\n');
                }
                Err(error) => self.recording.last_error = Some(error.to_string()),
            }
        }
        export.offset = end;
        if end >= export.events.len() {
            let write_bytes_per_sec = self.recording.export_write_bytes_per_sec();
            let Some(export) = self.recording.export.take() else {
                return;
            };
            self.recording.last_write_bytes_per_sec = write_bytes_per_sec;
            self.recording.export_started_at = None;
            let incomplete = self.recording.incomplete;
            let result =
                download_text_file(&export.file_name, "application/x-ndjson", export.content);
            if let Err(error) = &result {
                self.recording.last_error = Some(format!("导出录制失败：{error}"));
            }
            if let Some(task_id) = self.recording.export_task.take()
                && let Some(runtime) = self.runtime.as_ref()
            {
                match result {
                    Ok(()) => runtime.complete_task(
                        task_id,
                        if incomplete {
                            "录制已导出（数据不完整）"
                        } else {
                            "录制已导出"
                        },
                    ),
                    Err(error) => runtime.fail_task(task_id, error),
                };
            }
            self.serial.borrow_mut().status = if incomplete {
                "录制已停止（数据不完整）".to_owned()
            } else {
                "录制已导出".to_owned()
            };
        } else {
            self.serial.borrow_mut().status =
                format!("正在导出录制：{end}/{} 条", export.events.len());
            ctx.request_repaint();
        }
    }

    fn request_web_replay_file(&mut self, _ctx: &egui::Context) {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            self.replay_panel.message = Some("浏览器文档不可用".to_owned());
            return;
        };
        let Some(input) = document
            .create_element("input")
            .ok()
            .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            self.replay_panel.message = Some("无法创建浏览器文件选择器".to_owned());
            return;
        };
        input.set_type("file");
        input.set_accept(".jsonl,.ndjson,application/json");
        let Some(runtime) = self.runtime.clone() else {
            self.replay_panel.message = Some("当前浏览器没有可用的异步任务运行时".to_owned());
            return;
        };
        let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let Some(input) = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
            else {
                return;
            };
            let Some(file) = input.files().and_then(|files| files.get(0)) else {
                return;
            };
            let name = file.name();
            let future = async move {
                JsFuture::from(file.text())
                    .await
                    .map_err(|error| format!("读取回放文件失败：{error:?}"))
                    .and_then(|value| {
                        value
                            .as_string()
                            .ok_or_else(|| "回放文件不是有效文本".to_owned())
                    })
            };
            let _ = runtime.load_text("replay_load", name, future);
        }) as Box<dyn FnMut(web_sys::Event)>);
        if input
            .add_event_listener_with_callback("change", closure.as_ref().unchecked_ref())
            .is_err()
        {
            self.replay_panel.message = Some("无法监听文件选择事件".to_owned());
            return;
        }
        closure.forget();
        input.click();
    }

    fn request_web_theme_file(&mut self, _ctx: &egui::Context) {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            self.serial.borrow_mut().status = "浏览器文档不可用".to_owned();
            return;
        };
        let Some(input) = document
            .create_element("input")
            .ok()
            .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            self.serial.borrow_mut().status = "无法创建主题文件选择器".to_owned();
            return;
        };
        input.set_type("file");
        input.set_accept(".json,application/json");
        let Some(runtime) = self.runtime.clone() else {
            self.serial.borrow_mut().status = "当前浏览器没有可用的异步任务运行时".to_owned();
            return;
        };
        let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let Some(input) = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
            else {
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
        }) as Box<dyn FnMut(web_sys::Event)>);
        if input
            .add_event_listener_with_callback("change", closure.as_ref().unchecked_ref())
            .is_err()
        {
            self.serial.borrow_mut().status = "无法监听主题文件选择事件".to_owned();
            return;
        }
        closure.forget();
        input.click();
    }

    fn cancel_web_replay_analyzer(&mut self, reason: &str) {
        if !self.replay_analyzer.running {
            return;
        }
        if let Some(task_id) = self.replay_analyzer.task_id.take()
            && let Some(runtime) = self.runtime.as_ref()
        {
            runtime.cancel_task(task_id);
        }
        self.replay_analyzer.running = false;
        self.replay_analyzer.error = Some(reason.to_owned());
        self.replay_panel.analyzer_busy = false;
        self.replay_panel.message = Some(format!("Web Replay Analyzer 已取消：{reason}"));
    }

    fn start_web_replay_analyzer(&mut self, ctx: &egui::Context) {
        let input_events = self
            .runtime
            .as_ref()
            .map(WebRuntime::replay_raw_serial_events)
            .unwrap_or_default();
        if input_events.is_empty() {
            self.replay_panel.message = Some("请先选择并加载 JSONL 回放文件".to_owned());
            return;
        }
        if self.replay_analyzer.running {
            return;
        }
        let plugin_indices = self
            .plugins
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                record.persisted.enabled
                    && record.replay_instance.is_some()
                    && record.persisted.manifest.replay.is_some()
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if plugin_indices.is_empty() {
            self.replay_analyzer.error = Some(
                "没有可用的 Web Replay Analyzer；请安装并启用包含 replay 配置的 Web 插件。"
                    .to_owned(),
            );
            self.replay_panel.message = self.replay_analyzer.error.clone();
            return;
        }

        let task_id = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.begin_task("replay_analyzer", "正在启动 Web Replay Analyzer"));
        self.replay_analyzer = WebReplayAnalyzerState {
            running: true,
            task_id,
            plugin_indices,
            input_events,
            next_plugin: 0,
            input_indices: Vec::new(),
            input_offset: 0,
            derived_events: Vec::new(),
            logs: Vec::new(),
            error: None,
        };
        self.replay_panel.analyzer_busy = true;
        self.replay_panel.analyzer_logs.clear();
        self.replay_panel.message = Some("正在运行 Web Replay Analyzer…".to_owned());
        self.send_next_web_replay_analyzer(ctx);
    }

    fn send_next_web_replay_analyzer(&mut self, ctx: &egui::Context) {
        let Some(index) = self
            .replay_analyzer
            .plugin_indices
            .get(self.replay_analyzer.next_plugin)
            .copied()
        else {
            self.finish_web_replay_analyzer(ctx);
            return;
        };
        let Some(record) = self.plugins.records.get(index) else {
            self.replay_analyzer.error = Some("Web Replay Analyzer 插件已不存在".to_owned());
            self.finish_web_replay_analyzer(ctx);
            return;
        };
        let Some(replay_instance) = record.replay_instance else {
            self.replay_analyzer.error = Some(format!(
                "Web Replay Analyzer {} 尚未完成加载",
                record.persisted.manifest.name
            ));
            self.finish_web_replay_analyzer(ctx);
            return;
        };
        let Some(replay) = record.persisted.manifest.replay.as_ref() else {
            self.replay_analyzer.next_plugin += 1;
            self.send_next_web_replay_analyzer(ctx);
            return;
        };

        let input_indices = self
            .replay_analyzer
            .input_events
            .iter()
            .enumerate()
            .filter(|(_, event)| {
                replay.subscriptions.is_empty()
                    || replay
                        .subscriptions
                        .iter()
                        .any(|pattern| topic_matches(pattern, &event.topic))
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let session = serde_json::json!({
            "start_ms": self
                .replay_analyzer
                .input_events
                .first()
                .map(|event| event.timestamp_ms)
                .unwrap_or(0),
            "end_ms": self
                .replay_analyzer
                .input_events
                .last()
                .map(|event| event.timestamp_ms)
                .unwrap_or(0),
            "event_count": self.replay_analyzer.input_events.len(),
        });
        self.replay_analyzer.input_indices = input_indices;
        self.replay_analyzer.input_offset = 0;
        if let Err(error) = self
            .web_lua
            .replay_begin(replay_instance, PluginValue::from_json(&session))
        {
            self.replay_analyzer.error = Some(error.to_string());
            self.finish_web_replay_analyzer(ctx);
            return;
        }
        self.replay_panel.message = Some(format!(
            "正在运行 Web Replay Analyzer：{}（{} 个输入事件）",
            record.persisted.manifest.name,
            self.replay_analyzer.input_indices.len()
        ));
        self.send_next_web_replay_analyzer_batch(ctx);
    }

    fn send_next_web_replay_analyzer_batch(&mut self, ctx: &egui::Context) {
        const ANALYZER_BATCH: usize = 256;
        let Some(index) = self
            .replay_analyzer
            .plugin_indices
            .get(self.replay_analyzer.next_plugin)
            .copied()
        else {
            return;
        };
        let Some(replay_instance) = self
            .plugins
            .records
            .get(index)
            .and_then(|record| record.replay_instance)
        else {
            self.replay_analyzer.error = Some("Web Replay Analyzer Lua 实例已退出".to_owned());
            self.finish_web_replay_analyzer(ctx);
            return;
        };
        let start = self.replay_analyzer.input_offset;
        let end = (start + ANALYZER_BATCH).min(self.replay_analyzer.input_indices.len());
        if start < end {
            let events = self.replay_analyzer.input_indices[start..end]
                .iter()
                .filter_map(|index| self.replay_analyzer.input_events.get(*index))
                .cloned()
                .collect::<Vec<_>>();
            self.replay_analyzer.input_offset = end;
            for event in events {
                if let Err(error) = self
                    .web_lua
                    .replay_event(replay_instance, web_event_to_plugin_value(&event))
                {
                    self.replay_analyzer.error = Some(error.to_string());
                    self.finish_web_replay_analyzer(ctx);
                    return;
                }
            }
            self.replay_panel.message = Some(format!(
                "Web Replay Analyzer：处理 {}/{} 个输入事件",
                end,
                self.replay_analyzer.input_indices.len()
            ));
        } else {
            match self.web_lua.replay_end(replay_instance) {
                Ok(WebReplayOutput { events, logs }) => {
                    self.replay_analyzer.logs.extend(logs);
                    for value in events {
                        match plugin_event_to_core_event(value) {
                            Ok(event) => self.replay_analyzer.derived_events.push(event),
                            Err(error) => self.replay_analyzer.logs.push(error),
                        }
                    }
                    self.finish_web_replay_analyzer_plugin(index, ctx);
                    return;
                }
                Err(error) => {
                    self.replay_analyzer.error = Some(error.to_string());
                    self.finish_web_replay_analyzer(ctx);
                    return;
                }
            }
        }
        ctx.request_repaint();
    }

    fn finish_web_replay_analyzer_plugin(&mut self, index: usize, ctx: &egui::Context) {
        if !self.replay_analyzer.running
            || self
                .replay_analyzer
                .plugin_indices
                .get(self.replay_analyzer.next_plugin)
                != Some(&index)
        {
            return;
        }
        self.replay_analyzer.next_plugin += 1;
        self.send_next_web_replay_analyzer(ctx);
    }

    fn finish_web_replay_analyzer(&mut self, ctx: &egui::Context) {
        if !self.replay_analyzer.running {
            return;
        }
        self.replay_analyzer.running = false;
        self.replay_panel.analyzer_busy = false;
        self.replay_panel.analyzer_logs.clear();
        for line in self.replay_analyzer.logs.iter().rev().take(200).rev() {
            self.replay_panel.push_analyzer_log(line.clone());
        }
        if let Some(runtime) = self.runtime.as_ref() {
            if let Some(error) = self.replay_analyzer.error.clone() {
                runtime.replay_set_analyzer_error(error);
            } else {
                runtime.replay_set_analyzer_cache(self.replay_analyzer.derived_events.clone());
            }
        }
        let derived = self.replay_analyzer.derived_events.len();
        let message = match self.replay_analyzer.error.as_deref() {
            Some(error) => format!("Web Replay Analyzer 失败：{error}"),
            None => format!("Web Replay Analyzer 完成：生成 {derived} 个派生事件"),
        };
        if let Some(task_id) = self.replay_analyzer.task_id.take()
            && let Some(runtime) = self.runtime.as_ref()
        {
            if self.replay_analyzer.error.is_some() {
                runtime.fail_task(task_id, message.clone());
            } else {
                runtime.complete_task(task_id, message.clone());
            }
        }
        self.replay_panel.message = Some(message);
        ctx.request_repaint();
    }

    fn web_replay_status(&self) -> ReplayStatusView {
        self.runtime
            .as_ref()
            .map(WebRuntime::query_replay)
            .unwrap_or_else(empty_replay_status)
    }

    fn web_replay_panel_ui(&mut self, ui: &mut egui::Ui) {
        let status = self.web_replay_status();
        if let Some(path) = status.path.as_deref() {
            self.replay_panel.path = path.to_owned();
        }
        // The analyzer executes in Web Workers, but its lifecycle is owned by
        // the composition root. Mirror only its presentation DTO into the
        // shared panel so replay controls remain identical to Native.
        self.replay_panel.analyzer_busy = self.replay_analyzer.running;
        self.replay_panel.analyzer_logs.clear();
        for line in self.replay_analyzer.logs.iter().rev().take(200).rev() {
            self.replay_panel.push_analyzer_log(line.clone());
        }
        self.replay_panel.ui(ui, &status);

        if self.replay_panel.want_pick_file {
            self.replay_panel.want_pick_file = false;
            self.request_web_replay_file(ui.ctx());
        }
        if self.replay_panel.want_run_analyzers {
            self.replay_panel.want_run_analyzers = false;
            self.start_web_replay_analyzer(ui.ctx());
        }
        if self.replay_panel.want_cancel_analyzers {
            self.replay_panel.want_cancel_analyzers = false;
            self.cancel_web_replay_analyzer("用户取消");
        }
        if self.replay_panel.want_clear_on_play {
            self.replay_panel.want_clear_on_play = false;
            self.clear_web_replay_views();
        }
        if let Some(position_ms) = self.replay_panel.want_seek_replay.take() {
            self.dispatch_web_replay(AppCommand::ReplaySeek { position_ms }, true);
        }
        if let Some(steps) = self.replay_panel.want_step_backward.take() {
            self.dispatch_web_replay(
                AppCommand::ReplayStep {
                    delta: -(steps.min(i32::MAX as usize) as i32),
                },
                true,
            );
        }

        for command in self.replay_panel.take_commands() {
            match command {
                tool_panels::ReplayUiCommand::PickFile => {
                    self.request_web_replay_file(ui.ctx());
                }
                tool_panels::ReplayUiCommand::Load { .. } => {
                    self.replay_panel.message =
                        Some("Web 回放请使用“浏览”选择本地 JSONL 文件".to_owned());
                }
                tool_panels::ReplayUiCommand::Play => {
                    self.dispatch_web_replay(AppCommand::ReplayPlay, false);
                }
                tool_panels::ReplayUiCommand::Pause => {
                    self.dispatch_web_replay(AppCommand::ReplayPause, false);
                }
                tool_panels::ReplayUiCommand::Stop => {
                    self.dispatch_web_replay(AppCommand::ReplayStop, true);
                }
                tool_panels::ReplayUiCommand::Seek { position_ms }
                | tool_panels::ReplayUiCommand::SeekPanelPhase { position_ms }
                | tool_panels::ReplayUiCommand::SeekDataPhase { position_ms } => {
                    self.dispatch_web_replay(AppCommand::ReplaySeek { position_ms }, true);
                }
                tool_panels::ReplayUiCommand::SeekCursorPanelPhase { target_cursor }
                | tool_panels::ReplayUiCommand::SeekCursorDataPhase { target_cursor } => {
                    let current = self.web_replay_status().cursor;
                    let delta = target_cursor as i64 - current as i64;
                    if delta != 0 {
                        self.dispatch_web_replay(
                            AppCommand::ReplayStep {
                                delta: delta.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
                            },
                            true,
                        );
                    }
                }
                tool_panels::ReplayUiCommand::StepBackward { steps } => {
                    self.dispatch_web_replay(
                        AppCommand::ReplayStep {
                            delta: -(steps.min(i32::MAX as usize) as i32),
                        },
                        true,
                    );
                }
                tool_panels::ReplayUiCommand::SetSpeed(speed) => {
                    self.dispatch_web_replay(AppCommand::SetReplaySpeed { speed }, false);
                }
                tool_panels::ReplayUiCommand::SetPolicy(policy) => {
                    self.dispatch_web_replay(AppCommand::SetReplayPolicy { policy }, true);
                    self.persist_settings();
                }
                tool_panels::ReplayUiCommand::AddReplayBookmark { name } => {
                    self.dispatch_web_replay(AppCommand::AddReplayBookmark { name }, false);
                }
                tool_panels::ReplayUiCommand::RemoveReplayBookmark { position_ms } => {
                    self.dispatch_web_replay(
                        AppCommand::RemoveReplayBookmark { position_ms },
                        false,
                    );
                }
                tool_panels::ReplayUiCommand::SetLoop(value) => {
                    self.replay_panel.loop_playback = value;
                }
                tool_panels::ReplayUiCommand::SetStepSize(_) => {}
                tool_panels::ReplayUiCommand::SetAnalyzerCache(_)
                | tool_panels::ReplayUiCommand::SetAnalyzerError(_)
                | tool_panels::ReplayUiCommand::SetAnalyzerWarning(_)
                | tool_panels::ReplayUiCommand::ClearAnalyzerError
                | tool_panels::ReplayUiCommand::PushAnalyzerLog(_) => {}
            }
        }
        let status = self.web_replay_status();
        if self.replay_panel.loop_playback && status.state == ReplayStateView::Finished {
            self.clear_web_replay_views();
            self.dispatch_web_replay(AppCommand::ReplayStop, false);
            self.dispatch_web_replay(AppCommand::ReplayPlay, false);
        }
    }

    fn clear_web_replay_views(&mut self) {
        self.terminal_panel.clear();
        self.bottom_log_panel.clear();
        self.dynamic_panels.clear_charts();
    }

    fn dispatch_web_replay(&mut self, command: AppCommand, rebuild: bool) {
        if rebuild {
            self.clear_web_replay_views();
        }
        let Some(runtime) = self.runtime.as_ref() else {
            self.replay_panel.message = Some("Application 运行时不可用".to_owned());
            return;
        };
        if let Err(error) = runtime.dispatch(command) {
            self.replay_panel.message = Some(format!("回放操作失败：{error}"));
            self.serial.borrow_mut().status = format!("回放操作失败：{error}");
        }
    }

    fn request_web_plugin_files(&mut self, _ctx: &egui::Context) {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        let Some(input) = document
            .create_element("input")
            .ok()
            .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            return;
        };
        input.set_type("file");
        input.set_multiple(true);
        input.set_accept(".json,.lua");
        let Some(runtime) = self.runtime.clone() else {
            self.serial.borrow_mut().status = "当前浏览器没有可用的异步任务运行时".to_owned();
            return;
        };
        let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let Some(input) = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
            else {
                return;
            };
            let Some(files) = input.files() else {
                return;
            };
            let mut files_to_read = Vec::new();
            for index in 0..files.length() {
                if let Some(file) = files.get(index) {
                    files_to_read.push((file.name(), file));
                }
            }
            let future = async move {
                let mut contents = Vec::with_capacity(files_to_read.len());
                for (name, file) in files_to_read {
                    let text = JsFuture::from(file.text())
                        .await
                        .map_err(|error| format!("读取插件文件失败：{error:?}"))?
                        .as_string()
                        .ok_or_else(|| format!("插件文件不是文本：{name}"))?;
                    contents.push((name, text));
                }
                Ok::<_, String>(contents)
            };
            let _ = runtime.load_files("plugin_import", future);
        }) as Box<dyn FnMut(web_sys::Event)>);
        if input
            .add_event_listener_with_callback("change", closure.as_ref().unchecked_ref())
            .is_ok()
        {
            closure.forget();
            input.click();
        }
    }

    fn load_web_plugin(&mut self, index: usize) {
        let runtime = self.runtime.clone();
        let Some(record) = self.plugins.records.get(index) else {
            return;
        };
        if record.loading || record.lua_instance.is_some() || record.replay_instance.is_some() {
            return;
        }
        let plugin_name = record.persisted.manifest.name.clone();
        let plugin_id = record.persisted.manifest.id.clone();
        let plugin_version = record.persisted.manifest.version.clone();
        let script_name = record.persisted.manifest.live_main().to_owned();
        let runtime_name = record.persisted.manifest.runtime.clone();
        let source = record.persisted.source.clone();
        let replay_source = record.persisted.replay_source.clone();
        let replay_manifest = record.persisted.manifest.replay.clone();
        let settings =
            serde_json::to_string(&record.persisted.settings).unwrap_or_else(|_| "{}".to_owned());
        let permissions = record.persisted.manifest.live_permissions().to_vec();
        let context = PluginValue::from_json(&serde_json::json!({
            "settings": record.persisted.settings,
            "plugin_id": plugin_id,
        }));
        let Some(record) = self.plugins.records.get_mut(index) else {
            return;
        };
        record.loading = true;
        record.error = None;
        record.load_task = runtime.as_ref().map(|runtime| {
            runtime.begin_task("plugin_load", format!("正在加载 Lua 插件：{plugin_name}"))
        });

        if runtime_name != "lua" {
            let error = format!(
                "插件 {} 使用 runtime={}；Native/Web 统一使用 runtime=lua + main.lua",
                plugin_id, runtime_name
            );
            record.loading = false;
            record.error = Some(error.clone());
            if let Some(task_id) = record.load_task.take()
                && let Some(runtime) = runtime.as_ref()
            {
                runtime.fail_task(task_id, error);
            }
            return;
        }

        {
            let Some(runtime) = runtime else {
                record.loading = false;
                record.error = Some("Web Application runtime 不可用".to_owned());
                return;
            };
            self.plugin_data
                .borrow_mut()
                .entry(plugin_id.clone())
                .or_insert_with(|| WebPluginData {
                    settings: serde_json::from_str(&settings).unwrap_or_default(),
                    storage: BTreeMap::new(),
                    profiles: BTreeMap::new(),
                });
            let permission_set = PluginPermissions::from_permission_names(permissions);
            let missing = permission_set.missing_from([
                tool_plugin_api::PluginCapability::Bus,
                tool_plugin_api::PluginCapability::Config,
                tool_plugin_api::PluginCapability::Dialog,
                tool_plugin_api::PluginCapability::Filesystem,
                tool_plugin_api::PluginCapability::Log,
                tool_plugin_api::PluginCapability::Serial,
                tool_plugin_api::PluginCapability::Storage,
                tool_plugin_api::PluginCapability::Task,
                tool_plugin_api::PluginCapability::Timer,
                tool_plugin_api::PluginCapability::Ui,
            ]);
            if !missing.is_empty() {
                let error = format!(
                    "插件声明了当前浏览器不支持的权限：{}",
                    missing
                        .iter()
                        .map(|capability| capability.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                record.loading = false;
                record.error = Some(error.clone());
                if let Some(task_id) = record.load_task.take() {
                    runtime.fail_task(task_id, error);
                }
                return;
            }
            let host = Rc::new(WebPluginHost::new(
                runtime.clone(),
                plugin_id.clone(),
                self.plugin_data.clone(),
            ));
            let config = PluginLoadConfig {
                plugin_id: plugin_id.clone(),
                plugin_name: plugin_name.clone(),
                plugin_version: plugin_version.clone(),
                script_name,
                context: context.clone(),
                permissions: permission_set.clone(),
            };
            record.host = Some(host.clone());
            match self.web_lua.load_plugin(&source, config, host) {
                Ok(instance) => {
                    record.lua_instance = Some(instance);
                    if let (Some(replay_source), Some(replay_manifest)) =
                        (replay_source.as_deref(), replay_manifest.as_ref())
                    {
                        let replay_config = PluginLoadConfig {
                            plugin_id: plugin_id.clone(),
                            plugin_name: plugin_name.clone(),
                            plugin_version: plugin_version.clone(),
                            script_name: replay_manifest.main.clone(),
                            context,
                            permissions: permission_set,
                        };
                        match self.web_lua.load_replay_plugin(
                            replay_source,
                            replay_config,
                            replay_manifest.outputs.clone(),
                            record.host.as_ref().expect("host installed").clone(),
                        ) {
                            Ok(replay_instance) => record.replay_instance = Some(replay_instance),
                            Err(error) => {
                                record.error = Some(format!("Replay Lua 加载失败：{error}"));
                            }
                        }
                    }
                    record.loading = false;
                    record.panels_published = true;
                    if let Some(task_id) = record.load_task.take() {
                        runtime.complete_task(task_id, "Lua 插件已加载");
                    }
                    self.publish_web_plugin_panels(index);
                }
                Err(error) => {
                    record.loading = false;
                    record.error = Some(error.to_string());
                    if let Some(task_id) = record.load_task.take() {
                        runtime.fail_task(task_id, error.to_string());
                    }
                }
            }
        }
    }

    fn publish_web_plugin_panels(&self, index: usize) {
        let Some(runtime) = &self.runtime else {
            return;
        };
        let Some(record) = self.plugins.records.get(index) else {
            return;
        };
        if !record
            .persisted
            .manifest
            .live_permissions()
            .iter()
            .any(|permission| permission == "ui")
        {
            return;
        }
        let source = format!("plugin:{}", record.persisted.manifest.id);
        for panel in record.persisted.manifest.contributes.panels.clone() {
            runtime.publish_event(Event::new(
                topics::UI_PANEL_CREATE,
                source.clone(),
                Direction::Internal,
                Payload::Json(panel),
            ));
        }
    }

    fn remove_web_plugin_panels(&self, index: usize) {
        let Some(runtime) = &self.runtime else {
            return;
        };
        let Some(record) = self.plugins.records.get(index) else {
            return;
        };
        let source = format!("plugin:{}", record.persisted.manifest.id);
        for panel in &record.persisted.manifest.contributes.panels {
            let Some(id) = panel.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            runtime.publish_event(Event::new(
                topics::UI_PANEL_REMOVE,
                source.clone(),
                Direction::Internal,
                Payload::Json(serde_json::json!({ "id": id })),
            ));
        }
    }

    fn install_web_plugin(&mut self, persisted: WebPluginPersisted) {
        if let Some(index) = self
            .plugins
            .records
            .iter()
            .position(|record| record.persisted.manifest.id == persisted.manifest.id)
        {
            self.unload_web_plugin(index);
            self.plugins.records[index] = WebPluginRecord {
                persisted,
                host: None,
                lua_instance: None,
                replay_instance: None,
                load_task: None,
                loading: false,
                error: None,
                panels_published: false,
            };
            if self.plugins.records[index].persisted.enabled {
                self.load_web_plugin(index);
            }
        } else {
            self.plugins.records.push(WebPluginRecord {
                persisted,
                host: None,
                lua_instance: None,
                replay_instance: None,
                load_task: None,
                loading: false,
                error: None,
                panels_published: false,
            });
            let index = self.plugins.records.len() - 1;
            if self.plugins.records[index].persisted.enabled {
                self.load_web_plugin(index);
            }
        }
        self.persist_settings();
    }

    fn uninstall_web_plugin(&mut self, plugin_id: &str) {
        let Some(index) = self
            .plugins
            .records
            .iter()
            .position(|record| record.persisted.manifest.id == plugin_id)
        else {
            self.serial.borrow_mut().status = format!("Web 插件不存在：{plugin_id}");
            return;
        };
        self.unload_web_plugin(index);
        self.plugins.records.remove(index);
        if !self
            .plugins
            .records
            .iter()
            .any(|record| record.persisted.enabled)
            && let Some(runtime) = self.runtime.as_ref()
        {
            runtime.clear_plugin_events();
        }
        self.persist_settings();
        self.serial.borrow_mut().status = format!("Web 插件已卸载：{plugin_id}");
    }

    fn apply_web_plugin_files(&mut self, files: Vec<(String, String)>) {
        match parse_web_plugin_files(&files) {
            Ok(persisted) => self.install_web_plugin(persisted),
            Err(error) => self.serial.borrow_mut().status = error,
        }
    }

    fn apply_web_marketplace_files(&mut self, plugin_id: String, files: Vec<(String, String)>) {
        let entry = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.query_marketplace().registry)
            .and_then(|registry| {
                registry
                    .plugins
                    .into_iter()
                    .find(|entry| entry.id == plugin_id)
            });
        match parse_web_plugin_files(&files) {
            Ok(persisted) => {
                self.install_web_plugin(persisted);
                if let Some(runtime) = self.runtime.as_ref() {
                    runtime.finish_marketplace_install(&plugin_id, Ok(()));
                }
                if let Some(entry) = entry {
                    self.serial.borrow_mut().status =
                        format!("已安装 Web 插件 {} v{}", entry.name, entry.version);
                }
            }
            Err(error) => {
                if let Some(runtime) = self.runtime.as_ref() {
                    runtime.finish_marketplace_install(
                        &plugin_id,
                        Err(format!("安装 Web 插件失败：{error}")),
                    );
                }
            }
        }
    }

    fn poll_web_plugins(&mut self, _ctx: &egui::Context) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        let plugin_commands = runtime.take_plugin_commands();
        for command in plugin_commands {
            match command {
                tool_application::plugin::PluginCommand::Enable { plugin_id } => {
                    if let Some(index) = self
                        .plugins
                        .records
                        .iter()
                        .position(|record| record.persisted.manifest.id == plugin_id)
                    {
                        let should_load = self
                            .plugins
                            .records
                            .get(index)
                            .is_some_and(|record| !record.persisted.enabled);
                        if should_load {
                            self.plugins.records[index].persisted.enabled = true;
                            self.load_web_plugin(index);
                            self.persist_settings();
                        }
                    }
                }
                tool_application::plugin::PluginCommand::Disable { plugin_id } => {
                    if let Some(index) = self
                        .plugins
                        .records
                        .iter()
                        .position(|record| record.persisted.manifest.id == plugin_id)
                    {
                        let should_unload = self
                            .plugins
                            .records
                            .get(index)
                            .is_some_and(|record| record.persisted.enabled);
                        if should_unload {
                            self.plugins.records[index].persisted.enabled = false;
                            self.unload_web_plugin(index);
                            self.persist_settings();
                        }
                    }
                }
                tool_application::plugin::PluginCommand::Reload => {
                    let enabled = self
                        .plugins
                        .records
                        .iter()
                        .enumerate()
                        .filter_map(|(index, record)| record.persisted.enabled.then_some(index))
                        .collect::<Vec<_>>();
                    for index in enabled {
                        self.unload_web_plugin(index);
                        self.load_web_plugin(index);
                    }
                }
                tool_application::plugin::PluginCommand::Execute {
                    plugin_id,
                    command_id,
                    context,
                } => {
                    let Some(index) = self
                        .plugins
                        .records
                        .iter()
                        .position(|record| record.persisted.manifest.id == plugin_id)
                    else {
                        self.serial.borrow_mut().status = format!("Web 插件不存在：{plugin_id}");
                        continue;
                    };
                    if let Some(instance) = self
                        .plugins
                        .records
                        .get(index)
                        .and_then(|record| record.lua_instance)
                    {
                        let context = PluginValue::from_json(&context);
                        match self
                            .web_lua
                            .dispatch_command(instance, &command_id, context)
                        {
                            Ok(PluginCallResult::Completed(value)) => {
                                self.serial.borrow_mut().status = format!(
                                    "{}：{}",
                                    plugin_id,
                                    value
                                        .to_json()
                                        .map(|value| value.to_string())
                                        .unwrap_or_default()
                                );
                            }
                            Ok(PluginCallResult::Yielded { .. }) => {
                                self.serial.borrow_mut().status =
                                    format!("{plugin_id}：命令已进入异步任务");
                            }
                            Err(error) => self.serial.borrow_mut().status = error.to_string(),
                        }
                    }
                }
            }
        }
        self.sync_web_plugin_view();
        let events = if self
            .plugins
            .records
            .iter()
            .any(|record| record.persisted.enabled)
        {
            runtime.drain_plugin_events(256)
        } else {
            runtime.clear_plugin_events();
            Vec::new()
        };
        for event in events {
            let event_value = web_event_to_plugin_value(&event);
            for record in &self.plugins.records {
                if record.persisted.enabled
                    && record
                        .persisted
                        .manifest
                        .live_permissions()
                        .iter()
                        .any(|permission| permission == "bus")
                    && let Some(instance) = record.lua_instance
                    && (record.persisted.manifest.live_subscriptions().is_empty()
                        || record
                            .persisted
                            .manifest
                            .live_subscriptions()
                            .iter()
                            .any(|pattern| topic_matches(pattern, &event.topic))
                        || event.topic.starts_with("ui.")
                        || event.topic.starts_with("log.")
                        || event.topic == topics::PLUGIN_COMMAND_EXECUTE)
                    && let Err(error) = self.web_lua.dispatch_event(instance, event_value.clone())
                {
                    self.serial.borrow_mut().status = format!("Lua 插件发送事件失败：{error}");
                }
            }
        }
    }

    fn request_web_marketplace_refresh(&mut self, _ctx: &egui::Context) {
        if let Some(runtime) = self.runtime.as_ref() {
            match runtime.dispatch(AppCommand::RefreshMarketplace {
                url: self.marketplace_url.clone(),
            }) {
                Ok(CommandOutcome::Pending { message, .. }) => {
                    self.serial.borrow_mut().status = message;
                }
                Ok(CommandOutcome::Done) => {}
                Err(error) => self.serial.borrow_mut().status = error,
            }
        }
    }

    fn install_web_marketplace_entry(
        &mut self,
        plugin_id: String,
        manifest_url: String,
        main_url: String,
    ) {
        let Some(runtime) = self.runtime.as_ref() else {
            self.serial.borrow_mut().status = "异步任务运行时不可用".to_owned();
            return;
        };
        if let Err(error) = runtime.dispatch(AppCommand::InstallMarketplacePlugin {
            plugin_id,
            manifest_url,
            main_url,
        }) {
            self.serial.borrow_mut().status = error;
        }
    }

    fn unload_web_plugin(&mut self, index: usize) {
        self.remove_web_plugin_panels(index);
        if let Some(plugin_id) = self
            .plugins
            .records
            .get(index)
            .map(|record| record.persisted.manifest.id.clone())
        {
            self.plugins.clear_contribution_values(&plugin_id);
        }
        self.plugins
            .pending_lua_file_requests
            .borrow_mut()
            .retain(|_, (host, _)| {
                self.plugins
                    .records
                    .get(index)
                    .and_then(|record| record.host.as_ref())
                    .is_none_or(|record_host| !Rc::ptr_eq(record_host, host))
            });
        self.plugins
            .pending_lua_serial_requests
            .borrow_mut()
            .retain(|_, (host, _)| {
                self.plugins
                    .records
                    .get(index)
                    .and_then(|record| record.host.as_ref())
                    .is_none_or(|record_host| !Rc::ptr_eq(record_host, host))
            });
        let load_task = self
            .plugins
            .records
            .get_mut(index)
            .and_then(|record| record.load_task.take());
        if let Some(task_id) = load_task
            && let Some(runtime) = self.runtime.as_ref()
        {
            runtime.cancel_task(task_id);
        }
        if let Some(instance) = self
            .plugins
            .records
            .get(index)
            .and_then(|record| record.lua_instance)
        {
            let _ = self.web_lua.stop(instance);
        }
        if let Some(instance) = self
            .plugins
            .records
            .get(index)
            .and_then(|record| record.replay_instance)
        {
            let _ = self.web_lua.stop(instance);
        }
        let Some(record) = self.plugins.records.get_mut(index) else {
            return;
        };
        record.lua_instance = None;
        record.replay_instance = None;
        record.host = None;
        record.loading = false;
    }

    fn request_web_update_check(&mut self, _ctx: &egui::Context) {
        if let Some(runtime) = self.runtime.as_ref() {
            match runtime.dispatch(AppCommand::CheckForUpdate) {
                Ok(CommandOutcome::Pending { message, .. }) => {
                    self.serial.borrow_mut().status = message;
                }
                Ok(CommandOutcome::Done) => {}
                Err(error) => self.serial.borrow_mut().status = error,
            }
        }
    }

    fn tick_web_reconnect(&mut self, ctx: &egui::Context) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        let now = web_now_seconds();
        let request = {
            let mut serial = self.serial.borrow_mut();
            if !serial.auto_reconnect {
                let cancellation = serial
                    .reconnect
                    .as_ref()
                    .map(|pending| (pending.port.clone(), pending.task_id));
                serial.reconnect = None;
                cancellation.map(|(port, task_id)| (port, task_id, None))
            } else {
                let Some(pending) = serial.reconnect.as_mut() else {
                    return;
                };
                if pending.task_id.is_some() || now < pending.next_attempt_at {
                    if pending.task_id.is_none() {
                        ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                            (pending.next_attempt_at - now).max(0.05),
                        ));
                    }
                    return;
                }
                Some((pending.port.clone(), None, Some(serial.settings)))
            }
        };

        let Some((port, task_id, settings)) = request else {
            return;
        };
        if let Some(task_id) = task_id {
            let _ = runtime.cancel_task(task_id);
            let _ = runtime.dispatch(AppCommand::CancelReconnect { port });
            return;
        }
        let Some(settings) = settings else {
            return;
        };

        match runtime.dispatch(AppCommand::Connect {
            port: port.clone(),
            settings,
        }) {
            Ok(CommandOutcome::Pending { task_id, message }) => {
                let mut serial = self.serial.borrow_mut();
                if let Some(pending) = serial
                    .reconnect
                    .as_mut()
                    .filter(|pending| pending.port == port)
                {
                    pending.task_id = Some(task_id);
                    serial.status = message;
                    ctx.request_repaint();
                }
            }
            Ok(CommandOutcome::Done) => {
                self.serial.borrow_mut().reconnect = None;
            }
            Err(error) => {
                let mut serial = self.serial.borrow_mut();
                let mut stop = false;
                if let Some(pending) = serial
                    .reconnect
                    .as_mut()
                    .filter(|pending| pending.port == port)
                {
                    pending.attempts = pending.attempts.saturating_add(1);
                    if pending.attempts >= 10 {
                        stop = true;
                    } else {
                        pending.next_attempt_at = now + web_reconnect_delay(pending.attempts);
                    }
                }
                if stop {
                    serial.reconnect = None;
                    serial.status = format!("自动重连失败（已尝试 10 次）：{error}");
                } else {
                    serial.status = format!("自动重连失败，将稍后重试：{error}");
                }
            }
        }
    }

    fn poll_web_events(&mut self, ctx: &egui::Context) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        for event in runtime.drain_events() {
            let mut plugin_lua_file_resolution = None;
            let mut plugin_lua_serial_resolution = None;
            let mut plugin_files = None;
            let mut marketplace_files = None;
            let mut theme_import = None;
            let mut serial = self.serial.borrow_mut();
            match event {
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
                    let result = download_text_file(&name, "application/x-ndjson", content);
                    if let Some(runtime) = self.runtime.as_ref() {
                        runtime.finish_recording_export(id, result.clone());
                    }
                    serial.status = match result {
                        Ok(()) if incomplete => "录制已停止并导出（数据不完整）".to_owned(),
                        Ok(()) => "录制已停止并导出".to_owned(),
                        Err(error) => format!("录制导出失败：{error}"),
                    };
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
                        match runtime.dispatch(AppCommand::LoadReplayText { name, text }) {
                            Ok(CommandOutcome::Pending { .. }) => {
                                self.replay_panel.set_load_pending(true);
                                serial.status = "回放文件读取完成，正在后台解析".to_owned();
                            }
                            Ok(_) => serial.status = "回放文件已提交加载".to_owned(),
                            Err(error) => {
                                self.replay_panel.message = Some(error.clone());
                                serial.status = format!("回放加载失败：{error}");
                            }
                        }
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
                    serial.status = format!("已授权设备 {} 个", ports.len());
                    serial.ports = ports;
                    if serial.selected_port.as_ref().is_some_and(|selected| {
                        !serial.ports.iter().any(|port| &port.id == selected)
                    }) {
                        select_web_port_state(&mut serial, None);
                    }
                }
                WebAppEvent::PortRequested { id, port } => {
                    plugin_lua_serial_resolution =
                        Some((id, Ok(PluginSerialDevice::from(port.clone()))));
                    serial.ports.retain(|item| item.id != port.id);
                    select_web_port_state(&mut serial, Some(port.id.clone()));
                    serial.ports.push(port);
                    serial.status = "设备已授权，可连接".to_owned();
                }
                WebAppEvent::PortAttached(port) => {
                    serial.ports.retain(|item| item.id != port.id);
                    if serial
                        .reconnect
                        .as_ref()
                        .is_some_and(|pending| pending.matches(&port))
                        && let Some(pending) = serial.reconnect.as_mut()
                    {
                        pending.port = port.id.clone();
                        pending.vendor_id = port.vendor_id;
                        pending.product_id = port.product_id;
                        pending.next_attempt_at = 0.0;
                    }
                    serial.ports.push(port);
                    serial.status = "检测到已授权设备".to_owned();
                }
                WebAppEvent::PortDetached(port) => {
                    let descriptor = serial.ports.iter().find(|item| item.id == port).cloned();
                    serial.ports.retain(|item| item.id != port);
                    if serial.connected.as_ref() == Some(&port) {
                        serial.connected = None;
                        serial.status = "设备已拔出".to_owned();
                        if serial.auto_reconnect {
                            serial.reconnect = Some(WebReconnectState {
                                port: port.clone(),
                                vendor_id: descriptor.as_ref().and_then(|item| item.vendor_id),
                                product_id: descriptor.as_ref().and_then(|item| item.product_id),
                                attempts: 0,
                                next_attempt_at: 0.0,
                                task_id: None,
                            });
                        }
                    }
                    if serial.selected_port.as_ref() == Some(&port) {
                        select_web_port_state(&mut serial, None);
                    }
                }
                WebAppEvent::NetworkPortAdded(port) => {
                    serial.ports.retain(|item| item.id != port.id);
                    select_web_port_state(&mut serial, Some(port.id.clone()));
                    serial.ports.push(port);
                    serial.status = "网络串口已添加，可连接".to_owned();
                }
                WebAppEvent::NetworkPortRemoved(port) => {
                    serial.ports.retain(|item| item.id != port);
                    if serial.selected_port.as_ref() == Some(&port) {
                        select_web_port_state(&mut serial, None);
                    }
                    if serial.connected.as_ref() == Some(&port) {
                        serial.connected = None;
                    }
                    serial.status = "网络串口已移除".to_owned();
                }
                WebAppEvent::Connected { port } => {
                    serial.reconnect = None;
                    select_web_port_state(&mut serial, Some(port.clone()));
                    serial.connected = Some(port.clone());
                    serial.status = format!("已连接 {port} @ {}", settings_label(serial.settings));
                }
                WebAppEvent::Disconnected { port } => {
                    if serial.connected.as_ref() == Some(&port) {
                        serial.connected = None;
                        if serial.auto_reconnect {
                            let descriptor =
                                serial.ports.iter().find(|item| item.id == port).cloned();
                            serial.reconnect = Some(WebReconnectState {
                                port: port.clone(),
                                vendor_id: descriptor.as_ref().and_then(|item| item.vendor_id),
                                product_id: descriptor.as_ref().and_then(|item| item.product_id),
                                attempts: 0,
                                next_attempt_at: 0.0,
                                task_id: None,
                            });
                        }
                    }
                    serial.status = "设备已断开".to_owned();
                }
                WebAppEvent::Sent { bytes, .. } => {
                    serial.status = format!("发送成功（{bytes} 字节）");
                }
                WebAppEvent::SignalsChanged { signal, value, .. } => {
                    match signal {
                        tool_application::web::SignalKind::Dtr => serial.dtr = value,
                        tool_application::web::SignalKind::Rts => serial.rts = value,
                    }
                    serial.status = format!("{signal:?} 已更新");
                }
                WebAppEvent::TerminalSettingsChanged {
                    merge_window_ms,
                    max_entries,
                } => {
                    self.terminal_panel.merge_window_ms = merge_window_ms;
                    self.terminal_panel.set_max_entries(max_entries);
                }
                WebAppEvent::TaskFailed { id, error } => {
                    if self
                        .plugins
                        .pending_lua_file_requests
                        .borrow()
                        .contains_key(&id)
                    {
                        plugin_lua_file_resolution = Some((id, false, error.clone()));
                    }
                    if self
                        .plugins
                        .pending_lua_serial_requests
                        .borrow()
                        .contains_key(&id)
                    {
                        plugin_lua_serial_resolution = Some((id, Err(error.clone())));
                    }
                    if serial
                        .reconnect
                        .as_ref()
                        .is_some_and(|pending| pending.task_id == Some(id))
                    {
                        let mut stop = false;
                        if let Some(pending) = serial.reconnect.as_mut() {
                            pending.task_id = None;
                            pending.attempts = pending.attempts.saturating_add(1);
                            if pending.attempts >= 10 {
                                stop = true;
                            } else {
                                pending.next_attempt_at =
                                    web_now_seconds() + web_reconnect_delay(pending.attempts);
                            }
                        }
                        if stop {
                            serial.reconnect = None;
                            serial.status = format!("自动重连失败（已尝试 10 次）：{error}");
                        } else {
                            serial.status = format!("自动重连失败，将稍后重试：{error}");
                        }
                    } else {
                        serial.status = format!("操作失败：{error}");
                    }
                }
                WebAppEvent::TaskCancelled { id } => {
                    if self
                        .plugins
                        .pending_lua_file_requests
                        .borrow()
                        .contains_key(&id)
                    {
                        plugin_lua_file_resolution =
                            Some((id, false, "Lua 插件文件读取已取消".to_owned()));
                    }
                    if self
                        .plugins
                        .pending_lua_serial_requests
                        .borrow()
                        .contains_key(&id)
                    {
                        plugin_lua_serial_resolution =
                            Some((id, Err("Lua 插件串口授权已取消".to_owned())));
                    }
                    if let Some(pending) = serial
                        .reconnect
                        .as_mut()
                        .filter(|pending| pending.task_id == Some(id))
                    {
                        pending.task_id = None;
                        pending.attempts = pending.attempts.saturating_add(1);
                        pending.next_attempt_at =
                            web_now_seconds() + web_reconnect_delay(pending.attempts);
                        serial.status = "自动重连任务已取消，将稍后重试".to_owned();
                    } else {
                        serial.status = "操作已取消".to_owned();
                    }
                }
            }
            drop(serial);
            if let Some((_name, source)) = theme_import {
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
    }

    fn resolve_web_lua_file_request(&mut self, task_id: TaskId, ok: bool, result: &str) {
        let Some((host, request_id)) = self
            .plugins
            .pending_lua_file_requests
            .borrow_mut()
            .remove(&task_id)
        else {
            return;
        };
        let completion = PluginHostCompletion {
            request_id,
            result: if ok {
                Ok(PluginValue::String(result.to_owned()))
            } else {
                Err(result.to_owned())
            },
        };
        if let Err(error) = host.complete_request(completion) {
            self.serial.borrow_mut().status = format!("Lua 文件请求失败：{error}");
        }
    }

    fn resolve_web_lua_serial_request(
        &mut self,
        task_id: TaskId,
        result: Result<PluginSerialDevice, String>,
    ) {
        let Some((host, request_id)) = self
            .plugins
            .pending_lua_serial_requests
            .borrow_mut()
            .remove(&task_id)
        else {
            return;
        };
        let completion = PluginHostCompletion {
            request_id,
            result: result.map(|device| {
                PluginValue::from_json(
                    &serde_json::to_value(device).unwrap_or(serde_json::Value::Null),
                )
            }),
        };
        if let Err(error) = host.complete_request(completion) {
            self.serial.borrow_mut().status = format!("Lua 串口授权请求失败：{error}");
        }
    }

    fn poll_web_lua_host_requests(&mut self) {
        let hosts = self
            .plugins
            .records
            .iter()
            .filter_map(|record| record.host.clone())
            .collect::<Vec<_>>();
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        for host in hosts {
            for request in host.take_pending_requests() {
                if let PluginHostPendingRequest::SerialRequestDevice { request_id, .. } = request {
                    match runtime.dispatch(AppCommand::RequestPort) {
                        Ok(CommandOutcome::Pending { task_id, .. }) => {
                            self.plugins
                                .pending_lua_serial_requests
                                .borrow_mut()
                                .insert(task_id, (host.clone(), request_id));
                        }
                        Ok(CommandOutcome::Done) => {
                            let _ = host.complete_request(PluginHostCompletion {
                                request_id,
                                result: Err("浏览器串口授权请求未启动".to_owned()),
                            });
                        }
                        Err(error) => {
                            let _ = host.complete_request(PluginHostCompletion {
                                request_id,
                                result: Err(error),
                            });
                        }
                    }
                    continue;
                }
                let PluginHostPendingRequest::FileOpenText {
                    request_id,
                    title,
                    extensions,
                    ..
                } = request
                else {
                    continue;
                };
                let Some(document) = web_sys::window().and_then(|window| window.document()) else {
                    let _ = host.complete_request(PluginHostCompletion {
                        request_id,
                        result: Err("浏览器文件选择器不可用".to_owned()),
                    });
                    continue;
                };
                let Some(input) = document
                    .create_element("input")
                    .ok()
                    .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())
                else {
                    let _ = host.complete_request(PluginHostCompletion {
                        request_id,
                        result: Err("创建文件选择器失败".to_owned()),
                    });
                    continue;
                };
                input.set_type("file");
                let accept = extensions
                    .iter()
                    .filter(|extension| extension.as_str() != "*")
                    .map(|extension| {
                        if extension.starts_with('.') {
                            extension.clone()
                        } else {
                            format!(".{extension}")
                        }
                    })
                    .collect::<Vec<_>>();
                if !accept.is_empty() {
                    input.set_accept(&accept.join(","));
                }
                let host_for_change = host.clone();
                let request_id_for_change = request_id.clone();
                let pending = self.plugins.pending_lua_file_requests.clone();
                let runtime_for_change = runtime.clone();
                let title_for_task = title.clone();
                let closure =
                    Closure::wrap(Box::new(move |event: web_sys::Event| {
                        let Some(input) = event
                            .target()
                            .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
                        else {
                            return;
                        };
                        let Some(file) = input.files().and_then(|files| files.get(0)) else {
                            let _ = host_for_change.complete_request(PluginHostCompletion {
                                request_id: request_id_for_change.clone(),
                                result: Err("未选择文件".to_owned()),
                            });
                            return;
                        };
                        let future = async move {
                            JsFuture::from(file.text())
                                .await
                                .map_err(|error| format!("读取文件失败：{error:?}"))
                                .and_then(|value| {
                                    value
                                        .as_string()
                                        .ok_or_else(|| "文件不是有效文本".to_owned())
                                })
                        };
                        if let CommandOutcome::Pending { task_id, .. } = runtime_for_change
                            .load_text("lua_host_file", title_for_task.clone(), future)
                        {
                            pending.borrow_mut().insert(
                                task_id,
                                (host_for_change.clone(), request_id_for_change.clone()),
                            );
                        }
                    }) as Box<dyn FnMut(web_sys::Event)>);
                if input
                    .add_event_listener_with_callback("change", closure.as_ref().unchecked_ref())
                    .is_ok()
                {
                    closure.forget();
                    input.click();
                } else {
                    let _ = host.complete_request(PluginHostCompletion {
                        request_id,
                        result: Err("打开文件选择器失败".to_owned()),
                    });
                }
            }
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
            let Some(input) = document
                .create_element("input")
                .ok()
                .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())
            else {
                continue;
            };
            input.set_type("file");
            let panel_id = panel_id.to_owned();
            let field_id = field_id.to_owned();
            let runtime = runtime.clone();
            let repaint = ctx.clone();
            let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
                let Some(input) = event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
                else {
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
            }) as Box<dyn FnMut(web_sys::Event)>);
            if input
                .add_event_listener_with_callback("change", closure.as_ref().unchecked_ref())
                .is_ok()
            {
                closure.forget();
                input.click();
            }
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
                    command = Some(if serial.tx_hex {
                        AppCommand::SendHex {
                            port,
                            hex: serial.send_input.clone(),
                            strict: serial.hex_strict,
                        }
                    } else {
                        AppCommand::SendText {
                            port,
                            text: format!("{}{}", serial.send_input, serial.line_ending.suffix()),
                        }
                    });
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

    fn dispatch_serial(&mut self, command: AppCommand, ctx: &egui::Context) {
        if matches!(
            &command,
            AppCommand::Connect { .. }
                | AppCommand::Disconnect { .. }
                | AppCommand::Reconnect { .. }
        ) {
            self.serial.borrow_mut().reconnect = None;
        }
        let status = match self.runtime.as_ref() {
            Some(runtime) => match runtime.dispatch(command) {
                Ok(CommandOutcome::Pending { message, .. }) => message,
                Ok(CommandOutcome::Done) => "操作完成".to_owned(),
                Err(error) => format!("操作失败：{error}"),
            },
            None => "当前浏览器不支持 Web Serial".to_owned(),
        };
        self.serial.borrow_mut().status = status;
        ctx.request_repaint();
    }

    fn select_web_port(&mut self, selected: Option<PortId>) {
        select_web_port_state(&mut self.serial.borrow_mut(), selected);
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

        // The sender already owns Ctrl+Enter so it can validate the current
        // input and append the configured line ending before sending.
        if command_id == CMD_SEND {
            return;
        }
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
                if let Some(runtime) = self.runtime.as_ref()
                    && let Err(error) = runtime.dispatch(AppCommand::ExecutePluginCommand {
                        plugin_id: plugin_id.to_owned(),
                        command_id: plugin_command_id.to_owned(),
                        context: serde_json::json!({
                            "source": "web_command_palette",
                            "command": plugin_command_id,
                        }),
                    })
                {
                    self.serial.borrow_mut().status = error;
                }
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
            if serial.tx_hex {
                if let Some(error) = web_hex_error(&input, serial.hex_strict) {
                    serial.send_error = Some(error);
                    return;
                }
                (
                    AppCommand::SendHex {
                        port,
                        hex: serial.send_input.clone(),
                        strict: serial.hex_strict,
                    },
                    serial.send_input.clone(),
                )
            } else {
                (
                    AppCommand::SendText {
                        port,
                        text: format!("{}{}", serial.send_input, serial.line_ending.suffix()),
                    },
                    serial.send_input.clone(),
                )
            }
        };
        record_web_send_history(&mut self.serial.borrow_mut(), history);
        self.serial.borrow_mut().send_error = None;
        self.dispatch_serial(command, ctx);
        self.persist_settings();
    }
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
            Some(PanelKind::Builtin(BuiltinPanel::Replay)) => self.web_replay_panel_ui(ui),
            Some(PanelKind::Builtin(BuiltinPanel::Plugins)) => self.web_plugins_ui(ui),
            Some(PanelKind::Builtin(BuiltinPanel::Chart)) => {
                let started = self.perf.begin_frame();
                self.chart_panel.ui(ui);
                self.perf.record_chart_render(started);
            }
            Some(PanelKind::Builtin(BuiltinPanel::Terminal)) => {
                let started = self.perf.begin_frame();
                self.terminal_panel.ui(ui);
                self.perf.record_terminal_render(started);
            }
            Some(PanelKind::Builtin(BuiltinPanel::Sender)) => self.shared_sender_ui(ui),
            Some(PanelKind::Builtin(BuiltinPanel::Logs)) => {
                let started = self.perf.begin_frame();
                self.bottom_log_panel.ui(ui);
                self.perf.record_log_render(started);
            }
            Some(PanelKind::Builtin(BuiltinPanel::Settings)) => self.settings_ui(ui),
            Some(PanelKind::Dynamic { suffix }) => self.dynamic_panels.ui_body(ui, &suffix),
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
                transport_view
                    .as_ref()
                    .and_then(|view| view.connected.clone())
                    .or_else(|| serial.connected.clone()),
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
        let mut selected_port = selected.clone();
        let mut command = None;
        let mut cancel_reconnect = false;
        let reconnect_port = reconnect
            .as_ref()
            .map(|(port, _, _)| PortId::new(port.clone()));
        let mut collapsed_changed = false;
        ui.horizontal_wrapped(|ui| {
            let color = if connecting {
                theme::yellow()
            } else if connected.is_some() {
                theme::green()
            } else {
                theme::red()
            };
            let connected_label = connected
                .as_ref()
                .and_then(|id| ports.iter().find(|port| &port.id == id))
                .map(|port| web_port_display_name(port, &port_aliases))
                .unwrap_or_else(|| "未连接".to_owned());
            let connected_label = if connecting {
                format!("{connected_label} · 连接中")
            } else {
                connected_label
            };
            if ui
                .selectable_label(
                    !collapsed,
                    egui::RichText::new(format!(
                        "{} 串口 ▸ {}",
                        ICON_CABLE.codepoint, connected_label
                    ))
                    .color(color),
                )
                .clicked()
            {
                collapsed_changed = true;
            }
            if !collapsed {
                let selected_text = selected_port
                    .as_ref()
                    .and_then(|id| ports.iter().find(|port| &port.id == id))
                    .map(|port| web_port_display_name(port, &port_aliases))
                    .or_else(|| selected_port.as_ref().map(ToString::to_string))
                    .unwrap_or_else(|| {
                        if ports.is_empty() {
                            "无已授权设备".to_owned()
                        } else {
                            "请选择设备".to_owned()
                        }
                    });
                egui::ComboBox::from_id_salt("web-top-port")
                    .width(120.0)
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        if ports.is_empty() {
                            ui.add_enabled(false, egui::Label::new("无已授权设备"));
                        } else {
                            for port in &ports {
                                let label = web_port_display_name(port, &port_aliases);
                                if ui
                                    .selectable_label(
                                        selected_port.as_ref() == Some(&port.id),
                                        label,
                                    )
                                    .clicked()
                                {
                                    selected_port = Some(port.id.clone());
                                    ui.close();
                                }
                            }
                        }
                    });
                let selected_open = selected_port
                    .as_ref()
                    .is_some_and(|port| connected.as_ref() == Some(port));
                if !connecting && selected_open {
                    if design::icon_button(ui, ICON_REFRESH, "重连").clicked()
                        && let Some(port) = selected_port.clone()
                    {
                        command = Some(AppCommand::Reconnect { port: port.clone() });
                    }
                } else if connecting {
                    ui.add_enabled_ui(false, |ui| {
                        let _ = design::button(
                            ui,
                            ICON_POWER_SETTINGS_NEW,
                            "连接中",
                            design::ButtonKind::Ghost,
                        );
                    });
                } else if selected_port.is_some() {
                    if design::button(
                        ui,
                        ICON_POWER_SETTINGS_NEW,
                        "打开",
                        design::ButtonKind::Ghost,
                    )
                    .clicked()
                        && let Some(port) = selected_port.clone()
                    {
                        command = Some(AppCommand::Connect { port, settings });
                    }
                } else {
                    let _ = ui.add_enabled(
                        false,
                        egui::Button::new(design::icon_text(ICON_POWER_SETTINGS_NEW, "打开")),
                    );
                }
                let mut close = ui.add_enabled(
                    selected_open,
                    egui::Button::new(design::icon_text(ICON_LINK_OFF, "关闭")),
                );
                if !selected_open {
                    close = close.on_disabled_hover_text("端口未打开");
                }
                if close.clicked()
                    && let Some(port) = selected_port.clone()
                {
                    command = Some(AppCommand::Disconnect { port });
                }
            } else if connected.is_some() {
                ui.label(
                    egui::RichText::new(format!("· {}", settings_label(settings)))
                        .color(theme::text_secondary()),
                );
            }
            if let Some((port, attempts, next_attempt_at)) = reconnect {
                let remaining = (next_attempt_at - web_now_seconds()).max(0.0);
                let label = format!(
                    "{} 重连中 {} {:.1}s ({}/{})",
                    ICON_REFRESH.codepoint,
                    port,
                    remaining,
                    attempts + 1,
                    10
                );
                ui.label(egui::RichText::new(label).color(theme::yellow()))
                    .on_hover_text("点击取消自动重连");
                if design::icon_button(ui, ICON_CANCEL, "取消重连").clicked() {
                    cancel_reconnect = true;
                }
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
        if collapsed_changed {
            self.serial.borrow_mut().top_bar_serial_collapsed = !collapsed;
            self.persist_settings();
        }
        if cancel_reconnect {
            self.serial.borrow_mut().reconnect = None;
            if let Some(port) = reconnect_port {
                self.dispatch_serial(AppCommand::CancelReconnect { port }, ui.ctx());
            } else {
                self.serial.borrow_mut().status = "已取消自动重连".to_owned();
            }
        }
        if selected_port != selected {
            self.select_web_port(selected_port);
        }
        if let Some(command) = command {
            self.dispatch_serial(command, ui.ctx());
        }
    }

    fn render_status_bar(&mut self, ui: &mut egui::Ui) {
        let recording_status = self.runtime.as_ref().map(WebRuntime::query_recording);
        let recording_running = recording_status
            .as_ref()
            .is_some_and(|status| status.stats.running);
        let recording_paused = recording_status
            .as_ref()
            .is_some_and(|status| status.stats.paused);
        let recording_events = recording_status
            .as_ref()
            .map_or(0, |status| status.stats.events_written);
        let recording_bytes = recording_status
            .as_ref()
            .map_or(0, |status| status.stats.bytes_written);
        let recording_error = recording_status
            .as_ref()
            .and_then(|status| status.stats.last_error.clone());
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
                transport_view
                    .as_ref()
                    .and_then(|view| view.connected.clone())
                    .or_else(|| serial.connected.clone()),
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
        let terminal_dropped = self.terminal_panel.take_dropped_events();
        if terminal_dropped > 0 {
            self.push_web_notification(
                "terminal-data-loss",
                WebNotificationLevel::Error,
                format!("接收区缓冲已满，丢失 {terminal_dropped} 条最旧事件"),
            );
        }
        let log_dropped = self.bottom_log_panel.take_dropped_events();
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
        let notifications = self.current_web_notifications();
        let mut signal_command = None;
        ui.horizontal(|ui| {
            design::status_pill(
                ui,
                if connecting {
                    theme::yellow()
                } else if connected.is_some() {
                    theme::green()
                } else {
                    theme::text_secondary()
                },
                if connecting {
                    format!("{} 连接中", port_label)
                } else if connected.is_some() {
                    format!("{} @ {}", port_label, settings_label(settings))
                } else {
                    port_label
                },
            );
            design::status_pill(
                ui,
                if recording {
                    theme::red()
                } else {
                    theme::text_dimmed()
                },
                if recording {
                    if paused {
                        format!(
                            "录制已暂停 {events} 条 {:.1}MB",
                            bytes as f64 / 1024.0 / 1024.0
                        )
                    } else {
                        format!("录制中 {events} 条 {:.1}MB", bytes as f64 / 1024.0 / 1024.0)
                    }
                } else {
                    "未录制".to_owned()
                },
            );
            if let Some(port) = connected {
                ui.separator();
                if ui
                    .add(egui::Button::new(if dtr { "DTR 高" } else { "DTR 低" }).small())
                    .on_hover_text("切换 DTR")
                    .clicked()
                {
                    signal_command = Some(AppCommand::SetDtr {
                        port: port.clone(),
                        value: !dtr,
                    });
                }
                if ui
                    .add(egui::Button::new(if rts { "RTS 高" } else { "RTS 低" }).small())
                    .on_hover_text("切换 RTS")
                    .clicked()
                {
                    signal_command = Some(AppCommand::SetRts { port, value: !rts });
                }
            }
            self.web_ui_contribution_slot(ui, "status_bar.left");
            if let Some(notification) = notifications.first() {
                ui.separator();
                ui.label(
                    egui::RichText::new(notification.text.clone())
                        .color(notification.level.color()),
                )
                .on_hover_text(&notification.text);
                if notifications.len() > 1 {
                    let overflow_id = ui.id().with("web_notification_overflow");
                    let overflow_response =
                        ui.small_button(format!("通知 {} 条", notifications.len()));
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
                                    for item in &notifications {
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
            }
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
                if let Some(error) = recorder_error.as_deref() {
                    ui.colored_label(theme::red(), "录制错误")
                        .on_hover_text(error);
                }
            });
        });
        if let Some(command) = signal_command {
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
        let recorder = crate::web_perf::WebRecorderPerf {
            running: recording_status
                .as_ref()
                .is_some_and(|status| status.stats.running),
            queued_events: recording_status
                .as_ref()
                .map_or(0, |status| status.stats.backlog_events),
            queued_bytes: recording_status
                .as_ref()
                .map_or(0, |status| status.stats.backlog_bytes),
            seconds_behind: recording_status
                .as_ref()
                .map_or(0.0, |status| status.stats.seconds_behind),
            recorded_events: recording_status
                .as_ref()
                .map_or(0, |status| status.stats.events_written),
            recorded_bytes: recording_status
                .as_ref()
                .map_or(0, |status| status.stats.bytes_written),
            write_bytes_per_sec: 0,
            incomplete: recording_status
                .as_ref()
                .is_some_and(|status| status.stats.incomplete),
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

    fn web_command_palette_ui(&mut self, ctx: &egui::Context) {
        if !self.command_palette_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.command_palette_open = false;
            return;
        }

        let query = self.command_palette_query.trim().to_lowercase();
        let mut entries = self
            .web_palette_commands()
            .into_iter()
            .filter(|command| query.is_empty() || command.title.to_lowercase().contains(&query))
            .collect::<Vec<_>>();
        entries.sort_by_key(|command| {
            self.command_usage_order
                .iter()
                .position(|id| id == &command.id)
                .unwrap_or(usize::MAX)
        });

        if entries.is_empty() {
            self.command_palette_selected = None;
        } else if self
            .command_palette_selected
            .is_none_or(|index| index >= entries.len())
        {
            self.command_palette_selected = Some(0);
        }
        if !entries.is_empty() {
            if ctx.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
                let current = self.command_palette_selected.unwrap_or(0);
                self.command_palette_selected = Some((current + 1) % entries.len());
            }
            if ctx.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
                let current = self.command_palette_selected.unwrap_or(0);
                self.command_palette_selected = Some(if current == 0 {
                    entries.len() - 1
                } else {
                    current - 1
                });
            }
        }

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
            if let Some(runtime) = self.runtime.as_ref() {
                runtime.update_task(
                    state.task_id,
                    format!(
                        "正在导出 {}：已写入 {} 条",
                        if state.stem == "terminal" {
                            "终端"
                        } else {
                            "日志"
                        },
                        state.offset,
                    ),
                );
            }
            self.serial.borrow_mut().status = format!(
                "正在导出 {}：已写入 {} 条",
                if state.stem == "terminal" {
                    "终端"
                } else {
                    "日志"
                },
                state.offset,
            );
            ctx.request_repaint();
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("设置");
        ui.separator();
        let nav_id = ui.id().with("web-settings-category");
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
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_FOLDER, "工作区");
                ui.separator();
                ui.label("布局、主题和数据偏好会保存在当前浏览器的本地存储中。");
                ui.label("浏览器构建使用内嵌字体，不依赖本地文件系统。");
                ui.horizontal_wrapped(|ui| {
                    ui.label("工作区布局");
                    if ui
                        .button("恢复默认布局")
                        .on_hover_text("仅重置面板位置，不修改主题、串口和插件状态")
                        .clicked()
                    {
                        self.panels.reset_tiles_layout();
                        self.layout_dirty = true;
                    }
                });
                let mut bottom_visible = self.panels.bottom_visible();
                if ui.checkbox(&mut bottom_visible, "显示底部面板").changed() {
                    self.panels.set_bottom_visible(bottom_visible);
                    self.persist_settings();
                }
            });

            ui.add_space(8.0);
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_PALETTE, "外观");
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("界面主题");
                    egui::ComboBox::from_id_salt("web-theme")
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
                            .text("等宽字体"),
                    )
                    .changed()
                {
                    self.terminal_panel.font_size = font_size;
                    self.bottom_log_panel.font_size = font_size;
                    self.persist_settings();
                }
            });
        }

        if category == 1 {
            ui.add_space(8.0);
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_NETWORK_CHECK, "网络");
                ui.separator();
                ui.label("串口能力通过浏览器 Web Serial 提供，仅支持已授权设备。");
                let reconnect_changed = {
                    let mut serial = self.serial.borrow_mut();
                    ui.checkbox(&mut serial.auto_reconnect, "设备拔出后自动重连")
                        .changed()
                };
                if reconnect_changed {
                    self.persist_settings();
                }
            });
            ui.add_space(8.0);
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_TUNE, "数据");
                ui.separator();
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
                    if let Some(runtime) = self.runtime.as_ref() {
                        let _ = runtime.dispatch(AppCommand::SetTerminalMergeWindow {
                            ms: self.terminal_panel.merge_window_ms,
                        });
                        let _ = runtime.dispatch(AppCommand::SetTerminalMaxEntries {
                            max: terminal_max_entries,
                        });
                    }
                    self.persist_settings();
                }
            });
        }

        if category == 2 {
            ui.add_space(8.0);
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_KEYBOARD, "快捷键");
                ui.separator();
                ui.label("快捷键配置与桌面端使用相同的命令 ID，并保存在当前浏览器。");
                ui.add_space(4.0);
                let commands = self.web_keymap_commands();
                for command in &commands {
                    let bindings = self.keymap.get_bindings(&command.id);
                    let recording = self.key_recording.as_deref() == Some(command.id.as_str());
                    ui.horizontal(|ui| {
                        ui.set_min_height(28.0);
                        ui.label(egui::RichText::new(&command.title).size(14.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !bindings.is_empty() && ui.small_button("清除").clicked() {
                                self.keymap.set_bindings(&command.id, Vec::new());
                                self.persist_settings();
                            }
                            if recording {
                                ui.colored_label(theme::yellow(), "按下按键…");
                            } else if ui.small_button("录制").clicked() {
                                self.key_recording = Some(command.id.clone());
                            }
                            let text = if bindings.is_empty() {
                                "未绑定".to_owned()
                            } else {
                                bindings
                                    .iter()
                                    .map(KeyBinding::display)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            };
                            ui.label(egui::RichText::new(text).color(theme::cyan()));
                        });
                    });
                    ui.separator();
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("恢复默认快捷键").clicked() {
                        self.keymap = Keymap::default();
                        self.key_recording = None;
                        self.persist_settings();
                    }
                    ui.label("↑/↓ 浏览发送历史；Ctrl+Enter 发送当前内容。");
                });
                ui.small("浏览器保留系统级快捷键；录制时按 Escape 取消。");
            });
        }

        if category == 3 {
            ui.add_space(8.0);
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_APPS, "插件设置");
                ui.separator();
                ui.label("浏览器与桌面端执行同一份 Lua 插件；浏览器使用纯 Rust Lua VM。");
                ui.label("权限会在导入时显示，插件默认关闭，启用状态保存在当前浏览器。");
                if ui.button("打开插件面板").clicked() {
                    self.panels
                        .open_tab(PanelId::builtin(tool_panels::PANEL_PLUGINS));
                    self.layout_dirty = true;
                }
            });
        }

        if category == 4 {
            let update_status = self
                .runtime
                .as_ref()
                .map(WebRuntime::query_update)
                .unwrap_or_default();
            ui.add_space(8.0);
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_INFO, "关于与重置");
                ui.separator();
                ui.label(format!(
                    "硬件调试工作台 v{} · Web",
                    env!("CARGO_PKG_VERSION")
                ));
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
                if ui.button("恢复默认布局").clicked() {
                    self.panels.reset_tiles_layout();
                    self.layout_dirty = true;
                }
                if ui.button("恢复默认设置").clicked() {
                    let network_ports = {
                        let mut serial = self.serial.borrow_mut();
                        let network_ports = serial
                            .network_ports
                            .iter()
                            .map(NetworkSerialConfig::port_id)
                            .collect::<Vec<_>>();
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
                    if let Some(runtime) = self.runtime.as_ref() {
                        for port in network_ports {
                            let _ = runtime.dispatch(AppCommand::RemoveNetworkPort { port });
                        }
                    }
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
                    if let Some(runtime) = self.runtime.as_ref() {
                        let _ = runtime.dispatch(AppCommand::SetTerminalMergeWindow {
                            ms: default_terminal_merge_window_ms(),
                        });
                        let _ = runtime.dispatch(AppCommand::SetTerminalMaxEntries {
                            max: default_terminal_max_entries(),
                        });
                    }
                    self.bottom_log_panel
                        .set_max_entries(default_terminal_max_entries());
                    self.terminal_panel.font_size = default_font_size();
                    self.bottom_log_panel.font_size = default_font_size();
                    apply_web_theme(ui.ctx(), self.ui_theme);
                    self.persist_settings();
                }
            });
        }
    }

    fn serial_panel_ui(&mut self, ui: &mut egui::Ui, show_settings: bool, show_ports: bool) {
        let ctx = ui.ctx().clone();
        let transport_view = self.runtime.as_ref().map(WebRuntime::query_transport);
        let previous_settings = self.serial.borrow().settings;
        let mut actions = Vec::new();
        let mut network_to_add = None;
        let mut network_to_remove = None;
        let mut auto_reconnect_changed = false;

        // Keep the same visual order as Native: serial parameters, recording,
        // then the available-port list. The shared SerialPanel supplies the
        // individual sections; the composition root only orders them.
        if show_settings {
            let mut serial = self.serial.borrow_mut();
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_TUNE, "串口参数");
                ui.separator();
                SerialPanel::settings_ui(ui, &mut serial.settings);
                if ui
                    .checkbox(&mut serial.auto_reconnect, "串口拔出后自动重连")
                    .changed()
                {
                    auto_reconnect_changed = true;
                }
            });
            drop(serial);
            self.web_recording_ui(ui);
        }

        let (settings_changed, metadata_changed) = {
            let mut serial = self.serial.borrow_mut();
            let previous_aliases = serial.port_aliases.clone();
            let previous_groups = serial.port_groups.clone();
            let ports: Vec<SerialPortItem> = transport_view
                .as_ref()
                .map(|view| &view.ports)
                .unwrap_or(&serial.ports)
                .iter()
                .map(|port| SerialPortItem {
                    id: port.id.to_string(),
                    label: port.label.clone(),
                    kind: if port.kind == PortKind::Network {
                        "网络".to_owned()
                    } else {
                        String::new()
                    },
                })
                .collect();
            let connected = transport_view
                .as_ref()
                .and_then(|view| view.connected.clone())
                .or_else(|| serial.connected.clone())
                .map(|port| port.to_string());
            let connecting = transport_view
                .as_ref()
                .filter(|view| view.connecting)
                .and_then(|_| serial.selected_port.as_ref().map(ToString::to_string));
            let status = transport_view
                .as_ref()
                .filter(|view| !view.status.is_empty())
                .map(|view| view.status.clone())
                .unwrap_or_else(|| serial.status.clone());
            let connected_is_network = serial
                .connected
                .as_ref()
                .is_some_and(|port| port.as_str().starts_with("network://"));
            let network_port_available = ports.iter().any(|port| port.kind == "网络");
            let capabilities = if connected_is_network
                || (self
                    .runtime
                    .as_ref()
                    .is_some_and(|runtime| !runtime.serial_supported())
                    && network_port_available)
            {
                tool_platform::TransportCapabilities::WEB_NETWORK
            } else {
                transport_view
                    .as_ref()
                    .map(|view| view.capabilities)
                    .unwrap_or(tool_platform::TransportCapabilities::WEB_SERIAL)
            };
            let state = &mut *serial;
            let WebSerialState {
                settings,
                send_input,
                tx_hex,
                dtr,
                rts,
                port_aliases,
                port_groups,
                ..
            } = state;
            let mut view = SerialView {
                ports: &ports,
                connected: connected.as_deref(),
                connecting: connecting.as_deref(),
                status: &status,
                settings,
                send_input,
                tx_hex,
                dtr,
                rts,
                capabilities,
                show_ports,
                show_sender: false,
                metadata: show_settings.then_some(SerialPortMetadata {
                    aliases: port_aliases,
                    groups: port_groups,
                }),
            };
            if show_settings {
                design::card().show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    design::section_header(ui, ICON_CABLE, "可用端口");
                    ui.separator();
                    actions.extend(SerialPanel::port_list_ui(ui, &mut view));
                    SerialPanel::signal_ui(ui, &mut view, &mut actions);
                    ui.add_space(8.0);
                    ui.set_min_width(ui.available_width());
                    design::section_header(ui, ICON_NETWORK_CHECK, "网络串口");
                    ui.separator();
                    ui.label("通过 WebSocket 连接 Nexus/Moonraker 的 G-code 接口。");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("地址");
                        ui.add(
                            egui::TextEdit::singleline(&mut state.network_host)
                                .desired_width((ui.available_width() - 120.0).clamp(110.0, 190.0))
                                .hint_text("IP 或主机名"),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut state.network_port)
                                .desired_width(64.0)
                                .hint_text("7125"),
                        );
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("密钥");
                        ui.add(
                            egui::TextEdit::singleline(&mut state.network_api_key)
                                .desired_width((ui.available_width() - 100.0).clamp(120.0, 220.0))
                                .password(true)
                                .hint_text("API Key（可选）"),
                        );
                        if ui.button("添加并连接").clicked() {
                            let host = state.network_host.trim().to_owned();
                            let port = state.network_port.trim().parse::<u16>();
                            if host.is_empty() {
                                state.status = "请输入网络串口地址".to_owned();
                            } else if !matches!(port, Ok(port) if port > 0) {
                                state.status = "网络端口格式错误（1-65535）".to_owned();
                            } else {
                                let config = NetworkSerialConfig {
                                    host,
                                    port: port.unwrap_or(7125),
                                    api_key: (!state.network_api_key.trim().is_empty())
                                        .then(|| state.network_api_key.trim().to_owned()),
                                };
                                if state
                                    .network_ports
                                    .iter()
                                    .any(|item| item.port_id() == config.port_id())
                                {
                                    state.status = "网络串口已存在，正在连接".to_owned();
                                }
                                network_to_add = Some(config);
                            }
                        }
                    });
                    if !state.network_ports.is_empty() {
                        ui.small("已保存的网络串口：");
                        for config in &state.network_ports {
                            ui.horizontal(|ui| {
                                ui.label(format!("• {}", config.display_name()));
                                if ui.small_button("移除").clicked() {
                                    network_to_remove = Some(config.port_id());
                                }
                            });
                        }
                    }
                });
            }
            (
                *settings != previous_settings,
                serial.port_aliases != previous_aliases || serial.port_groups != previous_groups,
            )
        };

        if let Some(config) = network_to_add {
            let port = config.port_id();
            {
                let mut serial = self.serial.borrow_mut();
                if !serial
                    .network_ports
                    .iter()
                    .any(|item| item.port_id() == port)
                {
                    serial.network_ports.push(config.clone());
                }
            }
            self.select_web_port(Some(port.clone()));
            if let Some(runtime) = self.runtime.as_ref() {
                let _ = runtime.dispatch(AppCommand::RegisterNetworkPort {
                    config: config.clone(),
                });
                let settings = self.serial.borrow().settings;
                self.dispatch_serial(AppCommand::Connect { port, settings }, &ctx);
            }
            self.persist_settings();
        }

        if let Some(port) = network_to_remove {
            self.serial
                .borrow_mut()
                .network_ports
                .retain(|config| config.port_id() != port);
            if let Some(runtime) = self.runtime.as_ref() {
                let _ = runtime.dispatch(AppCommand::RemoveNetworkPort { port });
            }
            self.persist_settings();
        }

        if settings_changed || auto_reconnect_changed || metadata_changed {
            if settings_changed {
                let settings = self.serial.borrow().settings;
                if let Some(runtime) = self.runtime.as_ref()
                    && let Err(error) = runtime.dispatch(AppCommand::SetSerialSettings { settings })
                {
                    self.serial.borrow_mut().status = format!("串口参数更新失败：{error}");
                }
                let mut serial = self.serial.borrow_mut();
                if let Some(port) = serial.selected_port.clone() {
                    serial.port_profiles.insert(port.to_string(), settings);
                }
            }
            self.persist_settings();
        }
        for action in actions {
            let command = match action {
                SerialAction::Refresh => AppCommand::RefreshPorts,
                SerialAction::RequestPort => AppCommand::RequestPort,
                SerialAction::Connect { port, .. } => {
                    let port = PortId::new(port);
                    self.select_web_port(Some(port.clone()));
                    let settings = self.serial.borrow().settings;
                    AppCommand::Connect { port, settings }
                }
                SerialAction::Disconnect { port } => AppCommand::Disconnect {
                    port: PortId::new(port),
                },
                SerialAction::SendText { port, text } => AppCommand::SendText {
                    port: PortId::new(port),
                    text,
                },
                SerialAction::SendHex { port, hex } => AppCommand::SendHex {
                    port: PortId::new(port),
                    hex,
                    strict: self.serial.borrow().hex_strict,
                },
                SerialAction::SetDtr { port, value } => AppCommand::SetDtr {
                    port: PortId::new(port),
                    value,
                },
                SerialAction::SetRts { port, value } => AppCommand::SetRts {
                    port: PortId::new(port),
                    value,
                },
            };
            self.dispatch_serial(command, &ctx);
        }
    }

    fn serial_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.serial_panel_ui(ui, true, true);
            });
    }

    /// Render the same rich sender used by the Native composition root.
    ///
    /// Only the final action dispatch remains platform-specific.  Keeping the
    /// state adapter here (rather than duplicating widgets) means browser and
    /// desktop get identical mode switching, history navigation, validation,
    /// periodic controls and signal controls.
    fn shared_sender_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
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
            let connected = transport
                .as_ref()
                .and_then(|view| view.connected.clone())
                .or_else(|| serial.connected.clone());
            (ports, connected)
        };

        let mut line_ending = {
            let serial = self.serial.borrow();
            match serial.line_ending {
                WebLineEnding::None => SendLineEnding::None,
                WebLineEnding::Lf => SendLineEnding::Lf,
                WebLineEnding::Cr => SendLineEnding::Cr,
                WebLineEnding::Crlf => SendLineEnding::Crlf,
            }
        };
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
                max_history: WEB_MAX_SEND_HISTORY,
                layout: SendLayout::Vertical,
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
            serial.line_ending = match line_ending {
                SendLineEnding::None => WebLineEnding::None,
                SendLineEnding::Lf => WebLineEnding::Lf,
                SendLineEnding::Cr => WebLineEnding::Cr,
                SendLineEnding::Crlf => WebLineEnding::Crlf,
            };
        }

        for action in actions {
            match action {
                SendAction::SendText { port, text } => {
                    let history = self.serial.borrow().send_input.clone();
                    self.dispatch_serial(
                        AppCommand::SendText {
                            port: PortId::new(port),
                            text,
                        },
                        &ctx,
                    );
                    record_shared_send_history(
                        &mut self.serial.borrow_mut().send_history,
                        history,
                        WEB_MAX_SEND_HISTORY,
                    );
                }
                SendAction::SendHex { port, hex, strict } => {
                    let history = self.serial.borrow().send_input.clone();
                    self.dispatch_serial(
                        AppCommand::SendHex {
                            port: PortId::new(port),
                            hex,
                            strict,
                        },
                        &ctx,
                    );
                    record_shared_send_history(
                        &mut self.serial.borrow_mut().send_history,
                        history,
                        WEB_MAX_SEND_HISTORY,
                    );
                }
                SendAction::SetDtr { port, value } => {
                    self.dispatch_serial(
                        AppCommand::SetDtr {
                            port: PortId::new(port),
                            value,
                        },
                        &ctx,
                    );
                }
                SendAction::SetRts { port, value } => {
                    self.dispatch_serial(
                        AppCommand::SetRts {
                            port: PortId::new(port),
                            value,
                        },
                        &ctx,
                    );
                }
            }
        }
        self.web_ui_contribution_slot(ui, "send.toolbar");
        self.persist_settings();
    }

    // Kept in source only as a migration reference while old persisted Web
    // state is rolled forward. The production composition root uses the
    // shared sender above; do not call this legacy duplicate.
    #[cfg(any())]
    fn sender_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut commands = Vec::new();
        let mut persist = false;
        let application_connected = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.query_transport().connected);

        {
            let mut serial = self.serial.borrow_mut();
            let connected = application_connected.or_else(|| serial.connected.clone());
            let connected_label = connected.as_ref().map_or("未连接", PortId::as_str);

            ui.heading("发送器");
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label("发送到");
                ui.add_enabled(false, egui::Button::new(connected_label));
                ui.separator();
                if ui
                    .add(egui::Button::selectable(!serial.tx_hex, "文本").corner_radius(6.0))
                    .clicked()
                {
                    serial.tx_hex = false;
                    serial.send_error = None;
                    persist = true;
                }
                if ui
                    .add(egui::Button::selectable(serial.tx_hex, "HEX").corner_radius(6.0))
                    .clicked()
                {
                    serial.tx_hex = true;
                    serial.send_error = None;
                    persist = true;
                }
                if serial.tx_hex
                    && ui
                        .checkbox(&mut serial.hex_strict, "严格")
                        .on_hover_text("严格模式：每个 HEX token 必须是完整的两位字节")
                        .changed()
                {
                    serial.send_error = None;
                    persist = true;
                }
                ui.add_enabled_ui(!serial.tx_hex, |ui| {
                    egui::ComboBox::from_id_salt("web-send-line-ending")
                        .selected_text(serial.line_ending.label())
                        .show_ui(ui, |ui| {
                            for ending in WebLineEnding::ALL {
                                if ui
                                    .selectable_value(
                                        &mut serial.line_ending,
                                        ending,
                                        ending.label(),
                                    )
                                    .changed()
                                {
                                    persist = true;
                                }
                            }
                        });
                });
            });

            let input_height = (ui.available_height() - 150.0).max(110.0);
            let response = egui::ScrollArea::vertical()
                .id_salt("web-send-input-scroll")
                .max_height(input_height)
                .show(ui, |ui| {
                    ui.add_sized(
                        egui::vec2(ui.available_width(), input_height),
                        egui::TextEdit::multiline(&mut serial.send_input)
                            .desired_width(f32::INFINITY)
                            .hint_text(if connected.is_some() {
                                "输入要发送的文本或 HEX，Ctrl+Enter 发送"
                            } else {
                                "请选择或打开串口后发送"
                            }),
                    )
                })
                .inner;
            if response.changed() {
                serial.send_error = None;
                serial.periodic_send_count = 0;
                serial.history_index = None;
                serial.saved_input.clear();
            }

            if response.has_focus() && !serial.send_history.is_empty() {
                if ui.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
                    match serial.history_index {
                        None => {
                            serial.saved_input = serial.send_input.clone();
                            serial.history_index = Some(0);
                            serial.send_input = serial.send_history[0].clone();
                        }
                        Some(index) if index + 1 < serial.send_history.len() => {
                            serial.history_index = Some(index + 1);
                            serial.send_input = serial.send_history[index + 1].clone();
                        }
                        _ => {}
                    }
                } else if ui.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
                    match serial.history_index {
                        Some(0) => {
                            serial.history_index = None;
                            serial.send_input = std::mem::take(&mut serial.saved_input);
                        }
                        Some(index) => {
                            serial.history_index = Some(index - 1);
                            serial.send_input = serial.send_history[index - 1].clone();
                        }
                        None => {}
                    }
                }
            } else if !response.has_focus() {
                serial.history_index = None;
                serial.saved_input.clear();
            }

            let input_trimmed = serial.send_input.trim().to_owned();
            let hex_error = if serial.tx_hex && !input_trimmed.is_empty() {
                web_hex_error(&input_trimmed, serial.hex_strict)
            } else {
                None
            };
            let can_send = connected.is_some() && !input_trimmed.is_empty() && hex_error.is_none();
            let ctrl_enter = response.has_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter) && input.modifiers.command);

            ui.horizontal_wrapped(|ui| {
                let send = ui
                    .add_enabled(can_send, egui::Button::new("发送"))
                    .on_disabled_hover_text(
                        hex_error
                            .as_deref()
                            .unwrap_or("请先连接串口并输入要发送的内容"),
                    );
                if can_send && (send.clicked() || ctrl_enter) {
                    let port = connected.clone().expect("send enabled only with a port");
                    let command = if serial.tx_hex {
                        AppCommand::SendHex {
                            port,
                            hex: serial.send_input.clone(),
                            strict: serial.hex_strict,
                        }
                    } else {
                        AppCommand::SendText {
                            port,
                            text: format!("{}{}", serial.send_input, serial.line_ending.suffix()),
                        }
                    };
                    commands.push(command);
                    let history_text = serial.send_input.clone();
                    record_web_send_history(&mut serial, history_text);
                    serial.send_error = None;
                    serial.periodic_send_count = 0;
                    persist = true;
                }
                if ui.button("清空").clicked() {
                    serial.send_input.clear();
                    serial.send_error = None;
                    serial.periodic_send_count = 0;
                }
                ui.menu_button("历史", |ui| {
                    if serial.send_history.is_empty() {
                        ui.label("暂无发送历史");
                    } else {
                        let history = serial.send_history.clone();
                        egui::ScrollArea::vertical()
                            .max_height(240.0)
                            .show(ui, |ui| {
                                for item in history.iter().take(WEB_MAX_SEND_HISTORY) {
                                    let label = if item.chars().count() > 80 {
                                        format!("{}…", item.chars().take(80).collect::<String>())
                                    } else {
                                        item.clone()
                                    };
                                    if ui.button(label).clicked() {
                                        serial.send_input = item.clone();
                                        serial.history_index = None;
                                        ui.close();
                                    }
                                }
                            });
                    }
                });
            });

            ui.horizontal_wrapped(|ui| {
                let interval = serial.periodic_interval_ms.clone();
                let interval_valid = interval
                    .trim()
                    .parse::<f64>()
                    .map(|value| value > 0.0)
                    .unwrap_or(false);
                let can_toggle = interval_valid || serial.periodic_enabled;
                if ui
                    .add_enabled(
                        can_toggle,
                        egui::Checkbox::new(&mut serial.periodic_enabled, "周期发送"),
                    )
                    .changed()
                {
                    serial.periodic_send_count = 0;
                    serial.periodic_next_at = serial.periodic_enabled.then(|| {
                        ui.ctx().input(|input| input.time)
                            + interval.trim().parse::<f64>().unwrap_or(1000.0) / 1000.0
                    });
                    persist = true;
                }
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut serial.periodic_interval_ms)
                            .desired_width(72.0),
                    )
                    .changed()
                {
                    serial.periodic_next_at = None;
                    persist = true;
                }
                ui.label("ms");
                if let Some(error) = hex_error.as_deref() {
                    ui.colored_label(theme::red(), error);
                }
                if !interval_valid && serial.periodic_enabled {
                    ui.colored_label(theme::red(), "间隔必须 > 0ms");
                }
            });

            ui.horizontal_wrapped(|ui| {
                let enabled = connected.is_some();
                ui.add_enabled_ui(enabled, |ui| {
                    if ui.checkbox(&mut serial.dtr, "DTR").changed()
                        && let Some(port) = connected.clone()
                    {
                        commands.push(AppCommand::SetDtr {
                            port,
                            value: serial.dtr,
                        });
                    }
                    if ui.checkbox(&mut serial.rts, "RTS").changed()
                        && let Some(port) = connected.clone()
                    {
                        commands.push(AppCommand::SetRts {
                            port,
                            value: serial.rts,
                        });
                    }
                });
            });

            if let Some(error) = serial.send_error.as_deref() {
                ui.colored_label(theme::red(), error);
            }
        }

        // Native renders send.toolbar beside the built-in send/history
        // controls. The standalone Web sender keeps the same contribution
        // contract; it is placed after the shared controls so the browser
        // runtime never needs a second plugin UI implementation.
        self.web_ui_contribution_slot(ui, "send.toolbar");

        for command in commands {
            self.dispatch_serial(command, &ctx);
        }
        if persist {
            self.persist_settings();
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
        let running = status.as_ref().is_some_and(|status| status.stats.running);
        let paused = status.as_ref().is_some_and(|status| status.stats.paused);
        let stopping = status.as_ref().is_some_and(|status| status.stats.stopping);
        let events = status
            .as_ref()
            .map_or(0, |status| status.stats.events_written);
        let bytes = status
            .as_ref()
            .map_or(0, |status| status.stats.bytes_written);
        let backlog_events = status
            .as_ref()
            .map_or(0, |status| status.stats.backlog_events);
        let backlog_bytes = status
            .as_ref()
            .map_or(0, |status| status.stats.backlog_bytes);
        let last_error = status
            .as_ref()
            .and_then(|status| status.stats.last_error.as_deref());
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
            if let Some(runtime) = self.runtime.as_ref() {
                let mode = match self.recording_mode {
                    WebRecordMode::StandardReplay => {
                        tool_application::recording::RecordModeView::StandardReplay
                    }
                    WebRecordMode::RawSerial => {
                        tool_application::recording::RecordModeView::RawSerial
                    }
                };
                let _ = runtime.dispatch(AppCommand::SetRecordingMode { mode });
            }
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
                    if let Some(runtime) = self.runtime.as_ref() {
                        let _ = runtime.dispatch(command);
                    }
                }
            }
        }
    }

    fn web_plugins_ui(&mut self, ui: &mut egui::Ui) {
        let Some(runtime) = self.runtime.clone() else {
            ui.label("当前浏览器没有可用的 Application runtime");
            return;
        };
        let plugin_view = runtime.query_plugins();
        let marketplace_view = runtime.query_marketplace();

        design::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.heading("Web 插件");
            ui.separator();
            ui.label("浏览器与桌面端使用同一份 Lua 插件和宿主 API，不维护第二套插件源码。");
            ui.label("manifest、源码和启用状态保存在当前浏览器本地存储中。");
            if ui
                .button("导入 Lua 插件（plugin.json + main.lua）")
                .clicked()
            {
                self.request_web_plugin_files(ui.ctx());
            }
        });
        ui.add_space(design::SECTION_GAP);

        let mut marketplace_url_changed = false;
        design::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label("市场索引");
                marketplace_url_changed = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.marketplace_url)
                            .desired_width(420.0)
                            .hint_text("https://…/registry.json"),
                    )
                    .changed();
                if ui
                    .add_enabled(!marketplace_view.refreshing, egui::Button::new("刷新市场"))
                    .clicked()
                {
                    self.request_web_marketplace_refresh(ui.ctx());
                }
            });
        });
        if marketplace_url_changed {
            self.persist_settings();
        }

        if let Some(registry) = marketplace_view.registry.clone() {
            self.plugins_panel
                .set_market_registry_view(registry, self.marketplace_url.clone());
        } else {
            self.plugins_panel.clear_market_registry();
        }
        self.plugins_panel
            .set_market_refreshing(marketplace_view.refreshing);
        if let Some(error) = marketplace_view.error {
            self.plugins_panel.set_market_error(error);
        }
        self.plugins_panel.set_installed_ids(
            plugin_view
                .summaries
                .iter()
                .map(|summary| summary.id.clone())
                .collect(),
        );
        if let Some(registry) = marketplace_view.registry.as_ref() {
            for entry in &registry.plugins {
                if marketplace_view.installing.iter().any(|id| id == &entry.id) {
                    self.plugins_panel.mark_installing(&entry.id);
                } else {
                    self.plugins_panel.clear_installing(&entry.id);
                }
            }
        }

        let events =
            self.plugins_panel
                .ui_with_view(ui, &plugin_view.summaries, &plugin_view.diagnostics);
        for event in events {
            match event {
                tool_panels::PluginPanelEvent::Status(message, _is_error) => {
                    self.serial.borrow_mut().status = message;
                }
                tool_panels::PluginPanelEvent::Enable(plugin_id) => {
                    if let Err(error) = runtime.dispatch(AppCommand::EnablePlugin { plugin_id }) {
                        self.serial.borrow_mut().status = error;
                    }
                }
                tool_panels::PluginPanelEvent::Disable(plugin_id) => {
                    if let Err(error) = runtime.dispatch(AppCommand::DisablePlugin { plugin_id }) {
                        self.serial.borrow_mut().status = error;
                    }
                }
                tool_panels::PluginPanelEvent::RefreshMarket => {
                    self.request_web_marketplace_refresh(ui.ctx());
                }
                tool_panels::PluginPanelEvent::InstallPlugin(plugin_id) => {
                    if let Some(entry) = marketplace_view.registry.as_ref().and_then(|registry| {
                        registry.plugins.iter().find(|entry| entry.id == plugin_id)
                    }) {
                        let Some(manifest_url) = entry.manifest_url.clone() else {
                            self.serial.borrow_mut().status =
                                format!("插件 {} 缺少统一 plugin.json，无法安装", entry.id);
                            continue;
                        };
                        let Some(main_url) = entry.main_url.clone() else {
                            self.serial.borrow_mut().status =
                                format!("插件 {} 缺少统一 main.lua，无法安装", entry.id);
                            continue;
                        };
                        self.install_web_marketplace_entry(
                            entry.id.clone(),
                            manifest_url,
                            main_url,
                        );
                    }
                }
                tool_panels::PluginPanelEvent::UninstallPlugin(plugin_id) => {
                    self.uninstall_web_plugin(&plugin_id);
                }
            }
        }
        // The shared plugin panel represents Restart as Disable followed by
        // Enable once the old runtime has actually gone away. Native performs
        // this hand-off in its composition root; Web must do the same instead
        // of silently dropping the pending restart request.
        for plugin_id in self.plugins_panel.take_pending_restart() {
            if let Err(error) = runtime.dispatch(AppCommand::EnablePlugin { plugin_id }) {
                self.serial.borrow_mut().status = error;
            }
        }
        self.web_plugin_settings_ui(ui);
    }

    fn web_plugin_settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = Vec::new();
        for (index, record) in self.plugins.records.iter_mut().enumerate() {
            let settings = record.persisted.manifest.contributes.settings.clone();
            if settings.is_empty() {
                continue;
            }
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.heading(format!("{} 设置", record.persisted.manifest.name));
                ui.separator();
                for setting in settings {
                    let entry = record
                        .persisted
                        .settings
                        .entry(setting.id.clone())
                        .or_insert_with(|| {
                            setting
                                .default
                                .clone()
                                .unwrap_or_else(|| serde_json::Value::String(String::new()))
                        });
                    let mut did_change = false;
                    match setting.kind.as_str() {
                        "boolean" | "bool" | "checkbox" => {
                            let mut value = entry.as_bool().unwrap_or(false);
                            did_change = ui.checkbox(&mut value, &setting.title).changed();
                            if did_change {
                                *entry = serde_json::Value::Bool(value);
                            }
                        }
                        "number" => {
                            let min = setting.min.unwrap_or(f64::NEG_INFINITY);
                            let max = setting.max.unwrap_or(f64::INFINITY);
                            let mut value = entry.as_f64().unwrap_or_default().clamp(min, max);
                            ui.horizontal(|ui| {
                                ui.label(&setting.title);
                                let mut drag = egui::DragValue::new(&mut value);
                                if let Some(step) = setting.step.filter(|step| *step > 0.0) {
                                    drag = drag.speed(step);
                                }
                                if min.is_finite() && max.is_finite() {
                                    drag = drag.range(min..=max);
                                }
                                did_change = ui.add(drag).changed();
                            });
                            if did_change {
                                *entry = serde_json::json!(value.clamp(min, max));
                            }
                        }
                        "slider" | "range" => {
                            let min = setting.min.unwrap_or(0.0);
                            let max = setting.max.unwrap_or(100.0).max(min);
                            let mut value = entry.as_f64().unwrap_or(min).clamp(min, max);
                            ui.horizontal(|ui| {
                                ui.label(&setting.title);
                                let mut slider = egui::Slider::new(&mut value, min..=max);
                                if let Some(step) = setting.step.filter(|step| *step > 0.0) {
                                    slider = slider.step_by(step);
                                }
                                did_change = ui.add(slider).changed();
                            });
                            if did_change {
                                *entry = serde_json::json!(value);
                            }
                        }
                        "select" | "choice" | "enum" | "dropdown" => {
                            let options = setting.options.clone();
                            let mut selected = entry.clone();
                            let selected_text = options
                                .iter()
                                .find(|option| web_setting_option_value(option) == selected)
                                .map(web_setting_option_label)
                                .unwrap_or_else(|| web_setting_option_label(&selected));
                            ui.horizontal(|ui| {
                                ui.label(&setting.title);
                                egui::ComboBox::from_id_salt((
                                    "web-plugin-setting",
                                    index,
                                    &setting.id,
                                ))
                                .selected_text(selected_text)
                                .show_ui(ui, |ui| {
                                    for option in &options {
                                        let value = web_setting_option_value(option);
                                        let label = web_setting_option_label(option);
                                        if ui
                                            .selectable_value(&mut selected, value, label)
                                            .changed()
                                        {
                                            did_change = true;
                                        }
                                    }
                                });
                            });
                            if did_change {
                                *entry = selected;
                            }
                        }
                        "textarea" => {
                            let mut value = entry.as_str().unwrap_or_default().to_owned();
                            ui.label(&setting.title);
                            did_change = ui
                                .add(
                                    egui::TextEdit::multiline(&mut value)
                                        .desired_rows(setting.rows.unwrap_or(4).clamp(2, 20))
                                        .desired_width(ui.available_width()),
                                )
                                .changed();
                            if did_change {
                                *entry = serde_json::Value::String(value);
                            }
                        }
                        _ => {
                            let mut value = entry.as_str().unwrap_or_default().to_owned();
                            ui.horizontal(|ui| {
                                ui.label(&setting.title);
                                did_change =
                                    ui.add(egui::TextEdit::singleline(&mut value)).changed();
                            });
                            if did_change {
                                *entry = serde_json::Value::String(value);
                            }
                        }
                    }
                    if let Some(description) = setting.description.as_deref() {
                        ui.small(description);
                    }
                    if did_change {
                        changed.push((
                            index,
                            serde_json::to_string(&record.persisted.settings)
                                .unwrap_or_else(|_| "{}".to_owned()),
                        ));
                    }
                }
            });
            ui.add_space(6.0);
        }
        for (index, settings_json) in changed {
            if let Some(instance) = self
                .plugins
                .records
                .get(index)
                .and_then(|record| record.lua_instance)
            {
                let settings = serde_json::from_str::<serde_json::Value>(&settings_json)
                    .map(|value| PluginValue::from_json(&value));
                if let Ok(settings) = settings
                    && let Err(error) = self.web_lua.update_settings(instance, settings)
                {
                    self.serial.borrow_mut().status = error.to_string();
                }
            }
            self.persist_settings();
        }
    }

    #[cfg(any())]
    #[allow(dead_code)]
    fn web_plugins_legacy_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("插件");
        ui.separator();
        let plugin_summaries = self
            .runtime
            .as_ref()
            .map(WebRuntime::query_plugins)
            .map(|view| view.summaries)
            .unwrap_or_default();
        let running_plugins = plugin_summaries
            .iter()
            .filter(|plugin| plugin.state == PluginStateView::Running)
            .count();
        let failed_plugins = plugin_summaries
            .iter()
            .filter(|plugin| plugin.state == PluginStateView::Failed)
            .count();
        ui.horizontal_wrapped(|ui| {
            design::status_pill(
                ui,
                theme::cyan(),
                format!("已发现 {}", plugin_summaries.len()),
            );
            design::status_pill(ui, theme::green(), format!("运行 {running_plugins}"));
            if failed_plugins > 0 {
                design::status_pill(ui, theme::red(), format!("异常 {failed_plugins}"));
            }
        });
        ui.add_space(4.0);
        let tab = self.plugins.tab;
        design::elevated_card().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (selected, next, label) in [
                    (
                        tab == WebPluginTab::Installed,
                        WebPluginTab::Installed,
                        "已安装",
                    ),
                    (
                        tab == WebPluginTab::Marketplace,
                        WebPluginTab::Marketplace,
                        "市场",
                    ),
                ] {
                    if ui
                        .add(
                            egui::Button::selectable(
                                selected,
                                design::icon_text(
                                    if next == WebPluginTab::Installed {
                                        ICON_APPS
                                    } else {
                                        ICON_SHOPPING_CART
                                    },
                                    label,
                                ),
                            )
                            .corner_radius(7.0)
                            .min_size(egui::vec2(112.0, 32.0)),
                        )
                        .clicked()
                    {
                        self.plugins.tab = next;
                    }
                }
            });
        });
        ui.add_space(design::SECTION_GAP);
        if tab == WebPluginTab::Marketplace {
            if self.marketplace.entries.is_empty()
                && !self.marketplace.loading
                && self.marketplace.error.is_none()
            {
                self.request_web_marketplace_refresh(ui.ctx());
            }
            let mut marketplace_url_changed = false;
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.heading("插件市场");
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("索引");
                    marketplace_url_changed = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.marketplace.url)
                                .desired_width(420.0)
                                .hint_text("https://…/registry.json"),
                        )
                        .changed();
                    if ui
                        .add_enabled(!self.marketplace.loading, egui::Button::new("刷新"))
                        .clicked()
                    {
                        self.request_web_marketplace_refresh(ui.ctx());
                    }
                });
                if self.marketplace.loading {
                    ui.label("正在读取插件市场…");
                }
                if let Some(error) = &self.marketplace.error {
                    ui.colored_label(theme::red(), error);
                }
                if self.marketplace.entries.is_empty() && !self.marketplace.loading {
                    ui.label("暂无可用插件");
                }
            });
            if marketplace_url_changed {
                self.persist_settings();
            }
            let marketplace_entries = self.marketplace.entries.clone();
            for entry in marketplace_entries {
                design::card().show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.heading(&entry.name);
                        ui.label(format!("{} v{}", entry.id, entry.version));
                        if let Some(runtime) = &entry.runtime {
                            ui.label(format!("runtime={runtime}"));
                        }
                    });
                    if let Some(description) = &entry.description {
                        ui.label(description);
                    }
                    if !entry.permissions.is_empty() {
                        ui.small(format!("权限：{}", entry.permissions.join(", ")));
                    }
                    let web_package = entry.manifest_url.is_some() && entry.main_url.is_some();
                    ui.horizontal_wrapped(|ui| {
                        if web_package {
                            let installing =
                                self.marketplace.installing.as_deref() == Some(&entry.id);
                            if ui
                                .add_enabled(
                                    !installing && self.marketplace.installing.is_none(),
                                    egui::Button::new(if installing {
                                        "安装中…"
                                    } else {
                                        "安装 Lua 插件"
                                    }),
                                )
                                .clicked()
                            {
                                self.install_web_marketplace_entry(entry.clone(), ui.ctx());
                            }
                        } else {
                            ui.label("仅提供桌面插件包");
                        }
                    });
                });
                ui.add_space(6.0);
            }
            return;
        }

        ui.add_space(2.0);
        design::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.heading("Web 插件");
            ui.separator();
            ui.label(
                "浏览器与桌面端使用同一份 Lua 插件和 plugin.json；浏览器由纯 Rust Lua VM 执行。",
            );
            ui.label("文件、串口权限等平台能力由宿主异步提供，插件 API 不随平台分叉。");
            if ui
                .button("导入 Lua 插件（plugin.json + main.lua）")
                .clicked()
            {
                self.request_web_plugin_files(ui.ctx());
            }
            ui.label("导入后清单、源码和启用状态保存在当前浏览器本地存储中。");
        });

        ui.add_space(8.0);
        if plugin_summaries.is_empty() {
            design::empty_state(ui, ICON_CABLE, "尚未导入 Web 插件");
            return;
        }

        let mut changes = Vec::new();
        let mut commands = Vec::new();
        let mut settings_updates = Vec::new();
        for (index, record) in self.plugins.records.iter_mut().enumerate() {
            let summary = plugin_summaries
                .iter()
                .find(|summary| summary.id == record.persisted.manifest.id);
            let enabled_from_view =
                summary.is_some_and(|summary| !matches!(summary.state, PluginStateView::Disabled));
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.heading(&record.persisted.manifest.name);
                    ui.label(format!(
                        "{} v{} · Plugin API {}",
                        record.persisted.manifest.id,
                        record.persisted.manifest.version,
                        record.persisted.manifest.api_version
                    ));
                    let mut enabled = enabled_from_view;
                    if ui.checkbox(&mut enabled, "启用").changed() {
                        changes.push((index, enabled));
                    }
                });
                if let Some(description) = &record.persisted.manifest.description {
                    ui.label(description);
                }
                ui.label(format!("入口：{}", record.persisted.manifest.live_main()));
                if !record.persisted.manifest.live_permissions().is_empty() {
                    ui.label(format!(
                        "权限：{}",
                        record.persisted.manifest.live_permissions().join(", ")
                    ));
                }
                if !record.persisted.manifest.contributes.settings.is_empty() {
                    ui.collapsing("设置", |ui| {
                        for setting in &record.persisted.manifest.contributes.settings {
                            let entry = record
                                .persisted
                                .settings
                                .entry(setting.id.clone())
                                .or_insert_with(|| {
                                    setting
                                        .default
                                        .clone()
                                        .unwrap_or_else(|| serde_json::Value::String(String::new()))
                                });
                            let mut changed = false;
                            match setting.kind.as_str() {
                                "boolean" => {
                                    let mut value = entry.as_bool().unwrap_or(false);
                                    changed = ui.checkbox(&mut value, &setting.title).changed();
                                    if changed {
                                        *entry = serde_json::Value::Bool(value);
                                    }
                                }
                                "number" => {
                                    let mut value = entry.as_f64().unwrap_or_default();
                                    ui.horizontal(|ui| {
                                        ui.label(&setting.title);
                                        changed =
                                            ui.add(egui::DragValue::new(&mut value)).changed();
                                    });
                                    if changed {
                                        *entry = serde_json::json!(value);
                                    }
                                }
                                "textarea" => {
                                    let mut value = entry.as_str().unwrap_or_default().to_owned();
                                    ui.label(&setting.title);
                                    changed = ui
                                        .add(
                                            egui::TextEdit::multiline(&mut value)
                                                .desired_rows(6)
                                                .desired_width(ui.available_width()),
                                        )
                                        .changed();
                                    if changed {
                                        *entry = serde_json::Value::String(value);
                                    }
                                }
                                _ => {
                                    let mut value = entry.as_str().unwrap_or_default().to_owned();
                                    ui.horizontal(|ui| {
                                        ui.label(&setting.title);
                                        changed = ui
                                            .add(egui::TextEdit::singleline(&mut value))
                                            .changed();
                                    });
                                    if changed {
                                        *entry = serde_json::Value::String(value);
                                    }
                                }
                            }
                            if let Some(description) = setting.description.as_deref() {
                                ui.small(description);
                            }
                            if changed {
                                settings_updates.push((
                                    index,
                                    serde_json::to_string(&record.persisted.settings)
                                        .unwrap_or_else(|_| "{}".to_owned()),
                                ));
                            }
                        }
                    });
                }
                if enabled_from_view && record.lua_instance.is_some() {
                    for command in &record.persisted.manifest.contributes.commands {
                        if ui.button(&command.title).clicked() {
                            commands.push((index, command.id.clone()));
                        }
                    }
                    for contribution in &record.persisted.manifest.contributes.ui {
                        if contribution.kind == "button"
                            && contribution.slot == "plugins.toolbar"
                            && let Some(command_id) = contribution.command.as_deref()
                        {
                            let title = contribution.title.as_deref().unwrap_or(command_id);
                            let response = ui.button(title);
                            let response = if let Some(tooltip) = contribution.tooltip.as_deref() {
                                response.on_hover_text(tooltip)
                            } else {
                                response
                            };
                            if response.clicked() {
                                commands.push((index, command_id.to_owned()));
                            }
                        }
                    }
                }
                if record.loading {
                    ui.label("正在加载模块…");
                } else if record.lua_instance.is_some() && enabled_from_view {
                    design::status_pill(ui, theme::green(), "运行中");
                } else if let Some(error) = &record.error {
                    ui.colored_label(theme::red(), error);
                } else if enabled_from_view {
                    ui.label("等待模块加载");
                } else {
                    design::status_pill(ui, theme::text_dimmed(), "已禁用");
                }
            });
            ui.add_space(6.0);
        }
        for (index, command_id) in commands {
            let Some(plugin_id) = self
                .plugins
                .records
                .get(index)
                .map(|record| record.persisted.manifest.id.clone())
            else {
                continue;
            };
            let context = serde_json::json!({
                "source": "web_plugin_panel",
                "plugin_index": index,
            });
            if let Some(runtime) = self.runtime.as_ref()
                && let Err(error) = runtime.dispatch(AppCommand::ExecutePluginCommand {
                    plugin_id,
                    command_id,
                    context,
                })
            {
                self.serial.borrow_mut().status = error;
            }
        }
        for (index, settings_json) in settings_updates {
            if let Some(instance) = self
                .plugins
                .records
                .get(index)
                .and_then(|record| record.lua_instance)
            {
                let settings = serde_json::from_str::<serde_json::Value>(&settings_json)
                    .map(|value| PluginValue::from_json(&value));
                if let Ok(settings) = settings
                    && let Err(error) = self.web_lua.update_settings(instance, settings)
                {
                    self.serial.borrow_mut().status = error.to_string();
                }
            }
            self.persist_settings();
        }
        for (index, enabled) in changes {
            let Some(plugin_id) = self
                .plugins
                .records
                .get(index)
                .map(|record| record.persisted.manifest.id.clone())
            else {
                continue;
            };
            let command = if enabled {
                AppCommand::EnablePlugin { plugin_id }
            } else {
                AppCommand::DisablePlugin { plugin_id }
            };
            if let Some(runtime) = self.runtime.as_ref()
                && let Err(error) = runtime.dispatch(command)
            {
                self.serial.borrow_mut().status = error;
            }
        }
    }

    fn web_ui_contribution_context(&self, slot: &str) -> serde_json::Value {
        let serial = self.serial.borrow();
        let transport = self.runtime.as_ref().map(WebRuntime::query_transport);
        let connected = transport
            .as_ref()
            .and_then(|view| view.connected.clone())
            .or_else(|| serial.connected.clone());
        let open_ports = transport
            .as_ref()
            .map(|view| {
                view.ports
                    .iter()
                    .filter(|port| port.kind == PortKind::Serial)
                    .map(|port| port.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if slot.starts_with("send.") {
            serde_json::json!({
                "slot": slot,
                "send": {
                    "input": serial.send_input,
                    "target_port": connected,
                    "target_port_open": connected.is_some(),
                    "hex_mode": serial.tx_hex,
                    "line_ending": {
                        "label": serial.line_ending.label(),
                        "suffix": serial.line_ending.suffix(),
                    },
                    "periodic_enabled": serial.periodic_enabled,
                    "periodic_interval_ms": serial.periodic_interval_ms,
                },
                "serial": {
                    "selected_port": serial.selected_port,
                    "open_ports": open_ports,
                }
            })
        } else {
            serde_json::json!({ "slot": slot })
        }
    }

    /// Render the same chrome contribution slots that Native exposes. Lua
    /// contribution state stays in the browser composition root; the visible
    /// contract (slot, ordering, labels and command dispatch) remains identical.
    fn web_ui_contribution_slot(&mut self, ui: &mut egui::Ui, slot: &str) {
        let mut items = self
            .plugins
            .summaries()
            .into_iter()
            .filter(|summary| summary.state == PluginStateView::Running)
            .flat_map(|summary| {
                let plugin_id = summary.id;
                summary
                    .contributes
                    .ui
                    .into_iter()
                    .filter(|item| item.slot == slot && item.visible)
                    .map(move |item| {
                        let contribution_id = item.id;
                        (
                            plugin_id.clone(),
                            contribution_id.clone(),
                            item.kind,
                            item.title.unwrap_or_else(|| contribution_id.clone()),
                            item.command,
                            item.tooltip,
                            item.order,
                            item.enabled,
                            item.default,
                        )
                    })
            })
            .collect::<Vec<_>>();
        items.sort_by(|left, right| left.6.cmp(&right.6).then_with(|| left.3.cmp(&right.3)));

        let mut commands = Vec::new();
        for (plugin_id, contribution_id, kind, title, command, tooltip, order, enabled, default) in
            items
        {
            let _ = order;
            let state = self
                .plugins
                .contribution_value(&plugin_id, &contribution_id)
                .cloned()
                .unwrap_or(default);
            match kind.to_ascii_lowercase().as_str() {
                "separator" => {
                    ui.separator();
                }
                "label" | "status" => {
                    let display = state.as_str().unwrap_or(&title);
                    ui.label(egui::RichText::new(display).color(theme::text_secondary()));
                }
                "progress" => {
                    let value = state
                        .get("value")
                        .and_then(serde_json::Value::as_f64)
                        .or_else(|| state.as_f64())
                        .unwrap_or(0.0)
                        .clamp(0.0, 1.0) as f32;
                    let text = state
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    if state.get("visible").and_then(serde_json::Value::as_bool) == Some(false)
                        || (value <= 0.0 && text.is_empty())
                    {
                        continue;
                    }
                    ui.add(
                        egui::ProgressBar::new(value)
                            .desired_width(52.0)
                            .desired_height(8.0)
                            .text(text.to_owned()),
                    );
                }
                "toggle" => {
                    let current = state.as_bool().unwrap_or(false);
                    if ui.selectable_label(current, title).clicked() {
                        self.plugins.set_contribution_value(
                            &plugin_id,
                            &contribution_id,
                            serde_json::json!(!current),
                        );
                        if let Some(command) = command {
                            commands.push((plugin_id, command, contribution_id));
                        }
                    }
                }
                "button" | "small_button" | "" => {
                    let response = ui.add_enabled(enabled, egui::Button::new(title));
                    let response = if let Some(tooltip) = tooltip {
                        response.on_hover_text(tooltip)
                    } else {
                        response
                    };
                    if response.clicked()
                        && let Some(command) = command
                    {
                        commands.push((plugin_id, command, contribution_id));
                    }
                }
                _ => {
                    ui.add_enabled(false, egui::Button::new(title));
                }
            }
        }
        for (plugin_id, command_id, contribution_id) in commands {
            let mut context = self.web_ui_contribution_context(slot);
            if let Some(object) = context.as_object_mut() {
                object.insert(
                    "source".to_owned(),
                    serde_json::json!("web_ui_contribution"),
                );
                object.insert(
                    "contribution_id".to_owned(),
                    serde_json::json!(contribution_id),
                );
            }
            if let Some(runtime) = self.runtime.as_ref()
                && let Err(error) = runtime.dispatch(AppCommand::ExecutePluginCommand {
                    plugin_id,
                    command_id,
                    context,
                })
            {
                self.serial.borrow_mut().status = error;
            }
        }
    }
}

fn web_setting_option_value(option: &serde_json::Value) -> serde_json::Value {
    option
        .as_object()
        .and_then(|object| object.get("value"))
        .cloned()
        .unwrap_or_else(|| option.clone())
}

fn web_setting_option_label(option: &serde_json::Value) -> String {
    if let Some(label) = option
        .as_object()
        .and_then(|object| object.get("label"))
        .and_then(serde_json::Value::as_str)
    {
        return label.to_owned();
    }
    match option {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Null => String::new(),
        _ => option.to_string(),
    }
}

fn download_export(
    stem: &str,
    format: TerminalExportFormat,
    content: String,
) -> Result<(), String> {
    let extension = match format {
        TerminalExportFormat::Txt => "txt",
        TerminalExportFormat::Csv => "csv",
        TerminalExportFormat::Json => "json",
    };
    download_text_file(
        &format!("{stem}.{extension}"),
        match format {
            TerminalExportFormat::Txt => "text/plain",
            TerminalExportFormat::Csv => "text/csv",
            TerminalExportFormat::Json => "application/json",
        },
        content,
    )
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

fn replay_policy_option_view(policy: ReplayPolicyView) -> ReplayPolicyOption {
    match policy {
        ReplayPolicyView::AutoPreferRecorded => ReplayPolicyOption::AutoPreferRecorded,
        ReplayPolicyView::ExactRecorded => ReplayPolicyOption::ExactRecorded,
        ReplayPolicyView::ReparseRaw => ReplayPolicyOption::ReparseRaw,
    }
}

fn empty_replay_status() -> ReplayStatusView {
    ReplayStatusView {
        state: ReplayStateView::Empty,
        path: None,
        total_events: 0,
        cursor: 0,
        speed: 1.0,
        position_ms: 0,
        duration_ms: 0,
        policy: ReplayPolicyView::AutoPreferRecorded,
        effective_policy: ReplayPolicyView::AutoPreferRecorded,
        has_recorded_protocol: false,
        analyzer_cache_entries: 0,
        analyzer_cache_valid: false,
        analyzer_error: None,
        analyzer_warning: None,
        can_play: false,
        can_seek: false,
        block_reason: None,
        bookmarks: Vec::new(),
        load_report: None,
    }
}

fn web_keymap_title(command_id: &str) -> &'static str {
    BUILTIN_KEYMAP_COMMANDS
        .iter()
        .find(|command| command.id == command_id)
        .map_or("命令", |command| command.title)
}

fn web_port_display_name(port: &PortDescriptor, aliases: &BTreeMap<String, String>) -> String {
    let alias = aliases
        .get(port.id.as_str())
        .map(|alias| alias.trim())
        .filter(|alias| !alias.is_empty());
    match alias {
        Some(alias) => format!("{alias} ({})", port.id),
        None if port.label.trim().is_empty() => port.id.to_string(),
        None => format!("{} ({})", port.label, port.id),
    }
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

fn web_event_to_plugin_value(event: &Event) -> PluginValue {
    let payload = match &event.payload {
        Payload::Empty => PluginValue::Null,
        Payload::Bytes(bytes) => PluginValue::String(String::from_utf8_lossy(bytes).into_owned()),
        Payload::Text(text) => PluginValue::String(text.clone()),
        Payload::Json(value) => PluginValue::from_json(value),
    };
    PluginValue::Object(
        [
            ("id".to_owned(), PluginValue::Integer(event.id as i64)),
            (
                "timestamp_ms".to_owned(),
                PluginValue::Integer(event.timestamp_ms as i64),
            ),
            ("topic".to_owned(), PluginValue::String(event.topic.clone())),
            (
                "source".to_owned(),
                PluginValue::String(event.source.clone()),
            ),
            (
                "direction".to_owned(),
                PluginValue::String(format!("{:?}", event.direction).to_lowercase()),
            ),
            ("payload".to_owned(), payload),
            (
                "metadata".to_owned(),
                PluginValue::from_json(&event.metadata),
            ),
        ]
        .into_iter()
        .collect(),
    )
}

fn plugin_event_to_core_event(value: PluginValue) -> Result<Event, String> {
    let PluginValue::Object(mut object) = value else {
        return Err("Web Replay Analyzer 输出不是事件对象".to_owned());
    };
    let timestamp_ms = match object.remove("timestamp_ms") {
        Some(PluginValue::Integer(value)) if value >= 0 => value as u64,
        _ => return Err("Web Replay Analyzer 输出缺少有效 timestamp_ms".to_owned()),
    };
    let topic = match object.remove("topic") {
        Some(PluginValue::String(value)) if !value.is_empty() => value,
        _ => return Err("Web Replay Analyzer 输出缺少 topic".to_owned()),
    };
    let source = match object.remove("source") {
        Some(PluginValue::String(value)) => value,
        _ => "replay-analyzer:web".to_owned(),
    };
    let payload = match object.remove("payload").unwrap_or(PluginValue::Null) {
        PluginValue::Null => Payload::Empty,
        PluginValue::String(value) => Payload::Text(value),
        value => Payload::Json(
            value
                .to_json()
                .map_err(|error| format!("Web Replay Analyzer payload 无效：{error}"))?,
        ),
    };
    let metadata = object
        .remove("metadata")
        .unwrap_or(PluginValue::Object(BTreeMap::new()))
        .to_json()
        .map_err(|error| format!("Web Replay Analyzer metadata 无效：{error}"))?;
    Ok(Event {
        id: 0,
        timestamp_ms,
        topic,
        source,
        direction: Direction::Internal,
        payload,
        metadata,
    })
}

fn open_web_url(url: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.open_with_url(url);
    }
}

fn parse_web_plugin_files(files: &[(String, String)]) -> Result<WebPluginPersisted, String> {
    let manifest_text = files
        .iter()
        .find(|(name, _)| {
            name.rsplit_once('/')
                .map(|(_, leaf)| leaf.eq_ignore_ascii_case("plugin.json"))
                .unwrap_or_else(|| name.eq_ignore_ascii_case("plugin.json"))
        })
        .or_else(|| {
            files
                .iter()
                .find(|(name, _)| name.ends_with(".json") || name.ends_with(".JSON"))
        })
        .map(|(_, text)| text)
        .ok_or_else(|| "Web 插件缺少 plugin.json".to_owned())?;
    let manifest = serde_json::from_str::<WebPluginManifest>(manifest_text)
        .map_err(|error| format!("解析 Web 插件清单失败：{error}"))?;
    if manifest.id.trim().is_empty()
        || manifest.name.trim().is_empty()
        || manifest.version.trim().is_empty()
    {
        return Err("Web 插件清单必须包含 id、name 和 version".to_owned());
    }
    if manifest.runtime != "lua" {
        return Err(format!(
            "插件 {} 使用 runtime={}，浏览器只接受 runtime=lua；Web 与 Native 使用同一 main.lua",
            manifest.id, manifest.runtime
        ));
    }
    if manifest.api_version != WEB_PLUGIN_API_VERSION && manifest.api_version != "0.1" {
        return Err(format!(
            "插件 {} 使用插件 API {}，当前浏览器支持 {} / 0.1",
            manifest.id, manifest.api_version, WEB_PLUGIN_API_VERSION
        ));
    }
    let main_name = manifest
        .live_main()
        .rsplit_once('/')
        .map(|(_, leaf)| leaf)
        .unwrap_or(manifest.live_main());
    let source = files
        .iter()
        .find(|(name, _)| {
            let leaf = name.rsplit_once('/').map(|(_, leaf)| leaf).unwrap_or(name);
            leaf == main_name
        })
        .or_else(|| {
            files
                .iter()
                .find(|(name, _)| name.ends_with(".lua") || name.ends_with(".LUA"))
        })
        .map(|(_, text)| text.clone())
        .ok_or_else(|| format!("Web 插件缺少入口文件：{}", manifest.live_main()))?;
    let replay_source = manifest.replay.as_ref().and_then(|replay| {
        let replay_name = replay
            .main
            .rsplit_once('/')
            .map(|(_, leaf)| leaf)
            .unwrap_or(replay.main.as_str());
        files.iter().find_map(|(name, text)| {
            let leaf = name.rsplit_once('/').map(|(_, leaf)| leaf).unwrap_or(name);
            (leaf == replay_name).then_some(text.clone())
        })
    });
    if manifest.replay.is_some() && replay_source.is_none() {
        return Err(format!(
            "Web 插件声明了 replay，但缺少回放入口文件：{}",
            manifest
                .replay
                .as_ref()
                .map(|replay| replay.main.as_str())
                .unwrap_or("replay.lua")
        ));
    }
    let settings = manifest
        .contributes
        .settings
        .iter()
        .filter_map(|setting| {
            setting
                .default
                .clone()
                .map(|value| (setting.id.clone(), value))
        })
        .collect();
    Ok(WebPluginPersisted {
        manifest,
        source,
        replay_source,
        enabled: true,
        settings,
        storage: BTreeMap::new(),
        profiles: BTreeMap::new(),
    })
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

fn settings_label(settings: SerialSettings) -> String {
    format!(
        "{} {}{}{}",
        settings.baud_rate,
        settings.data_bits,
        match settings.parity {
            SerialParity::None => 'N',
            SerialParity::Odd => 'O',
            SerialParity::Even => 'E',
        },
        settings.stop_bits
    )
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
