//! The operator's own corpus: internal supply as a first-class provider.
//!
//! A bounded directory of documents queried with the `query` capability
//! (`docs/contracts/provider.md`): retrieve directly from a bounded corpus,
//! which is not a web search and is not declared as one. A PwC-style RAG
//! endpoint is configuration on the same shape, not new architecture; this
//! repository carries only the generic capability.
//!
//! Everything the run path expects of a remote provider holds here: the same
//! envelope, the same admission, the exact "response" sealed beside the run.
//! The response is the JSON document this adapter derives its envelopes from,
//! so a reviewer re-derives the parse the same way they would for Exa. Where
//! the corpus differs is honesty the open web cannot offer: the operator owns
//! the rights, so a licence — corpus-wide or per document — and a
//! per-document date may finally be
//! *declared*, by the corpus manifest, never inferred — and a shortfall
//! against the requested result count is a knowable coverage fact rather
//! than a shrug.

use commonmeasure_types::canonical::sha256_digest;
use std::path::{Path, PathBuf};

use commonmeasure_types::{
    AcquisitionCharge, ContextEnvelope, DeclaredDate, LicenceState, ProviderCapability,
};
use serde_json::{Value, json};

use crate::{Acquisition, SupplyAdapter, SupplyError};

pub(crate) const CAPABILITIES: &[ProviderCapability] = &[ProviderCapability::Query];

/// Names the corpus root directory for `supplier_from_environment`.
pub const CORPUS_VARIABLE: &str = "COMMONMEASURE_INTERNAL_CORPUS";

/// Documents larger than this are reported as skipped rather than read. The
/// budget arithmetic admits sources whole or not at all, so a document this
/// size could never usefully enter a context window anyway.
const MAX_DOCUMENT_BYTES: u64 = 1024 * 1024;

/// Extensions treated as corpus documents. Deliberate and small: a corpus is
/// declared supply, and quietly ingesting whatever else sits in the directory
/// would record content nobody meant to offer.
const DOCUMENT_EXTENSIONS: [&str; 3] = ["md", "markdown", "txt"];

/// An optional `corpus.json` at the corpus root.
///
/// Its substantive jobs are declarations only the operator can make: the
/// licence, and per-document dates. `Unknown` stays the default even here:
/// owning the directory is not a rights statement, and a declaration must be
/// written down to be recorded — a document with no declared date is undated,
/// not dated by its mtime, because a filesystem timestamp is a fact about the
/// disk, not the operator's claim about the content.
/// Unknown fields are load errors: a misspelled `"lisense"` is a rights
/// statement lost, not an absent one.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusManifest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    licence: Option<LicenceState>,
    /// Operator-declared per-document licences, corpus-relative document path
    /// to licence state, under the same attachment rule as `dates`. A
    /// document with an entry here carries that declaration instead of the
    /// corpus-wide `licence`; a document without one falls back to it. This
    /// is how one corpus honestly mixes rights — an aggregated corpus can
    /// hold documents the operator licensed and documents nobody did, and a
    /// single corpus-wide state would misdeclare one side or the other.
    #[serde(default)]
    licences: std::collections::BTreeMap<String, LicenceState>,
    /// Operator-declared dates, corpus-relative document path to ISO date.
    /// Each key must name a document that exists, because a date declared
    /// for a path nobody wrote — a typo, a deleted file — is a declaration
    /// lost, otherwise indistinguishable from one never made.
    #[serde(default)]
    dates: std::collections::BTreeMap<String, String>,
    /// Operator-declared governance metadata, corpus-relative document path
    /// to declaration, under the same attachment rule as `dates`. The
    /// adapter carries these declarations verbatim; what they mean is the
    /// business of whatever admission policy reads them downstream.
    #[serde(default)]
    documents: std::collections::BTreeMap<String, DocumentMeta>,
}

/// What the operator declares about one document for governance to read:
/// which edition, version range and integration path it documents, the
/// support status it claims, and the entitlement tier it belongs to. Plain
/// strings throughout — a rule set matches them; this adapter never
/// interprets them. Unknown fields are load errors for the same reason they
/// are on the manifest: a misspelled declaration is a declaration lost.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct DocumentMeta {
    edition: String,
    version_range: String,
    integration_path: String,
    support_status: String,
    entitlement: String,
}

pub struct InternalCorpusAdapter {
    root: PathBuf,
}

impl InternalCorpusAdapter {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }
}

impl SupplyAdapter for InternalCorpusAdapter {
    fn provider(&self) -> &str {
        "internal"
    }

    fn capabilities(&self) -> &'static [ProviderCapability] {
        CAPABILITIES
    }

    /// Query the corpus: deterministic term matching over every document,
    /// ranked by how many distinct query terms a document contains, then by
    /// total occurrences, then by path. No model, no sampling, no network —
    /// what makes the evidence record re-derivable from the sealed response.
    fn query(&self, query: &str, limit: u32) -> Result<Acquisition, SupplyError> {
        let started = std::time::Instant::now();
        let root = self
            .root
            .canonicalize()
            .map_err(|error| SupplyError::Transport {
                detail: format!(
                    "corpus root {} is not readable: {error}",
                    self.root.display()
                ),
            })?;
        let manifest = manifest(&root)?;
        let licence = manifest.licence.clone().unwrap_or(LicenceState::Unknown);

        let terms = terms(query);
        let mut documents = Vec::new();
        let mut skipped = Vec::new();
        let mut visited = std::collections::HashSet::from([root.clone()]);
        collect(&root, &root, &mut documents, &mut skipped, &mut visited)?;
        documents.sort();
        // Discovered and scanned are different facts: a document can
        // be found and then fail its read, and counting it as scanned would
        // publish an I/O failure as coverage. `skipped` holds the difference,
        // entry by entry.
        let discovered = documents.len();
        let mut scanned = 0usize;
        let mut matches: Vec<Match> = documents
            .into_iter()
            .filter_map(|path| {
                let scored = score(&root, &path, &terms, &manifest, &mut skipped);
                scanned += usize::from(scored.is_some());
                scored
            })
            .filter(|matched| matched.distinct_terms > 0)
            .collect();
        matches.sort_by(|a, b| {
            b.distinct_terms
                .cmp(&a.distinct_terms)
                .then(b.occurrences.cmp(&a.occurrences))
                .then(a.relative.cmp(&b.relative))
        });
        matches.truncate(limit as usize);

        // The response this adapter answers with, sealed verbatim beside the
        // run. The envelopes below are derived from this value and nothing
        // else, so reviewing the parse means reading this one document.
        let response = json!({
            "corpus": file_url(&root),
            "corpus_name": manifest.name,
            "query": query,
            "terms": terms,
            "documents_discovered": discovered,
            "documents_scanned": scanned,
            "requested": limit,
            "licence": licence,
            "matches": matches.iter().map(Match::to_value).collect::<Vec<_>>(),
            "skipped": skipped,
        });
        let raw_response =
            serde_json::to_vec_pretty(&response).map_err(|error| SupplyError::Malformed {
                detail: format!("could not serialise the corpus response: {error}"),
            })?;
        let envelopes = envelopes_from(&response)?;

        Ok(Acquisition {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Query,
            endpoint: file_url(&root),
            // No HTTP happened. A fabricated 200 would record a response
            // nobody sent.
            http_status: None,
            latency_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
            provider_request_id: None,
            // The operator is not billed per query and no price is disclosed;
            // absent stays absent rather than becoming a convenient zero.
            charge: AcquisitionCharge::default(),
            envelopes,
            raw_response,
            invocation: None,
        })
    }
}

struct Match {
    relative: String,
    url: String,
    title: String,
    text: String,
    /// The operator's declared date for this document, from the manifest;
    /// `None` where none was written down.
    declared_date: Option<String>,
    /// The operator's per-document licence declaration, from the manifest's
    /// `licences` map; `None` where none was written down, in which case the
    /// corpus-wide declaration (or unknown) speaks for this document.
    licence: Option<LicenceState>,
    /// The operator's declared governance metadata, from the manifest's
    /// `documents` map; `None` where none was written down — absent, never
    /// defaulted, because an invented declaration is not the operator's.
    governance: Option<DocumentMeta>,
    distinct_terms: u32,
    occurrences: u32,
}

impl Match {
    fn to_value(&self) -> Value {
        json!({
            "path": self.relative,
            "url": self.url,
            "title": self.title,
            "text": self.text,
            "content_hash": sha256_digest(self.text.as_bytes()),
            "declared_date": self.declared_date,
            "licence": self.licence,
            "governance": self.governance,
            "distinct_terms": self.distinct_terms,
            "occurrences": self.occurrences,
        })
    }
}

/// Envelopes from the sealed response, exactly as a remote adapter derives
/// envelopes from its provider's body.
fn envelopes_from(response: &Value) -> Result<Vec<ContextEnvelope>, SupplyError> {
    // Read back from the sealed response like every other field, so the
    // envelopes are derivable from the one document a reviewer reads. A
    // declaration that cannot be read back is an error: answering `Unknown`
    // here would strip rights the operator wrote down.
    let licence: LicenceState =
        serde_json::from_value(response["licence"].clone()).map_err(|error| {
            SupplyError::Malformed {
                detail: format!("the sealed response's licence could not be read back: {error}"),
            }
        })?;
    let mut envelopes = Vec::new();
    for (index, item) in response["matches"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let Some(url) = item.get("url").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        let text = item.get("text").and_then(Value::as_str).map(str::to_owned);
        // A per-document declaration, where the operator wrote one, outranks
        // the corpus-wide state, read back from the sealed response like the
        // licence above so the envelope's rights claim is re-derivable from
        // the one document a reviewer reads. Only a `null` means "never
        // declared" and falls back; a declaration that cannot be read back is
        // the same error the corpus-wide case is, because answering with the
        // corpus-wide state instead would swap the operator's rights
        // statement for a different one, silently.
        let licence = match item.get("licence") {
            None | Some(Value::Null) => licence.clone(),
            Some(declared) => serde_json::from_value(declared.clone()).map_err(|error| {
                SupplyError::Malformed {
                    detail: format!(
                        "the sealed response's licence for {url} could not be read back: {error}"
                    ),
                }
            })?,
        };
        let declared_date = item
            .get("declared_date")
            .and_then(Value::as_str)
            .map(|date| DeclaredDate {
                date: date.to_owned(),
                provenance: "operator-declared: document date in corpus.json".to_owned(),
            });
        envelopes.push(ContextEnvelope {
            // A `file://` URL has no host; the empty host is the true
            // value, not a normalisation accident.
            host: String::new(),
            content_hash: text.as_deref().map(|text| sha256_digest(text.as_bytes())),
            title: item.get("title").and_then(Value::as_str).map(str::to_owned),
            text,
            licence,
            declared_date,
            native_metadata: json!({
                "path": item.get("path"),
                "distinct_terms": item.get("distinct_terms"),
                "occurrences": item.get("occurrences"),
                "governance": item.get("governance"),
            }),
            retrieval_rank: index as u32 + 1,
            source_url: url,
        });
    }
    Ok(envelopes)
}

/// The corpus manifest, under the same containment as every document it
/// speaks for.
///
/// Presence is tested without following the link: a `corpus.json` that
/// resolves outside the root would declare rights over the corpus from a file
/// the operator never put in it, and a dangling one is a declaration that was
/// lost. Both are errors, because both are otherwise indistinguishable from a
/// corpus with no manifest at all.
fn manifest(root: &Path) -> Result<CorpusManifest, SupplyError> {
    let path = root.join("corpus.json");
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CorpusManifest::default());
        }
        Err(error) => {
            return Err(SupplyError::Transport {
                detail: format!("cannot stat {}: {error}", path.display()),
            });
        }
    }
    let resolved = resolve_within(root, &path).map_err(|reason| SupplyError::Transport {
        detail: format!("corpus manifest {}: {reason}", path.display()),
    })?;
    let encoded = std::fs::read(&resolved).map_err(|error| SupplyError::Transport {
        detail: format!("cannot read {}: {error}", path.display()),
    })?;
    // A manifest nobody can parse must not quietly become "no licence
    // declared": the operator wrote a rights statement and it was lost.
    let parsed: CorpusManifest =
        serde_json::from_slice(&encoded).map_err(|error| SupplyError::Malformed {
            detail: format!("{} is not a valid corpus manifest: {error}", path.display()),
        })?;
    // A declaration for a document that is not in the corpus — a typo, a
    // file since deleted — is a declaration lost, and losing it silently
    // would leave that document unannotated with nobody told. An error here
    // is the only state a reader can distinguish from "never declared". The
    // scan only ever annotates files it will scan, so an existing file the
    // scanner skips — the manifest itself, an image — is the same loss one
    // step over and fails the same way. `dates` and `documents`
    // attach by the same rule and fail the same way.
    let declarations = parsed
        .dates
        .keys()
        .map(|declared| (declared, "a date"))
        .chain(
            parsed
                .documents
                .keys()
                .map(|declared| (declared, "governance metadata")),
        )
        .chain(
            parsed
                .licences
                .keys()
                .map(|declared| (declared, "a licence")),
        );
    for (declared, kind) in declarations {
        let target = root.join(declared);
        let scannable = Path::new(declared)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                DOCUMENT_EXTENSIONS.contains(&extension.to_lowercase().as_str())
            });
        match resolve_within(root, &target) {
            Ok(resolved) if resolved.is_file() && scannable => {}
            _ => {
                return Err(SupplyError::Malformed {
                    detail: format!(
                        "{} declares {kind} for {declared:?}, which is not a document in the \
                         corpus, so the declaration cannot attach to anything",
                        path.display()
                    ),
                });
            }
        }
    }
    Ok(parsed)
}

/// Closed-class English words carrying no retrieval signal. A prompt is a
/// sentence, not a keyword list, and "the" or "does" occurs in every document
/// in the corpus — matching on them would retrieve everything and rank
/// nothing, and the coverage record would claim the corpus covers every job.
/// Fixed and small, because the term list is published in the sealed response
/// and must be re-derivable by a reader.
const STOP_WORDS: [&str; 52] = [
    "about", "and", "any", "anything", "are", "but", "can", "cannot", "could", "did", "does",
    "for", "from", "had", "has", "have", "how", "into", "its", "may", "might", "must", "not",
    "our", "out", "shall", "should", "some", "than", "that", "the", "their", "them", "there",
    "these", "they", "this", "those", "was", "were", "what", "when", "where", "which", "who",
    "whose", "why", "will", "with", "would", "you", "your",
];

/// Distinct lowercase query terms, punctuation-trimmed, at least three
/// characters, stop words removed. Short or closed-class function words would
/// match every document and rank nothing.
fn terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = query
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|word| word.chars().count() >= 3 && !STOP_WORDS.contains(&word.as_str()))
        .collect();
    terms.sort();
    terms.dedup();
    terms
}

/// Every document under `directory`, recursively, in no particular order; the
/// caller sorts. Unreadable directories are an error rather than a silently
/// smaller corpus.
///
/// Containment: the corpus is the operator's declared directory, and
/// a symlink inside it is a pointer, not a declaration. Every directory
/// entered and every candidate document must resolve beneath the canonical
/// root, or it lands in `skipped` — a live probe showed a symlink to a file
/// outside the root being read, sealed and admitted as corpus content with
/// its provenance misstated. `visited` holds every canonical location already
/// reached, directory and document alike, so a symlink cycle terminates
/// instead of recursing to exhaustion and one document stays one document
/// however many names lead to it.
fn collect(
    root: &Path,
    directory: &Path,
    documents: &mut Vec<PathBuf>,
    skipped: &mut Vec<Value>,
    visited: &mut std::collections::HashSet<PathBuf>,
) -> Result<(), SupplyError> {
    let entries = std::fs::read_dir(directory).map_err(|error| SupplyError::Transport {
        detail: format!("cannot read {}: {error}", directory.display()),
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| SupplyError::Transport {
            detail: format!("cannot read {}: {error}", directory.display()),
        })?;
        let path = entry.path();
        if path.is_dir() {
            match resolve_within(root, &path) {
                Ok(resolved) => {
                    if visited.insert(resolved) {
                        collect(root, &path, documents, skipped, visited)?;
                    }
                }
                Err(reason) => skipped.push(json!({
                    "path": relative(root, &path),
                    "reason": reason,
                })),
            }
            continue;
        }
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if !DOCUMENT_EXTENSIONS.contains(&extension.as_str()) {
            continue;
        }
        let resolved = match resolve_within(root, &path) {
            Ok(resolved) => resolved,
            Err(reason) => {
                skipped.push(json!({
                    "path": relative(root, &path),
                    "reason": reason,
                }));
                continue;
            }
        };
        // A file reachable under two names — an alias beside its target, or a
        // directory symlink crossing back over it — is one document. Admitting
        // it twice would hash the same bytes twice and publish a corpus larger
        // than the one on disk, with the coverage record measuring the job
        // against a document count nobody wrote.
        if !visited.insert(resolved.clone()) {
            continue;
        }
        // Stat the resolved target: an unmeasurable size is a skip, never a
        // zero that slides under the bound.
        let meta = match std::fs::metadata(&resolved) {
            Ok(meta) => meta,
            Err(error) => {
                skipped.push(json!({
                    "path": relative(root, &path),
                    "reason": format!("cannot stat: {error}"),
                }));
                continue;
            }
        };
        // The same stat answers what kind of thing this is. A FIFO named
        // `pipe.md` measures zero bytes, passes every size test, and then
        // blocks the read until someone writes to it — a corpus query that
        // never returns and a run that produces no record at all.
        if !meta.is_file() {
            skipped.push(json!({
                "path": relative(root, &path),
                "reason": "not a regular file and was not read",
            }));
            continue;
        }
        let bytes = meta.len();
        if bytes > MAX_DOCUMENT_BYTES {
            skipped.push(json!({
                "path": relative(root, &path),
                "reason": format!(
                    "{bytes} bytes exceeds the {MAX_DOCUMENT_BYTES}-byte document bound; a \
                     source this size is never admitted whole, and sources are never truncated"
                ),
            }));
            continue;
        }
        // The resolved location, never the name it was reached by: the name is
        // resolved once, checked once and read once. Re-resolving it at read
        // time would ask the filesystem the same question twice with the rest
        // of the walk in between, and a symlink swapped in between the two
        // answers would supply the one that reached the model.
        documents.push(resolved);
    }
    Ok(())
}

/// A candidate's canonical location, required to stay beneath the canonical
/// corpus root ([`crate::resolve_within`]). The `Err` is the `skipped`
/// reason, worded for the sealed response.
fn resolve_within(root: &Path, path: &Path) -> Result<PathBuf, String> {
    crate::resolve_within(root, path).map_err(|fault| match fault {
        crate::Containment::Unresolvable(error) => format!("cannot resolve: {error}"),
        crate::Containment::Outside => {
            "resolves outside the corpus root and was not read".to_owned()
        }
    })
}

/// Score one document against the query terms, or report why it was skipped.
fn score(
    root: &Path,
    path: &Path,
    terms: &[String],
    manifest: &CorpusManifest,
    skipped: &mut Vec<Value>,
) -> Option<Match> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            // Recorded like the UTF-8 branch below: a document that could not
            // be read is a fact about this scan, and dropping it silently
            // would publish an I/O failure as a fact about the corpus's
            // extent — a smaller result set and a coverage claim that counts
            // a document nobody scanned.
            skipped.push(json!({
                "path": relative(root, path),
                "reason": format!("cannot read: {error}"),
            }));
            return None;
        }
    };
    let Ok(text) = String::from_utf8(bytes) else {
        // Recorded, not lossily decoded: a hash over repaired bytes would
        // never match the document it claims to cover.
        skipped.push(json!({
            "path": relative(root, path),
            "reason": "not valid UTF-8 text",
        }));
        return None;
    };
    let lowered = text.to_lowercase();
    let mut distinct_terms = 0u32;
    let mut occurrences = 0u32;
    for term in terms {
        let count = lowered.matches(term.as_str()).count() as u32;
        if count > 0 {
            distinct_terms += 1;
            occurrences += count;
        }
    }
    let relative = relative(root, path);
    Some(Match {
        url: file_url(path),
        title: title_of(&text, path),
        text,
        declared_date: manifest.dates.get(&relative).cloned(),
        licence: manifest.licences.get(&relative).cloned(),
        governance: manifest.documents.get(&relative).cloned(),
        relative,
        distinct_terms,
        occurrences,
    })
}

/// A document's display title: its first Markdown heading, else its file stem.
fn title_of(text: &str, path: &Path) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix("# ").map(str::trim).map(str::to_owned))
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned()
        })
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn file_url(path: &Path) -> String {
    url::Url::from_file_path(path)
        .map(String::from)
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}
