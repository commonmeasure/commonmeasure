use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use crate::Money;

/// What is known about the right to use a source.
///
/// `Unknown` is the honest state for every open-web provider probed on
/// 1 August 2026 (`docs/knowledge-base/provider-verification.md`): none returns
/// a licence, rights or author field. An adapter never infers permission from
/// the fact that a crawler could reach the page, so there is deliberately no
/// variant meaning "probably fine".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LicenceState {
    Unknown,
    /// The supplier named a machine-readable licence or agreement as it handed
    /// over the content.
    Declared {
        reference: String,
    },
}

/// One date as something declared it: the date text exactly as written, and
/// who declared it, so every figure later derived from the date can name its
/// provenance (`docs/contracts/run-output.md` §Plans, freshness).
///
/// A declared date is a claim, never an observation: a supplier's
/// `publishedDate` is the supplier's word, an operator's corpus declaration
/// is the operator's word, and nothing in this runtime verifies either
/// against when the content was actually written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredDate {
    /// The date exactly as declared — an ISO calendar date or a longer
    /// timestamp. Kept verbatim even where it does not parse, so the record
    /// shows what was trusted, or what could not be.
    pub date: String,
    /// Who declared it, named for the evidence record: supplier-declared
    /// (naming the provider and field), operator-declared (naming the corpus
    /// manifest), or the replay manifest's capture date.
    pub provenance: String,
}

/// Whether a charge was seen or merely published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargeBasis {
    /// The provider reported this charge on the response to this request.
    Observed,
    /// Taken from the provider's published price. No charge was reported.
    Quoted,
}

/// A charge in the provider's own unit.
///
/// Providers do not share one unit: Exa reports decimal USD, Firecrawl integer
/// credits, Tavily nothing at all. `amount` keeps the provider's own number
/// verbatim so that a display can never silently convert credits into currency,
/// which is the false comparability `docs/contracts/experiment.md` forbids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeCharge {
    pub unit: String,
    pub amount: Number,
    pub basis: ChargeBasis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What one acquisition cost, as far as it was observed.
///
/// Both fields absent means the provider disclosed no price. That is unknown,
/// never zero: a plan whose cost is plotted at zero puts the provider that
/// never told you its price at the cheapest point on the frontier.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionCharge {
    /// Present only when the provider reported the charge in a currency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub money: Option<Money>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeCharge>,
}

/// Normalised supply: one source as it entered the runtime.
///
/// This is the `ContextEnvelope` of `ARCHITECTURE.md` §Core objects. It is
/// produced by a supply adapter from bytes that crossed the real
/// transport boundary and is the only shape policy and inference ever see, so
/// two providers' results are comparable without either one's wire format
/// leaking into the decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextEnvelope {
    pub source_url: String,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The text the provider returned, if it returned any. A search response
    /// that carries no excerpt leaves this `None` rather than an empty string,
    /// because "returned nothing" and "returned emptiness" are different facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// SHA-256 of `text` as retrieved, before any admission truncation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub licence: LicenceState,
    /// The date declared for this source, if anything declared one. Mapped by
    /// an adapter only from a field its supplier has actually been seen to
    /// return (or, for the internal corpus, from the operator's own
    /// manifest); absent for every source nothing dated. Absence is not
    /// staleness: an undated source's freshness is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_date: Option<DeclaredDate>,
    /// Provider-specific fields kept namespaced rather than normalised, so a
    /// provider's own score never masquerades as a cross-provider measure.
    pub native_metadata: Value,
    /// Position in the provider's own result order, 1-based.
    pub retrieval_rank: u32,
}
