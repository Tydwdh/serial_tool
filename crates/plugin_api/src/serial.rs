use serde::{Deserialize, Serialize};
use tool_platform::{PortDescriptor, PortId, SerialParity, SerialSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginParity {
    None,
    Odd,
    Even,
}

impl From<PluginParity> for SerialParity {
    fn from(value: PluginParity) -> Self {
        match value {
            PluginParity::None => Self::None,
            PluginParity::Odd => Self::Odd,
            PluginParity::Even => Self::Even,
        }
    }
}

impl From<SerialParity> for PluginParity {
    fn from(value: SerialParity) -> Self {
        match value {
            SerialParity::None => Self::None,
            SerialParity::Odd => Self::Odd,
            SerialParity::Even => Self::Even,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSerialSettings {
    pub baud_rate: u32,
    pub data_bits: u8,
    pub stop_bits: u8,
    pub parity: PluginParity,
}

impl Default for PluginSerialSettings {
    fn default() -> Self {
        Self {
            baud_rate: 115_200,
            data_bits: 8,
            stop_bits: 1,
            parity: PluginParity::None,
        }
    }
}

impl From<PluginSerialSettings> for SerialSettings {
    fn from(value: PluginSerialSettings) -> Self {
        Self {
            baud_rate: value.baud_rate,
            data_bits: value.data_bits,
            stop_bits: value.stop_bits,
            parity: value.parity.into(),
        }
    }
}

impl From<SerialSettings> for PluginSerialSettings {
    fn from(value: SerialSettings) -> Self {
        Self {
            baud_rate: value.baud_rate,
            data_bits: value.data_bits,
            stop_bits: value.stop_bits,
            parity: value.parity.into(),
        }
    }
}

/// Device DTO shown to a plugin. `id` is opaque; it is not a COM path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSerialDevice {
    pub id: PortId,
    pub label: String,
    pub kind: String,
    pub authorized: bool,
}

impl From<PortDescriptor> for PluginSerialDevice {
    fn from(value: PortDescriptor) -> Self {
        Self {
            id: value.id,
            label: value.label,
            kind: format!("{:?}", value.kind).to_lowercase(),
            authorized: value.authorized,
        }
    }
}
