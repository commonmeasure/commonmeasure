use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Money;

/// How hard the operator wants policy enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    /// Record the operator's policy, not enforce it; refuse only on the
    /// source's terms.
    Observe,
    /// Prefer eligible routes, but let an ineligible one through with the
    /// breach recorded.
    Prefer,
    /// Refuse an ineligible crossing before context reaches the model.
    Strict,
}

/// What the operator is optimising for. `Weighted` is the explicit utility of
/// `ARCHITECTURE.md`; its weights belong to the operator and are recorded in
/// the run manifest so a selection can be re-derived rather than trusted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Objective {
    MaximiseQuality,
    MinimiseCost,
    MinimiseLatency,
    Weighted {
        quality: f64,
        coverage: f64,
        freshness: f64,
        cost: f64,
        latency: f64,
        policy_risk: f64,
    },
}

/// A hard constraint. Hard constraints filter before anything is scored.
///
/// Every kind but one has set semantics: the list of denied hosts is a set,
/// and its order means nothing. `access_rule` is the exception. Access rules
/// are read in declaration order and the first whose pattern matches the
/// source's host decides; a later rule for the same host is never reached.
/// Order among access rules is therefore part of the policy, and an editor
/// that reorders them changes what is enforced.
///
/// An internally tagged enum cannot carry `deny_unknown_fields`, but every
/// variant's payload is mandatory, so a misspelled key already fails as the
/// mandatory one gone missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Constraint {
    AllowedProvider {
        provider: String,
    },
    DeniedProvider {
        provider: String,
    },
    /// Sources delivered by this adapter may pass the allowed-host list.
    /// Explicit host denials, ordered access rules and licences still apply.
    AllowedSourceProvider {
        provider: String,
    },
    AllowedSourceHost {
        host: String,
    },
    DeniedSourceHost {
        host: String,
    },
    RequiredLicence {
        licence: String,
    },
    MaximumAcquisitionCost {
        amount: Money,
    },
    MaximumContextTokens {
        tokens: u64,
    },
    MaximumLatencyMs {
        milliseconds: u64,
    },
    /// One ordered access rule: what happens to a source whose host matches
    /// `host`. Written as `{"kind": "access_rule", "host": "*.example.com",
    /// "action": "refuse"}`; a `require_licence` action also names the
    /// `licence` it requires.
    AccessRule {
        host: HostPattern,
        #[serde(flatten)]
        action: AccessAction,
    },
}

/// What an access rule does to the sources its pattern matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AccessAction {
    /// The host passes host policy: no later access rule and no allowed-host
    /// list is consulted for it. The denied-host list has already been
    /// applied by then, and a required licence still applies, because a
    /// licence is evidence about the content and not a fact about the host.
    Allow,
    /// The source is a breach: refused under `strict`, carried with the
    /// breach recorded under `observe` and `prefer`.
    Refuse,
    /// The source is admitted only when the supplier declared exactly this
    /// licence. An unknown licence is unknown, never permitted.
    RequireLicence { licence: String },
    /// Accepted by the loader and met at admission, because every crossing
    /// that reaches admission is a crossing Common Measure governed. No other
    /// path reads this rule: an observed crossing is not checked against it.
    RequireMediation,
}

impl AccessAction {
    /// The word an evidence record and a console use for this action.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Refuse => "refuse",
            Self::RequireLicence { .. } => "require_licence",
            Self::RequireMediation => "require_mediation",
        }
    }
}

/// The host part of an access rule: which hosts the rule is about.
///
/// Three forms, and nothing else parses. An exact host (`docs.example.com`)
/// matches that host alone. A wildcard host (`*.example.com`) matches
/// `example.com` and every host beneath it, because an operator refusing a
/// domain means the whole domain and a rule that spared the apex would
/// surprise them. `*` alone matches every source that has a host. A source
/// with no host, such as a document from the operator's own corpus, matches
/// no pattern: there is nothing for the rule to be about.
///
/// The host part is kept in the normalised form admission compares against
/// (lowercase, IDNA-encoded, no trailing dot), so a pattern the operator
/// wrote as `Docs.Example.COM.` names the host the record calls
/// `docs.example.com`. An empty or unparseable pattern is refused when it is
/// read, which is what makes the loader the place a bad rule is caught.
/// The schema form is the string an operator writes, because that is what
/// `try_from` parses; the two fields below are the parsed result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub struct HostPattern {
    /// The normalised host, or the normalised suffix for a wildcard; empty
    /// for the pattern that matches every host.
    host: String,
    wildcard: bool,
}

impl HostPattern {
    /// Parse a pattern as an operator wrote it.
    pub fn parse(pattern: &str) -> Result<Self, String> {
        let written = pattern.trim();
        if written.is_empty() {
            return Err("an access rule's host pattern cannot be empty".to_owned());
        }
        if written == "*" {
            return Ok(Self {
                host: String::new(),
                wildcard: true,
            });
        }
        let (wildcard, host) = match written.strip_prefix("*.") {
            Some(rest) => (true, rest),
            None => (false, written),
        };
        if host.contains('*') {
            return Err(format!(
                "access rule host pattern {written:?} is not a host, \"*.host\" or \"*\"; a wildcard \
                 stands only at the front"
            ));
        }
        let normalised = parsed_host(host)
            .ok_or_else(|| format!("access rule host pattern {written:?} does not name a host"))?;
        Ok(Self {
            host: normalised,
            wildcard,
        })
    }

    /// Whether this pattern is about `host`, which must already be in the
    /// normalised form the envelope carries.
    pub fn matches(&self, host: &str) -> bool {
        if host.is_empty() {
            return false;
        }
        if !self.wildcard {
            return host == self.host;
        }
        if self.host.is_empty() {
            return true;
        }
        host == self.host
            || host
                .strip_suffix(&self.host)
                .is_some_and(|prefix| prefix.ends_with('.'))
    }

    /// The pattern as an operator would write it back.
    pub fn as_written(&self) -> String {
        match (self.wildcard, self.host.is_empty()) {
            (true, true) => "*".to_owned(),
            (true, false) => format!("*.{}", self.host),
            (false, _) => self.host.clone(),
        }
    }
}

/// The operator's spelling of a host, reduced to the form an envelope's host
/// arrives in: `Url::host_str` of `https://{entry}/`, lowercased,
/// IDNA-encoded, trailing dots removed. Both sides of every host
/// comparison, in admission and in a pattern, go through this one function,
/// so "the same host" has one definition. An entry the parser rejects is
/// compared as written, lowercased, because losing a denial to a typo is the
/// worse failure; a pattern, which must name a host, refuses such an entry
/// instead ([`HostPattern::parse`]).
pub fn normalised_host(entry: &str) -> String {
    parsed_host(entry).unwrap_or_else(|| entry.trim().to_lowercase())
}

/// `url` with a domain host in [`normalised_host`]'s form, for comparing
/// whole URLs or URL prefixes. For http and https the parser has already
/// lowered the host's case, IDNA-encoded it and dropped the scheme's default
/// port; this also removes a fully qualified name's trailing dots, because
/// `https://example.com./a` is the resource `https://example.com/a` is. It
/// clears a username and password: credentials authenticate a request and
/// do not name a different resource, so `https://u:p@example.com/a` is
/// `https://example.com/a` too. An address host is left as parsed. The
/// scheme, any other port, the path and the query are kept, since each names
/// a different resource.
pub fn canonical_url(url: &url::Url) -> url::Url {
    let mut canonical = url.clone();
    // Both setters refuse only a URL that cannot carry credentials, which
    // then has none to clear.
    let _ = canonical.set_username("");
    let _ = canonical.set_password(None);
    if let Some(url::Host::Domain(domain)) = url.host() {
        let host = normalised_host(domain);
        // A host the setter refuses leaves the URL as parsed.
        if host != domain && !host.is_empty() {
            let _ = canonical.set_host(Some(&host));
        }
    }
    canonical
}

/// The form in which a URL is compared with a publisher's or an operator's
/// rule: [`canonical_url`], its path and query in [`matching_target`]'s form,
/// and no fragment, which the client never sends. `https://h/%6Eews/1#x` and
/// `https://h/news/1` are one resource, so a rule covering either covers
/// both. A rule written as a URL (an RSL absolute scope, an internal prefix)
/// is compared in this form too. The URL requested and the URL recorded are
/// not this form; only the comparison is.
///
/// This is a page's first reading; [`matching_url_readings`] gives all of
/// them. Dot segments need nothing here: the parser has already resolved
/// them, including `%2E` and `%2e` spellings (`/a/%2E%2E/b` parses as `/b`),
/// and the path it serialises is the path the client requests.
pub fn matching_url(url: &url::Url) -> String {
    let (origin, target) = origin_and_target(url);
    format!("{origin}{}", matching_target(&target))
}

/// Every reading of a page URL that a rule is compared with, in
/// [`matching_url`]'s form: the origin with each of
/// [`matching_target_readings`]. A rule covers the page if it covers a
/// reading; how the readings combine is the caller's ruling.
pub fn matching_url_readings(url: &url::Url) -> Result<Vec<String>, TooManyReadings> {
    let (origin, target) = origin_and_target(url);
    Ok(matching_target_readings(&target)?
        .into_iter()
        .map(|reading| format!("{origin}{reading}"))
        .collect())
}

fn origin_and_target(url: &url::Url) -> (String, String) {
    let mut canonical = canonical_url(url);
    canonical.set_fragment(None);
    (
        canonical[..url::Position::BeforePath].to_owned(),
        canonical[url::Position::BeforePath..].to_owned(),
    )
}

/// The most paths [`matching_target_readings`] reaches before it gives up.
/// Each operation leaves a path unchanged or shorter, so the closure ends,
/// but stripping parameters can expose a dot segment after resolving has
/// run, so the number of paths has no small proven bound. A crafted path
/// can reach the cap; past it the page is refused. Every path of a `/`
/// followed by up to six of `/`, `%2F`, `%5C`, `..`, `.`, `a`, `;` and `%3B`
/// reaches at most 28 paths, and at most 24 as the URL parser leaves it.
pub const MAX_PAGE_READINGS: usize = 64;

/// A page path that reaches more than [`MAX_PAGE_READINGS`] paths. Its
/// readings cannot all be compared, so a caller rules the page as if a
/// refusing rule covered every one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooManyReadings {
    /// The cap the path went past.
    pub cap: usize,
}

impl std::fmt::Display for TooManyReadings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the page's path has more than {} readings (decoding `%2F`, `%5C` and `%3B`, \
             merging `/`, resolving dot segments and stripping `;` parameters, in any order), so \
             it is refused without comparing them",
            self.cap
        )
    }
}

impl std::error::Error for TooManyReadings {}

/// The readings of a page's request target, each in [`matching_target`]'s
/// form, without repeats: the target as parsed, and every path reachable
/// from it by these operations, in any order, any number of times, with
/// the paths on the way included, since a server may apply only some of
/// them:
///
/// - decode: `%2F` or `%5C` (either case) read as `/`, as a server that
///   decodes the path before routing serves `/news%2F1`, and as IIS reads
///   `%5C`; `%3B` (either case) read as `;`, as a proxy that decodes the
///   path before a Java server does, and then stripped (nginx, when
///   `proxy_pass` names a URI, in front of Tomcat serves `/news%3Bx/1` as
///   `/news/1`);
/// - merge: each run of `/` read as one `/`, as nginx (`merge_slashes`) and
///   Python's `http.server` serve `//news/1`;
/// - resolve: dot segments resolved as the URL parser resolves them, `%2E`
///   spellings included;
/// - strip: in each segment, everything from the first raw `;` removed, as
///   Tomcat, Jetty and Undertow serve `/news;x=1/1` as `/news/1`.
///
/// So `/x//..%2Fnews/1` reads as `/news/1` (merged before resolving, as
/// nginx does) and as `/x/news/1` (resolved first, as the URL parser does),
/// and `/x/..;/news/1` reads as `/news/1` (stripped, then resolved).
/// The order is breadth-first: the target as parsed, then the paths one
/// operation away, then two, each step trying decode, merge, resolve and
/// strip in that order. Only the path is read; the query is the same in
/// every reading. Only a page is read these ways: a rule names the path its
/// author wrote, so a rule's `//`, `%2F`, `%3B` or `;` is never merged,
/// decoded or stripped.
///
/// A path that reaches more than [`MAX_PAGE_READINGS`] paths is an error,
/// which callers rule closed.
pub fn matching_target_readings(target: &str) -> Result<Vec<String>, TooManyReadings> {
    readings_within(target, MAX_PAGE_READINGS)
}

fn readings_within(target: &str, cap: usize) -> Result<Vec<String>, TooManyReadings> {
    let (path, query) = target.split_at(target.find('?').unwrap_or(target.len()));
    let operations: [fn(&str) -> String; 4] = [
        decoded_separators,
        merged_slashes,
        resolved_dot_segments,
        stripped_parameters,
    ];
    let mut paths = vec![path.to_owned()];
    let mut next = 0;
    while let Some(current) = paths.get(next).cloned() {
        next += 1;
        for operation in operations {
            let reached = operation(&current);
            if !paths.contains(&reached) {
                if paths.len() == cap {
                    return Err(TooManyReadings { cap });
                }
                paths.push(reached);
            }
        }
    }
    let mut readings: Vec<String> = Vec::with_capacity(paths.len());
    for path in paths {
        let reading = matching_target(&format!("{path}{query}"));
        if !readings.contains(&reading) {
            readings.push(reading);
        }
    }
    Ok(readings)
}

fn merged_slashes(path: &str) -> String {
    let mut merged = String::with_capacity(path.len());
    for character in path.chars() {
        if character != '/' || !merged.ends_with('/') {
            merged.push(character);
        }
    }
    merged
}

/// `path` with `%2F` and `%5C` read as `/` and `%3B` as `;`, either case.
/// Nothing it writes is a `%`, so decoding a decoded path changes nothing.
fn decoded_separators(path: &str) -> String {
    let mut decoded = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(at) = rest.find('%') {
        decoded.push_str(&rest[..at]);
        let escape = rest.get(at..at + 3);
        if let Some(delimiter) = escape.and_then(|escape| {
            if escape.eq_ignore_ascii_case("%2F") || escape.eq_ignore_ascii_case("%5C") {
                Some('/')
            } else if escape.eq_ignore_ascii_case("%3B") {
                Some(';')
            } else {
                None
            }
        }) {
            decoded.push(delimiter);
            rest = &rest[at + 3..];
        } else {
            decoded.push('%');
            rest = &rest[at + 1..];
        }
    }
    decoded.push_str(rest);
    decoded
}

/// `path` with each segment's path parameters removed: from the first raw
/// `;` to the end of the segment, as Tomcat, Jetty and Undertow read
/// `/news;x=1/1` as `/news/1` before routing. They do not strip at an
/// encoded `;` (`%3B`), but a proxy in front of them may decode it first, so
/// decoding reads `%3B` as `;` and this then strips it.
fn stripped_parameters(path: &str) -> String {
    path.split('/')
        .map(|segment| segment.split(';').next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("/")
}

/// `path` with its dot segments resolved by the URL parser, `%2E` spellings
/// included, so the reading is the path a server that resolves them serves.
fn resolved_dot_segments(path: &str) -> String {
    let mut url = url::Url::parse("http://h/").expect("a literal URL parses");
    url.set_path(path);
    url.path().to_owned()
}

/// A request target (path and optional query) in the one spelling rules are
/// matched in, following RFC 3986 section 6.2.2 and RFC 9309 section 2.2.2:
///
/// - a percent-encoded unreserved octet (`ALPHA DIGIT - . _ ~`) is decoded,
///   so `/%6Eews` is `/news`;
/// - every other percent-encoding keeps its encoding with upper-case hex, so
///   `%2f` is `%2F`, and `%2F` and `%3F` never become the `/` and `?` that
///   separate a path;
/// - a literal `*` or `$` is `%2A` or `%24`, the only spelling in which a
///   robots pattern can name one (RFC 9309 section 2.2.3), since a bare `*`
///   or final `$` in a pattern is an operator;
/// - an octet that is neither unreserved nor reserved, a non-ASCII byte
///   among them, is percent-encoded, so `/café` in a pattern is the
///   `/caf%C3%A9` a parsed URL carries;
/// - a `%` that starts no encoding (`%zz`, `%+1`, a trailing `%`) is `%25`.
///
/// Slashes are kept as written: a doubled slash is a page reading
/// ([`matching_target_readings`]), never a change to a rule.
pub fn matching_target(target: &str) -> String {
    let bytes = target.as_bytes();
    let mut form = String::with_capacity(target.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let encoded = (byte == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        if let Some(octet) = encoded {
            if is_unreserved(octet) {
                form.push(char::from(octet));
            } else {
                push_encoded(&mut form, octet);
            }
            index += 3;
            continue;
        }
        match byte {
            b'*' | b'$' | b'%' => push_encoded(&mut form, byte),
            _ if is_unreserved(byte) || is_reserved(byte) => form.push(char::from(byte)),
            _ => push_encoded(&mut form, byte),
        }
        index += 1;
    }
    form
}

/// An RFC 9309 path pattern (a robots `Allow`, `Disallow` or `Content-Usage`
/// path, or an RSL relative scope) in [`matching_target`]'s form: each
/// literal part between the `*` wildcards and before a final `$` anchor is
/// normalised, and the wildcards and the anchor are kept. A `$` that is not
/// final is a literal and is `%24`, as in a URL. A pattern is ranked by the
/// length of this form, so a spelling cannot make a rule more specific than
/// the path it names.
pub fn matching_pattern(pattern: &str) -> String {
    let (literal, anchored) = match pattern.strip_suffix('$') {
        Some(stripped) => (stripped, true),
        None => (pattern, false),
    };
    let mut form = literal
        .split('*')
        .map(matching_target)
        .collect::<Vec<_>>()
        .join("*");
    if anchored {
        form.push('$');
    }
    form
}

fn is_unreserved(octet: u8) -> bool {
    octet.is_ascii_alphanumeric() || matches!(octet, b'-' | b'.' | b'_' | b'~')
}

fn is_reserved(octet: u8) -> bool {
    matches!(
        octet,
        b':' | b'/'
            | b'?'
            | b'#'
            | b'['
            | b']'
            | b'@'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
    )
}

fn push_encoded(form: &mut String, octet: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    form.push('%');
    form.push(char::from(HEX[usize::from(octet >> 4)]));
    form.push(char::from(HEX[usize::from(octet & 0x0F)]));
}

/// The host `entry` names, when it names one.
fn parsed_host(entry: &str) -> Option<String> {
    let host = url::Url::parse(&format!("https://{}/", entry.trim()))
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))?;
    let host = host.trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_owned())
}

impl TryFrom<String> for HostPattern {
    type Error = String;

    fn try_from(pattern: String) -> Result<Self, Self::Error> {
        Self::parse(&pattern)
    }
}

impl From<HostPattern> for String {
    fn from(pattern: HostPattern) -> Self {
        pattern.as_written()
    }
}

impl std::fmt::Display for HostPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_written())
    }
}

/// Evidence the job says it needs. Whether it was obtained is a run outcome,
/// never an assumption.
///
/// `required` is declared, not enforced: it is projected into the run record
/// and shown to the reader, and no refusal, filter or verdict turns on it. It
/// says what the operator asked for, not what the runtime guaranteed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRequirement {
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

/// The unit of work. A job declares the task, the policy and the outcome
/// contract; it never names provider endpoints or wire details.
///
/// Unknown fields are load errors. Every list here defaults to empty, so a
/// misspelled `"contraints"` would otherwise deserialise into a job with no
/// policy at all and pass validation — enforcement silently absent because of
/// a typo nobody was told about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextJob {
    pub id: Uuid,
    pub kind: String,
    pub prompt: String,
    pub policy_mode: PolicyMode,
    pub objective: Objective,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    #[serde(default)]
    pub evidence_requirements: Vec<EvidenceRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    EmptyKind,
    EmptyPrompt,
    ConflictingProviderPolicy(String),
    ConflictingSourcePolicy(String),
    MixedBudgetCurrencies,
    DuplicateConstraint(&'static str),
    NegativeObjectiveWeight(&'static str),
    /// An access rule that could never be met as written: the rule's index
    /// among the constraints and the reason.
    InvalidAccessRule(usize, String),
}

impl ContextJob {
    /// Reject a job whose policy contradicts itself before it can be
    /// half-enforced. A provider that is both allowed and denied, or two
    /// budgets in different currencies, has no defensible reading, and
    /// choosing one at execution time would make the record unexplainable.
    ///
    /// A second cap, token ceiling or latency ceiling is refused for the same
    /// reason: each is read by first declaration, so a tighter one written
    /// later would be dropped from enforcement and from the record without
    /// anyone being told. The list constraints — providers, hosts, licences —
    /// are read whole and are meant to repeat.
    ///
    /// A negative objective weight is refused because nothing in the runtime
    /// gives one a meaning: the fraction arm's arithmetic would rank the
    /// least-covered window first while the single-term cost and latency
    /// arms defer to their minimising selectors whatever the sign — two
    /// readings of one field, neither of which a record could explain.
    pub fn validate(&self) -> Result<(), JobError> {
        if self.kind.trim().is_empty() {
            return Err(JobError::EmptyKind);
        }
        if self.prompt.trim().is_empty() {
            return Err(JobError::EmptyPrompt);
        }
        if let Objective::Weighted {
            quality,
            coverage,
            freshness,
            cost,
            latency,
            policy_risk,
        } = &self.objective
        {
            for (name, weight) in [
                ("quality", quality),
                ("coverage", coverage),
                ("freshness", freshness),
                ("cost", cost),
                ("latency", latency),
                ("policy_risk", policy_risk),
            ] {
                if *weight < 0.0 {
                    return Err(JobError::NegativeObjectiveWeight(name));
                }
            }
        }

        let mut budget_currency: Option<&str> = None;
        let mut declared: Vec<&'static str> = Vec::new();
        for (index, constraint) in self.constraints.iter().enumerate() {
            if let Constraint::AccessRule { action, .. } = constraint
                && let Err(reason) = action.validate()
            {
                return Err(JobError::InvalidAccessRule(index, reason));
            }
            if let Constraint::MaximumAcquisitionCost { amount } = constraint {
                match budget_currency {
                    Some(currency) if currency != amount.currency() => {
                        return Err(JobError::MixedBudgetCurrencies);
                    }
                    None => budget_currency = Some(amount.currency()),
                    _ => {}
                }
            }
            let scalar = match constraint {
                Constraint::MaximumAcquisitionCost { .. } => "maximum_acquisition_cost",
                Constraint::MaximumContextTokens { .. } => "maximum_context_tokens",
                Constraint::MaximumLatencyMs { .. } => "maximum_latency_ms",
                _ => continue,
            };
            if declared.contains(&scalar) {
                return Err(JobError::DuplicateConstraint(scalar));
            }
            declared.push(scalar);
        }

        if let Some(provider) = self
            .allowed_providers()
            .find(|candidate| self.denied_providers().any(|denied| denied == *candidate))
        {
            return Err(JobError::ConflictingProviderPolicy(provider.to_owned()));
        }
        if let Some(host) = self.allowed_source_hosts().find(|candidate| {
            self.denied_source_hosts()
                .any(|denied| denied == *candidate)
        }) {
            return Err(JobError::ConflictingSourcePolicy(host.to_owned()));
        }
        Ok(())
    }

    pub fn allowed_providers(&self) -> impl Iterator<Item = &str> {
        self.constraints.iter().filter_map(|c| match c {
            Constraint::AllowedProvider { provider } => Some(provider.as_str()),
            _ => None,
        })
    }

    pub fn denied_providers(&self) -> impl Iterator<Item = &str> {
        self.constraints.iter().filter_map(|c| match c {
            Constraint::DeniedProvider { provider } => Some(provider.as_str()),
            _ => None,
        })
    }

    /// Whether this trusted adapter's sources may pass the allowed-host list.
    /// This does not authorise dispatch to a provider otherwise excluded.
    pub fn allows_provider_sources(&self, provider: &str) -> bool {
        self.constraints.iter().any(|constraint| {
            matches!(constraint, Constraint::AllowedSourceProvider { provider: allowed } if allowed == provider)
        }) && !self.denied_providers().any(|denied| denied == provider)
            && (self.allowed_providers().next().is_none()
                || self.allowed_providers().any(|allowed| allowed == provider))
    }

    pub fn allowed_source_hosts(&self) -> impl Iterator<Item = &str> {
        self.constraints.iter().filter_map(|c| match c {
            Constraint::AllowedSourceHost { host } => Some(host.as_str()),
            _ => None,
        })
    }

    pub fn denied_source_hosts(&self) -> impl Iterator<Item = &str> {
        self.constraints.iter().filter_map(|c| match c {
            Constraint::DeniedSourceHost { host } => Some(host.as_str()),
            _ => None,
        })
    }

    pub fn maximum_acquisition_cost(&self) -> Option<&Money> {
        self.constraints.iter().find_map(|c| match c {
            Constraint::MaximumAcquisitionCost { amount } => Some(amount),
            _ => None,
        })
    }

    pub fn maximum_context_tokens(&self) -> Option<u64> {
        self.constraints.iter().find_map(|c| match c {
            Constraint::MaximumContextTokens { tokens } => Some(*tokens),
            _ => None,
        })
    }

    pub fn maximum_latency_ms(&self) -> Option<u64> {
        self.constraints.iter().find_map(|c| match c {
            Constraint::MaximumLatencyMs { milliseconds } => Some(*milliseconds),
            _ => None,
        })
    }

    pub fn required_licences(&self) -> impl Iterator<Item = &str> {
        self.constraints.iter().filter_map(|c| match c {
            Constraint::RequiredLicence { licence } => Some(licence.as_str()),
            _ => None,
        })
    }

    /// The access rules in declaration order, each with its position among
    /// the constraints so a ruling can name the rule it applied.
    pub fn access_rules(&self) -> impl Iterator<Item = (usize, &HostPattern, &AccessAction)> {
        self.constraints
            .iter()
            .enumerate()
            .filter_map(|(index, c)| match c {
                Constraint::AccessRule { host, action } => Some((index, host, action)),
                _ => None,
            })
    }

    /// The first access rule whose pattern matches `host`, which must be in
    /// the normalised form the envelope carries. First match wins; nothing
    /// after it is read.
    pub fn access_rule_for(&self, host: &str) -> Option<(usize, &HostPattern, &AccessAction)> {
        self.access_rules()
            .find(|(_, pattern, _)| pattern.matches(host))
    }
}

impl AccessAction {
    /// Whether the action can be met as written. A `require_licence` naming
    /// no licence could match no declaration and would refuse every source
    /// under it for a reason the operator never wrote.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::RequireLicence { licence } if licence.trim().is_empty() => {
                Err("a require_licence access rule must name the licence it requires".to_owned())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The closure reaches nginx's order (merge before resolving:
    /// `/news/1`), the URL parser's (`/x/news/1`), and each path on the
    /// way, breadth-first.
    #[test]
    fn a_path_is_read_in_every_order_of_decoding_merging_and_resolving() {
        for (target, readings) in [
            (
                "/x//..%2Fnews/1",
                vec![
                    "/x//..%2Fnews/1",
                    "/x//../news/1",
                    "/x/..%2Fnews/1",
                    "/x/../news/1",
                    "/x/news/1",
                    "/news/1",
                ],
            ),
            (
                "/a/b//..%2F%2F..%2Fc//..%2Fnews//1?q=//%2F",
                vec![
                    "/a/b//..%2F%2F..%2Fc//..%2Fnews//1?q=//%2F",
                    "/a/b//..//../c//../news//1?q=//%2F",
                    "/a/b/..%2F%2F..%2Fc/..%2Fnews/1?q=//%2F",
                    "/a/b/../../c/../news/1?q=//%2F",
                    "/a/b/c/news//1?q=//%2F",
                    "/a/b/..//../c/../news/1?q=//%2F",
                    "/news/1?q=//%2F",
                    "/a/b/c/news/1?q=//%2F",
                    "/a/news/1?q=//%2F",
                ],
            ),
        ] {
            assert_eq!(
                matching_target_readings(target),
                Ok(readings.iter().map(|r| r.to_string()).collect()),
                "{target}"
            );
        }
    }

    /// Stripping a `;` parameter can expose a dot segment that resolving
    /// then removes, `%5C` decodes as `%2F` does, and `%3B` decodes to a
    /// `;` that is then stripped.
    #[test]
    fn a_path_is_read_with_its_parameters_stripped_and_its_backslashes_decoded() {
        for (target, readings) in [
            (
                "/x/..;/news/1",
                vec!["/x/..;/news/1", "/x/../news/1", "/news/1"],
            ),
            (
                "/a%5C..%2Fnews;p/1",
                vec![
                    "/a%5C..%2Fnews;p/1",
                    "/a/../news;p/1",
                    "/a%5C..%2Fnews/1",
                    "/news;p/1",
                    "/a/../news/1",
                    "/news/1",
                ],
            ),
            ("/news/1;x?q=;y", vec!["/news/1;x?q=;y", "/news/1?q=;y"]),
            ("/news;a;b/1", vec!["/news;a;b/1", "/news/1"]),
            ("/news%3Bx/1", vec!["/news%3Bx/1", "/news;x/1", "/news/1"]),
            ("/news%3bx/1", vec!["/news%3Bx/1", "/news;x/1", "/news/1"]),
            (
                "/x/..%3B/news/1",
                vec![
                    "/x/..%3B/news/1",
                    "/x/..;/news/1",
                    "/x/../news/1",
                    "/news/1",
                ],
            ),
            ("/news%5c1", vec!["/news%5C1", "/news/1"]),
        ] {
            assert_eq!(
                matching_target_readings(target),
                Ok(readings.iter().map(|r| r.to_string()).collect()),
                "{target}"
            );
        }
    }

    /// The operations have no small proven bound once stripping can expose
    /// a dot segment, so this measures one: every path of a `/` followed by
    /// up to six of these pieces, as written and as parsed, stays within the
    /// cap. The most any reaches is 28 paths as written (`//%2F%3B;/.`) and
    /// 24 as parsed (`//%2F./.;`).
    #[test]
    fn no_short_path_reaches_the_cap() {
        const PIECES: [&str; 8] = ["/", "%2F", "%5C", "..", ".", "a", ";", "%3B"];
        let mut paths = vec![String::from("/")];
        let mut from = 0;
        for _ in 0..6 {
            let to = paths.len();
            for index in from..to {
                for piece in PIECES {
                    paths.push(format!("{}{piece}", paths[index]));
                }
            }
            from = to;
        }
        paths.push(String::from("/x/..;/..%2F..;%2Fa//..%5C..;/news/1"));
        let mut most = (0, 0);
        for path in &paths {
            let written = readings_within(path, MAX_PAGE_READINGS);
            let parsed = url::Url::parse(&format!("http://h{path}")).unwrap();
            let parsed = readings_within(parsed.path(), MAX_PAGE_READINGS);
            assert!(written.is_ok() && parsed.is_ok(), "{path}");
            most.0 = most.0.max(written.unwrap().len());
            most.1 = most.1.max(parsed.unwrap().len());
        }
        assert_eq!(most, (28, 24));
    }

    /// Stripping exposes dot segments that decoding and merging then move,
    /// so a crafted path, as the client sends it, has more than 64 readings.
    #[test]
    fn a_crafted_path_reaches_the_cap() {
        let path = "/x//..;/.../;/..;/a/%2Fb/;//%2F..%2F..%2F;%5Cc";
        let parsed = url::Url::parse(&format!("http://h{path}")).unwrap();
        assert_eq!(parsed.path(), path);
        assert_eq!(readings_within(path, 1000).map(|r| r.len()), Ok(65));
        assert_eq!(
            matching_target_readings(path),
            Err(TooManyReadings { cap: 64 })
        );
    }

    /// Past the cap there are no readings to compare, only the cap.
    #[test]
    fn a_path_past_the_cap_has_no_readings() {
        assert_eq!(
            readings_within("/x//..%2Fnews/1", 6).map(|r| r.len()),
            Ok(6)
        );
        assert_eq!(
            readings_within("/x//..%2Fnews/1", 5),
            Err(TooManyReadings { cap: 5 })
        );
        assert!(
            TooManyReadings { cap: 64 }
                .to_string()
                .contains("more than 64 readings")
        );
    }
}
