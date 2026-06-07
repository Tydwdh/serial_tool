use std::collections::BTreeSet;
use std::path::PathBuf;
use tool_core::{Event, LogLevel};
use tool_databus::DataBus;
use tool_extension::PluginManager;
use tool_panels::{ChartPanel, DynamicPanels, FormPanel, PanelManager, TerminalPanel};
use tool_transport::{DataBits, Parity, SerialConfig, StopBits, TransportManager};

/// 每个 COM 端口或主界面的完全独立工作区
pub struct Workspace {
    pub id: String,
    pub bus: DataBus,
    pub manager: TransportManager,
    pub plugin_manager: PluginManager,
    pub terminal: TerminalPanel,
    pub chart: ChartPanel,
    pub form: FormPanel,
    pub dynamic_panels: DynamicPanels,
    pub panels: PanelManager,
    pub send: PortSendState,
    pub inspector_visible: bool,
    pub status_message: String,
    pub port_name: String,
    pub baud_rate: String,
    pub data_bits: String,
    pub stop_bits: String,
    pub parity: String,
    pub timeout_ms: String,
    pub detached_dynamic_panels: BTreeSet<String>,
    pub top_bar_serial_collapsed: bool,
    pub last_rate_check_time: f64,
    pub last_event_count: usize,
    pub event_rate: f64,
    pub rx_byte_rate: f64,
    pub tx_byte_rate: f64,
    pub total_rx_bytes: u64,
    pub total_tx_bytes: u64,
}

#[derive(Clone, Default)]
pub struct PortSendState {
    pub input: String,
    pub hex_mode: bool,
    pub append_lf: bool,
    pub error: Option<String>,
}

impl Workspace {
    pub fn new_main() -> Self {
        let bus = DataBus::new();
        let manager = TransportManager::new(bus.clone());
        let mut pm = PluginManager::new(bus.clone(), manager.clone());
        let _ = pm.discover_roots([PathBuf::from("plugins")]);
        Self {
            id: "main".into(),
            terminal: TerminalPanel::new(&bus),
            chart: ChartPanel::new(&bus),
            form: FormPanel::new(&bus),
            dynamic_panels: DynamicPanels::new(&bus),
            panels: PanelManager::default(),
            send: PortSendState::default(),
            inspector_visible: false,
            status_message: "就绪".into(),
            port_name: String::new(),
            baud_rate: "115200".into(),
            data_bits: "8".into(),
            stop_bits: "1".into(),
            parity: "none".into(),
            timeout_ms: "50".into(),
            detached_dynamic_panels: BTreeSet::new(),
            top_bar_serial_collapsed: false,
            last_rate_check_time: 0.0, last_event_count: 0, event_rate: 0.0,
            rx_byte_rate: 0.0, tx_byte_rate: 0.0, total_rx_bytes: 0, total_tx_bytes: 0,
            bus, manager, plugin_manager: pm,
        }
    }

    pub fn new_port(port_name: String) -> Self {
        let bus = DataBus::new();
        let manager = TransportManager::new(bus.clone());
        let mut pm = PluginManager::new(bus.clone(), manager.clone());
        let _ = pm.discover_roots([PathBuf::from("plugins")]);
        Self {
            id: port_name.clone(),
            terminal: TerminalPanel::new(&bus),
            chart: ChartPanel::new(&bus),
            form: FormPanel::new(&bus),
            dynamic_panels: DynamicPanels::new(&bus),
            panels: PanelManager::default(),
            send: PortSendState::default(),
            inspector_visible: false,
            status_message: format!("{port_name} 就绪"),
            port_name,
            baud_rate: "115200".into(),
            data_bits: "8".into(),
            stop_bits: "1".into(),
            parity: "none".into(),
            timeout_ms: "50".into(),
            detached_dynamic_panels: BTreeSet::new(),
            top_bar_serial_collapsed: false,
            last_rate_check_time: 0.0, last_event_count: 0, event_rate: 0.0,
            rx_byte_rate: 0.0, tx_byte_rate: 0.0, total_rx_bytes: 0, total_tx_bytes: 0,
            bus, manager, plugin_manager: pm,
        }
    }

    pub fn open_serial(&mut self) -> Result<(), String> {
        let baud = self.baud_rate.parse().unwrap_or(115200);
        let cfg = SerialConfig {
            port_name: self.port_name.clone(),
            baud_rate: baud,
            data_bits: parse_db(&self.data_bits),
            stop_bits: parse_sb(&self.stop_bits),
            parity: parse_par(&self.parity),
            timeout_ms: self.timeout_ms.parse().unwrap_or(50),
        };
        self.manager.open_serial(cfg).map_err(|e| e.to_string())
    }

    pub fn log(&self, level: LogLevel, msg: impl Into<String>) {
        self.bus.publish(Event::system_log(level, &self.id, msg.into()));
    }
}

fn parse_db(v: &str) -> DataBits { match v { "5" => DataBits::Five, "6" => DataBits::Six, "7" => DataBits::Seven, _ => DataBits::Eight } }
fn parse_sb(v: &str) -> StopBits { match v { "2" => StopBits::Two, _ => StopBits::One } }
fn parse_par(v: &str) -> Parity { match v { "odd" => Parity::Odd, "even" => Parity::Even, _ => Parity::None } }
