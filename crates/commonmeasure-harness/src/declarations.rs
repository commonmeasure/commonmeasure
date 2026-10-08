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

use std::borrow::Cow;
use std::collections::BTreeMap;

use commonmeasure_runtime::agent_text::AgentText;
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
                            // The rule's path is the source's text; a
                            // statement's detail reaches the agent.
                            Some(rule_path) => format!(
                                "group {name}: Content-Usage: {} {label}",
                                crate::source_text::quoted_path(rule_path)
                            ),
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
    /// listed uses are allowed under this licence (RSL section 3.5). The
    /// list is the union across the licence's usage `<permits>` elements:
    /// §3.5 allows one, and a document that writes more states each as a
    /// term of the licence, so none is dropped and their order does not
    /// change what they mean (§3.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permits_usage: Option<Vec<String>>,
    /// `<prohibits type="usage">` tokens, or absent: the union across the
    /// licence's usage `<prohibits>` elements, as for `permits_usage`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prohibits_usage: Option<Vec<String>>,
    /// Every `<payment>` element, in document order. The grammar allows one;
    /// a document that writes more states each as a term of the licence, so
    /// none is dropped ([`Self::payment`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payments: Vec<RslPayment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reporting: Vec<RslReporting>,
    /// True where a `<permits>` or `<prohibits>` of type `user` or `geo`
    /// restricts the licence to a class of user or a region. The edge
    /// cannot show it is in the class, so the licence authorises nothing
    /// ([`ai_input_grant`]). For this edge it permits none of the usages it
    /// names, and, where it is silent on usage and asks for payment, no AI
    /// input ([`licence_terms`]).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restricted: bool,
}

impl RslLicence {
    /// The licence's payment term as ruled: the first `<payment>` that
    /// needs settlement, else the first. A licence whose `<payment>`
    /// elements disagree asks for payment if any one does, since reading
    /// the free one would take a priced term as met.
    pub fn payment(&self) -> Option<&RslPayment> {
        self.payments
            .iter()
            .find(|payment| payment.needs_settlement())
            .or(self.payments.first())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RslPayment {
    /// The `type` attribute as written. RSL 1.0 §3.7 says it MUST be one of
    /// `purchase`, `subscription`, `training`, `crawl`, `use`,
    /// `contribution`, `attribution` or `free`, and states no default for
    /// a `<payment>` that names none ([`Self::needs_settlement`] says how
    /// each is read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `<amount currency="…">decimal</amount>`, kept as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<RslAmount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
    /// `<accepts>`, the payment methods the source takes, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepts: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RslAmount {
    pub currency: String,
    pub decimal: String,
}

impl RslPayment {
    /// Whether this payment term asks for something only a settlement rail
    /// could give, so an edge without one cannot meet it. The one rule every
    /// ruling on a payment term uses: the payment refusal, the price the
    /// allowance gate reserves and the choice among a content entry's
    /// offers ([`governing_offer`]).
    ///
    /// A term is met without a rail only where it is known to be free:
    /// type `free` or `attribution`, or no type and nothing else stated (no
    /// `<amount>`, `<standard>`, `<custom>` or `<accepts>`), which says no
    /// more than an
    /// omitted `<payment>`, which RSL 1.0 §3.7 reads as free. Every other
    /// term needs settlement: the six monetary types, a type the
    /// specification does not define (one a later revision adds included),
    /// a case variant of a defined one (§3.7 gives the values as written,
    /// and XML attribute values are case-sensitive), and a term with no
    /// type that states an amount, points at terms or lists the payment
    /// methods it accepts, since §3.7 gives no default type and the edge
    /// does not read a stated price, licence or means of payment as free.
    pub fn needs_settlement(&self) -> bool {
        match self.kind.as_deref() {
            Some("free" | "attribution") => false,
            Some(_) => true,
            None => {
                self.amount.is_some()
                    || self.standard.is_some()
                    || self.custom.is_some()
                    || self.accepts.is_some()
            }
        }
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

/// Why an RSL document is not well-formed XML, and where. The reason reaches
/// the agent in a refusal, so it names the fault by kind and byte position:
/// quick-xml's messages for tags and declarations quote names the document
/// chose, and the document is the source's text.
fn xml_fault(error: &quick_xml::Error, at: u64) -> String {
    use quick_xml::Error;
    use quick_xml::errors::IllFormedError;
    let what = match error {
        // Syntax faults are stated in quick-xml's own words, none quoted.
        Error::Syntax(syntax) => syntax.to_string(),
        Error::IllFormed(IllFormedError::MissingEndTag(_)) => "a start tag is not closed".into(),
        Error::IllFormed(IllFormedError::UnmatchedEndTag(_)) => {
            "an end tag matches no open tag".into()
        }
        Error::IllFormed(IllFormedError::MismatchedEndTag { .. }) => {
            "an end tag does not match its start tag".into()
        }
        Error::IllFormed(IllFormedError::MissingDeclVersion(_)) => {
            "the XML declaration does not begin with its version".into()
        }
        Error::IllFormed(_) => "the document is not well-formed".into(),
        Error::InvalidAttr(_) => "an attribute is not well-formed".into(),
        Error::Encoding(_) => "the document is not in a readable encoding".into(),
        Error::Escape(_) => "a character or entity reference is not valid".into(),
        Error::Namespace(_) => "a namespace is not valid".into(),
        Error::Io(_) => "the document could not be read".into(),
    };
    format!("not well-formed XML at byte {at}: {what}")
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
            .map_err(|error| xml_fault(&error, reader.error_position()))?;
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
                    "amount" | "standard" | "custom" | "accepts" if payment.is_some() => {
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
                let value = bytes.decode().map_err(|_| {
                    format!(
                        "text is not decodable, at byte {}",
                        reader.buffer_position()
                    )
                })?;
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
                    "standard" | "custom" | "accepts" => {
                        if let (Some(_), Some(payment)) = (collecting.take(), payment.as_mut()) {
                            let value = Some(text.trim().to_owned());
                            match name.as_str() {
                                "standard" => payment.standard = value,
                                "custom" => payment.custom = value,
                                _ => payment.accepts = value,
                            }
                        }
                    }
                    "payment" => {
                        if let (Some(done), Some(licence)) = (payment.take(), licence.as_mut()) {
                            licence.payments.push(done);
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
    let Some(licence) = licence.as_mut() else {
        return;
    };
    // User and geo terms are not usage tokens. The edge cannot show which
    // class of user it acts for or where, so such a term marks the licence
    // as one it cannot take.
    if matches!(kind.as_deref(), Some("user" | "geo")) {
        licence.restricted = true;
        return;
    }
    if kind.as_deref() != Some("usage") {
        return;
    }
    let usage = if element == "permits" {
        &mut licence.permits_usage
    } else {
        &mut licence.prohibits_usage
    };
    usage
        .get_or_insert_with(Vec::new)
        .extend(text.split_whitespace().map(str::to_owned));
}

impl RslDocument {
    /// The `<content>` entry governing one page (RSL sections 3.1.1 and
    /// 4.9). A path pattern is matched against the page's path under RFC
    /// 9309 rules ([`matches_pattern`]); an absolute URL is matched as a
    /// prefix of the page URL, both in
    /// [`commonmeasure_types::matching_url`]'s form; an empty `url` names the
    /// scope the discovery mechanism established. Host case, trailing dots,
    /// default ports, credentials, percent-encoded unreserved characters,
    /// lower-case hex, a literal `*` or `$` and fragments do not take a page
    /// out of its entry.
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
    /// Of the entries that match, the narrowest govern, as RSL 1.0 §3.1.1
    /// gives the more specific declaration precedence: each matching entry
    /// whose scope strictly contains another matching entry's scope is set
    /// aside, and the entries left govern together. So `/ab` governs alone
    /// over `/a*b` for `/abc`, `/p` over `/*` for `/page` and `/p*` over
    /// `/*p` for `/p`. The same scope written twice, in two spellings or
    /// with and without a trailing `*` (`/n` and `/%6E`, `/` and `/*`)
    /// governs together, and so do scopes neither of which lies within the
    /// other (`/xy` and `/x*z` for `/xyz`). The length of a pattern plays no
    /// part. Whether one scope lies within another is decided by the
    /// matcher ([`Scope::within`]).
    ///
    /// An absolute scope the page is under takes part by its path and
    /// query, since the host has already matched: its scope is the literal
    /// prefix its request target names in the matching form. So
    /// `https://host/` and `/` are one scope and govern together, and
    /// `https://host/a` lies within `/*`. Of two absolute scopes of the same
    /// path in different spellings, the one matching the page as written is
    /// set aside for neither; one that does not, beside one that does, is
    /// set aside, then the shorter written beside the longer. That is the
    /// order such scopes had when they were matched only as written, so the
    /// order of the two in the document cannot change which governs. An
    /// empty `url` and an absolute scope that does not parse govern only
    /// where no other entry matches: the scope that does not parse, which
    /// matches the page as written, over the empty `url`, the longer
    /// written over the shorter, and two of one kind and written length
    /// together.
    ///
    /// Entries that govern together are read as one entry. RSL 1.0 §3.1
    /// says the order of elements must not affect their interpretation, so
    /// neither governs alone: the combined entry holds their licences in
    /// document order, numbered from 1 across them, and the first licence
    /// server any of them names, and its `url` is the first's. Its licences
    /// are then ruled as [`governing_offer`] and [`licence_terms`] rule one
    /// entry's licences, so a prohibition of AI input in any of them refuses
    /// the page.
    ///
    /// The containment tests are bounded by the work they do
    /// ([`SELECTION_BUDGET`]), counted before any test is made over every
    /// entry that matches the reading, absolute scopes included. Past the
    /// budget nothing is selected and the result is
    /// [`SelectionUnread::OverBudget`]; where entries match and none is left
    /// governing, it is [`SelectionUnread::NoneGoverns`]. Either leaves the
    /// licence unread for the page, and no weaker selection is made in its
    /// place. The readings before it are tried as usual; the one that is
    /// unread ends the search.
    pub fn content_for(
        &self,
        page_url: &str,
    ) -> Result<Option<Cow<'_, RslContent>>, SelectionUnread> {
        #[cfg(test)]
        SELECTIONS.with(|selections| selections.set(selections.get() + 1));
        let target = request_target(page_url);
        let targets = commonmeasure_types::matching_target_readings(&target)
            .unwrap_or_else(|_| vec![commonmeasure_types::matching_target(&target)]);
        // Both lists read the same target, so they align reading by reading.
        let pages = url::Url::parse(page_url).ok().map(|page| {
            commonmeasure_types::matching_url_readings(&page)
                .unwrap_or_else(|_| vec![commonmeasure_types::matching_url(&page)])
        });
        targets
            .iter()
            .enumerate()
            .find_map(|(index, target)| {
                let page = pages
                    .as_ref()
                    .and_then(|pages| pages.get(index))
                    .map(String::as_str);
                self.content_for_reading(target, page, page_url).transpose()
            })
            .transpose()
    }

    /// The entry one reading of the page selects, the entries that govern
    /// together combined: `target` and `page` are its request target and
    /// URL in the matching form.
    fn content_for_reading(
        &self,
        target: &str,
        page: Option<&str>,
        page_url: &str,
    ) -> Result<Option<Cow<'_, RslContent>>, SelectionUnread> {
        // The matching entries in document order: those with a scope the
        // containment tests read, and those with none, an empty `url` or an
        // absolute scope that does not parse, with their order among them.
        let mut scoped: Vec<ScopedEntry<'_>> = Vec::new();
        let mut unscoped: Vec<(&RslContent, (bool, usize))> = Vec::new();
        for content in &self.contents {
            let pattern = content.url.as_str();
            if pattern.is_empty() {
                unscoped.push((content, (false, 0)));
            } else if pattern.starts_with('/') {
                let form = commonmeasure_types::matching_pattern(pattern);
                if matches_form(&form, target) {
                    scoped.push((content, form, None));
                }
            } else {
                match absolute_scope(pattern, page, page_url) {
                    Some(Some(form)) => {
                        let order = (page_url.starts_with(pattern), pattern.len());
                        scoped.push((content, form, Some(order)));
                    }
                    Some(None) => unscoped.push((content, (true, pattern.len()))),
                    None => {}
                }
            }
        }
        let selected: Vec<&RslContent> = if scoped.is_empty() {
            let Some(first) = unscoped.iter().map(|(_, order)| *order).max() else {
                return Ok(None);
            };
            unscoped
                .iter()
                .filter(|(_, order)| *order == first)
                .map(|(content, _)| *content)
                .collect()
        } else {
            let governing = governing_forms(&scoped)?;
            // Of the absolute scopes of one form, those first in
            // written-form order govern.
            let mut first_absolute: BTreeMap<&str, (bool, usize)> = BTreeMap::new();
            for (_, form, order) in &scoped {
                if let Some(order) = order {
                    let first = first_absolute.entry(form.as_str()).or_insert(*order);
                    *first = (*first).max(*order);
                }
            }
            scoped
                .iter()
                .filter(|(_, form, order)| {
                    governing.contains(form.as_str())
                        && order
                            .is_none_or(|order| first_absolute.get(form.as_str()) == Some(&order))
                })
                .map(|(content, ..)| *content)
                .collect()
        };
        let Some((first, rest)) = selected.split_first() else {
            return Err(SelectionUnread::NoneGoverns);
        };
        if rest.is_empty() {
            return Ok(Some(Cow::Borrowed(*first)));
        }
        let mut combined = (*first).clone();
        for content in rest {
            combined.server = combined.server.take().or_else(|| content.server.clone());
            combined.licences.extend(content.licences.iter().cloned());
        }
        Ok(Some(Cow::Owned(combined)))
    }
}

/// A matching entry with a scope the containment tests read: the entry, its
/// scope's form, and an absolute scope's written-form order (whether it
/// matches the page as written, and its written length).
type ScopedEntry<'a> = (&'a RslContent, String, Option<(bool, usize)>);

/// The forms of the matching scopes that govern: those that strictly
/// contain no other matching scope. Every form is tested against every
/// other once, after the work that takes has been counted against
/// [`SELECTION_BUDGET`].
fn governing_forms<'a>(
    scoped: &'a [ScopedEntry<'_>],
) -> Result<std::collections::BTreeSet<&'a str>, SelectionUnread> {
    let mut forms: Vec<&str> = scoped.iter().map(|(_, form, _)| form.as_str()).collect();
    forms.sort_unstable();
    forms.dedup();
    let scopes: Vec<Scope> = forms.into_iter().map(Scope::new).collect();
    if selection_work(scoped, &scopes) > SELECTION_BUDGET {
        return Err(SelectionUnread::OverBudget);
    }
    let count = scopes.len();
    // `within[inner * count + outer]`: whether scope `inner` lies within
    // scope `outer`.
    let mut within = vec![true; count * count];
    for (inner, scope) in scopes.iter().enumerate() {
        for (outer, other) in scopes.iter().enumerate() {
            if inner != outer {
                within[inner * count + outer] = scope.within(other);
            }
        }
    }
    let strictly_within = |inner: usize, outer: usize| {
        within[inner * count + outer] && !within[outer * count + inner]
    };
    let governing: std::collections::BTreeSet<&str> = (0..count)
        .filter(|&outer| !(0..count).any(|inner| strictly_within(inner, outer)))
        .map(|index| scopes[index].form)
        .collect();
    if governing.is_empty() {
        return Err(SelectionUnread::NoneGoverns);
    }
    Ok(governing)
}

/// The work selection does among `scoped`, the matching entries, whose
/// distinct forms are `scopes`: each entry's form read once, and for each
/// ordered pair of distinct scopes the octets one containment test compares
/// (the inner scope's general target and the outer scope's form) plus
/// [`TEST_WORK`]. Counted in full before any test is made, so the answer
/// does not depend on where a test stops early.
fn selection_work(scoped: &[ScopedEntry<'_>], scopes: &[Scope]) -> u64 {
    let octets = |count: usize| u64::try_from(count).unwrap_or(u64::MAX);
    let read: u64 = scoped
        .iter()
        .map(|(_, form, _)| octets(form.len()))
        .fold(0, u64::saturating_add);
    let others = octets(scopes.len().saturating_sub(1));
    let compared: u64 = scopes
        .iter()
        .map(|scope| octets(scope.general.len() + scope.form.len()))
        .fold(0, u64::saturating_add);
    let tests = octets(scopes.len()).saturating_mul(others);
    read.saturating_add(others.saturating_mul(compared))
        .saturating_add(tests.saturating_mul(TEST_WORK))
}

/// The most work one selection of a `<content>` entry may do, in the unit
/// [`selection_work`] counts. The publisher writes the document, and within
/// the licence body cap the containment tests among its matching entries
/// could otherwise hold a crossing for seconds on every redirect hop to a
/// host that names it. Past the budget the licence is unread, which refuses
/// in every policy mode; selecting more weakly instead, as every matching
/// entry read as one or the first or last alone, could deliver a page the
/// full selection refuses. Entries that do not match the page do no work
/// here, so a long licence of which a page matches a few entries is read.
pub const SELECTION_BUDGET: u64 = 64_000_000;

/// What one containment test costs beyond the octets it compares, in the
/// same unit: splitting the outer form and setting up the match.
const TEST_WORK: u64 = 64;

/// Why a licence whose document parsed is unread for a page: selecting its
/// `<content>` entry did not finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionUnread {
    /// The containment tests among the entries matching the page would do
    /// more work than [`SELECTION_BUDGET`].
    OverBudget,
    /// Entries match the page and every one strictly contains another, so
    /// none governs. A strict containment order always leaves one; this is
    /// the answer where the test could not be shown to give one.
    NoneGoverns,
}

impl SelectionUnread {
    /// Why the licence is unread, as the record and the agent read it. It
    /// holds this edge's own words and nothing from the document.
    pub fn reason(self) -> AgentText {
        match self {
            Self::OverBudget => AgentText::fixed(
                "choosing among the <content> entries that match this page takes more work than \
                 this edge does for one page",
            ),
            Self::NoneGoverns => AgentText::fixed(
                "<content> entries match this page and none of them could be ruled the narrowest",
            ),
        }
    }
}

impl std::fmt::Display for SelectionUnread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason().as_str())
    }
}

#[cfg(test)]
thread_local! {
    /// How many times [`RslDocument::content_for`] ran on this thread, which
    /// tests read to count the selections a crossing makes.
    pub(crate) static SELECTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The one character a stand-in target puts where a pattern's `*` or open
/// end matches anything. [`commonmeasure_types::matching_pattern`] never
/// emits it: every literal octet of a form is unreserved, reserved but not
/// `*` or `$`, or a `%XX` encoding, and [`commonmeasure_types::matching_target`]
/// encodes a space as `%20`. So no literal part of a form can match it, and
/// only a `*`, or the open end of an unanchored form, can take it in.
const ANY_RUN: char = ' ';

/// A scope a containment test reads: its matching form, and the most
/// general target the form matches, made once so that each test is one
/// match. A relative entry's form is its pattern's; an absolute scope's is
/// its request target's, a literal prefix.
struct Scope<'a> {
    form: &'a str,
    general: String,
}

impl<'a> Scope<'a> {
    /// The general target is the form with each `*` read as [`ANY_RUN`], a
    /// final `$` removed, and one more [`ANY_RUN`] at the end when there is
    /// no final `$`. A literal `%2A` or `%24` is three literal octets and
    /// stays as it is.
    fn new(form: &'a str) -> Self {
        let (literal, anchored) = match form.strip_suffix('$') {
            Some(literal) => (literal, true),
            None => (form, false),
        };
        let mut general = literal.replace('*', &ANY_RUN.to_string());
        if !anchored {
            general.push(ANY_RUN);
        }
        Self { form, general }
    }

    /// Whether every target this scope matches, `outer` matches: whether
    /// `outer` matches this scope's general target, as [`matches_form`]
    /// matches a page. Selection sets an entry aside only on this test.
    ///
    /// Sound: `outer`'s literal parts cannot match [`ANY_RUN`], so where
    /// `outer` matches the general target they fall within this scope's
    /// literal parts and `outer`'s wildcards take in this scope's; any
    /// target this scope matches is then matched by `outer` the same way.
    /// Complete where `outer` lacks some character a target can carry
    /// literally: the general target with [`ANY_RUN`] read as that
    /// character is a target this scope matches and `outer` does not. For
    /// an `outer` that holds every such character, some 80 octets or more,
    /// completeness is not proven: a containment missed there would read as
    /// one two entries of which one lies within the other, or set aside one
    /// of two entries of the same scope, and where it left no entry
    /// governing the licence would be unread
    /// ([`SelectionUnread::NoneGoverns`]).
    fn within(&self, outer: &Scope) -> bool {
        matches_form(outer.form, &self.general)
    }

    /// Whether the two scopes match exactly the same targets.
    #[cfg(test)]
    fn same_scope(&self, other: &Scope) -> bool {
        self.within(other) && other.within(self)
    }
}

/// Where the page is under an absolute `<content>` scope: `Some` with the
/// form of the path and query the scope constrains, in the matching form,
/// for a scope that parses, and `Some(None)` for one that does not, which is
/// compared as written; `None` where the page is not under it. `page` is one
/// reading of the page in the matching form. A trailing dot, a change of
/// host case, an explicit default port, credentials, a percent-encoded
/// unreserved character or a fragment in either URL names the same
/// resource, so none of them may take a page out of its licence and the
/// licence's prohibitions and reporting demands out of the ruling. The
/// scope's own slashes are compared as written. A scope that does not
/// parse names no path, so it takes no part in the containment tests.
fn absolute_scope(pattern: &str, page: Option<&str>, page_url: &str) -> Option<Option<String>> {
    match url::Url::parse(pattern) {
        Ok(scope) => {
            let scope = commonmeasure_types::matching_url(&scope);
            let within = match page {
                Some(page) => page.starts_with(&scope),
                None => page_url.starts_with(pattern),
            };
            within.then(|| {
                Some(commonmeasure_types::matching_target(&request_target(
                    &scope,
                )))
            })
        }
        Err(_) => page_url.starts_with(pattern).then_some(None),
    }
}

/// The terms an RSL content entry states for AI input, evaluated across its
/// licences: each licence is one offer, so a category any licence permits is
/// allowed, one every applicable licence refuses is disallowed, and one no
/// licence mentions is unknown.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LicenceTerms {
    pub statements: Vec<Statement>,
    /// The payment term of the offer AI input is taken under
    /// ([`governing_offer`]), where there is one and it states a payment:
    /// [`RslLicence::payment`], the first of its `<payment>` elements that
    /// needs settlement, else the first. Where no licence authorises AI
    /// input, the payment term of the first restricted licence
    /// ([`RslLicence::restricted`]) whose payment needs settlement, which
    /// is recorded and not ruled: that licence gives a Disallow instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment: Option<RslPayment>,
    /// Every `<payment>` element of the licence `payment` is read from, in
    /// document order, where it has more than one; `payment` is the one
    /// ruled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payments: Vec<RslPayment>,
    /// Which `<license>` of the content entry that offer is, counted from 1
    /// in document order. Absent where no licence in the entry authorises AI
    /// input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<usize>,
    /// The licences of the entry, counted from 1, restricted to a class of
    /// user or a region ([`RslLicence::restricted`]), which authorise
    /// nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restricted: Vec<usize>,
    /// The reporting demands that bind an AI-input crossing
    /// ([`reporting_demands`]), whatever the licences say of AI input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reporting: Vec<RslReporting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
}

/// How a licence authorises AI input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AiInputGrant {
    /// Its `<permits type="usage">` covers AI input, and it does not
    /// prohibit AI input ([`prohibits_ai_input`]).
    ByName,
    /// It has no usage `<permits>`, and no licence of the content entry
    /// names AI input in a usage `<permits>` or `<prohibits>` (`ai-input`,
    /// `ai-all` or `all`). RSL 1.0 §3.5 restricts usage only where a
    /// `<permits>` of that type exists, so such a licence covers every
    /// usage on its own payment and reporting terms (§3.4 and the grammar
    /// make every child of `<license>` optional). It is the least specific
    /// statement about AI input a document can make, and §3.1.1 gives the
    /// more specific declaration precedence and reads licences
    /// conservatively, so a sibling that names AI input decides instead.
    Silent,
}

/// Whether a licence names AI input in a usage `<permits>` or
/// `<prohibits>`, by any token that covers it.
fn names_ai_input(licence: &RslLicence) -> bool {
    [&licence.permits_usage, &licence.prohibits_usage]
        .into_iter()
        .flatten()
        .any(|tokens| token_specificity(tokens, Category::AiInput).is_some())
}

/// Whether `licence` prohibits AI input: a usage `<prohibits>` covers it
/// and the licence's own usage `<permits>` does not name it more
/// specifically. Within one licence RSL 1.0 §3.1.1 gives the more specific
/// term precedence, so `permits ai-input` beside `prohibits all` (which
/// restates that unlisted uses are not licensed) does not prohibit, while
/// `prohibits ai-input` or `prohibits ai-all` beside `permits ai-input`
/// does. A licence restricted to a class of user or a region that
/// prohibits AI input prohibits it here too: the edge cannot show it is
/// outside the class or the region.
///
/// One such licence in the entry that governs a page refuses AI input
/// whatever any other licence of the entry permits, by name or by silence,
/// free or priced, and in either document order (owner decision, 3 October
/// 2026; §3.1.1, "the prohibition MUST take precedence"). [`governing_offer`]
/// then takes no offer and [`licence_terms`] states a Disallow naming it.
fn prohibits_ai_input(licence: &RslLicence) -> bool {
    let category = Category::AiInput;
    let Some(prohibit) = licence
        .prohibits_usage
        .as_deref()
        .and_then(|tokens| token_specificity(tokens, category))
    else {
        return false;
    };
    licence
        .permits_usage
        .as_deref()
        .and_then(|tokens| token_specificity(tokens, category))
        .is_none_or(|permit| prohibit >= permit)
}

/// How `licence` authorises AI input, if it does, read on its own: whether
/// another licence of the entry prohibits AI input is [`governing_offer`]'s
/// to rule. `silent_counts` is false where any licence of its content entry
/// names AI input ([`names_ai_input`]). A licence restricted to a class of
/// user or a region authorises nothing: the edge cannot show it meets the
/// restriction. A by-name permit stands where the licence does not itself
/// prohibit AI input ([`prohibits_ai_input`]), so a specific permit stands
/// over a blanket `prohibits all` in the same licence.
fn ai_input_grant(licence: &RslLicence, silent_counts: bool) -> Option<AiInputGrant> {
    if licence.restricted {
        return None;
    }
    match licence.permits_usage.as_deref() {
        Some(tokens) => token_specificity(tokens, Category::AiInput)
            .filter(|_| !prohibits_ai_input(licence))
            .map(|_| AiInputGrant::ByName),
        // A licence whose own `<prohibits>` covers AI input names it, so
        // `silent_counts` is false for it too.
        None => silent_counts.then_some(AiInputGrant::Silent),
    }
}

/// The licence of a content entry an AI-input crossing is taken under, with
/// its index in the entry: the one notion of the governing licence that the
/// payment term, the AI-input statement and the reporting demands all read.
///
/// Where any licence of the entry prohibits AI input
/// ([`prohibits_ai_input`]), there is no offer: a prohibition takes
/// precedence over a permit in another licence of the entry, or in another
/// `<content>` entry read with it ([`RslDocument::content_for`]), whatever
/// that permit's payment and whatever the document order.
///
/// Otherwise each `<license>` is an offer (RSL 1.0 §3.4: distinct term
/// sets), and the order they are written in does not change what they mean
/// (§3.1.1). The edge may take any offer whose terms it can meet, so among
/// the licences that authorise AI input ([`ai_input_grant`]), one whose
/// payment term needs no settlement ([`RslPayment::needs_settlement`], over
/// every `<payment>` it has) comes first, then document order. Where every
/// offer needs settlement, the first is the one named in the refusal. A
/// licence silent on usage is an offer only where no licence of the entry
/// names AI input, so it never competes with one that permits AI input by
/// name. The choice does not read reporting demands: of two free offers
/// that differ only in a demand this edge cannot meet on its route, the
/// first in document order is taken, so the order decides whether the page
/// is delivered (recorded in session evidence §Source declarations).
fn governing_offer(content: &RslContent) -> Option<(usize, &RslLicence, AiInputGrant)> {
    if content.licences.iter().any(prohibits_ai_input) {
        return None;
    }
    let silent_counts = !content.licences.iter().any(names_ai_input);
    content
        .licences
        .iter()
        .enumerate()
        .filter_map(|(index, licence)| {
            ai_input_grant(licence, silent_counts).map(|grant| (index, licence, grant))
        })
        .min_by_key(|(index, licence, _)| {
            let unmet = licence.payments.iter().any(RslPayment::needs_settlement);
            (unmet, *index)
        })
}

pub fn licence_terms(content: &RslContent, licence_url: &str) -> LicenceTerms {
    let mut terms = LicenceTerms {
        server: content.server.clone(),
        restricted: content
            .licences
            .iter()
            .enumerate()
            .filter(|(_, licence)| licence.restricted)
            .map(|(index, _)| index + 1)
            .collect(),
        ..LicenceTerms::default()
    };
    let offer = governing_offer(content);
    for category in Category::ALL {
        if category == Category::AiInput
            && let Some((index, licence, grant)) = offer
        {
            terms.statements.push(Statement {
                source: StatementSource::RslLicence,
                category,
                preference: Preference::Allow,
                detail: match grant {
                    AiInputGrant::ByName => {
                        format!("{licence_url}: permits usage {}", category.label())
                    }
                    AiInputGrant::Silent => format!(
                        "{licence_url}: licence {} lists no usage it permits, so covers usage {}",
                        index + 1,
                        category.label()
                    ),
                },
            });
            terms.payment = licence.payment().cloned();
            if licence.payments.len() > 1 {
                terms.payments = licence.payments.clone();
            }
            terms.offer = Some(index + 1);
            continue;
        }
        // Every other category, and AI input where no offer authorises it:
        // a licence silent on usage restricts nothing, so it adds no
        // statement here.
        let mut allowed = false;
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
                // A restricted licence permits the category only to a class
                // the edge cannot claim, so for this edge it permits none.
                (Some(_), _) if licence.restricted => disallowed_by.push(index),
                (Some(_), _) => allowed = true,
                (None, Some(_)) => disallowed_by.push(index),
                // Permits are listed and this category is not among them.
                (None, None) if licence.permits_usage.is_some() => disallowed_by.push(index),
                // A restricted licence silent on usage that asks for payment
                // covers AI input only on that payment and only for its
                // class. Inside the class the price binds and this edge
                // cannot pay; outside it nothing grants the page. Neither
                // admits AI input, so for this edge it permits none.
                (None, None)
                    if category == Category::AiInput
                        && licence.restricted
                        && licence.payments.iter().any(RslPayment::needs_settlement) =>
                {
                    disallowed_by.push(index)
                }
                (None, None) => {}
            }
        }
        // A prohibition of AI input in any licence of the entry takes
        // precedence over another licence's permit ([`prohibits_ai_input`]).
        let prohibiting: Vec<usize> = if category == Category::AiInput {
            content
                .licences
                .iter()
                .enumerate()
                .filter(|(_, licence)| prohibits_ai_input(licence))
                .map(|(index, _)| index)
                .collect()
        } else {
            Vec::new()
        };
        if allowed && !prohibiting.is_empty() {
            let numbered = |indices: Vec<usize>| {
                indices
                    .iter()
                    .map(|index| (index + 1).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let permitting: Vec<usize> = content
                .licences
                .iter()
                .enumerate()
                .filter(|(index, _)| !disallowed_by.contains(index))
                .filter(|(_, licence)| {
                    licence
                        .permits_usage
                        .as_deref()
                        .and_then(|tokens| token_specificity(tokens, category))
                        .is_some()
                })
                .map(|(index, _)| index)
                .collect();
            terms.statements.push(Statement {
                source: StatementSource::RslLicence,
                category,
                preference: Preference::Disallow,
                detail: format!(
                    "{licence_url}: licence {} prohibits usage {}, which takes precedence over \
                     the permit of licence {}",
                    numbered(prohibiting),
                    category.label(),
                    numbered(permitting)
                ),
            });
        } else if allowed {
            terms.statements.push(Statement {
                source: StatementSource::RslLicence,
                category,
                preference: Preference::Allow,
                detail: format!("{licence_url}: permits usage {}", category.label()),
            });
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
    // With no offer, a restricted licence that asks for payment is refused
    // on the Disallow above. Its price stays on the record, so the record
    // says what the source asked for and not only that it refused.
    if offer.is_none()
        && let Some(licence) = content.licences.iter().find(|licence| {
            licence.restricted && licence.payments.iter().any(RslPayment::needs_settlement)
        })
    {
        terms.payment = licence.payment().cloned();
        if licence.payments.len() > 1 {
            terms.payments = licence.payments.clone();
        }
    }
    terms.reporting = reporting_demands(content, offer.map(|(_, licence, _)| licence));
    terms
}

/// The reporting demands that bind an AI-input crossing of the page this
/// content entry governs (RSL section 3.12: a client satisfies every
/// applicable demand, or treats the activity as not licensed).
///
/// A demand applies to activity the enclosing licence authorises, so the
/// demands of the offer the crossing is taken under apply: the same licence
/// whose payment term is read, chosen once by [`governing_offer`]. Where no
/// licence authorises AI input, every demand in the entry applies, whatever
/// else refuses the crossing: the page is still the owner's, and taking it
/// outside the licence does not excuse the fetcher from the report the owner
/// asks for (owner decision, 22 September 2026).
fn reporting_demands(content: &RslContent, offer: Option<&RslLicence>) -> Vec<RslReporting> {
    match offer {
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

/// What a response's `Link` fields say about an RSL licence (RSL section 4.5).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LinkLicences {
    /// The first `rel="license"` link typed `application/rsl+xml`, resolved
    /// against the page URL.
    pub named: Option<String>,
    /// Members that name, or may name, an RSL licence and could not be read.
    /// Each is an unread licence: its terms are unknown, not absent.
    pub unread: Vec<UnreadLink>,
}

/// A `Link` member that names, or may name, an RSL licence and could not be
/// read.
#[derive(Debug, PartialEq, Eq)]
pub struct UnreadLink {
    /// The member's target as written, or the whole member where it could
    /// not be parsed, since a target read before the fault may not be the
    /// licence's; rendered where it is not UTF-8
    /// ([`commonmeasure_http::render_value`]). For a second RSL licence,
    /// which was read, the URL it resolves to.
    pub written: String,
    /// True where `written` is a rendering, so ASCII text that reads `\xE9`
    /// and the byte 0xE9 are told apart.
    pub rendered: bool,
    /// Why it could not be read.
    pub why: String,
}

/// Read every `Link` field of a response for RSL licences, from its bytes.
///
/// A link's target and its `rel` and `type` parameters are ASCII by grammar
/// (RFC 8288 section 3), and those are all this reads; a byte that is not
/// UTF-8 elsewhere, in a `title` or an extension, is not read and stops
/// nothing. A member that names a licence but whose target, `rel` or `type`
/// cannot be read, and a member that cannot be parsed and mentions
/// `license` at all, are unread licences rather than no licence: a licence
/// the source may have named is never taken as absent because this edge
/// could not read it. A member that cannot be parsed ends at the next comma
/// outside a quoted string or `<…>`, so it takes no other member with it
/// except where a quote or `<` is left open.
///
/// `rel*` and `type*` are read as `rel` and `type` once their RFC 8187
/// encoding is decoded. Where a member's `rel` and `rel*`, or its `type`
/// and `type*`, disagree about whether it names an RSL licence, the member
/// is an unread licence: RFC 8288 Appendix B.2 step 16 gives the starred
/// value precedence, a parser that does not apply it reads the plain one,
/// and the edge cannot know which reading the publisher meant. The first readable RSL
/// licence is `named`; a second distinct one is unread, since this edge
/// reads one licence per page and a page is never admitted under one
/// licence while another it names goes unread. Two members naming the same
/// URL name one licence.
pub fn rsl_links<'a>(fields: impl IntoIterator<Item = &'a [u8]>, page_url: &str) -> LinkLicences {
    let mut found = LinkLicences::default();
    for field in fields {
        for member in link_members(field) {
            let why = match (read_member(&member, page_url), &found.named) {
                (MemberReading::NoLicence, _) => continue,
                (MemberReading::Named(url), None) => {
                    found.named = Some(url);
                    continue;
                }
                (MemberReading::Named(url), Some(first)) if *first == url => continue,
                // A second licence has a URL that was read; it is recorded
                // by that URL.
                (MemberReading::Named(url), Some(_)) => {
                    found.unread.push(UnreadLink {
                        written: url,
                        rendered: false,
                        why: "the response names another RSL licence before this one, and this \
                              edge reads one licence per page, so this licence's terms are \
                              unknown"
                            .to_owned(),
                    });
                    continue;
                }
                (MemberReading::Unread(why), _) => why,
            };
            let written = match member.malformed {
                Some(_) => member.raw,
                None => member.target.unwrap_or(member.raw),
            };
            let (written, rendered) = commonmeasure_http::render_value(written);
            found.unread.push(UnreadLink {
                written: written.into_owned(),
                rendered,
                why,
            });
        }
    }
    found
}

/// One comma-separated member of a `Link` field.
struct LinkMember<'a> {
    /// The member's bytes, from its first byte to the end of its last
    /// parameter.
    raw: &'a [u8],
    /// Between `<` and `>`, where the member has them.
    target: Option<&'a [u8]>,
    /// The first `rel` and `type` parameters' values, unquoted; later ones
    /// are ignored (RFC 8288 section 3.3).
    rel: Option<Param>,
    media_type: Option<Param>,
    /// The first `rel*` and `type*` values, decoded (RFC 8187), kept apart
    /// from `rel` and `type` so that a member on which they disagree is
    /// known (RFC 8288 Appendix B.2 step 16).
    rel_star: Option<Param>,
    media_type_star: Option<Param>,
    /// True where a parameter's name is not a token, and so could be a
    /// `rel` or `type` this edge cannot recognise.
    odd_name: bool,
    /// Why the member could not be parsed, where it could not.
    malformed: Option<&'static str>,
}

/// A `rel` or `type` value.
enum Param {
    Read(Vec<u8>),
    /// A `rel*` or `type*` whose RFC 8187 encoding does not decode, kept as
    /// the bytes its valid percent-escapes decode to, so that what it
    /// mentions can still be tested.
    Undecoded(Vec<u8>),
}

impl Param {
    fn bytes(&self) -> &[u8] {
        match self {
            Param::Read(bytes) | Param::Undecoded(bytes) => bytes,
        }
    }
}

impl LinkMember<'_> {
    /// Whether the member mentions `license` anywhere: in its bytes with
    /// quoted-pair escapes removed, so an escaped `lic\ense` in a member
    /// that cannot be parsed counts, or in a `rel` or `type` as decoded.
    fn mentions_license(&self) -> bool {
        const LICENSE: &[u8] = b"license";
        let unescaped: Vec<u8> = self.raw.iter().copied().filter(|&b| b != b'\\').collect();
        contains_ignoring_case(&unescaped, LICENSE)
            || [
                &self.rel,
                &self.media_type,
                &self.rel_star,
                &self.media_type_star,
            ]
            .into_iter()
            .flatten()
            .any(|param| contains_ignoring_case(param.bytes(), LICENSE))
    }
}

enum MemberReading {
    NoLicence,
    Named(String),
    Unread(String),
}

fn read_member(member: &LinkMember<'_>, page_url: &str) -> MemberReading {
    if let Some(why) = member.malformed {
        return if member.mentions_license() {
            MemberReading::Unread(format!(
                "a Link member that mentions `license` could not be parsed: {why}"
            ))
        } else {
            MemberReading::NoLicence
        };
    }
    // A name that is not a token is ignored, as a parameter this edge does
    // not use is, unless the `rel` or `type` it may be is then missing
    // from a member that mentions a licence.
    if member.odd_name
        && ((member.rel.is_none() && member.rel_star.is_none())
            || (member.media_type.is_none() && member.media_type_star.is_none()))
        && member.mentions_license()
    {
        return MemberReading::Unread(
            "a Link member that mentions `license` has a parameter name that is not a token, \
             and it may be the member's rel or type"
                .to_owned(),
        );
    }
    // The member as read with each of `rel` and `rel*`, and each of `type`
    // and `type*`, that it has. RFC 8288 Appendix B.2 step 16 has a
    // starred parameter replace the plain one, while a parser that does not
    // apply it reads the plain one, so where the readings disagree about
    // whether the member names an RSL licence neither can be taken, and the
    // licence the member may name is unread (EDG-115).
    let mut readings = Vec::new();
    for rel in either(&member.rel, &member.rel_star) {
        for media_type in either(&member.media_type, &member.media_type_star) {
            readings.push(read_as(member, rel, media_type, page_url));
        }
    }
    let names = |reading: &MemberReading| !matches!(reading, MemberReading::NoLicence);
    if readings.iter().all(names) || !readings.iter().any(names) {
        // Agreed: the first reading, `rel` and `type` as written plain,
        // gives the reason where it is unread.
        return readings.swap_remove(0);
    }
    MemberReading::Unread(
        "the Link member's rel and rel*, or its type and type*, disagree about whether it \
         names an RSL licence (RFC 8288 Appendix B.2 step 16 gives the starred one precedence, \
         and a parser that does not apply it reads the plain one), so whether it names one is \
         not known"
            .to_owned(),
    )
}

/// The values a member's plain and starred forms of one parameter give
/// it: both where it has both, else the one it has.
fn either<'a>(plain: &'a Option<Param>, star: &'a Option<Param>) -> Vec<Option<&'a Param>> {
    match (plain, star) {
        (Some(plain), Some(star)) => vec![Some(plain), Some(star)],
        (plain, star) => vec![plain.as_ref().or(star.as_ref())],
    }
}

/// What `member` names read with `rel` as its rel and `media_type` as its
/// type.
fn read_as(
    member: &LinkMember<'_>,
    rel: Option<&Param>,
    media_type: Option<&Param>,
    page_url: &str,
) -> MemberReading {
    const LICENSE: &[u8] = b"license";
    let rel_unread = match rel {
        None => return MemberReading::NoLicence,
        Some(Param::Undecoded(_)) if member.mentions_license() => {
            return MemberReading::Unread(
                "a Link member that mentions `license` has a rel* whose RFC 8187 encoding \
                 does not decode, so whether it names a licence is not known"
                    .to_owned(),
            );
        }
        Some(Param::Undecoded(_)) => return MemberReading::NoLicence,
        Some(Param::Read(rel)) => match std::str::from_utf8(rel) {
            Ok(rel)
                if rel
                    .split_whitespace()
                    .any(|r| r.eq_ignore_ascii_case("license")) =>
            {
                false
            }
            Ok(_) => return MemberReading::NoLicence,
            Err(_) if contains_ignoring_case(rel, LICENSE) => true,
            Err(_) => return MemberReading::NoLicence,
        },
    };
    // A licence link of another type is not an RSL licence, and one with no
    // type is not either; this edge reads RSL only.
    let media_type = match media_type {
        None => None,
        Some(Param::Read(media)) => Some(std::str::from_utf8(media)),
        Some(Param::Undecoded(_)) => {
            return MemberReading::Unread(
                "the Link member names a licence and its type* does not decode (RFC 8187), so \
                 whether it is an RSL licence is not known"
                    .to_owned(),
            );
        }
    };
    match media_type {
        None => return MemberReading::NoLicence,
        Some(Ok(media)) => {
            let essence = media.split(';').next().unwrap_or_default().trim();
            if !essence.eq_ignore_ascii_case("application/rsl+xml") {
                return MemberReading::NoLicence;
            }
        }
        Some(Err(_)) => {
            return MemberReading::Unread(
                "the Link member names a licence and its type holds bytes that are not UTF-8, \
                 so whether it is an RSL licence is not known"
                    .to_owned(),
            );
        }
    }
    if rel_unread {
        return MemberReading::Unread(
            "the Link member's rel mentions `license` and holds bytes that are not UTF-8, so \
             whether it names a licence is not known"
                .to_owned(),
        );
    }
    let Some(target) = member.target.map(std::str::from_utf8) else {
        return MemberReading::Unread(
            "the Link member names an RSL licence and has no target".to_owned(),
        );
    };
    let Ok(target) = target else {
        return MemberReading::Unread(
            "the Link member names an RSL licence and its target holds bytes that are not \
             UTF-8, so no URL is read from it"
                .to_owned(),
        );
    };
    match url::Url::parse(page_url).and_then(|base| base.join(target.trim())) {
        Ok(joined) => MemberReading::Named(joined.to_string()),
        Err(error) => MemberReading::Unread(format!(
            "the Link member names an RSL licence and its target is not a URL: {error}"
        )),
    }
}

fn contains_ignoring_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

fn is_ows(byte: u8) -> bool {
    byte == b' ' || byte == b'\t'
}

/// Split one `Link` field into its members (RFC 8288 section 3):
/// `<target>` then `;`-separated parameters, each a token name with an
/// optional token or quoted-string value. A name runs to the next `=`, `;`,
/// `,` or whitespace and an unquoted value to the next `;` or `,`, as a
/// lenient reader of real headers takes them.
fn link_members(field: &[u8]) -> Vec<LinkMember<'_>> {
    let mut members = Vec::new();
    let mut at = 0;
    loop {
        while at < field.len() && (is_ows(field[at]) || field[at] == b',') {
            at += 1;
        }
        if at == field.len() {
            return members;
        }
        let start = at;
        let mut member = LinkMember {
            raw: &[],
            target: None,
            rel: None,
            media_type: None,
            rel_star: None,
            media_type_star: None,
            odd_name: false,
            malformed: None,
        };
        if let Err(why) = parse_member(field, &mut at, &mut member) {
            member.malformed = Some(why);
            at = next_member(field, start);
        }
        let mut end = at;
        while end > start && is_ows(field[end - 1]) {
            end -= 1;
        }
        member.raw = &field[start..end];
        members.push(member);
    }
}

fn parse_member<'a>(
    field: &'a [u8],
    at: &mut usize,
    member: &mut LinkMember<'a>,
) -> Result<(), &'static str> {
    let skip_ows = |at: &mut usize| {
        while *at < field.len() && is_ows(field[*at]) {
            *at += 1;
        }
    };
    if field[*at] != b'<' {
        return Err("it does not begin with a target in <…>");
    }
    let close = field[*at..]
        .iter()
        .position(|&byte| byte == b'>')
        .ok_or("its target has no closing >")?;
    member.target = Some(&field[*at + 1..*at + close]);
    *at += close + 1;
    loop {
        skip_ows(at);
        match field.get(*at) {
            None | Some(b',') => return Ok(()),
            Some(b';') => *at += 1,
            Some(_) => return Err("a parameter follows without a ;"),
        }
        skip_ows(at);
        let name_start = *at;
        while *at < field.len() && !matches!(field[*at], b'=' | b';' | b',' | b' ' | b'\t') {
            *at += 1;
        }
        let name = &field[name_start..*at];
        if name.is_empty() {
            match field.get(*at) {
                None | Some(b',' | b';') => continue,
                Some(_) => return Err("a parameter has no name"),
            }
        }
        member.odd_name |= !name.iter().all(|&byte| is_tchar(byte));
        skip_ows(at);
        let value = if field.get(*at) == Some(&b'=') {
            *at += 1;
            skip_ows(at);
            if field.get(*at) == Some(&b'"') {
                *at += 1;
                let mut value = Vec::new();
                loop {
                    match field.get(*at) {
                        None => return Err("a quoted value is not closed"),
                        Some(b'"') => {
                            *at += 1;
                            break;
                        }
                        Some(b'\\') => {
                            let escaped =
                                field.get(*at + 1).ok_or("a quoted value is not closed")?;
                            value.push(*escaped);
                            *at += 2;
                        }
                        Some(&byte) => {
                            value.push(byte);
                            *at += 1;
                        }
                    }
                }
                value
            } else {
                let value_start = *at;
                while *at < field.len() && !matches!(field[*at], b';' | b',') {
                    *at += 1;
                }
                field[value_start..*at].trim_ascii_end().to_vec()
            }
        } else {
            Vec::new()
        };
        // A name ending in `*` has an RFC 8187 encoded value (RFC 8288
        // section 3 and Appendix B.3 step 7.5). `rel*` and `type*` are kept
        // apart from `rel` and `type` (`read_member`).
        let (stem, starred) = match name.strip_suffix(b"*") {
            Some(stem) => (stem, true),
            None => (name, false),
        };
        let slot = if stem.eq_ignore_ascii_case(b"rel") {
            Some(if starred {
                &mut member.rel_star
            } else {
                &mut member.rel
            })
        } else if stem.eq_ignore_ascii_case(b"type") {
            Some(if starred {
                &mut member.media_type_star
            } else {
                &mut member.media_type
            })
        } else {
            None
        };
        if let Some(slot) = slot {
            slot.get_or_insert_with(|| {
                if starred {
                    decode_ext_value(&value)
                } else {
                    Param::Read(value)
                }
            });
        }
    }
}

/// Decode an RFC 8187 `ext-value`, `charset'[language]'value-chars`, in the
/// charsets it requires (UTF-8) and allows (ISO-8859-1). A value in another
/// charset, with a malformed escape, or with a byte that is not a visible
/// ASCII character is not decoded.
fn decode_ext_value(value: &[u8]) -> Param {
    let mut parts = value.splitn(3, |&byte| byte == b'\'');
    let (Some(charset), Some(_language), Some(encoded)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return Param::Undecoded(percent_decoded(value).0);
    };
    let (bytes, whole) = percent_decoded(encoded);
    if !whole || !encoded.iter().all(u8::is_ascii_graphic) {
        return Param::Undecoded(bytes);
    }
    if charset.eq_ignore_ascii_case(b"UTF-8") && std::str::from_utf8(&bytes).is_ok() {
        Param::Read(bytes)
    } else if charset.eq_ignore_ascii_case(b"ISO-8859-1") {
        Param::Read(
            bytes
                .iter()
                .map(|&byte| char::from(byte))
                .collect::<String>()
                .into_bytes(),
        )
    } else {
        Param::Undecoded(bytes)
    }
}

/// `value` with each `%` and two hex digits decoded, and whether every `%`
/// began one; a `%` that does not is kept as written.
fn percent_decoded(value: &[u8]) -> (Vec<u8>, bool) {
    let hex = |at: usize| {
        value
            .get(at)
            .and_then(|&byte| char::from(byte).to_digit(16))
    };
    let mut bytes = Vec::with_capacity(value.len());
    let mut whole = true;
    let mut at = 0;
    while at < value.len() {
        match (value[at], hex(at + 1), hex(at + 2)) {
            (b'%', Some(high), Some(low)) => {
                bytes.push((high * 16 + low) as u8);
                at += 3;
            }
            (byte, _, _) => {
                whole &= byte != b'%';
                bytes.push(byte);
                at += 1;
            }
        }
    }
    (bytes, whole)
}

/// Where the member after the one starting at `start` begins: the next comma
/// outside a quoted string and outside `<…>`, or the end of the field where a
/// quote or `<` is left open.
fn next_member(field: &[u8], start: usize) -> usize {
    let mut quoted = false;
    let mut angle = false;
    let mut at = start;
    while at < field.len() {
        match field[at] {
            b'\\' if quoted => at += 1,
            b'"' if !angle => quoted = !quoted,
            b'<' if !quoted => angle = true,
            b'>' if !quoted => angle = false,
            b',' if !quoted && !angle => return at,
            _ => {}
        }
        at += 1;
    }
    field.len()
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
    /// tie and are read as one entry in either order: the first's `url`,
    /// and the licences of both in document order.
    #[test]
    fn two_spellings_of_one_relative_scope_tie_and_are_read_as_one_entry() {
        let entry = |url: &str, rule: &str| {
            format!(
                r#"<content url="{url}"><license><{rule} type="usage">ai-input</{rule}></license></content>"#
            )
        };
        for (first, second) in [("/news/", "/%6Eews/"), ("/%6Eews/", "/news/")] {
            let document = parse_rsl(&format!(
                r#"<rsl xmlns="https://rslstandard.org/rsl">{}{}</rsl>"#,
                entry(first, "permits"),
                entry(second, "prohibits")
            ))
            .expect("parses");
            for page in ["https://example.com/news/1", "https://example.com/%6Eews/1"] {
                let content = document
                    .content_for(page)
                    .expect("within the bound")
                    .expect("the entry");
                assert_eq!(content.url, first, "{first} then {second}: {page}");
                assert_eq!(content.licences.len(), 2, "{first} then {second}: {page}");
                assert!(content.licences[0].permits_usage.is_some());
                assert!(content.licences[1].prohibits_usage.is_some());
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
            .expect("within the bound")
            .expect("the entry");
        licence_terms(&page, "https://example.com/license.xml")
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
            .expect("within the bound")
            .expect("the site-wide entry");
        assert_eq!(page.url, "/");
        let terms = licence_terms(&page, "https://example.com/license.xml");
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
        assert!(payment.needs_settlement());
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
            .expect("within the bound")
            .expect("the more specific entry");
        assert_eq!(archive.url, "/archive/*");
        let terms = licence_terms(&archive, "https://example.com/license.xml");
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
                        .expect("within the bound")
                        .map(|content| content.url.clone()),
                    Some(scope.to_owned()),
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
                    document
                        .content_for(elsewhere)
                        .expect("within the bound")
                        .is_none(),
                    "{scope} does not govern {elsewhere}"
                );
            }
        }
        let document = scoped("http://publisher.example:80/");
        assert!(
            document
                .content_for("http://publisher.example/x")
                .expect("within the bound")
                .is_some()
        );
        assert!(
            document
                .content_for("https://publisher.example/x")
                .expect("within the bound")
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
        .expect("within the bound")
        .map(|content| content.url.clone())
    }

    /// Entries are compared by the path they constrain, whatever the
    /// spelling of the host in front of it: a narrower scope governs under a
    /// broad one written longer (`https://PUBLISHER.example.:443/` is 31
    /// characters, `https://publisher.example/n/` 28), and a relative entry
    /// governs under an absolute scope whose path it lies within.
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

    /// An absolute scope and a relative entry of the same path are one
    /// scope: they are read as one entry in either document order, with the
    /// licences of both and the first entry's `url`, so neither's rule is
    /// dropped (RSL 3.1.1). An absolute scope within a relative pattern
    /// governs alone, and so does a relative entry within an absolute scope.
    #[test]
    fn an_absolute_scope_and_a_relative_entry_on_the_same_path_are_read_as_one_in_either_order() {
        let selected = |entries: [(&str, &str); 2], page: &str| {
            let contents: String = entries
                .iter()
                .map(|(url, rule)| {
                    format!(
                        r#"<content url="{url}"><license><{rule} type="usage">ai-input</{rule}></license></content>"#
                    )
                })
                .collect();
            let document = parse_rsl(&format!(
                r#"<rsl xmlns="https://rslstandard.org/rsl">{contents}</rsl>"#
            ))
            .expect("parses");
            let content = document
                .content_for(page)
                .expect("within the bound")
                .expect("an entry");
            (content.url.clone(), content.licences.len())
        };
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
            (
                ("https://publisher.example/", "prohibits"),
                ("/*", "permits"),
            ),
            (
                ("https://publisher.example/n", "permits"),
                ("/n*", "prohibits"),
            ),
        ] {
            for entries in [[absolute, relative], [relative, absolute]] {
                assert_eq!(
                    selected(entries, "https://publisher.example/n/1"),
                    (entries[0].0.to_owned(), 2),
                    "{entries:?}"
                );
            }
        }
        for (governs, aside) in [
            (
                ("https://publisher.example/n/", "permits"),
                ("/*", "prohibits"),
            ),
            (
                ("https://publisher.example/n/", "permits"),
                ("/n", "prohibits"),
            ),
            (
                ("/n/1", "permits"),
                ("https://publisher.example/", "prohibits"),
            ),
            (
                ("/*1", "permits"),
                ("https://publisher.example/", "prohibits"),
            ),
        ] {
            for entries in [[governs, aside], [aside, governs]] {
                assert_eq!(
                    selected(entries, "https://publisher.example/n/1"),
                    (governs.0.to_owned(), 1),
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

    /// Where the ranking leaves two entries equal (one relative scope in two
    /// spellings, and two absolute scopes of one written length that
    /// neither matches the page as written), they are read as one entry, so
    /// a permit and a prohibition of AI input are ruled in either document
    /// order as the same two licences written in one `<content>`, and a
    /// prohibition in one beside a licence silent on usage in the other is
    /// a Disallow.
    #[test]
    fn equally_ranked_entries_are_ruled_as_one_entry_in_either_order() {
        let licence =
            |rule: &str| format!(r#"<license><{rule} type="usage">ai-input</{rule}></license>"#);
        let ruling = |xml: String| {
            let document = parse_rsl(&xml).expect("parses");
            let content = document
                .content_for("https://publisher.example/n/1")
                .expect("within the bound")
                .expect("an entry");
            let terms = licence_terms(&content, "https://publisher.example/license.xml");
            (combine(&terms.statements)[&Category::AiInput], terms.offer)
        };
        for (permit, prohibition) in [
            ("/n", "/%6E"),
            (
                "HTTPS://PUBLISHER.EXAMPLE./n/",
                "https://publisher.example./n/",
            ),
        ] {
            for (first, second) in [(permit, prohibition), (prohibition, permit)] {
                let xml = format!(
                    r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="{first}">{}</content><content url="{second}"><license><payment type="free"/></license></content></rsl>"#,
                    licence("prohibits"),
                );
                assert_eq!(
                    ruling(xml),
                    (Effective::Disallow, None),
                    "{first} prohibits, then {second} silent"
                );
                let xml = format!(
                    r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="{first}"><license><payment type="free"/></license></content><content url="{second}">{}</content></rsl>"#,
                    licence("prohibits"),
                );
                assert_eq!(
                    ruling(xml),
                    (Effective::Disallow, None),
                    "{first} silent, then {second} prohibits"
                );
            }
            let written = |first: (&str, &str), second: (&str, &str)| {
                format!(
                    r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="{}">{}</content><content url="{}">{}</content></rsl>"#,
                    first.0,
                    licence(first.1),
                    second.0,
                    licence(second.1)
                )
            };
            let (permits, prohibits) = ((permit, "permits"), (prohibition, "prohibits"));
            let one_entry = |first: &str, second: &str| {
                format!(
                    r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/">{}{}</content></rsl>"#,
                    licence(first),
                    licence(second)
                )
            };
            assert_eq!(
                ruling(written(permits, prohibits)),
                ruling(one_entry("permits", "prohibits")),
                "{permit} then {prohibition}"
            );
            assert_eq!(
                ruling(written(prohibits, permits)),
                ruling(one_entry("prohibits", "permits")),
                "{prohibition} then {permit}"
            );
            assert_eq!(
                ruling(written(permits, prohibits)).0,
                ruling(written(prohibits, permits)).0,
                "{permit} and {prohibition}: the order does not change the ruling"
            );
        }
    }

    /// Every pattern over `a`, `b`, `/` and `*` of up to four characters
    /// after the leading `/`, with and without a final `$`: 682 patterns,
    /// each its own matching form.
    fn generated_patterns() -> Vec<String> {
        let mut open = vec![String::from("/")];
        let mut grown = open.clone();
        for _ in 0..4 {
            grown = grown
                .iter()
                .flat_map(|stem| ['a', 'b', '/', '*'].map(|next| format!("{stem}{next}")))
                .collect();
            open.extend(grown.iter().cloned());
        }
        let anchored: Vec<String> = open.iter().map(|pattern| format!("{pattern}$")).collect();
        open.extend(anchored);
        for pattern in &open {
            assert_eq!(&commonmeasure_types::matching_pattern(pattern), pattern);
        }
        open
    }

    /// Every target over `a`, `b`, `/` and `z` of up to five characters
    /// after the leading `/`: 1,365 targets. No pattern holds `z`, so it
    /// stands for an octet a pattern's literals cannot match, and five
    /// characters hold the most general target of every generated pattern.
    fn generated_targets() -> Vec<String> {
        let mut targets = vec![String::from("/")];
        let mut grown = targets.clone();
        for _ in 0..5 {
            grown = grown
                .iter()
                .flat_map(|stem| ['a', 'b', '/', 'z'].map(|next| format!("{stem}{next}")))
                .collect();
            targets.extend(grown.iter().cloned());
        }
        targets
    }

    /// Each generated pattern with the set of generated targets it matches.
    fn matched_sets() -> Vec<(String, Vec<bool>)> {
        let targets = generated_targets();
        generated_patterns()
            .into_iter()
            .map(|pattern| {
                let matched = targets
                    .iter()
                    .map(|target| matches_form(&pattern, target))
                    .collect();
                (pattern, matched)
            })
            .collect()
    }

    fn subset(inner: &[bool], outer: &[bool]) -> bool {
        inner
            .iter()
            .zip(outer)
            .all(|(&inner, &outer)| !inner || outer)
    }

    /// The containment test agrees with brute force: for every ordered pair
    /// of generated patterns (465,124 pairs), A lies within B exactly when
    /// B matches every generated target A matches.
    #[test]
    fn containment_is_what_brute_force_finds() {
        let sets = matched_sets();
        let mut within = 0;
        for (inner, inner_set) in &sets {
            for (outer, outer_set) in &sets {
                let expected = subset(inner_set, outer_set);
                assert_eq!(
                    Scope::new(inner).within(&Scope::new(outer)),
                    expected,
                    "{inner} within {outer}"
                );
                within += usize::from(expected);
            }
        }
        assert_eq!(sets.len(), 682);
        assert!(within > sets.len(), "{within}");
    }

    /// The entries that govern a page under two relative entries, for every
    /// pair of generated patterns and every target of up to four characters
    /// after the leading `/` that both match, in both document orders. An
    /// entry is set aside exactly when the other lies strictly within it,
    /// whatever their lengths, and both govern otherwise. Containment is
    /// read from the brute-force sets, not from [`Scope`].
    #[test]
    fn selection_sets_an_entry_aside_only_when_the_other_lies_strictly_within_it() {
        let sets = matched_sets();
        let short_targets: Vec<(usize, String)> = generated_targets()
            .into_iter()
            .enumerate()
            .filter(|(_, target)| target.len() <= 5)
            .collect();
        let entry = |url: &str| RslContent {
            url: url.to_owned(),
            server: None,
            licences: vec![RslLicence::default()],
        };
        let mut checked = 0;
        for (one, one_set) in &sets {
            for (other, other_set) in &sets {
                let within = subset(one_set, other_set);
                let contains = subset(other_set, one_set);
                // Which of (one, other) govern.
                let governs = (!contains || within, !within || contains);
                let document = RslDocument {
                    contents: vec![entry(one), entry(other)],
                };
                let expected = match governs {
                    (true, true) => (one.as_str(), 2),
                    (true, false) => (one.as_str(), 1),
                    (false, true) => (other.as_str(), 1),
                    (false, false) => panic!("{one} and {other}: nothing governs"),
                };
                for (index, target) in &short_targets {
                    if !(one_set[*index] && other_set[*index]) {
                        continue;
                    }
                    let content = document
                        .content_for_reading(target, None, "")
                        .expect("within the bound")
                        .expect("an entry");
                    assert_eq!(
                        (content.url.as_str(), content.licences.len()),
                        expected,
                        "{one} then {other} for {target}"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 1_000_000, "{checked}");
    }

    /// Three relative entries over `a`, `b` and `*` (up to two characters
    /// after the leading `/`, with and without a final `$`), in every
    /// order, for every target of up to three characters after the `/`
    /// that all three match: the entries that govern are exactly those that
    /// no other strictly lies within, by brute force, combined in document
    /// order.
    #[test]
    fn selection_among_three_entries_keeps_exactly_the_narrowest() {
        let mut patterns = vec![String::from("/")];
        let mut grown = patterns.clone();
        for _ in 0..2 {
            grown = grown
                .iter()
                .flat_map(|stem| ['a', 'b', '*'].map(|next| format!("{stem}{next}")))
                .collect();
            patterns.extend(grown.iter().cloned());
        }
        let anchored: Vec<String> = patterns.iter().map(|p| format!("{p}$")).collect();
        patterns.extend(anchored);
        let targets: Vec<String> = generated_targets()
            .into_iter()
            .filter(|target| target.len() <= 4)
            .collect();
        let sets: Vec<Vec<bool>> = patterns
            .iter()
            .map(|pattern| {
                targets
                    .iter()
                    .map(|target| matches_form(pattern, target))
                    .collect()
            })
            .collect();
        let strictly = |inner: usize, outer: usize| {
            subset(&sets[inner], &sets[outer]) && !subset(&sets[outer], &sets[inner])
        };
        let mut checked = 0;
        let count = patterns.len();
        for one in 0..count {
            for two in 0..count {
                for three in 0..count {
                    let order = [one, two, three];
                    let document = RslDocument {
                        contents: order
                            .iter()
                            .enumerate()
                            .map(|(position, &index)| RslContent {
                                url: patterns[index].clone(),
                                server: None,
                                licences: vec![RslLicence {
                                    reporting: vec![RslReporting {
                                        kind: position.to_string(),
                                        profile: String::new(),
                                        endpoint: None,
                                        config: None,
                                    }],
                                    ..RslLicence::default()
                                }],
                            })
                            .collect(),
                    };
                    let expected: Vec<String> = (0..3)
                        .filter(|&position| {
                            !order.iter().any(|&other| strictly(other, order[position]))
                        })
                        .map(|position| position.to_string())
                        .collect();
                    for (index, target) in targets.iter().enumerate() {
                        if !order.iter().all(|&entry| sets[entry][index]) {
                            continue;
                        }
                        let content = document
                            .content_for_reading(target, None, "")
                            .expect("within the bound")
                            .expect("an entry");
                        let positions: Vec<String> = content
                            .licences
                            .iter()
                            .map(|licence| licence.reporting[0].kind.clone())
                            .collect();
                        assert_eq!(
                            positions,
                            expected,
                            "{:?} for {target}",
                            order.map(|index| &patterns[index])
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 10_000, "{checked}");
    }

    /// A trailing `*`, or `*$`, is the same scope as the pattern without
    /// it; a final `$` after a literal is not. A literal `%2A` or `%24` is
    /// three literal octets: `/x%2A` lies within `/x*`, not the reverse.
    #[test]
    fn a_trailing_wildcard_is_the_same_scope() {
        for (one, other) in [
            ("/*", "/"),
            ("/**", "/"),
            ("/*$", "/"),
            ("/**$", "/"),
            ("/news*", "/news"),
            ("/news*$", "/news"),
            ("/a*b*", "/a*b"),
            ("/a*b*$", "/a*b"),
            ("/x%2A*", "/x%2A"),
        ] {
            let (one, other) = (
                commonmeasure_types::matching_pattern(one),
                commonmeasure_types::matching_pattern(other),
            );
            assert!(
                Scope::new(&one).same_scope(&Scope::new(&other)),
                "{one} and {other}"
            );
        }
        for (inner, outer) in [
            ("/p$", "/p"),
            ("/a*b$", "/a*b"),
            ("/x%2A", "/x*"),
            ("/x%24", "/x"),
            ("/x$", "/x%24"),
        ] {
            let (inner_form, outer_form) = (
                commonmeasure_types::matching_pattern(inner),
                commonmeasure_types::matching_pattern(outer),
            );
            let (inner_scope, outer_scope) = (Scope::new(&inner_form), Scope::new(&outer_form));
            assert_eq!(
                (
                    inner_scope.within(&outer_scope),
                    outer_scope.within(&inner_scope)
                ),
                (inner != "/x$", false),
                "{inner} and {outer}"
            );
        }
        assert!(!commonmeasure_types::matching_pattern("/ *$ x").contains(ANY_RUN));
    }

    /// The pairs the rule is stated by, in both document orders: the entry
    /// governing, or both when they are read as one, whatever their lengths.
    #[test]
    fn named_pairs_govern_by_containment() {
        let selected = |entries: [&str; 2], page: &str| {
            let contents: String = entries
                .iter()
                .map(|url| {
                    format!(r#"<content url="{url}"><license><payment type="free"/></license></content>"#)
                })
                .collect();
            let document = parse_rsl(&format!(
                r#"<rsl xmlns="https://rslstandard.org/rsl">{contents}</rsl>"#
            ))
            .expect("parses");
            let content = document
                .content_for(&format!("https://publisher.example{page}"))
                .expect("within the bound")
                .expect("an entry");
            (content.url.clone(), content.licences.len())
        };
        // (governs alone, set aside, page)
        for (governs, aside, page) in [
            // Equal length, the first within the second.
            ("/p", "/*", "/page"),
            ("/news/", "/news*", "/news/1"),
            ("/a/", "/*/", "/a/1"),
            ("/news/", "/n*ws/", "/news/1"),
            ("/p*", "/*p", "/p"),
            ("/news*", "/*news", "/news/1"),
            ("/abc$", "/ab*c", "/abc"),
            ("/x%2A", "/*%2A", "/x%2A"),
            ("/a*/x", "/a*/*", "/ab/x"),
            // Unequal length, the first within the second.
            ("/p$", "/p", "/p"),
            ("/pa", "/*", "/page"),
            ("/news/", "/news", "/news/1"),
            ("/ab", "/a*b", "/abc"),
            ("/news/", "/new*s/", "/news/1"),
            ("/p", "/*p", "/p"),
            ("/blog/a.pdf$", "/*.pdf$", "/blog/a.pdf"),
            ("/abc", "/a*", "/abc"),
        ] {
            for entries in [[governs, aside], [aside, governs]] {
                assert_eq!(
                    selected(entries, page),
                    (governs.to_owned(), 1),
                    "{entries:?} for {page}"
                );
            }
        }
        for (one, other, page) in [
            // The same scope.
            ("/", "/*", "/page"),
            ("/p", "/p*", "/page"),
            ("/p", "/p*$", "/page"),
            ("/news/", "/news/**", "/news/1"),
            ("/n", "/%6E", "/n/1"),
            ("/news/", "/%6Eews/", "/news/1"),
            ("/p", "/p", "/page"),
            // Neither within the other, of equal length or not.
            ("/a*/x", "/*b*x", "/ab/x"),
            ("/a*", "/*b", "/ab"),
            ("/a*/x", "/*b/x", "/ab/x"),
            ("/xy", "/x*z", "/xyz"),
            ("/ab", "/*c", "/abc"),
            ("/blog/", "/*.pdf$", "/blog/a.pdf"),
        ] {
            for entries in [[one, other], [other, one]] {
                assert_eq!(
                    selected(entries, page),
                    (entries[0].to_owned(), 2),
                    "{entries:?} for {page}"
                );
            }
        }
    }

    /// Every entry of a governing scope is read with it in document order,
    /// whatever its length; an entry a governing entry lies strictly within
    /// is set aside.
    #[test]
    fn every_entry_of_a_governing_scope_is_read_with_it() {
        let document = RslDocument {
            contents: ["/", "/p*", "/*", "/p", "/p*$", "/*p"]
                .map(|url| RslContent {
                    url: url.to_owned(),
                    server: None,
                    licences: vec![RslLicence::default()],
                })
                .to_vec(),
        };
        let content = document
            .content_for("https://publisher.example/page")
            .expect("within the bound")
            .expect("an entry");
        // `/p*$`, `/p*` and `/p` are one scope; `/`, `/*` and `/*p` each
        // contain it.
        assert_eq!((content.url.as_str(), content.licences.len()), ("/p*", 3));
        let content = document
            .content_for("https://publisher.example/xp")
            .expect("within the bound")
            .expect("an entry");
        // For `/xp`, `/*p` lies within `/` and `/*`.
        assert_eq!((content.url.as_str(), content.licences.len()), ("/*p", 1));
    }

    /// `count` relative entries `{prefix}*{w}*`, each `w` a distinct window
    /// of `len` characters of one pseudo-random page, so every entry
    /// matches `{prefix}{page}` and no two lie one within the other; and
    /// that page.
    fn incomparable(count: usize, len: usize, prefix: &str) -> (Vec<String>, String) {
        let mut seed = 7_u64;
        let page: String = (0..count + len)
            .map(|_| {
                seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                char::from(b'a' + u8::try_from((seed >> 33) % 26).expect("a letter"))
            })
            .collect();
        let urls = (0..count)
            .map(|at| format!("{prefix}*{}*", &page[at..at + len]))
            .collect();
        (urls, format!("{prefix}{page}"))
    }

    /// The work selection counts among the entries of `urls` that match
    /// `page`, as [`governing_forms`] counts it.
    fn work_for(urls: &[String], page: &str) -> u64 {
        let document = RslDocument {
            contents: urls
                .iter()
                .map(|url| RslContent {
                    url: url.clone(),
                    server: None,
                    licences: Vec::new(),
                })
                .collect(),
        };
        let target = commonmeasure_types::matching_target(page);
        let scoped: Vec<ScopedEntry<'_>> = document
            .contents
            .iter()
            .filter_map(|content| {
                let form = commonmeasure_types::matching_pattern(&content.url);
                matches_form(&form, &target).then_some((content, form, None))
            })
            .collect();
        let mut forms: Vec<&str> = scoped.iter().map(|(_, form, _)| form.as_str()).collect();
        forms.sort_unstable();
        forms.dedup();
        let scopes: Vec<Scope> = forms.into_iter().map(Scope::new).collect();
        selection_work(&scoped, &scopes)
    }

    /// The budget counts the work among the entries that match one reading
    /// of the page, absolute scopes included; entries that do not match do
    /// none. 256 matching entries with forms of 64 octets, none within
    /// another, are read. Past the budget the licence is unread, whatever
    /// the entries say, and a reading past it ends the search; one before it
    /// that selects keeps its entry.
    #[test]
    fn the_budget_counts_the_work_among_the_entries_matching_one_reading() {
        let document = |urls: Vec<String>| RslDocument {
            contents: urls
                .into_iter()
                .map(|url| RslContent {
                    url,
                    server: None,
                    licences: vec![RslLicence::default()],
                })
                .collect(),
        };
        let selected = |urls: Vec<String>, page: &str| {
            document(urls)
                .content_for(&format!("https://publisher.example{page}"))
                .map(|content| content.map(|content| (content.url.clone(), content.licences.len())))
        };
        let strings = |urls: &[&str]| urls.iter().map(|url| (*url).to_owned()).collect::<Vec<_>>();
        // 10,000 entries that do not match and 3 that do: `/pa*` and `/*e`
        // overlap, and both lie within `/p`.
        let mut urls: Vec<String> = (0..10_000).map(|n| format!("/x{n}")).collect();
        urls.extend(strings(&["/p", "/pa*", "/*e"]));
        assert_eq!(selected(urls, "/page"), Ok(Some(("/pa*".to_owned(), 2))));
        // The floor: 256 entries of 64-octet forms, none within another.
        let (urls, page) = incomparable(256, 61, "/");
        assert_eq!(commonmeasure_types::matching_pattern(&urls[0]).len(), 64);
        assert!(work_for(&urls, &page) <= SELECTION_BUDGET);
        assert_eq!(
            selected(urls.clone(), &page),
            Ok(Some((urls[0].clone(), 256)))
        );
        // The fewest such entries of 2,000-octet forms whose work is past
        // the budget, and one fewer.
        let (all, page) = incomparable(512, 1_996, "/");
        let past = (2..=all.len())
            .find(|&count| work_for(&all[..count], &page) > SELECTION_BUDGET)
            .expect("512 entries of 2,000 octets are past the budget");
        assert_eq!(
            selected(all[..past].to_vec(), &page),
            Err(SelectionUnread::OverBudget)
        );
        assert_eq!(
            selected(all[..past - 1].to_vec(), &page)
                .map(|content| content.map(|(_, licences)| licences)),
            Ok(Some(past - 1))
        );
        // One entry written again and again is one form: its copies are
        // read, not compared, and are read as one entry.
        let page = format!("/{}", "7".repeat(300));
        assert_eq!(
            selected(vec!["/7".to_owned(); 1_000], &page),
            Ok(Some(("/7".to_owned(), 1_000)))
        );
        // Absolute scopes count: nested paths of the page, each within the
        // one before, as absolute scopes, past the budget and one short of
        // it, where the longest governs alone.
        let long = "a".repeat(40_000);
        let paths: Vec<String> = (1..=200)
            .map(|n| format!("/{}", &long[..n * 200]))
            .collect();
        let page = format!("/{long}");
        let over = (2..=paths.len())
            .find(|&count| work_for(&paths[..count], &page) > SELECTION_BUDGET)
            .expect("200 nested paths are past the budget");
        let absolute: Vec<String> = paths[..over]
            .iter()
            .map(|path| format!("https://publisher.example{path}"))
            .collect();
        assert_eq!(
            selected(absolute.clone(), &page),
            Err(SelectionUnread::OverBudget)
        );
        assert_eq!(
            selected(absolute[..over - 1].to_vec(), &page),
            Ok(Some((absolute[over - 2].clone(), 1)))
        );
        // `/a%2Fb…` as parsed matches none of `/a/b*w*`; its later reading
        // `/a/b…` matches them all, past the budget, which ends the search.
        let (mut urls, page) = incomparable(past, 1_996, "/a/b");
        let page = page.replacen("/a/b", "/a%2Fb", 1);
        assert_eq!(
            selected(urls.clone(), &page),
            Err(SelectionUnread::OverBudget)
        );
        // An entry the page as parsed matches governs before that reading.
        urls.push("/a%2F".to_owned());
        assert_eq!(selected(urls, &page), Ok(Some(("/a%2F".to_owned(), 1))));
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
    /// of the request target a relative entry is matched against, so
    /// `/news?x=1` lies within `/news?x`.
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
            .expect("within the bound")
            .expect("entry");
        let terms = licence_terms(&page, "https://example.com/license.xml");
        let effective = combine(&terms.statements);
        assert_eq!(effective[&Category::AiInput], Effective::Allow);
        assert_eq!(effective[&Category::TrainAi], Effective::Allow);
        assert_eq!(effective[&Category::Search], Effective::Disallow);
        let payment = terms
            .payment
            .expect("the subscription travels with the permit");
        assert_eq!(payment.kind.as_deref(), Some("subscription"));
        assert!(payment.needs_settlement());

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
                .expect("within the bound")
                .expect("entry");
            let terms = licence_terms(&page, "https://example.com/license.xml");
            assert_eq!(
                combine(&terms.statements)[&Category::AiInput],
                Effective::Disallow,
                "prohibits {prohibits} is at least as specific as permits ai-input, so it wins"
            );
        }
    }

    /// A licence that prohibits AI input refuses the entry it is in,
    /// whatever another licence of the entry permits, in either order: no
    /// offer is taken, and the Disallow names the prohibiting licence. A
    /// specific permit over a blanket `prohibits all` in one licence is no
    /// prohibition, and a restricted licence that prohibits AI input is one.
    #[test]
    fn a_prohibition_in_any_licence_of_the_entry_refuses_whatever_another_permits() {
        const PROHIBITS: &str =
            r#"<license><prohibits type="usage">ai-input</prohibits></license>"#;
        const FREE: &str =
            r#"<license><permits type="usage">ai-input</permits><payment type="free"/></license>"#;
        const PRICED: &str = r#"<license><permits type="usage">ai-input</permits><payment type="use"><amount currency="USD">1</amount></payment></license>"#;
        const SILENT: &str = r#"<license><payment type="free"/></license>"#;
        const SPECIFIC: &str = r#"<license><permits type="usage">ai-input</permits><prohibits type="usage">all</prohibits><payment type="free"/></license>"#;
        const RESTRICTED: &str = r#"<license><permits type="user">education</permits><prohibits type="usage">ai-input</prohibits></license>"#;
        let ruling = |licences: &str| {
            let document = parse_rsl(&format!(
                r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/">{licences}</content></rsl>"#
            ))
            .expect("parses");
            let content = document
                .content_for("https://example.com/x")
                .expect("within the bound")
                .expect("an entry");
            let terms = licence_terms(&content, "L");
            let detail = terms
                .statements
                .iter()
                .find(|statement| statement.category == Category::AiInput)
                .map(|statement| statement.detail.clone());
            (
                combine(&terms.statements)[&Category::AiInput],
                terms.offer,
                detail,
            )
        };
        for (other, by_name) in [
            (FREE, true),
            (PRICED, true),
            (SILENT, false),
            (SPECIFIC, true),
        ] {
            for (licences, prohibiting, permitting) in [
                (format!("{PROHIBITS}{other}"), 1, 2),
                (format!("{other}{PROHIBITS}"), 2, 1),
                (format!("{RESTRICTED}{other}"), 1, 2),
            ] {
                let detail = if by_name {
                    format!(
                        "L: licence {prohibiting} prohibits usage ai-input, which takes \
                         precedence over the permit of licence {permitting}"
                    )
                } else {
                    format!("L: no licence permits usage ai-input (licence {prohibiting})")
                };
                assert_eq!(
                    ruling(&licences),
                    (Effective::Disallow, None, Some(detail)),
                    "{licences}"
                );
            }
        }
        assert_eq!(ruling(SPECIFIC).0, Effective::Allow);
        assert_eq!(ruling(SPECIFIC).1, Some(1));
        assert_eq!(ruling(&format!("{SPECIFIC}{FREE}")).1, Some(1));
    }

    #[test]
    fn an_rsl_document_without_content_is_an_error_and_malformed_xml_is_too() {
        assert!(parse_rsl("<rsl xmlns=\"https://rslstandard.org/rsl\"></rsl>").is_err());
        assert!(parse_rsl("<rsl><content url=\"/\">").is_err());
    }

    fn rsl_link(field: &[u8], page_url: &str) -> LinkLicences {
        rsl_links([field], page_url)
    }

    fn named(url: &str) -> LinkLicences {
        LinkLicences {
            named: Some(url.to_owned()),
            unread: Vec::new(),
        }
    }

    #[test]
    fn a_link_header_names_the_rsl_licence_only_when_typed_as_one() {
        assert_eq!(
            rsl_link(
                b"<https://example.com/license.xml>; rel=\"license\"; type=\"application/rsl+xml\"",
                "https://example.com/a"
            ),
            named("https://example.com/license.xml")
        );
        assert_eq!(
            rsl_link(
                b"</style.css>; rel=preload, </license.xml>; rel=license; type=\"application/rsl+xml\"",
                "https://example.com/a/b"
            ),
            named("https://example.com/license.xml")
        );
        assert_eq!(
            rsl_link(
                b"<https://creativecommons.org/licenses/by/4.0/>; rel=\"license\"",
                "https://example.com/a"
            ),
            LinkLicences::default(),
            "a licence link without the RSL type is not an RSL licence"
        );
        assert_eq!(
            rsl_link(
                b"</l.xml>; REL=\"author license\"; Type=\"Application/RSL+XML; v=1\"",
                "https://example.com/a"
            ),
            named("https://example.com/l.xml"),
            "rel is a list, and names and the media type are read without case"
        );
    }

    /// EDG-102 review P0: bytes that are not UTF-8 in a parameter this edge
    /// does not read, or in a member naming no licence, stop nothing.
    #[test]
    fn opaque_bytes_outside_the_licence_target_rel_and_type_are_not_read() {
        let page = "https://example.com/a";
        for field in [
            &b"</license.xml>; rel=\"license\"; type=\"application/rsl+xml\"; title=\"caf\xe9\""[..],
            b"</license.xml>; title=caf\xe9; rel=license; type=\"application/rsl+xml\"",
            b"</license.xml>; rel=license; type=\"application/rsl+xml\"; x\xe9",
            b"</caf\xe9>; rel=author, </license.xml>; rel=license; type=\"application/rsl+xml\"",
            b"</caf\xe9>; rel=\"caf\xe9\", </license.xml>; rel=license; type=\"application/rsl+xml\"",
        ] {
            assert_eq!(
                rsl_link(field, page),
                named("https://example.com/license.xml"),
                "{}",
                String::from_utf8_lossy(field)
            );
        }
    }

    /// A member that names a licence, or may, and cannot be read is an
    /// unread licence, recorded by its target or, where it has none that
    /// could be delimited, by the whole member.
    #[test]
    fn a_licence_member_that_cannot_be_read_is_unread() {
        let page = "https://example.com/a";
        for (field, written, why) in [
            (
                &b"</licen\xe9e.xml>; rel=license; type=\"application/rsl+xml\""[..],
                "/licen\\xE9e.xml",
                "its target holds bytes that are not UTF-8",
            ),
            (
                b"</l.xml>; rel=license; type=\"application/rsl+xml\xe9\"",
                "/l.xml",
                "its type holds bytes that are not UTF-8",
            ),
            (
                b"</l.xml>; rel=\"license\xe9\"; type=\"application/rsl+xml\"",
                "/l.xml",
                "rel mentions `license` and holds bytes that are not UTF-8",
            ),
            (
                b"</l.xml>; r\xe9l=license; type=\"application/rsl+xml\"",
                "/l.xml",
                "a parameter name that is not a token",
            ),
            (
                b"<http://[bad>; rel=license; type=\"application/rsl+xml\"",
                "http://[bad",
                "its target is not a URL",
            ),
            (
                b"</l.xml; rel=license; type=\"application/rsl+xml\"",
                "</l.xml; rel=license; type=\"application/rsl+xml\"",
                "could not be parsed: its target has no closing >",
            ),
            (
                b"</a>; title=\"open, </l.xml>; rel=\"license\"; type=\"application/rsl+xml\"",
                "</a>; title=\"open, </l.xml>; rel=\"license\"; type=\"application/rsl+xml\"",
                "could not be parsed",
            ),
            (
                b"l.xml; rel=license",
                "l.xml; rel=license",
                "could not be parsed: it does not begin with a target",
            ),
        ] {
            let found = rsl_link(field, page);
            assert_eq!(found.named, None, "{written}");
            assert_eq!(found.unread.len(), 1, "{written}: {found:?}");
            assert_eq!(found.unread[0].written, written);
            assert!(found.unread[0].why.contains(why), "{found:?}");
        }
    }

    /// A member that cannot be parsed and says nothing of a licence costs
    /// only itself: the licence beside it is read, in the same field or
    /// another, and an unread licence beside a readable one is kept too.
    #[test]
    fn a_member_that_cannot_be_parsed_takes_no_licence_with_it() {
        let page = "https://example.com/a";
        let licence = &b"</license.xml>; rel=\"license\"; type=\"application/rsl+xml\""[..];
        for stray in [&b"stray"[..], b"<unclosed; rel=author", b"</x>junk"] {
            let mut field = stray.to_vec();
            if stray.starts_with(b"<unclosed") {
                // A `<` left open runs to the end of the field, so the
                // licence goes in a field of its own.
                assert_eq!(
                    rsl_links([&field[..], licence], page),
                    named("https://example.com/license.xml")
                );
                continue;
            }
            field.extend_from_slice(b", ");
            field.extend_from_slice(licence);
            assert_eq!(
                rsl_link(&field, page),
                named("https://example.com/license.xml"),
                "{}",
                String::from_utf8_lossy(&field)
            );
        }
        let both = rsl_links(
            [
                &b"</licen\xe9e.xml>; rel=license; type=\"application/rsl+xml\""[..],
                licence,
            ],
            page,
        );
        assert_eq!(
            both.named.as_deref(),
            Some("https://example.com/license.xml")
        );
        assert_eq!(both.unread.len(), 1, "{both:?}");
    }

    /// An IRI target in UTF-8 is read as it was before the field was read
    /// from bytes, and a quoted rel keeps its escapes.
    #[test]
    fn a_utf8_target_and_an_escaped_quote_are_read() {
        assert_eq!(
            rsl_link(
                "</licénce.xml>; rel=\"lic\\ense\"; type=\"application/rsl+xml\"".as_bytes(),
                "https://example.com/a"
            ),
            named("https://example.com/lic%C3%A9nce.xml")
        );
        assert_eq!(
            rsl_link(
                b"</t>; title=\"a \\\" , b\"; rel=license; type=\"application/rsl+xml\"",
                "https://example.com/a"
            ),
            named("https://example.com/t")
        );
    }

    /// EDG-115: `rel*` and `type*` are `rel` and `type` once decoded. A
    /// member whose plain and starred values agree is read; one on which
    /// they disagree about whether it names an RSL licence is unread,
    /// whichever comes first.
    #[test]
    fn rel_star_and_type_star_are_decoded_as_rel_and_type() {
        let page = "https://example.com/a";
        for field in [
            &b"</l.xml>; rel*=UTF-8''license; type=\"application/rsl+xml\""[..],
            b"</l.xml>; Rel*=utf-8'en'lic%65nse; type=application/rsl+xml",
            b"</l.xml>; rel*=\"UTF-8''author%20license\"; type=application/rsl+xml",
            b"</l.xml>; rel*=ISO-8859-1''license; type=application/rsl+xml",
            b"</l.xml>; rel=license; type*=UTF-8''application%2Frsl%2Bxml",
            b"</l.xml>; rel=license; TYPE*=UTF-8''application/rsl+xml",
            b"</l.xml>; rel=license; rel*=UTF-8''license; type=application/rsl+xml",
            b"</l.xml>; rel=license; type=application/rsl+xml; type*=UTF-8''application%2Frsl%2Bxml",
        ] {
            assert_eq!(
                rsl_link(field, page),
                named("https://example.com/l.xml"),
                "{}",
                String::from_utf8_lossy(field)
            );
        }
        for field in [
            &b"</l.xml>; rel=author; rel*=UTF-8''license; type=application/rsl+xml"[..],
            b"</l.xml>; rel*=UTF-8''license; rel=author; type=application/rsl+xml",
            b"</l.xml>; rel=license; rel*=UTF-8''author; type=application/rsl+xml",
            b"</l.xml>; rel=license; type=text/html; type*=UTF-8''application%2Frsl%2Bxml",
            b"</l.xml>; rel=license; type*=UTF-8''text%2Fhtml; type=application/rsl+xml",
        ] {
            let read = rsl_link(field, page);
            assert_eq!(read.named, None, "{}", String::from_utf8_lossy(field));
            assert_eq!(read.unread.len(), 1, "{}", String::from_utf8_lossy(field));
            assert!(
                read.unread[0].why.contains("disagree"),
                "{}: {read:?}",
                String::from_utf8_lossy(field)
            );
        }
        // Agreement that the member names no licence leaves it none.
        assert_eq!(
            rsl_link(
                b"</l.xml>; rel=author; rel*=UTF-8''author; type=application/rsl+xml",
                page
            ),
            LinkLicences::default()
        );
    }

    /// EDG-115: a `rel*` or `type*` that does not decode in a member that
    /// may name a licence, and an escaped `license` in a member that
    /// cannot be parsed, are unread licences; a `rel*` that does not decode
    /// in a member that never mentions `license` is not.
    #[test]
    fn a_licence_behind_an_undecodable_value_or_an_escape_is_unread() {
        let page = "https://example.com/a";
        for (field, why) in [
            (
                &b"</t.xml>; rel*=x-unknown''license; type=application/rsl+xml"[..],
                "rel* whose RFC 8187 encoding does not decode",
            ),
            (
                b"</t.xml>; rel*=UTF-8''lic%65nse%ZZ; type=application/rsl+xml",
                "rel* whose RFC 8187 encoding does not decode",
            ),
            (
                b"</t.xml>; rel*=license; type=application/rsl+xml",
                "rel* whose RFC 8187 encoding does not decode",
            ),
            (
                b"</t.xml>; rel=license; type*=UTF-8''application%2Frsl%ZZxml",
                "type* does not decode",
            ),
            (
                b"</t.xml>; rel=\"lic\\ense\"; type=\"application/rsl+xml\"; x=\"open",
                "could not be parsed",
            ),
            (b"</t.xml>; rel=\"li\\cense\" junk", "could not be parsed"),
        ] {
            let found = rsl_link(field, page);
            assert_eq!(found.named, None, "{}", String::from_utf8_lossy(field));
            assert_eq!(found.unread.len(), 1, "{found:?}");
            assert!(found.unread[0].why.contains(why), "{found:?}");
        }
        assert_eq!(
            rsl_link(b"</t.xml>; rel*=x-unknown''author; type=text/html", page),
            LinkLicences::default()
        );
    }

    /// EDG-115: the first readable RSL licence is named and a second
    /// distinct one is unread, in one field or two; the same licence named
    /// twice, in any spelling that resolves to one URL, is one licence.
    #[test]
    fn a_second_distinct_rsl_licence_is_unread() {
        let page = "https://example.com/a/b";
        let first = &b"</free.xml>; rel=license; type=application/rsl+xml"[..];
        let second = &b"</paid.xml>; rel=license; type=application/rsl+xml"[..];
        for found in [
            rsl_links([first, second], page),
            rsl_link(&[first, b", ", second].concat(), page),
        ] {
            assert_eq!(found.named.as_deref(), Some("https://example.com/free.xml"));
            assert_eq!(found.unread.len(), 1, "{found:?}");
            assert_eq!(found.unread[0].written, "https://example.com/paid.xml");
            assert!(
                found.unread[0].why.contains("another RSL licence"),
                "{found:?}"
            );
        }
        assert_eq!(
            rsl_links(
                [
                    first,
                    b"<https://example.com/free.xml>; rel=\"license\"; type=\"application/rsl+xml\"",
                    b"<../free.xml>; rel*=UTF-8''license; type=application/rsl+xml",
                ],
                page
            ),
            named("https://example.com/free.xml")
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
