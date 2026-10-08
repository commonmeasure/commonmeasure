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
/// body that expands without limit is refused at the same size. It is also
/// the largest file a mediated fetch hands over (a PDF), so one constant
/// states both and the transfer stops at the bound rather than reading on.
pub const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// A body refused for its size, before more than [`MAX_BODY_BYTES`] of it
/// was read. Typed so a caller can say how large the body was, where the
/// peer declared it, without parsing the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyOverCeiling {
    /// The size the peer declared in `Content-Length`, where it declared
    /// one. A chunked or EOF-delimited body is known only to be larger than
    /// the ceiling.
    pub declared: Option<u64>,
    /// True where the served body was within the ceiling and its gzip
    /// decoding was not.
    pub decoded: bool,
}

impl std::fmt::Display for BodyOverCeiling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.decoded, self.declared) {
            (true, _) => write!(
                f,
                "gzip body decodes past the size ceiling of {MAX_BODY_BYTES} bytes"
            ),
            (false, Some(declared)) => write!(
                f,
                "body of {declared} bytes exceeds size ceiling of {MAX_BODY_BYTES} bytes; none \
                 of it was read"
            ),
            (false, None) => write!(
                f,
                "body exceeds size ceiling of {MAX_BODY_BYTES} bytes; reading stopped there"
            ),
        }
    }
}

impl std::error::Error for BodyOverCeiling {}

fn over_ceiling(declared: Option<u64>) -> anyhow::Error {
    anyhow::Error::new(BodyOverCeiling {
        declared,
        decoded: false,
    })
}

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
///
/// An inbound value may carry obs-text, octets 0x80 to 0xFF that are not
/// UTF-8 (RFC 9110 §5.5), as a Latin-1 `Content-Disposition` does. Such a
/// value is kept as the bytes received ([`Headers::get_bytes`]) and read as
/// text only through a lossless rendering ([`Headers::get`]); a use that
/// needs the exact text asks [`Headers::text`], which refuses it by name.
#[derive(Debug, Clone, Default)]
pub struct Headers(Vec<Field>);

#[derive(Debug, Clone)]
struct Field {
    name: String,
    /// The value as text: exactly the value where it is UTF-8, and otherwise
    /// the rendering [`render_opaque`] makes of `opaque`.
    value: String,
    /// The value as received, where it is not UTF-8.
    opaque: Option<Box<[u8]>>,
}

/// A header value that holds bytes that are not UTF-8, refused for a use
/// that must act on its exact text: a URL to request, or a media type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueValue {
    /// The header's name as the message spelt it.
    pub name: String,
}

impl std::fmt::Display for OpaqueValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the {} header holds bytes that are not UTF-8 (obs-text), so its value is not read \
             as text",
            self.name
        )
    }
}

impl std::error::Error for OpaqueValue {}

/// A fault in bytes a peer sent, stated two ways. `Display` quotes the
/// bytes, for a log or the source record; [`PeerError::named`] names them by
/// position only, for text a model may read, which must not carry words the
/// peer chose. [`named_chain`] renders a whole error that way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerError {
    quoted: String,
    named: String,
}

impl PeerError {
    pub(crate) fn error(quoted: impl Into<String>, named: impl Into<String>) -> anyhow::Error {
        anyhow::Error::new(Self {
            quoted: quoted.into(),
            named: named.into(),
        })
    }

    /// The fault with the peer's bytes named by position, not quoted.
    pub fn named(&self) -> &str {
        &self.named
    }
}

impl std::fmt::Display for PeerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.quoted)
    }
}

impl std::error::Error for PeerError {}

/// `error` as `{error:#}` renders it, with each [`PeerError`] in its chain
/// named by position rather than quoted.
pub fn named_chain(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(|cause| match cause.downcast_ref::<PeerError>() {
            Some(peer) => peer.named.clone(),
            None => cause.to_string(),
        })
        .collect::<Vec<_>>()
        .join(": ")
}

/// The most characters of a host name that text a model may read shows. A
/// cut is marked with `…`.
pub const QUOTED_HOST_CHARS: usize = 64;

/// A host name as text a model may read shows it: cut to
/// [`QUOTED_HOST_CHARS`] characters and marked with `…` where it was cut. A
/// source chooses the host a redirect or a licence names, and DNS syntax
/// keeps spaces out of it but not words (EDG-120); the cut bounds how many
/// it can spell. The source record keeps the whole name.
pub fn quoted_host(host: &str) -> String {
    match host.char_indices().nth(QUOTED_HOST_CHARS) {
        Some((cut, _)) => format!("{}…", &host[..cut]),
        None => host.to_owned(),
    }
}

/// `text` with `url`'s host, as the URL parser serialises it, shown as
/// [`quoted_host`] shows it wherever it occurs. For a fault this crate
/// raised for a request to `url`, which names the host it looked up,
/// connected to or started TLS with.
pub fn url_host_quoted(text: &str, url: &str) -> String {
    let Some(host) = url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
    else {
        return text.to_owned();
    };
    let shown = quoted_host(&host);
    if shown == host {
        text.to_owned()
    } else {
        text.replace(&host, &shown)
    }
}

/// `error`, raised for a request to `url`, as text a model may read gives
/// it: [`named_chain`], with `url`'s host shown as [`quoted_host`] shows it
/// ([`url_host_quoted`]) and a certificate that is not valid for the name
/// asked for stated with its names shown the same way
/// ([`named_certificate`]). `{error:#}` is the form the record keeps.
pub fn named_fault(error: &anyhow::Error, url: &str) -> String {
    let mut named = named_chain(error);
    for cause in error.chain() {
        let certificate = cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .and_then(|inner| inner.downcast_ref::<rustls::Error>())
            .or_else(|| cause.downcast_ref::<rustls::Error>());
        if let Some(rustls::Error::InvalidCertificate(
            certificate @ rustls::CertificateError::NotValidForNameContext {
                expected,
                presented,
            },
        )) = certificate
        {
            named = named.replace(
                &certificate.to_string(),
                &named_certificate(&expected.to_str(), presented),
            );
        }
    }
    url_host_quoted(&named, url)
}

/// rustls's statement that a certificate is not valid for the name asked
/// for, with each name shown as [`quoted_host`] shows it. The names the
/// certificate presents are its subject alternative names as rustls writes
/// them: a DNS name as `DnsName("…")`, an address as `IpAddress(…)`. Any
/// other kind, or a DNS name holding a character DNS syntax does not allow,
/// is named by position: its bytes are written as the certificate holds
/// them.
fn named_certificate(expected: &str, presented: &[String]) -> String {
    let named = |name: &String| {
        let dns = name
            .strip_prefix("DnsName(\"")
            .and_then(|rest| rest.strip_suffix("\")"))
            .filter(|dns| {
                !dns.is_empty()
                    && dns.bytes().all(|b| {
                        b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'*')
                    })
            });
        let address = name
            .strip_prefix("IpAddress(")
            .and_then(|rest| rest.strip_suffix(')'))
            .filter(|address| {
                address
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() || matches!(b, b'.' | b':'))
            });
        match (dns, address) {
            (Some(dns), _) => format!("DnsName(\"{}\")", quoted_host(dns)),
            (None, Some(_)) => name.clone(),
            (None, None) => "a name of another kind".to_owned(),
        }
    };
    let names: Vec<String> = presented.iter().map(named).collect();
    let valid_for = match names.as_slice() {
        [] => "is not valid for any names (according to its subjectAltName extension)".to_owned(),
        [one] => format!("is only valid for {one}"),
        [all_but_last @ .., last] => {
            format!("is only valid for {} or {last}", all_but_last.join(", "))
        }
    };
    format!(
        "certificate not valid for name \"{}\"; certificate {valid_for}",
        quoted_host(expected)
    )
}

/// A `Content-Type` value as text a model may read names it: its
/// top-level type where that is one IANA registers, as `text/…`, since the
/// subtype and parameters are the peer's words; otherwise `an unrecognised
/// media type`.
pub fn media_type_named(value: &str) -> &'static str {
    // Each name is this crate's own text, so the result is `'static` and
    // may be written into text built from nothing a peer sent.
    const TOP_LEVEL: [(&str, &str); 11] = [
        ("application", "application/…"),
        ("audio", "audio/…"),
        ("example", "example/…"),
        ("font", "font/…"),
        ("haptics", "haptics/…"),
        ("image", "image/…"),
        ("message", "message/…"),
        ("model", "model/…"),
        ("multipart", "multipart/…"),
        ("text", "text/…"),
        ("video", "video/…"),
    ];
    let top = value.split('/').next().unwrap_or_default().trim();
    match TOP_LEVEL
        .into_iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(top))
    {
        Some((_, named)) if value.contains('/') => named,
        _ => "an unrecognised media type",
    }
}

/// What kind of transport fault `error` is, for text that may carry nothing
/// the peer sent: each kind is named in this crate's words alone, with no
/// host, address, certificate name or peer bytes. `{error:#}` is the form
/// the record keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    /// The name did not resolve, or resolved to no address.
    Lookup,
    /// No answer arrived within the exchange budget.
    Timeout,
    /// The connection was refused or not established.
    Connect,
    /// The connection was closed before an answer was read.
    Closed,
    /// The certificate the peer presented is not valid for the name asked
    /// for.
    CertificateName,
    /// The certificate the peer presented is not one this edge trusts.
    CertificateUntrusted,
    /// The TLS session could not be started for another reason.
    Tls,
    /// The peer's bytes could not be read as an HTTP response.
    Peer,
    /// The body could not be decoded: its content coding was not the one
    /// requested, or its gzip stream did not decode.
    Body,
    /// Any other fault.
    Other,
}

impl FaultKind {
    /// The fault as text that carries nothing the peer sent names it.
    pub fn named(self) -> &'static str {
        match self {
            Self::Lookup => "its name could not be resolved",
            Self::Timeout => "it did not answer within the time allowed",
            Self::Connect => "the connection was refused or could not be made",
            Self::Closed => "the connection was closed before an answer was read",
            Self::CertificateName => {
                "the certificate it presented is not valid for the name asked for"
            }
            Self::CertificateUntrusted => {
                "the certificate it presented is not one this edge trusts"
            }
            Self::Tls => "the TLS session could not be started",
            Self::Peer => "its answer could not be read as an HTTP response",
            Self::Body => "its body could not be decoded",
            Self::Other => "the request failed",
        }
    }
}

/// The [`FaultKind`] of `error`, an error this crate raised for a request.
/// The timeout is recognised by the sentence [`send_to`](crate::send_to)
/// wrote for it, since that sentence replaces the socket's own error.
pub fn fault_kind(error: &anyhow::Error) -> FaultKind {
    for cause in error.chain() {
        if cause.downcast_ref::<PeerError>().is_some() {
            return FaultKind::Peer;
        }
        let tls = cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::get_ref)
            .and_then(|inner| inner.downcast_ref::<rustls::Error>())
            .or_else(|| cause.downcast_ref::<rustls::Error>());
        match tls {
            Some(rustls::Error::InvalidCertificate(
                rustls::CertificateError::NotValidForName
                | rustls::CertificateError::NotValidForNameContext { .. },
            )) => return FaultKind::CertificateName,
            Some(rustls::Error::InvalidCertificate(_)) => return FaultKind::CertificateUntrusted,
            Some(_) => return FaultKind::Tls,
            None => {}
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            use std::io::ErrorKind;
            return match io.kind() {
                ErrorKind::WouldBlock | ErrorKind::TimedOut => FaultKind::Timeout,
                ErrorKind::ConnectionRefused
                | ErrorKind::HostUnreachable
                | ErrorKind::NetworkUnreachable
                | ErrorKind::AddrNotAvailable => FaultKind::Connect,
                ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::BrokenPipe
                | ErrorKind::UnexpectedEof
                | ErrorKind::NotConnected => FaultKind::Closed,
                _ => FaultKind::Other,
            };
        }
    }
    let text = format!("{error:#}");
    if text.starts_with("resolve ") || text.contains(" resolved to no addresses") {
        FaultKind::Lookup
    } else if text.contains(" did not answer within the ")
        || text.contains("timed out before a connection was established")
    {
        FaultKind::Timeout
    } else if text.contains("no address to connect to") {
        FaultKind::Connect
    } else if text.contains("start tls session with ")
        || text.contains("is not a valid TLS server name")
    {
        FaultKind::Tls
    } else if text.contains("gzip body could not be decoded")
        || text.contains("response content coding ")
    {
        FaultKind::Body
    } else {
        FaultKind::Other
    }
}

/// A content coding as a fault message names it: the codings RFC 9110
/// registers by name, and any other as unrecognised, since the name is the
/// peer's choice.
fn coding_named(coding: &str) -> &'static str {
    const KNOWN: [&str; 8] = [
        "gzip",
        "x-gzip",
        "deflate",
        "compress",
        "x-compress",
        "br",
        "zstd",
        "dcb",
    ];
    KNOWN
        .into_iter()
        .find(|known| known.eq_ignore_ascii_case(coding))
        .unwrap_or("an unrecognised coding")
}

/// A lossless text rendering of a value that is not UTF-8: each byte from
/// 0x80 is written `\xHH` and a backslash `\\`, and every other byte, all
/// visible ASCII, space or HTAB by the time a value is held, as itself.
/// Nothing is guessed about a charset, so no byte is mis-decoded.
fn render_opaque(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(bytes.len() + 8);
    for &byte in bytes {
        match byte {
            b'\\' => rendered.push_str("\\\\"),
            0x80.. => {
                let _ = write!(rendered, "\\x{byte:02X}");
            }
            _ => rendered.push(char::from(byte)),
        }
    }
    rendered
}

/// A value as text for the source record, and whether that text is a
/// rendering: the value itself where it is UTF-8, and otherwise the lossless
/// rendering [`Headers::get`] gives. A reader needs the flag to read the text
/// back to the bytes, since UTF-8 text may itself read `\xE9`.
pub fn render_value(bytes: &[u8]) -> (std::borrow::Cow<'_, str>, bool) {
    match std::str::from_utf8(bytes) {
        Ok(text) => (std::borrow::Cow::Borrowed(text), false),
        Err(_) => (std::borrow::Cow::Owned(render_opaque(bytes)), true),
    }
}

impl Headers {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    fn first(&self, name: &str) -> Option<&Field> {
        self.0
            .iter()
            .find(|field| field.name.eq_ignore_ascii_case(name))
    }

    /// The first value for `name` as text. A value that is not UTF-8 comes
    /// back as its lossless rendering (`\xE9` for the byte 0xE9), which a
    /// parser of an ASCII grammar reads as it reads any unrecognised token.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.first(name).map(|field| field.value.as_str())
    }

    /// The first value for `name` exactly as text, refusing a value that is
    /// not UTF-8 rather than acting on its rendering.
    pub fn text(&self, name: &str) -> Result<Option<&str>, OpaqueValue> {
        match self.first(name) {
            None => Ok(None),
            Some(field) if field.opaque.is_some() => Err(OpaqueValue {
                name: field.name.clone(),
            }),
            Some(field) => Ok(Some(field.value.as_str())),
        }
    }

    /// The first value for `name` as the bytes it holds: the bytes received
    /// for an inbound value, and the UTF-8 of a value set here.
    pub fn get_bytes(&self, name: &str) -> Option<&[u8]> {
        self.first(name)
            .map(|field| field.opaque.as_deref().unwrap_or(field.value.as_bytes()))
    }

    /// Every value for `name` as the bytes it holds, in the order received.
    /// A list-valued field such as `Link` may be split over several fields,
    /// and a reader of the list reads them all.
    pub fn all_bytes<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.0
            .iter()
            .filter(move |field| field.name.eq_ignore_ascii_case(name))
            .map(|field| field.opaque.as_deref().unwrap_or(field.value.as_bytes()))
    }

    /// Replace any existing values for `name` with a single value, keeping
    /// the position the first of them held. Setting a header a second time
    /// changes its value, not where the message carries it.
    pub fn set(&mut self, name: &str, value: &str) {
        let mut replaced = false;
        self.0.retain_mut(|field| {
            if !field.name.eq_ignore_ascii_case(name) {
                return true;
            }
            if replaced {
                return false;
            }
            replaced = true;
            *field = Field::text(name, value);
            true
        });
        if !replaced {
            self.0.push(Field::text(name, value));
        }
    }

    pub fn append(&mut self, name: &str, value: &str) {
        self.0.push(Field::text(name, value));
    }

    /// Append a value read from the wire, kept as received.
    fn append_inbound(&mut self, name: &str, value: &[u8]) {
        let field = match std::str::from_utf8(value) {
            Ok(text) => Field::text(name, text),
            Err(_) => Field {
                name: name.to_string(),
                value: render_opaque(value),
                opaque: Some(value.into()),
            },
        };
        self.0.push(field);
    }

    /// Drop every value for `name`. A header a caller attached for one
    /// origin and must not send to another leaves through here.
    pub fn remove(&mut self, name: &str) {
        self.0
            .retain(|field| !field.name.eq_ignore_ascii_case(name));
    }

    /// Every field in order, each value as [`Headers::get`] gives it.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|field| (field.name.as_str(), field.value.as_str()))
    }
}

impl Field {
    fn text(name: &str, value: &str) -> Self {
        Self {
            name: name.to_string(),
            value: value.to_string(),
            opaque: None,
        }
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

/// One line of a message head or of chunked framing, without its CRLF, as
/// bytes: a field value, a reason phrase and a chunk extension may all carry
/// obs-text, so UTF-8 is not required here.
fn read_line_bounded(reader: &mut impl BufRead, budget: &mut usize) -> Result<Vec<u8>> {
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
    Ok(line)
}

/// A request line or status line, which carries no obs-text outside the
/// reason phrase; the reason phrase is split off before this is asked.
fn ascii_line(line: &[u8], what: &str) -> Result<String> {
    if !line.is_ascii() {
        return Err(PeerError::error(
            format!("{what} {:?} is not ASCII", render_opaque(line)),
            format!("the {what} is not ASCII"),
        ));
    }
    Ok(String::from_utf8_lossy(line).into_owned())
}

/// Outbound values retain the strict configuration guard: no control bytes,
/// including tabs, can reach a credentialed origin through a header value.
fn check_field(name: &str, value: &str) -> Result<()> {
    check_field_name(name)?;
    if let Some(byte) = value.bytes().find(|b| *b < 0x20 || *b == 0x7f) {
        bail!("header {name} has control byte {byte:#04x} in its value");
    }
    Ok(())
}

fn check_field_name(name: &str) -> Result<()> {
    if name.is_empty() || !name.bytes().all(is_token_byte) {
        return Err(PeerError::error(
            format!("header name {name:?} is not a token"),
            "a header name is not a token",
        ));
    }
    Ok(())
}

/// RFC 9110 §5.5 permits HTAB and obs-text (0x80 to 0xFF) inside an inbound
/// value. Other control bytes remain invalid; optional whitespace is stripped
/// by the reader.
fn check_inbound_field(name: &str, value: &[u8]) -> Result<()> {
    check_field_name(name)?;
    if let Some(byte) = value
        .iter()
        .find(|b| (**b < 0x20 && **b != b'\t') || **b == 0x7f)
    {
        return Err(PeerError::error(
            format!("header {name} has control byte {byte:#04x} in its value"),
            format!("a header has control byte {byte:#04x} in its value"),
        ));
    }
    Ok(())
}

/// A value without the optional whitespace, space and HTAB only, around it.
fn trim_ows(value: &[u8]) -> &[u8] {
    let ows = |b: &u8| *b == b' ' || *b == b'\t';
    let start = value.iter().position(|b| !ows(b)).unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|b| !ows(b))
        .map_or(start, |last| last + 1);
    &value[start..end]
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
        let Some(colon) = line.iter().position(|b| *b == b':') else {
            return Err(PeerError::error(
                format!("malformed header line {:?}", render_opaque(&line)),
                "a header line has no colon",
            ));
        };
        let (name, value) = (&line[..colon], &line[colon + 1..]);
        let Ok(name) = std::str::from_utf8(name) else {
            return Err(PeerError::error(
                format!("header name {:?} is not a token", render_opaque(name)),
                "a header name is not a token",
            ));
        };
        // Only optional whitespace around the value is the sender's to add; a
        // space before the colon makes the line ambiguous, not trimmable.
        let value = trim_ows(value);
        check_inbound_field(name, value)?;
        headers.append_inbound(name, value);
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
            return Err(PeerError::error(
                format!("message declares content-length {first:?} and {value:?}"),
                "the message declares two different content-length values",
            ));
        }
        declared = Some(value);
    }
    let Some(value) = declared else {
        return Ok(None);
    };
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(PeerError::error(
            format!("content-length {value:?} is not a decimal number"),
            "the content-length is not a decimal number",
        ));
    }
    let len: usize = value.parse().context("parse content-length")?;
    Ok(Some(len))
}

/// What a message is read as, for the content codings it may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Coded {
    /// A request, whose body is never decoded.
    Request,
    /// A response to a request whose `Accept-Encoding` named gzip (`true`)
    /// or identity only (`false`).
    Response { gzip_requested: bool },
}

fn is_gzip(coding: &str) -> bool {
    coding.eq_ignore_ascii_case("gzip") || coding.eq_ignore_ascii_case("x-gzip")
}

/// The content coding a message declares, where it is one this reader
/// removes: `None` for no coding or `identity`, `Some("gzip")` for gzip
/// (RFC 9110 §8.4.1.3 has a recipient treat `x-gzip` as gzip).
///
/// Every other coding is refused, as is more than one coding, whether in one
/// field or across repeated fields: a body this reader did not decode would
/// be hashed, stored and handed on as if it were the content it claims to be.
/// `gzip` is removed only where the caller can keep the served bytes beside
/// the decoded ones, which is a response, and only where the request asked
/// for it. A response in a coding the request did not accept is refused by
/// that name, so the record says the origin ignored the request rather than
/// that this edge lacks a decoder.
fn content_coding(headers: &Headers, reading: Coded) -> Result<Option<&'static str>> {
    let codings: Vec<&str> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
        .flat_map(|(_, value)| value.split(','))
        .map(|coding| coding.trim_matches([' ', '\t']))
        .filter(|coding| !coding.is_empty() && !coding.eq_ignore_ascii_case("identity"))
        .collect();
    let named = |codings: &[&str]| {
        codings
            .iter()
            .map(|coding| coding_named(coding))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let gzip_requested = match reading {
        Coded::Request if codings.is_empty() => return Ok(None),
        Coded::Request => {
            return Err(PeerError::error(
                format!("unsupported content encoding {}", codings.join(", ")),
                format!("unsupported content encoding {}", named(&codings)),
            ));
        }
        Coded::Response { gzip_requested } => gzip_requested,
    };
    let unrequested: Vec<&str> = codings
        .iter()
        .copied()
        .filter(|coding| !(gzip_requested && is_gzip(coding)))
        .collect();
    let accepted = if gzip_requested { "gzip" } else { "identity" };
    match (codings.as_slice(), unrequested.as_slice()) {
        ([], _) => Ok(None),
        ([_], []) => Ok(Some("gzip")),
        (_, []) => Err(PeerError::error(
            format!(
                "response content coding {} applies gzip more than once, and this edge removes \
                 one gzip coding only",
                codings.join(", ")
            ),
            "response content coding applies gzip more than once, and this edge removes one \
             gzip coding only",
        )),
        (_, unrequested) => Err(PeerError::error(
            format!(
                "response content coding {} was not requested: the request accepted {accepted} \
                 only",
                unrequested.join(", ")
            ),
            format!(
                "response content coding {} was not requested: the request accepted {accepted} \
                 only",
                named(unrequested)
            ),
        )),
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
        return Err(anyhow::Error::new(BodyOverCeiling {
            declared: None,
            decoded: true,
        }));
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
        return Err(PeerError::error(
            format!("unsupported transfer encoding {te}"),
            "unsupported transfer encoding: only chunked is read",
        ));
    }
    if let Some(len) = declared {
        if len > MAX_BODY_BYTES {
            return Err(over_ceiling(Some(len as u64)));
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
            return Err(over_ceiling(None));
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
        let size_hex = size_line.split(|b| *b == b';').next().unwrap_or_default();
        if size_hex.is_empty() || !size_hex.iter().all(u8::is_ascii_hexdigit) {
            return Err(PeerError::error(
                format!(
                    "chunk size {:?} is not hexadecimal",
                    render_opaque(&size_line)
                ),
                "a chunk size is not hexadecimal",
            ));
        }
        // All hexadecimal digits, so ASCII.
        let size_hex = String::from_utf8_lossy(size_hex);
        let size = usize::from_str_radix(&size_hex, 16)
            .with_context(|| format!("parse chunk size {size_hex:?}"))?;
        // Checked: `size` is attacker-controlled up to usize::MAX, and a
        // wrapping sum would pass the ceiling it exists to enforce.
        let total = body
            .len()
            .checked_add(size)
            .ok_or_else(|| over_ceiling(None))?;
        if total > MAX_BODY_BYTES {
            return Err(over_ceiling(None));
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
        return Err(PeerError::error(
            format!("unsupported version {version}"),
            "unsupported version: only HTTP/1.1 and HTTP/1.0 are spoken",
        ));
    }
    Ok(())
}

/// Read one request from a connection. Bodies require Content-Length or
/// chunked encoding; a request body is never EOF-delimited.
pub fn read_request(reader: &mut impl BufRead) -> Result<Request> {
    let mut budget = MAX_HEADER_BYTES;
    let request_line = ascii_line(&read_line_bounded(reader, &mut budget)?, "request line")?;
    let mut parts = request_line.splitn(3, ' ');
    let method = parts.next().context("missing method")?.to_string();
    let target = parts.next().context("missing target")?.to_string();
    check_version(parts.next().context("missing version")?)?;
    let headers = read_headers(reader, &mut budget)?;
    content_coding(&headers, Coded::Request)?;
    let body = read_body(reader, &headers, false)?;
    Ok(Request {
        method,
        target,
        headers,
        body,
    })
}

/// Read one response to a request that accepted gzip, the coding a request
/// through [`crate::send`] accepts unless its caller asked for identity.
pub fn read_response(reader: &mut impl BufRead) -> Result<Response> {
    read_response_to(reader, true)
}

/// Read one response, refusing a content coding the request did not accept:
/// gzip where `gzip_requested`, and otherwise none.
pub(crate) fn read_response_to(
    reader: &mut impl BufRead,
    gzip_requested: bool,
) -> Result<Response> {
    let mut budget = MAX_HEADER_BYTES;
    let status_line = read_line_bounded(reader, &mut budget)?;
    // The reason phrase may carry obs-text (RFC 9112 §4); it is kept as a
    // lossless rendering, and the version and status before it are ASCII.
    let mut parts = status_line.splitn(3, |b| *b == b' ');
    check_version(&ascii_line(
        parts.next().context("missing version")?,
        "status line",
    )?)?;
    let status: u16 = ascii_line(parts.next().context("missing status")?, "status line")?
        .parse()
        .context("parse status")?;
    let reason = parts.next().map_or_else(String::new, |reason| {
        std::str::from_utf8(reason).map_or_else(|_| render_opaque(reason), str::to_owned)
    });
    let headers = read_headers(reader, &mut budget)?;
    let coding = content_coding(&headers, Coded::Response { gzip_requested })?;
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
