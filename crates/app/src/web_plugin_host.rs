//! Browser implementation of the shared Lua plugin host protocol.
//!
//! This module deliberately exposes no JavaScript-worker ABI.  Lua receives
//! typed `PluginHostRequest`s and the browser composition translates those
//! requests into the same Application/DataBus commands used by the UI.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use tool_application::web::WebRuntime;
use tool_application::{AppCommand, CommandOutcome};
use tool_core::{Direction, Event, LogLevel as CoreLogLevel, Payload, topics};
use tool_platform::{PortId, SerialSettings};
use tool_plugin_api::{
    FileHandle, LogLevel, PluginError, PluginHostApi, PluginHostCompletion,
    PluginHostPendingRequest, PluginHostRequest, PluginResult, PluginSerialDevice, PluginUiCommand,
    PluginValue,
};

#[derive(Debug, Clone, Default)]
pub(crate) struct WebPluginData {
    pub(crate) settings: BTreeMap<String, serde_json::Value>,
    pub(crate) storage: BTreeMap<String, serde_json::Value>,
    pub(crate) profiles: BTreeMap<String, serde_json::Value>,
}

pub(crate) type WebPluginDataStore = Rc<RefCell<BTreeMap<String, WebPluginData>>>;

/// A per-plugin host.  Persisted settings/storage live in the shared store;
/// the runtime and plugin id are captured when the Lua instance is created.
pub(crate) struct WebPluginHost {
    runtime: WebRuntime,
    plugin_id: String,
    data: WebPluginDataStore,
    next_request_id: Cell<u64>,
    pending_requests: RefCell<Vec<PluginHostPendingRequest>>,
    completions: RefCell<Vec<PluginHostCompletion>>,
    files: RefCell<BTreeMap<String, String>>,
    opened_ports: RefCell<BTreeMap<PortId, ()>>,
}

impl WebPluginHost {
    pub(crate) fn new(
        runtime: WebRuntime,
        plugin_id: impl Into<String>,
        data: WebPluginDataStore,
    ) -> Self {
        Self {
            runtime,
            plugin_id: plugin_id.into(),
            data,
            next_request_id: Cell::new(1),
            pending_requests: RefCell::new(Vec::new()),
            completions: RefCell::new(Vec::new()),
            files: RefCell::new(BTreeMap::new()),
            opened_ports: RefCell::new(BTreeMap::new()),
        }
    }

    fn next_request_id(&self) -> String {
        let id = self.next_request_id.get();
        self.next_request_id.set(id.saturating_add(1));
        format!("{}:host:{}", self.plugin_id, id)
    }

    fn file_text(&self, file: &FileHandle) -> PluginResult<String> {
        self.files
            .borrow()
            .get(file.as_str())
            .cloned()
            .ok_or_else(|| {
                PluginError::Host(format!("未知或已失效的浏览器文件句柄：{}", file.as_str()))
            })
    }

    fn data_mut(&self) -> std::cell::RefMut<'_, WebPluginData> {
        std::cell::RefMut::map(self.data.borrow_mut(), |all| {
            all.entry(self.plugin_id.clone()).or_default()
        })
    }

    fn data(&self) -> Option<std::cell::Ref<'_, WebPluginData>> {
        let all = self.data.borrow();
        if !all.contains_key(&self.plugin_id) {
            return None;
        }
        Some(std::cell::Ref::map(all, |all| {
            all.get(&self.plugin_id).expect("checked above")
        }))
    }

    fn dispatch_done(&self, command: AppCommand) -> PluginResult<()> {
        match self.runtime.dispatch(command) {
            Ok(CommandOutcome::Done | CommandOutcome::Pending { .. }) => Ok(()),
            Err(error) => Err(PluginError::Host(error)),
        }
    }

    fn serial_devices(&self) -> PluginValue {
        let transport = self.runtime.query_transport();
        let devices = transport
            .ports
            .into_iter()
            .map(PluginSerialDevice::from)
            .collect::<Vec<_>>();
        PluginValue::from_json(
            &serde_json::to_value(devices).unwrap_or_else(|_| serde_json::Value::Array(vec![])),
        )
    }

    fn status(&self, port: &PortId) -> PluginValue {
        let transport = self.runtime.query_transport();
        let port_name = port.to_string();
        let open = transport.connected.as_ref() == Some(port);
        PluginValue::Object(
            [
                ("open".to_owned(), PluginValue::Bool(open)),
                ("connected".to_owned(), PluginValue::Bool(open)),
                ("port".to_owned(), PluginValue::String(port_name.clone())),
                (
                    "port_name".to_owned(),
                    if open {
                        PluginValue::String(port_name.clone())
                    } else {
                        PluginValue::Null
                    },
                ),
                (
                    "baud_rate".to_owned(),
                    if open {
                        PluginValue::Integer(i64::from(transport.settings.baud_rate))
                    } else {
                        PluginValue::Null
                    },
                ),
            ]
            .into_iter()
            .collect(),
        )
    }

    fn publish_ui(&self, command: PluginUiCommand) {
        let topic = match command.command.as_str() {
            "create_chart" | "create_form" | "create_gauge" | "create_attitude"
            | "create_table" => topics::UI_PANEL_CREATE,
            "remove_panel" => topics::UI_PANEL_REMOVE,
            "set_values" => topics::UI_PANEL_SET_VALUES,
            "set_value" => topics::UI_FORM_SET_VALUE,
            "set_enabled" => topics::UI_FORM_SET_ENABLED,
            "set_visible" => topics::UI_FORM_SET_VISIBLE,
            "table_set_rows" => topics::UI_TABLE_SET_ROWS,
            "table_append_rows" => topics::UI_TABLE_APPEND_ROWS,
            "table_remove_rows" => topics::UI_TABLE_REMOVE_ROWS,
            "table_clear" => topics::UI_TABLE_CLEAR,
            "set_contribution_value" => topics::UI_CONTRIBUTION_SET_VALUE,
            "set_status" => topics::UI_SET_STATUS,
            _ => return,
        };
        let payload = normalize_ui_payload(&self.plugin_id, &command.command, command.payload);
        self.runtime.publish_event(Event::new(
            topic,
            format!("plugin:{}", self.plugin_id),
            Direction::Internal,
            Payload::Json(payload.to_json().unwrap_or(serde_json::Value::Null)),
        ));
    }
}

fn normalize_ui_payload(plugin_id: &str, command: &str, payload: PluginValue) -> PluginValue {
    let kind = match command {
        "create_chart" => Some("chart"),
        "create_form" => Some("form"),
        "create_gauge" => Some("gauge"),
        "create_attitude" => Some("attitude"),
        "create_table" => Some("table"),
        _ => None,
    };
    let Some(kind) = kind else {
        return payload;
    };
    let PluginValue::Object(mut object) = payload else {
        return payload;
    };
    object.insert("kind".to_owned(), PluginValue::String(kind.to_owned()));
    object.insert(
        "plugin_id".to_owned(),
        PluginValue::String(plugin_id.to_owned()),
    );
    if !object.contains_key("id") {
        return PluginValue::Object(object);
    }
    if !object.contains_key("title") {
        let title = object
            .get("id")
            .cloned()
            .unwrap_or(PluginValue::String(kind.to_owned()));
        object.insert("title".to_owned(), title);
    }
    if kind == "chart" && !object.contains_key("topic_prefix") && !object.contains_key("topic") {
        object.insert(
            "topic_prefix".to_owned(),
            PluginValue::String("protocol.".to_owned()),
        );
    }
    if kind == "form" && !object.contains_key("fields") {
        object.insert("fields".to_owned(), PluginValue::Array(Vec::new()));
    }
    PluginValue::Object(object)
}

fn event_to_plugin_value(event: &Event) -> PluginValue {
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

impl PluginHostApi for WebPluginHost {
    fn take_pending_requests(&self) -> Vec<PluginHostPendingRequest> {
        std::mem::take(&mut *self.pending_requests.borrow_mut())
    }

    fn complete_request(&self, completion: PluginHostCompletion) -> PluginResult<()> {
        let result = match completion.result {
            Ok(PluginValue::String(text)) => {
                let handle = format!("{}:selected:{}", self.plugin_id, self.files.borrow().len());
                self.files.borrow_mut().insert(handle.clone(), text);
                Ok(PluginValue::String(handle))
            }
            Ok(value) => Ok(value),
            Err(error) => Err(error),
        };
        self.completions.borrow_mut().push(PluginHostCompletion {
            request_id: completion.request_id,
            result,
        });
        Ok(())
    }

    fn take_completions(&self) -> Vec<PluginHostCompletion> {
        std::mem::take(&mut *self.completions.borrow_mut())
    }

    fn request(&self, request: PluginHostRequest) -> PluginResult<PluginValue> {
        match request {
            PluginHostRequest::NowMs => Ok(PluginValue::Integer(
                web_sys::window()
                    .and_then(|window| window.performance())
                    .map(|performance| performance.now().max(0.0) as i64)
                    .unwrap_or_default(),
            )),
            PluginHostRequest::Log { level, message } => {
                let level = match level {
                    LogLevel::Trace => CoreLogLevel::Trace,
                    LogLevel::Debug => CoreLogLevel::Debug,
                    LogLevel::Info => CoreLogLevel::Info,
                    LogLevel::Warn => CoreLogLevel::Warn,
                    LogLevel::Error => CoreLogLevel::Error,
                };
                self.runtime.publish_event(Event::system_log(
                    level,
                    format!("plugin:{}", self.plugin_id),
                    message,
                ));
                Ok(PluginValue::Null)
            }
            PluginHostRequest::BusPublish { topic, value } => {
                if topic.starts_with("ui.")
                    || topic.starts_with("transport.")
                    || topic.starts_with("log.")
                {
                    return Err(PluginError::Host(format!(
                        "bus.publish: 保留主题前缀 {topic} 必须使用专用 API"
                    )));
                }
                self.runtime.publish_event(Event::new(
                    topic,
                    format!("plugin:{}", self.plugin_id),
                    Direction::Internal,
                    Payload::Json(
                        value
                            .to_json()
                            .map_err(|error| PluginError::InvalidValue(error.to_string()))?,
                    ),
                ));
                Ok(PluginValue::Null)
            }
            PluginHostRequest::BusHistory { topic, limit } => Ok(PluginValue::Array(
                self.runtime
                    .plugin_bus_history(&topic, limit)
                    .iter()
                    .map(event_to_plugin_value)
                    .collect(),
            )),
            PluginHostRequest::SerialDevices => Ok(self.serial_devices()),
            PluginHostRequest::SerialRequestDevice { task_id } => {
                let Some(task_id) = task_id else {
                    return Err(PluginError::UnsupportedCapability(
                        "serial.request_device must be triggered by a user gesture".to_owned(),
                    ));
                };
                let request_id = self.next_request_id();
                self.pending_requests.borrow_mut().push(
                    PluginHostPendingRequest::SerialRequestDevice {
                        request_id: request_id.clone(),
                        task_id,
                    },
                );
                Ok(PluginValue::Object(
                    [
                        ("pending".to_owned(), PluginValue::Bool(true)),
                        ("request_id".to_owned(), PluginValue::String(request_id)),
                    ]
                    .into_iter()
                    .collect(),
                ))
            }
            PluginHostRequest::SerialOpenPorts => Ok(PluginValue::Array(
                self.opened_ports
                    .borrow()
                    .keys()
                    .map(|port| PluginValue::String(port.to_string()))
                    .collect(),
            )),
            PluginHostRequest::SerialOpen { port, settings } => {
                self.dispatch_done(AppCommand::Connect {
                    port: port.clone(),
                    settings: SerialSettings::from(settings),
                })?;
                self.opened_ports.borrow_mut().insert(port, ());
                Ok(PluginValue::Null)
            }
            PluginHostRequest::SerialClose { port } => {
                self.dispatch_done(AppCommand::Disconnect { port: port.clone() })?;
                self.opened_ports.borrow_mut().remove(&port);
                Ok(PluginValue::Null)
            }
            PluginHostRequest::SerialSend { port, bytes } => {
                self.dispatch_done(AppCommand::SendRaw { port, bytes })?;
                Ok(PluginValue::Null)
            }
            PluginHostRequest::SerialStatus { port } => Ok(self.status(&port)),
            PluginHostRequest::Ui(command) => {
                self.publish_ui(command);
                Ok(PluginValue::Null)
            }
            PluginHostRequest::StorageGet { key } => Ok(self
                .data()
                .and_then(|data| data.storage.get(&key).cloned())
                .map_or(PluginValue::Null, |value| PluginValue::from_json(&value))),
            PluginHostRequest::StorageSet { key, value } => {
                self.data_mut().storage.insert(
                    key,
                    value
                        .to_json()
                        .map_err(|error| PluginError::InvalidValue(error.to_string()))?,
                );
                Ok(PluginValue::Null)
            }
            PluginHostRequest::StorageDelete { key } => {
                self.data_mut().storage.remove(&key);
                Ok(PluginValue::Null)
            }
            PluginHostRequest::StorageKeys => Ok(PluginValue::Array(
                self.data()
                    .map(|data| {
                        data.storage
                            .keys()
                            .cloned()
                            .map(PluginValue::String)
                            .collect()
                    })
                    .unwrap_or_default(),
            )),
            PluginHostRequest::ConfigGet { key, default } => Ok(self
                .data()
                .and_then(|data| data.settings.get(&key).cloned())
                .map_or(default, |value| PluginValue::from_json(&value))),
            PluginHostRequest::ConfigSet { key, value } => {
                self.data_mut().settings.insert(
                    key,
                    value
                        .to_json()
                        .map_err(|error| PluginError::InvalidValue(error.to_string()))?,
                );
                Ok(PluginValue::Null)
            }
            PluginHostRequest::ConfigRemove { key } => {
                self.data_mut().settings.remove(&key);
                Ok(PluginValue::Null)
            }
            PluginHostRequest::ConfigKeys => Ok(PluginValue::Array(
                self.data()
                    .map(|data| {
                        data.settings
                            .keys()
                            .cloned()
                            .map(PluginValue::String)
                            .collect()
                    })
                    .unwrap_or_default(),
            )),
            PluginHostRequest::ConfigProfileList => Ok(PluginValue::Array(
                self.data()
                    .map(|data| {
                        data.profiles
                            .keys()
                            .cloned()
                            .map(PluginValue::String)
                            .collect()
                    })
                    .unwrap_or_default(),
            )),
            PluginHostRequest::ConfigProfileLoad { name } => Ok(self
                .data()
                .and_then(|data| data.profiles.get(&name).cloned())
                .map_or(PluginValue::Null, |value| PluginValue::from_json(&value))),
            PluginHostRequest::ConfigProfileSave { name, value } => {
                self.data_mut().profiles.insert(
                    name,
                    value
                        .to_json()
                        .map_err(|error| PluginError::InvalidValue(error.to_string()))?,
                );
                Ok(PluginValue::Null)
            }
            PluginHostRequest::ConfigProfileDelete { name } => {
                self.data_mut().profiles.remove(&name);
                Ok(PluginValue::Null)
            }
            PluginHostRequest::FileOpenText {
                task_id,
                title,
                extensions,
            } => {
                let request_id = self.next_request_id();
                self.pending_requests
                    .borrow_mut()
                    .push(PluginHostPendingRequest::FileOpenText {
                        request_id: request_id.clone(),
                        task_id,
                        title,
                        extensions,
                    });
                Ok(PluginValue::Object(
                    [
                        ("pending".to_owned(), PluginValue::Bool(true)),
                        ("request_id".to_owned(), PluginValue::String(request_id)),
                    ]
                    .into_iter()
                    .collect(),
                ))
            }
            PluginHostRequest::FileReadText { file } => {
                Ok(PluginValue::String(self.file_text(&file)?))
            }
        }
    }
}

#[allow(dead_code)]
fn _opaque_file_handle_is_platform_neutral(_file: FileHandle) {}
