pub mod service;
mod task_model;

#[cfg(target_arch = "wasm32")]
pub mod web;

pub use task_model::{TaskId, TaskSnapshot, TaskState};

pub mod command;
#[cfg(not(target_arch = "wasm32"))]
pub mod error;
#[cfg(not(target_arch = "wasm32"))]
pub mod event;
#[cfg(not(target_arch = "wasm32"))]
pub mod model;
#[cfg(not(target_arch = "wasm32"))]
pub mod perf;
#[cfg(not(target_arch = "wasm32"))]
pub mod query;
#[cfg(not(target_arch = "wasm32"))]
pub mod task;
#[cfg(not(target_arch = "wasm32"))]
pub mod workbench;

pub use command::{AppCommand, CommandOutcome};
#[cfg(not(target_arch = "wasm32"))]
pub use error::AppError;
#[cfg(not(target_arch = "wasm32"))]
pub use task::{AppEvent, TaskManager, TaskResult};
#[cfg(not(target_arch = "wasm32"))]
pub use workbench::{
    ApplicationConfig, EventSink, TransportEndpoint, UiEventSubscriptions, Workbench,
};

// Presentation compatibility surface.
//
// These are deliberately explicit facades: the application crate controls
// which dependency types cross its public API.
// New UI code should prefer `AppCommand`, `Workbench::query_*` and the DTOs in
// `query`; these modules remain for the legacy panel adapters that still need
// serialization/event vocabulary while they are migrated.
pub mod api {
    pub mod core {
        pub use ::tool_core::config;
        pub use ::tool_core::topics;
        pub use ::tool_core::{Direction, Event, LogLevel, Payload, now_timestamp_ms};
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub mod databus {
        pub use ::tool_databus::{
            DataBus, DataBusPerfSnapshot, RingSubscription, Subscription, SubscriptionBacklog,
            TopicFilter,
        };
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub mod extension {
        pub use ::tool_extension::{
            PluginDiagnostic, PluginDiagnosticSeverity, PluginState, PluginSummary,
        };
        pub mod manifest {
            pub use ::tool_extension::manifest::{
                PluginCommand, PluginContributes, PluginSetting, ReplayAnalyzerEntry,
            };
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub mod lua_host {
        pub use ::tool_lua_host::{
            ConfigStore, DialogRequest, FileAccessBroker, FileFilter, LuaReplayConfig,
            run_replay_analyzer_with_cancel,
        };
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub mod marketplace {
        pub use ::tool_marketplace::{
            DEFAULT_REGISTRY_URL, Registry, RegistryFetch, RegistryPlugin, fetch_registry,
            install_plugin, retire_old_plugin_dirs,
        };
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub mod recorder {
        pub use ::tool_recorder::{
            RecordMode, ReplayBlockReason, ReplayManager, ReplayPolicy, ReplayState, ReplayStatus,
        };
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub mod transport {
        pub use ::tool_transport::{
            NetworkSerialConfig, PortType, RepaintWaker, SerialPortDescriptor, TransportStatus,
            hex_preview, natural_sort_key, parse_hex, send_impl_to, translate_error,
        };
    }
}
