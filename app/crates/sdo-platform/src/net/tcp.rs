use super::transport::Wire;
use super::{ClientEvent, ClientTransport, PING_INTERVAL, TIMEOUT};
use anyhow::{Context, Result, ensure};
use sdo_net::{Command, Event, PROTOCOL};
use std::{
    net::{TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
pub(super) struct Backend {
    commands: SyncSender<Command>,
    events: Receiver<(ClientEvent, u64)>,
    waker: Arc<mio::Waker>,
    stop: Arc<AtomicBool>,
}
impl ClientTransport for Backend {
    fn connect(
        address: String,
        name: String,
        epoch: Instant,
        wake: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        ensure!(address.len() <= 255, "Server address is too long");
        let (commands, rx) = mpsc::sync_channel::<Command>(512);
        let (tx, events) = mpsc::sync_channel::<(ClientEvent, u64)>(512);
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
                        if ping.elapsed() >= PING_INTERVAL {
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
                        ensure!(last.elapsed() < TIMEOUT, "Lobby server timed out");
                        if count > 0 {
                            wake();
                        }
                        if drained && count < 128 && !wire.read_again() {
                            let wait = PING_INTERVAL.saturating_sub(ping.elapsed());
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
            stop,
        })
    }
    fn send(&self, command: Command) -> Result<()> {
        self.commands
            .try_send(command)
            .context("Lobby connection is unavailable or overloaded")?;
        self.waker.wake()?;
        Ok(())
    }
    fn drain(&mut self) -> Vec<(ClientEvent, u64)> {
        self.events.try_iter().collect()
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.waker.wake();
    }
}
