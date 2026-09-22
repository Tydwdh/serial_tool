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
