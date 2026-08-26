//! Application-owned transport view data shared by Native and Web.
//!
//! The composition roots may keep ephemeral widget state such as the current
//! text-edit buffer, but the device list and transport lifecycle belong to
//! Application. Keeping this DTO platform-neutral also prevents the Web UI
//! from growing a second, subtly different serial state machine.

use tool_platform::{PortDescriptor, PortId, SerialSettings, TransportCapabilities};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportView {
    base_capabilities: TransportCapabilities,
    pub capabilities: TransportCapabilities,
    pub ports: Vec<PortDescriptor>,
    pub connected: Option<PortId>,
    pub connecting: bool,
    pub settings: SerialSettings,
    pub status: String,
}

impl TransportView {
    pub fn new(capabilities: TransportCapabilities) -> Self {
        Self {
            base_capabilities: capabilities,
            capabilities,
            ports: Vec::new(),
            connected: None,
            connecting: false,
            settings: SerialSettings::default(),
            status: String::new(),
        }
    }

    pub fn set_connected(&mut self, port: Option<PortId>) {
        self.connected = port;
        self.connecting = false;
        self.capabilities = if self
            .connected
            .as_ref()
            .is_some_and(|port| port.as_str().starts_with("network://"))
        {
            TransportCapabilities::WEB_NETWORK
        } else {
            self.base_capabilities
        };
    }

    pub fn upsert_port(&mut self, port: PortDescriptor) {
        self.ports.retain(|item| item.id != port.id);
        self.ports.push(port);
    }

    pub fn remove_port(&mut self, port: &PortId) {
        self.ports.retain(|item| &item.id != port);
        if self.connected.as_ref() == Some(port) {
            self.connected = None;
            self.connecting = false;
            self.capabilities = self.base_capabilities;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_lifecycle_clears_connecting_state() {
        let mut view = TransportView::new(TransportCapabilities::WEB_SERIAL);
        assert!(!view.connecting);

        view.connecting = true;
        let port = PortId::new("browser-port");
        view.set_connected(Some(port.clone()));
        assert_eq!(view.connected, Some(port.clone()));
        assert!(!view.connecting);

        view.connecting = true;
        view.remove_port(&port);
        assert_eq!(view.connected, None);
        assert!(!view.connecting);
    }
}
