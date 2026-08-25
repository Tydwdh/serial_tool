pub mod command;
pub mod error;
pub mod event;
pub mod model;
pub mod perf;
pub mod query;
pub mod service;
pub mod task;
pub mod workbench;

pub use command::{AppCommand, CommandOutcome};
pub use error::AppError;
pub use task::{AppEvent, TaskId, TaskManager, TaskResult, TaskSnapshot, TaskState};
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

    pub mod databus {
        pub use ::tool_databus::{
            DataBus, DataBusPerfSnapshot, RingSubscription, Subscription, SubscriptionBacklog,
            TopicFilter,
        };
    }

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

    pub mod lua_host {
        pub use ::tool_lua_host::{
            ConfigStore, DialogRequest, FileAccessBroker, FileFilter, LuaReplayConfig,
            run_replay_analyzer_with_cancel,
        };
    }

    pub mod marketplace {
        pub use ::tool_marketplace::{
            DEFAULT_REGISTRY_URL, Registry, RegistryFetch, RegistryPlugin, fetch_registry,
            install_plugin, retire_old_plugin_dirs,
        };
    }

    pub mod recorder {
        pub use ::tool_recorder::{
            RecordMode, ReplayBlockReason, ReplayManager, ReplayPolicy, ReplayState, ReplayStatus,
        };
    }

    pub mod transport {
        pub use ::tool_transport::{
            NetworkSerialConfig, PortType, RepaintWaker, SerialPortDescriptor, TransportStatus,
            hex_preview, natural_sort_key, parse_hex, send_impl_to, translate_error,
        };
    }
}
