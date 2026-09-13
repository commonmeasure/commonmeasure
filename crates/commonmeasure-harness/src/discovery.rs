//! Reading a source's declarations for one page: the per-host cache of
//! `robots.txt` and licence documents, and the record a crossing carries.
//!
//! `robots.txt` is fetched once per host and cached for 24 hours, the age the
//! attachment draft and RFC 9309 allow; a licence document is cached with it
//! under its URL. A probe that failed is cached for five minutes, so a host
//! that is down is not asked again on every crossing and the failure is
//! still on each record. The cache is bytes and dates only; what they mean
//! is computed for each page, because `robots.txt` rules depend on the path.
//!
//! Every outbound request goes through the prober the caller supplies, which
//! is the mediated fetch path with its host policy and address floor
//! (`DECISIONS.md` §Source declarations). Nothing here resolves a name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::declarations::{
    self, Category, Effective, LicenceTerms, PRODUCT_TOKEN, RobotsReading, Statement,
    StatementSource,
};
use crate::policy::TermsDeclaration;

/// How long a `robots.txt` copy is used before it is fetched again.
pub const ROBOTS_CACHE_AGE: chrono::Duration = chrono::Duration::hours(24);
/// How long a failed probe is remembered before the host is asked again.
pub const FAILURE_CACHE_AGE: chrono::Duration = chrono::Duration::minutes(5);
/// The exchange budget for a probe. Shorter than a page fetch: a probe is
/// overhead on a crossing, and a slow host should cost the crossing little.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// The most of a `robots.txt` or licence body kept. Larger bodies are
/// recorded as too large and read as unavailable.
const MAX_PROBE_BODY: usize = 512 * 1024;

/// One probe as cached: the URL asked for, what answered, and until when the
/// answer is used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    pub url: String,
    pub fetched_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// The URL that answered, after redirects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// The body as text, for a 2xx answer within the size bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Why there is no body: a transport failure, a policy refusal of the
    /// probe's host, or a body over the bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Everything cached for one host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub robots: Option<Probe>,
    /// Licence documents by URL.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub licences: BTreeMap<String, Probe>,
}

/// The cache directory, `<home>/declarations/`, one JSON file per host.
pub struct DeclarationCache {
    dir: PathBuf,
}

/// A fetch of one URL through the mediated path: the final URL and the
/// response, or the reason nothing was fetched (a refusal or a failure).
pub type Prober<'a> = &'a dyn Fn(&str) -> Result<(String, commonmeasure_http::Response), String>;

impl DeclarationCache {
    pub fn open(home: &Path) -> Self {
        Self {
            dir: home.join("declarations"),
        }
    }

    fn path_for(&self, host: &str) -> PathBuf {
        // Hosts are DNS names or IP literals; the characters outside that
        // set, brackets and colons of an IPv6 literal, are folded so the name
        // is a plain file name.
        let name: String = host
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{name}.json"))
    }

    pub fn load(&self, host: &str) -> HostRecord {
        std::fs::read(self.path_for(host))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Best-effort: a cache that cannot be written costs a repeated probe,
    /// never a crossing. Written whole and renamed into place, so two
    /// servers on one home never leave a reader half a file.
    pub fn save(&self, host: &str, record: &HostRecord) {
        let _ = std::fs::create_dir_all(&self.dir);
        if let Ok(bytes) = serde_json::to_vec_pretty(record) {
            let _ = crate::declaration::replace(&self.path_for(host), &bytes);
        }
    }
}

/// Use the cached probe of `url` while it is live, otherwise fetch it. The
/// second value says which happened, for the record.
fn cached_probe(
    slot: &mut Option<Probe>,
    url: &str,
    now: DateTime<Utc>,
    probe: Prober<'_>,
) -> (Probe, CacheDecision) {
    if let Some(existing) = slot.as_ref()
        && existing.url == url
        && existing.expires_at > now
    {
        return (existing.clone(), CacheDecision::Reused);
    }
    let fresh = match probe(url) {
        // An answer outside 2xx and 404 is a failure of the probe, not a
        // statement by the host: a 503 is remembered for the failure age
        // and asked again, never for the day a file would be.
        Ok((final_url, response))
            if !(200..300).contains(&response.status) && response.status != 404 =>
        {
            Probe {
                url: url.to_owned(),
                fetched_at: now,
                expires_at: now + FAILURE_CACHE_AGE,
                final_url: Some(final_url),
                status: Some(response.status),
                body: None,
                error: Some(format!("{url} answered {}", response.status)),
            }
        }
        Ok((final_url, response)) => {
            let ok = (200..300).contains(&response.status);
            let (body, error) = if !ok {
                (None, None)
            } else if response.body.len() > MAX_PROBE_BODY {
                (
                    None,
                    Some(format!(
                        "the body is {} bytes, over the {MAX_PROBE_BODY} byte bound this reader \
                         keeps",
                        response.body.len()
                    )),
                )
            } else {
                (
                    Some(String::from_utf8_lossy(&response.body).into_owned()),
                    None,
                )
            };
            let age = declarations::max_age(response.headers.get("Cache-Control"))
                .map(|seconds| chrono::Duration::seconds(seconds.min(i64::MAX as u64) as i64))
                .unwrap_or(ROBOTS_CACHE_AGE)
                .min(ROBOTS_CACHE_AGE);
            Probe {
                url: url.to_owned(),
                fetched_at: now,
                expires_at: now + age,
                final_url: Some(final_url),
                status: Some(response.status),
                body,
                error,
            }
        }
        Err(error) => Probe {
            url: url.to_owned(),
            fetched_at: now,
            expires_at: now + FAILURE_CACHE_AGE,
            final_url: None,
            status: None,
            body: None,
            error: Some(error),
        },
    };
    *slot = Some(fresh.clone());
    (fresh, CacheDecision::Fetched)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheDecision {
    Fetched,
    Reused,
}

/// What `robots.txt` said for this page, as recorded on the crossing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobotsOutcome {
    pub url: String,
    pub cache: CacheDecision,
    pub fetched_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Why the file could not be read. A 404 is not an error: the host
    /// publishes no rules, and the reading below is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<RobotsReading>,
}

/// How a licence document was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LicenceMechanism {
    /// A `License:` directive in the selected `robots.txt` group, or a
    /// global one.
    RobotsLicense,
    /// A `Link: rel="license"; type="application/rsl+xml"` header on the
    /// page response.
    LinkHeader,
}

/// One licence document as read for this page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LicenceOutcome {
    pub url: String,
    pub mechanism: LicenceMechanism,
    pub cache: CacheDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Why no terms were read: the probe failed, the document did not parse,
    /// or no `<content>` entry matched the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// The `url` of the `<content>` entry that matched the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms: Option<LicenceTerms>,
}

/// What decided the AI-input question for this crossing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Governing {
    /// Terms the operator declared for the host: a contract overrides a
    /// preference (vocabulary draft section 5.2). Any statement is recorded
    /// and none is enforced.
    OperatorTerms,
    /// The combined statements.
    Statements,
}

/// The declarations record on a mediated crossing: what was read, from
/// where, and what it adds up to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Declarations {
    pub robots: RobotsOutcome,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub licences: Vec<LicenceOutcome>,
    /// The page response's `Content-Usage` header as received, once the page
    /// has been fetched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_usage_header: Option<String>,
    /// Every statement gathered, with its source.
    pub statements: Vec<Statement>,
    /// Most-restrictive-wins per category over `statements`.
    pub effective: BTreeMap<Category, Effective>,
    /// The operator's terms for the host, where declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms: Option<TermsDeclaration>,
    pub governing: Governing,
    /// How a telemetry reporting demand was ruled on, where the licence or
    /// the terms carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting: Option<ReportingRuling>,
}

/// The ruling on a reporting demand: what was demanded, the receiver the
/// session would report through, and whether the demand is met. A demand is
/// met only where the profile is the Content Telemetry binding, the level
/// is one the relay emits, the session's scope clears telemetry egress and
/// a receiver is configured; `reason` names the first of those that fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportingRuling {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conformance_level: Option<String>,
    /// The receiver named in `relay.json`, or absent when none is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    pub telemetry_egress_cleared: bool,
    pub met: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Declarations {
    /// The combined preference for AI input, which is the use a mediated
    /// fetch makes of a page.
    pub fn ai_input(&self) -> Effective {
        self.effective
            .get(&Category::AiInput)
            .copied()
            .unwrap_or(Effective::Unknown)
    }

    /// The statements that disallow AI input, for a refusal or a breach to
    /// name.
    pub fn ai_input_disallowed_by(&self) -> Vec<&Statement> {
        self.statements
            .iter()
            .filter(|statement| {
                statement.category == Category::AiInput
                    && statement.preference == declarations::Preference::Disallow
            })
            .collect()
    }

    /// The licence terms that govern AI input: the page-level licence from
    /// the `Link` header where one was read, else the site-level one from
    /// `robots.txt` (RSL section 4.9, the more specific licence first).
    pub fn licence_terms(&self) -> Option<(&LicenceOutcome, &LicenceTerms)> {
        let ranked = |mechanism: LicenceMechanism| {
            self.licences
                .iter()
                .filter(move |licence| licence.mechanism == mechanism)
                .find_map(|licence| licence.terms.as_ref().map(|terms| (licence, terms)))
        };
        ranked(LicenceMechanism::LinkHeader).or_else(|| ranked(LicenceMechanism::RobotsLicense))
    }

    fn recombine(&mut self) {
        self.effective = declarations::combine(&self.statements);
    }
}

/// Read what a host declares for one page before the page is fetched:
/// `robots.txt` for the product token's group, and the RSL licence the file
/// names, if any.
pub fn before_fetch(
    cache: &DeclarationCache,
    page_url: &str,
    terms: Option<&TermsDeclaration>,
    now: DateTime<Utc>,
    probe: Prober<'_>,
) -> Declarations {
    let host = crate::grounding::host_of(page_url);
    let mut record = cache.load(&host);
    let robots_url = robots_url_of(page_url);
    let (robots_probe, cache_decision) = cached_probe(&mut record.robots, &robots_url, now, probe);
    let path = declarations::request_target(page_url);
    let (reading, unavailable) = match (&robots_probe.body, &robots_probe.status) {
        (Some(body), _) => (
            Some(declarations::parse_robots(body).read(PRODUCT_TOKEN, &path)),
            None,
        ),
        (None, Some(status)) if *status == 404 => (
            Some(declarations::parse_robots("").read(PRODUCT_TOKEN, &path)),
            None,
        ),
        (None, Some(status)) => (
            None,
            Some(
                robots_probe
                    .error
                    .clone()
                    .unwrap_or_else(|| format!("{robots_url} answered {status}")),
            ),
        ),
        (None, None) => (
            None,
            Some(
                robots_probe
                    .error
                    .clone()
                    .unwrap_or_else(|| "no answer".to_owned()),
            ),
        ),
    };
    let mut statements = reading
        .as_ref()
        .map(|reading| reading.statements.clone())
        .unwrap_or_default();
    let mut licences = Vec::new();
    // The first candidate licence in the applicable scope. RSL lets a client
    // retrieve each listed document; one is read here, and the record names
    // which.
    // RSL requires the directive's value to be absolute; a relative one is
    // resolved against the file it was read from rather than dropped, and
    // the record carries the resolved URL.
    if let Some(licence_url) = reading
        .as_ref()
        .and_then(|reading| reading.licences.first().cloned())
        .and_then(|licence| {
            url::Url::parse(&robots_url)
                .and_then(|base| base.join(&licence))
                .ok()
                .map(|joined| joined.to_string())
        })
    {
        let outcome = read_licence(
            &mut record,
            &licence_url,
            LicenceMechanism::RobotsLicense,
            page_url,
            now,
            probe,
        );
        if let Some(terms) = &outcome.terms {
            statements.extend(terms.statements.iter().cloned());
        }
        licences.push(outcome);
    }
    cache.save(&host, &record);
    let mut declarations = Declarations {
        robots: RobotsOutcome {
            url: robots_url,
            cache: cache_decision,
            fetched_at: robots_probe.fetched_at,
            expires_at: robots_probe.expires_at,
            status: robots_probe.status,
            unavailable,
            reading,
        },
        licences,
        content_usage_header: None,
        statements,
        effective: BTreeMap::new(),
        terms: terms.cloned(),
        governing: if terms.is_some() {
            Governing::OperatorTerms
        } else {
            Governing::Statements
        },
        reporting: None,
    };
    declarations.recombine();
    declarations
}

/// Add what the page response itself carried: the `Content-Usage` header and
/// a `Link` header naming an RSL licence.
pub fn after_fetch(
    cache: &DeclarationCache,
    declarations: &mut Declarations,
    page_url: &str,
    response: &commonmeasure_http::Response,
    now: DateTime<Utc>,
    probe: Prober<'_>,
) {
    if let Some(header) = response.headers.get("Content-Usage") {
        declarations.content_usage_header = Some(header.to_owned());
        for (category, preference, label) in declarations::parse_content_usage(header) {
            declarations.statements.push(Statement {
                source: StatementSource::ContentUsageHeader,
                category,
                preference,
                detail: format!("Content-Usage: {label}"),
            });
        }
    }
    if let Some(licence_url) = response
        .headers
        .get("Link")
        .and_then(|link| declarations::rsl_link(link, page_url))
        && !declarations
            .licences
            .iter()
            .any(|licence| licence.url == licence_url)
    {
        let host = crate::grounding::host_of(page_url);
        let mut record = cache.load(&host);
        let outcome = read_licence(
            &mut record,
            &licence_url,
            LicenceMechanism::LinkHeader,
            page_url,
            now,
            probe,
        );
        cache.save(&host, &record);
        if let Some(terms) = &outcome.terms {
            declarations
                .statements
                .extend(terms.statements.iter().cloned());
        }
        declarations.licences.push(outcome);
    }
    declarations.recombine();
}

fn read_licence(
    record: &mut HostRecord,
    licence_url: &str,
    mechanism: LicenceMechanism,
    page_url: &str,
    now: DateTime<Utc>,
    probe: Prober<'_>,
) -> LicenceOutcome {
    let mut slot = record.licences.remove(licence_url);
    let (licence_probe, cache_decision) = cached_probe(&mut slot, licence_url, now, probe);
    if let Some(probe) = slot {
        record.licences.insert(licence_url.to_owned(), probe);
    }
    let mut outcome = LicenceOutcome {
        url: licence_url.to_owned(),
        mechanism,
        cache: cache_decision,
        status: licence_probe.status,
        unavailable: None,
        content: None,
        terms: None,
    };
    let Some(body) = &licence_probe.body else {
        outcome.unavailable =
            Some(
                licence_probe
                    .error
                    .clone()
                    .unwrap_or_else(|| match licence_probe.status {
                        Some(status) => format!("{licence_url} answered {status}"),
                        None => "no answer".to_owned(),
                    }),
            );
        return outcome;
    };
    match declarations::parse_rsl(body) {
        Ok(document) => match document.content_for(page_url) {
            Some(content) => {
                outcome.content = Some(content.url.clone());
                outcome.terms = Some(declarations::licence_terms(content, licence_url));
            }
            None => {
                outcome.unavailable =
                    Some("no <content> entry in the licence matches this page".to_owned());
            }
        },
        Err(error) => outcome.unavailable = Some(error),
    }
    outcome
}

/// `robots.txt` at the page's origin: same scheme, host and port.
pub fn robots_url_of(page_url: &str) -> String {
    match url::Url::parse(page_url) {
        Ok(mut parsed) => {
            parsed.set_path("/robots.txt");
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_) => format!("{page_url}/robots.txt"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_robots_url_is_the_origins() {
        assert_eq!(
            robots_url_of("https://example.com/a/b?q=1#f"),
            "https://example.com/robots.txt"
        );
        assert_eq!(
            robots_url_of("http://127.0.0.1:4711/doc"),
            "http://127.0.0.1:4711/robots.txt"
        );
    }

    /// A 503 is a failed probe: remembered for the failure age, then asked
    /// again; the answer that follows is read. A 404 is an answer.
    #[test]
    fn a_non_2xx_non_404_probe_is_cached_for_the_failure_age_and_retried() {
        use std::cell::Cell;
        let home = tempfile::tempdir().unwrap();
        let cache = DeclarationCache::open(home.path());
        let asked = Cell::new(0);
        let probe = |url: &str| -> Result<(String, commonmeasure_http::Response), String> {
            asked.set(asked.get() + 1);
            if url.ends_with("/robots.txt") {
                if asked.get() == 1 {
                    return Ok((
                        url.to_owned(),
                        commonmeasure_http::Response::text(503, "busy"),
                    ));
                }
                return Ok((
                    url.to_owned(),
                    commonmeasure_http::Response::text(
                        200,
                        "User-agent: *\nContent-Signal: ai-input=no\n",
                    ),
                ));
            }
            Err("not asked".to_owned())
        };
        let start = Utc::now();
        let first = before_fetch(&cache, "https://host.example/a", None, start, &probe);
        assert_eq!(first.robots.status, Some(503));
        assert!(first.robots.unavailable.as_deref().unwrap().contains("503"));
        assert_eq!(first.robots.expires_at, start + FAILURE_CACHE_AGE);
        assert_eq!(
            first.ai_input(),
            Effective::Unknown,
            "a failure states nothing"
        );

        let soon = before_fetch(
            &cache,
            "https://host.example/b",
            None,
            start + chrono::Duration::minutes(1),
            &probe,
        );
        assert_eq!(soon.robots.cache, CacheDecision::Reused);
        assert_eq!(
            asked.get(),
            1,
            "the failure is not asked again within its age"
        );

        let later = before_fetch(
            &cache,
            "https://host.example/c",
            None,
            start + chrono::Duration::minutes(6),
            &probe,
        );
        assert_eq!(later.robots.cache, CacheDecision::Fetched);
        assert_eq!(later.robots.status, Some(200));
        assert_eq!(later.ai_input(), Effective::Disallow);
        assert_eq!(asked.get(), 2);
    }

    #[test]
    fn a_host_record_round_trips_through_the_cache_directory() {
        let home = tempfile::tempdir().unwrap();
        let cache = DeclarationCache::open(home.path());
        let now = Utc::now();
        let record = HostRecord {
            robots: Some(Probe {
                url: "https://example.com/robots.txt".to_owned(),
                fetched_at: now,
                expires_at: now + ROBOTS_CACHE_AGE,
                final_url: Some("https://example.com/robots.txt".to_owned()),
                status: Some(200),
                body: Some("User-agent: *\nAllow: /\n".to_owned()),
                error: None,
            }),
            licences: BTreeMap::new(),
        };
        cache.save("example.com", &record);
        assert_eq!(cache.load("example.com"), record);
        assert_eq!(cache.load("[::1]"), HostRecord::default());
    }
}

// ---------------------------------------------------------------------------
// The Content Telemetry discovery manifest.

/// How long a manifest answer is used when the response names no cache age.
/// The standard tells hosts to use one hour during onboarding
/// (section 8.7); a host that names none is treated as one still onboarding.
pub const MANIFEST_DEFAULT_AGE: chrono::Duration = chrono::Duration::hours(1);
/// How long a 404 is remembered: the host publishes no manifest today, and
/// asking again each crossing would make every fetch pay for the absence.
pub const MANIFEST_NOT_FOUND_AGE: chrono::Duration = chrono::Duration::hours(24);
/// The longest a manifest's own cache age is honoured.
pub const MANIFEST_MAX_AGE: chrono::Duration = chrono::Duration::days(7);

/// What discovery established for one host, as cached and as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ManifestOutcome {
    /// A manifest was read and passed the consumer rules. Boxed: the facts
    /// are most of the record, and the other variants are one string.
    Verified {
        facts: Box<crate::manifest::ManifestFacts>,
    },
    /// The host answered 404 at the well-known path (and at the apex where
    /// one was tried): the participant is unverified and nothing is rejected
    /// on that basis (section 8.7).
    NotPublished,
    /// A document arrived and the consumer rules reject it. The participant
    /// is unverified.
    Rejected { reason: String },
    /// No answer, or an answer that is neither a document nor a 404: a
    /// transport failure, a policy refusal of the probe, or another status.
    Unavailable { reason: String },
}

/// One probe of a manifest URL, cached per host with the outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestProbe {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestRecord {
    pub host: String,
    pub fetched_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// The cache age the response named, in seconds, where it named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<u64>,
    /// The URLs asked, in order: the well-known path at the host, then the
    /// apex when the host answered 404 and has one.
    pub probes: Vec<ManifestProbe>,
    #[serde(flatten)]
    pub outcome: ManifestOutcome,
}

/// The manifest cache, `<home>/manifests/`, one JSON file per host.
pub struct ManifestCache {
    dir: PathBuf,
}

impl ManifestCache {
    pub fn open(home: &Path) -> Self {
        Self {
            dir: home.join("manifests"),
        }
    }

    fn path_for(&self, host: &str) -> PathBuf {
        let name: String = host
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{name}.json"))
    }

    fn load(&self, host: &str) -> Option<ManifestRecord> {
        std::fs::read(self.path_for(host))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    fn save(&self, host: &str, record: &ManifestRecord) {
        let _ = std::fs::create_dir_all(&self.dir);
        if let Ok(bytes) = serde_json::to_vec_pretty(record) {
            let _ = crate::declaration::replace(&self.path_for(host), &bytes);
        }
    }

    /// Whether a live cache entry says this host published a manifest that
    /// passed the consumer rules: the state a fetch consults to decide
    /// whether the host is a telemetry participant worth a correlation id.
    pub fn verified(&self, host: &str, now: DateTime<Utc>) -> bool {
        self.load(host).is_some_and(|record| {
            record.expires_at > now && matches!(record.outcome, ManifestOutcome::Verified { .. })
        })
    }
}

/// What a crossing records about the manifest of the host it reached: the
/// cached or fresh outcome and which it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestResolution {
    pub cache: CacheDecision,
    #[serde(flatten)]
    pub record: ManifestRecord,
}

/// Resolve the manifest for the host of `page_url`, after the page was
/// fetched: from the cache while it is live, otherwise by probing the
/// well-known path and, on a 404 at a subdomain, the apex. Every probe goes
/// through the caller's mediated path.
pub fn resolve_manifest(
    cache: &ManifestCache,
    page_url: &str,
    now: DateTime<Utc>,
    probe: Prober<'_>,
) -> Option<ManifestResolution> {
    let host = crate::grounding::host_of(page_url);
    if host.is_empty() {
        return None;
    }
    if let Some(record) = cache.load(&host)
        && record.expires_at > now
    {
        return Some(ManifestResolution {
            cache: CacheDecision::Reused,
            record,
        });
    }
    let first_url = crate::manifest::well_known_url(page_url)?;
    let mut probes = Vec::new();
    let mut max_age = None;
    let mut candidate = Some(first_url);
    let mut outcome = ManifestOutcome::NotPublished;
    let mut age = MANIFEST_NOT_FOUND_AGE;
    while let Some(url) = candidate.take() {
        match probe(&url) {
            Ok((final_url, response)) => {
                probes.push(ManifestProbe {
                    url: url.clone(),
                    status: Some(response.status),
                });
                if response.status == 404 {
                    // A subdomain may be claimed by the apex manifest
                    // (section 8.6), so the apex is asked once.
                    candidate = crate::manifest::apex_url(&url);
                    continue;
                }
                if !(200..300).contains(&response.status) {
                    outcome = ManifestOutcome::Unavailable {
                        reason: format!("{url} answered {}", response.status),
                    };
                    age = FAILURE_CACHE_AGE;
                    break;
                }
                let named = declarations::max_age(response.headers.get("Cache-Control"));
                max_age = named;
                age = named
                    .map(|seconds| chrono::Duration::seconds(seconds.min(i64::MAX as u64) as i64))
                    .unwrap_or(MANIFEST_DEFAULT_AGE)
                    .min(MANIFEST_MAX_AGE);
                outcome = match crate::manifest::read(&final_url, &response.body) {
                    Ok(facts) => ManifestOutcome::Verified {
                        facts: Box::new(facts),
                    },
                    Err(reason) => ManifestOutcome::Rejected { reason },
                };
                break;
            }
            Err(reason) => {
                probes.push(ManifestProbe { url, status: None });
                outcome = ManifestOutcome::Unavailable { reason };
                age = FAILURE_CACHE_AGE;
                break;
            }
        }
    }
    let record = ManifestRecord {
        host: host.clone(),
        fetched_at: now,
        expires_at: now + age,
        max_age,
        probes,
        outcome,
    };
    cache.save(&host, &record);
    Some(ManifestResolution {
        cache: CacheDecision::Fetched,
        record,
    })
}
