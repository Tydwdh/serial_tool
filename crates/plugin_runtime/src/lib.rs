//! VM adapters for the platform-neutral plugin protocol.

#[cfg(target_arch = "wasm32")]
mod web_lua;

#[cfg(target_arch = "wasm32")]
pub use web_lua::{WebLuaEngine, WebReplayOutput};
