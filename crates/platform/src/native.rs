//! Native implementation of the platform transport capability.

use std::thread;

use futures_channel::oneshot;
use futures_executor::block_on;
use tool_transport::{DataBits, Parity, SerialConfig, StopBits, TransportManager};

use crate::{
    PortDescriptor, PortId, PortKind, SerialParity, SerialSettings, TransportBackend,
    TransportCapabilities, TransportError, TransportFuture, TransportResult,
};

#[derive(Clone)]
pub struct NativeTransportBackend {
    manager: TransportManager,
}

impl NativeTransportBackend {
    pub fn new(manager: TransportManager) -> Self {
        Self { manager }
    }

    pub fn manager(&self) -> &TransportManager {
        &self.manager
    }
}

fn spawn_blocking<T, F>(work: F) -> TransportFuture<T>
where
    T: Send + 'static,
    F: FnOnce() -> TransportResult<T> + Send + 'static,
{
    let (sender, receiver) = oneshot::channel();
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    Box::pin(async move {
        receiver
            .await
            .map_err(|_| TransportError::Operation("native transport worker stopped".into()))?
    })
}

fn settings_to_native(port: PortId, settings: SerialSettings) -> SerialConfig {
    SerialConfig {
        port_name: port.to_string(),
        baud_rate: settings.baud_rate,
        data_bits: match settings.data_bits {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        },
        stop_bits: if settings.stop_bits == 2 {
            StopBits::Two
        } else {
            StopBits::One
        },
        parity: match settings.parity {
            SerialParity::Odd => Parity::Odd,
            SerialParity::Even => Parity::Even,
            SerialParity::None => Parity::None,
        },
    }
}

impl TransportBackend for NativeTransportBackend {
    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities::NATIVE_SERIAL
    }

    fn list_known_ports(&self) -> TransportFuture<Vec<PortDescriptor>> {
        let manager = self.manager.clone();
        spawn_blocking(move || {
            manager
                .list_serial_ports()
                .map(|ports| {
                    ports
                        .into_iter()
                        .map(|port| PortDescriptor {
                            id: PortId::new(port.port_name.clone()),
                            label: port.port_name,
                            kind: PortKind::Serial,
                            vendor_id: None,
                            product_id: None,
                            authorized: true,
                        })
                        .collect()
                })
                .map_err(|error| TransportError::Operation(error.to_string()))
        })
    }

    fn request_port(&self) -> TransportFuture<PortDescriptor> {
        // Native has no permission prompt. The closest equivalent is choosing
        // the first currently known port; desktop UI normally exposes the
        // complete list separately.
        let future = self.list_known_ports();
        Box::pin(async move {
            future
                .await?
                .into_iter()
                .next()
                .ok_or(TransportError::NoPortAvailable)
        })
    }

    fn connect(&self, port: PortId, settings: SerialSettings) -> TransportFuture<()> {
        let manager = self.manager.clone();
        spawn_blocking(move || {
            manager
                .open_serial(settings_to_native(port, settings))
                .map_err(|error| TransportError::Operation(error.to_string()))
        })
    }

    fn disconnect(&self, port: PortId) -> TransportFuture<()> {
        let manager = self.manager.clone();
        spawn_blocking(move || {
            manager.close_port(port.as_str());
            Ok(())
        })
    }

    fn send(&self, port: PortId, bytes: Vec<u8>) -> TransportFuture<()> {
        let manager = self.manager.clone();
        spawn_blocking(move || {
            manager
                .send_to(port.as_str(), bytes)
                .map_err(|error| TransportError::Operation(error.to_string()))
        })
    }

    fn set_dtr(&self, port: PortId, value: bool) -> TransportFuture<()> {
        let manager = self.manager.clone();
        spawn_blocking(move || {
            manager
                .set_dtr(port.as_str(), value)
                .map_err(|error| TransportError::Operation(error.to_string()))
        })
    }

    fn set_rts(&self, port: PortId, value: bool) -> TransportFuture<()> {
        let manager = self.manager.clone();
        spawn_blocking(move || {
            manager
                .set_rts(port.as_str(), value)
                .map_err(|error| TransportError::Operation(error.to_string()))
        })
    }
}

/// Run a platform transport future on a native worker.
pub fn block_on_transport<T>(future: TransportFuture<T>) -> TransportResult<T> {
    block_on(future)
}
