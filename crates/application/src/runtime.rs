//! Platform-neutral application runtime boundary.

use crate::{AppCapabilities, AppCommand, CommandOutcome};

/// The only runtime contract the shared UI needs to know about.
pub trait AppRuntime {
    fn capabilities(&self) -> AppCapabilities;

    fn tick(&mut self);

    fn dispatch(&mut self, command: AppCommand) -> Result<CommandOutcome, String>;
}
