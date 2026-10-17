//! The display tool, `context_show`: a reply rendered against a signed
//! release this session's `context_fetch` delivered.
//!
//! The reply arrives as items that say what they are (a quote by passage id,
//! a sentence drawn from the source with the passages it cites, a sentence
//! of the assistant's own). The integrity checker
//! (`commonmeasure_runtime::processor::integrity`) gives each one outcome,
//! and the display is built from the release's text and those outcomes
//! alone: a sentence of the reply appears only where the rule shows it, and
//! always under the label the rule gave it.
//!
//! The release is the one the release verifier read on the fetch path, held
//! in memory by the server that delivered it. Nothing is fetched or verified
//! again here, and a release from any other session or server is not known.
//! What is held is the delivered text and its sidecar, which the session's
//! screens already ruled on; nothing of it is written anywhere new.

use std::collections::{HashSet, VecDeque};

use commonmeasure_runtime::processor::integrity::{Block, Rendered};
use commonmeasure_runtime::processor::provenance::{
    DeclaredMining, DisplayIngredient, Grade, IngredientTerms, Permission, Reporting,
    SourceManifest, Usage,
};
use commonmeasure_runtime::processor::release::Read;
use commonmeasure_types::canonical::canonical_digest;
use commonmeasure_types::display::{Counts, Item, Reply};
use commonmeasure_types::release::{C2paState, UsageTerm, VerificationState};
use serde_json::{Value, json};

use crate::declarations::{Category, Effective};
use crate::discovery::Declarations;

/// The releases one server holds for display, newest last, counted by
/// release identity. A release past this many is let go, and a display
/// against it is refused until it is fetched again.
pub const HELD_RELEASES: usize = 16;

/// The most items one display takes. The checker's word matching is
/// quadratic in a sentence's words and runs synchronously.
pub const MAX_ITEMS: usize = 64;

/// The most characters one item's sentence or passage id may hold.
pub const MAX_ITEM_CHARS: usize = 2_000;

/// What the counts are: a working rule Common Measure follows and the open
/// standards bodies hold (`docs/contracts/display-counts.md`).
pub const COUNTS_STATUS: &str = "working_rule";

/// One release delivered by this session's `context_fetch`.
#[derive(Debug)]
pub struct Held {
    /// The release as the verifier read it, with its verification state.
    pub read: Read,
    /// The URL the release was served at, which the checker's record names.
    pub url: String,
    /// The acquisition handles of the fetches that delivered this release.
    pub acquisition_ids: Vec<String>,
    /// What the crossing that delivered it recorded of the source's
    /// declarations and the operator's terms.
    pub declared: Declared,
    /// What makes two deliveries the same release ([`Identity`]).
    identity: Identity,
}

/// What a crossing recorded of the source's declarations and the operator's
/// terms for the host, kept with a held release for the terms its display's
/// manifest carries (`docs/contracts/display-provenance.md` §Terms).
#[derive(Debug, Clone, Default)]
pub struct Declared {
    /// The effective answer per category, the RSL licences read and the
    /// `Content-Usage` header, as the crossing recorded them.
    pub summary: Value,
    /// The operator's terms reference for the host and whether it requires
    /// reporting. Never the institution identifiers or the Anchor.
    pub host_policy_terms: Option<Value>,
    /// AI input as the declarations state it; `None` where none does.
    pub ai_input: Option<Permission>,
    /// Training as the declarations state it; `None` where none does.
    pub ai_training: Option<Permission>,
    /// Whether an RSL licence or the operator's terms require reporting.
    pub reporting_required: bool,
    /// Every reporting endpoint an RSL licence names.
    pub reporting_destinations: Vec<String>,
}

impl Declared {
    /// The canonical digest of everything here, which the display's
    /// ingredient terms are built from.
    fn digest(&self) -> String {
        canonical_digest(&json!({
            "summary": self.summary,
            "host_policy_terms": self.host_policy_terms,
            "ai_input": self.ai_input,
            "ai_training": self.ai_training,
            "reporting_required": self.reporting_required,
            "reporting_destinations": self.reporting_destinations,
        }))
    }

    /// What `declarations` say, as the display's ingredient terms read them.
    pub fn of(declarations: &Declarations) -> Self {
        let stated = |category: Category| match declarations.effective.get(&category) {
            Some(Effective::Allow) => Some(Permission::Allow),
            Some(Effective::Disallow) => Some(Permission::Disallow),
            Some(Effective::Unknown) | None => None,
        };
        let reporting: Vec<_> = declarations
            .licences
            .iter()
            .filter_map(|licence| licence.terms.as_ref())
            .flat_map(|terms| terms.reporting.iter())
            .collect();
        let host_requires = declarations
            .terms
            .as_ref()
            .is_some_and(|terms| terms.requires_reporting);
        let mut destinations: Vec<String> = Vec::new();
        for endpoint in reporting.iter().filter_map(|r| r.endpoint.as_ref()) {
            if !destinations.contains(endpoint) {
                destinations.push(endpoint.clone());
            }
        }
        Self {
            summary: json!({
                "effective": declarations.effective,
                "licences": declarations
                    .licences
                    .iter()
                    .map(|licence| licence.url.as_str())
                    .collect::<Vec<_>>(),
                "content_usage": declarations.content_usage_header,
            }),
            host_policy_terms: declarations.terms.as_ref().map(|terms| {
                json!({
                    "reference": terms.reference,
                    "requires_reporting": terms.requires_reporting,
                })
            }),
            ai_input: stated(Category::AiInput),
            ai_training: stated(Category::TrainAi),
            reporting_required: !reporting.is_empty() || host_requires,
            reporting_destinations: destinations,
        }
    }
}

/// Two deliveries are one held delivery only where everything the display
/// reads from them is the same: the text, the sidecar as signed and served,
/// the verification state, the URL the checker's record names, and the
/// declarations and host terms the crossing recorded. The text alone is not
/// enough: the same words under another display mode, other passages or
/// markers, or another signature display differently; the same release
/// fetched under another `Content-Usage` header or other terms is signed
/// under those terms; and a handle keeps the delivery its fetch made.
#[derive(Debug, PartialEq, Eq)]
struct Identity {
    content_hash: String,
    sidecar_digest: String,
    verification: VerificationState,
    url: String,
    declared_digest: String,
}

impl Identity {
    fn of(read: &Read, url: &str, declared: &Declared) -> Self {
        let sidecar = serde_json::to_value(&read.release.sidecar).expect("a sidecar serialises");
        Self {
            content_hash: read.content_hash.clone(),
            sidecar_digest: canonical_digest(&sidecar),
            verification: read.record.verification,
            url: url.to_owned(),
            declared_digest: declared.digest(),
        }
    }
}

/// The deliveries a display can name, by content hash and acquisition
/// handle. Held in memory for the server's life and never written.
#[derive(Debug, Default)]
pub struct Delivered {
    releases: VecDeque<Held>,
    other: HashSet<String>,
    let_go: HashSet<String>,
}

impl Delivered {
    /// Hold a delivered release. The same release fetched again under the
    /// same declarations and terms is held once, under every handle that
    /// delivered it; a different release with the same text, or the same
    /// release under other declarations or terms, is held beside it, and no
    /// handle ever moves from the delivery its fetch made or takes another
    /// delivery's terms.
    pub fn hold_release(
        &mut self,
        read: Read,
        url: &str,
        acquisition_id: Option<String>,
        declared: Declared,
    ) {
        let identity = Identity::of(&read, url, &declared);
        let held = match self
            .releases
            .iter()
            .position(|held| held.identity == identity)
            .and_then(|at| self.releases.remove(at))
        {
            Some(mut earlier) => {
                earlier.acquisition_ids.extend(acquisition_id);
                earlier
            }
            None => Held {
                read,
                url: url.to_owned(),
                acquisition_ids: acquisition_id.into_iter().collect(),
                declared,
                identity,
            },
        };
        self.let_go.remove(&held.read.content_hash);
        for id in &held.acquisition_ids {
            self.let_go.remove(id);
        }
        self.releases.push_back(held);
        while self.releases.len() > HELD_RELEASES {
            if let Some(oldest) = self.releases.pop_front() {
                self.let_go.insert(oldest.read.content_hash);
                self.let_go.extend(oldest.acquisition_ids);
            }
        }
    }

    /// Note a delivery that was not a signed release, so a display naming it
    /// is told why it cannot be rendered.
    pub fn note_other(&mut self, content_hash: &str, acquisition_id: Option<&str>) {
        self.other.insert(content_hash.to_owned());
        self.other.extend(acquisition_id.map(str::to_owned));
    }

    /// The release `key` names, by acquisition handle or content hash, or
    /// why there is none to display against. A handle names the one release
    /// its fetch delivered. A content hash names a release only while one
    /// held release has that text; where several do, the hash is refused as
    /// ambiguous rather than one of them chosen, and nothing is displayed
    /// or signed for it.
    pub fn release(&self, key: &str) -> Result<&Held, String> {
        if let Some(held) = self
            .releases
            .iter()
            .find(|held| held.acquisition_ids.iter().any(|id| id == key))
        {
            return Ok(held);
        }
        let by_text: Vec<&Held> = self
            .releases
            .iter()
            .filter(|held| held.read.content_hash == key)
            .collect();
        match by_text.as_slice() {
            [held] => return Ok(held),
            [] => {}
            several => {
                let handles: Vec<&str> = several
                    .iter()
                    .flat_map(|held| held.acquisition_ids.iter().map(String::as_str))
                    .collect();
                let mut refusal = format!(
                    "refused: this server holds {} different deliveries of a signed release \
                     with that content_hash (the same text under another sidecar, signature or \
                     URL, or fetched under other declarations or terms), so the hash does not \
                     say which to display against or whose terms to sign; name the release by \
                     the acquisition_id its context_fetch returned",
                    several.len()
                );
                if !handles.is_empty() {
                    refusal.push_str(&format!(" ({})", handles.join(", ")));
                }
                return Err(refusal);
            }
        }
        Err(if self.let_go.contains(key) {
            format!(
                "refused: this server holds the last {HELD_RELEASES} signed releases it \
                 delivered and has let that one go; fetch it again with context_fetch and \
                 display against the new result"
            )
        } else if self.other.contains(key) {
            "refused: what this session's context_fetch delivered under that acquisition_id or \
             content_hash was not served as a signed release, so there are no signed passages \
             or markers to display a reply against"
                .to_owned()
        } else {
            "refused: no context_fetch in this session delivered a signed release under that \
             acquisition_id or content_hash; a display renders only against a release this \
             server fetched and verified"
                .to_owned()
        })
    }
}

/// The `release` argument and the reply, or why the call cannot be read.
/// The error names the argument and never quotes it.
pub fn arguments(arguments: &Value) -> Result<(String, Reply), String> {
    let release = arguments
        .get("release")
        .and_then(Value::as_str)
        .filter(|release| !release.is_empty())
        .ok_or(
            "release is required: the acquisition_id or the content_hash a context_fetch in this \
             session returned for a signed release",
        )?;
    let items = arguments
        .get("items")
        .and_then(Value::as_array)
        .ok_or("items is required: the reply as a list of quote, from_source and own items")?;
    if items.is_empty() {
        return Err("items is empty: a display needs at least one item".to_owned());
    }
    if items.len() > MAX_ITEMS {
        return Err(format!(
            "items holds {} items; one display takes at most {MAX_ITEMS}",
            items.len()
        ));
    }
    let mut reply = Reply { items: Vec::new() };
    for (index, item) in items.iter().enumerate() {
        let item: Item = serde_json::from_value(item.clone()).map_err(|_| {
            format!(
                "items[{index}] is not an item: give kind \"quote\" with passage (and optionally \
                 sentence), kind \"from_source\" with text (and optionally passages), or kind \
                 \"own\" with text"
            )
        })?;
        let longest = match &item {
            Item::Quote { passage, .. } => passage.chars().count(),
            Item::FromSource { text, passages } => passages
                .iter()
                .map(|id| id.chars().count())
                .chain([text.chars().count()])
                .max()
                .unwrap_or(0),
            Item::Own { text } => text.chars().count(),
        };
        if longest > MAX_ITEM_CHARS {
            return Err(format!(
                "items[{index}] holds a string of {longest} characters; an item's sentence or \
                 passage id holds at most {MAX_ITEM_CHARS}"
            ));
        }
        reply.items.push(item);
    }
    Ok((release.to_owned(), reply))
}

/// Why a release that was delivered cannot be displayed, where it cannot:
/// the verifier did not verify its signature.
pub fn unverified(held: &Held) -> Option<&'static str> {
    match held.read.record.verification {
        VerificationState::Verified => None,
        VerificationState::Missing => Some("missing"),
        VerificationState::Failed => Some("failed"),
    }
}

/// What a `context_fetch` result says of displaying the release it
/// delivered: where `context_show` can, and for how long the server holds
/// it, or why it will refuse.
pub fn availability(verification: VerificationState) -> String {
    match verification {
        VerificationState::Verified => format!(
            "context_show can display a reply against this release by its acquisition_id or \
             content_hash while this server process holds it: the last {HELD_RELEASES} \
             releases it delivered, in memory only, cleared when the process exits"
        ),
        VerificationState::Missing | VerificationState::Failed => {
            "context_show refuses this release: its signature did not verify".to_owned()
        }
    }
}

/// The counts as the result and the record carry them: names and vector in
/// the order `docs/contracts/display-counts.md` sets, and the rule's status.
pub fn counts(counts: &Counts) -> Value {
    json!({
        "names": Counts::NAMES,
        "vector": counts.as_vec(),
        "status": COUNTS_STATUS,
    })
}

/// A displayed block as the structured payload carries it: the rule's
/// outcome and label, the text shown, the signed passage and span it comes
/// from and the passages a confirmed sentence links to. A passage id the
/// release did not sign is the reply's own and is not carried.
fn block(read: &Read, block: &Block) -> Value {
    let mut shown = json!({
        "outcome": block.outcome,
        "label": block.label,
        "text": block.text,
        "reason": block.reason,
    });
    if let Some(passage) = block
        .passage
        .as_deref()
        .filter(|id| read.release.passage(id).is_some())
    {
        shown["passage"] = json!(passage);
    }
    if let Some(span) = block.span {
        shown["span"] = json!(span);
    }
    if !block.sources.is_empty() {
        shown["sources"] = json!(block.sources);
    }
    shown
}

/// The structured payload of a display.
pub fn payload(held: &Held, rendered: &Rendered) -> Value {
    json!({
        "release": {
            "content_hash": held.read.content_hash,
            "acquisition_ids": held.acquisition_ids,
            "verification": held.read.record.verification,
        },
        "display_mode": rendered.display_mode,
        "blocks": rendered.blocks.iter().map(|b| block(&held.read, b)).collect::<Vec<_>>(),
        "counts": counts(&rendered.counts),
    })
}

/// The display as plain text: one paragraph per item, each opening with its
/// label in brackets and the passages it comes from or links to, then the
/// counts. A refused quote shows its label and nothing else.
pub fn plain(held: &Held, rendered: &Rendered) -> String {
    let mut out = format!(
        "Display of the signed release {} ({} display)\n",
        held.read.content_hash, rendered.display_mode
    );
    for b in &rendered.blocks {
        let passages: Vec<&str> = match b
            .passage
            .as_deref()
            .filter(|id| held.read.release.passage(id).is_some())
        {
            Some(id) => vec![id],
            None => b.sources.iter().map(|s| s.passage.as_str()).collect(),
        };
        out.push('\n');
        out.push('[');
        out.push_str(&b.label);
        if !passages.is_empty() {
            out.push_str("; ");
            out.push_str(&passages.join(", "));
        }
        out.push(']');
        if !b.text.is_empty() {
            out.push(' ');
            out.push_str(&b.text);
        }
        out.push('\n');
    }
    let named = Counts::NAMES
        .iter()
        .zip(rendered.counts.as_vec())
        .map(|(name, count)| format!("{name} {count}"))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("\nCounts ({COUNTS_STATUS}): {named}\n"));
    out
}

/// The `display_rendered` record: the integrity checker's record by its
/// `seq`, the output-provenance record by its `seq` where one was written,
/// the hash of the display text the manifest binds, the release by hash and
/// handle, each item's outcome and the counts. No sentence, passage text or
/// reason.
pub fn record(
    held: &Held,
    rendered: &Rendered,
    processor_record: u64,
    provenance_record: Option<u64>,
    display_text_sha256: &str,
) -> Value {
    json!({
        "tool": "context_show",
        "processor_record": processor_record,
        "provenance_record": provenance_record,
        "display_text_sha256": display_text_sha256,
        "release": {
            "content_hash": held.read.content_hash,
            "acquisition_ids": held.acquisition_ids,
        },
        "display_mode": rendered.display_mode,
        "outcomes": rendered.blocks.iter().map(|b| b.outcome).collect::<Vec<_>>(),
        "counts": counts(&rendered.counts),
    })
}

/// The held release as a display manifest's ingredient: by URL, content
/// hash, grade and sidecar digest, with the terms it was admitted under and
/// its own C2PA store where the verifier found one that verifies. A
/// `context_fetch` delivery is always a mediated crossing.
pub fn ingredient(held: &Held, trust_anchors: Option<&str>) -> DisplayIngredient {
    let signed = &held.read.release.sidecar.signed;
    let permission = |term: UsageTerm| match term {
        UsageTerm::Allow => Permission::Allow,
        UsageTerm::Disallow => Permission::Disallow,
    };
    let usage = signed.usage.as_ref();
    let mut destinations: Vec<String> = usage
        .and_then(|usage| usage.reporting.as_ref())
        .and_then(|reporting| reporting.destination.clone())
        .into_iter()
        .collect();
    for destination in &held.declared.reporting_destinations {
        if !destinations.contains(destination) {
            destinations.push(destination.clone());
        }
    }
    let terms = IngredientTerms {
        licence: signed.licence.clone(),
        source_terms: json!({"usage": signed.usage, "terms": signed.terms}),
        declarations: held.declared.summary.clone(),
        host_policy_terms: held.declared.host_policy_terms.clone(),
        usage: Usage {
            ai_input: Permission::stated([
                usage.map(|usage| permission(usage.ai_input)),
                held.declared.ai_input,
            ]),
            onward_commercial_product: Permission::stated([
                usage.map(|usage| permission(usage.onward_commercial_product))
            ]),
            reporting: Reporting {
                required: usage
                    .and_then(|usage| usage.reporting.as_ref())
                    .is_some_and(|reporting| reporting.required)
                    || held.declared.reporting_required,
                destinations,
            },
        },
        mining: DeclaredMining {
            ai_training: Permission::stated([held.declared.ai_training]),
            ai_generative_training: Permission::stated([held.declared.ai_training]),
            data_mining: Permission::Unknown,
        },
    };
    DisplayIngredient {
        reference: held.url.clone(),
        content_hash: held.read.content_hash.clone(),
        grade: Grade::Mediated,
        sidecar_digest: held.identity.sidecar_digest.clone(),
        source_manifest: source_manifest(held, trust_anchors),
        terms,
    }
}

/// The release's own C2PA store, where the verifier recorded one that
/// verifies, with the reader's validation results for it.
fn source_manifest(held: &Held, trust_anchors: Option<&str>) -> Option<SourceManifest> {
    use base64::Engine as _;
    use commonmeasure_runtime::processor::provenance::read_store_with_trust;
    let recorded = held.read.record.c2pa.as_ref()?;
    let state = match recorded.state {
        C2paState::Invalid => return None,
        C2paState::Untrusted => "untrusted",
        C2paState::Trusted => "trusted",
    };
    let store = base64::engine::general_purpose::STANDARD
        .decode(held.read.release.sidecar.c2pa.as_deref()?)
        .ok()?;
    let reading = read_store_with_trust(&store, &held.read.release.text, trust_anchors).ok()?;
    Some(SourceManifest {
        store,
        state: state.to_owned(),
        validation_results: reading.validation_results(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_name_what_is_missing_and_never_quote_it() {
        let secret = "zephyr-lantern";
        for (given, expected) in [
            (json!({}), "release is required"),
            (json!({"release": "sha256:x"}), "items is required"),
            (
                json!({"release": "sha256:x", "items": []}),
                "items is empty",
            ),
            (
                json!({"release": "sha256:x", "items": [{"kind": "opinion", "text": secret}]}),
                "items[0] is not an item",
            ),
            (
                json!({"release": "sha256:x", "items": [{"kind": "own", "text": secret.repeat(200)}]}),
                "items[0] holds a string of 2800 characters",
            ),
        ] {
            let error = arguments(&given).unwrap_err();
            assert!(error.starts_with(expected), "{error}");
            assert!(!error.contains(secret), "{error}");
        }
        let many = vec![json!({"kind": "own", "text": "a"}); MAX_ITEMS + 1];
        assert!(
            arguments(&json!({"release": "h", "items": many}))
                .unwrap_err()
                .contains("at most 64")
        );
        let (release, reply) = arguments(&json!({
            "release": "sha256:x",
            "items": [{"kind": "quote", "passage": "p0"}, {"kind": "own", "text": "b"}],
        }))
        .unwrap();
        assert_eq!(release, "sha256:x");
        assert_eq!(reply.items.len(), 2);
    }
}
