//! The codec against bytes a hostile or broken peer would send, and against
//! header fields a hostile configuration would have it send. Every provider and
//! gateway call crosses this parser, and the MCP server's contract ("a mediator
//! that dies takes the agent's tools with it") means a malformed message must
//! come back as an `Err`, never a panic.

use commonmeasure_http::{
    Request, Response, read_request, read_response, write_request, write_response,
};

fn response(bytes: &[u8]) -> anyhow::Result<commonmeasure_http::Response> {
    read_response(&mut &bytes[..])
}

fn request(bytes: &[u8]) -> anyhow::Result<commonmeasure_http::Request> {
    read_request(&mut &bytes[..])
}

/// The 2 Aug gate's live probe: a chunk size chosen so `body.len() + size`
/// wraps past the ceiling check. Before the checked sum this panicked the
/// process — debug on the add, release on the slice behind it.
#[test]
fn a_chunk_size_that_overflows_the_running_total_is_an_error() {
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                4\r\nAAAA\r\nfffffffffffffffc\r\n";
    let error = response(raw).expect_err("an overflowing chunk must not decode");
    assert!(
        format!("{error:#}").contains("exceeds size ceiling"),
        "got: {error:#}"
    );
}

/// A chunk that does not overflow but exceeds the body ceiling outright is
/// refused before any allocation of its declared size.
#[test]
fn a_chunk_beyond_the_body_ceiling_is_an_error() {
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffff\r\n";
    let error = response(raw).expect_err("an oversized chunk must not decode");
    assert!(format!("{error:#}").contains("exceeds size ceiling"));
}

/// A declared Content-Length beyond the ceiling is refused up front, not
/// allocated and then read.
#[test]
fn a_content_length_beyond_the_ceiling_is_an_error() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 999999999999\r\n\r\n";
    assert!(response(raw).is_err());
}

/// A header block that never ends stops at the byte budget instead of
/// buffering without bound.
#[test]
fn an_unbounded_header_block_is_an_error() {
    let mut raw = b"HTTP/1.1 200 OK\r\n".to_vec();
    for n in 0..40_000 {
        raw.extend_from_slice(format!("X-{n}: y\r\n").as_bytes());
    }
    raw.extend_from_slice(b"\r\n");
    let error = response(&raw).expect_err("an oversized header block must not decode");
    assert!(format!("{error:#}").contains("header block too large"));
}

#[test]
fn a_header_line_without_a_colon_is_an_error() {
    let raw = b"HTTP/1.1 200 OK\r\nnot a header\r\n\r\n";
    let error = response(raw).expect_err("a malformed header must not decode");
    assert!(format!("{error:#}").contains("malformed header line"));
}

#[test]
fn malformed_request_lines_are_errors() {
    for raw in [
        &b"GET\r\n\r\n"[..],
        &b"GET /\r\n\r\n"[..],
        &b"GET / SPDY/3\r\n\r\n"[..],
        &b"\r\n\r\n"[..],
        // A request line is method SP target SP version and nothing else.
        // Whitespace-splitting swallows the extra words, and the peer that
        // sent them means something by them.
        &b"GET / HTTP/1.1 JUNK EXTRA\r\n\r\n"[..],
        &b"GET\t/\tHTTP/1.1\r\n\r\n"[..],
        &b"GET  / HTTP/1.1\r\n\r\n"[..],
        // There is no HTTP/1.9, and a peer claiming one is not describing
        // anything this transport agreed to speak.
        &b"GET / HTTP/1.9\r\n\r\n"[..],
    ] {
        assert!(request(raw).is_err(), "{raw:?} must not parse");
    }
}

/// The probe that motivated the check: the chunk that says it is four bytes
/// long is followed by `ZZ` instead of CRLF, and the two stray bytes eat the
/// start of the next size line. Unchecked, this reassembles as `AAAABBBB` —
/// a body no compliant parser on the same path would agree with.
#[test]
fn a_chunk_not_terminated_by_crlf_is_an_error() {
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                4\r\nAAAAZZ4\r\nBBBBZZ0\r\n\r\n";
    let error = response(raw).expect_err("a mis-terminated chunk must not decode");
    assert!(
        format!("{error:#}").contains("terminated by CRLF"),
        "got: {error:#}"
    );
}

/// Content-Length beside Transfer-Encoding is a request smuggling primitive:
/// this parser reading the chunked body and the next one reading the length
/// disagree about where the message ends (RFC 9112 §6.1).
#[test]
fn a_message_declaring_both_framings_is_an_error() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nTransfer-Encoding: chunked\r\n\r\n\
                4\r\nHARM\r\n0\r\n\r\nBENIGN123";
    let error = response(raw).expect_err("two framings must not decode");
    assert!(
        format!("{error:#}").contains("both Content-Length and Transfer-Encoding"),
        "got: {error:#}"
    );
}

/// Two lengths that disagree, likewise: taking the first leaves the second
/// message's bytes on the connection for whoever reads next.
#[test]
fn conflicting_content_lengths_are_an_error() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nContent-Length: 5\r\n\r\nSMUGG";
    let error = response(raw).expect_err("conflicting lengths must not decode");
    assert!(
        format!("{error:#}").contains("content-length"),
        "got: {error:#}"
    );
}

fn gzip(plain: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(plain).expect("compress");
    encoder.finish().expect("finish")
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn coded_response(coding: &str, body: &[u8]) -> Vec<u8> {
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Encoding: {coding}\r\n\
         Content-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    raw
}

/// Some origins send gzip whatever the request accepts. The body is decoded,
/// and the bytes the origin served are kept beside it: the retrieved hash of
/// the session-evidence contract is over those coded bytes and the content
/// hash over what the decoding yields, so the two differ and each is
/// recoverable from the response.
#[test]
fn a_gzip_body_decodes_and_keeps_the_bytes_served() {
    let plain = b"The cap is set quarterly, and this page was served gzipped.";
    let coded = gzip(plain);
    for coding in ["gzip", "GZIP", "x-gzip"] {
        let decoded = response(&coded_response(coding, &coded)).expect("gzip decodes");
        assert_eq!(decoded.body, plain, "{coding}");
        let kept = decoded.coded.as_ref().expect("the served bytes are kept");
        assert_eq!(kept.coding, "gzip");
        assert_eq!(kept.bytes, coded);
        assert_eq!(decoded.served_body(), coded.as_slice());
        let retrieved_hash = sha256(decoded.served_body());
        let content_hash = sha256(&decoded.body);
        assert_eq!(retrieved_hash, sha256(&coded));
        assert_eq!(content_hash, sha256(plain));
        assert_ne!(retrieved_hash, content_hash);
    }

    // Chunked framing is removed before the coding: the kept bytes are the
    // coded body, not the chunk framing around it.
    let mut chunked =
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for piece in coded.chunks(7) {
        chunked.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
        chunked.extend_from_slice(piece);
        chunked.extend_from_slice(b"\r\n");
    }
    chunked.extend_from_slice(b"0\r\n\r\n");
    let decoded = response(&chunked).expect("chunked gzip decodes");
    assert_eq!(decoded.body, plain);
    assert_eq!(decoded.served_body(), coded.as_slice());

    // Written back out, the response carries the bytes its Content-Encoding
    // describes.
    let mut wire = Vec::new();
    write_response(&mut wire, &decoded).expect("write");
    assert_eq!(response(&wire).expect("round trip").body, plain);

    // A body with no coding keeps nothing extra, and identity is no coding.
    let identity = b"HTTP/1.1 200 OK\r\nContent-Encoding: identity\r\nContent-Length: 2\r\n\r\nok";
    let plain_response = response(identity).expect("identity decodes");
    assert_eq!(plain_response.body, b"ok");
    assert!(plain_response.coded.is_none());
}

/// A small gzip body that expands past the body ceiling is refused at the
/// ceiling, not decoded into memory without bound.
#[test]
fn a_gzip_bomb_past_the_body_ceiling_is_refused() {
    let bomb = gzip(&vec![0u8; 33 * 1024 * 1024]);
    assert!(
        bomb.len() < 64 * 1024,
        "the coded body is small: {}",
        bomb.len()
    );
    let error = response(&coded_response("gzip", &bomb)).expect_err("a bomb must not decode");
    assert!(
        format!("{error:#}").contains("gzip body decodes past the size ceiling"),
        "got: {error:#}"
    );
}

/// A gzip body that does not decode completely is an error naming the cause,
/// never a shorter or empty body: a flipped checksum, a truncated stream and
/// bytes that are not gzip at all.
#[test]
fn a_corrupt_gzip_body_is_refused() {
    let coded = gzip(b"a page whose checksum will not match what it decodes to");
    let mut flipped = coded.clone();
    let crc = flipped.len() - 8;
    flipped[crc] ^= 0xff;
    let truncated = coded[..coded.len() / 2].to_vec();
    let not_gzip = b"\x1f\x8b but nothing after the magic".to_vec();
    for body in [flipped, truncated, not_gzip] {
        let error = response(&coded_response("gzip", &body)).expect_err("corrupt gzip decodes");
        assert!(
            format!("{error:#}").contains("gzip body could not be decoded"),
            "got: {error:#}"
        );
    }
}

/// Only gzip is decoded. Brotli, zstd and deflate are refused, as is more than
/// one coding in one field or across two, and a request body under any coding:
/// a body this parser did not decode would be hashed and recorded as the
/// content it only claims to be.
#[test]
fn every_other_content_coding_is_still_refused() {
    let coded = gzip(b"stacked");
    for coding in [
        "br",
        "zstd",
        "deflate",
        "compress",
        "gzip, gzip",
        "gzip, br",
    ] {
        let error = response(&coded_response(coding, &coded))
            .expect_err("a coding this parser does not decode must not pass");
        assert!(
            format!("{error:#}").contains("unsupported content encoding"),
            "{coding}: {error:#}"
        );
    }
    let mut two_fields =
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Encoding: gzip\r\n".to_vec();
    two_fields.extend_from_slice(format!("Content-Length: {}\r\n\r\n", coded.len()).as_bytes());
    two_fields.extend_from_slice(&coded);
    assert!(response(&two_fields).is_err(), "two coding fields decode");

    let mut coded_request = format!(
        "POST / HTTP/1.1\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
        coded.len()
    )
    .into_bytes();
    coded_request.extend_from_slice(&coded);
    let error = request(&coded_request).expect_err("a coded request body must not pass");
    assert!(format!("{error:#}").contains("unsupported content encoding"));
}

/// Chunk extensions share one budget with the rest of the framing. Bounded per
/// chunk, forty chunks buy forty times the header ceiling.
#[test]
fn chunk_extensions_are_bounded_in_aggregate() {
    let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for _ in 0..4 {
        raw.extend_from_slice(b"1;");
        raw.extend(std::iter::repeat_n(b'x', 30_000));
        raw.extend_from_slice(b"\r\nA\r\n");
    }
    raw.extend_from_slice(b"0\r\n\r\n");
    let error = response(&raw).expect_err("aggregate extensions must not decode");
    assert!(
        format!("{error:#}").contains("header block too large"),
        "got: {error:#}"
    );
}

/// Numbers a peer sends are decimal or hexadecimal digits and nothing else.
/// `+5` and a padded chunk size parse differently in different parsers, which
/// is the whole of the disagreement an attacker needs.
#[test]
fn decorated_lengths_and_chunk_sizes_are_errors() {
    for raw in [
        &b"HTTP/1.1 200 OK\r\nContent-Length: +5\r\n\r\nhello"[..],
        &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n+4\r\nAAAA\r\n0\r\n\r\n"[..],
        &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n  4  \r\nAAAA\r\n0\r\n\r\n"[..],
    ] {
        assert!(response(raw).is_err(), "{raw:?} must not parse");
    }
}

/// A field this parser accepts is one it will also send on. A name that is not
/// a token, or a value carrying control bytes, is a field a compliant peer
/// would refuse — so it must not be minted here.
#[test]
fn malformed_header_fields_are_errors_on_read() {
    for raw in [
        &b"HTTP/1.1 200 OK\r\n: v\r\n\r\n"[..],
        &b"HTTP/1.1 200 OK\r\nX-A : v\r\n\r\n"[..],
        &b"HTTP/1.1 200 OK\r\nX-A: v\0w\r\n\r\n"[..],
    ] {
        assert!(response(raw).is_err(), "{raw:?} must not parse");
    }
}

/// The probe that reached a credentialed origin: an API key read from operator
/// configuration carries a CRLF, and the interpolated header line becomes three
/// header lines at the far end.
#[test]
fn a_header_value_carrying_crlf_is_refused_on_write() {
    let mut request = Request::get("/");
    request
        .headers
        .set("X-API-Key", "secret\r\nX-Injected: yes\r\nX-Also: yes");
    let mut wire = Vec::new();
    let error =
        write_request(&mut wire, &request).expect_err("an injected header must not be sent");
    assert!(
        format!("{error:#}").contains("control byte"),
        "got: {error:#}"
    );
    assert!(
        wire.is_empty(),
        "a refused request still reached the wire: {:?}",
        String::from_utf8_lossy(&wire)
    );

    let mut response = Response::text(200, "body");
    response.headers.set("X-Detail", "line\r\nX-Injected: yes");
    let mut wire = Vec::new();
    assert!(write_response(&mut wire, &response).is_err());

    let mut response = Response::text(200, "body");
    response.headers.append("Bad Name", "v");
    let mut wire = Vec::new();
    assert!(write_response(&mut wire, &response).is_err());
}

#[test]
fn a_truncated_chunked_body_is_an_error_not_a_hang_or_panic() {
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nAA";
    assert!(response(raw).is_err());
}

/// The positive control: well-formed chunked framing still decodes, so the
/// refusals above are bounds, not a broken decoder.
#[test]
fn well_formed_chunked_bodies_still_decode() {
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                4\r\nAAAA\r\n3\r\nBBB\r\n0\r\n\r\n";
    let decoded = response(raw).expect("valid chunked framing decodes");
    assert_eq!(decoded.body, b"AAAABBB");
}
