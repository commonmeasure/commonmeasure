//! Client and server exercised against each other over real sockets.

use commonmeasure_http::{
    MAX_CONCURRENT_CONNECTIONS, Request, Response, Server, send, send_with_timeout,
};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[test]
fn get_roundtrip_preserves_headers_and_body() {
    let server = Server::bind("127.0.0.1:0").expect("bind");
    let handle = server
        .spawn(|req| {
            assert_eq!(req.method, "GET");
            assert_eq!(req.target, "/article?id=7");
            let seen = req.headers.get("X-Probe").map(str::to_string);
            let mut resp = Response::text(200, "hello world");
            if let Some(v) = seen {
                resp.headers.set("X-Probe-Echo", &v);
            }
            resp
        })
        .expect("spawn");

    let mut req = Request::get("/");
    req.headers.set("X-Probe", "42");
    let resp = send(&format!("{}/article?id=7", handle.url()), req).expect("send");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"hello world");
    assert_eq!(resp.headers.get("X-Probe-Echo"), Some("42"));
}

#[test]
fn post_roundtrip_carries_body_both_ways() {
    let server = Server::bind("127.0.0.1:0").expect("bind");
    let handle = server
        .spawn(|req| {
            assert_eq!(req.method, "POST");
            assert_eq!(req.headers.get("Content-Type"), Some("application/json"));
            let mut echoed = b"got: ".to_vec();
            echoed.extend_from_slice(&req.body);
            Response::new(200, echoed)
        })
        .expect("spawn");

    let req = Request::post("/", br#"{"k":1}"#.to_vec(), "application/json");
    let resp = send(&handle.url(), req).expect("send");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, br#"got: {"k":1}"#);
}

/// A provider's error body is evidence. Statuses outside 2xx must arrive intact
/// rather than being turned into a transport error that loses what was said.
#[test]
fn non_200_statuses_and_bodies_survive() {
    let server = Server::bind("127.0.0.1:0").expect("bind");
    let handle = server
        .spawn(|_| {
            let mut resp = Response::json(403, r#"{"title":"Forbidden"}"#);
            resp.headers.set("X-Detail", "no publisher agreement");
            resp
        })
        .expect("spawn");

    let resp = send(&handle.url(), Request::get("/")).expect("send");
    assert_eq!(resp.status, 403);
    assert_eq!(resp.headers.get("X-Detail"), Some("no publisher agreement"));
    assert_eq!(resp.body, br#"{"title":"Forbidden"}"#);
}

/// Short enough to watch a deadline pass without the suite waiting out
/// `SERVER_TIMEOUT`, long enough that a loopback exchange is not racing it.
const TEST_TIMEOUT: Duration = Duration::from_millis(400);

/// A client that connects and says nothing is answered and closed at its
/// deadline, which is what returns the handler thread.
#[test]
fn a_silent_client_is_closed_at_its_deadline() {
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .timeout(TEST_TIMEOUT)
        .spawn(|_| Response::text(200, "served"))
        .expect("spawn");

    let started = Instant::now();
    let mut silent = TcpStream::connect(handle.addr()).expect("connect");
    silent
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set read timeout");
    let mut seen = Vec::new();
    silent
        .read_to_end(&mut seen)
        .expect("server closes the socket");

    let text = String::from_utf8_lossy(&seen);
    assert!(text.starts_with("HTTP/1.1 408"), "got: {text:?}");
    assert!(
        started.elapsed() < TEST_TIMEOUT * 5,
        "the silent connection outlived its deadline by {:?}",
        started.elapsed()
    );
}

/// The deadline is a budget for the connection, not a gap between bytes. A
/// client dripping a well-formed request slower than the budget must be cut off
/// mid-request: per-read timeouts it renews forever, and while it does it holds
/// one of [`MAX_CONCURRENT_CONNECTIONS`] handler threads.
#[test]
fn a_drip_feeding_client_is_cut_off_at_its_deadline() {
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .timeout(TEST_TIMEOUT)
        .spawn(|_| Response::text(200, "served the drip"))
        .expect("spawn");

    let drip = TcpStream::connect(handle.addr()).expect("connect");
    let mut writer = drip.try_clone().expect("clone");
    // A byte every 50ms: never idle long enough to trip a 400ms per-read
    // timeout, and not finished before 1.4s.
    std::thread::spawn(move || {
        for byte in b"GET / HTTP/1.1\r\nHost: drip\r\n\r\n" {
            if writer.write_all(&[*byte]).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });

    let mut status = String::new();
    BufReader::new(drip)
        .read_line(&mut status)
        .expect("server answers");
    assert!(status.starts_with("HTTP/1.1 408"), "got: {status:?}");
}

/// The connection cap is a queue, not a refusal: with every slot held, the next
/// client waits for one to free and is then served normally.
///
/// Every slot is held by a handler that has started and is parked, so the cap
/// is full before the next request is sent however slowly the connections
/// were set up. The queued request must then not reach the handler until one
/// of them is released. A server that would serve it at once is given a
/// moment to do so; a loaded machine can only make that moment too short to
/// catch a lifted cap, never fail a server that keeps it.
#[test]
fn a_connection_past_the_cap_is_queued_not_refused() {
    let events = Arc::new((Mutex::new(Vec::<&str>::new()), Condvar::new()));
    let gate = Arc::new((Mutex::new(0usize), Condvar::new()));
    let seen = Arc::clone(&events);
    let held_until = Arc::clone(&gate);
    let handle = Server::bind("127.0.0.1:0")
        .expect("bind")
        .spawn(move |request| {
            let (log, changed) = &*seen;
            if request.target != "/held" {
                log.lock().unwrap().push("queued");
                changed.notify_all();
                return Response::text(200, "served");
            }
            log.lock().unwrap().push("held");
            changed.notify_all();
            let (released, opened) = &*held_until;
            let mut released = opened
                .wait_while(released.lock().unwrap(), |released| *released == 0)
                .unwrap();
            *released -= 1;
            Response::text(200, "held")
        })
        .expect("spawn");

    let held: Vec<_> = (0..MAX_CONCURRENT_CONNECTIONS)
        .map(|_| {
            let url = format!("{}/held", handle.url());
            std::thread::spawn(move || {
                send_with_timeout(&url, Request::get("/"), Duration::from_secs(60))
                    .expect("a held request is answered once released")
            })
        })
        .collect();
    let (log, changed) = &*events;
    let (full, timed_out) = changed
        .wait_timeout_while(log.lock().unwrap(), Duration::from_secs(60), |log| {
            log.len() < MAX_CONCURRENT_CONNECTIONS
        })
        .unwrap();
    assert!(
        !timed_out.timed_out(),
        "only {} slots were taken",
        full.len()
    );
    drop(full);

    let url = handle.url();
    let queued = std::thread::spawn(move || {
        send_with_timeout(&url, Request::get("/"), Duration::from_secs(60))
    });
    let (early, _) = changed
        .wait_timeout_while(log.lock().unwrap(), TEST_TIMEOUT, |log| {
            !log.contains(&"queued")
        })
        .unwrap();
    assert!(
        !early.contains(&"queued"),
        "a request past the cap was served with every slot held: the cap is not being enforced"
    );
    drop(early);

    let (released, opened) = &*gate;
    log.lock().unwrap().push("released");
    *released.lock().unwrap() += 1;
    opened.notify_all();
    let resp = queued
        .join()
        .unwrap()
        .expect("a queued request is served, not refused");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"served");
    let order = log.lock().unwrap().clone();
    let at = |event| order.iter().position(|seen| *seen == event).unwrap();
    assert!(
        at("released") < at("queued"),
        "the queued request was served before a slot was released: {order:?}"
    );

    *released.lock().unwrap() += MAX_CONCURRENT_CONNECTIONS;
    opened.notify_all();
    for held in held {
        assert_eq!(held.join().unwrap().status, 200);
    }
}

/// The client's budget covers the whole exchange. An origin dripping its
/// response header is the same attack as the drip-feeding client, and a `send`
/// that waits it out returns success long after the timeout it publishes.
#[test]
fn a_dripping_origin_fails_inside_the_client_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        let Ok((mut socket, _)) = listener.accept() else {
            return;
        };
        let mut request = [0u8; 1024];
        let _ = socket.read(&mut request);
        for byte in b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi" {
            if socket.write_all(&[*byte]).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });

    let error = send_with_timeout(&format!("http://{addr}/"), Request::get("/"), TEST_TIMEOUT)
        .expect_err("a dripping origin must not be waited out");
    assert!(
        format!("{error:#}").contains("did not answer within"),
        "{error:#}"
    );
}

#[test]
fn unsupported_schemes_are_refused_not_mangled() {
    let err = send("ftp://example.com/", Request::get("/")).expect_err("ftp must fail");
    assert!(err.to_string().contains("http and https only"));
}

/// The one test that proves the TLS client works against a real origin, and the
/// only test in this workspace that touches the network. It is ignored by
/// default so the offline gates stay green; run it with
/// `cargo test -p commonmeasure-http -- --ignored`.
#[test]
#[ignore = "requires network access"]
fn https_reaches_a_real_origin() {
    let resp = send("https://example.com/", Request::get("/")).expect("https send");
    assert_eq!(resp.status, 200);
    assert!(!resp.body.is_empty(), "a verified fetch returned no bytes");
}

/// The whole request head reaches the origin in one read.
///
/// A header written per syscall leaves a peer that reads once holding only
/// `GET /… HTTP/1.1` and deciding what to do with a request it has not seen.
/// This probe reads exactly once and must hold the complete head.
#[test]
fn the_request_head_arrives_in_one_read() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a probe origin");
    let address = listener.local_addr().expect("the probe's address");
    let probe = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut buffer = [0u8; 8192];
        let read = socket.read(&mut buffer).expect("one read");
        let seen = String::from_utf8_lossy(&buffer[..read]).into_owned();
        socket
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("answer");
        seen
    });

    let mut request = Request::get("/article?id=7");
    request.headers.set("X-First", "1");
    request.headers.set("X-Second", "2");
    request.headers.set("Accept", "application/json");
    send(&format!("http://{address}/article?id=7"), request).expect("send");

    let seen = probe.join().expect("the probe thread");
    assert!(
        seen.ends_with("\r\n\r\n"),
        "the head was not complete in one read: {seen:?}"
    );
    for line in [
        "GET /article?id=7 HTTP/1.1",
        "X-First: 1",
        "X-Second: 2",
        "Accept: application/json",
    ] {
        assert!(seen.contains(line), "{line} missing from {seen:?}");
    }
}

/// Setting a header twice changes its value, not where the message carries
/// it. Anything that reads a message by position — a capture a reviewer
/// compares, a signature base built over fields in a named order — sees the
/// same shape whether a value was set once or replaced.
#[test]
fn setting_a_header_again_keeps_its_position() {
    let mut request = Request::get("/");
    request.headers.set("A", "1");
    request.headers.set("B", "2");
    request.headers.set("C", "3");
    request.headers.set("b", "replaced");

    let names: Vec<&str> = request.headers.iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["A", "b", "C"]);
    assert_eq!(request.headers.get("B"), Some("replaced"));
}
