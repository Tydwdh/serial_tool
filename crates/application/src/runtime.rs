//! Platform-neutral application runtime boundary.

use crate::plugin::PluginView;
use crate::recording::RecordingStatusView;
use crate::replay::ReplayStatusView;
use crate::transport::TransportView;
use crate::{AppCapabilities, AppCommand, CommandOutcome, TaskSnapshot};

/// The runtime contract the shared UI reads through.
///
/// Writes go through [`dispatch`](Self::dispatch); reads go through the
/// `query_*` methods. Both composition roots implement this one interface so
/// the shared shell never binds a concrete runtime type.
pub trait AppRuntime {
    fn capabilities(&self) -> AppCapabilities;

    fn query_transport(&self) -> TransportView;
    fn query_recording(&self) -> RecordingStatusView;
    fn query_replay(&self) -> ReplayStatusView;
    fn query_plugins(&self) -> PluginView;
    fn task_snapshots(&self) -> Vec<TaskSnapshot>;

    fn tick(&mut self);

    fn dispatch(&mut self, command: AppCommand) -> Result<CommandOutcome, String>;
}
