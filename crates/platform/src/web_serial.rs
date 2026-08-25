//! Web Serial backend.
//!
//! `request_port` must only be called from a user gesture. The backend never
//! calls it during startup or a background refresh; `list_known_ports` maps to
//! the browser's already-authorized `navigator.serial.getPorts()` instead.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use js_sys::{Array, JsString, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    Navigator, ReadableStreamDefaultReader, Serial, SerialOptions, SerialOutputSignals, SerialPort,
    WritableStreamDefaultWriter,
};

use crate::{
    PortDescriptor, PortId, PortKind, SerialParity, SerialSettings, TransportBackend,
    TransportCapabilities, TransportError, TransportFuture, TransportResult,
};

#[derive(Clone)]
pub struct WebSerialTransport {
    serial: Serial,
    ports: Rc<RefCell<HashMap<PortId, SerialPort>>>,
    next_requested_id: Rc<Cell<u64>>,
}

impl WebSerialTransport {
    pub fn from_navigator(navigator: Navigator) -> Result<Self, TransportError> {
        let serial_value = Reflect::get(navigator.as_ref(), &JsString::from("serial"))
            .map_err(|error| TransportError::Operation(js_error(&error)))?;
        if serial_value.is_undefined() || serial_value.is_null() {
            return Err(TransportError::Unsupported);
        }
        Ok(Self {
            serial: navigator.serial(),
            ports: Rc::new(RefCell::new(HashMap::new())),
            next_requested_id: Rc::new(Cell::new(1)),
        })
    }

    pub fn from_window() -> Result<Self, TransportError> {
        let window = web_sys::window()
            .ok_or_else(|| TransportError::Operation("window unavailable".into()))?;
        Self::from_navigator(window.navigator())
    }

    fn descriptor(
        &self,
        port: &SerialPort,
        ordinal: usize,
        requested: bool,
    ) -> (PortId, PortDescriptor) {
        let info = port.get_info();
        let vendor_id = info.get_usb_vendor_id();
        let product_id = info.get_usb_product_id();
        let id = if requested {
            let next = self.next_requested_id.get();
            self.next_requested_id.set(next + 1);
            PortId::new(format!("webserial:requested:{next}"))
        } else {
            PortId::new(format!(
                "webserial:{:04x}:{:04x}:{ordinal}",
                vendor_id.unwrap_or_default(),
                product_id.unwrap_or_default()
            ))
        };
        let label = match (vendor_id, product_id) {
            (Some(vendor), Some(product)) => format!("USB {vendor:04X}:{product:04X}"),
            _ => format!("Web Serial {ordinal}"),
        };
        let descriptor = PortDescriptor {
            id: id.clone(),
            label,
            kind: PortKind::Serial,
            vendor_id,
            product_id,
            authorized: true,
        };
        (id, descriptor)
    }

    fn remember(&self, id: PortId, port: SerialPort) {
        self.ports.borrow_mut().insert(id, port);
    }

    fn port(&self, id: &PortId) -> Result<SerialPort, TransportError> {
        self.ports
            .borrow()
            .get(id)
            .cloned()
            .ok_or_else(|| TransportError::PortNotKnown(id.clone()))
    }

    /// Start the browser read loop. The callback is invoked for each
    /// `Uint8Array` chunk and must forward bytes into DataBus.
    pub fn start_receive(
        &self,
        id: PortId,
        sink: Rc<dyn Fn(Vec<u8>)>,
    ) -> Result<(), TransportError> {
        self.start_receive_with_disconnect(id, sink, Rc::new(|_| {}))
    }

    /// Start the browser read loop and notify the application when the stream
    /// ends or reports an error (including a device unplug).
    pub fn start_receive_with_disconnect(
        &self,
        id: PortId,
        sink: Rc<dyn Fn(Vec<u8>)>,
        on_disconnect: Rc<dyn Fn(PortId)>,
    ) -> Result<(), TransportError> {
        let port = self.port(&id)?;
        let readable = port.readable();
        let reader_object = readable.get_reader();
        let reader: ReadableStreamDefaultReader = reader_object.unchecked_into();
        spawn_local(async move {
            loop {
                let result = match JsFuture::from(reader.read()).await {
                    Ok(result) => result,
                    Err(error) => {
                        web_sys::console::error_1(&error);
                        break;
                    }
                };
                let done = Reflect::get(&result, &JsString::from("done"))
                    .ok()
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                if done {
                    break;
                }
                let Ok(value) = Reflect::get(&result, &JsString::from("value")) else {
                    break;
                };
                if value.is_undefined() || value.is_null() {
                    continue;
                }
                sink(Uint8Array::new(&value).to_vec());
            }
            reader.release_lock();
            on_disconnect(id);
        });
        Ok(())
    }

    async fn await_promise<P>(promise: P) -> TransportResult<JsValue>
    where
        P: JsCast,
    {
        JsFuture::from(promise.unchecked_into::<js_sys::Promise>())
            .await
            .map_err(|error| TransportError::Operation(js_error(&error)))
    }
}

fn js_error(value: &JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}

impl TransportBackend for WebSerialTransport {
    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities::WEB_SERIAL
    }

    fn list_known_ports(&self) -> TransportFuture<Vec<PortDescriptor>> {
        let backend = self.clone();
        Box::pin(async move {
            let array =
                Array::from(&WebSerialTransport::await_promise(backend.serial.get_ports()).await?);
            let mut descriptors = Vec::with_capacity(array.length() as usize);
            for (ordinal, value) in array.iter().enumerate() {
                let port: SerialPort = value
                    .dyn_into()
                    .map_err(|_| TransportError::Operation("invalid SerialPort value".into()))?;
                let (id, descriptor) = backend.descriptor(&port, ordinal, false);
                backend.remember(id, port);
                descriptors.push(descriptor);
            }
            Ok(descriptors)
        })
    }

    fn request_port(&self) -> TransportFuture<PortDescriptor> {
        let backend = self.clone();
        // Construct the browser Promise before returning the future. The
        // Application invokes this method directly from the button command,
        // preserving the transient user activation required by requestPort().
        let promise = backend.serial.request_port();
        Box::pin(async move {
            let value = WebSerialTransport::await_promise(promise).await?;
            let port: SerialPort = value
                .dyn_into()
                .map_err(|_| TransportError::PermissionDenied)?;
            let (id, descriptor) = backend.descriptor(&port, 0, true);
            backend.remember(id, port);
            Ok(descriptor)
        })
    }

    fn connect(&self, port_id: PortId, settings: SerialSettings) -> TransportFuture<()> {
        let backend = self.clone();
        Box::pin(async move {
            let port = backend.port(&port_id)?;
            let options = SerialOptions::new(settings.baud_rate);
            options.set_data_bits(settings.data_bits);
            options.set_stop_bits(settings.stop_bits);
            options.set_parity(match settings.parity {
                SerialParity::None => web_sys::ParityType::None,
                SerialParity::Odd => web_sys::ParityType::Odd,
                SerialParity::Even => web_sys::ParityType::Even,
            });
            WebSerialTransport::await_promise(port.open(&options)).await?;
            Ok(())
        })
    }

    fn disconnect(&self, port_id: PortId) -> TransportFuture<()> {
        let backend = self.clone();
        Box::pin(async move {
            let port = backend.port(&port_id)?;
            WebSerialTransport::await_promise(port.close()).await?;
            Ok(())
        })
    }

    fn send(&self, port_id: PortId, bytes: Vec<u8>) -> TransportFuture<()> {
        let backend = self.clone();
        Box::pin(async move {
            let port = backend.port(&port_id)?;
            let writer_object = port.writable().get_writer().map_err(|error| {
                TransportError::Operation(format!("get writer failed: {}", js_error(&error)))
            })?;
            let writer: WritableStreamDefaultWriter = writer_object;
            let value: JsValue = Uint8Array::from(bytes.as_slice()).into();
            WebSerialTransport::await_promise(writer.write_with_chunk(&value)).await?;
            writer.release_lock();
            Ok(())
        })
    }

    fn set_dtr(&self, port_id: PortId, value: bool) -> TransportFuture<()> {
        self.set_signals(port_id, Some(value), None)
    }

    fn set_rts(&self, port_id: PortId, value: bool) -> TransportFuture<()> {
        self.set_signals(port_id, None, Some(value))
    }
}

impl WebSerialTransport {
    fn set_signals(
        &self,
        port_id: PortId,
        dtr: Option<bool>,
        rts: Option<bool>,
    ) -> TransportFuture<()> {
        let backend = self.clone();
        Box::pin(async move {
            let port = backend.port(&port_id)?;
            let signals = SerialOutputSignals::new();
            if let Some(value) = dtr {
                signals.set_data_terminal_ready(value);
            }
            if let Some(value) = rts {
                signals.set_request_to_send(value);
            }
            WebSerialTransport::await_promise(port.set_signals_with_signals(&signals)).await?;
            Ok(())
        })
    }
}
