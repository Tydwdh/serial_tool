use std::rc::Rc;

use crate::{
    FileHandle, PluginError, PluginResult, PluginSerialDevice, PluginSerialSettings, PluginValue,
};
use tool_platform::PortId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Typed host request shared by every Lua engine.
///
/// A Web host may turn a request into an application task while Native may
/// complete it immediately. The VM adapter only knows this protocol; it never
/// needs to know whether the backend is a thread, a browser Promise, or a
/// DataBus operation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PluginHostRequest {
    NowMs,
    Log {
        level: LogLevel,
        message: String,
    },
    BusPublish {
        topic: String,
        value: PluginValue,
    },
    BusHistory {
        topic: String,
        limit: usize,
    },
    SerialDevices,
    /// Request a newly authorized browser device. Native callers leave
    /// `task_id` empty and may complete synchronously; Web callers provide
    /// the owning Lua task so the browser Promise can resume it later.
    SerialRequestDevice {
        task_id: Option<String>,
    },
    SerialOpenPorts,
    SerialOpen {
        port: PortId,
        settings: PluginSerialSettings,
    },
    SerialClose {
        port: PortId,
    },
    SerialSend {
        port: PortId,
        bytes: Vec<u8>,
    },
    SerialStatus {
        port: PortId,
    },
    Ui(PluginUiCommand),
    StorageGet {
        key: String,
    },
    StorageSet {
        key: String,
        value: PluginValue,
    },
    StorageDelete {
        key: String,
    },
    StorageKeys,
    ConfigGet {
        key: String,
        default: PluginValue,
    },
    ConfigSet {
        key: String,
        value: PluginValue,
    },
    ConfigRemove {
        key: String,
    },
    ConfigKeys,
    ConfigProfileList,
    ConfigProfileLoad {
        name: String,
    },
    ConfigProfileSave {
        name: String,
        value: PluginValue,
    },
    ConfigProfileDelete {
        name: String,
    },
    FileOpenText {
        task_id: String,
        title: String,
        extensions: Vec<String>,
    },
    FileReadText {
        file: FileHandle,
    },
}

/// An operation that must be started by the platform composition root, such
/// as a browser file picker.  The Lua engine only sees the request id and
/// yields the owning task until a completion is delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginHostPendingRequest {
    SerialRequestDevice {
        request_id: String,
        task_id: String,
    },
    FileOpenText {
        request_id: String,
        task_id: String,
        title: String,
        extensions: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginHostCompletion {
    pub request_id: String,
    pub result: Result<PluginValue, String>,
}

/// Declarative UI intent. The host turns this into the existing `ui.*` event
/// model; a Lua VM never receives an egui object.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PluginUiCommand {
    pub command: String,
    pub payload: PluginValue,
}

/// Host-side implementation of the stable `ctx.*` protocol.
///
/// Methods have conservative unsupported defaults so a platform can expose a
/// capability incrementally. Operations that cannot complete synchronously
/// (browser permission prompts and file pickers) return a pending host request;
/// the owning Lua task is resumed through the completion queue.
pub trait PluginHostApi {
    fn request(&self, _request: PluginHostRequest) -> PluginResult<PluginValue> {
        Err(PluginError::UnsupportedCapability("host".to_owned()))
    }

    fn take_pending_requests(&self) -> Vec<PluginHostPendingRequest> {
        Vec::new()
    }

    fn complete_request(&self, _completion: PluginHostCompletion) -> PluginResult<()> {
        Err(PluginError::UnsupportedCapability(
            "async host request".to_owned(),
        ))
    }

    fn take_completions(&self) -> Vec<PluginHostCompletion> {
        Vec::new()
    }

    fn now_ms(&self) -> PluginResult<u64> {
        match self.request(PluginHostRequest::NowMs)? {
            PluginValue::Integer(value) if value >= 0 => Ok(value as u64),
            PluginValue::Number(value) if value.is_finite() && value >= 0.0 => Ok(value as u64),
            value => Err(PluginError::InvalidValue(format!(
                "host now_ms returned {value:?}"
            ))),
        }
    }

    fn log(&self, level: LogLevel, message: &str) -> PluginResult<()> {
        self.request(PluginHostRequest::Log {
            level,
            message: message.to_owned(),
        })
        .map(|_| ())
    }

    fn bus_publish(&self, topic: &str, value: PluginValue) -> PluginResult<()> {
        self.request(PluginHostRequest::BusPublish {
            topic: topic.to_owned(),
            value,
        })
        .map(|_| ())
    }

    fn serial_devices(&self) -> PluginResult<Vec<PluginSerialDevice>> {
        let value = self.request(PluginHostRequest::SerialDevices)?;
        let json = value.to_json()?;
        serde_json::from_value(json)
            .map_err(|error| PluginError::InvalidValue(format!("serial devices: {error}")))
    }

    fn serial_request_device(&self) -> PluginResult<PluginSerialDevice> {
        let value = self.request(PluginHostRequest::SerialRequestDevice { task_id: None })?;
        serde_json::from_value(value.to_json()?)
            .map_err(|error| PluginError::InvalidValue(format!("serial device: {error}")))
    }

    fn serial_open(&self, port: &PortId, settings: PluginSerialSettings) -> PluginResult<()> {
        self.request(PluginHostRequest::SerialOpen {
            port: port.clone(),
            settings,
        })
        .map(|_| ())
    }

    fn serial_close(&self, port: &PortId) -> PluginResult<()> {
        self.request(PluginHostRequest::SerialClose { port: port.clone() })
            .map(|_| ())
    }

    fn serial_send(&self, port: &PortId, bytes: &[u8]) -> PluginResult<()> {
        self.request(PluginHostRequest::SerialSend {
            port: port.clone(),
            bytes: bytes.to_vec(),
        })
        .map(|_| ())
    }

    fn serial_status(&self, port: &PortId) -> PluginResult<PluginValue> {
        self.request(PluginHostRequest::SerialStatus { port: port.clone() })
    }

    fn ui_command(&self, command: PluginUiCommand) -> PluginResult<()> {
        self.request(PluginHostRequest::Ui(command)).map(|_| ())
    }

    fn storage_get(&self, key: &str) -> PluginResult<PluginValue> {
        self.request(PluginHostRequest::StorageGet {
            key: key.to_owned(),
        })
    }

    fn storage_set(&self, key: &str, value: PluginValue) -> PluginResult<()> {
        self.request(PluginHostRequest::StorageSet {
            key: key.to_owned(),
            value,
        })
        .map(|_| ())
    }

    fn storage_delete(&self, key: &str) -> PluginResult<()> {
        self.request(PluginHostRequest::StorageDelete {
            key: key.to_owned(),
        })
        .map(|_| ())
    }

    fn config_get(&self, key: &str, default: PluginValue) -> PluginResult<PluginValue> {
        self.request(PluginHostRequest::ConfigGet {
            key: key.to_owned(),
            default,
        })
    }

    fn config_set(&self, key: &str, value: PluginValue) -> PluginResult<()> {
        self.request(PluginHostRequest::ConfigSet {
            key: key.to_owned(),
            value,
        })
        .map(|_| ())
    }

    fn config_remove(&self, key: &str) -> PluginResult<()> {
        self.request(PluginHostRequest::ConfigRemove {
            key: key.to_owned(),
        })
        .map(|_| ())
    }

    fn config_keys(&self) -> PluginResult<PluginValue> {
        self.request(PluginHostRequest::ConfigKeys)
    }

    fn read_text(&self, file: &FileHandle) -> PluginResult<String> {
        match self.request(PluginHostRequest::FileReadText { file: file.clone() })? {
            PluginValue::String(value) => Ok(value),
            value => Err(PluginError::InvalidValue(format!(
                "file read returned {value:?}"
            ))),
        }
    }
}

pub type SharedPluginHost = Rc<dyn PluginHostApi>;
