//! Each WebSocket message carries one unprefixed game packet.
use super::MAX_BUFFER;
use super::{ClientEvent, ClientTransport, PING_INTERVAL, TIMEOUT};
use anyhow::{Context, Result, anyhow, ensure};
use js_sys::{ArrayBuffer, Uint8Array};
use sdo_net::{
    Command, Event, PROTOCOL,
    codec::{MAX_FRAME, Packet},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{BinaryType, MessageEvent, WebSocket};
use web_time::Instant;

type Callback = Closure<dyn FnMut()>;

struct Connection {
    socket: WebSocket,
    epoch: Instant,
    last_received: Cell<Instant>,
    events: RefCell<Vec<(ClientEvent, u64)>>,
    opening: RefCell<Vec<Vec<u8>>>,
    closed: Cell<bool>,
    wake: Box<dyn Fn()>,
}

fn js_error(error: JsValue) -> anyhow::Error {
    anyhow!("Browser WebSocket: {error:?}")
}

impl Connection {
    fn stamp(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    fn fail(&self, reason: impl ToString) {
        if !self.closed.replace(true) {
            self.events
                .borrow_mut()
                .push((ClientEvent::Disconnected(reason.to_string()), self.stamp()));
            let _ = self.socket.close();
            (self.wake)();
        }
    }

    fn write(&self, packet: &[u8]) -> Result<()> {
        ensure!(
            self.socket.buffered_amount() as usize + packet.len() <= MAX_BUFFER,
            "Network send queue exceeded"
        );
        self.socket.send_with_u8_array(packet).map_err(js_error)
    }

    fn opened(&self, name: &str) -> Result<()> {
        self.write(
            &Command::Hello {
                protocol: PROTOCOL,
                name: name.into(),
            }
            .encode()?,
        )?;
        for packet in self.opening.take() {
            self.write(&packet)?;
        }
        self.ping()
    }

    fn ping(&self) -> Result<()> {
        ensure!(
            self.last_received.get().elapsed() < TIMEOUT,
            "Lobby server timed out"
        );
        if self.socket.ready_state() == WebSocket::OPEN {
            self.write(
                &Command::Ping {
                    sent_us: self.stamp(),
                }
                .encode()?,
            )?;
        }
        Ok(())
    }

    fn received(&self, message: MessageEvent) -> Result<()> {
        let at = self.stamp();
        let buffer = message
            .data()
            .dyn_into::<ArrayBuffer>()
            .map_err(|_| anyhow!("Expected binary lobby packet"))?;
        ensure!(
            buffer.byte_length() as usize <= MAX_FRAME,
            "Network frame exceeds limit"
        );
        ensure!(
            self.events.borrow().len() < 512,
            "Lobby event queue overflow"
        );
        let event = Event::decode(&Uint8Array::new(&buffer).to_vec())?;
        self.last_received.set(Instant::now());
        self.events
            .borrow_mut()
            .push((ClientEvent::Message(event), at));
        (self.wake)();
        Ok(())
    }
}

pub(super) struct Backend {
    connection: Rc<Connection>,
    timer: i32,
    // JS keeps references to these closures until Drop removes the listeners.
    _opened: Callback,
    _closed: Callback,
    _message: Closure<dyn FnMut(MessageEvent)>,
    _heartbeat: Callback,
}

impl ClientTransport for Backend {
    fn connect(
        address: String,
        name: String,
        epoch: Instant,
        wake: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let window = web_sys::window().context("Browser window unavailable")?;
        let socket = WebSocket::new(&address).map_err(js_error)?;
        socket.set_binary_type(BinaryType::Arraybuffer);
        let connection = Rc::new(Connection {
            socket,
            epoch,
            last_received: Cell::new(epoch),
            events: RefCell::default(),
            opening: RefCell::default(),
            closed: Cell::new(false),
            wake: Box::new(wake),
        });
        let state = connection.clone();
        let opened = Callback::new(move || {
            if let Err(error) = state.opened(&name) {
                state.fail(error);
            }
        });
        let state = connection.clone();
        let closed = Callback::new(move || state.fail("Lobby WebSocket closed"));
        let state = connection.clone();
        let message = Closure::new(move |event: MessageEvent| {
            if !state.closed.get()
                && let Err(error) = state.received(event)
            {
                state.fail(error);
            }
        });
        let state = connection.clone();
        let heartbeat = Callback::new(move || {
            if !state.closed.get()
                && let Err(error) = state.ping()
            {
                state.fail(error);
            }
        });
        let timer = window
            .set_interval_with_callback_and_timeout_and_arguments_0(
                heartbeat.as_ref().unchecked_ref(),
                PING_INTERVAL.as_millis() as i32,
            )
            .map_err(js_error)?;
        connection
            .socket
            .set_onopen(Some(opened.as_ref().unchecked_ref()));
        connection
            .socket
            .set_onclose(Some(closed.as_ref().unchecked_ref()));
        connection
            .socket
            .set_onmessage(Some(message.as_ref().unchecked_ref()));
        Ok(Self {
            connection,
            timer,
            _opened: opened,
            _closed: closed,
            _message: message,
            _heartbeat: heartbeat,
        })
    }

    fn send(&self, command: Command) -> Result<()> {
        let state = &self.connection;
        ensure!(!state.closed.get(), "Lobby connection closed");
        let packet = command.encode()?;
        ensure!(packet.len() <= MAX_FRAME, "Network frame exceeds limit");
        match state.socket.ready_state() {
            WebSocket::CONNECTING => {
                let mut opening = state.opening.borrow_mut();
                ensure!(
                    opening.len() < 512
                        && opening.iter().map(Vec::len).sum::<usize>() + packet.len() <= MAX_BUFFER,
                    "Network send queue exceeded"
                );
                opening.push(packet);
                Ok(())
            }
            WebSocket::OPEN => state.write(&packet),
            _ => anyhow::bail!("Lobby connection closed"),
        }
    }

    fn drain(&mut self) -> Vec<(ClientEvent, u64)> {
        self.connection.events.take()
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        if let Some(window) = web_sys::window() {
            window.clear_interval_with_handle(self.timer);
        }
        let socket = &self.connection.socket;
        socket.set_onclose(None);
        socket.set_onopen(None);
        socket.set_onmessage(None);
        let _ = socket.close();
    }
}
