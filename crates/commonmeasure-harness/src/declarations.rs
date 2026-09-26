//! What a source declares about AI use, read for a mediated crossing.
//!
//! Four mechanisms are read: the `Content-Usage` response header and
//! `robots.txt` rule of the IETF AI-preferences attachment draft,
//! Cloudflare's `Content-Signal` line, and an RSL licence discovered from a
//! `robots.txt` `License:` line or a `Link: rel="license"` header. Each
//! produces statements; a statement is a preference, not a licence and not
//! enforcement. Statements are recorded with their source and combined
//! most-restrictive-wins per category, as the vocabulary draft's section 5.1
//! says; an absent statement is unknown, never disallow.
//!
//! Nothing here opens a socket. The caller fetches `robots.txt` and licence
//! documents through the same address-checked path as a page and hands the
//! bytes in, so a probe obeys the same host policy and privacy floor as a
//! crossing.

use std::collections::BTreeMap;

use commonmeasure_types::Money;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The product token publishers address in `robots.txt`. Chosen once and
/// never renamed, because a publisher's rules name it.
pub const PRODUCT_TOKEN: &str = "CommonMeasureBot";

/// The profile URI of the Content Telemetry reporting binding an RSL
/// `<reporting>` element names.
pub const TELEMETRY_PROFILE: &str = "https://contenttelemetry.org/profiles/spur";

/// The categories of use a statement can address, in one vocabulary across
/// the three mechanisms. Each mechanism's own label is mapped on the way in
/// and the mapping is recorded on the statement, so a reader can see which
/// label produced which category.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    /// Training or fine-tuning a model. aipref `train-ai`, Content Signals
    /// and RSL `ai-train`.
    TrainAi,
    /// Content used as input to a model: grounding, retrieval-augmented
    /// generation, summaries. Content Signals and RSL `ai-input`; the aipref
    /// working group's proposed `ai-use` label maps here. A mediated fetch
    /// that puts page text into the model's context is this use.
    AiInput,
    /// Inclusion in an AI system's retrieval index. RSL `ai-index` only.
    AiIndex,
    /// A search index that links back to the source. All three vocabularies
    /// name it `search`.
    Search,
}

impl Category {
    pub const ALL: [Category; 4] = [
        Category::TrainAi,
        Category::AiInput,
        Category::AiIndex,
        Category::Search,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::TrainAi => "train-ai",
            Category::AiInput => "ai-input",
            Category::AiIndex => "ai-index",
            Category::Search => "search",
        }
    }

    /// The aipref vocabulary's labels (draft-ietf-aipref-vocab section 6.1),
    /// plus the `ai-use` label of the working group's open pull request. An
    /// unknown label is ignored, as section 6.4 requires.
    fn from_aipref_label(label: &str) -> Option<Self> {
        match label {
            "train-ai" => Some(Category::TrainAi),
            "search" => Some(Category::Search),
            "ai-use" => Some(Category::AiInput),
            _ => None,
        }
    }

    /// Cloudflare's three signals.
    fn from_content_signal(label: &str) -> Option<Self> {
        match label {
            "ai-train" => Some(Category::TrainAi),
            "ai-input" => Some(Category::AiInput),
            "search" => Some(Category::Search),
            _ => None,
        }
    }

    /// RSL's usage vocabulary (RSL 1.0 section 3.4.1.1). `ai-all` and `all`
    /// are collective tokens and expand to the categories they include.
    fn from_rsl_token(token: &str) -> Vec<Self> {
        match token {
            "ai-train" => vec![Category::TrainAi],
            "ai-input" => vec![Category::AiInput],
            "ai-index" => vec![Category::AiIndex],
            "search" => vec![Category::Search],
            "ai-all" => vec![Category::TrainAi, Category::AiInput, Category::AiIndex],
            "all" => Category::ALL.to_vec(),
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preference {
    Allow,
    Disallow,
}

/// Where a statement was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StatementSource {
    /// The page response's `Content-Usage` header (attachment draft
    /// section 2).
    ContentUsageHeader,
    /// A `Content-Usage` rule in the selected `robots.txt` group (attachment
    /// draft section 3).
    RobotsContentUsage,
    /// A `Content-Signal` line in the selected `robots.txt` group.
    RobotsContentSignal,
    /// An RSL licence's `<permits>` or `<prohibits>` usage terms.
    RslLicence,
    /// An RSL licence in a `<script type="application/rsl+xml">` the user
    /// pasted (`crate::prompt`).
    PastedRsl,
    /// A `Content-Usage:` line in an HTTP response the user pasted.
    PastedContentUsage,
    /// A C2PA training-and-data-mining assertion in a manifest embedded in
    /// pasted text or HTML.
    PastedC2pa,
}

/// One statement of preference as read: the category it addresses, what it
/// says, and where it came from. `detail` names the mechanism's own label or
/// rule, so the mapping onto `category` is checkable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub source: StatementSource,
    pub category: Category,
    pub preference: Preference,
    pub detail: String,
}

/// The combined answer for one category (vocabulary draft section 5.1):
/// disallow if any statement disallows, else allow if any allows, else
/// unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effective {
    Allow,
    Disallow,
    Unknown,
}

/// Most-restrictive-wins per category over every statement gathered.
pub fn combine(statements: &[Statement]) -> BTreeMap<Category, Effective> {
    let mut effective = BTreeMap::new();
    for category in Category::ALL {
        let mut answer = Effective::Unknown;
        for statement in statements.iter().filter(|s| s.category == category) {
            match statement.preference {
                Preference::Disallow => {
                    answer = Effective::Disallow;
                    break;
                }
                Preference::Allow => answer = Effective::Allow,
            }
        }
        effective.insert(category, answer);
    }
    effective
}

// ---------------------------------------------------------------------------
// The structured-field dictionary the aipref drafts serialise preferences in.

/// Parse a `Content-Usage` value (vocabulary draft section 6.5): a
/// structured-field dictionary whose keys are usage labels and whose values
/// are the tokens `y` and `n`. Returns one entry per known label, in file
/// order; a label whose value is not `y` or `n`, an unknown label and a
/// parameter are all ignored, as the draft says. A dictionary that does not
/// parse yields nothing, which is every preference unknown.
pub fn parse_content_usage(value: &str) -> Vec<(Category, Preference, String)> {
    let Some(members) = parse_dictionary(value) else {
        return Vec::new();
    };
    // The last occurrence of a key applies (RFC 9651 section 4.2.2), so the
    // members are folded into a map keyed by label before mapping.
    let mut last: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (key, token) in members {
        last.insert(key, token);
    }
    let mut out = Vec::new();
    for (key, token) in last {
        let Some(category) = Category::from_aipref_label(&key) else {
            continue;
        };
        let preference = match token.as_deref() {
            Some("y") => Preference::Allow,
            Some("n") => Preference::Disallow,
            _ => continue,
        };
        out.push((
            category,
            preference,
            format!("{key}={}", token.unwrap_or_default()),
        ));
    }
    out
}

/// The subset of RFC 9651 section 4.2.2 this needs: members separated by
/// commas, each a key with an optional `=` value and optional `;` parameters.
/// Only a Token value is kept (as `Some`); any other value type is `None`,
/// which the caller reads as unknown. Returns `None` when the text is not a
/// dictionary at all, because a parse failure makes every preference unknown
/// rather than some of them.
fn parse_dictionary(value: &str) -> Option<Vec<(String, Option<String>)>> {
    let mut members = Vec::new();
    let mut rest = value.trim_matches([' ', '\t']);
    if rest.is_empty() {
        return Some(members);
    }
    loop {
        let (key, after_key) = take_key(rest)?;
        rest = after_key;
        let mut token = None;
        if let Some(after_eq) = rest.strip_prefix('=') {
            let (value, after_value) = take_value(after_eq)?;
            token = value;
            rest = after_value;
        }
        // Parameters carry no defined semantics (section 6.5.2) and are
        // skipped, value and all.
        while let Some(after_semicolon) = rest.strip_prefix(';') {
            let (_, after_param_key) = take_key(after_semicolon.trim_start_matches(' '))?;
            rest = after_param_key;
            if let Some(after_eq) = rest.strip_prefix('=') {
                let (_, after_value) = take_value(after_eq)?;
                rest = after_value;
            }
        }
        members.push((key, token));
        let trimmed = rest.trim_start_matches([' ', '\t']);
        if trimmed.is_empty() {
            return Some(members);
        }
        rest = trimmed.strip_prefix(',')?.trim_start_matches([' ', '\t']);
        if rest.is_empty() {
            // A trailing comma is a parse failure (RFC 9651 section 4.2.2).
            return None;
        }
    }
}

/// A dictionary key: a lowercase letter or `*`, then lowercase letters,
/// digits, `_`, `-`, `.` and `*`.
fn take_key(text: &str) -> Option<(String, &str)> {
    let mut end = 0;
    for (index, byte) in text.bytes().enumerate() {
        let ok = if index == 0 {
            byte.is_ascii_lowercase() || byte == b'*'
        } else {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-.*".contains(&byte)
        };
        if !ok {
            break;
        }
        end = index + 1;
    }
    if end == 0 {
        return None;
    }
    Some((text[..end].to_owned(), &text[end..]))
}

/// A bare item. A Token is returned as `Some`; a string, integer, decimal,
/// boolean or byte sequence is consumed and returned as `None`.
fn take_value(text: &str) -> Option<(Option<String>, &str)> {
    let first = text.as_bytes().first().copied()?;
    if first.is_ascii_alphabetic() || first == b'*' {
        let end = text
            .bytes()
            .position(|b| !(is_tchar(b) || b == b':' || b == b'/'))
            .unwrap_or(text.len());
        return Some((Some(text[..end].to_owned()), &text[end..]));
    }
    if first == b'"' {
        let mut escaped = false;
        for (index, byte) in text.bytes().enumerate().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            match byte {
                b'\\' => escaped = true,
                b'"' => return Some((None, &text[index + 1..])),
                _ => {}
            }
        }
        return None;
    }
    if first == b'?' {
        return match text.as_bytes().get(1) {
            Some(b'0' | b'1') => Some((None, &text[2..])),
            _ => None,
        };
    }
    if first == b':' {
        let end = text[1..].find(':')? + 2;
        return Some((None, &text[end..]));
    }
    if first == b'-' || first.is_ascii_digit() {
        let end = text
            .bytes()
            .enumerate()
            .position(|(index, b)| !(b.is_ascii_digit() || b == b'.' || (index == 0 && b == b'-')))
            .unwrap_or(text.len());
        return Some((None, &text[end..]));
    }
    None
}

fn is_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

// ---------------------------------------------------------------------------
// Cloudflare Content Signals.

/// Parse a `Content-Signal` value: `search=yes, ai-input=no, ai-train=no`.
/// Three signals only; a value other than `yes` or `no` and an unknown label
/// are ignored.
pub fn parse_content_signal(value: &str) -> Vec<(Category, Preference, String)> {
    value
        .split(',')
        .filter_map(|member| {
            let (key, answer) = member.trim().split_once('=')?;
            let category = Category::from_content_signal(key.trim())?;
            let preference = match answer.trim().to_ascii_lowercase().as_str() {
                "yes" => Preference::Allow,
                "no" => Preference::Disallow,
                _ => return None,
            };
            Some((
                category,
                preference,
                format!("{}={}", key.trim(), answer.trim()),
            ))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// robots.txt

/// A parsed `robots.txt` (RFC 9309 groups) with the extensions this reader
/// knows: `Content-Usage` rules, `Content-Signal` lines, `License:`
/// directives and `Crawl-delay`. Unknown lines are ignored, as the protocol
/// requires.
///
/// Two scopes are read. Access rules and `Content-Usage` rules belong to the
/// RFC 9309 group: every rule after a `User-agent` line until the next one,
/// blank lines included. `License:` and `Content-Signal:` lines belong to a
/// group only while they follow its lines without a break; a blank line or
/// a `Sitemap:` line ends the group's block, and a directive after that
/// break, or before any group, is site-wide. That is where publishers put a
/// licence meant for the whole site, and RSL reads a `License:` outside a
/// group as applying to every agent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RobotsFile {
    groups: Vec<Group>,
    /// Site-wide `License:` directives (RSL section 4.4.2).
    global_licences: Vec<String>,
    /// Site-wide `Content-Signal:` lines, read where the selected group
    /// carries none of its own.
    global_content_signal: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Group {
    agents: Vec<String>,
    rules: Vec<AccessRule>,
    /// `(path, preference text)`; `None` path means the whole site.
    content_usage: Vec<(Option<String>, String)>,
    content_signal: Vec<String>,
    licences: Vec<String>,
    /// `Crawl-delay` values as written, in file order.
    crawl_delay: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AccessRule {
    allow: bool,
    pattern: String,
}

/// Parse the file. Access and `Content-Usage` rules before the first
/// `User-agent` line have no group and are dropped; a `License:` or
/// `Content-Signal:` line there is site-wide.
pub fn parse_robots(text: &str) -> RobotsFile {
    let mut file = RobotsFile::default();
    let mut current: Option<Group> = None;
    // Consecutive User-agent lines share one group; a rule line closes the
    // agent list, so a later User-agent line starts a new group.
    let mut agents_open = false;
    // Whether the current group's block is unbroken: a blank line or a
    // Sitemap line ends it, and a rule line of the group resumes it. A
    // License or Content-Signal line outside the block is site-wide.
    let mut in_block = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            in_block = false;
            continue;
        }
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        let label = label.trim().to_ascii_lowercase();
        let value = value.trim();
        match label.as_str() {
            "user-agent" => {
                if !agents_open {
                    if let Some(group) = current.take() {
                        file.groups.push(group);
                    }
                    current = Some(Group::default());
                    agents_open = true;
                }
                in_block = true;
                if let Some(group) = current.as_mut() {
                    group.agents.push(value.to_ascii_lowercase());
                }
            }
            "allow" | "disallow" => {
                agents_open = false;
                if let Some(group) = current.as_mut() {
                    in_block = true;
                    group.rules.push(AccessRule {
                        allow: label == "allow",
                        pattern: value.to_owned(),
                    });
                }
            }
            "content-usage" => {
                agents_open = false;
                if let Some(group) = current.as_mut() {
                    in_block = true;
                    let (path, preference) = if value.starts_with('/') {
                        match value.find([' ', '\t']) {
                            Some(split) => (
                                Some(value[..split].to_owned()),
                                value[split..].trim().to_owned(),
                            ),
                            None => (Some(value.to_owned()), String::new()),
                        }
                    } else {
                        (None, value.to_owned())
                    };
                    group.content_usage.push((path, preference));
                }
            }
            "crawl-delay" => {
                // Not in RFC 9309, which says crawlers may interpret other
                // records; read as a group record, as the engines that honour
                // it do, so a line before any group is dropped.
                agents_open = false;
                if let Some(group) = current.as_mut() {
                    in_block = true;
                    group.crawl_delay.push(value.to_owned());
                }
            }
            "content-signal" => {
                agents_open = false;
                match current.as_mut().filter(|_| in_block) {
                    Some(group) => group.content_signal.push(value.to_owned()),
                    None => file.global_content_signal.push(value.to_owned()),
                }
            }
            "license" => {
                agents_open = false;
                match current.as_mut().filter(|_| in_block) {
                    Some(group) => group.licences.push(value.to_owned()),
                    None => file.global_licences.push(value.to_owned()),
                }
            }
            "sitemap" => {
                // A site-wide record (RFC 9309 section 2.2.4), so the group's
                // block ends here; the RSL convention places `License:`
                // beside it.
                agents_open = false;
                in_block = false;
            }
            _ => agents_open = false,
        }
    }
    if let Some(group) = current.take() {
        file.groups.push(group);
    }
    file
}

/// What the selected group of a `robots.txt` says about one path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobotsReading {
    /// The product token whose group was selected: `CommonMeasureBot` where a
    /// group names it, `*` otherwise, absent when the file has neither.
    pub group: Option<String>,
    /// True when the `*` group was selected because no group names the
    /// product token: its rules address every fetcher, not this one by name.
    #[serde(default)]
    pub group_is_wildcard: bool,
    /// Whether the selected group's Allow and Disallow rules permit fetching
    /// this path. Absent when no group applies, which permits everything.
    pub crawlable: Option<bool>,
    /// The Allow or Disallow rule that decided `crawlable`, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_rule: Option<String>,
    /// Whether the deciding rule's pattern uses `*` (RFC 9309 section
    /// 2.2.3), so it matched the path by wildcard rather than as a literal
    /// prefix. Absent when no rule decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_rule_wildcard: Option<bool>,
    /// Statements from the group's `Content-Usage` rules and its
    /// `Content-Signal` lines, or the site-wide `Content-Signal` lines where
    /// the group has none, that apply to this path. Empty when the path is
    /// not crawlable: the attachment draft says preferences are implied for
    /// no resource that cannot be crawled.
    pub statements: Vec<Statement>,
    /// Candidate RSL licence URLs: the group's own `License:` directives, or
    /// the site-wide ones when the group has none (RSL section 4.4.2).
    pub licences: Vec<String>,
    /// The selected group's `Crawl-delay`. Absent when the group has no
    /// such line; a `*` group's delay is not read when a group names the
    /// product token, as for its access rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crawl_delay: Option<CrawlDelay>,
    /// Set where the page's path has more readings than
    /// [`commonmeasure_types::MAX_PAGE_READINGS`]: no rule was compared,
    /// `crawlable` is false and `access_rule` names the cap.
    #[serde(skip)]
    pub too_many_readings: Option<commonmeasure_types::TooManyReadings>,
}

/// The most `Crawl-delay` this edge keeps between two requests to one host.
/// A longer delay is kept at this bound and recorded as capped: an hour
/// between requests would leave a host unreadable to an agent session, and
/// the bound is stated so a publisher can see what is honoured.
pub const MAX_CRAWL_DELAY: std::time::Duration = std::time::Duration::from_secs(60);

/// A group's `Crawl-delay`, as read and as kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrawlDelay {
    /// The value the delay was read from, as written. Where the group has
    /// several readable values, the longest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The delay read, in milliseconds. Absent when no value is readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
    /// The delay this edge keeps between requests to the host: `delay_ms`,
    /// or [`MAX_CRAWL_DELAY`] where it is longer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub honoured_ms: Option<u64>,
    /// True when `honoured_ms` is the bound rather than the delay read.
    #[serde(default)]
    pub capped: bool,
    /// Values in the group that are not a non-negative decimal number of
    /// seconds, as written. They are ignored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
}

impl CrawlDelay {
    /// Read a group's `Crawl-delay` values: seconds as a non-negative
    /// decimal (`2`, `0.5`); anything else is unreadable. Several readable
    /// values keep the longest, the most restrictive reading.
    fn read(values: &[String]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let mut delay = CrawlDelay {
            value: None,
            delay_ms: None,
            honoured_ms: None,
            capped: false,
            unreadable: Vec::new(),
        };
        for value in values {
            match seconds_in_ms(value) {
                Some(ms) if delay.delay_ms.is_none_or(|kept| ms > kept) => {
                    delay.value = Some(value.clone());
                    delay.delay_ms = Some(ms);
                }
                Some(_) => {}
                None => delay.unreadable.push(value.clone()),
            }
        }
        if let Some(ms) = delay.delay_ms {
            let bound = u64::try_from(MAX_CRAWL_DELAY.as_millis()).unwrap_or(u64::MAX);
            delay.capped = ms > bound;
            delay.honoured_ms = Some(ms.min(bound));
        }
        Some(delay)
    }
}

/// A non-negative decimal number of seconds in milliseconds, rounded; `None`
/// for a sign, an exponent, `inf`, an empty value or anything else a float
/// parser would take that a publisher would not write.
fn seconds_in_ms(value: &str) -> Option<u64> {
    let digits = value.chars().filter(char::is_ascii_digit).count();
    let dots = value.chars().filter(|c| *c == '.').count();
    if digits == 0 || dots > 1 || digits + dots != value.len() {
        return None;
    }
    let seconds: f64 = value.parse().ok()?;
    // Saturates at u64::MAX for an absurd value, which the cap then bounds.
    Some((seconds * 1000.0).round() as u64)
}

impl RobotsFile {
    /// Select the groups for `token` (the `*` groups as fallback) and read
    /// what they say about `path`. `path` is the request target: path and
    /// query.
    ///
    /// RFC 9309 section 2.2.1 requires the records of *every* group naming
    /// the product token to be merged into one, and the `*` group to be
    /// considered only where no group names it. A site that writes two
    /// `User-agent: CommonMeasureBot` groups therefore has both groups'
    /// rules applied; taking the first alone would fetch what the second
    /// refused. Within the merged records the RFC's own precedence decides:
    /// the longest matching pattern wins and `Allow` wins an equal-length
    /// tie (section 2.2.2). The merged `Crawl-delay` values keep the longest,
    /// as several values in one group do.
    ///
    /// A path with more readings than
    /// [`commonmeasure_types::MAX_PAGE_READINGS`] is refused, with or
    /// without a group, as if a `Disallow` covered every reading.
    pub fn read(&self, token: &str, path: &str) -> RobotsReading {
        self.read_readings(token, commonmeasure_types::matching_target_readings(path))
    }

    /// [`RobotsFile::read`] over the page's readings, or over the cap they
    /// went past.
    fn read_readings(
        &self,
        token: &str,
        readings: Result<Vec<String>, commonmeasure_types::TooManyReadings>,
    ) -> RobotsReading {
        let readings = match readings {
            Ok(readings) => readings,
            Err(too_many) => {
                return RobotsReading {
                    group: None,
                    group_is_wildcard: false,
                    crawlable: Some(false),
                    access_rule: Some(too_many.to_string()),
                    access_rule_wildcard: None,
                    statements: Vec::new(),
                    licences: self.global_licences.clone(),
                    crawl_delay: None,
                    too_many_readings: Some(too_many),
                };
            }
        };
        let wanted = token.to_ascii_lowercase();
        let named: Vec<&Group> = self
            .groups
            .iter()
            .filter(|group| group.agents.contains(&wanted))
            .collect();
        let group_is_wildcard = named.is_empty();
        let (groups, name) = if group_is_wildcard {
            let fallback: Vec<&Group> = self
                .groups
                .iter()
                .filter(|group| group.agents.iter().any(|a| a == "*"))
                .collect();
            if fallback.is_empty() {
                return RobotsReading {
                    group: None,
                    group_is_wildcard: false,
                    crawlable: None,
                    access_rule: None,
                    access_rule_wildcard: None,
                    statements: signal_statements(&self.global_content_signal, None),
                    licences: self.global_licences.clone(),
                    crawl_delay: None,
                    too_many_readings: None,
                };
            }
            (fallback, "*".to_owned())
        } else {
            (named, token.to_owned())
        };
        let rules: Vec<&AccessRule> = groups.iter().flat_map(|group| group.rules.iter()).collect();
        let content_usage: Vec<&(Option<String>, String)> = groups
            .iter()
            .flat_map(|group| group.content_usage.iter())
            .collect();
        let content_signal: Vec<String> = groups
            .iter()
            .flat_map(|group| group.content_signal.iter().cloned())
            .collect();
        let group_licences: Vec<String> = groups
            .iter()
            .flat_map(|group| group.licences.iter().cloned())
            .collect();
        let crawl_delay_values: Vec<String> = groups
            .iter()
            .flat_map(|group| group.crawl_delay.iter().cloned())
            .collect();
        // Each reading of the page is decided on its own, and a Disallow any
        // reading reaches refuses the page: a server may serve `//news/1` or
        // `/news%2F1` as `/news/1`, so an Allow reached through one reading
        // cannot lift a Disallow reached through another.
        let decisions: Vec<&AccessRule> = readings
            .iter()
            .filter_map(|reading| {
                longest_match(
                    rules.iter().map(|rule| (rule.pattern.as_str(), *rule)),
                    reading,
                )
                .max_by(|(length_a, rule_a), (length_b, rule_b)| {
                    // Among equally long matches Allow wins (RFC 9309
                    // section 2.2.2).
                    length_a
                        .cmp(length_b)
                        .then_with(|| rule_a.allow.cmp(&rule_b.allow))
                })
                .map(|(_, rule)| rule)
            })
            .collect();
        let decided = decisions
            .iter()
            .find(|rule| !rule.allow)
            .or_else(|| decisions.first())
            .copied();
        let crawlable = decided.is_none_or(|rule| rule.allow);
        let access_rule = decided.map(|rule| {
            format!(
                "{}: {}",
                if rule.allow { "Allow" } else { "Disallow" },
                rule.pattern
            )
        });
        let access_rule_wildcard = decided.map(|rule| rule.pattern.contains('*'));
        let mut statements = Vec::new();
        if crawlable {
            // The Content-Usage rule with the longest matching path applies;
            // identical paths with conflicting preferences apply separately
            // and combine most-restrictive-wins (attachment draft
            // section 3.1). The rules each reading of the page selects all
            // apply, combined the same way.
            let mut selected: Vec<&(Option<String>, String)> = Vec::new();
            for reading in &readings {
                let candidates: Vec<(usize, &(Option<String>, String))> = content_usage
                    .iter()
                    .filter_map(|rule| {
                        let pattern = rule.0.as_deref().unwrap_or("");
                        if pattern.is_empty() {
                            Some((0, *rule))
                        } else {
                            let form = commonmeasure_types::matching_pattern(pattern);
                            matches_form(&form, reading).then_some((form.len(), *rule))
                        }
                    })
                    .collect();
                let longest = candidates.iter().map(|(length, _)| *length).max();
                for (length, rule) in candidates {
                    if Some(length) == longest
                        && !selected.iter().any(|kept| std::ptr::eq(*kept, rule))
                    {
                        selected.push(rule);
                    }
                }
            }
            for (rule_path, preference) in selected {
                for (category, preference, label) in parse_content_usage(preference) {
                    statements.push(Statement {
                        source: StatementSource::RobotsContentUsage,
                        category,
                        preference,
                        detail: match rule_path {
                            Some(rule_path) => {
                                format!("group {name}: Content-Usage: {rule_path} {label}")
                            }
                            None => format!("group {name}: Content-Usage: {label}"),
                        },
                    });
                }
            }
            let (signals, scope) = if content_signal.is_empty() {
                (&self.global_content_signal, None)
            } else {
                (&content_signal, Some(name.as_str()))
            };
            statements.extend(signal_statements(signals, scope));
        }
        let licences = if group_licences.is_empty() {
            self.global_licences.clone()
        } else {
            group_licences
        };
        RobotsReading {
            group_is_wildcard,
            group: Some(name),
            crawlable: Some(crawlable),
            access_rule,
            access_rule_wildcard,
            statements,
            licences,
            crawl_delay: CrawlDelay::read(&crawl_delay_values),
            too_many_readings: None,
        }
    }
}

/// Statements from `Content-Signal` lines; `group` names the group they were
/// read from, or is absent for site-wide lines.
fn signal_statements(signals: &[String], group: Option<&str>) -> Vec<Statement> {
    signals
        .iter()
        .flat_map(|signal| parse_content_signal(signal))
        .map(|(category, preference, label)| Statement {
            source: StatementSource::RobotsContentSignal,
            category,
            preference,
            detail: match group {
                Some(name) => format!("group {name}: Content-Signal: {label}"),
                None => format!("site-wide Content-Signal: {label}"),
            },
        })
        .collect()
}

/// The rules whose pattern matches one reading of the page (already in
/// [`commonmeasure_types::matching_target`]'s form), each with the length it
/// ranks by.
fn longest_match<'a, T: 'a>(
    rules: impl Iterator<Item = (&'a str, T)>,
    reading: &'a str,
) -> impl Iterator<Item = (usize, T)> {
    rules.filter_map(move |(pattern, rule)| {
        if pattern.is_empty() {
            // An empty Allow or Disallow matches nothing (RFC 9309
            // section 2.2.2).
            return None;
        }
        let form = commonmeasure_types::matching_pattern(pattern);
        matches_form(&form, reading).then_some((form.len(), rule))
    })
}

/// RFC 9309 section 2.2.3 matching of one target: a prefix match with `*`
/// matching any sequence and a final `$` anchoring the end. Both sides are
/// compared byte-wise in one spelling
/// ([`commonmeasure_types::matching_pattern`] and
/// [`commonmeasure_types::matching_target`]): a percent-encoded unreserved
/// octet is decoded, the hex of every other encoding is upper-cased, a
/// reserved octet stays encoded, and a literal `*` or `$` in the target is
/// `%2A` or `%24`. So `/%6Eews/1` is under `/news/`, `/news/1` is under
/// `/%6Eews/`, `/file-*.html` is under `/file-%2A.html`, and `/a%2Fb` is not
/// under `/a/b`. Slashes are compared as written. A page is compared reading
/// by reading ([`commonmeasure_types::matching_target_readings`]), which is
/// where `//news/1`, `/news%2F1` and `/news;x/1` meet `/news/`; this function compares
/// `target` as given. Rules rank by the length of the pattern's form.
pub fn matches_pattern(pattern: &str, target: &str) -> bool {
    matches_form(
        &commonmeasure_types::matching_pattern(pattern),
        &commonmeasure_types::matching_target(target),
    )
}

/// [`matches_pattern`] over a pattern and a target already in the matching
/// form. The parts before the last match leftmost; an anchored pattern's
/// last part must end the target, wherever else it occurs, so `/*.pdf$`
/// covers `/a.pdf/b.pdf`.
fn matches_form(pattern: &str, target: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(stripped) => (stripped, true),
        None => (pattern, false),
    };
    let mut parts: Vec<&str> = pattern.split('*').collect();
    let last = if anchored { parts.pop() } else { None };
    let Some((first, middle)) = parts.split_first() else {
        // An anchored pattern with no `*`: the target must be it exactly.
        return last.is_some_and(|last| target == last);
    };
    let Some(mut rest) = target.strip_prefix(first) else {
        return false;
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    match last {
        Some(last) => rest.ends_with(last),
        None => true,
    }
}

// ---------------------------------------------------------------------------
// RSL 1.0

/// An RSL document as parsed: the `<content>` entries and their licences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RslDocument {
    pub contents: Vec<RslContent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RslContent {
    /// The `url` attribute: a path pattern under the origin, an absolute URL,
    /// or empty for the scope the discovery mechanism established.
    pub url: String,
    /// An RSL licence server the client must obtain a licence from before
    /// access, whatever the payment type (RSL section 3.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    pub licences: Vec<RslLicence>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RslLicence {
    /// `<permits type="usage">` tokens, or absent. When present, only the
    /// listed uses are allowed under this licence (RSL section 3.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permits_usage: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prohibits_usage: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment: Option<RslPayment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reporting: Vec<RslReporting>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RslPayment {
    /// `purchase`, `subscription`, `training`, `crawl`, `use`,
    /// `contribution`, `attribution` or `free`; absent means free
    /// (RSL section 3.7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `<amount currency="…">decimal</amount>`, kept as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<RslAmount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RslAmount {
    pub currency: String,
    pub decimal: String,
}

impl RslPayment {
    /// Whether this payment type asks for money. `attribution` and `free`
    /// ask for none; an absent type is free by the specification.
    pub fn is_monetary(&self) -> bool {
        matches!(
            self.kind.as_deref(),
            Some("purchase" | "subscription" | "training" | "crawl" | "use" | "contribution")
        )
    }

    /// The quoted price, where the amount is a decimal this runtime can
    /// represent exactly. An amount it cannot stays unknown.
    pub fn price(&self) -> Option<Money> {
        let amount = self.amount.as_ref()?;
        Money::from_decimal_str(amount.currency.as_str(), amount.decimal.trim())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RslReporting {
    /// `telemetry`, `provenance` or `audit`.
    pub kind: String,
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// The element body parsed as JSON, where it carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
}

/// Parse an RSL document. Elements outside the vocabulary this reader knows
/// are skipped, as RSL says of extension tokens; a document with no
/// `<content>` is an error, because it can govern nothing.
pub fn parse_rsl(xml: &str) -> Result<RslDocument, String> {
    use quick_xml::Reader;
    use quick_xml::events::{BytesStart, Event};

    fn attribute(start: &BytesStart<'_>, name: &str) -> Option<String> {
        start
            .attributes()
            .filter_map(Result::ok)
            .find(|attribute| attribute.key.local_name().as_ref() == name.as_bytes())
            .and_then(|attribute| {
                attribute
                    .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .ok()
                    .map(|value| value.into_owned())
            })
    }

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    // An empty element is read as a start and an end, so `<payment type="free"/>`
    // takes the same path as the open form.
    reader.config_mut().expand_empty_elements = true;
    let mut contents = Vec::new();
    let mut content: Option<RslContent> = None;
    let mut licence: Option<RslLicence> = None;
    let mut payment: Option<RslPayment> = None;
    let mut reporting: Option<RslReporting> = None;
    let mut reporting_body = String::new();
    // The element whose text is being collected, and the attribute that
    // qualifies it (the `type` of permits/prohibits, the currency of amount).
    let mut collecting: Option<(String, Option<String>)> = None;
    let mut text = String::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|error| format!("not well-formed XML: {error}"))?;
        match event {
            Event::Start(start) => {
                let name = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                match name.as_str() {
                    "content" => {
                        content = Some(RslContent {
                            url: attribute(&start, "url").unwrap_or_default(),
                            server: attribute(&start, "server"),
                            licences: Vec::new(),
                        });
                    }
                    "license" if content.is_some() => {
                        licence = Some(RslLicence::default());
                    }
                    "permits" | "prohibits" if licence.is_some() => {
                        text.clear();
                        collecting = Some((name.clone(), attribute(&start, "type")));
                    }
                    "payment" if licence.is_some() => {
                        payment = Some(RslPayment {
                            kind: attribute(&start, "type"),
                            ..RslPayment::default()
                        });
                    }
                    "amount" | "standard" | "custom" if payment.is_some() => {
                        text.clear();
                        collecting = Some((name.clone(), attribute(&start, "currency")));
                    }
                    "reporting" if licence.is_some() => {
                        reporting = Some(RslReporting {
                            kind: attribute(&start, "type").unwrap_or_default(),
                            profile: attribute(&start, "profile").unwrap_or_default(),
                            endpoint: attribute(&start, "endpoint"),
                            config: None,
                        });
                        reporting_body.clear();
                    }
                    _ => {}
                }
            }
            Event::Text(bytes) => {
                let value = bytes
                    .decode()
                    .map_err(|error| format!("text is not decodable: {error}"))?;
                if collecting.is_some() {
                    text.push_str(&value);
                } else if reporting.is_some() {
                    reporting_body.push_str(&value);
                }
            }
            Event::CData(bytes) => {
                let value = String::from_utf8_lossy(&bytes).into_owned();
                if reporting.is_some() {
                    reporting_body.push_str(&value);
                } else if collecting.is_some() {
                    text.push_str(&value);
                }
            }
            Event::End(end) => {
                let name = String::from_utf8_lossy(end.local_name().as_ref()).into_owned();
                match name.as_str() {
                    "content" => {
                        if let Some(done) = content.take() {
                            contents.push(done);
                        }
                    }
                    "license" => {
                        if let (Some(done), Some(content)) = (licence.take(), content.as_mut()) {
                            content.licences.push(done);
                        }
                    }
                    "permits" | "prohibits" => {
                        if let Some((_, kind)) = collecting.take() {
                            finish_usage(&mut licence, &name, kind, &text);
                        }
                    }
                    "amount" => {
                        if let (Some((_, currency)), Some(payment)) =
                            (collecting.take(), payment.as_mut())
                        {
                            payment.amount = Some(RslAmount {
                                currency: currency.unwrap_or_default(),
                                decimal: text.trim().to_owned(),
                            });
                        }
                    }
                    "standard" | "custom" => {
                        if let (Some(_), Some(payment)) = (collecting.take(), payment.as_mut()) {
                            let value = Some(text.trim().to_owned());
                            if name == "standard" {
                                payment.standard = value;
                            } else {
                                payment.custom = value;
                            }
                        }
                    }
                    "payment" => {
                        if let (Some(done), Some(licence)) = (payment.take(), licence.as_mut()) {
                            licence.payment = Some(done);
                        }
                    }
                    "reporting" => {
                        if let (Some(mut done), Some(licence)) =
                            (reporting.take(), licence.as_mut())
                        {
                            let body = reporting_body.trim();
                            if !body.is_empty() {
                                done.config = serde_json::from_str(body).ok();
                            }
                            licence.reporting.push(done);
                        }
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if contents.is_empty() {
        return Err("the RSL document declares no <content> element".to_owned());
    }
    Ok(RslDocument { contents })
}

fn finish_usage(licence: &mut Option<RslLicence>, element: &str, kind: Option<String>, text: &str) {
    // Only the usage vocabulary is read here; user and geo terms are
    // recorded by nobody yet and must not be mistaken for usage tokens.
    if kind.as_deref() != Some("usage") {
        return;
    }
    let tokens: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
    if let Some(licence) = licence.as_mut() {
        if element == "permits" {
            licence.permits_usage = Some(tokens);
        } else {
            licence.prohibits_usage = Some(tokens);
        }
    }
}

impl RslDocument {
    /// The `<content>` entry governing one page: the most specific matching
    /// `url` (RSL sections 3.1.1 and 4.9). A path pattern is matched against
    /// the page's path under RFC 9309 rules ([`matches_pattern`]); an
    /// absolute URL is matched as a prefix of the page URL, both in
    /// [`commonmeasure_types::matching_url`]'s form; an empty `url` names the
    /// scope the discovery mechanism established and matches with the least
    /// specificity. Host case, trailing dots, default ports, credentials,
    /// percent-encoded unreserved characters, lower-case hex, a literal `*`
    /// or `$` and fragments do not take a page out of its entry.
    ///
    /// The page is read as parsed and as every path decoding `%2F` and
    /// `%5C` as `/` and `%3B` as `;`, merging slashes, resolving dot
    /// segments and stripping `;` parameters reach from it, in any order
    /// ([`commonmeasure_types::matching_target_readings`]); an entry's `url`
    /// is read as written. The entry the page as parsed selects governs;
    /// when it selects none, the entry the first later reading selects, in
    /// that function's breadth-first order. So a page an entry governs as
    /// parsed keeps that entry, and `//news/1`, `/news%2F1`,
    /// `/x//..%2Fnews/1`, `/news;x/1` or `/news%3Bx/1` falls to `/news/`
    /// only when no entry covers it as parsed. The requested spelling can
    /// therefore select a less restrictive entry: a catch-all beside a
    /// narrower entry, or a permissive sub-scope a reading climbs out of. A
    /// page with more readings than the cap is ruled on as parsed only here;
    /// the robots ruling refuses it before any licence is read.
    ///
    /// Specificity is the length of the path the entry constrains, as RFC
    /// 9309 ranks rules: a path pattern's length and an absolute scope's path
    /// and query, each in the matching form, since the host has already
    /// matched. Ranking
    /// by the written length would let a broad scope spelled long, or any
    /// absolute scope, outrank a narrower entry and its prohibition. An
    /// absolute scope that does not parse ranks 0, with the empty `url`, as
    /// the least specific: it matches the page as written, so `http://` would
    /// otherwise outrank a valid narrower scope.
    ///
    /// At equal specificity an absolute scope governs over a relative entry.
    /// Two absolute scopes of equal specificity name the same scope in
    /// different spellings; the one matching the page as written governs,
    /// then the one written longer. That is the order scopes had when they
    /// were matched only as written and ranked by written length, so wherever
    /// that matching reached either scope, the order of the two in the
    /// document cannot change which governs. The last entry in the document
    /// governs only between two absolute scopes of equal written length that
    /// both match the page as written, which makes them the same string, or
    /// that neither does; and between two relative entries, or two empty
    /// `url`s, of equal length in the matching form, whatever their written
    /// lengths.
    pub fn content_for(&self, page_url: &str) -> Option<&RslContent> {
        let target = request_target(page_url);
        let targets = commonmeasure_types::matching_target_readings(&target)
            .unwrap_or_else(|_| vec![commonmeasure_types::matching_target(&target)]);
        // Both lists read the same target, so they align reading by reading.
        let pages = url::Url::parse(page_url).ok().map(|page| {
            commonmeasure_types::matching_url_readings(&page)
                .unwrap_or_else(|_| vec![commonmeasure_types::matching_url(&page)])
        });
        targets.iter().enumerate().find_map(|(index, target)| {
            let page = pages
                .as_ref()
                .and_then(|pages| pages.get(index))
                .map(String::as_str);
            self.content_for_reading(target, page, page_url)
        })
    }

    /// The entry one reading of the page selects: `target` and `page` are
    /// its request target and URL in the matching form.
    fn content_for_reading(
        &self,
        target: &str,
        page: Option<&str>,
        page_url: &str,
    ) -> Option<&RslContent> {
        self.contents
            .iter()
            .filter_map(|content| {
                let pattern = content.url.as_str();
                let absolute = !pattern.is_empty() && !pattern.starts_with('/');
                let matched = if pattern.is_empty() {
                    Some(0)
                } else if pattern.starts_with('/') {
                    let form = commonmeasure_types::matching_pattern(pattern);
                    matches_form(&form, target).then_some(form.len())
                } else {
                    within_absolute_scope(pattern, page, page_url)
                };
                // Written-form tie-breaks for absolute scopes. For a relative
                // entry both are constant, so two relative entries that are
                // one path in two spellings tie and the later one governs.
                let as_written = absolute && page_url.starts_with(pattern);
                let written = if absolute { pattern.len() } else { 0 };
                matched.map(|length| ((length, absolute, as_written, written), content))
            })
            .max_by_key(|(rank, _)| *rank)
            .map(|(_, content)| content)
    }
}

/// The specificity of an absolute `<content>` scope the page is under, or
/// `None` when it is not. `page` is one reading of the page in the matching
/// form. A trailing dot, a change of host case, an explicit default port,
/// credentials, a percent-encoded unreserved character or a fragment in
/// either URL names the same resource, so none of them may take a page out
/// of its licence and the licence's prohibitions and reporting demands out
/// of the ruling. The scope's own slashes are compared as written. A parsed
/// scope ranks by its request target in the matching form, the path and
/// query it constrains; a scope that does not parse is compared as written
/// and ranks 0, since its written length says nothing about the path it
/// constrains.
fn within_absolute_scope(pattern: &str, page: Option<&str>, page_url: &str) -> Option<usize> {
    match url::Url::parse(pattern) {
        Ok(scope) => {
            let scope = commonmeasure_types::matching_url(&scope);
            let within = match page {
                Some(page) => page.starts_with(&scope),
                None => page_url.starts_with(pattern),
            };
            within.then(|| commonmeasure_types::matching_target(&request_target(&scope)).len())
        }
        Err(_) => page_url.starts_with(pattern).then_some(0),
    }
}

/// The terms an RSL content entry states for AI input, evaluated across its
/// licences: each licence is one offer, so a category any licence permits is
/// allowed, one every applicable licence refuses is disallowed, and one no
/// licence mentions is unknown.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LicenceTerms {
    pub statements: Vec<Statement>,
    /// The payment terms of the licence under which AI input is permitted,
    /// where one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment: Option<RslPayment>,
    /// The reporting demands that bind an AI-input crossing
    /// ([`reporting_demands`]), whatever the licences say of AI input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reporting: Vec<RslReporting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

pub fn licence_terms(content: &RslContent, licence_url: &str) -> LicenceTerms {
    let mut terms = LicenceTerms {
        server: content.server.clone(),
        ..LicenceTerms::default()
    };
    for category in Category::ALL {
        let mut allowed_by: Option<&RslLicence> = None;
        let mut disallowed_by: Vec<usize> = Vec::new();
        for (index, licence) in content.licences.iter().enumerate() {
            // RSL 3.1.1: a more specific declaration takes precedence over a
            // less specific one, where one term both permits and prohibits
            // the same usage the prohibition wins, and the document is read
            // conservatively. So `permits ai-input` stands over `prohibits
            // all`, which restates the default that unlisted uses are not
            // licensed, while a prohibition naming an AI use — `ai-input`
            // itself or `ai-all` — beside a permit that covers it prohibits.
            let permitted = licence
                .permits_usage
                .as_deref()
                .and_then(|tokens| token_specificity(tokens, category));
            let prohibited = licence
                .prohibits_usage
                .as_deref()
                .and_then(|tokens| token_specificity(tokens, category));
            match (permitted, prohibited) {
                (Some(permit), Some(prohibit)) if prohibit >= permit => disallowed_by.push(index),
                (Some(_), _) => {
                    allowed_by.get_or_insert(licence);
                }
                (None, Some(_)) => disallowed_by.push(index),
                // Permits are listed and this category is not among them.
                (None, None) if licence.permits_usage.is_some() => disallowed_by.push(index),
                (None, None) => {}
            }
        }
        if let Some(licence) = allowed_by {
            terms.statements.push(Statement {
                source: StatementSource::RslLicence,
                category,
                preference: Preference::Allow,
                detail: format!("{licence_url}: permits usage {}", category.label()),
            });
            if category == Category::AiInput {
                terms.payment = licence.payment.clone();
            }
        } else if !disallowed_by.is_empty() {
            terms.statements.push(Statement {
                source: StatementSource::RslLicence,
                category,
                preference: Preference::Disallow,
                detail: format!(
                    "{licence_url}: no licence permits usage {} (licence {})",
                    category.label(),
                    disallowed_by
                        .iter()
                        .map(|index| (index + 1).to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    }
    terms.reporting = reporting_demands(content);
    terms
}

/// The reporting demands that bind an AI-input crossing of the page this
/// content entry governs (RSL section 3.12: a client satisfies every
/// applicable demand, or treats the activity as not licensed).
///
/// A demand applies to activity the enclosing licence authorises. A licence
/// authorises AI input where its `<permits type="usage">` covers it, or where
/// it has no usage `<permits>` at all, since section 3.5 restricts usage only
/// where a `<permits>` of that type exists; in both cases no usage
/// `<prohibits>` may cover it. The demands of the licence the crossing is
/// taken under apply: one that permits AI input by name before one silent
/// on usage, matching the licence whose payment term is read. Where no
/// licence authorises AI input and the crossing goes on anyway, outside
/// `strict`, every demand in the entry applies: the page is still the
/// owner's, and taking it outside the licence does not excuse the fetcher
/// from the report the owner asks for (owner decision, 22 September 2026).
fn reporting_demands(content: &RslContent) -> Vec<RslReporting> {
    let category = Category::AiInput;
    let authorises = |licence: &&RslLicence, by_name: bool| {
        let prohibited = licence
            .prohibits_usage
            .as_deref()
            .and_then(|tokens| token_specificity(tokens, category));
        match licence.permits_usage.as_deref() {
            Some(tokens) if by_name => token_specificity(tokens, category)
                .is_some_and(|permit| prohibited.is_none_or(|prohibit| prohibit < permit)),
            None if !by_name => prohibited.is_none(),
            _ => false,
        }
    };
    let governing = content
        .licences
        .iter()
        .find(|licence| authorises(licence, true))
        .or_else(|| {
            content
                .licences
                .iter()
                .find(|licence| authorises(licence, false))
        });
    match governing {
        Some(licence) => licence.reporting.clone(),
        None => content
            .licences
            .iter()
            .flat_map(|licence| licence.reporting.iter().cloned())
            .collect(),
    }
}

/// How specifically a usage token list names `category`: `all`, which
/// restates the default that unlisted uses are not licensed, ranks below
/// any token that names an AI use or the category itself. `None` where no
/// token covers it.
fn token_specificity(tokens: &[String], category: Category) -> Option<u8> {
    tokens
        .iter()
        .filter(|token| Category::from_rsl_token(token).contains(&category))
        .map(|token| if token == "all" { 1 } else { 2 })
        .max()
}

/// The path and query of a URL, the target `robots.txt` rules are matched
/// against. `/` when the URL has no path.
pub fn request_target(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => {
            let mut target = parsed.path().to_owned();
            if let Some(query) = parsed.query() {
                target.push('?');
                target.push_str(query);
            }
            target
        }
        Err(_) => "/".to_owned(),
    }
}

/// The `Link` header's `rel="license"` target of type `application/rsl+xml`,
/// resolved against the page URL (RSL section 4.5). The first such link.
pub fn rsl_link(link_header: &str, page_url: &str) -> Option<String> {
    for member in split_link_members(link_header) {
        let (target, params) = member.split_once('>')?;
        let target = target.trim().strip_prefix('<')?;
        let mut rel = false;
        let mut rsl = false;
        for param in params.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let Some((key, value)) = param.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            match key.trim().to_ascii_lowercase().as_str() {
                "rel" => {
                    rel = value
                        .split_whitespace()
                        .any(|r| r.eq_ignore_ascii_case("license"))
                }
                "type" => rsl = value.eq_ignore_ascii_case("application/rsl+xml"),
                _ => {}
            }
        }
        if rel && rsl {
            let base = url::Url::parse(page_url).ok()?;
            return base.join(target).ok().map(|joined| joined.to_string());
        }
    }
    None
}

/// Split a `Link` header on the commas between members, leaving commas
/// inside `<…>` and quoted strings alone.
fn split_link_members(header: &str) -> Vec<&str> {
    let mut members = Vec::new();
    let mut depth_angle = false;
    let mut quoted = false;
    let mut start = 0;
    for (index, byte) in header.bytes().enumerate() {
        match byte {
            b'<' if !quoted => depth_angle = true,
            b'>' if !quoted => depth_angle = false,
            b'"' => quoted = !quoted,
            b',' if !depth_angle && !quoted => {
                members.push(header[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    members.push(header[start..].trim());
    members.into_iter().filter(|m| !m.is_empty()).collect()
}

/// A `Cache-Control` max-age in seconds, where the header names one.
pub fn max_age(cache_control: Option<&str>) -> Option<u64> {
    cache_control?
        .split(',')
        .filter_map(|directive| directive.trim().split_once('='))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("max-age"))
        .and_then(|(_, seconds)| seconds.trim().trim_matches('"').parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The attachment draft's own example (draft-ietf-aipref-attach-05,
    /// figure 2), read for the named crawler and for everyone else.
    const DRAFT_EXAMPLE: &str = "\
User-Agent: *
Allow: /
Disallow: /never/
Content-Usage: train-ai=n
Content-Usage: /ai-ok/ train-ai=y

User-Agent: ExampleBot
Allow: /
Content-Usage: train-ai=y
";

    #[test]
    fn the_draft_example_reads_as_the_draft_says() {
        let file = parse_robots(DRAFT_EXAMPLE);

        let named = file.read("ExampleBot", "/anything");
        assert_eq!(named.group.as_deref(), Some("ExampleBot"));
        assert_eq!(named.crawlable, Some(true));
        assert_eq!(
            combine(&named.statements)[&Category::TrainAi],
            Effective::Allow
        );

        let other = file.read(PRODUCT_TOKEN, "/page");
        assert_eq!(other.group.as_deref(), Some("*"));
        assert_eq!(
            combine(&other.statements)[&Category::TrainAi],
            Effective::Disallow
        );
        assert_eq!(
            combine(&other.statements)[&Category::Search],
            Effective::Unknown,
            "an absent statement is unknown, never disallow"
        );

        let ok = file.read(PRODUCT_TOKEN, "/ai-ok/paper");
        assert_eq!(
            combine(&ok.statements)[&Category::TrainAi],
            Effective::Allow,
            "the longer path rule wins"
        );

        let never = file.read(PRODUCT_TOKEN, "/never/secret");
        assert_eq!(never.crawlable, Some(false));
        assert!(
            never.statements.is_empty(),
            "no preference is implied for a resource that cannot be crawled"
        );
    }

    #[test]
    fn the_named_group_is_selected_case_insensitively_and_carries_its_own_licence() {
        let file = parse_robots(
            "License: https://a.example/site.xml\n\
             User-agent: commonmeasurebot\n\
             Disallow: /private/\n\
             License: https://a.example/bots.xml\n\
             Content-Signal: ai-input=no, search=yes\n\
             \n\
             User-agent: *\n\
             Allow: /\n",
        );
        let reading = file.read(PRODUCT_TOKEN, "/public/x");
        assert_eq!(reading.group.as_deref(), Some(PRODUCT_TOKEN));
        assert_eq!(reading.licences, ["https://a.example/bots.xml"]);
        let effective = combine(&reading.statements);
        assert_eq!(effective[&Category::AiInput], Effective::Disallow);
        assert_eq!(effective[&Category::Search], Effective::Allow);
        assert_eq!(effective[&Category::TrainAi], Effective::Unknown);

        let private = file.read(PRODUCT_TOKEN, "/private/x");
        assert_eq!(private.crawlable, Some(false));
        assert_eq!(private.access_rule.as_deref(), Some("Disallow: /private/"));

        let other = file.read("SomeoneElse", "/x");
        assert_eq!(other.group.as_deref(), Some("*"));
        assert_eq!(
            other.licences,
            ["https://a.example/site.xml"],
            "a group without its own License: uses the global directives"
        );
    }

    /// A licence after the last group and a blank line is the site's, not
    /// the last group's: the `*` group, selected for a token no group names,
    /// reads it.
    #[test]
    fn a_licence_after_the_last_group_and_a_blank_line_is_site_wide() {
        let file = parse_robots(
            "User-agent: *\n\
             Allow: /\n\
             \n\
             User-agent: SomeOtherBot\n\
             Disallow: /\n\
             \n\
             License: https://a.example/site.xml\n",
        );
        let star = file.read(PRODUCT_TOKEN, "/news/1");
        assert_eq!(star.group.as_deref(), Some("*"));
        assert_eq!(star.crawlable, Some(true));
        assert_eq!(star.licences, ["https://a.example/site.xml"]);
        let other = file.read("SomeOtherBot", "/news/1");
        assert_eq!(other.licences, ["https://a.example/site.xml"]);
    }

    /// A `Sitemap:` line is a site-wide record and ends the group's block,
    /// so a licence after it is site-wide even without a blank line.
    #[test]
    fn a_licence_after_a_sitemap_line_is_site_wide() {
        let file = parse_robots(
            "User-agent: *\n\
             Disallow: /m/\n\
             Sitemap: https://a.example/sitemap.xml\n\
             License: https://a.example/site.xml\n",
        );
        let star = file.read(PRODUCT_TOKEN, "/story");
        assert_eq!(star.licences, ["https://a.example/site.xml"]);
        assert_eq!(
            file.read(PRODUCT_TOKEN, "/m/story").crawlable,
            Some(false),
            "the Sitemap line does not lose the group's rules"
        );
        let unnamed = parse_robots(
            "Sitemap: https://a.example/sitemap.xml\nLicense: https://a.example/site.xml\n",
        )
        .read(PRODUCT_TOKEN, "/story");
        assert_eq!(unnamed.group, None);
        assert_eq!(unnamed.licences, ["https://a.example/site.xml"]);
    }

    /// A licence written among a group's lines is that group's alone: a
    /// group without one, and no site-wide directive, reads none.
    #[test]
    fn a_licence_inside_a_group_is_that_groups_alone() {
        let file = parse_robots(
            "User-agent: *\n\
             Allow: /\n\
             License: https://a.example/everyone.xml\n\
             \n\
             User-agent: SomeOtherBot\n\
             Disallow: /\n",
        );
        assert_eq!(
            file.read(PRODUCT_TOKEN, "/x").licences,
            ["https://a.example/everyone.xml"]
        );
        assert!(file.read("SomeOtherBot", "/x").licences.is_empty());
    }

    /// The RFC 9309 group is not ended by a blank line: a rule after one
    /// still belongs to the group, and so does a licence once a rule has
    /// resumed the block.
    #[test]
    fn a_blank_line_does_not_end_the_group_for_its_rules() {
        let file = parse_robots(
            "User-agent: *\n\
             Disallow: /a\n\
             \n\
             Disallow: /b\n\
             License: https://a.example/star.xml\n\
             \n\
             User-agent: SomeOtherBot\n\
             Allow: /\n",
        );
        assert_eq!(file.read(PRODUCT_TOKEN, "/b/1").crawlable, Some(false));
        assert_eq!(
            file.read(PRODUCT_TOKEN, "/c").licences,
            ["https://a.example/star.xml"]
        );
        assert!(file.read("SomeOtherBot", "/c").licences.is_empty());
    }

    /// A `Content-Signal` line outside any group's block is site-wide and is
    /// read where the selected group carries none of its own.
    #[test]
    fn a_site_wide_content_signal_is_read_where_the_group_has_none() {
        let file = parse_robots(
            "User-agent: *\n\
             Allow: /\n\
             \n\
             User-agent: Signalled\n\
             Content-Signal: ai-input=yes\n\
             \n\
             Content-Signal: search=yes, ai-input=no\n",
        );
        let star = file.read(PRODUCT_TOKEN, "/x");
        assert_eq!(
            combine(&star.statements)[&Category::AiInput],
            Effective::Disallow
        );
        assert!(
            star.statements
                .iter()
                .all(|s| s.detail.starts_with("site-wide Content-Signal:"))
        );
        let own = file.read("Signalled", "/x");
        assert_eq!(
            combine(&own.statements)[&Category::AiInput],
            Effective::Allow
        );
        assert_eq!(
            combine(&own.statements)[&Category::Search],
            Effective::Unknown
        );
    }

    #[test]
    fn identical_paths_with_conflicting_preferences_combine_most_restrictive() {
        let file = parse_robots(
            "User-agent: *\nContent-Usage: /docs/ train-ai=y\nContent-Usage: /docs/ train-ai=n\n",
        );
        let reading = file.read(PRODUCT_TOKEN, "/docs/a");
        assert_eq!(reading.statements.len(), 2);
        assert_eq!(
            combine(&reading.statements)[&Category::TrainAi],
            Effective::Disallow
        );
    }

    #[test]
    fn wildcard_and_anchor_patterns_match_as_the_protocol_says() {
        assert!(matches_pattern("/a/*.pdf$", "/a/x/y.pdf"));
        assert!(!matches_pattern("/a/*.pdf$", "/a/x.pdf?d"));
        assert!(matches_pattern("/a/*.pdf", "/a/x.pdf?d"));
        assert!(matches_pattern("/", "/anything"));
        assert!(!matches_pattern("/b", "/a"));
        assert!(matches_pattern("/*", "/"));
    }

    /// An anchored pattern's last part must end the target, wherever else it
    /// occurs; the parts before it match leftmost and the last must follow
    /// them.
    #[test]
    fn an_anchored_pattern_matches_where_the_target_ends() {
        assert!(matches_pattern("/*.pdf$", "/a.pdf/b.pdf"));
        assert!(matches_pattern("/*.pdf$", "/.pdf"));
        assert!(!matches_pattern("/*.pdf$", "/a.pdf/b"));
        assert!(matches_pattern("/a*b$", "/axbyb"));
        assert!(!matches_pattern("/*ab*b$", "/ab"));
        assert!(matches_pattern("/a$", "/a"));
        assert!(!matches_pattern("/a$", "/ab"));
        assert!(matches_pattern("/a*$", "/abc"));
        let file = parse_robots("User-agent: *\nDisallow: /*.pdf$\n");
        let reading = file.read(PRODUCT_TOKEN, "/a.pdf/b.pdf");
        assert_eq!(reading.crawlable, Some(false), "{reading:?}");
        let file = parse_robots("User-agent: *\nAllow: /\nContent-Usage: /*.pdf$ ai-use=n\n");
        let reading = file.read(PRODUCT_TOKEN, "/a.pdf/b.pdf");
        assert_eq!(
            combine(&reading.statements)[&Category::AiInput],
            Effective::Disallow
        );
    }

    /// A bare `*` or final `$` in a pattern is an operator, so a rule names a
    /// literal `*` or `$` as `%2A` or `%24` (RFC 9309 section 2.2.3), and the
    /// URL's literal meets it there. A `$` that is not final is a literal.
    #[test]
    fn a_rule_names_a_literal_star_or_dollar_by_its_encoding() {
        let crawlable = |rules: &str, path: &str| {
            parse_robots(&format!("User-agent: *\n{rules}\n"))
                .read(PRODUCT_TOKEN, path)
                .crawlable
        };
        assert_eq!(crawlable("Disallow: /a$", "/a$"), Some(true));
        assert_eq!(crawlable("Disallow: /a*", "/a$"), Some(false));
        assert_eq!(crawlable("Disallow: /a*", "/a%2A"), Some(false));
        assert_eq!(
            crawlable(
                "Disallow: /path/file-with-a-%2A.html",
                "/path/file-with-a-*.html"
            ),
            Some(false)
        );
        assert_eq!(
            crawlable("Disallow: /path/foo-%24", "/path/foo-$"),
            Some(false)
        );
        assert_eq!(crawlable("Disallow: /a$b", "/a$b"), Some(false));
        assert_eq!(crawlable("Disallow: /a$b", "/a%24b"), Some(false));
    }

    /// A page is read as parsed, merged and with its separators decoded. A
    /// `Disallow` any reading reaches refuses it, and no `Allow` in another
    /// reading lifts that; `Content-Usage` rules every reading selects all
    /// apply. A rule's own `//` is read as written.
    #[test]
    fn a_disallow_any_reading_of_the_page_reaches_refuses_it() {
        let read = |rules: &str, path: &str| {
            parse_robots(&format!("User-agent: *\n{rules}\n")).read(PRODUCT_TOKEN, path)
        };
        for (rules, path, crawlable, rule) in [
            (
                "Disallow: /a/\nAllow: /a/b",
                "/a//b",
                false,
                "Disallow: /a/",
            ),
            ("Disallow: /a/\nAllow: /a/b", "/a/b", true, "Allow: /a/b"),
            (
                "Disallow: /*/private",
                "//private",
                false,
                "Disallow: /*/private",
            ),
            ("Disallow: /news/", "//news/1", false, "Disallow: /news/"),
            ("Disallow: /news/", "/news%2F1", false, "Disallow: /news/"),
            (
                "Disallow: /news/",
                "/x/..%2Fnews/1",
                false,
                "Disallow: /news/",
            ),
            (
                "Disallow: /\nAllow: /news/",
                "//news/1",
                false,
                "Disallow: /",
            ),
            (
                "Disallow: /\nAllow: /news/",
                "/news/1",
                true,
                "Allow: /news/",
            ),
            ("Disallow: //", "//news/1", false, "Disallow: //"),
            (
                "Allow: /\nDisallow: /news/",
                "//news/1",
                false,
                "Disallow: /news/",
            ),
            (
                "Allow: /\nDisallow: /news/",
                "/news%2F1",
                false,
                "Disallow: /news/",
            ),
            (
                "Allow: /\nDisallow: /news/",
                "/x/..%2Fnews/1",
                false,
                "Disallow: /news/",
            ),
            (
                "Allow: /\nDisallow: /news/",
                "/news/1",
                false,
                "Disallow: /news/",
            ),
            (
                "Disallow: /a/b%2Fc",
                "/a//b%2Fc",
                false,
                "Disallow: /a/b%2Fc",
            ),
            (
                "Allow: /\nDisallow: /news//",
                "/news//1",
                false,
                "Disallow: /news//",
            ),
        ] {
            let reading = read(rules, path);
            assert_eq!(reading.crawlable, Some(crawlable), "{rules} {path}");
            assert_eq!(reading.access_rule.as_deref(), Some(rule), "{rules} {path}");
        }
        for (rules, path) in [
            ("Disallow: //", "/"),
            ("Disallow: //", "/news/1"),
            ("Disallow: //", "/news//1"),
            ("Disallow: /*//", "/news/1"),
            ("Disallow: /*//", "/news/a/b"),
            ("Allow: /\nDisallow: /news//", "/news/1"),
            ("Disallow: /news/1/", "/news%2F1"),
            ("Disallow: /a%2Fb", "/a/b"),
        ] {
            assert_ne!(read(rules, path).crawlable, Some(false), "{rules} {path}");
        }
        let reading = read(
            "Content-Usage: /a/ ai-use=n\nContent-Usage: /a/b ai-use=y",
            "/a//b",
        );
        assert_eq!(
            combine(&reading.statements)[&Category::AiInput],
            Effective::Disallow
        );
        assert!(
            read("Content-Usage: // ai-use=n", "/news/1")
                .statements
                .is_empty()
        );
    }

    /// Decoding `%2F`, merging `/` and resolving dot segments, in any order
    /// and including only some of them: a `Disallow` any of those paths
    /// reaches refuses the page, and a `Content-Usage` rule any of them
    /// selects applies.
    #[test]
    fn a_disallow_any_order_of_the_operations_reaches_refuses_it() {
        let read = |rules: &str, path: &str| {
            parse_robots(&format!("User-agent: *\n{rules}\n")).read(PRODUCT_TOKEN, path)
        };
        for (rules, path, rule) in [
            ("Disallow: /news/", "/x//..%2Fnews/1", "Disallow: /news/"),
            (
                "Disallow: /x/news/",
                "/x//..%2Fnews/1",
                "Disallow: /x/news/",
            ),
            (
                "Allow: /\nDisallow: /news/",
                "/x//..%2Fnews/1",
                "Disallow: /news/",
            ),
            (
                "Disallow: /a/news/",
                "/a//..%2F/news/1",
                "Disallow: /a/news/",
            ),
            (
                "Disallow: /a/news/",
                "/a/b//..%2F%2F..%2Fnews/1",
                "Disallow: /a/news/",
            ),
            ("Disallow: //news/", "/a//..%2F/news/1", "Disallow: //news/"),
            (
                "Disallow: /news/",
                "/news%2F..%2Fsports/1",
                "Disallow: /news/",
            ),
        ] {
            let reading = read(rules, path);
            assert_eq!(reading.crawlable, Some(false), "{rules} {path}");
            assert_eq!(reading.access_rule.as_deref(), Some(rule), "{rules} {path}");
        }
        let reading = read(
            "Content-Usage: /news/ ai-use=n\nContent-Usage: /x/ ai-use=y",
            "/x//..%2Fnews/1",
        );
        assert_eq!(
            combine(&reading.statements)[&Category::AiInput],
            Effective::Disallow
        );
    }

    /// A page past the readings cap is refused unread, with or without a
    /// group, and the reason names the cap.
    #[test]
    fn a_page_past_the_readings_cap_is_refused() {
        let too_many = commonmeasure_types::TooManyReadings { cap: 64 };
        for robots in [
            "User-agent: *\nAllow: /\n",
            "",
            "User-agent: other\nDisallow: /\n",
        ] {
            let reading = parse_robots(robots).read_readings(PRODUCT_TOKEN, Err(too_many));
            assert_eq!(reading.crawlable, Some(false), "{robots:?}");
            assert_eq!(reading.too_many_readings, Some(too_many), "{robots:?}");
            assert!(
                reading
                    .access_rule
                    .as_deref()
                    .is_some_and(|reason| reason.contains("more than 64 readings")),
                "{robots:?}: {reading:?}"
            );
            assert!(reading.statements.is_empty(), "{robots:?}");
        }
    }

    /// Rules rank by the length of their matching form, so spelling a rule
    /// longer does not make it more specific; two spellings of one path tie,
    /// and the tie rule decides as it does for two identical rules.
    #[test]
    fn a_rule_ranks_by_the_path_it_names_whatever_its_spelling() {
        let file = parse_robots("User-agent: *\nDisallow: /%6Eews/\nAllow: /news/1\n");
        let reading = file.read(PRODUCT_TOKEN, "/news/1");
        assert_eq!(reading.crawlable, Some(true), "{reading:?}");
        assert_eq!(reading.access_rule.as_deref(), Some("Allow: /news/1"));
        let reading = file.read(PRODUCT_TOKEN, "/%6Eews/2");
        assert_eq!(reading.crawlable, Some(false), "{reading:?}");

        // A tie that only exists in the matching form: Allow wins it, as it
        // wins any equal-length match (RFC 9309 section 2.2.2).
        let file = parse_robots("User-agent: *\nDisallow: /%6Eews/\nAllow: /news/\n");
        for path in ["/news/1", "/%6Eews/1"] {
            let reading = file.read(PRODUCT_TOKEN, path);
            assert_eq!(reading.crawlable, Some(true), "{path}");
            assert_eq!(reading.access_rule.as_deref(), Some("Allow: /news/"));
        }

        // Two Content-Usage rules naming one path in two spellings are
        // identical paths: both apply, most restrictive wins.
        let file = parse_robots(
            "User-agent: *\nContent-Usage: /news/ ai-use=y\nContent-Usage: /%6Eews/ ai-use=n\n",
        );
        for path in ["/news/1", "/%6Eews/1"] {
            let reading = file.read(PRODUCT_TOKEN, path);
            assert_eq!(
                combine(&reading.statements)[&Category::AiInput],
                Effective::Disallow,
                "{path}"
            );
        }
    }

    /// Two relative `<content>` entries naming one path in two spellings
    /// tie; the later one governs in either order, as between two identical
    /// entries.
    #[test]
    fn two_spellings_of_one_relative_scope_tie_and_the_later_governs() {
        let entry = |url: &str| format!(r#"<content url="{url}"><license/></content>"#);
        for (first, second) in [("/news/", "/%6Eews/"), ("/%6Eews/", "/news/")] {
            let document = parse_rsl(&format!(
                r#"<rsl xmlns="https://rslstandard.org/rsl">{}{}</rsl>"#,
                entry(first),
                entry(second)
            ))
            .expect("parses");
            for page in ["https://example.com/news/1", "https://example.com/%6Eews/1"] {
                assert_eq!(
                    document
                        .content_for(page)
                        .map(|content| content.url.as_str()),
                    Some(second),
                    "{first} then {second}: {page}"
                );
            }
        }
    }

    #[test]
    fn a_content_usage_dictionary_follows_the_vocabulary_drafts_rules() {
        let read = |value: &str| {
            parse_content_usage(value)
                .into_iter()
                .map(|(c, p, _)| (c, p))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            read("train-ai=n"),
            [(Category::TrainAi, Preference::Disallow)]
        );
        assert_eq!(
            read("train-ai=y, search=n"),
            [
                (Category::Search, Preference::Disallow),
                (Category::TrainAi, Preference::Allow),
            ]
        );
        // Section 6.5.1: a repeated key applies its last value, and a
        // non-Token value makes the preference unknown.
        assert!(read("train-ai=y, train-ai, search=n, search=\"n\"").is_empty());
        // Section 6.5.2: a parameter is carried and ignored.
        assert_eq!(
            read("train-ai;allow=n, train-ai=y"),
            [(Category::TrainAi, Preference::Allow)]
        );
        // Uppercase is a parse failure: the format operates on bytes.
        assert!(read("Train-AI=n").is_empty());
        assert!(read("search=maybe").is_empty());
        assert_eq!(
            read("ai-use=n, unknown-thing=y"),
            [(Category::AiInput, Preference::Disallow)]
        );
    }

    #[test]
    fn a_content_signal_line_reads_three_signals_and_nothing_else() {
        let read: Vec<_> = parse_content_signal("search=yes, ai-input=no, ai-train=no, other=yes")
            .into_iter()
            .map(|(c, p, _)| (c, p))
            .collect();
        assert_eq!(
            read,
            [
                (Category::Search, Preference::Allow),
                (Category::AiInput, Preference::Disallow),
                (Category::TrainAi, Preference::Disallow),
            ]
        );
    }

    const RSL: &str = r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/">
    <license>
      <permits type="usage">search</permits>
      <payment type="attribution"/>
    </license>
    <license>
      <permits type="usage">ai-input</permits>
      <payment type="use">
        <amount currency="USD">0.015</amount>
        <standard>https://example.com/licenses/pay-per-use</standard>
      </payment>
      <reporting type="telemetry" profile="https://contenttelemetry.org/profiles/spur"
                 endpoint="https://telemetry.example.com/v1/events">
        <![CDATA[{"conformance_level": "grounding", "privacy_level": "minimal"}]]>
      </reporting>
    </license>
  </content>
  <content url="/archive/*">
    <license>
      <prohibits type="usage">ai-all</prohibits>
    </license>
  </content>
</rsl>"#;

    fn demand_profiles(xml: &str) -> Vec<String> {
        let document = parse_rsl(xml).expect("parses");
        let page = document
            .content_for("https://example.com/a")
            .expect("the entry");
        licence_terms(page, "https://example.com/license.xml")
            .reporting
            .into_iter()
            .map(|demand| demand.profile)
            .collect()
    }

    #[test]
    fn reporting_demands_bind_whatever_the_licence_says_of_ai_input() {
        let wrap = |licences: &str| {
            format!(
                r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/">{licences}</content></rsl>"#
            )
        };
        let reporting = |profile: &str| {
            format!(r#"<reporting type="telemetry" profile="https://p.example/{profile}"/>"#)
        };
        // No usage <permits>: section 3.5 restricts nothing, so the licence
        // authorises AI input and its demand binds.
        assert_eq!(
            demand_profiles(&wrap(&format!(
                "<license>{}</license>",
                reporting("silent")
            ))),
            ["https://p.example/silent"]
        );
        // A licence permitting AI input by name is the one taken, before one
        // silent on usage.
        assert_eq!(
            demand_profiles(&wrap(&format!(
                r#"<license>{}</license><license><permits type="usage">ai-input</permits>{}</license>"#,
                reporting("silent"),
                reporting("named")
            ))),
            ["https://p.example/named"]
        );
        // No licence authorises AI input: every demand in the entry binds a
        // crossing that goes on anyway.
        assert_eq!(
            demand_profiles(&wrap(&format!(
                r#"<license><prohibits type="usage">ai-all</prohibits>{}</license><license><permits type="usage">search</permits>{}</license>"#,
                reporting("prohibits"),
                reporting("search")
            ))),
            ["https://p.example/prohibits", "https://p.example/search"]
        );
    }

    #[test]
    fn an_rsl_document_yields_statements_payment_and_the_reporting_binding() {
        let document = parse_rsl(RSL).expect("parses");
        assert_eq!(document.contents.len(), 2);

        let page = document
            .content_for("https://example.com/news/1")
            .expect("the site-wide entry");
        assert_eq!(page.url, "/");
        let terms = licence_terms(page, "https://example.com/license.xml");
        let effective = combine(&terms.statements);
        assert_eq!(effective[&Category::AiInput], Effective::Allow);
        assert_eq!(effective[&Category::Search], Effective::Allow);
        assert_eq!(
            effective[&Category::TrainAi],
            Effective::Disallow,
            "both licences list usage permits and neither lists training"
        );
        let payment = terms.payment.expect("the ai-input licence's payment");
        assert_eq!(payment.kind.as_deref(), Some("use"));
        assert!(payment.is_monetary());
        assert_eq!(payment.price(), Some(Money::new("USD", 15_000)));
        assert_eq!(terms.reporting.len(), 1);
        assert_eq!(terms.reporting[0].profile, TELEMETRY_PROFILE);
        assert_eq!(
            terms.reporting[0]
                .config
                .as_ref()
                .map(|c| c["conformance_level"].clone()),
            Some(serde_json::json!("grounding"))
        );

        let archive = document
            .content_for("https://example.com/archive/2020/x")
            .expect("the more specific entry");
        assert_eq!(archive.url, "/archive/*");
        let terms = licence_terms(archive, "https://example.com/license.xml");
        assert_eq!(
            combine(&terms.statements)[&Category::AiInput],
            Effective::Disallow
        );
        assert_eq!(
            combine(&terms.statements)[&Category::Search],
            Effective::Unknown,
            "ai-all covers the AI uses and says nothing about search"
        );
    }

    /// An absolute scope governs every spelling of a page under it, whichever
    /// side carries the spelling, and nothing that names another resource.
    #[test]
    fn an_absolute_content_scope_matches_every_spelling_of_the_same_resource() {
        let scoped = |scope: &str| {
            parse_rsl(&format!(
                r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="{scope}"><license>
      <prohibits type="usage">ai-input</prohibits></license></content></rsl>"#
            ))
            .expect("parses")
        };
        for scope in [
            "https://publisher.example/news/",
            "https://publisher.example./news/",
            "https://PUBLISHER.example/news/",
            "https://publisher.example:443/news/",
        ] {
            let document = scoped(scope);
            for page in [
                "https://publisher.example/news/1",
                "https://publisher.example./news/1",
                "https://Publisher.Example/news/1",
                "https://publisher.example:443/news/1",
                "HTTPS://PUBLISHER.EXAMPLE.:443/news/1",
            ] {
                assert_eq!(
                    document
                        .content_for(page)
                        .map(|content| content.url.as_str()),
                    Some(scope),
                    "{scope} governs {page}"
                );
            }
            for elsewhere in [
                "http://publisher.example/news/1",
                "https://publisher.example:8443/news/1",
                "https://publisher.example/News/1",
                "https://publisher.example.evil.test/news/1",
                "https://publisher.example/sport/1",
            ] {
                assert!(
                    document.content_for(elsewhere).is_none(),
                    "{scope} does not govern {elsewhere}"
                );
            }
        }
        let document = scoped("http://publisher.example:80/");
        assert!(document.content_for("http://publisher.example/x").is_some());
        assert!(
            document
                .content_for("https://publisher.example/x")
                .is_none()
        );
    }

    /// The `url` of the entry governing `page` in a licence of `(url, rule)`
    /// entries, each permitting or prohibiting AI input.
    fn governing(entries: &[(&str, &str)], page: &str) -> Option<String> {
        let contents: String = entries
            .iter()
            .map(|(url, rule)| {
                format!(
                    r#"<content url="{url}"><license><{rule} type="usage">ai-input</{rule}></license></content>"#
                )
            })
            .collect();
        parse_rsl(&format!(
            r#"<rsl xmlns="https://rslstandard.org/rsl">{contents}</rsl>"#
        ))
        .expect("parses")
        .content_for(page)
        .map(|content| content.url.clone())
    }

    /// Entries rank by the path they constrain, whatever the spelling of the
    /// host in front of it: a narrower scope governs under a broad one
    /// written longer (`https://PUBLISHER.example.:443/` is 31 characters,
    /// `https://publisher.example/n/` 28), and a relative entry governs under
    /// an absolute scope that constrains a shorter path.
    #[test]
    fn the_entry_constraining_the_longer_path_governs_whatever_its_spelling() {
        let broad = ("https://PUBLISHER.example.:443/", "permits");
        let narrow = ("https://publisher.example/n/", "prohibits");
        for entries in [[broad, narrow], [narrow, broad]] {
            assert_eq!(
                governing(&entries, "https://publisher.example/n/1").as_deref(),
                Some(narrow.0),
                "{entries:?}"
            );
            assert_eq!(
                governing(&entries, "https://publisher.example/x").as_deref(),
                Some(broad.0),
                "{entries:?}"
            );
        }
        let site = ("https://publisher.example/", "permits");
        let secret = ("/news/secret/", "prohibits");
        for entries in [[site, secret], [secret, site]] {
            assert_eq!(
                governing(&entries, "https://publisher.example/news/secret/1").as_deref(),
                Some(secret.0),
                "{entries:?}"
            );
        }
        // An empty `url` governs only where nothing else matches.
        let unscoped = ("", "prohibits");
        assert_eq!(
            governing(&[site, unscoped], "https://publisher.example/a").as_deref(),
            Some(site.0)
        );
    }

    /// An absolute scope and a relative entry constraining the same path rank
    /// equally by length. The absolute scope governs in either document
    /// order, so which of opposite rules applies does not depend on the order
    /// (RSL 3.1.1).
    #[test]
    fn an_absolute_scope_governs_a_relative_entry_on_the_same_path_in_either_order() {
        for (absolute, relative) in [
            (
                ("https://publisher.example/n/", "prohibits"),
                ("/n/", "permits"),
            ),
            (
                ("https://publisher.example/n/", "permits"),
                ("/n/", "prohibits"),
            ),
            (
                ("https://publisher.example/", "prohibits"),
                ("/", "permits"),
            ),
        ] {
            for entries in [[absolute, relative], [relative, absolute]] {
                assert_eq!(
                    governing(&entries, "https://publisher.example/n/1").as_deref(),
                    Some(absolute.0),
                    "{entries:?}"
                );
            }
        }
    }

    /// Two spellings of one absolute scope are equally specific. The one
    /// matching the page as written governs, then the one written longer, in
    /// either document order, so a permit written after a prohibition of the
    /// same scope does not admit the page.
    #[test]
    fn two_spellings_of_one_absolute_scope_rank_as_written_in_either_order() {
        for (prohibition, permit, page) in [
            // Both match as written; the longer spelling governs.
            (
                "https://publisher.example/",
                "https://publisher.example",
                "https://publisher.example/n/1",
            ),
            // Only the prohibition matches as written.
            (
                "https://publisher.example/n/",
                "HTTPS://PUBLISHER.EXAMPLE./n/",
                "https://publisher.example/n/1",
            ),
            // An explicit default port on the scopes and the page.
            (
                "https://publisher.example:443/",
                "https://publisher.example:443",
                "https://publisher.example:443/n/1",
            ),
        ] {
            let (prohibits, permits) = ((prohibition, "prohibits"), (permit, "permits"));
            for entries in [[prohibits, permits], [permits, prohibits]] {
                assert_eq!(
                    governing(&entries, page).as_deref(),
                    Some(prohibition),
                    "{entries:?}"
                );
            }
        }
    }

    /// Where the ranking leaves two entries equal, the later governs: two
    /// relative entries of one length, and two absolute scopes of one written
    /// length that neither matches the page as written. Only the order with
    /// the prohibition last is pinned. Refusal there is also the outcome a
    /// conforming evaluation of tied entries gives, so this holds when one
    /// replaces document order.
    #[test]
    fn a_prohibition_written_after_an_equally_ranked_permit_governs() {
        for (permit, prohibition) in [
            ("/n", "/*"),
            (
                "HTTPS://PUBLISHER.EXAMPLE./n/",
                "https://publisher.example./n/",
            ),
        ] {
            assert_eq!(
                governing(
                    &[(permit, "permits"), (prohibition, "prohibits")],
                    "https://publisher.example/n/1"
                )
                .as_deref(),
                Some(prohibition)
            );
        }
    }

    /// An absolute scope that does not parse is matched as written, so
    /// `http://` is under every `http` page; it ranks as the least specific
    /// and cannot outrank a valid narrower scope.
    #[test]
    fn an_absolute_scope_that_does_not_parse_ranks_below_a_valid_one() {
        let malformed = ("http://", "permits");
        let valid = ("http://publisher.example/n/", "prohibits");
        for entries in [[malformed, valid], [valid, malformed]] {
            assert_eq!(
                governing(&entries, "http://publisher.example/n/1").as_deref(),
                Some(valid.0),
                "{entries:?}"
            );
            assert_eq!(
                governing(&entries, "http://publisher.example/x").as_deref(),
                Some(malformed.0),
                "{entries:?}"
            );
        }
    }

    /// A scope that does not parse and an empty `url` both rank 0. The scope
    /// governs in either document order, as it did when it ranked by its
    /// written length.
    #[test]
    fn an_absolute_scope_that_does_not_parse_governs_over_an_empty_url_in_either_order() {
        let malformed = ("https://", "prohibits");
        let unscoped = ("", "permits");
        for entries in [[malformed, unscoped], [unscoped, malformed]] {
            assert_eq!(
                governing(&entries, "https://publisher.example/n/1").as_deref(),
                Some(malformed.0),
                "{entries:?}"
            );
        }
    }

    /// The query is part of the path an absolute scope constrains, as it is
    /// of the request target a relative entry is matched against.
    #[test]
    fn an_absolute_scope_ranks_by_its_query_as_well_as_its_path() {
        let queried = ("https://publisher.example/news?x=1", "prohibits");
        let relative = ("/news?x", "permits");
        for entries in [[queried, relative], [relative, queried]] {
            assert_eq!(
                governing(&entries, "https://publisher.example/news?x=1").as_deref(),
                Some(queried.0),
                "{entries:?}"
            );
        }
    }

    /// The shape a national newspaper publishes: AI training and AI input
    /// permitted under a subscription, everything else prohibited by a
    /// blanket `all`. RSL 3.1.1 ranks the specific permit over the general
    /// prohibition, so AI input is permitted with the payment term attached,
    /// and search, which nothing permits, is disallowed. A prohibition
    /// naming an AI use — the same token as the permit, or `ai-all` — wins
    /// over the permit, read conservatively.
    #[test]
    fn a_specific_permit_stands_over_a_blanket_prohibition_and_a_matching_one_does_not() {
        let document = parse_rsl(
            r#"<rsl xmlns="https://rslstandard.org/rsl">
  <content url="/">
    <license>
      <permits type="usage">ai-train ai-input</permits>
      <payment type="subscription">
        <custom>https://licensing.example.com/</custom>
      </payment>
      <prohibits type="usage">all</prohibits>
    </license>
  </content>
</rsl>"#,
        )
        .expect("parses");
        let page = document
            .content_for("https://example.com/x")
            .expect("entry");
        let terms = licence_terms(page, "https://example.com/license.xml");
        let effective = combine(&terms.statements);
        assert_eq!(effective[&Category::AiInput], Effective::Allow);
        assert_eq!(effective[&Category::TrainAi], Effective::Allow);
        assert_eq!(effective[&Category::Search], Effective::Disallow);
        let payment = terms
            .payment
            .expect("the subscription travels with the permit");
        assert_eq!(payment.kind.as_deref(), Some("subscription"));
        assert!(payment.is_monetary());

        for prohibits in ["ai-input", "ai-all"] {
            let document = parse_rsl(&format!(
                r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
      <permits type="usage">ai-input</permits>
      <prohibits type="usage">{prohibits}</prohibits>
    </license></content></rsl>"#
            ))
            .expect("parses");
            let page = document
                .content_for("https://example.com/x")
                .expect("entry");
            let terms = licence_terms(page, "https://example.com/license.xml");
            assert_eq!(
                combine(&terms.statements)[&Category::AiInput],
                Effective::Disallow,
                "prohibits {prohibits} is at least as specific as permits ai-input, so it wins"
            );
        }
    }

    #[test]
    fn an_rsl_document_without_content_is_an_error_and_malformed_xml_is_too() {
        assert!(parse_rsl("<rsl xmlns=\"https://rslstandard.org/rsl\"></rsl>").is_err());
        assert!(parse_rsl("<rsl><content url=\"/\">").is_err());
    }

    #[test]
    fn a_link_header_names_the_rsl_licence_only_when_typed_as_one() {
        assert_eq!(
            rsl_link(
                "<https://example.com/license.xml>; rel=\"license\"; type=\"application/rsl+xml\"",
                "https://example.com/a"
            )
            .as_deref(),
            Some("https://example.com/license.xml")
        );
        assert_eq!(
            rsl_link(
                "</style.css>; rel=preload, </license.xml>; rel=license; type=\"application/rsl+xml\"",
                "https://example.com/a/b"
            )
            .as_deref(),
            Some("https://example.com/license.xml")
        );
        assert!(
            rsl_link(
                "<https://creativecommons.org/licenses/by/4.0/>; rel=\"license\"",
                "https://example.com/a"
            )
            .is_none(),
            "a licence link without the RSL type is not an RSL licence"
        );
    }

    #[test]
    fn max_age_is_read_from_cache_control() {
        assert_eq!(max_age(Some("public, max-age=3600")), Some(3600));
        assert_eq!(max_age(Some("no-store")), None);
        assert_eq!(max_age(None), None);
    }

    fn delay_of(robots: &str) -> Option<CrawlDelay> {
        parse_robots(robots).read(PRODUCT_TOKEN, "/").crawl_delay
    }

    #[test]
    fn crawl_delay_is_read_from_the_group_the_access_rules_come_from() {
        let named = delay_of(
            "User-agent: *\nCrawl-delay: 10\n\nUser-agent: CommonMeasureBot\nCrawl-delay: 2\n",
        )
        .unwrap();
        assert_eq!(named.value.as_deref(), Some("2"));
        assert_eq!(named.delay_ms, Some(2_000));
        assert_eq!(named.honoured_ms, Some(2_000));
        assert!(!named.capped);

        let fallback = delay_of("User-agent: *\nDisallow: /private\nCrawl-delay: 0.25\n").unwrap();
        assert_eq!(fallback.delay_ms, Some(250));

        // A group naming the product token without a delay stands over the
        // `*` group's, as its access rules do.
        assert_eq!(
            delay_of("User-agent: *\nCrawl-delay: 10\n\nUser-agent: CommonMeasureBot\nAllow: /\n"),
            None
        );
        // Before any group there is no group to hold it.
        assert_eq!(delay_of("Crawl-delay: 5\nUser-agent: *\nAllow: /\n"), None);
        // The label is case-insensitive, as every robots.txt label is.
        assert_eq!(
            delay_of("user-agent: commonmeasurebot\nCRAWL-DELAY: 3\n")
                .unwrap()
                .delay_ms,
            Some(3_000)
        );
    }

    #[test]
    fn an_unreadable_crawl_delay_is_recorded_and_ignored() {
        for written in ["-1", "soon", "1e3", "inf", "+2", "1.2.3", "", "."] {
            let delay = delay_of(&format!("User-agent: *\nCrawl-delay: {written}\n")).unwrap();
            assert_eq!(delay.delay_ms, None, "{written:?}");
            assert_eq!(delay.honoured_ms, None, "{written:?}");
            assert_eq!(delay.unreadable, vec![written.to_owned()], "{written:?}");
        }
        let mixed =
            delay_of("User-agent: *\nCrawl-delay: never\nCrawl-delay: 1.5\nCrawl-delay: 1\n")
                .unwrap();
        assert_eq!(mixed.value.as_deref(), Some("1.5"));
        assert_eq!(mixed.delay_ms, Some(1_500));
        assert_eq!(mixed.unreadable, vec!["never".to_owned()]);
    }

    #[test]
    fn a_crawl_delay_over_the_bound_is_kept_at_the_bound_and_recorded_as_capped() {
        let delay = delay_of("User-agent: CommonMeasureBot\nCrawl-delay: 3600\n").unwrap();
        assert_eq!(delay.delay_ms, Some(3_600_000));
        assert_eq!(delay.honoured_ms, Some(60_000));
        assert!(delay.capped);
        let absurd = delay_of("User-agent: *\nCrawl-delay: 99999999999999999999999\n").unwrap();
        assert_eq!(absurd.honoured_ms, Some(60_000));
        assert!(absurd.capped);
    }

    #[test]
    fn every_group_naming_the_token_is_merged_before_the_wildcard_is_considered() {
        // RFC 9309 section 2.2.1: a second group naming the token is part of
        // the same group, so its Disallow applies (QA-EGR-27).
        let two_groups = "\
User-agent: *
Disallow: /

User-agent: CommonMeasureBot
Allow: /

User-agent: CommonMeasureBot
Disallow: /private
Crawl-delay: 7
";
        let file = parse_robots(two_groups);
        let open = file.read(PRODUCT_TOKEN, "/news");
        assert_eq!(open.crawlable, Some(true));
        assert!(!open.group_is_wildcard);
        let closed = file.read(PRODUCT_TOKEN, "/private/one");
        assert_eq!(closed.crawlable, Some(false), "{closed:?}");
        assert_eq!(closed.access_rule.as_deref(), Some("Disallow: /private"));
        // The `*` group's Disallow is not what refused it: a group names the
        // token, so the wildcard is not read at all.
        assert_eq!(open.group.as_deref(), Some(PRODUCT_TOKEN));

        // The delay of the second group is the group's delay.
        assert_eq!(
            delay_of(two_groups).and_then(|delay| delay.delay_ms),
            Some(7_000)
        );
    }

    #[test]
    fn a_crawl_delay_split_across_two_groups_keeps_the_longest() {
        let delay = delay_of(
            "User-agent: CommonMeasureBot\nCrawl-delay: 2\n\nUser-agent: CommonMeasureBot\n\
             Crawl-delay: 9\n\nUser-agent: *\nCrawl-delay: 30\n",
        )
        .unwrap();
        assert_eq!(delay.delay_ms, Some(9_000));
        assert_eq!(delay.value.as_deref(), Some("9"));

        // Two wildcard groups merge the same way when no group names the
        // token.
        let wildcard = delay_of(
            "User-agent: *\nCrawl-delay: 3\n\nUser-agent: *\nAllow: /\n\
             Crawl-delay: 11\n",
        )
        .unwrap();
        assert_eq!(wildcard.delay_ms, Some(11_000));
    }

    #[test]
    fn merged_groups_keep_every_licence_and_content_usage_rule() {
        let file = parse_robots(
            "User-agent: CommonMeasureBot\nAllow: /\nContent-Usage: /news/ train-ai=n\n\
             License: https://a.example/one.xml\n\nUser-agent: CommonMeasureBot\nAllow: /\n\
             License: https://a.example/two.xml\n",
        );
        let reading = file.read(PRODUCT_TOKEN, "/news/1");
        assert_eq!(
            reading.licences,
            vec![
                "https://a.example/one.xml".to_owned(),
                "https://a.example/two.xml".to_owned()
            ]
        );
        assert!(
            reading
                .statements
                .iter()
                .any(|statement| statement.detail.contains("train-ai")),
            "{:?}",
            reading.statements
        );
    }
}
