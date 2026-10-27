//! Application-owned transport view data shared by Native and Web.
//!
//! The composition roots may keep ephemeral widget state such as the current
//! text-edit buffer, but the device list and transport lifecycle belong to
//! Application. Keeping this DTO platform-neutral also prevents the Web UI
//! from growing a second, subtly different serial state machine.

use tool_platform::{PortDescriptor, PortId, PortKind, SerialSettings, TransportCapabilities};

/// Native port detail retained for display (USB product name, Bluetooth, PCI).
///
/// Web descriptors do not carry this detail; they map to [`Self::Unknown`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortTypeView {
    Usb(String),
    Bluetooth,
    Pci,
    Network,
    Unknown,
}

impl PortTypeView {
    pub fn is_network(&self) -> bool {
        matches!(self, Self::Network)
    }
}

impl std::fmt::Display for PortTypeView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usb(product) if !product.is_empty() => write!(f, "{product}"),
            Self::Usb(_) => write!(f, "USB"),
            Self::Bluetooth => write!(f, "Bluetooth"),
            Self::Pci => write!(f, "PCI"),
            Self::Network => write!(f, "网络"),
            Self::Unknown => Ok(()),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<tool_transport::PortType> for PortTypeView {
    fn from(port_type: tool_transport::PortType) -> Self {
        match port_type {
            tool_transport::PortType::Usb(product) => Self::Usb(product),
            tool_transport::PortType::Bluetooth => Self::Bluetooth,
            tool_transport::PortType::Pci => Self::Pci,
            tool_transport::PortType::Network => Self::Network,
            tool_transport::PortType::Unknown => Self::Unknown,
        }
    }
}

/// One port entry shared by both composition roots.
///
/// `id`/`label`/`kind`/`vendor_id`/`product_id`/`authorized` come from the
/// platform-neutral descriptor. `detail` is the native-only display detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortView {
    pub id: PortId,
    pub label: String,
    pub kind: PortKind,
    pub detail: PortTypeView,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub authorized: bool,
}

impl PortView {
    /// A manually registered network port, as used by the native device panel.
    pub fn network(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id: PortId::new(name.clone()),
            label: name,
            kind: PortKind::Network,
            detail: PortTypeView::Network,
            vendor_id: None,
            product_id: None,
            authorized: true,
        }
    }
}

impl From<PortDescriptor> for PortView {
    fn from(port: PortDescriptor) -> Self {
        let detail = match port.kind {
            PortKind::Network => PortTypeView::Network,
            PortKind::Serial | PortKind::Unknown => PortTypeView::Unknown,
        };
        Self {
            id: port.id,
            label: port.label,
            kind: port.kind,
            detail,
            vendor_id: port.vendor_id,
            product_id: port.product_id,
            authorized: port.authorized,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<tool_transport::SerialPortDescriptor> for PortView {
    fn from(port: tool_transport::SerialPortDescriptor) -> Self {
        let detail = PortTypeView::from(port.port_type);
        let kind = if detail.is_network() {
            PortKind::Network
        } else {
            PortKind::Serial
        };
        let label = port.port_name.clone();
        Self {
            id: PortId::new(port.port_name),
            label,
            kind,
            detail,
            vendor_id: None,
            product_id: None,
            authorized: true,
        }
    }
}

/// Native serial configuration as editable strings.
///
/// The native settings panel edits these strings directly. Web does not use
/// them and keeps the default value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialConfigView {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: String,
    pub stop_bits: String,
    pub parity: String,
}

impl Default for SerialConfigView {
    fn default() -> Self {
        Self {
            port_name: String::new(),
            baud_rate: 115_200,
            data_bits: "8".to_owned(),
            stop_bits: "1".to_owned(),
            parity: "none".to_owned(),
        }
    }
}

/// Per-port lifecycle status in the shared view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortStatusView {
    pub open: bool,
    pub port_name: Option<String>,
    pub baud_rate: Option<u32>,
    pub connecting: bool,
}

impl PortStatusView {
    pub fn closed() -> Self {
        Self {
            open: false,
            port_name: None,
            baud_rate: None,
            connecting: false,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<tool_transport::TransportStatus> for PortStatusView {
    fn from(status: tool_transport::TransportStatus) -> Self {
        Self {
            open: status.open,
            port_name: status.port_name,
            baud_rate: status.baud_rate,
            connecting: status.connecting,
        }
    }
}

/// The single read view for transport state.
///
/// `statuses`/`open_ports`/`config`/`auto_reconnect` are populated by the
/// native runtime; web populates `ports`/`connected`/`connecting`/`settings`/
/// `status` and leaves the native-only fields empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportView {
    base_capabilities: TransportCapabilities,
    pub capabilities: TransportCapabilities,
    pub ports: Vec<PortView>,
    pub open_ports: Vec<String>,
    pub statuses: Vec<PortStatusView>,
    pub connected: Option<PortId>,
    pub connecting: bool,
    pub settings: SerialSettings,
    pub config: SerialConfigView,
    pub auto_reconnect: bool,
    pub status: String,
}

impl TransportView {
    pub fn new(capabilities: TransportCapabilities) -> Self {
        Self {
            base_capabilities: capabilities,
            capabilities,
            ports: Vec::new(),
            open_ports: Vec::new(),
            statuses: Vec::new(),
            connected: None,
            connecting: false,
            settings: SerialSettings::default(),
            config: SerialConfigView::default(),
            auto_reconnect: false,
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

    pub fn upsert_port(&mut self, port: impl Into<PortView>) {
        let port = port.into();
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

    /// Per-port status by name, if the runtime publishes statuses.
    pub fn status_for(&self, port: &str) -> Option<&PortStatusView> {
        self.statuses
            .iter()
            .find(|status| status.port_name.as_deref() == Some(port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(target_arch = "wasm32"))]
    use tool_transport::SerialPortDescriptor;

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

    #[test]
    fn native_descriptor_keeps_detail_and_network_kind() {
        let serial = PortView::from(SerialPortDescriptor {
            port_name: "COM3".to_owned(),
            port_type: tool_transport::PortType::Usb("CP210x".to_owned()),
        });
        assert_eq!(serial.id.as_str(), "COM3");
        assert_eq!(serial.kind, PortKind::Serial);
        assert_eq!(serial.detail.to_string(), "CP210x");

        let network = PortView::from(SerialPortDescriptor {
            port_name: "127.0.0.1:7125".to_owned(),
            port_type: tool_transport::PortType::Network,
        });
        assert_eq!(network.kind, PortKind::Network);
        assert!(network.detail.is_network());
    }
}
