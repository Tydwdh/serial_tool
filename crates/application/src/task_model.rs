//! Shared task identity and lifecycle DTOs used by Native and Web.
//!
//! 这里放的是**两端都要有、且必须一字不差**的那份规则：`task.rs`（native only）与
//! `web.rs`（wasm only）互相看不见对方的代码，规则一旦各写一份，就再没有编译器能发现它们分叉。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaskId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Stable task-kind vocabulary shared by both composition roots.
///
/// The snapshot keeps a `String` so existing UI/serde consumers stay
/// compatible, but producers no longer spell the names by hand: native and
/// web pass the same variants, so `reconnect_serial` cannot drift into
/// `reconnect` on one side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    RefreshPorts,
    RequestPort,
    ConnectSerial,
    ConnectNetwork,
    ReconnectSerial,
    ReconnectNetwork,
    DisconnectSerial,
    DisconnectNetwork,
    CancelReconnect,
    SetDtr,
    SetRts,
    SendSerial,
    SendNetwork,
    ReloadPlugins,
    DiscoverPlugins,
    ExportTerminal,
    ExportLog,
    PluginLoad,
    MarketplaceRefresh,
    MarketplaceInstall,
    UpdateCheck,
    RemoveNetworkPort,
}

impl TaskKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RefreshPorts => "refresh_ports",
            Self::RequestPort => "request_port",
            Self::ConnectSerial => "connect_serial",
            Self::ConnectNetwork => "connect_network",
            Self::ReconnectSerial => "reconnect_serial",
            Self::ReconnectNetwork => "reconnect_network",
            Self::DisconnectSerial => "disconnect_serial",
            Self::DisconnectNetwork => "disconnect_network",
            Self::CancelReconnect => "cancel_reconnect",
            Self::SetDtr => "set_dtr",
            Self::SetRts => "set_rts",
            Self::SendSerial => "send_serial",
            Self::SendNetwork => "send_network",
            Self::ReloadPlugins => "reload_plugins",
            Self::DiscoverPlugins => "discover_plugins",
            Self::ExportTerminal => "export_terminal",
            Self::ExportLog => "export_log",
            Self::PluginLoad => "plugin_load",
            Self::MarketplaceRefresh => "marketplace_refresh",
            Self::MarketplaceInstall => "marketplace_install",
            Self::UpdateCheck => "update_check",
            Self::RemoveNetworkPort => "remove_network_port",
        }
    }

    pub fn from_name(value: &str) -> Option<Self> {
        Some(match value {
            "refresh_ports" => Self::RefreshPorts,
            "request_port" => Self::RequestPort,
            "connect_serial" => Self::ConnectSerial,
            "connect_network" => Self::ConnectNetwork,
            "reconnect_serial" => Self::ReconnectSerial,
            "reconnect_network" => Self::ReconnectNetwork,
            "disconnect_serial" => Self::DisconnectSerial,
            "disconnect_network" => Self::DisconnectNetwork,
            "cancel_reconnect" => Self::CancelReconnect,
            "set_dtr" => Self::SetDtr,
            "set_rts" => Self::SetRts,
            "send_serial" => Self::SendSerial,
            "send_network" => Self::SendNetwork,
            "reload_plugins" => Self::ReloadPlugins,
            "discover_plugins" => Self::DiscoverPlugins,
            "export_terminal" => Self::ExportTerminal,
            "export_log" => Self::ExportLog,
            "plugin_load" => Self::PluginLoad,
            "marketplace_refresh" => Self::MarketplaceRefresh,
            "marketplace_install" => Self::MarketplaceInstall,
            "update_check" => Self::UpdateCheck,
            "remove_network_port" => Self::RemoveNetworkPort,
            _ => return None,
        })
    }
}

impl std::fmt::Display for TaskKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<TaskKind> for String {
    fn from(kind: TaskKind) -> Self {
        kind.as_str().to_owned()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: TaskId,
    pub kind: String,
    pub state: TaskState,
    pub message: String,
}

/// 快照数超过保留上限时，按 id 升序淘汰已结束（Completed/Failed/Cancelled）的任务。
///
/// 只回收终态快照：仍在 Pending/Running 的任务必须一直可见，否则 UI 的
/// 「按 kind 查活跃任务」会把它看成已经结束。
pub(crate) fn prune_finished_snapshots(snapshots: &mut BTreeMap<TaskId, TaskSnapshot>) {
    const MAX_RETAINED_SNAPSHOTS: usize = 256;
    while snapshots.len() > MAX_RETAINED_SNAPSHOTS {
        let Some(id) = snapshots.iter().find_map(|(id, snapshot)| {
            matches!(
                snapshot.state,
                TaskState::Completed | TaskState::Failed | TaskState::Cancelled
            )
            .then_some(*id)
        }) else {
            break;
        };
        snapshots.remove(&id);
    }
}

/// 从「端口 -> 任务」表里摘下来的 id 统一走这里：有就取消，没有就什么都不做。
///
/// 为什么这道保护必须有：登记重连/连接任务就是往表里 `insert(port, task_id)` —— 不先取消
/// 表上原有的那一个，它的 id 会被**就地覆盖**，此后没有任何地方还能点名取消它：终态快照上
/// 的清理按 id 比对（`retain(|_, id| *id != snapshot.id)`），认不出那个已经被盖掉的值；
/// 只有 `Connected` / `Disconnected` 会按端口 `remove`。于是同一端口连点两次连接，
/// 前一个循环就永久失主。
///
/// 为什么只收这一步：native 的容器是 `HashMap<String, TaskId>` + `TaskManager::cancel`，
/// web 的是 `Rc<RefCell<BTreeMap<PortId, TaskId>>>` + `WebApplication::cancel_task`，
/// 两边连键类型都不同；把 remove/insert 也收进来就要泛型或 trait。所以「摘出来」与「存回去」
/// 留在各自调用点（那里同时管规范名），**决定要不要动手、动谁**只有这一份。
/// web 的四份连接/重连收尾经 `WebApplication::register_reconnect_task` 调它，
/// native 的 `start_reconnect` / `cancel_reconnect` 直接调它，
/// 由 `crates/application/tests/headless.rs::reconnecting_the_same_port_twice_cancels_the_first_loop`
/// 驱动。
pub(crate) fn cancel_deregistered_task(registered: Option<TaskId>, cancel: impl FnOnce(TaskId)) {
    if let Some(registered) = registered {
        cancel(registered);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(id: u64, state: TaskState) -> TaskSnapshot {
        TaskSnapshot {
            id: TaskId(id),
            kind: "t".to_owned(),
            state,
            message: String::new(),
        }
    }

    /// 上面那段文档说了两件事，此前**没有任何用例钉过**：
    /// 未结束的快照一条都不能回收（哪怕早就越过 256 的上限），以及回收要正好停在上限。
    #[test]
    fn retention_never_touches_live_snapshots_and_stops_exactly_at_the_bound() {
        const BOUND: usize = 256;

        let mut live = BTreeMap::new();
        for id in 0..(BOUND as u64 + 1) {
            live.insert(TaskId(id), snapshot(id, TaskState::Running));
        }
        prune_finished_snapshots(&mut live);
        assert_eq!(
            live.len(),
            BOUND + 1,
            "全是未结束快照时，超过上限也不得回收 —— 否则 UI 的「按 kind 查活跃任务」会查空"
        );

        let mut mixed = live;
        for id in 0..3 {
            mixed.insert(TaskId(id), snapshot(id, TaskState::Failed));
        }
        assert_eq!(mixed.len(), BOUND + 1);
        prune_finished_snapshots(&mut mixed);
        assert_eq!(
            mixed.len(),
            BOUND,
            "回收必须停在上限，不能把所有终态一次清光"
        );
        assert!(
            !mixed.contains_key(&TaskId(0))
                && mixed.contains_key(&TaskId(1))
                && mixed.contains_key(&TaskId(2)),
            "必须按 id 升序掐最早的那一条终态快照"
        );
    }
}
