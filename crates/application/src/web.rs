//! Browser Application composition: AppCommand → TaskId → Promise worker → AppEvent.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::rc::Rc;

use tool_databus::DataBus;
use tool_platform::web_serial::WebSerialTransport;
use tool_platform::{
    PortDescriptor, PortId, TransportBackend, TransportFuture, serial_rx_event, serial_tx_event,
};
use wasm_bindgen_futures::spawn_local;

use crate::command::{AppCommand, CommandOutcome};
use crate::task_model::{TaskId, TaskSnapshot, TaskState};

pub type RepaintWaker = Rc<dyn Fn()>;

#[derive(Debug, Clone)]
pub enum WebAppEvent {
    TaskStateChanged(TaskSnapshot),
    PortsRefreshed(Vec<PortDescriptor>),
    PortRequested(PortDescriptor),
    Connected {
        port: PortId,
    },
    Disconnected {
        port: PortId,
    },
    Sent {
        port: PortId,
        bytes: usize,
    },
    SignalsChanged {
        port: PortId,
        signal: SignalKind,
        value: bool,
    },
    TaskFailed {
        id: TaskId,
        error: String,
    },
    TaskCancelled {
        id: TaskId,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum SignalKind {
    Dtr,
    Rts,
}

struct WebTaskRegistry {
    next_id: u64,
    snapshots: BTreeMap<TaskId, TaskSnapshot>,
    cancelled: BTreeSet<TaskId>,
    events: Vec<WebAppEvent>,
}

impl Default for WebTaskRegistry {
    fn default() -> Self {
        Self {
            next_id: 1,
            snapshots: BTreeMap::new(),
            cancelled: BTreeSet::new(),
            events: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct WebApplication {
    bus: DataBus,
    transport: WebSerialTransport,
    tasks: Rc<RefCell<WebTaskRegistry>>,
    repaint_waker: Rc<RefCell<Option<RepaintWaker>>>,
}

impl WebApplication {
    pub fn new(bus: DataBus) -> Result<Self, String> {
        Ok(Self {
            bus,
            transport: WebSerialTransport::from_window().map_err(|error| error.to_string())?,
            tasks: Rc::new(RefCell::new(WebTaskRegistry::default())),
            repaint_waker: Rc::new(RefCell::new(None)),
        })
    }

    pub fn set_repaint_waker(&self, waker: RepaintWaker) {
        *self.repaint_waker.borrow_mut() = Some(waker);
    }

    fn wake(&self) {
        wake_handle(&self.repaint_waker);
    }

    pub fn bus(&self) -> DataBus {
        self.bus.clone()
    }

    pub fn task_snapshots(&self) -> Vec<TaskSnapshot> {
        self.tasks.borrow().snapshots.values().cloned().collect()
    }

    pub fn drain_events(&self) -> Vec<WebAppEvent> {
        std::mem::take(&mut self.tasks.borrow_mut().events)
    }

    pub fn cancel_task(&self, id: TaskId) -> bool {
        let mut tasks = self.tasks.borrow_mut();
        if !tasks.snapshots.contains_key(&id) {
            return false;
        }
        tasks.cancelled.insert(id);
        tasks.events.push(WebAppEvent::TaskCancelled { id });
        if let Some(snapshot) = tasks.snapshots.get_mut(&id) {
            snapshot.state = TaskState::Cancelled;
            snapshot.message = "任务已取消".to_owned();
        }
        drop(tasks);
        self.wake();
        true
    }

    fn spawn<T, F>(
        &self,
        kind: &str,
        future: F,
        complete: impl FnOnce(T) -> WebAppEvent + 'static,
    ) -> TaskId
    where
        T: 'static,
        F: Future<Output = tool_platform::TransportResult<T>> + 'static,
    {
        let future: TransportFuture<T> = Box::pin(future);
        let (id, tasks) = {
            let mut tasks = self.tasks.borrow_mut();
            let id = TaskId(tasks.next_id);
            tasks.next_id += 1;
            let snapshot = TaskSnapshot {
                id,
                kind: kind.to_owned(),
                state: TaskState::Pending,
                message: "等待异步操作启动".to_owned(),
            };
            tasks.snapshots.insert(id, snapshot.clone());
            tasks.events.push(WebAppEvent::TaskStateChanged(snapshot));
            (id, self.tasks.clone())
        };
        self.wake();

        {
            let mut tasks = tasks.borrow_mut();
            if let Some(snapshot) = tasks.snapshots.get_mut(&id) {
                snapshot.state = TaskState::Running;
                snapshot.message = "异步操作运行中".to_owned();
                let snapshot = snapshot.clone();
                tasks.events.push(WebAppEvent::TaskStateChanged(snapshot));
            }
        }
        self.wake();

        let repaint_waker = self.repaint_waker.clone();
        spawn_local(async move {
            let result = future.await;
            let should_wake = {
                let mut tasks = tasks.borrow_mut();
                if tasks.cancelled.remove(&id) {
                    false
                } else {
                    match result {
                        Ok(value) => {
                            if let Some(snapshot) = tasks.snapshots.get_mut(&id) {
                                snapshot.state = TaskState::Completed;
                                snapshot.message = "异步操作完成".to_owned();
                            }
                            tasks.events.push(complete(value));
                        }
                        Err(error) => {
                            if let Some(snapshot) = tasks.snapshots.get_mut(&id) {
                                snapshot.state = TaskState::Failed;
                                snapshot.message = error.to_string();
                            }
                            tasks.events.push(WebAppEvent::TaskFailed {
                                id,
                                error: error.to_string(),
                            });
                        }
                    }
                    true
                }
            };
            if should_wake {
                wake_handle(&repaint_waker);
            }
        });
        id
    }

    pub fn dispatch(&self, command: AppCommand) -> Result<CommandOutcome, String> {
        match command {
            AppCommand::RefreshPorts => {
                let transport = self.transport.clone();
                let task_id = self.spawn(
                    "refresh_ports",
                    async move { transport.list_known_ports().await },
                    WebAppEvent::PortsRefreshed,
                );
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "正在刷新已授权串口".to_owned(),
                })
            }
            AppCommand::RequestPort => {
                let transport = self.transport.clone();
                // requestPort() must be created while the browser still has
                // the button's transient user activation.  The transport
                // deliberately constructs the Promise in request_port(),
                // before returning the future to this scheduler.
                let future = transport.request_port();
                let task_id = self.spawn("request_port", future, WebAppEvent::PortRequested);
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "等待浏览器选择串口".to_owned(),
                })
            }
            AppCommand::Connect {
                port_name,
                settings,
            } => {
                let transport = self.transport.clone();
                let port = PortId::new(port_name);
                let task_port = port.clone();
                let bus = self.bus.clone();
                let task_events = self.tasks.clone();
                let repaint_waker = self.repaint_waker.clone();
                let task_id = self.spawn(
                    "connect_serial",
                    async move {
                        transport.connect(port.clone(), settings).await?;
                        let rx_port = port.clone();
                        let rx_bus = bus.clone();
                        let rx_waker = repaint_waker.clone();
                        let sink = Rc::new(move |bytes: Vec<u8>| {
                            rx_bus.publish(serial_rx_event(&rx_port, bytes));
                            wake_handle(&rx_waker);
                        });
                        let disconnect_waker = repaint_waker.clone();
                        let on_disconnect = Rc::new(move |port| {
                            task_events
                                .borrow_mut()
                                .events
                                .push(WebAppEvent::Disconnected { port });
                            wake_handle(&disconnect_waker);
                        });
                        transport
                            .start_receive_with_disconnect(port.clone(), sink, on_disconnect)
                            .map_err(|error| {
                                tool_platform::TransportError::Operation(error.to_string())
                            })?;
                        Ok(())
                    },
                    move |_| WebAppEvent::Connected { port: task_port },
                );
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "正在打开串口".to_owned(),
                })
            }
            AppCommand::Disconnect { port_name } => {
                let transport = self.transport.clone();
                let port = PortId::new(port_name);
                let task_port = port.clone();
                let task_id = self.spawn(
                    "disconnect_serial",
                    async move { transport.disconnect(port).await },
                    move |_| WebAppEvent::Disconnected { port: task_port },
                );
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "正在关闭串口".to_owned(),
                })
            }
            AppCommand::SendText { port_name, text } => {
                self.send(PortId::new(port_name), text.into_bytes())
            }
            AppCommand::SendHex { port_name, hex } => {
                self.send(PortId::new(port_name), parse_hex(&hex)?)
            }
            AppCommand::SendRaw { port_name, bytes } => self.send(PortId::new(port_name), bytes),
            AppCommand::SetDtr { port_name, value } => {
                let transport = self.transport.clone();
                let port = PortId::new(port_name);
                let task_port = port.clone();
                let task_id = self.spawn(
                    "set_dtr",
                    async move { transport.set_dtr(port, value).await },
                    move |_| WebAppEvent::SignalsChanged {
                        port: task_port,
                        signal: SignalKind::Dtr,
                        value,
                    },
                );
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "正在设置 DTR".to_owned(),
                })
            }
            AppCommand::SetRts { port_name, value } => {
                let transport = self.transport.clone();
                let port = PortId::new(port_name);
                let task_port = port.clone();
                let task_id = self.spawn(
                    "set_rts",
                    async move { transport.set_rts(port, value).await },
                    move |_| WebAppEvent::SignalsChanged {
                        port: task_port,
                        signal: SignalKind::Rts,
                        value,
                    },
                );
                Ok(CommandOutcome::Pending {
                    task_id,
                    message: "正在设置 RTS".to_owned(),
                })
            }
            _ => Err("该命令尚未在 Web V1 开放".to_owned()),
        }
    }

    fn send(&self, port: PortId, bytes: Vec<u8>) -> Result<CommandOutcome, String> {
        let transport = self.transport.clone();
        let task_port = port.clone();
        let bus = self.bus.clone();
        let byte_count = bytes.len();
        let task_id = self.spawn(
            "send_serial",
            async move {
                transport.send(port.clone(), bytes.clone()).await?;
                bus.publish(serial_tx_event(&port, bytes));
                Ok(())
            },
            move |_| WebAppEvent::Sent {
                port: task_port,
                bytes: byte_count,
            },
        );
        Ok(CommandOutcome::Pending {
            task_id,
            message: "正在发送串口数据".to_owned(),
        })
    }
}

fn parse_hex(input: &str) -> Result<Vec<u8>, String> {
    let compact = input.split_whitespace().collect::<String>();
    if compact.len() % 2 != 0 {
        return Err("HEX 数据长度必须为偶数".to_owned());
    }
    (0..compact.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&compact[index..index + 2], 16)
                .map_err(|_| format!("无效 HEX：{}", &compact[index..index + 2]))
        })
        .collect()
}

fn wake_handle(handle: &Rc<RefCell<Option<RepaintWaker>>>) {
    let waker = handle.borrow().clone();
    if let Some(waker) = waker {
        waker();
    }
}
