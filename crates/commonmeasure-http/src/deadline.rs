//! One deadline for a whole exchange, rather than one per read.
//!
//! `set_read_timeout` bounds a single syscall, so a peer that sends one byte
//! just inside the timeout renews it forever: the socket never goes quiet long
//! enough to trip, and the exchange runs for as long as the peer cares to drip.
//! Wrapping the socket lets every read and write arm it with what is left of a
//! single budget instead, so the exchange ends when the budget does.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub(crate) struct Deadline {
    socket: TcpStream,
    expires: Instant,
}

impl Deadline {
    pub(crate) fn new(socket: TcpStream, budget: Duration) -> Self {
        Self {
            socket,
            expires: Instant::now() + budget,
        }
    }

    /// Start a fresh budget. The server answers on the socket it read from, so
    /// a request that consumed the whole budget still has to be told so.
    pub(crate) fn renew(&mut self, budget: Duration) {
        self.expires = Instant::now() + budget;
    }

    pub(crate) fn expired(&self) -> bool {
        self.remaining().is_none()
    }

    fn remaining(&self) -> Option<Duration> {
        let left = self.expires.saturating_duration_since(Instant::now());
        (!left.is_zero()).then_some(left)
    }
}

fn exhausted() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "connection deadline reached")
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.remaining().ok_or_else(exhausted)?;
        self.socket.set_read_timeout(Some(left))?;
        self.socket.read(buf)
    }
}

impl Write for Deadline {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let left = self.remaining().ok_or_else(exhausted)?;
        self.socket.set_write_timeout(Some(left))?;
        self.socket.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}
