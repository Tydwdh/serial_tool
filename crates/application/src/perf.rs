use tool_databus::DataBusPerfSnapshot;
use tool_recorder::RecorderStats;

/// Application-owned performance counters exposed as read-only DTOs.
#[derive(Debug, Clone, Default)]
pub struct ApplicationPerfSnapshot {
    pub databus: DataBusPerfSnapshot,
    pub recorder: RecorderStats,
}
