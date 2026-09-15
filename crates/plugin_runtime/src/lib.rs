//! VM adapters for the platform-neutral plugin protocol.
//!
//! Native runs the mlua engine in `tool-lua-host`; the browser runs the pure
//! Rust engine in [`web_lua`]. Both implement `tool_plugin_api::LuaEngine`, so
//! the browser engine also builds on the host target: `cargo test` exercises
//! the same VM the browser executes instead of leaving it to manual checks.

mod web_lua;

pub use web_lua::{WebLuaEngine, WebReplayOutput};
