//! The real transport boundary.
//!
//! Every byte Common Measure acquires from a content or skill supplier, and every
//! byte it exchanges with an inference gateway, crosses this crate. There is
//! deliberately no transport trait and no second implementation: a test that
//! wants recorded bytes starts a real [`Server`] on loopback and points the
//! caller's base URL at it, so the client, the framing and the parser under
//! test are the ones a live run uses.
//!
//! One request per connection (`Connection: close`) keeps the state machine
//! trivial. `https` is supported on the client side only, via `rustls` with a
//! vendored root set (see [`tls`]); the server does not terminate TLS.
//!
//! What owning the transport buys: the bytes an origin served are always
//! available as served, because the one content coding a response is decoded
//! from (gzip, which some origins send whatever the request accepts) keeps its
//! coded bytes beside the decoded ones ([`Response::coded`]); header order is
//! preserved for the message signatures a mediated crossing will need (RFC
//! 9421); and the whole byte path is auditable with the rest of the supply
//! chain. What it forgoes, deliberately: HTTP/2, every other content coding,
//! connection reuse and proxies. An origin that requires any of those fails
//! loudly rather than being quietly accommodated.

mod deadline;
mod message;
mod server;
mod tls;

pub use message::{
    CodedBody, Headers, Request, Response, read_request, read_response, write_request,
    write_response,
};
pub use server::{MAX_CONCURRENT_CONNECTIONS, SERVER_TIMEOUT, Server, ServerHandle};

use anyhow::{Context, Result, anyhow, bail};
use deadline::Deadline;
use std::io::{BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Budget for one client exchange: connect, request and response together, not
/// per read. Provider acquisition sits in an agent's hot path, and an origin
/// that answers a byte at a time is as costly there as one that never answers.
/// Name resolution is the host resolver's and is not covered.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

/// One client connection, plain or TLS. The TLS variant is boxed because a
/// `ClientConnection` carries kilobytes of session buffers, which an unboxed
/// enum would make every plain-HTTP request pay for.
enum Stream {
    Plain(Deadline),
    Tls(Box<tls::TlsStream>),
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// Connect within what is left of the exchange budget. `TcpStream::connect`
/// carries no timeout of its own, so an origin that accepts nothing would hang
/// here, before the deadline covering the rest of the exchange applied.
fn connect_by(addresses: &[SocketAddr], expires: Instant) -> Result<TcpStream> {
    let mut refusal = None;
    for addr in addresses {
        let left = expires.saturating_duration_since(Instant::now());
        if left.is_zero() {
            bail!("timed out before a connection was established");
        }
        match TcpStream::connect_timeout(addr, left) {
            Ok(tcp) => return Ok(tcp),
            Err(err) => refusal = Some(err),
        }
    }
    match refusal {
        Some(err) => Err(err.into()),
        None => bail!("no address to connect to"),
    }
}

/// Where a URL points, before anything is resolved or connected to.
struct Origin {
    secure: bool,
    host: String,
    port: u16,
    /// Origin-form request target: path and query.
    target: String,
    /// What the `Host` header must say.
    authority: String,
}

fn origin_of(url: &str) -> Result<Origin> {
    let parsed = url::Url::parse(url).with_context(|| format!("parse url {url}"))?;
    let secure = match parsed.scheme() {
        "http" => false,
        "https" => true,
        other => bail!("commonmeasure-http speaks http and https only, got {other}"),
    };
    let host = parsed.host_str().context("url has no host")?.to_owned();
    let default_port = if secure { 443 } else { 80 };
    let port = parsed.port().unwrap_or(default_port);
    let mut target = parsed.path().to_string();
    if let Some(query) = parsed.query() {
        target.push('?');
        target.push_str(query);
    }
    let authority = if port == default_port {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    Ok(Origin {
        secure,
        host,
        port,
        target,
        authority,
    })
}

/// Every address `url`'s host currently resolves to.
///
/// Separate from [`send_to`] so that a caller who must judge *where* a request
/// goes can do so before a socket is opened. A URL's spelling says nothing
/// about the address it reaches: a public name whose record points into a
/// private network is an ordinary DNS answer, and by the time [`send`] has
/// connected the judgement is too late to make. Handing the vetted addresses
/// straight to [`send_to`] also leaves no second lookup in between for a
/// different answer to arrive in.
pub fn resolve(url: &str) -> Result<Vec<SocketAddr>> {
    let origin = origin_of(url)?;
    let addresses: Vec<SocketAddr> = (origin.host.as_str(), origin.port)
        .to_socket_addrs()
        .with_context(|| format!("resolve {}", origin.host))?
        .collect();
    if addresses.is_empty() {
        bail!("{} resolved to no addresses", origin.host);
    }
    Ok(addresses)
}

/// A read or write that exhausted the exchange budget is a timeout, whatever
/// errno the socket raised on the way out of it.
///
/// A socket read timeout surfaces as `WouldBlock`, which reported verbatim
/// names neither the timeout nor how long the caller waited: the crossing
/// record then cannot tell a slow origin from a hiccup. Named here, once,
/// before the error leaves the crate, so no caller has to recognise an errno.
fn name_the_timeout(error: anyhow::Error, authority: &str, budget: Duration) -> anyhow::Error {
    let timed_out = error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|cause| {
            matches!(
                cause.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            )
        });
    if timed_out {
        anyhow!("{authority} did not answer within the {budget:?} exchange budget")
    } else {
        error
    }
}

/// Send `request` to the origin named by `url` in origin form, over `http` or
/// `https`.
///
/// The `Host` header is set from the URL; every other header the caller built
/// reaches the wire unchanged, which matters because provider authentication
/// travels in headers and must not be rewritten or logged on the way past.
pub fn send(url: &str, request: Request) -> Result<Response> {
    send_with_timeout(url, request, CLIENT_TIMEOUT)
}

/// [`send`] with an explicit budget, for callers that cannot wait
/// [`CLIENT_TIMEOUT`] to find out that an origin is not answering.
pub fn send_with_timeout(url: &str, request: Request, budget: Duration) -> Result<Response> {
    let addresses = resolve(url)?;
    send_to(url, &addresses, request, budget)
}

/// [`send_with_timeout`] to addresses the caller has already resolved and
/// approved, for a caller whose policy is about the address rather than the
/// name. Nothing here resolves the host again: the connection goes to one of
/// these addresses or to none.
pub fn send_to(
    url: &str,
    addresses: &[SocketAddr],
    request: Request,
    budget: Duration,
) -> Result<Response> {
    let origin = origin_of(url)?;
    let authority = origin.authority.clone();
    exchange(origin, addresses, request, budget)
        .map_err(|error| name_the_timeout(error, &authority, budget))
}

/// Connect, write and read, all inside one budget. Every way out of this is a
/// way out of the crate, which is why the budget is named on the error here
/// and nowhere else.
fn exchange(
    origin: Origin,
    addresses: &[SocketAddr],
    mut request: Request,
    budget: Duration,
) -> Result<Response> {
    request.target = origin.target;
    request.headers.set("Host", &origin.authority);
    request.headers.set("Connection", "close");

    let expires = Instant::now() + budget;
    let tcp =
        connect_by(addresses, expires).with_context(|| format!("connect {}", origin.authority))?;
    let tcp = Deadline::new(tcp, expires.saturating_duration_since(Instant::now()));

    let mut stream = if origin.secure {
        Stream::Tls(Box::new(tls::connect(&origin.host, tcp)?))
    } else {
        Stream::Plain(tcp)
    };

    // The TLS handshake runs here, on first write, so a certificate rejection
    // surfaces as a failure to write the request.
    write_request(&mut stream, &request).context("write request")?;
    stream.flush().context("flush request")?;
    let mut reader = BufReader::new(&mut stream);
    read_response(&mut reader).context("read response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// An origin that accepts the connection and then says nothing at all.
    /// Returns its URL and the listener, which must outlive the exchange.
    fn silent_origin() -> (TcpListener, String) {
        let listener = (47700..47799)
            .find_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
            .expect("a free port in 47700-47798");
        let url = format!("http://{}/quiet", listener.local_addr().expect("addr"));
        (listener, url)
    }

    /// What a crossing records has to be why the fetch failed. `WouldBlock` is
    /// the errno a socket read timeout raises, and it names neither the
    /// timeout nor the budget that ran out.
    #[test]
    fn a_silent_origin_is_reported_as_a_timeout_not_an_errno() {
        let (listener, url) = silent_origin();
        let accepting = std::thread::spawn(move || {
            let held = listener.accept();
            std::thread::sleep(Duration::from_millis(600));
            drop(held);
        });

        let error = send_with_timeout(&url, Request::get("/"), Duration::from_millis(200))
            .expect_err("a silent origin cannot answer");
        let reported = format!("{error:#}");
        assert!(
            reported.contains("did not answer within") && reported.contains("200ms"),
            "{reported}"
        );
        assert!(!reported.contains("os error"), "{reported}");
        accepting.join().expect("the origin thread");
    }

    /// A refusal still reads as a refusal: only a budget that ran out is
    /// renamed, or every transport failure would be reported as a timeout.
    #[test]
    fn a_refused_connection_still_names_the_refusal() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/x", listener.local_addr().expect("addr"));
        drop(listener);

        let error = send_with_timeout(&url, Request::get("/"), Duration::from_millis(500))
            .expect_err("nothing is listening");
        let reported = format!("{error:#}");
        assert!(reported.contains("onnection refused"), "{reported}");
    }

    /// The seam a caller with an address policy needs: resolution is a step of
    /// its own, so where a name points can be judged before a socket exists.
    /// `localhost.` is the shape of the problem in miniature — a spelling no
    /// private-host rule matches, pointing at the loopback interface.
    #[test]
    fn resolution_reports_the_addresses_a_name_actually_reaches() {
        let addresses = resolve("http://localhost.:47700/x").expect("localhost. resolves");
        assert!(!addresses.is_empty());
        assert!(
            addresses.iter().all(|address| address.ip().is_loopback()),
            "{addresses:?}"
        );
    }

    /// And the other half: what was vetted is what is connected to. The host
    /// here resolves to nothing at all, so a second lookup — the window a
    /// rebinding answer needs — would fail the exchange instead of serving it.
    #[test]
    fn a_vetted_address_is_the_one_connected_to() {
        let mut origin = (47700..47799)
            .find_map(|port| Server::bind(&format!("127.0.0.1:{port}")).ok())
            .expect("a free port in 47700-47798")
            .spawn(|_| Response::text(200, "served"))
            .expect("spawn");

        let response = send_to(
            "http://nowhere.invalid/x",
            &[origin.addr()],
            Request::get("/"),
            Duration::from_secs(5),
        )
        .expect("the address was given, so no name has to resolve");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"served");
        origin.stop();
    }
}
