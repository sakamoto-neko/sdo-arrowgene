use crate::codec::Packet;
use anyhow::{Result, ensure};
use std::{
    collections::VecDeque,
    io::{ErrorKind, Read, Write},
    net::TcpStream,
    sync::Arc,
};
const MAX_FRAME: usize = 3 * 1024 * 1024;
const MAX_BUFFER: usize = 6 * 1024 * 1024;
pub(crate) struct Wire {
    stream: mio::net::TcpStream,
    input: Vec<u8>,
    output: VecDeque<Frame>,
    queued: usize,
    read_budget_used: bool,
}
struct Frame {
    header: [u8; 4],
    bytes: Arc<Vec<u8>>,
    written: usize,
}
impl Wire {
    pub fn new(stream: TcpStream) -> Result<Self> {
        stream.set_nonblocking(true)?;
        Self::from_socket(mio::net::TcpStream::from_std(stream))
    }
    fn from_socket(stream: mio::net::TcpStream) -> Result<Self> {
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            input: Vec::new(),
            output: VecDeque::new(),
            queued: 0,
            read_budget_used: false,
        })
    }
    pub fn register(&mut self, poll: &mio::Poll, token: usize) -> Result<()> {
        poll.registry().register(
            &mut self.stream,
            mio::Token(token),
            mio::Interest::READABLE | mio::Interest::WRITABLE,
        )?;
        Ok(())
    }
    pub fn send(&mut self, value: &impl Packet) -> Result<()> {
        self.send_encoded(Arc::new(value.encode()?))
    }
    /// Shared immutable frames avoid copying pending TCP data.
    pub fn send_encoded(&mut self, bytes: Arc<Vec<u8>>) -> Result<()> {
        ensure!(
            bytes.len() <= MAX_FRAME && self.queued + bytes.len() + 4 <= MAX_BUFFER,
            "Network send queue exceeded"
        );
        self.queued += bytes.len() + 4;
        self.output.push_back(Frame {
            header: (bytes.len() as u32).to_be_bytes(),
            bytes,
            written: 0,
        });
        Ok(())
    }
    pub fn poll<T: Packet>(&mut self) -> Result<Vec<T>> {
        let mut buffer = [0; 16384];
        self.read_budget_used = false;
        for i in 0..64 {
            match self.stream.read(&mut buffer) {
                Ok(0) => anyhow::bail!("Connection closed"),
                Ok(n) => {
                    self.read_budget_used = i == 63;
                    self.input.extend(&buffer[..n]);
                    ensure!(
                        self.input.len() <= MAX_BUFFER,
                        "Network receive queue exceeded"
                    );
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }
        let mut events = Vec::new();
        let mut consumed = 0;
        while events.len() < 128 && self.input.len() - consumed >= 4 {
            let len =
                u32::from_be_bytes(self.input[consumed..consumed + 4].try_into().unwrap()) as usize;
            ensure!(len <= MAX_FRAME, "Network frame exceeds limit");
            if self.input.len() - consumed < 4 + len {
                break;
            }
            events.push(T::decode(&self.input[consumed + 4..consumed + 4 + len])?);
            consumed += 4 + len;
        }
        self.input.drain(..consumed);
        self.flush()?;
        Ok(events)
    }
    pub fn read_again(&self) -> bool {
        self.read_budget_used
    }
    pub fn flush(&mut self) -> Result<()> {
        while let Some(frame) = self.output.front_mut() {
            let written = if frame.written < 4 {
                self.stream.write_vectored(&[
                    std::io::IoSlice::new(&frame.header[frame.written..]),
                    std::io::IoSlice::new(&frame.bytes),
                ])
            } else {
                self.stream.write(&frame.bytes[frame.written - 4..])
            };
            match written {
                Ok(0) => anyhow::bail!("Connection closed"),
                Ok(n) => {
                    frame.written += n;
                    self.queued -= n;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
            if frame.written == frame.bytes.len() + 4 {
                self.output.pop_front();
            }
        }
        Ok(())
    }
}

pub struct ServerListener {
    socket: mio::net::TcpListener,
    poll: mio::Poll,
    events: mio::Events,
}
impl ServerListener {
    pub fn bind(address: &str) -> Result<Self> {
        let socket = std::net::TcpListener::bind(address)?;
        socket.set_nonblocking(true)?;
        let mut socket = mio::net::TcpListener::from_std(socket);
        let poll = mio::Poll::new()?;
        poll.registry()
            .register(&mut socket, mio::Token(0), mio::Interest::READABLE)?;
        Ok(Self {
            socket,
            poll,
            events: mio::Events::with_capacity(64),
        })
    }
    pub fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.socket.local_addr()
    }
    pub fn accept(&self, token: usize) -> Result<Option<ServerConnection>> {
        let socket = match self.socket.accept() {
            Ok((socket, _)) => socket,
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut wire = Wire::from_socket(socket)?;
        wire.register(&self.poll, token)?;
        Ok(Some(ServerConnection {
            wire,
            movement: Default::default(),
        }))
    }
    pub fn wait(&mut self, timeout: std::time::Duration) -> std::io::Result<()> {
        self.poll.poll(&mut self.events, Some(timeout))
    }
}
/// Typed server side of a connected protocol stream.
pub struct ServerConnection {
    wire: Wire,
    movement: std::collections::BTreeMap<crate::PlayerId, (crate::RoomId, u64, crate::Walker)>,
}
impl ServerConnection {
    pub fn poll(&mut self) -> Result<Vec<crate::Command>> {
        self.wire.poll()
    }
    pub fn send(&mut self, event: &crate::Event) -> Result<()> {
        match event {
            crate::Event::Movement {
                room,
                state_us,
                walkers,
            } => {
                for &walker in walkers {
                    self.movement.insert(walker.id, (*room, *state_us, walker));
                }
                Ok(())
            }
            crate::Event::RoomState(_) => {
                self.movement.clear();
                self.wire.send(event)
            }
            _ => self.wire.send(event),
        }
    }
    pub fn flush(&mut self) -> Result<()> {
        self.wire.flush()?;
        if self.wire.output.is_empty() && !self.movement.is_empty() {
            let mut ticks = std::collections::BTreeMap::<_, Vec<_>>::new();
            for (_, (room, at, walker)) in std::mem::take(&mut self.movement) {
                ticks.entry((room, at)).or_default().push(walker);
            }
            for ((room, state_us), walkers) in ticks {
                self.wire.send(&crate::Event::Movement {
                    room,
                    state_us,
                    walkers,
                })?;
            }
            self.wire.flush()?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    fn pair() -> (TcpStream, Wire) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        (socket, Wire::new(stream).unwrap())
    }
    #[test]
    fn fragmented_frames_reassemble_and_oversized_frames_are_rejected() -> Result<()> {
        let (mut socket, mut wire) = pair();
        let message = serde_json::to_vec(&crate::Command::Start)?;
        let len = (message.len() as u32).to_be_bytes();
        socket.write_all(&len[..2])?;
        assert!(wire.poll::<crate::Command>()?.is_empty());
        socket.write_all(&len[2..])?;
        socket.write_all(&message[..3])?;
        assert!(wire.poll::<crate::Command>()?.is_empty());
        socket.write_all(&message[3..])?;
        let began = std::time::Instant::now();
        let events = loop {
            let events = wire.poll::<crate::Command>()?;
            if !events.is_empty() {
                break events;
            }
            ensure!(
                began.elapsed() < std::time::Duration::from_secs(1),
                "Read timed out"
            );
            std::thread::yield_now();
        };
        assert!(matches!(events[0], crate::Command::Start));
        socket.write_all(&u32::MAX.to_be_bytes())?;
        let began = std::time::Instant::now();
        loop {
            if wire.poll::<crate::Command>().is_err() {
                break;
            }
            ensure!(
                began.elapsed() < std::time::Duration::from_secs(1),
                "Oversized frame was not rejected"
            );
            std::thread::yield_now();
        }
        Ok(())
    }
}
