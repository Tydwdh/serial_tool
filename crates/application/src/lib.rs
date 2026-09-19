pub mod service;
mod task_model;

pub mod capability;
pub mod marketplace;
pub mod plugin;
pub mod recording;
pub mod replay;
pub mod runtime;
pub mod transport;
pub mod updater;

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
/// 发送路由的唯一决策点：无任何 `cfg` 门控，native 与 wasm 的两个 `dispatch`
/// 都调它，避免「同一命令在两端算出不同任务种类或不同字节」。
pub mod send_plan;
#[cfg(not(target_arch = "wasm32"))]
pub mod task;
#[cfg(not(target_arch = "wasm32"))]
pub mod workbench;

pub use capability::{AppCapabilities, Capability};
pub use command::{AppCommand, CommandOutcome};
#[cfg(not(target_arch = "wasm32"))]
pub use error::AppError;
pub use runtime::AppRuntime;
#[cfg(not(target_arch = "wasm32"))]
pub use task::{AppEvent, TaskManager, TaskResult};
pub use transport::TransportView;
#[cfg(not(target_arch = "wasm32"))]
pub use workbench::{
    ApplicationConfig, EventSink, TransportEndpoint, UiEventSubscriptions, Workbench,
};
#[cfg(not(target_arch = "wasm32"))]
pub type NativeRuntime = Workbench;
