pub mod service;
mod task_model;

pub mod marketplace;
pub mod plugin;
pub mod recording;
pub mod replay;
pub mod transport;
pub mod updater;

pub use task_model::{TaskId, TaskKind, TaskSnapshot, TaskState};

pub mod command;
pub mod error;
pub mod event;
pub mod perf;
pub mod query;
/// 发送路由的唯一决策点：native `dispatch` 调它，避免同一命令算出不同任务种类或不同字节。
pub mod send_plan;
pub mod task;
pub mod workbench;

pub use command::{AppCommand, CommandOutcome};
pub use error::AppError;
pub use task::{AppEvent, TaskManager, TaskResult};
pub use transport::{PortTypeView, PortView, SerialConfigView, TransportView};
pub use workbench::{
    ApplicationConfig, EventSink, TransportEndpoint, UiEventSubscriptions, Workbench,
};
