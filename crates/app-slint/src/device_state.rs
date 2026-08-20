use tool_transport::{SerialConfig, SerialPortDescriptor, TransportManager, TransportStatus};

#[derive(Clone)]
pub struct DeviceState {
    pub ports: Vec<SerialPortDescriptor>,
    pub network_ports: Vec<tool_transport::NetworkSerialConfig>,
    pub selected_port: Option<String>,
    pub baud_rate: String,
    pub data_bits: String,
    pub stop_bits: String,
    pub parity: String,
    pub port_aliases: std::collections::HashMap<String, String>,
    pub auto_reconnect: bool,
    pub last_error: Option<String>,
}

impl DeviceState {
    pub fn from_config(cfg: &crate::config::PersistedConfig) -> Self {
        Self {
            ports: Vec::new(),
            network_ports: cfg.network_ports.clone(),
            selected_port: cfg.selected_port.clone(),
            baud_rate: cfg.baud_rate.clone(),
            data_bits: cfg.data_bits.clone(),
            stop_bits: cfg.stop_bits.clone(),
            parity: cfg.parity.clone(),
            port_aliases: cfg.port_aliases.clone(),
            auto_reconnect: cfg.auto_reconnect,
            last_error: None,
        }
    }

    pub fn refresh_ports(&mut self, transport: &TransportManager) {
        match transport.list_serial_ports() {
            Ok(list) => {
                self.ports = list;
                self.last_error = None;
            }
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    pub fn open_selected(&mut self, transport: &TransportManager) -> Result<(), String> {
        let port = self.selected_port.clone().ok_or_else(|| "未选择端口".to_owned())?;
        let cfg = SerialConfig {
            port_name: port.clone(),
            baud_rate: self.baud_rate.parse().unwrap_or(115200),
            data_bits: parse_data_bits(&self.data_bits),
            stop_bits: parse_stop_bits(&self.stop_bits),
            parity: parse_parity(&self.parity),
        };
        transport.open_serial(cfg).map_err(|e| e.to_string())
    }

    pub fn close_selected(&self, transport: &TransportManager) {
        if let Some(port) = &self.selected_port {
            transport.close_port(port);
        }
    }

    pub fn port_label(&self, port: &str) -> String {
        if let Some(alias) = self.port_aliases.get(port).filter(|a| !a.trim().is_empty()) {
            format!("{alias} ({port})")
        } else {
            port.to_owned()
        }
    }

    pub fn status_for(&self, transport: &TransportManager, port: &str) -> TransportStatus {
        transport.status_port(port)
    }
}

fn parse_data_bits(v: &str) -> tool_transport::DataBits {
    match v {
        "5" => tool_transport::DataBits::Five,
        "6" => tool_transport::DataBits::Six,
        "7" => tool_transport::DataBits::Seven,
        _ => tool_transport::DataBits::Eight,
    }
}
fn parse_stop_bits(v: &str) -> tool_transport::StopBits {
    match v {
        "2" => tool_transport::StopBits::Two,
        _ => tool_transport::StopBits::One,
    }
}
fn parse_parity(v: &str) -> tool_transport::Parity {
    match v {
        "odd" => tool_transport::Parity::Odd,
        "even" => tool_transport::Parity::Even,
        _ => tool_transport::Parity::None,
    }
}
