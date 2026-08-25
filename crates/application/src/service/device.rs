//! Device/Transport 领域服务 — 供 Workbench 组合。

use tool_databus::DataBus;
use tool_transport::TransportManager;

pub struct DeviceService {
    bus: DataBus,
    transport: TransportManager,
}

impl DeviceService {
    pub fn new(bus: DataBus, transport: TransportManager) -> Self {
        Self { bus, transport }
    }
}
