pub mod command;
pub mod error;
pub mod event;
pub mod model;
pub mod query;
pub mod service;
pub mod workbench;

pub use command::{AppCommand, CommandOutcome};
pub use error::AppError;
pub use workbench::{ApplicationConfig, Workbench};

// ── Business dependency aggregate re-export ──
// Panels (egui) access business types through tool-application only,
// never depending on tool-recorder/extension/marketplace/transport directly.
pub extern crate tool_core;
pub extern crate tool_databus;
pub extern crate tool_extension;
pub extern crate tool_lua_host;
pub extern crate tool_marketplace;
pub extern crate tool_recorder;
pub extern crate tool_transport;
