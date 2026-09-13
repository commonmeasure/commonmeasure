//! The Content Telemetry discovery manifest
//! (`/.well-known/content-telemetry.json`, standard section 8), read as a
//! consumer reads it (section 8.7).
//!
//! The manifest identifies an owner, agent or platform and names its
//! telemetry endpoint. It carries no demand: a reporting duty comes from a
//! licence or from the operator's terms, never from here (`DECISIONS.md`
//! §Source declarations). Discovery therefore never delays a crossing and is
//! written as its own record kind, so a probe can never project as a
//! retrieval.
//!
//! The consumer rules are implemented here rather than by a schema validator
//! at run time, because the standard's rules go past its schema (duplicate
//! key ids, foreign `domains`, any `1.x` version) and the runtime carries no
//! schema library. The pinned schema and the standard's own fixtures
//! (`schema/manifest.v1.json`, `schema/manifest-tests/`) hold this reader to
//! the standard in the test suite.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The path every manifest is served at, under a domain root or an operated
/// agent's prefix (section 8.1).
pub const WELL_KNOWN_PATH: &str = "/.well-known/content-telemetry.json";

/// The facts a verified manifest states, kept for the record. Keys are
/// counted and their ids kept; the key material itself stays in the cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFacts {
    pub schema_version: String,
    pub id: String,
    pub roles: Vec<String>,
    pub operator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conformance_level: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub identifier_prefixes: Vec<String>,
}

/// Read a manifest fetched from `manifest_url` under the consumer rules of
/// section 8.7 and the field rules of sections 8.2 to 8.6. `Err` is the
/// rejection reason, stated for the record.
pub fn read(manifest_url: &str, body: &[u8]) -> Result<ManifestFacts, String> {
    let document: Value =
        serde_json::from_slice(body).map_err(|error| format!("invalid JSON: {error}"))?;
    let object = document
        .as_object()
        .ok_or("the manifest is not a JSON object")?;

    // Version: accept any 1.x, reject 0.x and anything that is not a
    // major.minor string (section 8.7, by way of 5.7.4).
    let schema_version = string_field(object, "schema_version")?;
    let mut parts = schema_version.split('.');
    let major = parts.next().and_then(|part| part.parse::<u32>().ok());
    let minor = parts.next().and_then(|part| part.parse::<u32>().ok());
    match (major, minor, parts.next()) {
        (Some(1), Some(_), None) => {}
        _ => {
            return Err(format!(
                "schema_version {schema_version:?} is not a 1.x version this consumer accepts"
            ));
        }
    }

    // id: an https URL at the well-known location (section 8.2).
    let id = string_field(object, "id")?;
    let id_url =
        url::Url::parse(&id).map_err(|error| format!("id {id:?} is not a URL: {error}"))?;
    if id_url.scheme() != "https" || id_url.query().is_some() || id_url.fragment().is_some() {
        return Err(format!(
            "id {id:?} is not an https URL without query or fragment"
        ));
    }
    if !id_url.path().ends_with(WELL_KNOWN_PATH) {
        return Err(format!("id {id:?} is not at {WELL_KNOWN_PATH}"));
    }
    if id_url.host_str().is_none_or(|host| host.contains(' ')) {
        return Err(format!("id {id:?} names no host"));
    }
    // The id's host is the host that served it: what TLS and the well-known
    // location prove is control of the domain the manifest was fetched
    // from, after redirects, and a manifest claiming another domain's id
    // has not shown that.
    let fetched_host = url::Url::parse(manifest_url)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase()))
        .ok_or_else(|| format!("the manifest URL {manifest_url:?} names no host"))?;
    let id_host = id_url
        .host_str()
        .map(|host| host.to_ascii_lowercase())
        .unwrap_or_default();
    if id_host != fetched_host {
        return Err(format!(
            "id host {id_host:?} is not the host {fetched_host:?} the manifest was fetched from"
        ));
    }

    // roles: a non-empty set from the three defined (section 8.2).
    let roles: Vec<String> = object
        .get("roles")
        .and_then(Value::as_array)
        .ok_or("roles is missing or not an array")?
        .iter()
        .map(|role| {
            role.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "roles carries a value that is not a string".to_owned())
        })
        .collect::<Result<_, _>>()?;
    if roles.is_empty() {
        return Err("roles is empty; at least one role is required".to_owned());
    }
    for (index, role) in roles.iter().enumerate() {
        if !matches!(role.as_str(), "content_owner" | "agent" | "platform") {
            return Err(format!(
                "role {role:?} is not one of content_owner, agent, platform"
            ));
        }
        if roles[..index].contains(role) {
            return Err(format!("role {role:?} is repeated; roles are a set"));
        }
    }

    // operator.name (section 8.3).
    let operator = object
        .get("operator")
        .and_then(Value::as_object)
        .ok_or("operator is missing")?;
    let operator_name = string_field(operator, "name")?;
    let operator_domain = operator
        .get("domain")
        .and_then(Value::as_str)
        .map(str::to_owned);

    // keys: id, type Ed25519 and publicKey each required; ids unique
    // (sections 8.4 and 8.7).
    let mut key_ids = Vec::new();
    if let Some(keys) = object.get("keys") {
        let keys = keys.as_array().ok_or("keys is not an array")?;
        for key in keys {
            let key = key.as_object().ok_or("a keys entry is not an object")?;
            let key_id =
                string_field(key, "id").map_err(|_| "a keys entry has no id".to_owned())?;
            let key_type =
                string_field(key, "type").map_err(|_| format!("key {key_id:?} has no type"))?;
            if key_type != "Ed25519" {
                return Err(format!(
                    "key {key_id:?} has type {key_type:?}; v1 defines Ed25519 only"
                ));
            }
            string_field(key, "publicKey")
                .map_err(|_| format!("key {key_id:?} has no publicKey"))?;
            if key_ids.contains(&key_id) {
                return Err(format!("duplicate keys[].id {key_id:?}"));
            }
            key_ids.push(key_id);
        }
    }

    // telemetry: an https endpoint, a known conformance level, an https
    // ctx_resolution on agent and platform manifests only, coverage modes
    // from the defined set (section 8.5).
    let mut telemetry_endpoint = None;
    let mut conformance_level = None;
    if let Some(telemetry) = object.get("telemetry") {
        let telemetry = telemetry.as_object().ok_or("telemetry is not an object")?;
        let endpoint = string_field(telemetry, "endpoint")
            .map_err(|_| "telemetry has no endpoint".to_owned())?;
        require_https("telemetry.endpoint", &endpoint)?;
        telemetry_endpoint = Some(endpoint);
        if let Some(level) = telemetry.get("conformance_level") {
            let level = level
                .as_str()
                .ok_or("telemetry.conformance_level is not a string")?;
            if !matches!(level, "retrieval" | "grounding" | "citation") {
                return Err(format!(
                    "telemetry.conformance_level {level:?} is not one of retrieval, grounding, \
                     citation"
                ));
            }
            conformance_level = Some(level.to_owned());
        }
        if let Some(resolution) = telemetry.get("ctx_resolution") {
            let resolution = resolution
                .as_str()
                .ok_or("telemetry.ctx_resolution is not a string")?;
            require_https("telemetry.ctx_resolution", resolution)?;
            if !roles
                .iter()
                .any(|role| role == "agent" || role == "platform")
            {
                return Err(
                    "telemetry.ctx_resolution is valid on agent and platform manifests only"
                        .to_owned(),
                );
            }
        }
        if let Some(coverage) = telemetry.get("coverage") {
            let coverage = coverage
                .as_object()
                .ok_or("telemetry.coverage is not an object")?;
            for (event, entry) in coverage {
                let mode = entry
                    .get("mode")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("telemetry.coverage.{event} has no mode"))?;
                if !matches!(mode, "complete" | "sampled" | "aggregated" | "selected") {
                    return Err(format!(
                        "telemetry.coverage.{event} mode {mode:?} is not one of complete, \
                         sampled, aggregated, selected"
                    ));
                }
            }
        }
    }

    // domains: only on a root manifest, each the manifest's own host or a
    // subdomain of it, literal or wildcard (sections 8.6 and 8.7).
    let mut domains = Vec::new();
    if let Some(declared) = object.get("domains") {
        let declared = declared.as_array().ok_or("domains is not an array")?;
        if id_url.path() != WELL_KNOWN_PATH {
            return Err("only root manifests carry domains".to_owned());
        }
        for domain in declared {
            let domain = domain
                .as_str()
                .ok_or("a domains entry is not a string")?
                .to_ascii_lowercase();
            let bare = domain.strip_prefix("*.").unwrap_or(&domain);
            let own = bare == fetched_host || bare.ends_with(&format!(".{fetched_host}"));
            if !own {
                return Err(format!(
                    "domains entry {domain:?} is not the manifest host {fetched_host:?} or a \
                     subdomain of it"
                ));
            }
            domains.push(domain);
        }
    }

    // identifier_schemes: on content_owner manifests only, each prefix
    // non-empty, resolution https (section 8.6).
    let mut identifier_prefixes = Vec::new();
    if let Some(schemes) = object.get("identifier_schemes") {
        let schemes = schemes
            .as_array()
            .ok_or("identifier_schemes is not an array")?;
        if !roles.iter().any(|role| role == "content_owner") {
            return Err("identifier_schemes only on content_owner manifests".to_owned());
        }
        for scheme in schemes {
            let scheme = scheme
                .as_object()
                .ok_or("an identifier_schemes entry is not an object")?;
            let prefix = string_field(scheme, "prefix")
                .map_err(|_| "an identifier_schemes entry has no prefix".to_owned())?;
            if prefix.is_empty() {
                return Err("an identifier_schemes prefix is empty".to_owned());
            }
            if let Some(resolution) = scheme.get("resolution") {
                let resolution = resolution
                    .as_str()
                    .ok_or("an identifier_schemes resolution is not a string")?;
                require_https("identifier_schemes[].resolution", resolution)?;
            }
            identifier_prefixes.push(prefix);
        }
    }

    Ok(ManifestFacts {
        schema_version,
        id,
        roles,
        operator: operator_name,
        operator_domain,
        telemetry_endpoint,
        conformance_level,
        domains,
        key_ids,
        identifier_prefixes,
    })
}

fn string_field(object: &serde_json::Map<String, Value>, name: &str) -> Result<String, String> {
    object
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{name} is missing or not a string"))
}

fn require_https(field: &str, value: &str) -> Result<(), String> {
    if value.starts_with("https://") {
        Ok(())
    } else {
        Err(format!("{field} {value:?} is not an https URL"))
    }
}

/// The manifest URL for a page: the well-known path at the page's origin.
pub fn well_known_url(page_url: &str) -> Option<String> {
    let mut parsed = url::Url::parse(page_url).ok()?;
    parsed.set_path(WELL_KNOWN_PATH);
    parsed.set_query(None);
    parsed.set_fragment(None);
    Some(parsed.to_string())
}

/// The apex to try when a subdomain answers 404: the host with its first
/// label removed, while at least two labels remain. Without a public suffix
/// list this is one step up and no more; the record names the URL tried.
pub fn apex_url(manifest_url: &str) -> Option<String> {
    let mut parsed = url::Url::parse(manifest_url).ok()?;
    let host = parsed.host_str()?.to_owned();
    if parsed
        .host()
        .is_some_and(|host| !matches!(host, url::Host::Domain(_)))
    {
        return None;
    }
    let (_, parent) = host.split_once('.')?;
    if !parent.contains('.') {
        return None;
    }
    parsed.set_host(Some(parent)).ok()?;
    Some(parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_well_known_url_is_at_the_origin_and_the_apex_is_one_label_up() {
        assert_eq!(
            well_known_url("https://news.example.com/a/b?q").as_deref(),
            Some("https://news.example.com/.well-known/content-telemetry.json")
        );
        assert_eq!(
            apex_url("https://news.example.com/.well-known/content-telemetry.json").as_deref(),
            Some("https://example.com/.well-known/content-telemetry.json")
        );
        assert!(
            apex_url("https://example.com/.well-known/content-telemetry.json").is_none(),
            "a two-label host has no apex above it this reader will try"
        );
        assert!(
            apex_url("http://127.0.0.1:8/.well-known/content-telemetry.json").is_none(),
            "an address has no apex"
        );
    }

    #[test]
    fn an_id_on_another_host_than_the_one_fetched_from_is_rejected() {
        let body = br#"{"schema_version":"1.0","id":"https://publisher.example/.well-known/content-telemetry.json","roles":["content_owner"],"operator":{"name":"Example"}}"#;
        assert!(
            read(
                "https://publisher.example/.well-known/content-telemetry.json",
                body
            )
            .is_ok()
        );
        assert!(
            read(
                "https://PUBLISHER.example:8443/.well-known/content-telemetry.json",
                body
            )
            .is_ok(),
            "the comparison is on the host alone, case folded"
        );
        let rejected = read(
            "https://mirror.example/.well-known/content-telemetry.json",
            body,
        )
        .expect_err("another host's id");
        assert!(rejected.contains("publisher.example"), "{rejected}");
    }

    #[test]
    fn a_later_minor_version_is_accepted_and_a_zero_version_is_not() {
        let body = |version: &str| {
            format!(
                r#"{{"schema_version":"{version}","id":"https://example.com/.well-known/content-telemetry.json","roles":["content_owner"],"operator":{{"name":"Example"}}}}"#
            )
        };
        let url = "https://example.com/.well-known/content-telemetry.json";
        assert!(read(url, body("1.3").as_bytes()).is_ok());
        assert!(read(url, body("0.9").as_bytes()).is_err());
        assert!(read(url, body("2.0").as_bytes()).is_err());
    }
}
