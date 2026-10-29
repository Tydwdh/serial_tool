use tool_databus::DataBusPerfSnapshot;

/// Application-owned performance counters exposed as read-only DTOs.
#[derive(Debug, Clone, Default)]
pub struct ApplicationPerfSnapshot {
    pub databus: DataBusPerfSnapshot,
}
