//! What the prompt-submission hook reads from a submitted prompt: which URLs
//! the user named, as hashes, and any statement embedded in pasted content.
//!
//! "Provided by the user" means the user supplied the content bytes; a URL
//! the user pastes is a reference, and fetching it is acquisition by the
//! system. The hook therefore records two different things. For each URL in
//! the prompt it records a hash, so a later mediated crossing can say
//! whether the user or the agent named the source without any prompt text
//! entering the record. For pasted material it reads the statements the
//! paste itself carries: an RSL licence in HTML, a `Content-Usage` line in a
//! pasted HTTP response, and a C2PA manifest embedded in text under Annex A.8
//! or in HTML under A.7, read with the C2PA SDK. Plain text with nothing
//! embedded is recorded as carrying no embedded statement, which is unknown,
//! never disallow.
//!
//! Nothing here opens a socket. A `constraint_info` URL on a C2PA assertion
//! and a `<link rel="license">` reference are recorded as references for the
//! mediated path to follow; the hook must never delay the host.

use commonmeasure_types::canonical::sha256_digest;
use std::io::Cursor;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::declarations::{self, Category, Preference, Statement, StatementSource};
use crate::grounding;

/// How each URL hash is computed, carried on the record so a reader can
/// recompute one from a URL they hold.
pub const URL_HASH_BASIS: &str =
    "sha256 over the URL text as written in the prompt, trailing punctuation removed";

/// The CAWG training and data mining assertion's label (version 1.1).
pub const TRAINING_MINING_LABEL: &str = "cawg.training-mining";

/// A reference the paste carried to a document that was not fetched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddedReference {
    /// `rsl-link` for an HTML `<link rel="license" type="application/rsl+xml">`,
    /// `c2pa-manifest-link` for `<link rel="c2pa-manifest">`,
    /// `constraint-info` for a CAWG `constrained` entry's policy URL.
    pub kind: String,
    pub href: String,
}

/// What the C2PA SDK read from an embedded manifest store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct C2paReading {
    /// `annex-a8` for the invisible variation-selector wrapper in text,
    /// `annex-a7-script` for an inline `<script type="application/c2pa">`.
    pub method: String,
    /// The SDK's validation state: `invalid`, `valid` (integrity checks pass,
    /// signer not on a trust list) or `trusted`. Recorded as reported.
    pub validation_state: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validation_status: Vec<String>,
    /// The active manifest's claim generator, where the SDK names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_generator: Option<String>,
    /// The training-and-data-mining entries as declared: label to `use`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub training_mining: Option<Value>,
    /// Why the store could not be read, when it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The record the hook writes for one prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptScan {
    pub url_hashes: Vec<String>,
    pub url_hash_basis: &'static str,
    /// Statements read from pasted material, each with its source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statements: Vec<Statement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<EmbeddedReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c2pa: Option<C2paReading>,
    /// `no embedded statement` when the paste carried nothing this reader
    /// knows, otherwise `statements read`.
    pub embedded: &'static str,
    /// What was not scanned and why, so the record does not read as a scan
    /// that found nothing.
    pub not_scanned: Vec<&'static str>,
}

/// Scan one prompt. Pure and total: any text yields a record.
pub fn scan(prompt: &str) -> PromptScan {
    let mut url_hashes: Vec<String> = grounding::extract_urls(&Value::String(prompt.to_owned()))
        .iter()
        .map(|url| sha256_digest(url.as_bytes()))
        .collect();
    url_hashes.sort();
    url_hashes.dedup();

    let mut statements = Vec::new();
    let mut references = Vec::new();

    for document in rsl_scripts(prompt) {
        // An inline licence applies to the content it is embedded in (RSL
        // section 4.6.2): the first content entry, whose `url` may be empty
        // for exactly that scope. A document that does not parse yields no
        // statement, which is unknown.
        if let Ok(parsed) = declarations::parse_rsl(&document)
            && let Some(content) = parsed.contents.first()
        {
            statements.extend(
                declarations::licence_terms(content, "pasted RSL <script>")
                    .statements
                    .into_iter()
                    .map(|statement| Statement {
                        source: StatementSource::PastedRsl,
                        ..statement
                    }),
            );
        }
    }
    for href in rsl_links(prompt) {
        references.push(EmbeddedReference {
            kind: "rsl-link".to_owned(),
            href,
        });
    }
    for line in prompt.lines() {
        if let Some(value) = header_value(line, "content-usage:") {
            for (category, preference, label) in declarations::parse_content_usage(value) {
                statements.push(Statement {
                    source: StatementSource::PastedContentUsage,
                    category,
                    preference,
                    detail: format!("Content-Usage: {label}"),
                });
            }
        }
    }

    let mut c2pa = None;
    match c2pa_text::extract_manifest(prompt) {
        Ok(extracted) => {
            if let Some(manifest) = extracted.manifest {
                let reading = read_c2pa("annex-a8", &manifest, &mut statements, &mut references);
                c2pa = Some(reading);
            }
        }
        Err(error) => {
            c2pa = Some(C2paReading {
                method: "annex-a8".to_owned(),
                validation_state: "invalid".to_owned(),
                validation_status: Vec::new(),
                claim_generator: None,
                training_mining: None,
                error: Some(format!("the text wrapper could not be read: {error:?}")),
            });
        }
    }
    if c2pa.is_none()
        && let Ok(Some(html)) = c2pa_text::html::extract_html(prompt)
    {
        if let Some(manifest) = html.manifest {
            c2pa = Some(read_c2pa(
                "annex-a7-script",
                &manifest,
                &mut statements,
                &mut references,
            ));
        } else if let Some(href) = html.reference {
            references.push(EmbeddedReference {
                kind: "c2pa-manifest-link".to_owned(),
                href,
            });
        }
    }

    let embedded = if statements.is_empty() {
        "no embedded statement"
    } else {
        "statements read"
    };
    PromptScan {
        url_hashes,
        url_hash_basis: URL_HASH_BASIS,
        statements,
        references,
        c2pa,
        embedded,
        not_scanned: vec![
            "images: the prompt hook receives text only, so a manifest embedded in a pasted \
             image is not read",
        ],
    }
}

/// Read a manifest store with the C2PA SDK and take the training-and-data-
/// mining entries. The SDK validates integrity; without a trust list a
/// well-signed store reads as `valid`, not `trusted`, and the record says
/// which.
fn read_c2pa(
    method: &str,
    manifest: &[u8],
    statements: &mut Vec<Statement>,
    references: &mut Vec<EmbeddedReference>,
) -> C2paReading {
    let mut reading = C2paReading {
        method: method.to_owned(),
        validation_state: "invalid".to_owned(),
        validation_status: Vec::new(),
        claim_generator: None,
        training_mining: None,
        error: None,
    };
    let reader = match c2pa::Reader::from_context(c2pa::Context::new())
        .with_stream("application/c2pa", Cursor::new(manifest.to_vec()))
    {
        Ok(reader) => reader,
        Err(error) => {
            reading.error = Some(format!("the manifest store could not be read: {error}"));
            return reading;
        }
    };
    reading.validation_state = match reader.validation_state() {
        c2pa::ValidationState::Invalid => "invalid",
        c2pa::ValidationState::Valid => "valid",
        c2pa::ValidationState::Trusted => "trusted",
    }
    .to_owned();
    reading.validation_status = reader
        .validation_status()
        .unwrap_or_default()
        .iter()
        .map(|status| status.code().to_owned())
        .collect();
    let Some(active) = reader.active_manifest() else {
        reading.error = Some("the manifest store has no active manifest".to_owned());
        return reading;
    };
    // A version 2 claim names its generator in `claim_generator_info`; the
    // older single string is the fallback.
    reading.claim_generator = active
        .claim_generator_info
        .as_ref()
        .and_then(|infos| infos.first())
        .map(|info| match &info.version {
            Some(version) => format!("{}/{version}", info.name),
            None => info.name.clone(),
        })
        .or_else(|| active.claim_generator().map(str::to_owned));
    let Ok(assertion) = active.find_assertion::<Value>(TRAINING_MINING_LABEL) else {
        return reading;
    };
    let entries = assertion["entries"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut recorded = serde_json::Map::new();
    for (label, entry) in &entries {
        let usage = entry["use"].as_str().unwrap_or_default();
        recorded.insert(label.clone(), json!(usage));
        let category = match label.as_str() {
            "cawg.ai_inference" => Some(Category::AiInput),
            "cawg.ai_training" | "cawg.ai_generative_training" => Some(Category::TrainAi),
            _ => None,
        };
        // `constrained` carries the terms in `constraint_info`, which the
        // specification says may be a policy URL, and is treated as
        // notAllowed absent more information.
        let preference = match usage {
            "allowed" => Preference::Allow,
            "notAllowed" | "constrained" => Preference::Disallow,
            _ => continue,
        };
        if let Some(constraint) = entry["constraint_info"].as_str() {
            references.push(EmbeddedReference {
                kind: "constraint-info".to_owned(),
                href: constraint.to_owned(),
            });
        }
        if let Some(category) = category {
            statements.push(Statement {
                source: StatementSource::PastedC2pa,
                category,
                preference,
                detail: format!("{TRAINING_MINING_LABEL} {label}: {usage}"),
            });
        }
    }
    reading.training_mining = Some(Value::Object(recorded));
    reading
}

/// The value of `line` when it begins with the ASCII header `name`,
/// compared case-insensitively. `str::get` refuses an index that is not a
/// character boundary, so a multi-byte character within the first bytes of a
/// line (a bullet, a dash, a curly quote) is a non-match rather than a panic.
fn header_value<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let trimmed = line.trim_start();
    let head = trimmed.get(..name.len())?;
    if !head.eq_ignore_ascii_case(name) {
        return None;
    }
    trimmed.get(name.len()..).map(str::trim)
}

/// The bodies of `<script type="application/rsl+xml">` elements, in order.
fn rsl_scripts(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<script") {
        let open = from + at;
        let Some(close_tag) = lower[open..].find('>') else {
            break;
        };
        let tag = &lower[open..open + close_tag];
        let Some(body_end) = lower[open + close_tag..].find("</script") else {
            break;
        };
        let body_start = open + close_tag + 1;
        let body_end = open + close_tag + body_end;
        if tag.contains("application/rsl+xml") {
            let body = text[body_start..body_end].trim();
            // The body may be wrapped in CDATA or an HTML comment.
            let body = body
                .strip_prefix("<![CDATA[")
                .and_then(|b| b.strip_suffix("]]>"))
                .unwrap_or(body)
                .trim();
            found.push(body.to_owned());
        }
        from = body_end;
    }
    found
}

/// The `href` of every `<link rel="license" type="application/rsl+xml">`.
fn rsl_links(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<link") {
        let open = from + at;
        let Some(close) = lower[open..].find('>') else {
            break;
        };
        let tag = &text[open..open + close];
        let tag_lower = &lower[open..open + close];
        if tag_lower.contains("application/rsl+xml")
            && tag_lower.contains("license")
            && let Some(href) = attribute_value(tag, "href")
        {
            found.push(href);
        }
        from = open + close;
    }
    found
}

fn attribute_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&format!("{name}="))?;
    let rest = &tag[at + name.len() + 1..];
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let end = rest[1..].find(quote)?;
        Some(rest[1..1 + end].to_owned())
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '>')
            .unwrap_or(rest.len());
        Some(rest[..end].to_owned())
    }
}

/// Who named a source: whether the URL a mediated crossing was asked for
/// was written in a prompt of this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamedBy {
    /// A prompt of this session carried this URL.
    User,
    /// Prompts were recorded for this session and none carried this URL.
    Agent,
    /// No prompt of this session was recorded, so nothing can be said: the
    /// host supplies no prompt hook, or none has fired yet.
    Unknown,
}

/// Decide who named `url` from the session's recorded prompt hashes.
/// `prompt_hashes` is `None` when the log holds no prompt record at all.
pub fn named_by(url: &str, prompt_hashes: Option<&[String]>) -> NamedBy {
    let Some(hashes) = prompt_hashes else {
        return NamedBy::Unknown;
    };
    // The URL as fetched may differ from the URL as written by a trailing
    // slash, which a browser bar and a prompt both elide; both spellings are
    // asked about.
    let candidates = [
        url.to_owned(),
        url.trim_end_matches('/').to_owned(),
        format!("{}/", url.trim_end_matches('/')),
    ];
    if candidates
        .iter()
        .any(|candidate| hashes.contains(&sha256_digest(candidate.as_bytes())))
    {
        NamedBy::User
    } else {
        NamedBy::Agent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_hashed_and_no_text_is_kept() {
        let scan = scan("Please read https://www.gov.uk/guidance and https://example.com/a.");
        assert_eq!(scan.url_hashes.len(), 2);
        assert!(
            scan.url_hashes
                .contains(&sha256_digest(b"https://www.gov.uk/guidance"))
        );
        let recorded = serde_json::to_string(&scan).unwrap();
        assert!(
            !recorded.contains("gov.uk"),
            "no URL text enters the record"
        );
        assert_eq!(scan.embedded, "no embedded statement");
        assert!(scan.statements.is_empty());
    }

    #[test]
    fn a_pasted_rsl_script_and_a_content_usage_line_are_read() {
        let prompt = "Summarise this page I copied:\n\
            <html><head>\n\
            <script type=\"application/rsl+xml\"><rsl xmlns=\"https://rslstandard.org/rsl\">\
            <content url=\"\"><license><prohibits type=\"usage\">ai-input</prohibits>\
            </license></content></rsl></script>\n\
            <link rel=\"license\" type=\"application/rsl+xml\" href=\"https://pub.example/license.xml\">\n\
            </head></html>\n\
            And the response headers:\n\
            HTTP/1.1 200 OK\n\
            Content-Usage: train-ai=n\n";
        let scan = scan(prompt);
        assert_eq!(scan.embedded, "statements read");
        assert!(
            scan.statements
                .iter()
                .any(|s| s.source == StatementSource::PastedRsl
                    && s.category == Category::AiInput
                    && s.preference == Preference::Disallow)
        );
        assert!(
            scan.statements
                .iter()
                .any(|s| s.source == StatementSource::PastedContentUsage
                    && s.category == Category::TrainAi
                    && s.preference == Preference::Disallow)
        );
        assert_eq!(scan.references[0].kind, "rsl-link");
        assert_eq!(scan.references[0].href, "https://pub.example/license.xml");
    }

    #[test]
    fn a_multibyte_character_near_the_start_of_a_line_does_not_panic() {
        // The agent-completion notice Claude Code submits as a prompt puts a
        // middle dot and curly quotes early in a line; byte 14 fell inside one.
        let prompt = "Explore repo · finished\n\
            ⏺ Agent \u{201c}Explore\u{201d} finished · 5m 45s\n\
            \u{2014} content-usage: train-ai=n\n\
            Content-Usage: train-ai=n\n";
        let scan = scan(prompt);
        assert_eq!(
            header_value("Explore repo · finished", "content-usage:"),
            None
        );
        assert_eq!(
            header_value("  CONTENT-USAGE: train-ai=n ", "content-usage:"),
            Some("train-ai=n")
        );
        assert_eq!(
            scan.statements
                .iter()
                .filter(|s| s.source == StatementSource::PastedContentUsage)
                .count(),
            1
        );
    }

    #[test]
    fn who_named_a_source_follows_the_recorded_hashes() {
        let hashes = vec![sha256_digest(b"https://a.example/page")];
        assert_eq!(
            named_by("https://a.example/page/", Some(&hashes)),
            NamedBy::User
        );
        assert_eq!(
            named_by("https://b.example/other", Some(&hashes)),
            NamedBy::Agent
        );
        assert_eq!(named_by("https://a.example/page", None), NamedBy::Unknown);
        assert_eq!(
            named_by("https://a.example/page", Some(&[])),
            NamedBy::Agent,
            "a prompt with no URL was recorded, so the agent chose this one"
        );
    }
}
