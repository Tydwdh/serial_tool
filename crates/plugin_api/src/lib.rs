//! Native plugin protocol.
//!
//! This crate deliberately has no dependency on `mlua`, `egui`, or a native
//! filesystem. It carries the values, permissions, and `ctx.*` rules shared by
//! the Lua host and the extension manager.

mod capability;
pub mod ctx;
mod error;
mod value;

pub use capability::{PluginCapability, PluginPermissions};
pub use error::{PluginError, PluginResult};
pub use value::PluginValue;
