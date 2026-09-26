//! Relay configuration: who receives the projection, if anyone, and which
//! suppliers' events that receiver takes.
//!
//! No configuration means no egress. That is not a fallback but the shipped
//! state: there is no default receiver, no ambient environment variable, and
//! no code path that sends without a receiver having been named explicitly in
//! `<home>/relay.json` or on the command line. A malformed file is an error,
//! never a silent absence, following the policy file's discipline: egress
//! nobody chose must not start, and egress somebody chose must not silently
//! stop.
//!
//! The relay (`commonmeasure-relay`) and the harness's reporting ruling read
//! the file through this one parser. A file the relay refuses is a file whose
//! reports cannot leave, so the ruling must refuse it too rather than read the
//! parts it understands.

use crate::declaration;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Unknown fields are load errors, like the policy file's: a misspelled
/// `"api_kye"` must fail the load, not deliver unauthenticated.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RelayConfig {
    /// Base URL of the Content Telemetry receiver, e.g. `http://localhost:8080`.
    /// Batches are posted to [`events_url`] of it. A URL with a query, a
    /// fragment or credentials is refused at load ([`receiver_endpoint`]).
    pub receiver: String,
    /// API key presented as `X-API-Key`. Optional because the standard does
    /// not prescribe an auth scheme; a conforming receiver may require one.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Suppliers whose content this receiver takes, e.g. `["ozone"]` for a
    /// supplier's own telemetry server. When set, only events that name one
    /// of them (`data.commonmeasure-supplier`) leave for this receiver: the
    /// operator's own fetches, other suppliers' results and turn boundaries
    /// stay home, and the batches carry no refused count. Absent or `null`
    /// means every cleared event, as before. An empty list is scoped to no
    /// supplier, so the receiver takes nothing. It scopes the configured
    /// receiver, and a `--receiver` override naming the same endpoint
    /// ([`same_receiver`]); an override to another receiver is not scoped by
    /// it.
    ///
    /// This is a local narrowing, not the authority on what the receiver may
    /// see. That follows the supplier's grant; the list may narrow what this
    /// edge sends within it and never widens it (owner decision, 22 September
    /// 2026).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppliers: Option<Vec<String>>,
}

impl RelayConfig {
    /// Load `<home>/relay.json`. Absent means no receiver is configured; a
    /// file that does not parse, or whose receiver or supplier list is not
    /// one the relay can act on, is an error naming the file and the fault.
    pub fn load(home: &Path) -> Result<Option<Self>, String> {
        let source = home.join("relay.json");
        if !source.exists() {
            return Ok(None);
        }
        let encoded = std::fs::read(&source)
            .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
        Self::parse(&encoded)
            .map(Some)
            .map_err(|error| format!("{} is not a valid relay config: {error}", source.display()))
    }

    /// Parse and check the bytes of a relay configuration. The receiver and
    /// the supplier list are checked together, so a receiver is never read
    /// from a file whose scope is malformed.
    pub fn parse(encoded: &[u8]) -> Result<Self, String> {
        let config: RelayConfig =
            serde_json::from_slice(encoded).map_err(|error| error.to_string())?;
        config.check()?;
        Ok(config)
    }

    /// Whether the relay can act on this configuration: a receiver it can
    /// post to as named ([`receiver_endpoint`]) and a supplier list that
    /// names suppliers. [`Self::parse`] and [`Self::store`] both hold a
    /// configuration to it, so nothing writes a file the relay would refuse.
    pub fn check(&self) -> Result<(), String> {
        receiver_endpoint(&self.receiver).map_err(|fault| fault.with_remedy())?;
        if self
            .suppliers
            .iter()
            .flatten()
            .any(|supplier| supplier.is_empty())
        {
            return Err("`suppliers` lists an empty name, which names no supplier".to_owned());
        }
        Ok(())
    }

    /// Write `<home>/relay.json` atomically, readable by the owner only:
    /// it carries the ingest key. The temporary file is this writer's own,
    /// so two `connect` runs cannot rename each other's file away. Written by
    /// enrolment only; the relay itself never writes its own configuration.
    /// A configuration [`Self::load`] would refuse is not written.
    pub fn store(&self, home: &Path) -> Result<(), String> {
        self.check()
            .map_err(|error| format!("relay config not written: {error}"))?;
        let encoded = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("serialise relay config: {error}"))?;
        declaration::replace_private(&home.join("relay.json"), &encoded)
    }
}

/// The URL the relay posts a batch to for `receiver`: the receiver with its
/// trailing slashes removed, then `/events`. The relay's transport derives
/// its request URL here and nowhere else, so [`same_receiver`] compares what
/// is actually reached.
pub fn events_url(receiver: &str) -> String {
    format!("{}/events", receiver.trim_end_matches('/'))
}

/// The endpoint a batch for `receiver` reaches: [`events_url`] parsed as
/// `commonmeasure-http` parses it, keeping what the transport sends (scheme,
/// host, port, path and query) and dropping what it does not (credentials
/// and fragment). The parse applies the transport's normalisations: scheme
/// and host case, an explicit default port, an empty path, dot segments,
/// IDNA host names and IPv4 and IPv6 address spellings. The host is then
/// read as a crossing's host is ([`crate::grounding::host_of`]), which
/// removes every terminal dot: a host written with its trailing root dot is
/// the same DNS name. The transport keeps the dots, and counting more
/// spellings as one receiver can only apply a scope, never lift one. `None`
/// where the transport cannot post: not a URL, not http or https, or no host.
fn posted_endpoint(receiver: &str) -> Option<url::Url> {
    let mut endpoint = url::Url::parse(&events_url(receiver)).ok()?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return None;
    }
    let host = endpoint.host_str().filter(|host| !host.is_empty())?;
    let read = crate::grounding::host_of(endpoint.as_str());
    if read != host && !read.is_empty() {
        endpoint.set_host(Some(&read)).ok()?;
    }
    endpoint.set_username("").ok()?;
    endpoint.set_password(None).ok()?;
    endpoint.set_fragment(None);
    Some(endpoint)
}

/// Why the relay refuses a configured receiver. The text names the fault
/// and, where the URL parses, its origin ([`receiver_origin`]); it never
/// carries the rest of the URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverFault {
    fault: String,
    /// What to change in `relay.json`, where the fault has a remedy there.
    remedy: Option<&'static str>,
}

impl ReceiverFault {
    /// The fault followed by its remedy in `relay.json`, as a load or store
    /// error reports it.
    pub fn with_remedy(&self) -> String {
        match self.remedy {
            Some(remedy) => format!("{}; {remedy}", self.fault),
            None => self.fault.clone(),
        }
    }
}

impl std::fmt::Display for ReceiverFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.fault)
    }
}

/// How an error or a status names a configured receiver: its origin
/// (scheme, host and port, e.g. `https://hub.example:8443`), or `None` where
/// the URL has no such origin. The rest of the URL can hold a key, in its
/// credentials, its query or a tokenised path, and these texts reach status
/// output, the source record and, through a reporting ruling, an agent's
/// context. No fault needs the path to locate the receiver.
pub fn receiver_origin(receiver: &str) -> Option<String> {
    url::Url::parse(receiver)
        .ok()
        .and_then(|url| origin_text(&url))
}

fn origin_text(url: &url::Url) -> Option<String> {
    let origin = url.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

/// Check a configured receiver, returning the endpoint it reaches.
///
/// Refused, because the relay cannot post to them as written: a scheme other
/// than http and https, no host, credentials in the URL (the transport never
/// sends them; the key belongs in `api_key`), and a query or fragment, which
/// the transport's `/events` suffix would land inside rather than on the
/// path. A value that does not parse is named by no part of itself, since
/// nothing in it can be told apart from a key.
pub fn receiver_endpoint(receiver: &str) -> Result<url::Url, ReceiverFault> {
    let fault = |fault: String, remedy| ReceiverFault { fault, remedy };
    let named = url::Url::parse(receiver)
        .map_err(|error| fault(format!("`receiver` is not a URL: {error}"), None))?;
    let at = match origin_text(&named) {
        Some(origin) => format!("the receiver at {origin}"),
        None => "the receiver".to_owned(),
    };
    if !matches!(named.scheme(), "http" | "https") {
        return Err(fault(format!("{at} is not an http or https URL"), None));
    }
    if named.host_str().is_none_or(str::is_empty) {
        return Err(fault(format!("{at} names no host"), None));
    }
    if !named.username().is_empty() || named.password().is_some() {
        return Err(fault(
            format!("{at} carries credentials in the URL"),
            Some("give the key as `api_key`"),
        ));
    }
    if named.query().is_some() || named.fragment().is_some() {
        return Err(fault(
            format!(
                "{at} has a query or fragment, and batches are posted to its path followed by \
                 /events"
            ),
            Some("give a key as `api_key`"),
        ));
    }
    posted_endpoint(receiver).ok_or_else(|| fault("`receiver` is not a URL".to_owned(), None))
}

/// Whether batches for two receiver URLs reach one endpoint
/// ([`posted_endpoint`]). A URL the transport cannot post to matches
/// nothing. The relay decides whether the configured supplier scope applies
/// to the receiver it delivers to, a `--receiver` override included, by this
/// function alone.
pub fn same_receiver(one: &str, other: &str) -> bool {
    matches!(
        (posted_endpoint(one), posted_endpoint(other)),
        (Some(one), Some(other)) if one == other
    )
}

/// A `suppliers` member as written, and the scope it parses to or a
/// fragment of the load error.
#[cfg(test)]
pub(crate) type SupplierCase = (
    &'static str,
    Result<Option<&'static [&'static str]>, &'static str>,
);

#[cfg(test)]
pub(crate) const SUPPLIER_TABLE: &[SupplierCase] = &[
    ("", Ok(None)),
    (r#","suppliers":null"#, Ok(None)),
    (r#","suppliers":[]"#, Ok(Some(&[]))),
    (r#","suppliers":["ozone"]"#, Ok(Some(&["ozone"]))),
    (
        r#","suppliers":["ozone","exa"]"#,
        Ok(Some(&["ozone", "exa"])),
    ),
    (
        r#","suppliers":"ozone""#,
        Err("invalid type: string \"ozone\""),
    ),
    (r#","suppliers":{"ozone":true}"#, Err("invalid type: map")),
    (
        r#","suppliers":["ozone",1]"#,
        Err("invalid type: integer `1`"),
    ),
    (r#","suppliers":[""]"#, Err("empty name")),
];

/// Refused receivers each holding the key `ak_PLANTED` somewhere a key is
/// put: in the credentials, in the query, in a tokenised path (refused here
/// for its query), in a value that does not parse, and in a URL that parses
/// but that the transport cannot post to. Each with a fragment of its fault
/// and the origin its error may name. No error, status or ruling that
/// reports the refusal may carry the key.
#[cfg(test)]
pub(crate) const PLANTED_RECEIVERS: &[(&str, &str, Option<&str>)] = &[
    (
        "https://ops:ak_PLANTED@hub.example:8443/api/v1/telemetry",
        "carries credentials",
        Some("https://hub.example:8443"),
    ),
    (
        "https://hub.example/api/v1/telemetry?api_key=ak_PLANTED",
        "query or fragment",
        Some("https://hub.example"),
    ),
    (
        "https://hub.example/hooks/ak_PLANTED/telemetry?tenant=a",
        "query or fragment",
        Some("https://hub.example"),
    ),
    ("hub.example/hooks/ak_PLANTED", "not a URL", None),
    ("https://r.1../hooks/ak_PLANTED", "not a URL", None),
    (
        "ftp://ak_PLANTED@hub.example/x",
        "not an http or https URL",
        Some("ftp://hub.example"),
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Two `connect` runs at once each store a relay configuration. Each
    /// writes through its own temporary file, so neither renames the other's
    /// away, and the file that lands is one of theirs, whole and owner-only.
    #[test]
    fn concurrent_stores_each_land_whole_and_owner_only() {
        let home = tempfile::tempdir().unwrap();
        let failures = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for n in 0..4 {
                let (home, failures) = (home.path(), &failures);
                scope.spawn(move || {
                    let config = RelayConfig {
                        receiver: "https://hub.example".to_owned(),
                        api_key: Some(format!("key-{n}-{}", "x".repeat(200 * n))),
                        suppliers: None,
                    };
                    for _ in 0..40 {
                        if let Err(error) = config.store(home) {
                            failures.lock().unwrap().push(error);
                        }
                    }
                });
            }
        });
        assert_eq!(failures.into_inner().unwrap(), Vec::<String>::new());
        let landed = RelayConfig::load(home.path()).unwrap().unwrap();
        assert!(landed.api_key.unwrap().starts_with("key-"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(home.path().join("relay.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let leftovers: Vec<String> = std::fs::read_dir(home.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    /// `store` holds a configuration to the rule `load` applies, so it never
    /// writes a file the relay would refuse, and one it does write loads.
    #[test]
    fn a_configuration_load_would_refuse_is_not_stored() {
        for (receiver, suppliers, fault) in [
            (
                "http://user@127.0.0.1:9/api/v1/telemetry",
                None,
                "credentials",
            ),
            ("https://hub.example/t?tenant=a", None, "query or fragment"),
            ("https://hub.example/t#x", None, "query or fragment"),
            ("hub.example/t", None, "not a URL"),
            (
                "https://hub.example/t",
                Some(vec![String::new()]),
                "empty name",
            ),
        ] {
            let home = tempfile::tempdir().unwrap();
            let config = RelayConfig {
                receiver: receiver.to_owned(),
                api_key: Some("ak".to_owned()),
                suppliers,
            };
            let error = config.store(home.path()).expect_err(receiver);
            assert!(error.contains(fault), "{receiver}: {error}");
            assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
        }
        let home = tempfile::tempdir().unwrap();
        RelayConfig {
            receiver: "https://hub.example/api/v1/telemetry".to_owned(),
            api_key: Some("ak".to_owned()),
            suppliers: Some(vec!["ozone".to_owned()]),
        }
        .store(home.path())
        .unwrap();
        let loaded = RelayConfig::load(home.path()).unwrap().unwrap();
        assert_eq!(loaded.suppliers, Some(vec!["ozone".to_owned()]));
    }

    /// A load error names the fault and the receiver's origin, never the
    /// rest of the URL: a key in the credentials, the query or the path
    /// stays out of the error, and so out of every status and ruling that
    /// reports it.
    #[test]
    fn a_refused_receiver_is_named_by_its_origin_alone() {
        for (receiver, fault, origin) in PLANTED_RECEIVERS {
            let home = tempfile::tempdir().unwrap();
            let file = serde_json::json!({ "receiver": receiver, "api_key": "ak" }).to_string();
            std::fs::write(home.path().join("relay.json"), file).unwrap();
            let error = RelayConfig::load(home.path()).expect_err(receiver);
            assert!(error.contains(fault), "{receiver}: {error}");
            assert!(!error.contains("ak_PLANTED"), "{receiver}: {error}");
            match origin {
                Some(origin) => {
                    assert!(error.contains(&format!("at {origin} ")), "{error}")
                }
                None => assert!(!error.contains("hub.example") && !error.contains("r.1")),
            }
            let direct = receiver_endpoint(receiver).expect_err(receiver).to_string();
            assert!(!direct.contains("ak_PLANTED"), "{receiver}: {direct}");
        }
        assert_eq!(
            receiver_origin("https://ops:ak@Hub.Example:443/p?q#f").as_deref(),
            Some("https://hub.example")
        );
        assert_eq!(
            receiver_origin("http://[::1]:9/p").as_deref(),
            Some("http://[::1]:9")
        );
        assert_eq!(receiver_origin("hub.example/p"), None);
    }

    #[test]
    fn the_supplier_list_parses_to_a_scope_or_an_error() {
        for (suppliers, expected) in SUPPLIER_TABLE {
            let file = format!(r#"{{"receiver":"https://receiver.example/v1"{suppliers}}}"#);
            let parsed = RelayConfig::parse(file.as_bytes()).map(|config| config.suppliers);
            match expected {
                Ok(scope) => {
                    let scope = scope.map(|names| names.iter().map(|n| n.to_string()).collect());
                    assert_eq!(parsed, Ok(scope), "{file}");
                }
                Err(fault) => {
                    let error = parsed.expect_err(&file);
                    assert!(error.contains(fault), "{file}: {error}");
                }
            }
        }
    }

    /// Each normalisation the transport applies, and the two this function
    /// adds, makes the spellings one receiver. Everything else that differs
    /// on the wire is another receiver.
    #[test]
    fn spellings_the_transport_posts_to_one_endpoint_are_one_receiver() {
        let base = "https://receiver.example/telemetry";
        for same in [
            "https://receiver.example/telemetry/",
            "https://receiver.example/telemetry//",
            "HTTPS://receiver.example/telemetry",
            "https://RECEIVER.Example/telemetry",
            "https://receiver.example:443/telemetry",
            "https://receiver.example/./telemetry",
            "https://receiver.example/x/../telemetry",
            "https://receiver.example./telemetry",
            " https://receiver.example/telemetry",
            // Sent by the transport without the credentials or the fragment.
            "https://user:pass@receiver.example/telemetry",
            "https://receiver.example/telemetry/events#x",
        ] {
            assert!(same_receiver(base, same), "{same}");
        }
        assert!(same_receiver(
            "https://receiver.example",
            "https://receiver.example/"
        ));
        assert!(same_receiver("http://127.0.0.1:80", "http://127.0.0.1/"));
        assert!(same_receiver("http://[0:0::1]:9/r", "http://[::1]:9/r/"));
        assert!(same_receiver("http://127.0.0.1:9/r", "http://127.1:9/r"));
        // A trailing root dot on the host: the transport resolves the same
        // name, rustls strips the dot from SNI and webpki matches the
        // certificate, so the dotted spelling reaches the same receiver.
        assert!(same_receiver(
            "https://receiver.example/",
            "https://receiver.example./"
        ));
        // More than one terminal dot is not a valid DNS name; counting it as
        // the same receiver applies the scope to one more spelling.
        assert!(same_receiver("https://r:8443/t", "https://r..:8443/t"));
        assert!(same_receiver("https://r.:8443/t", "https://r..:8443/t"));
        assert!(same_receiver(
            "https://bücher.example",
            "https://xn--bcher-kva.example"
        ));
        // A host of dots alone has no name left once its terminal dots are
        // removed, so it is kept as written: accepted, one receiver with
        // itself, and another receiver from every other host.
        let dotted = ["https://./t", "https://../t"];
        for host in dotted {
            assert!(receiver_endpoint(host).is_ok(), "{host}");
            assert!(same_receiver(host, host), "{host}");
            for other in ["https://r/t", "https://r./t", "https://.../t"] {
                assert!(!same_receiver(host, other), "{host} {other}");
            }
        }
        assert!(!same_receiver(dotted[0], dotted[1]));
        for other in [
            "http://receiver.example/telemetry",
            "https://receiver.example:8443/telemetry",
            "https://receiver.example/Telemetry",
            "https://receiver.example/telemetry/v2",
            "https://other.example/telemetry",
            // Posted to `/telemetry?tenant=a/events`.
            "https://receiver.example/telemetry?tenant=a",
        ] {
            assert!(!same_receiver(base, other), "{other}");
        }
    }

    #[test]
    fn a_receiver_the_relay_cannot_post_to_as_named_is_refused() {
        for (receiver, fault) in [
            (
                "https://receiver.example/telemetry?tenant=a",
                "query or fragment",
            ),
            ("https://receiver.example/telemetry?", "query or fragment"),
            (
                "https://receiver.example/telemetry#events",
                "query or fragment",
            ),
            (
                "https://user:pass@receiver.example/telemetry",
                "credentials",
            ),
            ("https://user@receiver.example/telemetry", "credentials"),
            (
                "ftp://receiver.example/telemetry",
                "not an http or https URL",
            ),
            ("file:///tmp/telemetry", "not an http or https URL"),
            ("receiver.example/telemetry", "not a URL"),
            ("", "not a URL"),
        ] {
            let error = receiver_endpoint(receiver).expect_err(receiver).to_string();
            assert!(error.contains(fault), "{receiver}: {error}");
            let file = serde_json::json!({ "receiver": receiver }).to_string();
            let error = RelayConfig::parse(file.as_bytes()).expect_err(receiver);
            assert!(error.contains(fault), "{receiver}: {error}");
        }
    }
}
