//! A small threaded HTTP server: one thread per connection, one request per
//! connection.
//!
//! Its job is to be a real upstream on loopback. `AGENTS.md` allows recorded
//! external bytes to stand in for an uncontrolled network but bans a test-only
//! implementation standing in for our own code, so a test that wants a
//! provider's recorded response serves those exact bytes from here and lets the
//! real client, framing and parser run.

use anyhow::{Context, Result};
use std::io::BufReader;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::deadline::Deadline;
use crate::message::{Request, Response, read_request, write_response};

/// How long one connection gets to deliver its request, and then how long it
/// gets to receive the answer. It is a budget for the whole read, not a gap
/// between bytes, so a client that connects and says nothing and a client that
/// drips a byte at a time are bounded the same way: at the deadline the
/// connection is answered `408` and closed, and its handler thread returned.
pub const SERVER_TIMEOUT: Duration = Duration::from_secs(30);

/// Ceiling on concurrently served connections. Past it the accept loop waits
/// for a handler to finish rather than spawning without bound; waiting
/// connections sit in the kernel's listen backlog.
pub const MAX_CONCURRENT_CONNECTIONS: usize = 64;

pub struct Server {
    listener: TcpListener,
    timeout: Duration,
}

pub struct ServerHandle {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Server {
    /// Bind an address. Loopback is the expected home (`127.0.0.1:0` in tests,
    /// the configured console port in the binary), but `--listen` can move it,
    /// so the serving loop is written for a listener that unfriendly clients
    /// can reach: socket timeouts, a connection cap, and an accept loop that
    /// survives both accept and thread-creation failure.
    pub fn bind(addr: &str) -> Result<Self> {
        let listener = TcpListener::bind(addr).with_context(|| format!("bind {addr}"))?;
        Ok(Self {
            listener,
            timeout: SERVER_TIMEOUT,
        })
    }

    /// Shorten the per-connection deadline. A test that wants to watch a
    /// connection hit it should not wait [`SERVER_TIMEOUT`] to do so.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.listener.local_addr().context("local addr")
    }

    /// Serve on a background thread. The handler runs on a per-connection
    /// thread; a handler panic tears down that connection only. At most
    /// [`MAX_CONCURRENT_CONNECTIONS`] handlers run at once, and every
    /// connection carries one deadline, so no client — silent, slow, or
    /// merely unlucky — holds a thread past it.
    pub fn spawn<H>(self, handler: H) -> Result<ServerHandle>
    where
        H: Fn(Request) -> Response + Send + Sync + 'static,
    {
        let addr = self.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_accept = Arc::clone(&stop);
        let handler = Arc::new(handler);
        let listener = self.listener;
        let timeout = self.timeout;
        let slots = Arc::new(Slots::default());
        let join = std::thread::Builder::new()
            .name("commonmeasure-http-accept".to_owned())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stop_accept.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else {
                        // A persistently failing accept (fd exhaustion, say)
                        // must not hot-spin the loop.
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    };
                    let Some(slot) = slots.acquire(&stop_accept) else {
                        break;
                    };
                    let handler = Arc::clone(&handler);
                    // Builder::spawn reports thread-creation failure instead
                    // of panicking the accept loop. On failure the closure is
                    // dropped unrun, which drops the connection and frees its
                    // slot; the server keeps accepting.
                    let _ = std::thread::Builder::new()
                        .name("commonmeasure-http-connection".to_owned())
                        .spawn(move || {
                            let _slot = slot;
                            handle_connection(stream, handler.as_ref(), timeout);
                        });
                }
            })
            .context("spawn accept thread")?;
        Ok(ServerHandle {
            addr,
            stop,
            join: Some(join),
        })
    }
}

/// The connection cap: a count behind a mutex so the accept loop can wait for
/// a slot, and a condvar so a finishing handler wakes it.
#[derive(Default)]
struct Slots {
    active: Mutex<usize>,
    freed: Condvar,
}

impl Slots {
    /// Block until a slot is free, checking `stop` while waiting so shutdown
    /// is never stuck behind a full server. `None` means stop was requested.
    fn acquire(self: &Arc<Self>, stop: &AtomicBool) -> Option<SlotGuard> {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        while *active >= MAX_CONCURRENT_CONNECTIONS {
            if stop.load(Ordering::SeqCst) {
                return None;
            }
            let (reacquired, _) = self
                .freed
                .wait_timeout(active, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner());
            active = reacquired;
        }
        *active += 1;
        Some(SlotGuard(Arc::clone(self)))
    }
}

/// Releases its slot on drop, which covers a panicking handler too.
struct SlotGuard(Arc<Slots>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let mut active = self.0.active.lock().unwrap_or_else(|e| e.into_inner());
        *active = active.saturating_sub(1);
        self.0.freed.notify_one();
    }
}

fn handle_connection<H>(stream: TcpStream, handler: &H, timeout: Duration)
where
    H: Fn(Request) -> Response,
{
    let mut reader = BufReader::new(Deadline::new(stream, timeout));
    let response = match read_request(&mut reader) {
        Ok(request) => handler(request),
        Err(_) if reader.get_ref().expired() => Response::text(408, "request timed out"),
        Err(_) => Response::text(400, "malformed request"),
    };
    reader.get_mut().renew(timeout);
    let _ = write_response(reader.get_mut(), &response);
}

impl ServerHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Block until the server stops, which for a server nobody stops is
    /// forever: the way a foreground `serve` command waits for Ctrl-C.
    pub fn wait(mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    /// Stop accepting. In-flight connections finish on their own threads.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock the accept loop with a throwaway connection.
        let _ = TcpStream::connect(self.addr);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}
