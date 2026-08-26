//! Platform-neutral plugin protocol.
//!
//! This crate deliberately has no dependency on `mlua`, `egui`, or a native
//! filesystem.  Native and Web Lua engines adapt their values and host calls
//! to these types, so the plugin manifest and `ctx.*` contract do not change
//! when the underlying Lua VM changes.

mod capability;
mod engine;
mod error;
mod file;
mod host;
mod serial;
mod value;

pub use capability::{PluginCapability, PluginPermissions};
pub use engine::{
    CoroutineId, LuaEngine, PluginCallResult, PluginFunctionId, PluginInstanceId, PluginLoadConfig,
    PluginYield,
};
pub use error::{PluginError, PluginResult};
pub use file::FileHandle;
pub use host::{
    LogLevel, PluginHostApi, PluginHostCompletion, PluginHostPendingRequest, PluginHostRequest,
    PluginUiCommand, SharedPluginHost,
};
pub use serial::{PluginParity, PluginSerialDevice, PluginSerialSettings};
pub use value::PluginValue;
