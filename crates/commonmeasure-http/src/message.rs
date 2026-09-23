//! HTTP/1.1 message reading and writing. Bounded reads throughout: these are
//! bytes from parties Common Measure does not control.

use anyhow::{Context, Result, bail};
use std::fmt::Write as _;
use std::io::{BufRead, Read, Write};

/// Hard ceiling on header block size.
const MAX_HEADER_BYTES: usize = 64 * 1024;
/// Hard ceiling on body size. Provider search responses and extracted pages fit
/// comfortably; anything larger is not something to place in a context window
/// anyway. It bounds a decoded body as well as a served one, so a small gzip
/// body that expands without limit is refused at the same size.
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Ordered, case-insensitive header map.
///
/// A `Vec` rather than a map because two things depend on order. Repeated
/// values of one field name combine in the order they were appended, which
/// is what HTTP field-value combination means and what an RFC 9421 signature
/// covers when it names that field. And a message reads on the wire in the
/// order it was built, so what a reviewer sees in a capture is what the code
/// above asked for. [`Headers::set`] replaces in place for the same reason.
///
/// Wire order is not itself signed: an RFC 9421 signature base is built from
/// the components the signature input names, in the order it names them.
#[derive(Debug, Clone, Default)]
pub struct Headers(Vec<(String, String)>);

impl Headers {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Replace any existing values for `name` with a single value, keeping
    /// the position the first of them held. Setting a header a second time
    /// changes its value, not where the message carries it.
    pub fn set(&mut self, name: &str, value: &str) {
        let mut replaced = false;
        self.0.retain_mut(|(existing, held)| {
            if !existing.eq_ignore_ascii_case(name) {
                return true;
            }
            if replaced {
                return false;
            }
            replaced = true;
            *existing = name.to_string();
            *held = value.to_string();
            true
        });
        if !replaced {
            self.0.push((name.to_string(), value.to_string()));
        }
    }

    pub fn append(&mut self, name: &str, value: &str) {
        self.0.push((name.to_string(), value.to_string()));
    }

    /// Drop every value for `name`. A header a caller attached for one
    /// origin and must not send to another leaves through here.
    pub fn remove(&mut self, name: &str) {
        self.0.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// Origin-form (`/path?q`) or absolute-form (`http://host/path`) target.
    pub target: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

impl Request {
    pub fn get(target: &str) -> Self {
        Self {
            method: "GET".to_string(),
            target: target.to_string(),
            headers: Headers::new(),
            body: Vec::new(),
        }
    }

    pub fn post(target: &str, body: Vec<u8>, content_type: &str) -> Self {
        let mut headers = Headers::new();
        headers.set("Content-Type", content_type);
        Self {
            method: "POST".to_string(),
            target: target.to_string(),
            headers,
            body,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub reason: String,
    pub headers: Headers,
    /// The body with any content coding removed: the representation itself.
    pub body: Vec<u8>,
    /// The body as the origin served it, where a content coding was removed
    /// to produce `body`; absent where `body` is exactly the bytes served.
    pub coded: Option<CodedBody>,
}

/// A response body as served under a content coding this reader removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodedBody {
    /// The content coding, as registered: `gzip` (`x-gzip` is read as it).
    pub coding: &'static str,
    /// The bytes on the wire after transfer framing was removed.
    pub bytes: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            reason: reason_for(status).to_string(),
            headers: Headers::new(),
            body,
            coded: None,
        }
    }

    /// The body as the origin served it: the coded bytes where a content
    /// coding was removed, and otherwise `body`.
    pub fn served_body(&self) -> &[u8] {
        self.coded
            .as_ref()
            .map_or(self.body.as_slice(), |coded| coded.bytes.as_slice())
    }

    pub fn text(status: u16, body: &str) -> Self {
        let mut r = Self::new(status, body.as_bytes().to_vec());
        r.headers.set("Content-Type", "text/plain; charset=utf-8");
        r
    }

    pub fn json(status: u16, body: &str) -> Self {
        let mut r = Self::new(status, body.as_bytes().to_vec());
        r.headers.set("Content-Type", "application/json");
        r
    }
}

fn reason_for(status: u16) -> &'static str {
    match status {
        200 => "OK",
        301 => "Moved Permanently",
        302 => "Found",
        400 => "Bad Request",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        422 => "Unprocessable Content",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
}

fn read_line_bounded(reader: &mut impl BufRead, budget: &mut usize) -> Result<String> {
    let mut line = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).context("read header byte")?;
        *budget = budget.checked_sub(1).context("header block too large")?;
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line).context("header line is not utf-8")
}

/// A field name is a token and a field value carries no control bytes
/// (RFC 9110 §5.1, §5.5). Checked on the way out because header values arrive
/// from operator configuration — an API key holding a CRLF would otherwise
/// write extra header lines into a request to a credentialed origin — and on
/// the way in so this parser cannot mint a field a compliant peer would refuse.
fn check_field(name: &str, value: &str) -> Result<()> {
    if name.is_empty() || !name.bytes().all(is_token_byte) {
        bail!("header name {name:?} is not a token");
    }
    if let Some(byte) = value.bytes().find(|b| *b < 0x20 || *b == 0x7f) {
        bail!("header {name} has control byte {byte:#04x} in its value");
    }
    Ok(())
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn read_headers(reader: &mut impl BufRead, budget: &mut usize) -> Result<Headers> {
    let mut headers = Headers::new();
    loop {
        let line = read_line_bounded(reader, budget)?;
        if line.is_empty() {
            return Ok(headers);
        }
        let (name, value) = line
            .split_once(':')
            .with_context(|| format!("malformed header line {line:?}"))?;
        // Only optional whitespace around the value is the sender's to add; a
        // space before the colon makes the line ambiguous, not trimmable.
        let value = value.trim_matches([' ', '\t']);
        check_field(name, value)?;
        headers.append(name, value);
    }
}

/// The single declared body length, if the message declares one.
///
/// Two lengths that disagree, or a length beside a transfer coding, are a
/// smuggling primitive rather than a message: two parsers on the same path
/// resolve them differently, so RFC 9112 §6.1 and §6.3 require an error here
/// instead of a preference.
fn content_length(headers: &Headers) -> Result<Option<usize>> {
    let mut declared: Option<&str> = None;
    for (name, value) in headers.iter() {
        if !name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        if let Some(first) = declared
            && first != value
        {
            bail!("message declares content-length {first:?} and {value:?}");
        }
        declared = Some(value);
    }
    let Some(value) = declared else {
        return Ok(None);
    };
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        bail!("content-length {value:?} is not a decimal number");
    }
    let len: usize = value.parse().context("parse content-length")?;
    Ok(Some(len))
}

/// The content coding a message declares, where it is one this reader
/// removes: `None` for no coding or `identity`, `Some("gzip")` for gzip
/// (RFC 9110 §8.4.1.3 has a recipient treat `x-gzip` as gzip).
///
/// Every other coding is refused, as is more than one coding, whether in one
/// field or across repeated fields: a body this reader did not decode would
/// be hashed, stored and handed on as if it were the content it claims to be.
/// `gzip` is removed only where the caller can keep the served bytes beside
/// the decoded ones, which is a response.
fn content_coding(headers: &Headers, gzip_allowed: bool) -> Result<Option<&'static str>> {
    let codings: Vec<&str> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
        .flat_map(|(_, value)| value.split(','))
        .map(|coding| coding.trim_matches([' ', '\t']))
        .filter(|coding| !coding.is_empty() && !coding.eq_ignore_ascii_case("identity"))
        .collect();
    match codings.as_slice() {
        [] => Ok(None),
        [coding]
            if gzip_allowed
                && (coding.eq_ignore_ascii_case("gzip")
                    || coding.eq_ignore_ascii_case("x-gzip")) =>
        {
            Ok(Some("gzip"))
        }
        codings => bail!("unsupported content encoding {}", codings.join(", ")),
    }
}

/// Remove the gzip coding from a served body, every member in turn, with the
/// output bounded by the body ceiling. A body that does not decode completely
/// is an error naming the cause, never a shorter or empty body.
fn gunzip(coded: &[u8]) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    flate2::read::MultiGzDecoder::new(coded)
        .take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut decoded)
        .map_err(|error| anyhow::anyhow!("gzip body could not be decoded: {error}"))?;
    if decoded.len() > MAX_BODY_BYTES {
        bail!("gzip body decodes past the size ceiling of {MAX_BODY_BYTES} bytes");
    }
    Ok(decoded)
}

fn read_body(
    reader: &mut impl BufRead,
    headers: &Headers,
    allow_eof_delimited: bool,
) -> Result<Vec<u8>> {
    let declared = content_length(headers)?;
    if let Some(te) = headers.get("Transfer-Encoding") {
        if declared.is_some() {
            bail!("message declares both Content-Length and Transfer-Encoding");
        }
        if te.eq_ignore_ascii_case("chunked") {
            return read_chunked(reader);
        }
        bail!("unsupported transfer encoding {te}");
    }
    if let Some(len) = declared {
        if len > MAX_BODY_BYTES {
            bail!("body exceeds size ceiling");
        }
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).context("read body")?;
        return Ok(body);
    }
    if allow_eof_delimited {
        let mut body = Vec::new();
        reader
            .by_ref()
            .take(MAX_BODY_BYTES as u64 + 1)
            .read_to_end(&mut body)
            .context("read body to eof")?;
        if body.len() > MAX_BODY_BYTES {
            bail!("body exceeds size ceiling");
        }
        return Ok(body);
    }
    Ok(Vec::new())
}

fn read_chunked(reader: &mut impl BufRead) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    // One budget for the whole framing. Per chunk it would bound a single
    // chunk extension and nothing at all across a stream of them.
    let mut budget = MAX_HEADER_BYTES;
    loop {
        let size_line = read_line_bounded(reader, &mut budget)?;
        let size_hex = size_line.split(';').next().unwrap_or("");
        if size_hex.is_empty() || !size_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("chunk size {size_line:?} is not hexadecimal");
        }
        let size = usize::from_str_radix(size_hex, 16)
            .with_context(|| format!("parse chunk size {size_line:?}"))?;
        // Checked: `size` is attacker-controlled up to usize::MAX, and a
        // wrapping sum would pass the ceiling it exists to enforce.
        let total = body
            .len()
            .checked_add(size)
            .context("chunked body exceeds size ceiling")?;
        if total > MAX_BODY_BYTES {
            bail!("chunked body exceeds size ceiling");
        }
        if size == 0 {
            // Trailer section, then final CRLF.
            loop {
                let line = read_line_bounded(reader, &mut budget)?;
                if line.is_empty() {
                    return Ok(body);
                }
            }
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader
            .read_exact(&mut body[start..])
            .context("read chunk")?;
        let mut crlf = [0u8; 2];
        reader
            .read_exact(&mut crlf)
            .context("read chunk terminator")?;
        // Unchecked, a chunk that runs long by two bytes silently swallows the
        // start of the next chunk size line and the body reassembles wrong.
        if &crlf != b"\r\n" {
            bail!("chunk data is not terminated by CRLF");
        }
    }
}

/// The two versions this transport speaks. Anything else — including a
/// plausible-looking `HTTP/1.9` — is a peer making assumptions this parser has
/// not agreed to.
fn check_version(version: &str) -> Result<()> {
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        bail!("unsupported version {version}");
    }
    Ok(())
}

/// Read one request from a connection. Bodies require Content-Length or
/// chunked encoding; a request body is never EOF-delimited.
pub fn read_request(reader: &mut impl BufRead) -> Result<Request> {
    let mut budget = MAX_HEADER_BYTES;
    let request_line = read_line_bounded(reader, &mut budget)?;
    let mut parts = request_line.splitn(3, ' ');
    let method = parts.next().context("missing method")?.to_string();
    let target = parts.next().context("missing target")?.to_string();
    check_version(parts.next().context("missing version")?)?;
    let headers = read_headers(reader, &mut budget)?;
    content_coding(&headers, false)?;
    let body = read_body(reader, &headers, false)?;
    Ok(Request {
        method,
        target,
        headers,
        body,
    })
}

pub fn read_response(reader: &mut impl BufRead) -> Result<Response> {
    let mut budget = MAX_HEADER_BYTES;
    let status_line = read_line_bounded(reader, &mut budget)?;
    let mut parts = status_line.splitn(3, ' ');
    check_version(parts.next().context("missing version")?)?;
    let status: u16 = parts
        .next()
        .context("missing status")?
        .parse()
        .context("parse status")?;
    let reason = parts.next().unwrap_or("").to_string();
    let headers = read_headers(reader, &mut budget)?;
    let coding = content_coding(&headers, true)?;
    let served = if (100..200).contains(&status) || status == 204 || status == 304 {
        Vec::new()
    } else {
        read_body(reader, &headers, true)?
    };
    // An empty body has nothing to decode, whatever coding it declares.
    let (body, coded) = match coding {
        Some(coding) if !served.is_empty() => (
            gunzip(&served)?,
            Some(CodedBody {
                coding,
                bytes: served,
            }),
        ),
        _ => (served, None),
    };
    Ok(Response {
        status,
        reason,
        headers,
        body,
        coded,
    })
}

/// Write a request: the whole head in one call, then the body.
///
/// The head is built into one buffer first. Writing it line by line puts one
/// syscall on the wire per header, and a peer that reads once then decides
/// what to do sees only the request line — a real origin behaves that way,
/// and so does a probe.
pub fn write_request(writer: &mut impl Write, req: &Request) -> Result<()> {
    // Before the first byte: half a request on the wire is one the origin
    // still has to parse.
    for (name, value) in req.headers.iter() {
        check_field(name, value)?;
    }
    let mut head = format!("{} {} HTTP/1.1\r\n", req.method, req.target);
    let mut has_length = false;
    for (name, value) in req.headers.iter() {
        if name.eq_ignore_ascii_case("content-length") {
            has_length = true;
        }
        let _ = write!(head, "{name}: {value}\r\n");
    }
    if !req.body.is_empty() && !has_length {
        let _ = write!(head, "Content-Length: {}\r\n", req.body.len());
    }
    head.push_str("\r\n");
    writer.write_all(head.as_bytes())?;
    writer.write_all(&req.body)?;
    writer.flush()?;
    Ok(())
}

/// Write a response: the whole head in one call, then the body as served, for
/// the same reason as [`write_request`]. A response read under a content
/// coding keeps its `Content-Encoding` field, so the coded bytes are what
/// agree with it.
///
/// The caller's headers keep their order. Framing — `Content-Length` and
/// `Connection` — is decided here rather than passed through, so those two
/// are dropped from the caller's list and written last, whatever position
/// the caller gave them.
pub fn write_response(writer: &mut impl Write, resp: &Response) -> Result<()> {
    for (name, value) in resp.headers.iter() {
        check_field(name, value)?;
    }
    let mut head = format!("HTTP/1.1 {} {}\r\n", resp.status, resp.reason);
    for (name, value) in resp.headers.iter() {
        if name.eq_ignore_ascii_case("content-length")
            || name.eq_ignore_ascii_case("transfer-encoding")
            || name.eq_ignore_ascii_case("connection")
        {
            continue;
        }
        let _ = write!(head, "{name}: {value}\r\n");
    }
    let body = resp.served_body();
    let _ = write!(head, "Content-Length: {}\r\n", body.len());
    head.push_str("Connection: close\r\n\r\n");
    writer.write_all(head.as_bytes())?;
    writer.write_all(body)?;
    writer.flush()?;
    Ok(())
}
