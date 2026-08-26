//! Web Serial backend.
//!
//! `request_port` must only be called from a user gesture. The backend never
//! calls it during startup or a background refresh; `list_known_ports` maps to
//! the browser's already-authorized `navigator.serial.getPorts()` instead.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use js_sys::{Array, JsString, Object, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    Event, EventTarget, Navigator, ReadableStreamDefaultReader, Serial, SerialOptions,
    SerialOutputSignals, SerialPort, WritableStreamDefaultWriter,
};

use crate::{
    PortDescriptor, PortId, PortKind, SerialParity, SerialSettings, TransportBackend,
    TransportCapabilities, TransportError, TransportFuture, TransportResult,
};

type ConnectionListener = Closure<dyn FnMut(Event)>;

#[derive(Clone)]
pub struct WebSerialTransport {
    serial: Serial,
    ports: Rc<RefCell<HashMap<PortId, SerialPort>>>,
    readers: Rc<RefCell<HashMap<PortId, ReadableStreamDefaultReader>>>,
    next_session_id: Rc<Cell<u64>>,
    connection_listeners: Rc<RefCell<Vec<ConnectionListener>>>,
}

#[derive(Debug, Clone)]
pub enum WebSerialEvent {
    Connected(PortDescriptor),
    Disconnected(PortId),
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
            readers: Rc::new(RefCell::new(HashMap::new())),
            next_session_id: Rc::new(Cell::new(1)),
            connection_listeners: Rc::new(RefCell::new(Vec::new())),
        })
    }

    pub fn from_window() -> Result<Self, TransportError> {
        let window = web_sys::window()
            .ok_or_else(|| TransportError::Operation("window unavailable".into()))?;
        Self::from_navigator(window.navigator())
    }

    /// Listen for browser-level device insertion/removal. This complements
    /// the readable-stream callback: the browser may report a device being
    /// connected before it is opened, or removed while no reader is active.
    pub fn watch_connection_events(
        &self,
        callback: Rc<dyn Fn(WebSerialEvent)>,
    ) -> Result<(), TransportError> {
        let target: &EventTarget = self.serial.unchecked_ref();

        let connect_backend = self.clone();
        let connect_callback = callback.clone();
        let on_connect = Closure::wrap(Box::new(move |event: Event| {
            let Ok(value) = Reflect::get(event.as_ref(), &JsString::from("port")) else {
                return;
            };
            let Ok(port) = value.dyn_into::<SerialPort>() else {
                return;
            };
            let (id, descriptor) = connect_backend.descriptor(&port, 0);
            connect_backend.remember(id, port);
            connect_callback(WebSerialEvent::Connected(descriptor));
        }) as Box<dyn FnMut(Event)>);
        target
            .add_event_listener_with_callback("connect", on_connect.as_ref().unchecked_ref())
            .map_err(|error| TransportError::Operation(js_error(&error)))?;
        // Keep the closure alive before installing the second listener. If
        // the browser rejects the disconnect listener, the connect listener
        // must not be dropped accidentally.
        self.connection_listeners.borrow_mut().push(on_connect);

        let disconnect_backend = self.clone();
        let disconnect_callback = callback;
        let on_disconnect = Closure::wrap(Box::new(move |event: Event| {
            let Ok(value) = Reflect::get(event.as_ref(), &JsString::from("port")) else {
                return;
            };
            let Ok(port) = value.dyn_into::<SerialPort>() else {
                return;
            };
            let id = disconnect_backend
                .find_session_id(&port)
                .unwrap_or_else(|| disconnect_backend.descriptor(&port, 0).0);
            disconnect_callback(WebSerialEvent::Disconnected(id));
        }) as Box<dyn FnMut(Event)>);
        target
            .add_event_listener_with_callback("disconnect", on_disconnect.as_ref().unchecked_ref())
            .map_err(|error| TransportError::Operation(js_error(&error)))?;

        self.connection_listeners.borrow_mut().push(on_disconnect);
        Ok(())
    }

    fn descriptor(&self, port: &SerialPort, ordinal: usize) -> (PortId, PortDescriptor) {
        let info = port.get_info();
        let vendor_id = info.get_usb_vendor_id();
        let product_id = info.get_usb_product_id();
        let id = self
            .find_session_id(port)
            .unwrap_or_else(|| self.allocate_session_id());
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

    fn allocate_session_id(&self) -> PortId {
        let next = self.next_session_id.get();
        self.next_session_id.set(next + 1);
        PortId::new(format!("webserial:session:{next}"))
    }

    fn find_session_id(&self, port: &SerialPort) -> Option<PortId> {
        let value: &JsValue = port.as_ref();
        self.ports
            .borrow()
            .iter()
            .find_map(|(id, known)| Object::is(value, known.as_ref()).then(|| id.clone()))
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
        self.readers.borrow_mut().insert(id.clone(), reader.clone());
        let readers = self.readers.clone();
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
            readers.borrow_mut().remove(&id);
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
                let (id, descriptor) = backend.descriptor(&port, ordinal);
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
            let (id, descriptor) = backend.descriptor(&port, 0);
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
            let reader = { backend.readers.borrow_mut().remove(&port_id) };
            if let Some(reader) = reader {
                // A SerialPort cannot be closed while its readable stream is
                // locked. Cancelling first also wakes the receive task so it
                // can publish the matching Disconnected event.
                let _ = WebSerialTransport::await_promise(reader.cancel()).await;
                reader.release_lock();
            }
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
