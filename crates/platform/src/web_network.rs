//! Browser WebSocket transport for the network-serial endpoint.
//!
//! Native uses the same Nexus/Moonraker JSON-RPC protocol from a worker
//! thread. Browsers already provide an event-driven WebSocket API, so this
//! implementation keeps the socket and its callbacks in the platform layer
//! and exposes only asynchronous connect/send/disconnect operations to the
//! application composition root.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use futures_channel::oneshot;
use js_sys::{ArrayBuffer, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

use crate::{NetworkSerialConfig, PortId, TransportError, TransportFuture, TransportResult};

type ReceiveHandler = Rc<dyn Fn(Vec<u8>)>;
type DisconnectHandler = Rc<dyn Fn(PortId)>;

/// Event-driven browser network serial backend.
#[derive(Clone, Default)]
pub struct WebNetworkTransport {
    connections: Rc<RefCell<BTreeMap<PortId, WebNetworkConnection>>>,
}

struct WebNetworkConnection {
    socket: WebSocket,
    next_id: Rc<Cell<u64>>,
    closing: Rc<Cell<bool>>,
    on_open: Closure<dyn FnMut(Event)>,
    on_error: Closure<dyn FnMut(Event)>,
    on_close: Closure<dyn FnMut(CloseEvent)>,
    on_message: Closure<dyn FnMut(MessageEvent)>,
}

impl WebNetworkConnection {
    fn close(self) {
        self.closing.set(true);
        self.socket.set_onopen(None);
        self.socket.set_onerror(None);
        self.socket.set_onclose(None);
        self.socket.set_onmessage(None);
        let _ = self.socket.close();
        // Keep the callback fields in the struct until after close() has been
        // called. The browser may synchronously dispatch a close event.
        drop((self.on_open, self.on_error, self.on_close, self.on_message));
    }
}

impl WebNetworkTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_connected(&self, port: &PortId) -> bool {
        self.connections.borrow().contains_key(port)
    }

    /// Open a Nexus/Moonraker WebSocket and install the RX/disconnect sinks.
    /// The returned future completes after the browser's `open` event, not
    /// merely after `WebSocket::new` has created a connecting socket.
    pub fn connect(
        &self,
        config: NetworkSerialConfig,
        on_receive: ReceiveHandler,
        on_disconnect: DisconnectHandler,
    ) -> TransportFuture<()> {
        let connections = self.connections.clone();
        Box::pin(async move {
            let port = config.port_id();
            if let Some(existing) = connections.borrow_mut().remove(&port) {
                existing.close();
            }

            let socket = WebSocket::new(&websocket_url(&config))
                .map_err(|error| TransportError::Operation(js_error(&error)))?;
            socket.set_binary_type(BinaryType::Arraybuffer);

            let opened = Rc::new(Cell::new(false));
            let closing = Rc::new(Cell::new(false));
            let (open_sender, open_receiver) = oneshot::channel::<TransportResult<()>>();
            let open_sender = Rc::new(RefCell::new(Some(open_sender)));

            let open_sender_for_open = open_sender.clone();
            let opened_for_open = opened.clone();
            let on_open = Closure::wrap(Box::new(move |_event: Event| {
                opened_for_open.set(true);
                if let Some(sender) = open_sender_for_open.borrow_mut().take() {
                    let _ = sender.send(Ok(()));
                }
            }) as Box<dyn FnMut(Event)>);

            let open_sender_for_error = open_sender.clone();
            let on_error = Closure::wrap(Box::new(move |_event: Event| {
                if let Some(sender) = open_sender_for_error.borrow_mut().take() {
                    let _ = sender.send(Err(TransportError::Operation(
                        "网络串口 WebSocket 连接失败".to_owned(),
                    )));
                }
            }) as Box<dyn FnMut(Event)>);

            let open_sender_for_close = open_sender.clone();
            let opened_for_close = opened.clone();
            let closing_for_close = closing.clone();
            let disconnected_port = port.clone();
            let disconnect_handler = on_disconnect.clone();
            let on_close = Closure::wrap(Box::new(move |_event: CloseEvent| {
                if !opened_for_close.get()
                    && let Some(sender) = open_sender_for_close.borrow_mut().take()
                {
                    let _ = sender.send(Err(TransportError::Operation(
                        "网络串口 WebSocket 在打开前关闭".to_owned(),
                    )));
                } else if !closing_for_close.get() {
                    disconnect_handler(disconnected_port.clone());
                }
            }) as Box<dyn FnMut(CloseEvent)>);

            let receive_handler = on_receive.clone();
            let on_message = Closure::wrap(Box::new(move |event: MessageEvent| {
                if let Some(text) = event.data().as_string() {
                    if let Some(content) = parse_gcode_response(&text) {
                        let mut bytes = content.into_bytes();
                        bytes.push(b'\n');
                        receive_handler(bytes);
                    }
                    return;
                }
                if let Ok(buffer) = event.data().dyn_into::<ArrayBuffer>() {
                    receive_handler(Uint8Array::new(&buffer).to_vec());
                }
            }) as Box<dyn FnMut(MessageEvent)>);

            socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
            socket.set_onerror(Some(on_error.as_ref().unchecked_ref()));
            socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
            socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

            let open_result = match open_receiver.await {
                Ok(result) => result,
                Err(_) => Err(TransportError::Operation(
                    "网络串口连接任务已取消".to_owned(),
                )),
            };
            if let Err(error) = open_result {
                socket.set_onopen(None);
                socket.set_onerror(None);
                socket.set_onclose(None);
                socket.set_onmessage(None);
                let _ = socket.close();
                return Err(error);
            }

            // Moonraker accepts this identify notification before the gcode
            // requests. Keep the API key field for compatible gateways; the
            // native transport historically ignored it when it was absent.
            let mut params = serde_json::json!({
                "client_name": "Hardware Workbench",
                "version": env!("CARGO_PKG_VERSION"),
                "type": "web",
            });
            if let Some(api_key) = &config.api_key {
                params["access_token"] = serde_json::Value::String(api_key.clone());
            }
            let identify = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "server.connection.identify",
                "params": params,
                "id": 1,
            });
            if let Err(error) = socket.send_with_str(&identify.to_string()) {
                socket.set_onopen(None);
                socket.set_onerror(None);
                socket.set_onclose(None);
                socket.set_onmessage(None);
                let _ = socket.close();
                return Err(TransportError::Operation(js_error(&error)));
            }

            let connection = WebNetworkConnection {
                socket,
                next_id: Rc::new(Cell::new(2)),
                closing,
                on_open,
                on_error,
                on_close,
                on_message,
            };
            connections.borrow_mut().insert(port, connection);
            Ok(())
        })
    }

    pub fn disconnect(&self, port: PortId) -> TransportFuture<()> {
        let connections = self.connections.clone();
        Box::pin(async move {
            if let Some(connection) = connections.borrow_mut().remove(&port) {
                connection.close();
                Ok(())
            } else {
                Err(TransportError::PortNotConnected(port))
            }
        })
    }

    pub fn send(&self, port: PortId, bytes: Vec<u8>) -> TransportFuture<()> {
        let connections = self.connections.clone();
        Box::pin(async move {
            let (socket, next_id) = {
                let connections = connections.borrow();
                let connection = connections
                    .get(&port)
                    .ok_or_else(|| TransportError::PortNotConnected(port.clone()))?;
                (connection.socket.clone(), connection.next_id.clone())
            };
            if socket.ready_state() != WebSocket::OPEN {
                return Err(TransportError::PortNotConnected(port));
            }
            let script = String::from_utf8_lossy(&bytes);
            if script.trim().is_empty() {
                return Ok(());
            }
            let id = next_id.get();
            next_id.set(id.saturating_add(1));
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "printer.gcode.script",
                "params": { "script": script.as_ref() },
                "id": id,
            });
            socket
                .send_with_str(&request.to_string())
                .map_err(|error| TransportError::Operation(js_error(&error)))?;
            Ok(())
        })
    }
}

fn websocket_url(config: &NetworkSerialConfig) -> String {
    let secure = web_sys::window()
        .and_then(|window| window.location().protocol().ok())
        .is_some_and(|protocol| protocol == "https:");
    let scheme = if secure { "wss" } else { "ws" };
    format!("{scheme}://{}:{}/websocket", config.host, config.port)
}

fn parse_gcode_response(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("method").and_then(serde_json::Value::as_str) != Some("notify_gcode_response") {
        return None;
    }
    value
        .get("params")?
        .as_array()?
        .first()?
        .as_str()
        .map(str::to_owned)
}

fn js_error(value: &JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}

#[cfg(test)]
mod tests {
    use super::parse_gcode_response;

    #[test]
    fn parses_only_gcode_notifications() {
        assert_eq!(
            parse_gcode_response(
                r#"{"jsonrpc":"2.0","method":"notify_gcode_response","params":["ok"]}"#
            )
            .as_deref(),
            Some("ok")
        );
        assert_eq!(
            parse_gcode_response(
                r#"{"jsonrpc":"2.0","method":"notify_status_update","params":[]}"#
            ),
            None
        );
    }
}
