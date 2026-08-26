//! 统一后台任务模型。
//!
//! 所有可能执行文件 IO、串口 IO 或大量数据处理的应用操作，都通过这里
//! 产生 `TaskId`，由 worker 线程执行，再把 `AppEvent` 投递回 Workbench。

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use tool_extension::PluginScan;
use tool_platform::storage::FileHandle;
use tool_platform::{PortDescriptor, PortId};
use tool_recorder::ReplayLoadData;
use tool_transport::SerialPortDescriptor;

pub use crate::task_model::{TaskId, TaskSnapshot, TaskState};

/// worker 完成后交给 UI/Workbench 应用的结果。
#[derive(Debug)]
pub enum TaskResult {
    PortsRefreshed(Vec<SerialPortDescriptor>),
    PlatformPortsRefreshed(Vec<PortDescriptor>),
    Connected { port_name: String },
    Disconnected { port_name: String },
    Reconnected { port_name: String },
    TransportConnected { port: PortId },
    TransportDisconnected { port: PortId },
    TransportSent { port: PortId, bytes: usize },
    TransportSignalChanged { port: PortId },
    ReplayLoaded(ReplayLoadData),
    PluginsDiscovered(PluginScan),
    FileExported { file: FileHandle },
}

#[derive(Debug)]
pub enum AppEvent {
    TaskStateChanged { snapshot: TaskSnapshot },
    TaskCompleted { id: TaskId, result: TaskResult },
    TaskFailed { id: TaskId, error: String },
    TaskCancelled { id: TaskId },
}

/// worker 可轮询的取消令牌。
#[derive(Debug, Clone)]
pub struct TaskContext {
    id: TaskId,
    cancelled: Arc<AtomicBool>,
}

impl TaskContext {
    pub fn id(&self) -> TaskId {
        self.id
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn check_cancelled(&self) -> Result<(), TaskCancelled> {
        if self.is_cancelled() {
            Err(TaskCancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TaskCancelled;

struct TaskControl {
    cancelled: Arc<AtomicBool>,
}

/// Workbench 持有的任务调度器。它本身只在 UI 线程访问，worker 只持有 channel
/// 和取消令牌，因此不会把 Workbench 或任何 egui 状态跨线程借出。
pub struct TaskManager {
    next_id: AtomicU64,
    events_tx: Sender<AppEvent>,
    events_rx: Receiver<AppEvent>,
    snapshots: BTreeMap<TaskId, TaskSnapshot>,
    controls: BTreeMap<TaskId, TaskControl>,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskManager {
    pub fn new() -> Self {
        let (events_tx, events_rx) = unbounded();
        Self {
            next_id: AtomicU64::new(1),
            events_tx,
            events_rx,
            snapshots: BTreeMap::new(),
            controls: BTreeMap::new(),
        }
    }

    pub fn spawn<F>(&mut self, kind: impl Into<String>, work: F) -> TaskId
    where
        F: FnOnce(TaskContext) -> Result<TaskResult, String> + Send + 'static,
    {
        let id = TaskId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let kind = kind.into();
        let cancelled = Arc::new(AtomicBool::new(false));
        let context = TaskContext {
            id,
            cancelled: Arc::clone(&cancelled),
        };
        self.snapshots.insert(
            id,
            TaskSnapshot {
                id,
                kind: kind.clone(),
                state: TaskState::Pending,
                message: "等待后台任务启动".to_owned(),
            },
        );
        self.controls.insert(id, TaskControl { cancelled });

        let events = self.events_tx.clone();
        std::thread::Builder::new()
            .name(format!("app-task-{}", id.0))
            .spawn(move || {
                let _ = events.send(AppEvent::TaskStateChanged {
                    snapshot: TaskSnapshot {
                        id,
                        kind,
                        state: TaskState::Running,
                        message: "后台任务运行中".to_owned(),
                    },
                });

                let outcome = catch_unwind(AssertUnwindSafe(|| work(context.clone())));
                match outcome {
                    Ok(Ok(_result)) if context.is_cancelled() => {
                        let _ = events.send(AppEvent::TaskCancelled { id });
                    }
                    Ok(Ok(result)) => {
                        let _ = events.send(AppEvent::TaskCompleted { id, result });
                    }
                    Ok(Err(_error)) if context.is_cancelled() => {
                        let _ = events.send(AppEvent::TaskCancelled { id });
                    }
                    Ok(Err(error)) => {
                        let _ = events.send(AppEvent::TaskFailed { id, error });
                    }
                    Err(_) => {
                        let _ = events.send(AppEvent::TaskFailed {
                            id,
                            error: "后台任务线程异常退出".to_owned(),
                        });
                    }
                }
            })
            .expect("spawn application task worker");

        id
    }

    pub fn cancel(&mut self, id: TaskId) -> bool {
        let Some(control) = self.controls.get(&id) else {
            return false;
        };
        control.cancelled.store(true, Ordering::Relaxed);
        true
    }

    pub fn drain_events(&mut self) -> Vec<AppEvent> {
        let events = self.events_rx.try_iter().collect::<Vec<_>>();
        for event in &events {
            match event {
                AppEvent::TaskStateChanged { snapshot } => {
                    self.snapshots.insert(snapshot.id, snapshot.clone());
                }
                AppEvent::TaskCompleted { id, .. } => {
                    if let Some(snapshot) = self.snapshots.get_mut(id) {
                        snapshot.state = TaskState::Completed;
                        snapshot.message = "后台任务完成".to_owned();
                    }
                    self.controls.remove(id);
                }
                AppEvent::TaskFailed { id, error } => {
                    if let Some(snapshot) = self.snapshots.get_mut(id) {
                        snapshot.state = TaskState::Failed;
                        snapshot.message = error.clone();
                    }
                    self.controls.remove(id);
                }
                AppEvent::TaskCancelled { id } => {
                    if let Some(snapshot) = self.snapshots.get_mut(id) {
                        snapshot.state = TaskState::Cancelled;
                        snapshot.message = "任务已取消".to_owned();
                    }
                    self.controls.remove(id);
                }
            }
        }
        const MAX_RETAINED_SNAPSHOTS: usize = 256;
        while self.snapshots.len() > MAX_RETAINED_SNAPSHOTS {
            let Some(id) = self.snapshots.iter().find_map(|(id, snapshot)| {
                matches!(
                    snapshot.state,
                    TaskState::Completed | TaskState::Failed | TaskState::Cancelled
                )
                .then_some(*id)
            }) else {
                break;
            };
            self.snapshots.remove(&id);
        }
        events
    }

    pub fn snapshots(&self) -> Vec<TaskSnapshot> {
        self.snapshots.values().cloned().collect()
    }

    pub fn active_task_id(&self, kind: &str) -> Option<TaskId> {
        self.snapshots.values().find_map(|snapshot| {
            (snapshot.kind == kind
                && matches!(snapshot.state, TaskState::Pending | TaskState::Running))
            .then_some(snapshot.id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_transitions_to_completed() {
        let mut manager = TaskManager::new();
        let id = manager.spawn("test", |_context| {
            Ok(TaskResult::FileExported {
                file: FileHandle::named("test.txt"),
            })
        });

        for _ in 0..100 {
            manager.drain_events();
            if manager
                .snapshots()
                .iter()
                .any(|snapshot| snapshot.id == id && snapshot.state == TaskState::Completed)
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("task did not complete");
    }

    #[test]
    fn cancelled_task_reports_cancelled() {
        let mut manager = TaskManager::new();
        let id = manager.spawn("cancel", |context| {
            loop {
                context
                    .check_cancelled()
                    .map_err(|_| "cancelled".to_owned())?;
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        assert!(manager.cancel(id));

        for _ in 0..100 {
            manager.drain_events();
            if manager
                .snapshots()
                .iter()
                .any(|snapshot| snapshot.id == id && snapshot.state == TaskState::Cancelled)
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("task was not cancelled");
    }
}
