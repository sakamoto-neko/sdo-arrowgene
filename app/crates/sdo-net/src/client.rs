use crate::{Command, Event, PROTOCOL, transport::Wire};
use anyhow::{Context, Result, ensure};
use std::{
    collections::VecDeque,
    net::{TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
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
    commands: SyncSender<Command>,
    events: Receiver<(ClientEvent, u64)>,
    waker: Arc<mio::Waker>,
    epoch: Instant,
    stop: Arc<AtomicBool>,
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
        ensure!(address.len() <= 255, "Server address is too long");
        let (commands, rx) = mpsc::sync_channel::<Command>(512);
        let (tx, events) = mpsc::sync_channel::<(ClientEvent, u64)>(512);
        let epoch = Instant::now();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let mut poll = mio::Poll::new()?;
        let waker = Arc::new(mio::Waker::new(poll.registry(), mio::Token(0))?);
        thread::Builder::new()
            .name("lobby-client".into())
            .spawn(move || {
                let result = (|| -> Result<()> {
                    let addresses = address
                        .to_socket_addrs()
                        .context("Cannot resolve lobby address")?
                        .take(8)
                        .collect::<Vec<_>>();
                    let mut connected = None;
                    for address in addresses {
                        if stopping.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        if let Ok(stream) =
                            TcpStream::connect_timeout(&address, Duration::from_secs(2))
                        {
                            connected = Some(stream);
                            break;
                        }
                    }
                    let mut wire = Wire::new(connected.context("Cannot connect to lobby server")?)?;
                    wire.register(&poll, 1)?;
                    let mut readiness = mio::Events::with_capacity(4);
                    wire.send(&Command::Hello {
                        protocol: PROTOCOL,
                        name,
                    })?;
                    let mut ping = Instant::now() - Duration::from_secs(1);
                    let mut last = Instant::now();
                    while !stopping.load(Ordering::Acquire) {
                        let mut drained = false;
                        for _ in 0..128 {
                            match rx.try_recv() {
                                Ok(command) => wire.send(&command)?,
                                Err(mpsc::TryRecvError::Empty) => {
                                    drained = true;
                                    break;
                                }
                                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                            }
                        }
                        if ping.elapsed() >= Duration::from_millis(300) {
                            ping = Instant::now();
                            wire.send(&Command::Ping {
                                sent_us: epoch.elapsed().as_micros() as u64,
                            })?;
                        }
                        let messages = wire.poll::<Event>()?;
                        let count = messages.len();
                        for message in messages {
                            last = Instant::now();
                            tx.try_send((
                                ClientEvent::Message(message),
                                epoch.elapsed().as_micros() as u64,
                            ))
                            .context("Lobby event queue overflow")?;
                        }
                        ensure!(
                            last.elapsed() < Duration::from_secs(10),
                            "Lobby server timed out"
                        );
                        if count > 0 {
                            wake();
                        }
                        if drained && count < 128 && !wire.read_again() {
                            let wait = Duration::from_millis(300).saturating_sub(ping.elapsed());
                            if let Err(e) = poll.poll(&mut readiness, Some(wait))
                                && e.kind() != std::io::ErrorKind::Interrupted
                            {
                                return Err(e.into());
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    let _ = tx.try_send((
                        ClientEvent::Disconnected(error.to_string()),
                        epoch.elapsed().as_micros() as u64,
                    ));
                    wake();
                }
            })?;
        Ok(Self {
            waker,
            commands,
            events,
            epoch,
            stop,
            clock: Clock::default(),
        })
    }
    pub fn send(&self, command: Command) -> Result<()> {
        self.commands
            .try_send(command)
            .context("Lobby connection is unavailable or overloaded")?;
        self.waker.wake()?;
        Ok(())
    }
    pub fn poll(&mut self) -> Vec<ClientEvent> {
        let mut result = Vec::new();
        while let Ok((event, received)) = self.events.try_recv() {
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
    pub fn deadline(&self, server_us: u64) -> Result<Instant> {
        ensure!(self.clock.synchronized(), "Clock is not synchronized");
        let now = Instant::now();
        let server_now = self.server_us().context("No clock estimate")?;
        ensure!(
            server_us > server_now + 100_000,
            "Start countdown arrived too late"
        );
        Ok(now + Duration::from_micros(server_us - server_now))
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.waker.wake();
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
