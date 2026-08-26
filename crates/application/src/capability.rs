//! Runtime capabilities exposed to the presentation layer.
//!
//! Capabilities describe what a composition root can do. They are independent
//! from the target platform so panels do not need platform-specific branches.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Serial,
    RequestPort,
    FileSystem,
    Recorder,
    Replay,
    Plugins,
    Marketplace,
    Updater,
    NetworkSerial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppCapabilities {
    pub serial: bool,
    pub request_port: bool,
    pub filesystem: bool,
    pub recorder: bool,
    pub replay: bool,
    pub plugins: bool,
    pub marketplace: bool,
    pub updater: bool,
    pub network_serial: bool,
}

impl AppCapabilities {
    pub const fn native() -> Self {
        Self {
            serial: true,
            request_port: false,
            filesystem: true,
            recorder: true,
            replay: true,
            plugins: true,
            marketplace: true,
            updater: true,
            network_serial: true,
        }
    }

    pub const fn web() -> Self {
        Self {
            serial: true,
            request_port: true,
            filesystem: false,
            // Browser recording is implemented as an in-memory lossless
            // subscription followed by a Blob download. It does not need a
            // native filesystem, but it still has an explicit backpressure
            // stop condition in the Web composition root.
            recorder: true,
            // Web replay reads JSONL through the browser file picker and
            // publishes the same Event stream as Native ReplayManager.
            replay: true,
            // Browser plugins use the explicit Web module ABI. Native Lua
            // plugins are intentionally not silently treated as compatible.
            plugins: true,
            // The browser equivalent is a remote registry and a release-page
            // check; it intentionally does not replace the native binary in
            // place, but the user-facing capabilities are available.
            marketplace: true,
            updater: true,
            network_serial: true,
        }
    }

    pub const fn supports(self, capability: Capability) -> bool {
        match capability {
            Capability::Serial => self.serial,
            Capability::RequestPort => self.request_port,
            Capability::FileSystem => self.filesystem,
            Capability::Recorder => self.recorder,
            Capability::Replay => self.replay,
            Capability::Plugins => self.plugins,
            Capability::Marketplace => self.marketplace,
            Capability::Updater => self.updater,
            Capability::NetworkSerial => self.network_serial,
        }
    }
}
