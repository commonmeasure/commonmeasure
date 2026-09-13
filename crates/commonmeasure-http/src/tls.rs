//! Client-side TLS, and only client-side. Common Measure dials providers and
//! inference gateways over `https`; it never terminates TLS.
//!
//! Verification is the Mozilla root set via `webpki-roots`, pinned into the
//! binary rather than read from the host trust store. A runtime whose evidence
//! claim rests on what it retrieved should not inherit whatever certificates the
//! host happens to trust, and a vendored root set is auditable in the same way
//! the rest of the supply chain is. There is no way to disable verification: an
//! unverified fetch would produce an evidence record asserting a provenance the
//! runtime cannot stand behind.

use crate::deadline::Deadline;
use anyhow::{Context, Result};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::sync::{Arc, OnceLock};

/// One process-wide client config. Building it parses the whole root set,
/// which is wasted work per connection, and the config is immutable.
fn config() -> Result<Arc<ClientConfig>> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    if let Some(cfg) = CONFIG.get() {
        return Ok(cfg.clone());
    }

    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    // `builder_with_provider` rather than `builder`: the latter reads a
    // process-global default provider, which is install-order dependent
    // and therefore a poor thing for a library to rely on.
    let cfg =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .context("select tls protocol versions")?
            .with_root_certificates(roots)
            .with_no_client_auth();

    Ok(CONFIG.get_or_init(|| Arc::new(cfg)).clone())
}

/// The stream type for one TLS connection. `StreamOwned` is boxed at the
/// call site: a `ClientConnection` carries kilobytes of buffers, and the
/// enum that wraps it would otherwise pay that on every plain-HTTP fetch.
pub type TlsStream = StreamOwned<ClientConnection, Deadline>;

/// Complete a TLS handshake with `host` over an established `tcp`.
///
/// The handshake is not driven here. `StreamOwned` performs it lazily on
/// first read or write, so a certificate failure surfaces as an I/O error
/// from `write_request`, carrying the peer's actual complaint rather than
/// a handshake error stripped of context.
pub fn connect(host: &str, tcp: Deadline) -> Result<TlsStream> {
    let conn = ClientConnection::new(config()?, server_name(host)?)
        .with_context(|| format!("start tls session with {host}"))?;
    Ok(StreamOwned::new(conn, tcp))
}

/// The TLS server name for `host`, or the reason it is not one.
///
/// A bare IP address is a server name of its own kind rather than a DNS name,
/// so verifying it needs a matching IP entry in the certificate; it never
/// falls back to a DNS match.
fn server_name(host: &str) -> Result<ServerName<'static>> {
    ServerName::try_from(host.to_string())
        .with_context(|| format!("{host} is not a valid TLS server name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// U11 (`docs/knowledge-base/unverified-assumptions.md`), the half that
    /// needs no network: the client is built for verified `https` before any
    /// socket is opened. Reaching a real origin is the ignored test in
    /// `tests/roundtrip.rs`.
    #[test]
    fn u11_the_client_is_built_for_verified_https() {
        assert!(
            !webpki_roots::TLS_SERVER_ROOTS.is_empty(),
            "verification rests on the vendored Mozilla root set"
        );
        let first = config().expect("the client config builds offline");
        let second = config().expect("the client config builds offline");
        assert!(
            Arc::ptr_eq(&first, &second),
            "the root set is parsed once for the process"
        );

        assert!(matches!(
            server_name("example.com").expect("a DNS host is a server name"),
            ServerName::DnsName(_)
        ));
        assert!(matches!(
            server_name("127.0.0.1").expect("an IP host is a server name"),
            ServerName::IpAddress(_)
        ));
        for refused in ["", "not a host", "exa mple.com"] {
            assert!(
                server_name(refused).is_err(),
                "`{refused}` is not a TLS server name and must be refused before a socket opens"
            );
        }
    }
}
