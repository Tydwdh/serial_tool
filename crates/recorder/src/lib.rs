//! tool-recorder：事件录制与回放。
//!
//! 三个子模块：
//! - [`format`]：录制文件格式与过滤策略（纯函数，可独立测试）。
//! - [`recorder`]：`JsonlRecorder`，订阅 DataBus 异步写入 jsonl。
//! - [`replay`]：`ReplayManager`，加载录制文件并控制回放（seek/step/play、analyzer cache、书签）。
//!
//! 录制与回放零耦合，可独立演进。

pub mod format;
pub mod recorder;
pub mod replay;

pub use format::RecordMode;
pub use recorder::{JsonlRecorder, RecorderStats};
pub use replay::{
    ReplayBlockReason, ReplayLoadData, ReplayLoadReport, ReplayManager, ReplayPolicy, ReplayState,
    ReplayStatus,
};
