//! Typed multiplayer client with a target-selected transport.
use anyhow::{Context, Result, ensure};
use sdo_net::{Command, Event};
use std::{collections::VecDeque, time::Duration};
use web_time::Instant;

#[cfg_attr(target_arch = "wasm32", path = "net/websocket.rs")]
#[cfg_attr(not(target_arch = "wasm32"), path = "net/tcp.rs")]
mod platform;
#[cfg(not(target_arch = "wasm32"))]
mod transport;
#[cfg(not(target_arch = "wasm32"))]
pub use transport::{ServerConnection, ServerListener};

const MAX_BUFFER: usize = 6 * 1024 * 1024;

const PING_INTERVAL: Duration = Duration::from_millis(300);
const TIMEOUT: Duration = Duration::from_secs(10);

// Backends timestamp events on arrival, before the game loop polls them.
trait ClientTransport: Sized {
    fn connect(
        address: String,
        name: String,
        epoch: Instant,
        wake: impl Fn() + Send + 'static,
    ) -> Result<Self>;
    fn send(&self, command: Command) -> Result<()>;
    fn drain(&mut self) -> Vec<(ClientEvent, u64)>;
}

#[derive(Clone, Debug)]
pub enum ClientEvent {
    Message(Event),
    Disconnected(String),
}
/// Minimum-RTT sample estimates monotonic server time without wall-clock dependence.
#[derive(Default, Clone, Debug)]
pub struct Clock {
    samples: VecDeque<(u64, i128)>,
}
impl Clock {
    pub fn observe(&mut self, sent: u64, received: u64, server: u64) {
        if received < sent {
            return;
        }
        self.samples.push_back((
            received - sent,
            server as i128 - (sent as i128 + received as i128) / 2,
        ));
        if self.samples.len() > 32 {
            self.samples.pop_front();
        }
    }
    pub fn rtt_us(&self) -> Option<u64> {
        self.samples.iter().map(|s| s.0).min()
    }
    pub fn synchronized(&self) -> bool {
        self.samples.len() >= 3 && self.rtt_us().is_some_and(|r| r < 500_000)
    }
    pub fn server_us(&self, local: u64) -> Option<u64> {
        self.samples
            .iter()
            .min_by_key(|s| s.0)
            .map(|s| (local as i128 + s.1).clamp(0, u64::MAX as i128) as u64)
    }
}
pub struct Client {
    transport: platform::Backend,
    epoch: Instant,
    pub clock: Clock,
}

impl Client {
    pub fn connect(address: String, name: String) -> Result<Self> {
        Self::connect_with_wake(address, name, || {})
    }

    pub fn connect_with_wake(
        address: String,
        name: String,
        wake: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let epoch = Instant::now();
        Ok(Self {
            transport: platform::Backend::connect(address, name, epoch, wake)?,
            epoch,
            clock: Clock::default(),
        })
    }

    pub fn send(&self, command: Command) -> Result<()> {
        self.transport.send(command)
    }
    pub fn poll(&mut self) -> Vec<ClientEvent> {
        let mut result = Vec::new();
        for (event, received) in self.transport.drain() {
            if let ClientEvent::Message(Event::Pong { sent_us, server_us }) = &event {
                self.clock.observe(*sent_us, received, *server_us);
            }
            result.push(event);
        }
        result
    }
    pub fn local_us(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }
    pub fn server_us(&self) -> Option<u64> {
        self.clock.server_us(self.local_us())
    }
    pub fn start_delay(&self, server_us: u64) -> Result<Duration> {
        ensure!(self.clock.synchronized(), "Clock is not synchronized");
        let server_now = self.server_us().context("No clock estimate")?;
        ensure!(
            server_us > server_now + 100_000,
            "Start countdown arrived too late"
        );
        Ok(Duration::from_micros(server_us - server_now))
    }
    pub fn deadline(&self, server_us: u64) -> Result<Instant> {
        Ok(Instant::now() + self.start_delay(server_us)?)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_prefers_low_latency_and_ignores_wall_time() {
        let mut c = Clock::default();
        c.observe(100, 120, 1010);
        c.observe(200, 400, 1200);
        c.observe(300, 320, 1210);
        assert!(c.synchronized());
        assert_eq!(c.rtt_us(), Some(20));
        assert_eq!(c.server_us(500), Some(1400));
    }
}
