//! Reading a source's declarations for one page: the per-host cache of
//! `robots.txt` and licence documents, and the record a crossing carries.
//!
//! `robots.txt` is fetched once per host and cached for 24 hours, the age the
//! attachment draft and RFC 9309 allow; a licence document is cached with it
//! under its URL. A probe that failed is cached for five minutes, so a host
//! that is down is not asked again on every crossing and the failure is
//! still on each record. A failed `robots.txt` probe never discards the last
//! copy the host answered with: RFC 9309 §2.4 lets a crawler use it past its
//! life while the file is unreachable, and without it an unreachable file is
//! a complete disallow (§2.3.1.4). The cache is bytes and dates only; what they mean
//! is computed for each page, because `robots.txt` rules depend on the path.
//!
//! Every outbound request goes through the prober the caller supplies, which
//! is the mediated fetch path with its host policy and address floor.
//! Nothing here resolves a name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::crawl_delay::{DelayOutcome, DelayRuling, Pacing, millis};
use crate::declarations::{
    self, Category, Effective, LicenceTerms, PRODUCT_TOKEN, RobotsReading, Statement,
    StatementSource,
};
use crate::policy::{AssessedApplicability, TermsDeclaration};
use commonmeasure_types::PolicyMode;

/// How long a `robots.txt` copy is used before it is fetched again.
pub const ROBOTS_CACHE_AGE: chrono::Duration = chrono::Duration::hours(24);
/// How long a failed probe is remembered before the host is asked again.
pub const FAILURE_CACHE_AGE: chrono::Duration = chrono::Duration::minutes(5);
/// The exchange budget for a probe. Shorter than a page fetch: a probe is
/// overhead on a crossing, and a slow host should cost the crossing little.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// The most of a `robots.txt` parsed, and of a licence body kept. RFC 9309
/// §2.5 asks a crawler to parse at least the first 500 KiB, so a larger
/// `robots.txt` is read from its complete lines within the bound and the
/// rest ignored. A licence is a document that does not parse cut short, so
/// a larger one is recorded as too large and read as unavailable.
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
    /// The body as text, for a 2xx answer: the whole body within the size
    /// bound, or for a `robots.txt` over it the complete lines within it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Set where `body` is the part of a larger `robots.txt` within the
    /// bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<Truncated>,
    /// Why there is no body: a transport failure, a redirect this edge
    /// declined, a policy refusal of the probe's host, a request this edge
    /// cut short, or a licence body over the bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// True where this edge did not send the request ([`ProbeFailure::NotSent`]).
    /// A failure without it was sent, or tried, and nothing answered.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub not_sent: bool,
    /// The redirect target this edge declined to request
    /// ([`ProbeFailure::RedirectDeclined`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declined_redirect: Option<String>,
    /// True where the probe failed for a reason of this edge's own
    /// ([`ProbeFailure::CutShort`]). The host is not held to have failed, so
    /// the record expires when it is made and the next crossing asks again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut_short: bool,
}

impl Probe {
    /// Whether the host answered with something the access rule can be read
    /// from: a file within the bound, or a 4xx other than 429, which RFC 9309
    /// §2.3.1.3 reads as no rules for this fetcher.
    fn answered(&self) -> bool {
        self.body.is_some() || self.status.is_some_and(robots_status_is_no_rules)
    }

    /// Whether the host could not be reached for this file (RFC 9309
    /// §2.3.1.4): a 429, a 5xx or another status that is neither a file nor a
    /// 4xx, a request sent or tried that nothing answered, or a redirect this
    /// edge declined to follow. A request this edge did not send at all, or
    /// cut short itself, is not.
    fn unreachable(&self) -> bool {
        match self.status {
            Some(status) => !(200..300).contains(&status) && !robots_status_is_no_rules(status),
            None => !self.not_sent && !self.cut_short && self.error.is_some(),
        }
    }

    /// A `robots.txt` probe cached by an earlier build that this edge now
    /// reads differently, so it is asked for again rather than reused: a 2xx
    /// over the bound, kept with no body, and a probe recorded as not sent,
    /// which covered a redirect this edge declined.
    fn read_differently_now(&self) -> bool {
        self.not_sent
            || (self.body.is_none()
                && self
                    .status
                    .is_some_and(|status| (200..300).contains(&status)))
    }
}

/// How much of an oversized `robots.txt` was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truncated {
    /// The size of the body the host sent, in bytes, after any content
    /// coding was removed.
    pub size: u64,
    /// The bytes parsed: the complete lines within the bound.
    pub read: u64,
}

/// The part of an oversized `robots.txt` that is parsed: the first
/// [`MAX_PROBE_BODY`] bytes, cut after the last line break within them, so a
/// rule is never read cut in half (RFC 9309 §2.5). A line break is ASCII, so
/// the cut never splits a UTF-8 sequence. A first line longer than the bound
/// leaves nothing to parse.
fn within_parsing_limit(body: &[u8]) -> &[u8] {
    let head = &body[..MAX_PROBE_BODY.min(body.len())];
    let line_break = |byte: &u8| matches!(byte, b'\n' | b'\r');
    // A line ending exactly at the bound is complete.
    if body.get(MAX_PROBE_BODY).is_none_or(line_break) {
        return head;
    }
    head.iter()
        .rposition(line_break)
        .map_or(&[][..], |end| &head[..=end])
}

/// A `robots.txt` answered with a 4xx other than 429 states no rules for this
/// fetcher (RFC 9309 §2.3.1.3). A 429 is a rate limit, not permission, so it
/// counts as unreachable.
fn robots_status_is_no_rules(status: u16) -> bool {
    (400..500).contains(&status) && status != 429
}

/// Everything cached for one host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub robots: Option<Probe>,
    /// The last `robots.txt` answer the host gave, kept aside while `robots`
    /// holds a failure, so a failed probe never overwrites it. Absent while
    /// `robots` is itself an answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub robots_held: Option<Probe>,
    /// Licence documents by URL.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub licences: BTreeMap<String, Probe>,
    /// The licence each page's response named in a `Link` header, by page
    /// URL, as last seen. A licence a page names is only known once the page
    /// has answered, so this is what lets a later crossing read it before
    /// the page rather than after.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub page_licences: BTreeMap<String, String>,
}

/// The cache directory, `<home>/declarations/`, one JSON file per host.
pub struct DeclarationCache {
    dir: PathBuf,
}

/// A fetch of one URL through the mediated path: the final URL and the
/// response, or why nothing was fetched.
pub type Prober<'a> =
    &'a dyn Fn(&str) -> Result<(String, commonmeasure_http::Response), ProbeFailure>;

/// Why a probe brought back no response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeFailure {
    /// The request was sent, or tried, and nothing answered: a timeout, or a
    /// name, TLS or connection failure. For `robots.txt` this is RFC 9309's
    /// unreachable case.
    Unreachable(String),
    /// This edge did not send the request: policy refused the probe's host,
    /// or it is an address this edge does not mediate or the hub's origin.
    NotSent(String),
    /// The request was sent and answered with a redirect this edge declined
    /// to follow: to an address it does not mediate, to the hub's origin, or
    /// to a host the operator's policy refuses. The document was not read.
    /// RFC 9309 §2.3.1.2 expects a crawler to follow at least five
    /// redirects, so for `robots.txt` this is unreachable, not unavailable.
    RedirectDeclined {
        /// The URL the redirect named, which was not requested.
        target: String,
        reason: String,
    },
    /// The probe failed for a reason of this edge's own, not the host's: the
    /// call's time limit left nothing for the request, or ended it before
    /// the whole [`PROBE_TIMEOUT`] had passed, or the request could not be
    /// signed. It is never remembered as a failure of the host. For
    /// `robots.txt` the file is not read, and a page is not fetched under a
    /// file this edge has not read, so the crossing is refused.
    CutShort(String),
}

impl ProbeFailure {
    pub fn reason(&self) -> &str {
        match self {
            Self::Unreachable(reason)
            | Self::NotSent(reason)
            | Self::RedirectDeclined { reason, .. }
            | Self::CutShort(reason) => reason,
        }
    }
}

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
///
/// `pacing` is the crossing's `Crawl-delay` pacing where this probe is one
/// the host counts as a request, the licence document, with what the probe
/// must leave of the budget for the requests still to come after it. A
/// probe whose wait does not fit is not sent, and what is remembered expires
/// at once, so the next crossing asks again rather than treating the host as
/// unreadable for the failure age.
///
/// `kind` says what the document is. A `robots.txt` that answers a 4xx other
/// than 429 is a statement that there are no rules, kept as a file would be;
/// a licence
/// that answers 404 or 410 is a declared licence that is missing, remembered
/// for the failure age as a failure is and asked again after it, because the
/// publisher's link may be mended.
fn cached_probe(
    slot: &mut Option<Probe>,
    url: &str,
    now: DateTime<Utc>,
    probe: Prober<'_>,
    pacing: Option<(&Pacing, Duration)>,
    kind: ProbeKind,
) -> (Probe, CacheDecision) {
    if let Some(existing) = slot.as_ref()
        && existing.url == url
        && existing.expires_at > now
        && !(kind == ProbeKind::Robots && existing.read_differently_now())
    {
        return (existing.clone(), CacheDecision::Reused);
    }
    // The probe's turn is taken before it is sent, and what it records is
    // dated when it was sent rather than when the wait began: a recorded
    // fetch time that preceded the request by the delay would date the
    // cache's life from an instant the host never saw.
    let now = match pacing {
        Some((pacing, reserve)) => match pacing.probe_reserving(url, now, reserve) {
            Ok(sent_at) => sent_at,
            Err(reason) => {
                let unasked = Probe {
                    url: url.to_owned(),
                    fetched_at: now,
                    expires_at: now,
                    final_url: None,
                    status: None,
                    body: None,
                    truncated: None,
                    error: Some(reason),
                    not_sent: true,
                    declined_redirect: None,
                    cut_short: false,
                };
                *slot = Some(unasked.clone());
                return (unasked, CacheDecision::NotAsked);
            }
        },
        None => now,
    };
    let fresh = match probe(url) {
        // A licence that is gone: no body and no error, so the reader tells
        // it from a failure by its status. Redirects were followed, so the
        // final status decides.
        Ok((final_url, response))
            if kind == ProbeKind::Licence && licence_is_missing(response.status) =>
        {
            Probe {
                url: url.to_owned(),
                fetched_at: now,
                expires_at: now + FAILURE_CACHE_AGE,
                final_url: Some(final_url),
                status: Some(response.status),
                body: None,
                truncated: None,
                error: None,
                not_sent: false,
                declined_redirect: None,
                cut_short: false,
            }
        }
        // Any other answer outside 2xx is a failure of the probe, not a
        // statement by the host: a 503 is remembered for the failure age
        // and asked again, never for the day a file would be. For
        // `robots.txt` a 4xx other than 429 is the host's statement.
        Ok((final_url, response))
            if !(200..300).contains(&response.status)
                && !(kind == ProbeKind::Robots && robots_status_is_no_rules(response.status)) =>
        {
            Probe {
                url: url.to_owned(),
                fetched_at: now,
                expires_at: now + FAILURE_CACHE_AGE,
                final_url: Some(final_url),
                status: Some(response.status),
                body: None,
                truncated: None,
                error: Some(format!("{url} answered {}", response.status)),
                not_sent: false,
                declined_redirect: None,
                cut_short: false,
            }
        }
        Ok((final_url, response)) => {
            let ok = (200..300).contains(&response.status);
            let (body, truncated, error) = if !ok {
                (None, None, None)
            } else if response.body.len() <= MAX_PROBE_BODY {
                (
                    Some(String::from_utf8_lossy(&response.body).into_owned()),
                    None,
                    None,
                )
            } else if kind == ProbeKind::Robots {
                let read = within_parsing_limit(&response.body);
                (
                    Some(String::from_utf8_lossy(read).into_owned()),
                    Some(Truncated {
                        size: response.body.len() as u64,
                        read: read.len() as u64,
                    }),
                    None,
                )
            } else {
                (
                    None,
                    None,
                    Some(format!(
                        "the body is {} bytes, over the {MAX_PROBE_BODY} byte bound this reader \
                         keeps",
                        response.body.len()
                    )),
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
                truncated,
                error,
                not_sent: false,
                declined_redirect: None,
                cut_short: false,
            }
        }
        Err(failure) => {
            let cut_short = matches!(failure, ProbeFailure::CutShort(_));
            Probe {
                url: url.to_owned(),
                fetched_at: now,
                // A probe this edge cut short says nothing about the host, so
                // it is not kept as the host's failure: the next crossing
                // asks again.
                expires_at: if cut_short {
                    now
                } else {
                    now + FAILURE_CACHE_AGE
                },
                final_url: None,
                status: None,
                body: None,
                truncated: None,
                not_sent: matches!(failure, ProbeFailure::NotSent(_)),
                declined_redirect: match &failure {
                    ProbeFailure::RedirectDeclined { target, .. } => Some(target.clone()),
                    _ => None,
                },
                cut_short,
                error: Some(failure.reason().to_owned()),
            }
        }
    };
    *slot = Some(fresh.clone());
    (fresh, CacheDecision::Fetched)
}

/// What a probed document is, which decides how a 404 or 410 is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeKind {
    Robots,
    Licence,
}

/// A licence the publisher named that answers 404 or 410 is missing: the
/// crossing proceeds as though no licence terms were declared, and the
/// record says the declared licence is missing (owner decision, 22 September
/// 2026). Every other status outside 2xx, 401, 403 and 451 among them, says
/// the document exists and is withheld or could not be served, so it is
/// unread.
fn licence_is_missing(status: u16) -> bool {
    matches!(status, 404 | 410)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheDecision {
    Fetched,
    Reused,
    /// Nothing was asked for: the host's `Crawl-delay` put the request
    /// beyond what this fetch may still spend waiting, or the crossing was
    /// refused before the document was reached. What is remembered is held
    /// only until the host may next be asked, so a later crossing asks
    /// again.
    NotAsked,
}

/// What `robots.txt` said for one requested URL, as recorded on the
/// crossing. A redirect chain has one of these per hop: each hop is judged
/// against the file at its own origin, so a shortener's rule is recorded
/// against the shortener and never against the page it pointed at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobotsOutcome {
    /// The URL this evaluation is for: the URL asked for, or the hop a
    /// redirect named. `reading` is about this URL's path.
    #[serde(default)]
    pub requested_url: String,
    /// The `robots.txt` that governs `requested_url`: the one at its origin.
    pub url: String,
    /// The URL that answered the request for `url` after redirects. Where
    /// the host answered with a file, the reading came from it: RFC 9309
    /// §2.3.1.2 applies a redirected file's rules to the origin that
    /// redirected. A held copy names its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
    /// The redirect target this edge declined to request for `url`, which
    /// made the file unreachable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declined_redirect: Option<String>,
    pub cache: CacheDecision,
    pub fetched_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Why the file could not be read. A 4xx other than 429 is not an
    /// error: the host publishes no rules, and the reading below is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// True where the file could not be reached (RFC 9309 §2.3.1.4): a 429,
    /// a 5xx, a timeout, name, TLS or connection failure, or a redirect this
    /// edge declined to follow, which `unavailable` names. The reading is
    /// then the held copy's, or there is none and the crossing is refused.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unreachable: bool,
    /// True where this edge's own request for the file failed for a reason
    /// of its own ([`ProbeFailure::CutShort`]), which `unavailable` names.
    /// The reading is then the held copy's, or there is none and the
    /// crossing is refused.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut_short: bool,
    /// The last answer the host gave, which `reading` was read from because
    /// the file could not be read now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_copy: Option<HeldCopy>,
    /// Set where the copy `reading` was read from was over the parsing
    /// limit: its size, and the bytes read, which are its complete lines
    /// within the limit (RFC 9309 §2.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<Truncated>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<RobotsReading>,
    /// The policy mode the access rule was ruled under, once ruled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PolicyMode>,
    /// What the mode did with the access rule, once ruled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RobotsRuling>,
    /// What the group's `Crawl-delay` did to this request, once the request
    /// took its turn. Absent where the group states none, or where the
    /// request was refused before the delay was reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<DelayRuling>,
}

/// The copy of `robots.txt` a crossing was ruled by when the file could not
/// be read: when the host gave it, and what it answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldCopy {
    pub fetched_at: DateTime<Utc>,
    /// When the copy stopped being current. A held copy is used past it for
    /// as long as the file cannot be read (RFC 9309 §2.4).
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// The URL that gave the copy, after redirects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_url: Option<String>,
}

/// What the session's mode did with a `robots.txt` access rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RobotsRuling {
    /// The selected group's rules permit the path, or the host publishes
    /// no rules for this fetcher.
    Allowed,
    /// The rules disallow the path and the crossing stopped before the
    /// request. A `Disallow` binds in every policy mode.
    Refused,
    /// The rules disallow the path and `observe` or `prefer` made the
    /// request with the breach recorded. Written by 0.3.5 and earlier and
    /// no longer: a `Disallow` binds in every mode (WP-29). Kept so those
    /// records still read.
    Carried,
    /// The file could not be reached and no copy of it was held, so every
    /// path is disallowed (RFC 9309 §2.3.1.4) and the crossing stopped
    /// before the request, in every mode.
    Unreachable,
    /// This edge's own request for the file failed for a reason of its own
    /// (the call's time limit, or a signing failure) and no copy was held.
    /// The host is not held to have failed, but a page is not fetched under
    /// a file this edge has not read, so the crossing stopped before the
    /// request, in every mode.
    CutShort,
    /// The file was not requested, because this edge does not send a
    /// request to its address (the operator's policy, the address floor or
    /// the hub's origin), and no copy was held, so no rule was applied. The
    /// page is at the same origin and is refused by the same check.
    Unavailable,
}

impl RobotsOutcome {
    /// Rule the access rule and record the ruling on this outcome, with the
    /// session's `mode` beside it. Returns the attribution where the rules
    /// disallow the path, for the caller to refuse on.
    ///
    /// A `Disallow` binds in every policy mode (owner decision, 14 September
    /// 2026): the mode is recorded, not consulted. Whether a crawler obeys
    /// `robots.txt` is judged by what it fetched, not by the mode its
    /// operator chose.
    pub fn rule(&mut self, mode: PolicyMode) -> Option<String> {
        self.mode = Some(mode);
        let Some(reading) = &self.reading else {
            if self.unreachable {
                self.outcome = Some(RobotsRuling::Unreachable);
                return Some(self.attribution());
            }
            if self.cut_short {
                self.outcome = Some(RobotsRuling::CutShort);
                return Some(self.attribution());
            }
            self.outcome = Some(RobotsRuling::Unavailable);
            return None;
        };
        if reading.crawlable != Some(false) {
            self.outcome = Some(RobotsRuling::Allowed);
            return None;
        }
        self.outcome = Some(RobotsRuling::Refused);
        Some(self.attribution())
    }

    /// Whether the access rule stopped this crossing: a `Disallow`, or a
    /// file that could not be reached or read, with no copy held.
    pub fn refuses(&self) -> bool {
        matches!(
            self.outcome,
            Some(RobotsRuling::Refused | RobotsRuling::Unreachable | RobotsRuling::CutShort)
        )
    }

    /// The file as the record names it: the URL asked for, and the URL the
    /// reading came from where a redirect led elsewhere.
    fn file(&self) -> String {
        let answered = match &self.held_copy {
            Some(copy) => &copy.final_url,
            None => &self.final_url,
        };
        match answered {
            Some(answered) if *answered != self.url => {
                format!("{} (redirected to {answered})", self.url)
            }
            _ => self.url.clone(),
        }
    }

    /// Whether the delay let this request go. True where the group states
    /// none, or the turn was taken and waited out; false where the turn was
    /// refused or could not be kept, and [`RobotsOutcome::delay_refusal`]
    /// says why.
    pub fn sends(&self) -> bool {
        self.delay.as_ref().is_none_or(DelayRuling::sends)
    }

    /// Take this request's turn under the selected group's `Crawl-delay` from
    /// `pacing`, waiting it out where the wait fits what the call may still
    /// spend asleep, and record what the delay did. `pacing` has slept by the
    /// time this returns, so the caller sends as soon as it says it may.
    ///
    /// The delay binds in every policy mode (owner decision, 14 September
    /// 2026), so the mode is not consulted here, and it is kept whether the
    /// request is signed or not.
    pub fn take_turn(&mut self, pacing: &Pacing, now: DateTime<Utc>) -> bool {
        let Some(honoured) = self
            .reading
            .as_ref()
            .and_then(|reading| reading.crawl_delay.as_ref())
            .and_then(|delay| delay.honoured_ms)
        else {
            return true;
        };
        if honoured == 0 {
            return true;
        }
        let host = crate::grounding::host_of(&self.requested_url);
        let ruling = pacing.turn(&host, Duration::from_millis(honoured), now);
        let sends = ruling.sends();
        self.delay = Some(ruling);
        sends
    }

    /// Why the request was not sent, where the delay stopped it: the file,
    /// the delay as written, the host and when it may next be asked.
    pub fn delay_refusal(&self) -> Option<String> {
        let ruling = self.delay.as_ref()?;
        let read = self
            .reading
            .as_ref()
            .and_then(|reading| reading.crawl_delay.as_ref());
        let written = read
            .and_then(|delay| delay.value.clone())
            .unwrap_or_else(|| crate::crawl_delay::seconds(ruling.delay_ms));
        let capped = if read.is_some_and(|delay| delay.capped) {
            format!(
                ", kept at this edge's bound of {}s",
                crate::crawl_delay::seconds(ruling.delay_ms)
            )
        } else {
            String::new()
        };
        let stated = format!(
            "{} states `Crawl-delay: {written}` for {PRODUCT_TOKEN}{capped}, and it binds in \
             every policy mode",
            self.file()
        );
        // A licence with no current reading is read before the page, so a
        // refusal decided on the licence's turn says so: the page was not
        // asked for because its terms could not be read first.
        if let Some(licence) = &ruling.licence_first
            && ruling.outcome == DelayOutcome::Refused
        {
            let first = format!(
                "The licence {licence} has not been read, and a page is not fetched under terms \
                 this edge has not read, so the licence takes the host's next turn and the page \
                 the turn after it"
            );
            let budget = crate::crawl_delay::seconds(ruling.budget_ms);
            return Some(
                match (&ruling.unavailable, ruling.next_at, ruling.wait_ms) {
                    (Some(reason), _, _) => format!(
                        "{stated}. {first}; the licence was not read ({reason}), so the page was not \
                     requested."
                    ),
                    (None, Some(next), Some(wait)) => format!(
                        "{stated}. {first}: {}s in all, the host next free {}, which is beyond the \
                     {budget}s this fetch may still wait. Nothing was requested.",
                        crate::crawl_delay::seconds(wait),
                        next.to_rfc3339(),
                    ),
                    _ => format!(
                        "{stated}. {first}, and the two turns together are beyond the {budget}s this \
                     fetch may still wait: this is a hosted edge, whose sessions are served over \
                     HTTP, where the host ends a tool call well before a whole delay could be \
                     waited out. Nothing was requested."
                    ),
                },
            );
        }
        match ruling.outcome {
            DelayOutcome::Clear | DelayOutcome::Waited => None,
            // A hosted edge paces every tenant it serves from one identity,
            // so the time the host was last asked is another tenant's fetch
            // and is not disclosed with the refusal.
            DelayOutcome::Refused => Some(match (ruling.next_at, ruling.wait_ms) {
                (Some(next), Some(wait)) => format!(
                    "{stated}. The next request to {} may be sent {}, in {}s, which is beyond \
                     the {}s this fetch may still wait. Nothing was requested.",
                    ruling.host,
                    next.to_rfc3339(),
                    crate::crawl_delay::seconds(wait),
                    crate::crawl_delay::seconds(ruling.budget_ms),
                ),
                _ => format!(
                    "{stated}. {} was asked inside its delay by this edge, which paces every \
                     session it serves as one fetcher, and the wait is beyond the {}s this fetch \
                     may still wait: this is a hosted edge, whose sessions are served over HTTP, \
                     where the host ends a tool call well before a whole delay could be waited \
                     out. Nothing was requested; ask again shortly.",
                    ruling.host,
                    crate::crawl_delay::seconds(ruling.budget_ms),
                ),
            }),
            DelayOutcome::Unavailable => Some(format!(
                "{stated}. The turn could not be kept for {}, so the delay could not be kept \
                 either, and a fetch that cannot keep it is not sent: {}. Nothing was requested.",
                ruling.host,
                ruling
                    .unavailable
                    .as_deref()
                    .unwrap_or("no reason recorded"),
            )),
        }
    }

    /// The attribution a person or agent reads: which URL, which file,
    /// which group and which rule, with a wildcard named as one.
    pub fn attribution(&self) -> String {
        let failure = self.unavailable.as_deref().unwrap_or("no answer");
        let Some(reading) = &self.reading else {
            if self.unreachable {
                // A declined redirect is unreachable because RFC 9309 expects
                // redirects to be followed, so both sections are cited.
                let sections = if self.declined_redirect.is_some() {
                    "section 2.3.1.4, and section 2.3.1.2 on redirects"
                } else {
                    "section 2.3.1.4"
                };
                return format!(
                    "{} could not be reached for {}: {failure}. No copy of it is held, so every \
                     path on {} is treated as disallowed for {PRODUCT_TOKEN} (RFC 9309 \
                     {sections})",
                    self.file(),
                    self.requested_url,
                    crate::grounding::host_of(&self.requested_url),
                );
            }
            if self.cut_short {
                return format!(
                    "{} was not read for {}: {failure}. The request failed for a reason of this \
                     edge's own, so the host is not held to have failed and the file is asked \
                     for again at the next crossing. No copy of it is held, and a page is not \
                     fetched under a robots.txt this edge has not read",
                    self.url, self.requested_url,
                );
            }
            return format!(
                "{} could not be read for {}: {failure}",
                self.url, self.requested_url,
            );
        };
        let mut about_copy = self.held_copy.as_ref().map_or_else(String::new, |copy| {
            format!(
                " (ruled by the copy read {}, held because the file could not be read now: \
                 {failure})",
                copy.fetched_at.to_rfc3339()
            )
        });
        if let Some(cut) = &self.truncated {
            about_copy.push_str(&format!(
                " (read from the first {} of its {} bytes, the complete lines within this \
                 edge's parsing limit of {MAX_PROBE_BODY} bytes, RFC 9309 section 2.5)",
                cut.read, cut.size
            ));
        }
        let Some(group) = &reading.group else {
            return format!(
                "{} publishes no group for {PRODUCT_TOKEN} or `*`, so no access rule applies to \
                 {}{about_copy}",
                self.file(),
                self.requested_url
            );
        };
        let group = if reading.group_is_wildcard {
            format!(
                "the `User-agent: {group}` group, which addresses every fetcher because no group names {PRODUCT_TOKEN}"
            )
        } else {
            format!("the `User-agent: {group}` group, which names {PRODUCT_TOKEN}")
        };
        let rule = match (&reading.access_rule, reading.access_rule_wildcard) {
            (Some(rule), Some(true)) => format!("rule `{rule}`, a wildcard pattern"),
            (Some(rule), _) => format!("rule `{rule}`, a literal path prefix"),
            (None, _) => "no rule matching the path, which permits it".to_owned(),
        };
        let verdict = if reading.crawlable == Some(false) {
            "disallows"
        } else {
            "allows"
        };
        format!(
            "{} {verdict} {PRODUCT_TOKEN} at {}: {group}; {rule}{about_copy}",
            self.file(),
            self.requested_url
        )
    }
}
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
    /// True where the document could not be read at all: it was not asked
    /// for, the request failed or timed out, the host answered outside 2xx
    /// other than 404 and 410 (401, 403 and 451 included: the document
    /// exists and is withheld), the body was over the bound or did not parse. Its terms are unknown,
    /// not absent, so the crossing is refused in every policy mode: a page
    /// is not admitted under terms nobody has read. False where the document
    /// was read and simply has no entry for this page.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unread: bool,
    /// Set where the publisher named this licence and it answered 404 or
    /// 410: the declared licence is missing. The crossing proceeds with no
    /// licence terms, not with unknown ones, and this gap names the URL and
    /// the status on the crossing and in the tool result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<commonmeasure_types::Gap>,
}

/// What decided the AI-input question for this crossing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Governing {
    /// An applicable operator assessment covers this content and AI input.
    /// Source preferences remain recorded; its assessed terms govern.
    OperatorTerms,
    /// The combined statements.
    Statements,
}

/// The declarations record on a mediated crossing: what was read, from
/// where, and what it adds up to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Declarations {
    /// The `robots.txt` evaluation of the URL this crossing names: the URL
    /// that answered, or the hop that was refused.
    pub robots: RobotsOutcome,
    /// The evaluations of the earlier hops of a redirect chain, in the order
    /// they were requested, each against the file at its own origin. Empty
    /// when nothing redirected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redirects: Vec<RobotsOutcome>,
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
    /// Scope evaluation and policy outcome, separate from source statements
    /// and the operator's assessment retained in `terms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment_decision: Option<AssessmentDecision>,
    /// How a telemetry reporting demand was ruled on, where the licence or
    /// the terms carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting: Option<ReportingRuling>,
}

/// Application of an operator assessment to one actual acquisition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssessmentDecision {
    /// The policy rule that compares the terms assessment with the crossing.
    pub rule: String,
    /// Actual content requested at this hop.
    pub url: String,
    /// The use made by the tool, rather than an agent's claimed use.
    pub intended_use: Category,
    /// `applied`, `out_of_scope` or `unresolved`.
    pub applicability: String,
    /// Why the assessment governs or falls back to source statements.
    pub reason: String,
    /// The policy mode used when ruling on declarations.
    pub mode: Option<PolicyMode>,
    /// `allowed`, `refused` or `allowed_with_breach` after declaration checks.
    pub outcome: Option<String>,
}

impl AssessmentDecision {
    fn for_fetch(terms: &TermsDeclaration, page_url: &str) -> Self {
        let (applicability, reason) = match &terms.assessment {
            None => (
                "unresolved",
                "No scoped operator assessment accompanies this reference.",
            ),
            Some(a) if a.applicability == AssessedApplicability::Unresolved => {
                ("unresolved", "The operator left applicability unresolved.")
            }
            Some(a) if !a.content.iter().any(|content| content == page_url) => (
                "out_of_scope",
                "The requested content is outside the assessment's exact URL scope.",
            ),
            Some(a) if !a.intended_uses.contains(&Category::AiInput) => (
                "out_of_scope",
                "AI input is outside the assessment's intended uses.",
            ),
            Some(_) => (
                "applied",
                "The operator assessed this basis as applicable to the requested content and AI input.",
            ),
        };
        Self {
            rule: format!("terms ({}) assessment", terms.host),
            url: page_url.to_owned(),
            intended_use: Category::AiInput,
            applicability: applicability.to_owned(),
            reason: reason.to_owned(),
            mode: None,
            outcome: None,
        }
    }
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
///
/// `pacing` is the crossing's `Crawl-delay` pacing, where the caller keeps
/// one. The delay read here is recorded on it, so the licence probe and the
/// manifest probe to the same host are paced by the delay the page is paced
/// by; a publisher's log cannot tell those requests apart.
///
/// The order the requests are made in is what `mode` is for. `robots.txt`
/// takes no turn, so the access rule can be ruled before anything is owed to
/// the host: a crossing refused on a `Disallow` takes no turn and does not
/// ask for the licence. Where the crossing may go on, a licence with
/// no current reading takes the host's next turn, and the page's own turn is
/// left to [`take_page_turn`] once the caller has ruled on the terms.
pub fn before_fetch(
    cache: &DeclarationCache,
    page_url: &str,
    terms: Option<&TermsDeclaration>,
    now: DateTime<Utc>,
    probe: Prober<'_>,
    pacing: Option<&Pacing>,
    mode: PolicyMode,
) -> Declarations {
    let read = read_robots(cache, page_url, now, probe, pacing, mode);
    read_declared(cache, read, page_url, terms, now, probe, pacing)
}

/// Take the page's own turn, after every licence it waits behind, where
/// nothing has refused the crossing yet. The caller rules on what the
/// licences say first, so a crossing its terms refuse takes no turn and
/// waits for nothing. Returns whether the page may be sent.
pub fn take_page_turn(
    declarations: &mut Declarations,
    pacing: &Pacing,
    now: DateTime<Utc>,
) -> bool {
    if declarations.robots.delay.is_some() {
        return declarations.robots.sends();
    }
    if declarations.robots.refuses() {
        return false;
    }
    declarations.robots.take_turn(pacing, now)
}

/// What one host's `robots.txt` says for one URL, read and ruled on, before
/// anything the file names is asked for.
pub struct RobotsRead {
    record: HostRecord,
    robots: RobotsOutcome,
    host: String,
    robots_url: String,
    refused_by_the_access_rule: bool,
}

impl RobotsRead {
    /// The host the rules were read from.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Whether the access rule has already refused this crossing, so nothing
    /// further should be asked of the host.
    pub fn refused(&self) -> bool {
        self.refused_by_the_access_rule
    }

    /// The licence `robots.txt` names for this page, resolved against the
    /// file it was read from.
    fn robots_licence(&self) -> Option<String> {
        self.robots
            .reading
            .as_ref()
            .and_then(|reading| reading.licences.first().cloned())
            .and_then(|licence| {
                url::Url::parse(&self.robots_url)
                    .and_then(|base| base.join(&licence))
                    .ok()
                    .map(|joined| joined.to_string())
            })
    }

    /// The licences to be read before `page_url`, each with whether it is
    /// the one `robots.txt` names: that one, and on a paced host the licence
    /// the page's response named when it was last fetched, each only where
    /// no current reading of it is cached. The second is read so that it is
    /// in the cache when the response names it again; it is ruled on from
    /// the response, as a `Link` licence is.
    fn licences_first(
        &self,
        page_url: &str,
        now: DateTime<Utc>,
        paced: bool,
    ) -> Vec<(String, bool)> {
        if self.refused_by_the_access_rule {
            return Vec::new();
        }
        let current = |url: &str| {
            self.record
                .licences
                .get(url)
                .is_some_and(|probe| probe.url == url && probe.expires_at > now)
        };
        let robots_licence = self.robots_licence();
        let mut first = Vec::new();
        if let Some(url) = &robots_licence
            && !current(url)
        {
            first.push((url.clone(), true));
        }
        if paced
            && let Some(url) = self.record.page_licences.get(page_url)
            && Some(url) != robots_licence.as_ref()
            && !current(url)
        {
            first.push((url.clone(), false));
        }
        first
    }

    /// Whether a licence will be read before `page_url` on this crossing, so
    /// the host's next turn is the licence's and not a manifest's.
    pub fn reads_a_licence_first(&self, page_url: &str, now: DateTime<Utc>) -> bool {
        !self.licences_first(page_url, now, true).is_empty()
    }
}

/// Read and rule on `robots.txt` for `page_url`. The file takes no turn: the
/// delay cannot be read without it, and it is fetched once per host per day.
/// Nothing else has been asked of the host when this returns, so a crossing
/// refused on a `Disallow` costs the host only this file.
pub fn read_robots(
    cache: &DeclarationCache,
    page_url: &str,
    now: DateTime<Utc>,
    probe: Prober<'_>,
    pacing: Option<&Pacing>,
    mode: PolicyMode,
) -> RobotsRead {
    let host = crate::grounding::host_of(page_url);
    let mut record = cache.load(&host);
    let robots_url = robots_url_of(page_url);
    let previous = record.robots.clone();
    // `robots.txt` takes no turn: the delay cannot be read without it, and
    // it is fetched once per host per day.
    let (robots_probe, cache_decision) = cached_probe(
        &mut record.robots,
        &robots_url,
        now,
        probe,
        None,
        ProbeKind::Robots,
    );
    // The last answer is set aside when a fetch fails rather than lost with
    // the slot it was in, and dropped once the host answers again.
    if cache_decision == CacheDecision::Fetched {
        if robots_probe.answered() {
            record.robots_held = None;
        } else if let Some(answer) =
            previous.filter(|probe| probe.url == robots_url && probe.answered())
        {
            record.robots_held = Some(answer);
        }
    }
    let path = declarations::request_target(page_url);
    let read = |answer: &Probe| {
        declarations::parse_robots(answer.body.as_deref().unwrap_or("")).read(PRODUCT_TOKEN, &path)
    };
    let (reading, unavailable, held_copy, truncated) = if robots_probe.answered() {
        (
            Some(read(&robots_probe)),
            None,
            None,
            robots_probe.truncated,
        )
    } else {
        let failure = robots_probe
            .error
            .clone()
            .unwrap_or_else(|| match robots_probe.status {
                Some(status) => format!("{robots_url} answered {status}"),
                None => "no answer".to_owned(),
            });
        // A file that cannot be read now is ruled by the last answer the host
        // gave, however old: an unreachable file never reverts to no rules
        // with age.
        match record
            .robots_held
            .as_ref()
            .filter(|held| held.url == robots_url)
        {
            Some(held) => (
                Some(read(held)),
                Some(failure),
                Some(HeldCopy {
                    fetched_at: held.fetched_at,
                    expires_at: held.expires_at,
                    status: held.status,
                    final_url: held.final_url.clone(),
                }),
                held.truncated,
            ),
            None => (None, Some(failure), None, None),
        }
    };
    if let Some(pacing) = pacing
        && let Some(honoured) = reading
            .as_ref()
            .and_then(|reading| reading.crawl_delay.as_ref())
            .and_then(|delay| delay.honoured_ms)
    {
        pacing.learn(&host, honoured);
    }
    let mut robots = RobotsOutcome {
        requested_url: page_url.to_owned(),
        url: robots_url.clone(),
        final_url: robots_probe.final_url.clone(),
        declined_redirect: robots_probe.declined_redirect.clone(),
        cache: cache_decision,
        fetched_at: robots_probe.fetched_at,
        expires_at: robots_probe.expires_at,
        status: robots_probe.status,
        unavailable,
        unreachable: robots_probe.unreachable(),
        cut_short: robots_probe.cut_short,
        held_copy,
        truncated,
        reading,
        mode: None,
        outcome: None,
        delay: None,
    };
    // Ruled from `robots.txt`, which took no turn, so nothing is owed to the
    // host yet. `rule` is consulted again when the crossing is ruled on as a
    // whole; it reads the same reading and gives the same answer.
    robots.rule(mode);
    let refused_by_the_access_rule = robots.refuses();
    RobotsRead {
        record,
        robots,
        host,
        robots_url,
        refused_by_the_access_rule,
    }
}

/// Read the RSL licences a page is governed by and take the page's turn
/// under the host's `Crawl-delay`.
///
/// A page is never admitted under a licence this edge has not read, so a
/// licence with no current reading is read before the page: the one
/// `robots.txt` names, and, on a paced host, the one the page's response
/// named last time it was fetched. Each takes the host's next turn and the
/// page waits for its own turn after them, which spends a delay per licence
/// on a first crossing. Whether the whole sequence fits what the call may
/// still spend asleep is decided before any of it is sent: where it does
/// not, the crossing is refused naming the licence and the delay, and the
/// host is asked for nothing, rather than read for a licence and then
/// refused the page. A licence already read, and still current, takes no
/// turn, so the page's turn is the only one.
///
/// Where the access rule has refused the crossing nothing further is asked
/// and the licence is recorded as not asked.
pub fn read_declared(
    cache: &DeclarationCache,
    read: RobotsRead,
    page_url: &str,
    terms: Option<&TermsDeclaration>,
    now: DateTime<Utc>,
    probe: Prober<'_>,
    pacing: Option<&Pacing>,
) -> Declarations {
    // The first candidate licence in the applicable scope. RSL lets a client
    // retrieve each listed document; one is read here, and the record names
    // which. RSL requires the directive's value to be absolute; a relative
    // one is resolved against the file it was read from rather than dropped,
    // and the record carries the resolved URL.
    let robots_licence = read.robots_licence();
    let page_delay = pacing.and_then(|pacing| pacing.delay_for(&read.host));
    let first = read.licences_first(page_url, now, page_delay.is_some());
    let RobotsRead {
        mut record,
        mut robots,
        host,
        robots_url: _,
        refused_by_the_access_rule,
    } = read;
    let mut statements = robots
        .reading
        .as_ref()
        .map(|reading| reading.statements.clone())
        .unwrap_or_default();
    let first_named = first
        .iter()
        .map(|(url, _)| url.as_str())
        .collect::<Vec<_>>()
        .join(" and ");
    // Whether licence-then-page fits, decided before any turn is taken.
    let refusal = match (pacing, page_delay) {
        (Some(pacing), Some(delay)) if !first.is_empty() => {
            let same_host = first
                .iter()
                .filter(|(url, _)| crate::grounding::host_of(url) == host)
                .count() as u32;
            let pending = pacing.pending_wait(&host, now);
            let need = pending + delay * same_host;
            let spendable = pacing.spendable_now();
            (need > spendable).then(|| {
                let mut ruling = DelayRuling {
                    host: host.clone(),
                    delay_ms: millis(delay),
                    outcome: DelayOutcome::Refused,
                    wait_ms: Some(millis(need)),
                    budget_ms: millis(spendable),
                    next_at: now
                        .checked_add_signed(chrono::Duration::milliseconds(millis(pending) as i64)),
                    unavailable: None,
                    recovered: None,
                    licence_first: Some(first_named.clone()),
                };
                if !pacing.names_the_edge() {
                    ruling.withhold_times();
                }
                ruling
            })
        }
        _ => None,
    };
    let mut licences = Vec::new();
    let mut now = now;
    if let Some(ruling) = refusal {
        robots.delay = Some(ruling);
        if let Some(url) = &robots_licence {
            licences.push(not_asked(
                url,
                "reading this licence before the page, and the page after it, does not fit \
                 what this fetch may still spend waiting under the source's `Crawl-delay`, so \
                 neither was asked for",
            ));
        }
    } else {
        // Each licence read before the page leaves what the requests after it
        // on the page's host will wait: a delay for each later turn there.
        let mut unsent: Option<String> = None;
        for (index, (url, from_robots)) in first.iter().enumerate() {
            let same_after = first[index + 1..]
                .iter()
                .filter(|(later, _)| crate::grounding::host_of(later) == host)
                .count() as u32;
            let reserve = match (pacing, page_delay) {
                (Some(_), Some(delay)) if crate::grounding::host_of(url) == host => {
                    delay * (same_after + 1)
                }
                (Some(pacing), Some(delay)) => pacing.pending_wait(&host, now) + delay * same_after,
                _ => Duration::ZERO,
            };
            let paced = pacing.map(|pacing| (pacing, reserve));
            let (cache, sent_at) = if *from_robots {
                let (outcome, sent_at) = read_licence(
                    &mut record,
                    url,
                    LicenceMechanism::RobotsLicense,
                    page_url,
                    now,
                    probe,
                    paced,
                );
                let cache = (outcome.cache, outcome.unavailable.clone());
                if let Some(terms) = &outcome.terms {
                    statements.extend(terms.statements.iter().cloned());
                }
                licences.push(outcome);
                (cache, sent_at)
            } else {
                let mut slot = record.licences.remove(url.as_str());
                let (answer, decision) =
                    cached_probe(&mut slot, url, now, probe, paced, ProbeKind::Licence);
                if let Some(kept) = slot {
                    record.licences.insert(url.clone(), kept);
                }
                ((decision, answer.error), answer.fetched_at)
            };
            match cache {
                (CacheDecision::NotAsked, reason) => {
                    unsent = Some(format!(
                        "{url} was not asked for: {}",
                        reason.unwrap_or_else(|| "no reason recorded".to_owned())
                    ));
                    break;
                }
                _ => now = now.max(sent_at),
            }
        }
        // A licence read first on a paced host that was not sent leaves the
        // page without the terms it would be admitted under, so the page's
        // turn is not taken and the crossing is refused naming the licence.
        // On a host with no delay the unread licence refuses the crossing
        // when the declarations are ruled on.
        if let (Some(reason), Some(delay)) = (&unsent, page_delay) {
            robots.delay = Some(DelayRuling {
                host: host.clone(),
                delay_ms: millis(delay),
                outcome: DelayOutcome::Refused,
                wait_ms: None,
                budget_ms: pacing.map_or(0, |pacing| millis(pacing.spendable_now())),
                next_at: None,
                unavailable: Some(reason.clone()),
                recovered: None,
                licence_first: Some(first_named.clone()),
            });
        }
        // A current reading of the licence `robots.txt` names was not read
        // above; it is reused here and takes no turn.
        if let Some(url) = &robots_licence
            && !licences
                .iter()
                .any(|licence: &LicenceOutcome| &licence.url == url)
        {
            let outcome = if refused_by_the_access_rule {
                let mut outcome = not_asked(
                    url,
                    "the access rule refused this crossing before the licence was read, so the \
                     source was not asked for it",
                );
                // Refused already; its terms are not what refuses it.
                outcome.unread = false;
                outcome
            } else {
                read_licence(
                    &mut record,
                    url,
                    LicenceMechanism::RobotsLicense,
                    page_url,
                    now,
                    probe,
                    pacing.map(|pacing| (pacing, Duration::ZERO)),
                )
                .0
            };
            if let Some(terms) = &outcome.terms {
                statements.extend(terms.statements.iter().cloned());
            }
            licences.push(outcome);
        }
    }
    cache.save(&host, &record);
    let assessment_decision = terms.map(|terms| AssessmentDecision::for_fetch(terms, page_url));
    let mut declarations = Declarations {
        robots,
        redirects: Vec::new(),
        licences,
        content_usage_header: None,
        statements,
        effective: BTreeMap::new(),
        terms: terms.cloned(),
        governing: if assessment_decision
            .as_ref()
            .is_some_and(|d| d.applicability == "applied")
        {
            Governing::OperatorTerms
        } else {
            Governing::Statements
        },
        assessment_decision,
        reporting: None,
    };
    declarations.recombine();
    declarations
}

/// A licence `robots.txt` names that this crossing did not ask for, and why.
fn not_asked(url: &str, why: &str) -> LicenceOutcome {
    LicenceOutcome {
        url: url.to_owned(),
        mechanism: LicenceMechanism::RobotsLicense,
        cache: CacheDecision::NotAsked,
        status: None,
        unavailable: Some(why.to_owned()),
        content: None,
        terms: None,
        unread: true,
        missing: None,
    }
}

/// Add what the page response itself carried: the `Content-Usage` header and
/// a `Link` header naming an RSL licence.
///
/// A licence the response names is read before the page's bytes are
/// admitted; where it has no current reading it takes the host's next turn
/// from what the call has left, and where that does not fit it is recorded
/// unread and the bytes are withheld. The page's record keeps which licence
/// it named, so the next crossing reads it before the page instead.
pub fn after_fetch(
    cache: &DeclarationCache,
    declarations: &mut Declarations,
    page_url: &str,
    response: &commonmeasure_http::Response,
    now: DateTime<Utc>,
    probe: Prober<'_>,
    pacing: Option<&Pacing>,
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
    let named = response
        .headers
        .get("Link")
        .and_then(|link| declarations::rsl_link(link, page_url));
    let host = crate::grounding::host_of(page_url);
    let mut record = cache.load(&host);
    let remembered = record.page_licences.get(page_url).cloned();
    match &named {
        Some(url) => {
            record
                .page_licences
                .insert(page_url.to_owned(), url.clone());
        }
        None => {
            record.page_licences.remove(page_url);
        }
    }
    if let Some(licence_url) = named
        && !declarations
            .licences
            .iter()
            .any(|licence| licence.url == licence_url)
    {
        let (outcome, _) = read_licence(
            &mut record,
            &licence_url,
            LicenceMechanism::LinkHeader,
            page_url,
            now,
            probe,
            pacing.map(|pacing| (pacing, Duration::ZERO)),
        );
        if let Some(terms) = &outcome.terms {
            declarations
                .statements
                .extend(terms.statements.iter().cloned());
        }
        declarations.licences.push(outcome);
        cache.save(&host, &record);
    } else if remembered != record.page_licences.get(page_url).cloned() {
        cache.save(&host, &record);
    }
    declarations.recombine();
}

/// Read one licence document for `page_url`, from the cache where current.
/// The second value is when the request was sent, or `now` where nothing
/// was.
fn read_licence(
    record: &mut HostRecord,
    licence_url: &str,
    mechanism: LicenceMechanism,
    page_url: &str,
    now: DateTime<Utc>,
    probe: Prober<'_>,
    pacing: Option<(&Pacing, Duration)>,
) -> (LicenceOutcome, DateTime<Utc>) {
    let mut slot = record.licences.remove(licence_url);
    let (licence_probe, cache_decision) = cached_probe(
        &mut slot,
        licence_url,
        now,
        probe,
        pacing,
        ProbeKind::Licence,
    );
    if let Some(probe) = slot {
        record.licences.insert(licence_url.to_owned(), probe);
    }
    let sent_at = match cache_decision {
        CacheDecision::Fetched => licence_probe.fetched_at,
        _ => now,
    };
    let mut outcome = LicenceOutcome {
        url: licence_url.to_owned(),
        mechanism,
        cache: cache_decision,
        status: licence_probe.status,
        unavailable: None,
        content: None,
        terms: None,
        unread: false,
        missing: None,
    };
    if licence_probe.body.is_none()
        && licence_probe.error.is_none()
        && let Some(status) = licence_probe.status.filter(|s| licence_is_missing(*s))
    {
        let reason = format!(
            "the declared licence {licence_url} is missing: it answered {status}, so the \
             crossing proceeds with no licence terms"
        );
        outcome.unavailable = Some(reason.clone());
        outcome.missing = Some(commonmeasure_types::Gap::new(
            commonmeasure_types::GapReason::EvidenceMissing,
            reason,
        ));
        return (outcome, sent_at);
    }
    let Some(body) = &licence_probe.body else {
        outcome.unread = true;
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
        return (outcome, sent_at);
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
        Err(error) => {
            outcome.unread = true;
            outcome.unavailable = Some(error);
        }
    }
    (outcome, sent_at)
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

    /// A crossing record 0.3.5 wrote under observe, when a `Disallow` was
    /// carried: the requested page and a redirect hop both `carried`. It
    /// still reads, keeps the outcome it was written with, and writes it
    /// back unchanged, so an old session log is not rewritten as refused.
    #[test]
    fn a_carried_disallow_written_by_0_3_5_still_reads() {
        let robots = |requested: &str| {
            serde_json::json!({
                "requested_url": requested,
                "url": "https://short.example/robots.txt", "cache": "fetched",
                "fetched_at": "2026-09-01T10:00:00Z", "expires_at": "2026-09-02T10:00:00Z",
                "status": 200,
                "reading": {"group": "*", "group_is_wildcard": true, "crawlable": false,
                            "access_rule": "Disallow: /", "access_rule_wildcard": false,
                            "statements": [], "licences": []},
                "mode": "observe", "outcome": "carried"
            })
        };
        let recorded = serde_json::json!({
            "robots": robots("https://short.example/page"),
            "redirects": [robots("https://short.example/abc")],
            "statements": [],
            "effective": {},
            "governing": "statements"
        });
        let read: Declarations =
            serde_json::from_value(recorded.clone()).expect("an old record reads");
        assert_eq!(read.robots.outcome, Some(RobotsRuling::Carried));
        assert_eq!(read.robots.mode, Some(PolicyMode::Observe));
        assert_eq!(read.redirects[0].outcome, Some(RobotsRuling::Carried));
        assert_eq!(
            serde_json::to_value(&read).expect("writes")["robots"]["outcome"],
            "carried"
        );
    }

    /// The ruling itself: a `Disallow` refuses under every mode, the mode is
    /// recorded beside it, and the attribution is returned for the refusal.
    #[test]
    fn a_disallow_is_refused_under_every_mode() {
        for mode in [PolicyMode::Strict, PolicyMode::Observe, PolicyMode::Prefer] {
            let mut outcome = RobotsOutcome {
                requested_url: "https://publisher.example/article".to_owned(),
                url: "https://publisher.example/robots.txt".to_owned(),
                final_url: None,
                declined_redirect: None,
                cache: CacheDecision::Fetched,
                fetched_at: Utc::now(),
                expires_at: Utc::now(),
                status: Some(200),
                unavailable: None,
                unreachable: false,
                cut_short: false,
                held_copy: None,
                truncated: None,
                reading: Some(
                    declarations::parse_robots("User-agent: *\nDisallow: /\n")
                        .read(PRODUCT_TOKEN, "/article"),
                ),
                mode: None,
                outcome: None,
                delay: None,
            };
            let attribution = outcome.rule(mode).expect("disallowed");
            assert!(attribution.contains("`Disallow: /`"), "{attribution}");
            assert_eq!(outcome.outcome, Some(RobotsRuling::Refused), "{mode:?}");
            assert_eq!(outcome.mode, Some(mode));
        }
    }

    /// A licence that answers 404 or 410 is missing: no terms, not unread,
    /// the gap naming the URL and status, and remembered for the failure age
    /// then asked again, as a failure is. A 401, 403 or 451 is withheld, and
    /// unread.
    #[test]
    fn a_missing_licence_is_kept_for_the_failure_age_and_a_withheld_one_is_unread() {
        use std::cell::Cell;
        for (status, missing) in [
            (404, true),
            (410, true),
            (401, false),
            (403, false),
            (451, false),
        ] {
            let asked = Cell::new(0);
            let probe =
                |url: &str| -> Result<(String, commonmeasure_http::Response), ProbeFailure> {
                    asked.set(asked.get() + 1);
                    Ok((
                        url.to_owned(),
                        commonmeasure_http::Response::text(status, "no"),
                    ))
                };
            let mut record = HostRecord::default();
            let now = Utc::now();
            let url = "https://example.com/license.xml";
            let page = "https://example.com/a";
            let (first, _) = read_licence(
                &mut record,
                url,
                LicenceMechanism::RobotsLicense,
                page,
                now,
                &probe,
                None,
            );
            assert_eq!(first.status, Some(status));
            assert_eq!(first.unread, !missing, "{status}: {first:?}");
            assert_eq!(first.missing.is_some(), missing, "{status}: {first:?}");
            if let Some(gap) = &first.missing {
                assert_eq!(gap.reason, commonmeasure_types::GapReason::EvidenceMissing);
                assert!(gap.detail.contains(url) && gap.detail.contains(&status.to_string()));
            }
            assert_eq!(
                record.licences[url].expires_at,
                record.licences[url].fetched_at + FAILURE_CACHE_AGE,
                "{status}"
            );
            let (reused, _) = read_licence(
                &mut record,
                url,
                LicenceMechanism::RobotsLicense,
                page,
                now + chrono::Duration::minutes(4),
                &probe,
                None,
            );
            assert_eq!(reused.cache, CacheDecision::Reused, "{status}");
            assert_eq!(reused.missing.is_some(), missing, "{status}");
            let (again, _) = read_licence(
                &mut record,
                url,
                LicenceMechanism::RobotsLicense,
                page,
                now + FAILURE_CACHE_AGE + chrono::Duration::seconds(1),
                &probe,
                None,
            );
            assert_eq!(again.cache, CacheDecision::Fetched, "{status}");
            assert_eq!(asked.get(), 2, "{status}");
        }
    }

    /// A 503 is a failed probe: remembered for the failure age, then asked
    /// again; the answer that follows is read. With no earlier answer held,
    /// the file is unreachable and every path is refused, in every mode.
    #[test]
    fn a_non_2xx_non_404_probe_is_cached_for_the_failure_age_and_retried() {
        use std::cell::Cell;
        let home = tempfile::tempdir().unwrap();
        let cache = DeclarationCache::open(home.path());
        let asked = Cell::new(0);
        let probe = |url: &str| -> Result<(String, commonmeasure_http::Response), ProbeFailure> {
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
            Err(ProbeFailure::Unreachable("not asked".to_owned()))
        };
        let start = Utc::now();
        let first = before_fetch(
            &cache,
            "https://host.example/a",
            None,
            start,
            &probe,
            None,
            PolicyMode::Observe,
        );
        assert_eq!(first.robots.status, Some(503));
        assert!(first.robots.unavailable.as_deref().unwrap().contains("503"));
        assert_eq!(first.robots.expires_at, start + FAILURE_CACHE_AGE);
        assert!(first.robots.unreachable);
        assert_eq!(first.robots.outcome, Some(RobotsRuling::Unreachable));
        assert!(first.robots.refuses());
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
            None,
            PolicyMode::Observe,
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
            None,
            PolicyMode::Observe,
        );
        assert_eq!(later.robots.cache, CacheDecision::Fetched);
        assert_eq!(later.robots.status, Some(200));
        assert_eq!(later.ai_input(), Effective::Disallow);
        assert_eq!(asked.get(), 2);
    }

    /// RFC 9309 §2.3.1 and §2.4 on one host over three days: an answer, then
    /// failures of each unreachable kind, then an answer again. A failure
    /// never discards the answer before it, the held answer rules however
    /// old it is, and a probe this edge did not send, or a 4xx, is not
    /// unreachable.
    #[test]
    fn a_failed_robots_probe_keeps_the_last_answer_and_is_ruled_by_it() {
        use std::cell::RefCell;
        let home = tempfile::tempdir().unwrap();
        let cache = DeclarationCache::open(home.path());
        let answer: RefCell<Result<(u16, &str), ProbeFailure>> =
            RefCell::new(Ok((200, "User-agent: *\nDisallow: /private/\n")));
        let probe = |url: &str| -> Result<(String, commonmeasure_http::Response), ProbeFailure> {
            answer.borrow().clone().map(|(status, body)| {
                (
                    url.to_owned(),
                    commonmeasure_http::Response::text(status, body),
                )
            })
        };
        let start = Utc::now();
        let at = |days: i64| start + chrono::Duration::days(days);
        let read = |page: &str, now| {
            before_fetch(&cache, page, None, now, &probe, None, PolicyMode::Observe).robots
        };
        let open = "https://host.example/open";
        let private = "https://host.example/private/a";

        assert_eq!(read(private, at(0)).outcome, Some(RobotsRuling::Refused));

        for (day, failure) in [
            (2, Ok((503, "busy"))),
            (3, Ok((429, "slow down"))),
            (4, Err(ProbeFailure::Unreachable("timed out".to_owned()))),
        ] {
            *answer.borrow_mut() = failure;
            let ruled = read(open, at(day));
            assert_eq!(ruled.cache, CacheDecision::Fetched, "day {day}");
            assert!(ruled.unreachable, "day {day}");
            let held = ruled.held_copy.as_ref().expect("the answer is held");
            assert!(held.expires_at < at(day), "day {day}: past its life");
            assert_eq!(ruled.outcome, Some(RobotsRuling::Allowed), "day {day}");
            assert!(ruled.attribution().contains("held because"), "day {day}");
            let ruled = read(private, at(day));
            assert_eq!(ruled.cache, CacheDecision::Reused, "day {day}");
            assert_eq!(ruled.outcome, Some(RobotsRuling::Refused), "day {day}");
        }

        *answer.borrow_mut() = Err(ProbeFailure::NotSent("refused by policy".to_owned()));
        let ruled = read(open, at(5));
        assert!(!ruled.unreachable);
        assert!(ruled.held_copy.is_some(), "the held answer still rules");

        *answer.borrow_mut() = Ok((403, "no"));
        let ruled = read(private, at(6));
        assert!(!ruled.unreachable && ruled.held_copy.is_none());
        assert_eq!(ruled.status, Some(403));
        assert_eq!(
            ruled.outcome,
            Some(RobotsRuling::Allowed),
            "a 4xx is no rules"
        );
        assert_eq!(
            cache.load("host.example").robots.unwrap().expires_at,
            at(6) + ROBOTS_CACHE_AGE,
            "kept as a file is"
        );
        assert!(cache.load("host.example").robots_held.is_none());

        // The 403 is now the last answer: a later failure is ruled by it.
        *answer.borrow_mut() = Ok((502, "bad gateway"));
        let ruled = read(private, at(8));
        assert_eq!(
            ruled.held_copy.as_ref().and_then(|held| held.status),
            Some(403)
        );
        assert_eq!(ruled.outcome, Some(RobotsRuling::Allowed));
    }

    /// With nothing held, an unreachable file refuses under every mode and
    /// the attribution names the host and the failure.
    #[test]
    fn an_unreachable_robots_file_with_nothing_held_refuses_in_every_mode() {
        let home = tempfile::tempdir().unwrap();
        let cache = DeclarationCache::open(home.path());
        let probe = |_: &str| -> Result<(String, commonmeasure_http::Response), ProbeFailure> {
            Err(ProbeFailure::Unreachable(
                "host.example did not answer within the 5s exchange budget".to_owned(),
            ))
        };
        for mode in [PolicyMode::Strict, PolicyMode::Observe, PolicyMode::Prefer] {
            let read = read_robots(
                &cache,
                "https://host.example/a",
                Utc::now(),
                &probe,
                None,
                mode,
            );
            assert!(read.refused(), "{mode:?}");
            let mut robots = read.robots;
            let attribution = robots.rule(mode).expect("a refusal");
            assert_eq!(robots.outcome, Some(RobotsRuling::Unreachable));
            for needed in ["host.example", "did not answer within", "2.3.1.4"] {
                assert!(attribution.contains(needed), "{needed}: {attribution}");
            }
        }
    }

    /// A `robots.txt` redirect this edge declines is unreachable, not
    /// unavailable: with nothing held it refuses in every mode and names the
    /// redirect, and with an answer held that answer rules.
    #[test]
    fn a_declined_robots_redirect_is_unreachable() {
        use std::cell::RefCell;
        let home = tempfile::tempdir().unwrap();
        let cache = DeclarationCache::open(home.path());
        let declined = "https://example.com/robots.txt redirected to \
                        https://www.example.com/robots.txt, which this edge does not follow: \
                        host www.example.com is not in the allowed source hosts";
        let answer: RefCell<Result<(u16, &str), ProbeFailure>> =
            RefCell::new(Err(ProbeFailure::RedirectDeclined {
                target: "https://www.example.com/robots.txt".to_owned(),
                reason: declined.to_owned(),
            }));
        let probe = |url: &str| -> Result<(String, commonmeasure_http::Response), ProbeFailure> {
            answer.borrow().clone().map(|(status, body)| {
                (
                    url.to_owned(),
                    commonmeasure_http::Response::text(status, body),
                )
            })
        };
        let start = Utc::now();
        for mode in [PolicyMode::Strict, PolicyMode::Observe, PolicyMode::Prefer] {
            let read = read_robots(&cache, "https://example.com/a", start, &probe, None, mode);
            assert!(read.refused(), "{mode:?}");
            assert!(read.robots.unreachable, "{mode:?}");
            assert_eq!(read.robots.outcome, Some(RobotsRuling::Unreachable));
            let attribution = read.robots.attribution();
            assert!(attribution.contains(declined), "{attribution}");
            assert!(attribution.contains("2.3.1.4"), "{attribution}");
        }

        // An answer, then the redirect again a day later: the answer rules.
        *answer.borrow_mut() = Ok((200, "User-agent: *\nDisallow: /private/\n"));
        let at = |days: i64| start + chrono::Duration::days(days);
        let read = |page: &str, now| {
            before_fetch(&cache, page, None, now, &probe, None, PolicyMode::Observe).robots
        };
        assert_eq!(
            read("https://example.com/open", at(1)).outcome,
            Some(RobotsRuling::Allowed)
        );
        *answer.borrow_mut() = Err(ProbeFailure::RedirectDeclined {
            target: "https://www.example.com/robots.txt".to_owned(),
            reason: declined.to_owned(),
        });
        let ruled = read("https://example.com/private/a", at(3));
        assert!(ruled.unreachable && ruled.held_copy.is_some());
        assert_eq!(ruled.outcome, Some(RobotsRuling::Refused));
        assert!(ruled.attribution().contains("held because"));
        let ruled = read("https://example.com/open", at(3));
        assert_eq!(ruled.outcome, Some(RobotsRuling::Allowed));
    }

    /// RFC 9309 §2.5: an oversized `robots.txt` is parsed up to the bound,
    /// cut after the last complete line, and the cut recorded. A rule inside
    /// the bound binds; one past it is not read.
    #[test]
    fn an_oversized_robots_file_is_read_to_its_last_complete_line_within_the_bound() {
        let head = "User-agent: *\nDisallow: /private/\n";
        let padding = "# padding\n".repeat(MAX_PROBE_BODY / 10);
        // The bound falls in the middle of the `Disallow: /late/` line.
        let mut body = format!("{head}{padding}");
        body.truncate(MAX_PROBE_BODY - 5);
        let complete = body.rfind('\n').unwrap() + 1;
        body.truncate(complete);
        body.push_str("Disallow: /late/\n# tail\n");
        let cut = within_parsing_limit(body.as_bytes());
        assert_eq!(cut.len(), complete, "cut after the last complete line");
        assert!(body.len() > MAX_PROBE_BODY);

        let home = tempfile::tempdir().unwrap();
        let cache = DeclarationCache::open(home.path());
        let probe = |url: &str| -> Result<(String, commonmeasure_http::Response), ProbeFailure> {
            Ok((
                url.to_owned(),
                commonmeasure_http::Response::text(200, &body),
            ))
        };
        for mode in [PolicyMode::Strict, PolicyMode::Observe, PolicyMode::Prefer] {
            let home = tempfile::tempdir().unwrap();
            let cache = DeclarationCache::open(home.path());
            let read = read_robots(
                &cache,
                "https://host.example/private/a",
                Utc::now(),
                &probe,
                None,
                mode,
            );
            assert!(read.refused(), "{mode:?}");
            let robots = read.robots;
            assert_eq!(robots.outcome, Some(RobotsRuling::Refused), "{mode:?}");
            assert_eq!(
                robots.truncated,
                Some(Truncated {
                    size: body.len() as u64,
                    read: complete as u64,
                })
            );
            assert!(robots.unavailable.is_none() && !robots.unreachable);
            let attribution = robots.attribution();
            assert!(
                attribution.contains(&format!("the first {complete} of its {}", body.len()))
                    && attribution.contains("section 2.5"),
                "{attribution}"
            );
        }
        let late = before_fetch(
            &cache,
            "https://host.example/late/a",
            None,
            Utc::now(),
            &probe,
            None,
            PolicyMode::Strict,
        );
        assert_eq!(late.robots.outcome, Some(RobotsRuling::Allowed));
        let cached = cache.load("host.example").robots.expect("cached");
        assert_eq!(cached.body.as_ref().map(String::len), Some(complete));
        assert!(cached.truncated.is_some() && cached.error.is_none());
    }

    #[test]
    fn the_parsing_limit_keeps_a_line_that_ends_at_the_bound_and_no_part_of_a_longer_one() {
        let mut exact = "x".repeat(MAX_PROBE_BODY - 1);
        exact.push('\n');
        let mut over = exact.clone();
        over.push_str("Disallow: /\n");
        assert_eq!(within_parsing_limit(over.as_bytes()).len(), MAX_PROBE_BODY);
        // The bound falls just before a line break: the line is complete.
        let mut at_break = "x".repeat(MAX_PROBE_BODY);
        at_break.push_str("\r\nDisallow: /\n");
        assert_eq!(
            within_parsing_limit(at_break.as_bytes()).len(),
            MAX_PROBE_BODY
        );
        // One line longer than the bound: nothing is parsed.
        let long = "#".repeat(MAX_PROBE_BODY + 10);
        assert!(within_parsing_limit(long.as_bytes()).is_empty());
    }

    /// A 2xx over the bound cached by an earlier build, with no body, is
    /// asked for again rather than read as unavailable for the rest of its
    /// day.
    #[test]
    fn an_oversized_robots_file_cached_without_its_body_is_asked_for_again() {
        let home = tempfile::tempdir().unwrap();
        let cache = DeclarationCache::open(home.path());
        let now = Utc::now();
        cache.save(
            "host.example",
            &HostRecord {
                robots: Some(Probe {
                    url: "https://host.example/robots.txt".to_owned(),
                    fetched_at: now,
                    expires_at: now + ROBOTS_CACHE_AGE,
                    final_url: None,
                    status: Some(200),
                    body: None,
                    truncated: None,
                    error: Some("the body is 600000 bytes, over the bound".to_owned()),
                    not_sent: false,
                    declined_redirect: None,
                    cut_short: false,
                }),
                ..Default::default()
            },
        );
        let probe = |url: &str| -> Result<(String, commonmeasure_http::Response), ProbeFailure> {
            Ok((
                url.to_owned(),
                commonmeasure_http::Response::text(200, "User-agent: *\nDisallow: /\n"),
            ))
        };
        let read = read_robots(
            &cache,
            "https://host.example/a",
            now,
            &probe,
            None,
            PolicyMode::Observe,
        );
        assert_eq!(read.robots.cache, CacheDecision::Fetched);
        assert!(read.refused());
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
                truncated: None,
                error: None,
                not_sent: false,
                declined_redirect: None,
                cut_short: false,
            }),
            robots_held: None,
            licences: BTreeMap::new(),
            page_licences: BTreeMap::from([(
                "https://example.com/a".to_owned(),
                "https://example.com/licence.xml".to_owned(),
            )]),
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

/// When the manifest probe may take its turn on a host that states a delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestTurn {
    /// Only a turn that costs nothing: the probe is sent where the host is
    /// clear and is otherwise not sent. A paced host is asked this way at the
    /// start of a crossing, before the page's turn, because after the page
    /// the probe would need a whole delay from what the page left and a host
    /// stating more than half the budget would never be resolved at all.
    Free,
    /// A turn from what the call may still spend asleep, as the page takes
    /// one. A host that states no delay is unpaced either way.
    Paced,
}

/// Resolve the manifest for the host of `page_url`: from the cache while it
/// is live, otherwise by probing the well-known path and, on a 404 at a
/// subdomain, the apex. Every probe goes through the caller's mediated path.
///
/// `turn` says what the probe may spend on a host that states a delay.
pub fn resolve_manifest(
    cache: &ManifestCache,
    page_url: &str,
    now: DateTime<Utc>,
    probe: Prober<'_>,
    pacing: Option<&Pacing>,
    turn: ManifestTurn,
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
    let mut asked = true;
    let mut now = now;
    while let Some(url) = candidate.take() {
        // The manifest is a request to the same host as the page, so it
        // takes a turn under the host's delay. Where the turn is not there to
        // take, the probe is not sent and what is recorded is held only until
        // the host may next be asked, so the next crossing past the delay
        // asks the manifest at its first free turn instead of rewriting this
        // record on every crossing.
        if let Some(pacing) = pacing {
            let taken = match turn {
                ManifestTurn::Free => pacing.free_probe(&url, now),
                ManifestTurn::Paced => pacing.probe(&url, now),
            };
            match taken {
                Ok(sent_at) => now = sent_at,
                Err(reason) => {
                    probes.push(ManifestProbe { url, status: None });
                    outcome = ManifestOutcome::Unavailable { reason };
                    age = pacing
                        .delay_for(&host)
                        .and_then(|delay| chrono::Duration::from_std(delay).ok())
                        .unwrap_or_else(chrono::Duration::zero);
                    asked = false;
                    break;
                }
            }
        }
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
            Err(failure) => {
                probes.push(ManifestProbe { url, status: None });
                outcome = ManifestOutcome::Unavailable {
                    reason: failure.reason().to_owned(),
                };
                // A probe this edge cut short says nothing about the host.
                age = match failure {
                    ProbeFailure::CutShort(_) => chrono::Duration::zero(),
                    _ => FAILURE_CACHE_AGE,
                };
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
        cache: match asked {
            true => CacheDecision::Fetched,
            false => CacheDecision::NotAsked,
        },
        record,
    })
}

#[cfg(test)]
mod manifest_pacing_tests {
    use super::*;
    use crate::crawl_delay::{CrawlDelayStore, Pace, WAIT_BUDGET};
    use std::cell::Cell;

    const HOST: &str = "publisher.example";
    const PAGE: &str = "https://publisher.example/article";

    #[test]
    fn a_host_stating_45_seconds_has_its_manifest_resolved_by_a_later_crossing() {
        let home = tempfile::tempdir().unwrap();
        let store = CrawlDelayStore::open(home.path());
        let cache = ManifestCache::open(home.path());
        let asked = Cell::new(0);
        let probe = |url: &str| -> Result<(String, commonmeasure_http::Response), ProbeFailure> {
            asked.set(asked.get() + 1);
            Ok((
                url.to_owned(),
                commonmeasure_http::Response::text(404, "no manifest"),
            ))
        };
        let delay = std::time::Duration::from_secs(45);
        let now = Utc::now();

        // A crossing that finds the host inside its delay: the page, or
        // another tenant, has just taken the host's turn. The manifest takes
        // no wait from the page's budget, so it is not sent, and what is
        // recorded lasts until the host may next be asked rather than
        // expiring at once and being rewritten on every crossing.
        store.take_turn(HOST, delay, WAIT_BUDGET, now, Pace::Own);
        let busy = Pacing::new(store.clone(), Pace::Own);
        busy.learn(HOST, 45_000);
        let unasked =
            resolve_manifest(&cache, PAGE, now, &probe, Some(&busy), ManifestTurn::Free).unwrap();
        assert_eq!(unasked.cache, CacheDecision::NotAsked);
        assert_eq!(
            unasked.record.expires_at,
            now + chrono::Duration::seconds(45)
        );
        assert!(
            matches!(&unasked.record.outcome, ManifestOutcome::Unavailable { reason } if reason.starts_with("not sent: ")),
            "{unasked:?}"
        );
        assert_eq!(asked.get(), 0, "nothing was sent");

        // A second crossing inside the delay reuses the record: no probe, no
        // rewrite.
        let soon = now + chrono::Duration::seconds(10);
        let again = Pacing::new(store.clone(), Pace::Own);
        again.learn(HOST, 45_000);
        let reused =
            resolve_manifest(&cache, PAGE, soon, &probe, Some(&again), ManifestTurn::Free).unwrap();
        assert_eq!(reused.cache, CacheDecision::Reused);
        assert_eq!(asked.get(), 0);

        // A later crossing past the delay finds the host clear, and the
        // manifest takes that crossing's first free turn.
        let later = now + chrono::Duration::seconds(50);
        let clear = Pacing::new(store.clone(), Pace::Own);
        clear.learn(HOST, 45_000);
        let resolved = resolve_manifest(
            &cache,
            PAGE,
            later,
            &probe,
            Some(&clear),
            ManifestTurn::Free,
        )
        .unwrap();
        assert_eq!(resolved.cache, CacheDecision::Fetched);
        assert_eq!(resolved.record.outcome, ManifestOutcome::NotPublished);
        assert_eq!(resolved.record.probes[0].status, Some(404));
        assert!(asked.get() >= 1);
        // The free turn was taken, so the page waits a whole delay behind
        // it, and none of the call's budget has been spent. Asked with no
        // budget, the store reports the wait without sleeping it.
        let page = store.take_turn(HOST, delay, std::time::Duration::ZERO, later, Pace::Own);
        assert_eq!(page.wait_ms, Some(45_000), "{page:?}");
        assert!(clear.left() > std::time::Duration::from_secs(200));
    }
}

#[cfg(test)]
mod licence_first_tests {
    use super::*;
    use crate::crawl_delay::{CrawlDelayStore, Pace};
    use std::cell::RefCell;
    use std::time::Duration;

    const HOST: &str = "publisher.example";
    const PAGE: &str = "https://publisher.example/article";
    const LICENCE: &str = "https://publisher.example/license.xml";
    const RSL: &str = r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/"><license><permits type="usage">ai-input</permits></license></content></rsl>"#;

    /// A publisher whose `robots.txt` states `delay` seconds, optionally
    /// naming the licence, and which logs every request it is sent.
    fn publisher(robots_licence: bool, delay: u64) -> (String, RefCell<Vec<String>>) {
        let licence = if robots_licence {
            format!("License: {LICENCE}\n")
        } else {
            String::new()
        };
        (
            format!("{licence}User-agent: *\nAllow: /\nCrawl-delay: {delay}\n"),
            RefCell::new(Vec::new()),
        )
    }

    fn answer(
        robots: &str,
        log: &RefCell<Vec<String>>,
        url: &str,
    ) -> Result<(String, commonmeasure_http::Response), ProbeFailure> {
        log.borrow_mut().push(url.to_owned());
        let body = if url.ends_with("/robots.txt") {
            robots
        } else if url == LICENCE {
            RSL
        } else {
            "the page"
        };
        Ok((
            url.to_owned(),
            commonmeasure_http::Response::text(200, body),
        ))
    }

    /// On a hosted edge a host stating 30 seconds with an uncached licence
    /// cannot have licence and page read inside the 20 seconds one call may
    /// wait: refused before anything is sent, with no instant disclosed.
    #[test]
    fn a_hosted_crossing_that_cannot_fit_licence_then_page_asks_for_nothing() {
        let home = tempfile::tempdir().unwrap();
        let cache = DeclarationCache::open(home.path());
        let (robots, log) = publisher(true, 30);
        let probe = |url: &str| answer(&robots, &log, url);
        let pacing = Pacing::new(CrawlDelayStore::open(home.path()), Pace::Hosted);
        let now = Utc::now();
        let read = read_robots(
            &cache,
            PAGE,
            now,
            &probe,
            Some(&pacing),
            PolicyMode::Observe,
        );
        let mut declarations = read_declared(&cache, read, PAGE, None, now, &probe, Some(&pacing));
        assert!(!take_page_turn(&mut declarations, &pacing, now));
        assert_eq!(
            *log.borrow(),
            ["https://publisher.example/robots.txt"],
            "only the file the delay is read from"
        );
        let delay = declarations.robots.delay.as_ref().expect("a ruling");
        assert_eq!(delay.outcome, DelayOutcome::Refused);
        assert_eq!(delay.licence_first.as_deref(), Some(LICENCE));
        assert!(
            delay.wait_ms.is_none() && delay.next_at.is_none(),
            "{delay:?}"
        );
        assert_eq!(declarations.licences[0].cache, CacheDecision::NotAsked);
        assert!(declarations.licences[0].unread);
        let reason = declarations.robots.delay_refusal().expect("a refusal");
        assert!(reason.contains("has not been read"), "{reason}");
        assert!(reason.contains("hosted edge"), "{reason}");
    }

    /// A licence a page's response names is only known once the page has
    /// answered. Where its turn does not fit what the call has left it is not
    /// read, the page's bytes are not admitted under it, and the host record
    /// keeps which licence the page named so the next crossing reads it
    /// before the page.
    #[test]
    fn a_link_licence_not_read_after_the_page_is_read_before_it_next_time() {
        let home = tempfile::tempdir().unwrap();
        let cache = DeclarationCache::open(home.path());
        let store = CrawlDelayStore::open(home.path());
        let (robots, log) = publisher(false, 3);
        let probe = |url: &str| answer(&robots, &log, url);
        let now = Utc::now();
        let mut page = commonmeasure_http::Response::text(200, "the page");
        page.headers.set(
            "Link",
            &format!("<{LICENCE}>; rel=\"license\"; type=\"application/rsl+xml\""),
        );

        // Two seconds left to wait, and the licence needs the three after
        // the page's turn.
        let short = Pacing::with_budget(store.clone(), Duration::from_secs(2), Pace::Own);
        let read = read_robots(&cache, PAGE, now, &probe, Some(&short), PolicyMode::Observe);
        let mut first = read_declared(&cache, read, PAGE, None, now, &probe, Some(&short));
        assert!(take_page_turn(&mut first, &short, now));
        after_fetch(&cache, &mut first, PAGE, &page, now, &probe, Some(&short));
        let unread = &first.licences[0];
        assert_eq!(unread.mechanism, LicenceMechanism::LinkHeader);
        assert_eq!(unread.cache, CacheDecision::NotAsked);
        assert!(unread.unread, "{unread:?}");
        assert!(!log.borrow().iter().any(|url| url == LICENCE));
        assert_eq!(
            cache.load(HOST).page_licences.get(PAGE).map(String::as_str),
            Some(LICENCE)
        );

        // Past the delay, the licence the page named is read before the page.
        let later = now + chrono::Duration::seconds(10);
        let pacing = Pacing::new(store.clone(), Pace::Own);
        let read = read_robots(
            &cache,
            PAGE,
            later,
            &probe,
            Some(&pacing),
            PolicyMode::Observe,
        );
        assert!(read.reads_a_licence_first(PAGE, later));
        let mut second = read_declared(&cache, read, PAGE, None, later, &probe, Some(&pacing));
        assert_eq!(log.borrow().last().map(String::as_str), Some(LICENCE));
        assert!(
            second.robots.delay.is_none(),
            "the page's turn is still to come"
        );
        // The response names it again, and the reading is reused: no turn.
        after_fetch(
            &cache,
            &mut second,
            PAGE,
            &page,
            later,
            &probe,
            Some(&pacing),
        );
        let read = &second.licences[0];
        assert_eq!(read.cache, CacheDecision::Reused);
        assert!(!read.unread && read.terms.is_some(), "{read:?}");
    }
}
