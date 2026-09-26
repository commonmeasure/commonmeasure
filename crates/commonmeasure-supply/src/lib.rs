//! Supply adapters.
//!
//! One adapter per provider, each implementing only the capabilities it
//! actually offers (`docs/contracts/provider.md`). Every remote adapter
//! speaks its provider's real wire format over [`commonmeasure_http`]; the internal
//! adapter reads the operator's own corpus from disk; the skill adapter
//! executes a bundle the operator's catalogue declares. All of them return
//! both the normalised [`ContextEnvelope`]s and the exact response bytes that
//! produced them, so a reviewer can check the parse rather than trust it.
//!
//! One module per provider, no supply-class directories: grouping adapters by
//! `open-web/` versus `licensed/` reads as a trust score and is not one; the
//! supply class belongs in the adapter's declared capabilities and its
//! verification state (`docs/contracts/provider.md` §Verification states),
//! where the runtime can act on it. An adapter declares only the capabilities
//! it implements, never the ones its vendor documents, and no adapter is
//! described in a state above `spec-verified` without a dated artefact
//! supporting the claim.
//!
//! There is no transport seam and no fixture adapter. A test that needs a
//! provider's recorded response starts a real loopback server, serves those
//! bytes, and points `base_url` at it; the client, the framing and the parser
//! under test are then the ones a live run uses. Recorded replay enters
//! through the same boundary. A skill has no recorded form at all: a produced
//! result is not a response anybody published, so the skill tests execute
//! real code under the real limits and the live path is the only path.

pub mod credentials;
mod dataville;
mod exa;
mod firecrawl;
mod internal;
mod keenable;
mod linkup;
mod nimble;
mod ozone;
mod parallel;
mod redpine;
mod search1api;
mod serpdive;
mod skill;
mod tavily;
mod tinyfish;
mod tollbit;
mod you;

pub use dataville::DatavilleAdapter;
pub use exa::ExaAdapter;
pub use firecrawl::FirecrawlAdapter;
pub use internal::{CORPUS_VARIABLE, InternalCorpusAdapter};
pub use keenable::KeenableAdapter;
pub use linkup::LinkupAdapter;
pub use nimble::NimbleAdapter;
pub use ozone::OzoneAdapter;
pub use parallel::ParallelAdapter;
pub use redpine::RedpineAdapter;
pub use search1api::Search1ApiAdapter;
pub use serpdive::SerpdiveAdapter;
pub use skill::{
    CATALOGUE_VARIABLE, INTERNAL_PROVIDER, Invocation, LocalSkillAdapter, SKILL_PROVIDER_PREFIX,
    SkillCatalogue,
};
pub use tavily::TavilyAdapter;
pub use tinyfish::TinyfishAdapter;
pub use tollbit::TollbitAdapter;
pub use you::YouAdapter;

use commonmeasure_types::canonical::sha256_digest;
use commonmeasure_types::{
    AcquisitionCharge, ChargeBasis, ContextEnvelope, DeclaredDate, LicenceState, Money,
    NativeCharge, ProviderCapability, ProviderRef,
};
use serde_json::{Number, Value};

/// Adapter revision recorded on every plan and envelope. Bump it whenever a
/// parser changes, because a normalised envelope is only interpretable
/// alongside the code that produced it.
pub const ADAPTER_VERSION: &str = "commonmeasure-supply/0.1";

/// Content providers with an implemented adapter, in a fixed order so that a
/// run's plan list does not depend on map iteration. `internal` is the
/// operator's own corpus (`query`); `redpine` is a licensed supplier bought by
/// quote then confirm; `ozone` retrieves from a licensed publisher corpus with
/// no quote gate; `dataville` searches public-source catalogues; the rest are
/// open-web providers (`search`).
///
/// Skill providers are deliberately absent and cannot be added: each one is
/// named for a bundle the operator's catalogue declares at run time
/// ([`SKILL_PROVIDER_PREFIX`]), so the set is not knowable at compile time
/// and a list that pretended otherwise would be a list of somebody else's
/// machine.
pub const IMPLEMENTED_PROVIDERS: [&str; 16] = [
    "dataville",
    "exa",
    "firecrawl",
    "internal",
    "keenable",
    "linkup",
    "nimble",
    "ozone",
    "parallel",
    "redpine",
    "search1api",
    "serpdive",
    "tavily",
    "tinyfish",
    "tollbit",
    "you",
];

/// One acquisition attempt that reached a provider and came back.
///
/// Everything here is observed. Fields a provider did not report are `None`,
/// never a default: `docs/contracts/experiment.md` forbids scoring an absent
/// charge as a cheap one.
#[derive(Debug)]
pub struct Acquisition {
    pub provider: String,
    pub capability: ProviderCapability,
    /// The URL actually called, credential-free. For a local corpus this is
    /// the corpus root as a `file://` URL.
    pub endpoint: String,
    /// `None` when no HTTP transport was involved — a local corpus query has
    /// no status code, and inventing a `200` would record a response nobody
    /// sent.
    pub http_status: Option<u16>,
    /// Wall-clock time this runtime measured around the request.
    pub latency_ms: u64,
    pub provider_request_id: Option<String>,
    pub charge: AcquisitionCharge,
    pub envelopes: Vec<ContextEnvelope>,
    /// Exactly what the provider returned, before parsing. The runtime seals
    /// this alongside the run so a reviewer can re-derive the envelopes and
    /// check the content hashes against their input.
    pub raw_response: Vec<u8>,
    /// Present only on an `invoke` acquisition: what was executed, under what
    /// declaration, and what it exited with. `None` for every content
    /// acquisition — nothing was executed, and an empty invocation record
    /// would suggest something was.
    pub invocation: Option<Invocation>,
}

/// A supplier's answer to "what would this acquisition cost?", obtained
/// before anything is bought.
///
/// Everything public here is observed from the quote exchange and enters the
/// plan evidence *before* the purchase decision — that ordering is the whole
/// point of a quote-then-buy supplier. Fields the provider did not report are
/// `None`, never a default. The continuation state a confirm needs (the
/// provider session) is private: like a credential, it travels only in
/// request headers and never into an artefact.
pub struct SupplyQuote {
    pub provider: String,
    /// The URL the quote legs were sent to, credential-free.
    pub endpoint: String,
    /// The provider's handle for unlocking this quote's result.
    pub preview_id: String,
    /// The provider's own workflow word for what happens next
    /// (`requires_confirm` is the only recorded value; `complete` is
    /// documented to mean the data is free and already unlocked).
    pub workflow: String,
    /// How the provider says this purchase would be billed, verbatim
    /// (`trial` is the only recorded value).
    pub billing_status: Option<String>,
    /// The provider's own sentence about what confirming will consume.
    pub billing_note: Option<String>,
    pub trial_remaining: Option<u64>,
    pub trial_total: Option<u64>,
    /// The account balance before any purchase, verbatim as the provider
    /// reported it.
    pub balance_before: Option<String>,
    /// The quoted price, in the provider's own unit, basis `Quoted`.
    pub charge: AcquisitionCharge,
    /// Wall-clock time across all quote legs, measured by this runtime.
    pub latency_ms: u64,
    /// Exactly what the provider returned on the leg that priced the tool,
    /// before parsing; sealed beside the run like a response.
    pub raw_inspect: Vec<u8>,
    /// Exactly what the provider returned on the preview leg, before parsing.
    pub raw_preview: Vec<u8>,
    /// The provider session the confirm leg must present. Deliberately
    /// crate-private and absent from `Debug`: the recon captures redact it on
    /// both sides, and evidence must too.
    pub(crate) session: String,
}

impl std::fmt::Debug for SupplyQuote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SupplyQuote")
            .field("provider", &self.provider)
            .field("endpoint", &self.endpoint)
            .field("preview_id", &self.preview_id)
            .field("workflow", &self.workflow)
            .field("billing_status", &self.billing_status)
            .field("billing_note", &self.billing_note)
            .field("trial_remaining", &self.trial_remaining)
            .field("trial_total", &self.trial_total)
            .field("balance_before", &self.balance_before)
            .field("charge", &self.charge)
            .field("latency_ms", &self.latency_ms)
            .field("session", &"«redacted»")
            .finish_non_exhaustive()
    }
}

/// Why an acquisition did not happen or did not produce usable supply.
///
/// Every variant is a recordable gap. None of them has a "carry on with less"
/// reading: a missing credential produces an unavailable plan, not a quieter
/// success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupplyError {
    /// A variable the adapter needs is not configured: an API credential for
    /// a remote provider, or the corpus root for the internal one.
    CredentialMissing { variable: String },
    /// The request never completed.
    Transport { detail: String },
    /// The provider answered, and refused or failed.
    Status {
        status: u16,
        endpoint: String,
        /// A bounded excerpt of the provider's own error body. Providers put
        /// the reason here (TollBit's `403` names a publisher disallowance),
        /// and a status code alone would lose it.
        detail: String,
    },
    /// The provider answered with something this adapter cannot read.
    Malformed { detail: String },
    /// A declared entrypoint ran and produced nothing this runtime will offer
    /// as supply: the enforced wall-clock timeout elapsed and the child was
    /// killed, or its output passed the declared cap and the result is
    /// refused whole.
    ///
    /// A non-zero exit status is deliberately not this. A validator exiting
    /// `1` with a named cause has answered the job, and recording that answer
    /// as a run error would lose it.
    Execution { detail: String },
    /// The adapter does not implement the requested capability. Plans are
    /// validated against declared capabilities before execution, so reaching
    /// this means a caller bypassed the plan; it is a backstop, never a mode.
    CapabilityUnavailable {
        provider: String,
        capability: ProviderCapability,
    },
}

impl std::fmt::Display for SupplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CredentialMissing { variable } => {
                write!(f, "required variable {variable} is not configured")
            }
            Self::Transport { detail } => write!(f, "transport failed: {detail}"),
            Self::Status {
                status,
                endpoint,
                detail,
            } => write!(f, "provider answered {status} for {endpoint}: {detail}"),
            Self::Malformed { detail } => write!(f, "unreadable provider response: {detail}"),
            Self::Execution { detail } => {
                write!(f, "the invocation produced no usable result: {detail}")
            }
            Self::CapabilityUnavailable {
                provider,
                capability,
            } => write!(
                f,
                "provider {provider} does not implement {capability:?}, and its adapter declares \
                 only what it implements"
            ),
        }
    }
}

impl std::error::Error for SupplyError {}

/// A provider that can supply something a job needs.
///
/// An adapter overrides exactly the operations its declared capabilities
/// name; the defaults answer for everything it does not implement. `search`
/// discovers candidates on the open web; `query` retrieves directly from a
/// bounded corpus the operator owns; `invoke` executes a declared entrypoint
/// and returns what it produced. Adding `fetch` means another method here and
/// another declared capability on the provider reference; it does not mean
/// another envelope shape or another decision path.
///
/// Content and skills travel one dispatch path with disjoint declared
/// capabilities, which keeps content and skill suppliers from being treated
/// as equivalent in code: a skill adapter never implements `search`, `query`
/// or `quote`, so nothing can ask a local execution for search semantics, a
/// supplier price or a licence it has no way to state.
pub trait SupplyAdapter {
    /// This adapter's provider name, as policy, plans and the router use it.
    /// Borrowed rather than `'static` because a skill provider is named for
    /// the skill it invokes (`skill:<name>`), which the operator's catalogue
    /// declares at run time and no literal in this crate can spell.
    fn provider(&self) -> &str;
    /// The capabilities this adapter implements, which is what a plan is
    /// validated against. It is never the capability list of the provider's
    /// documentation.
    fn capabilities(&self) -> &'static [ProviderCapability];

    /// Discover candidates on the open web.
    ///
    /// `include_hosts` is the job's allowed source hosts, passed so an adapter
    /// that supports provider-side domain scoping can restrict the query to
    /// them. It is a hint, not the enforcement: admission still checks every
    /// returned source against the operator's host policy, so an adapter that
    /// cannot scope — or a provider that ignores the filter — changes nothing
    /// about what is admitted, only what was fetched and paid for. An empty
    /// slice means the job named no allowed hosts and the search is unscoped.
    fn search(
        &self,
        _query: &str,
        _limit: u32,
        _include_hosts: &[&str],
    ) -> Result<Acquisition, SupplyError> {
        Err(SupplyError::CapabilityUnavailable {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Search,
        })
    }

    /// The most results this provider's `search` will return, where it
    /// publishes a ceiling below what a job may ask for.
    ///
    /// `None` means no published ceiling. Dataville caps its response at one,
    /// Ozone at 100 and TollBit at 20. A job asking for more gets the
    /// cap, so the runtime records the shortfall as a gap naming both
    /// numbers rather than letting a provider comparison run at two sizes
    /// with nothing saying so.
    fn maximum_search_results(&self) -> Option<u32> {
        None
    }

    fn query(&self, _query: &str, _limit: u32) -> Result<Acquisition, SupplyError> {
        Err(SupplyError::CapabilityUnavailable {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Query,
        })
    }

    /// Retrieve a single named URL and return what the provider extracted from
    /// it. Unlike `search`, which discovers candidates for a query, `fetch`
    /// names the target: the URL is the whole request, so there is no query and
    /// no result limit. A provider that answers the transport but cannot serve
    /// the URL — an origin `403`, a paywall — returns an acquisition carrying no
    /// admissible envelope with the provider's own per-URL error sealed in the
    /// raw response, never a fabricated body.
    fn fetch(&self, _url: &str) -> Result<Acquisition, SupplyError> {
        Err(SupplyError::CapabilityUnavailable {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Fetch,
        })
    }

    /// Execute this supplier's declared entrypoint against `input` and return
    /// what it produced, whatever its exit status.
    ///
    /// There is no `limit`: an invocation produces one result, so the plan
    /// step's result limit does not govern it and the record says so rather
    /// than passing a number nothing reads. A non-zero exit is a result — the
    /// verdict a validator was asked for — and only failing to execute, an
    /// enforced timeout or a result over the declared cap is an error here.
    fn invoke(&self, _input: &str) -> Result<Acquisition, SupplyError> {
        Err(SupplyError::CapabilityUnavailable {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Invoke,
        })
    }

    /// The provider's published price for one call of `capability`, where a
    /// spec-verified one exists, in currency. `None` — the default — means no
    /// price is declared, not that the call is free.
    ///
    /// This is a declaration, not a quote: no request is made and the
    /// provider is not consulted. It exists for the allowance ledger, which
    /// reserves the declared price before dispatch and reconciles against the
    /// observed receipt. Implement it only where the published price has been
    /// verified against a dated observed charge — a stale figure here would
    /// reserve the wrong amount silently, so each implementation names its
    /// evidence.
    fn published_price(
        &self,
        _capability: ProviderCapability,
    ) -> Option<commonmeasure_types::Money> {
        None
    }

    /// Price an acquisition without buying it. Implemented only by suppliers
    /// that genuinely return price/terms before delivery; the quote's charge
    /// and billing note enter plan evidence before any purchase decision.
    fn quote(&self, _query: &str, _limit: u32) -> Result<SupplyQuote, SupplyError> {
        Err(SupplyError::CapabilityUnavailable {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Quote,
        })
    }

    /// Buy what `quote` priced. The receipt's observed charge rides the
    /// returned acquisition; a caller that decided against the purchase
    /// records that decision instead of calling this.
    fn confirm(&self, _quote: &SupplyQuote) -> Result<Acquisition, SupplyError> {
        Err(SupplyError::CapabilityUnavailable {
            provider: self.provider().to_owned(),
            capability: ProviderCapability::Quote,
        })
    }
}

/// The environment variable `provider` needs before an adapter can exist:
/// an API credential for a remote provider, the corpus root for the internal
/// one, the catalogue path for a skill. `None` for an unimplemented provider.
///
/// This is the one mapping `supplier_from_environment` reads, so an
/// unavailable message, the credentials doctor and the adapter construction
/// can never name different variables.
pub fn required_variable(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "dataville" => "DATAVILLE_API_KEY",
        "exa" => "EXA_API_KEY",
        "firecrawl" => "FIRECRAWL_API_KEY",
        "parallel" => "PARALLEL_API_KEY",
        "keenable" => "KEENABLE_API_KEY",
        "linkup" => "LINKUP_API_KEY",
        "nimble" => "NIMBLE_API_KEY",
        "ozone" => "OZONE_LIVE_API_KEY",
        "search1api" => "SEARCH1API_API_KEY",
        "serpdive" => "SERPDIVE_API_KEY",
        "you" => "YOU_API_KEY",
        "tavily" => "TAVILY_API_KEY",
        "tinyfish" => "TINYFISH_API_KEY",
        "redpine" => "REDPINE_API_KEY",
        "tollbit" => "TOLLBIT_API_KEY",
        "internal" => CORPUS_VARIABLE,
        // A bare `skill:` names no bundle, so it is not a provider here
        // either — the same rule `declared_provider_ref` applies.
        skill
            if skill.starts_with(SKILL_PROVIDER_PREFIX)
                && skill.len() > SKILL_PROVIDER_PREFIX.len() =>
        {
            CATALOGUE_VARIABLE
        }
        _ => return None,
    })
}

/// The adapter for a remote provider, against `origin` or, where that is
/// `None`, the provider's own default origin.
///
/// The one place a remote provider's name is turned into an adapter. A live
/// run reaches it through [`supplier_from_environment`] with the operator's
/// credential; a replay run reaches it with a loopback origin and a
/// placeholder, so both build the same adapter over the same transport and a
/// provider cannot be wired into one path and not the other.
///
/// `None` for the internal corpus and for a skill: neither is a remote origin
/// with a credential, and neither can be replayed from a recorded response.
pub fn remote_adapter(
    provider: &str,
    origin: Option<&str>,
    credential: &str,
) -> Option<Box<dyn SupplyAdapter>> {
    let at = |default: &'static str| origin.unwrap_or(default).to_owned();
    Some(match provider {
        "dataville" => Box::new(DatavilleAdapter::new(
            &at(dataville::DEFAULT_BASE_URL),
            credential,
        )),
        "exa" => Box::new(ExaAdapter::new(&at(exa::DEFAULT_BASE_URL), credential)),
        "firecrawl" => Box::new(FirecrawlAdapter::new(
            &at(firecrawl::DEFAULT_BASE_URL),
            credential,
        )),
        "keenable" => Box::new(KeenableAdapter::new(
            &at(keenable::DEFAULT_BASE_URL),
            credential,
        )),
        "linkup" => Box::new(LinkupAdapter::new(
            &at(linkup::DEFAULT_BASE_URL),
            credential,
        )),
        "nimble" => Box::new(NimbleAdapter::new(
            &at(nimble::DEFAULT_BASE_URL),
            credential,
        )),
        "ozone" => Box::new(OzoneAdapter::new(&at(ozone::DEFAULT_BASE_URL), credential)),
        "parallel" => Box::new(ParallelAdapter::new(
            &at(parallel::DEFAULT_BASE_URL),
            credential,
        )),
        "redpine" => Box::new(RedpineAdapter::new(
            &at(redpine::DEFAULT_BASE_URL),
            credential,
        )),
        "search1api" => Box::new(Search1ApiAdapter::new(
            &at(search1api::DEFAULT_BASE_URL),
            credential,
        )),
        "serpdive" => Box::new(SerpdiveAdapter::new(
            &at(serpdive::DEFAULT_BASE_URL),
            credential,
        )),
        "tavily" => Box::new(TavilyAdapter::new(
            &at(tavily::DEFAULT_BASE_URL),
            credential,
        )),
        "tinyfish" => Box::new(TinyfishAdapter::new(
            &at(tinyfish::DEFAULT_BASE_URL),
            credential,
        )),
        "tollbit" => Box::new(TollbitAdapter::new(
            &at(tollbit::DEFAULT_BASE_URL),
            credential,
        )),
        "you" => Box::new(YouAdapter::new(&at(you::DEFAULT_BASE_URL), credential)),
        _ => return None,
    })
}

/// Build an adapter for `provider` from environment credentials.
///
/// The credential is read here and travels only in a request header. It is
/// never placed on a command line, in a URL or in any artefact.
pub fn supplier_from_environment(provider: &str) -> Result<Box<dyn SupplyAdapter>, SupplyError> {
    supplier_with_released(provider, None)
}

/// [`supplier_from_environment`], with a remote provider's credential taken
/// from `released` where the environment does not hold its variable
/// (`docs/contracts/supplier-credentials.md` §Edge side). The environment
/// wins, and the corpus root and the skill catalogue are never read from
/// `released`. With `None`, or a store holding nothing for the provider, this
/// is `supplier_from_environment`: an absent credential is
/// [`SupplyError::CredentialMissing`] naming the same variable.
pub fn supplier_with_released(
    provider: &str,
    released: Option<&credentials::ReleasedStore>,
) -> Result<Box<dyn SupplyAdapter>, SupplyError> {
    let local_only = provider == "internal" || provider.starts_with(SKILL_PROVIDER_PREFIX);
    let credential = |variable: &str| {
        credential_for(variable, released.filter(|_| !local_only), |name| {
            std::env::var(name).ok()
        })
    };
    // Every implemented provider names its variable in `required_variable`;
    // reading it through that mapping is what keeps the two in step.
    let needed = |named: &str| {
        credential(required_variable(named).expect("an implemented provider names its variable"))
    };
    match provider {
        // Not a credential but the same contract: absent configuration is an
        // explicit unavailable plan naming the variable, never a quiet skip.
        "internal" => Ok(Box::new(InternalCorpusAdapter::new(std::path::Path::new(
            &needed("internal")?,
        )))),
        // A skill is admitted by the operator's catalogue and by nothing
        // else. An unlisted name is refused here, before any adapter exists
        // to invoke it, which is the first of the two independent layers
        // between a job and a running child (the second is job policy, which
        // decides eligibility before this is ever called).
        skill if skill.starts_with(SKILL_PROVIDER_PREFIX) => {
            let name = &skill[SKILL_PROVIDER_PREFIX.len()..];
            let path = std::path::PathBuf::from(needed(skill)?);
            let catalogue = SkillCatalogue::load(&path)?;
            let declaration =
                catalogue
                    .skills
                    .get(name)
                    .cloned()
                    .ok_or_else(|| SupplyError::Malformed {
                        detail: format!(
                            "the skill catalogue {} declares no skill named {name}, so there is \
                             nothing this operator has admitted for provider {skill} to invoke",
                            path.display()
                        ),
                    })?;
            Ok(Box::new(LocalSkillAdapter::new(&path, name, declaration)))
        }
        remote => match required_variable(remote) {
            // The credential is read before the adapter is built, so an
            // unconfigured provider names its variable rather than producing
            // an adapter that could only fail on the wire.
            Some(_) => {
                let credential = needed(remote)?;
                remote_adapter(remote, None, &credential).ok_or_else(|| SupplyError::Malformed {
                    detail: format!("no adapter is implemented for provider {remote}"),
                })
            }
            None => Err(SupplyError::Malformed {
                detail: format!("no adapter is implemented for provider {remote}"),
            }),
        },
    }
}

/// The value for `variable`: the environment's where it holds a non-blank
/// one, otherwise the hub-released one, otherwise
/// [`SupplyError::CredentialMissing`]. The environment is asked through
/// `environment` so the precedence is testable without mutating process
/// state.
fn credential_for(
    variable: &str,
    released: Option<&credentials::ReleasedStore>,
    environment: impl Fn(&str) -> Option<String>,
) -> Result<String, SupplyError> {
    environment(variable)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| released.and_then(|store| store.value_of(variable)))
        .ok_or_else(|| SupplyError::CredentialMissing {
            variable: variable.to_owned(),
        })
}

/// The declared capabilities of an implemented provider, without needing a
/// credential to ask. Plan construction happens before credentials are
/// resolved, so an unconfigured provider still produces a plan that is
/// recorded as unavailable rather than silently dropped.
pub fn declared_provider_ref(provider: &str) -> Option<ProviderRef> {
    let capabilities: &[ProviderCapability] = match provider {
        "dataville" => dataville::CAPABILITIES,
        "exa" => exa::CAPABILITIES,
        "firecrawl" => firecrawl::CAPABILITIES,
        "internal" => internal::CAPABILITIES,
        "keenable" => keenable::CAPABILITIES,
        "linkup" => linkup::CAPABILITIES,
        "nimble" => nimble::CAPABILITIES,
        "ozone" => ozone::CAPABILITIES,
        "parallel" => parallel::CAPABILITIES,
        "redpine" => redpine::CAPABILITIES,
        "search1api" => search1api::CAPABILITIES,
        "serpdive" => serpdive::CAPABILITIES,
        "tavily" => tavily::CAPABILITIES,
        "tinyfish" => tinyfish::CAPABILITIES,
        "tollbit" => tollbit::CAPABILITIES,
        "you" => you::CAPABILITIES,
        // Every catalogued skill is invoked by one adapter, so the capability
        // is this crate's to declare and needs no catalogue to answer for —
        // which is what lets an unconfigured or unlisted skill still produce
        // a plan, recorded as unavailable naming what is missing, rather than
        // being dropped from the comparison before anyone sees it. A bare
        // `skill:` names nothing and is not a provider.
        skill
            if skill.starts_with(SKILL_PROVIDER_PREFIX)
                && skill.len() > SKILL_PROVIDER_PREFIX.len() =>
        {
            skill::CAPABILITIES
        }
        _ => return None,
    };
    Some(ProviderRef {
        name: provider.to_owned(),
        adapter_version: ADAPTER_VERSION.to_owned(),
        capabilities: capabilities.to_vec(),
    })
}

/// Send one request and measure it.
///
/// Latency is wall-clock around the exchange, which is the one timing figure
/// this runtime observes itself; provider-reported search times are kept in
/// native metadata rather than substituted for it. A non-2xx answer is an
/// error carrying the provider's own words, not a silent empty result.
pub(crate) fn execute(
    endpoint: &str,
    request: commonmeasure_http::Request,
) -> Result<(commonmeasure_http::Response, u64), SupplyError> {
    let started = std::time::Instant::now();
    let response =
        commonmeasure_http::send(endpoint, request).map_err(|error| SupplyError::Transport {
            detail: format!("{error:#}"),
        })?;
    let latency_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    // A provider's response is sealed as the exact bytes it served and
    // replayed by parsing those bytes, so a body the transport decoded from a
    // content coding is refused here rather than sealed as something the
    // provider did not send.
    if let Some(coded) = &response.coded {
        return Err(SupplyError::Transport {
            detail: format!(
                "{endpoint} answered with content encoding {}, which a sealed provider \
                 response does not accept",
                coded.coding
            ),
        });
    }
    if !(200..300).contains(&response.status) {
        return Err(SupplyError::Status {
            status: response.status,
            endpoint: endpoint.to_owned(),
            detail: error_detail(&response.body),
        });
    }
    Ok((response, latency_ms))
}

pub(crate) fn parse_json(body: &[u8]) -> Result<Value, SupplyError> {
    serde_json::from_slice(body).map_err(|error| SupplyError::Malformed {
        detail: format!("response is not valid JSON: {error}"),
    })
}

/// A field a 2xx body must carry for the adapter to read it, at `pointer`
/// (a JSON pointer such as `/results` or `/data/web`), or a `Malformed` error
/// naming the endpoint and the field.
///
/// Absent is not empty. A provider that renames or drops the field it once
/// answered under has changed its shape, and reading the reply as a successful
/// empty result would record a charge against nothing and hide the change —
/// the silently-weaker mode `docs/FAIL-POLICY.md` §5 forbids. The error makes
/// the run record a gap instead, and no acquisition (so no charge) is built.
pub(crate) fn required_field<'a>(
    body: &'a Value,
    pointer: &str,
    endpoint: &str,
) -> Result<&'a Value, SupplyError> {
    body.pointer(pointer).ok_or_else(|| SupplyError::Malformed {
        detail: format!(
            "the response from {endpoint} carries no `{}` field; an absent field is a changed \
             shape, not an empty result",
            pointer.trim_start_matches('/')
        ),
    })
}

/// The results array a 2xx body must carry, at `pointer`, or a `Malformed`
/// error naming the endpoint and the missing or mis-typed field. A present but
/// empty array is a legitimate empty result and is returned as such; see
/// [`required_field`] for why absence is not.
pub(crate) fn results_array<'a>(
    body: &'a Value,
    pointer: &str,
    endpoint: &str,
) -> Result<&'a [Value], SupplyError> {
    let field = required_field(body, pointer, endpoint)?;
    field
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| SupplyError::Malformed {
            detail: format!(
                "the response from {endpoint} carries `{}` as {}, not an array",
                pointer.trim_start_matches('/'),
                json_kind(field)
            ),
        })
}

/// The JSON type of a value, for an error that says what was found.
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Why a path under a declared root was not usable.
///
/// The containment rule both local suppliers hold to: a declared root is the
/// operator's declaration, and a symlink inside it is a pointer, not a
/// declaration. The corpus refuses to *read* through one; a skill
/// refuses to *execute* through one, which is the same rule with a larger
/// consequence. The fault is returned rather than worded here, because each
/// caller's record says what it declined to do.
pub(crate) enum Containment {
    Unresolvable(std::io::Error),
    Outside,
}

/// A path's canonical location, required to stay beneath the canonical root.
pub(crate) fn resolve_within(
    root: &std::path::Path,
    path: &std::path::Path,
) -> Result<std::path::PathBuf, Containment> {
    match path.canonicalize() {
        Err(error) => Err(Containment::Unresolvable(error)),
        Ok(resolved) if resolved.starts_with(root) => Ok(resolved),
        Ok(_) => Err(Containment::Outside),
    }
}

/// The host of a source URL, as source policy compares it.
///
/// `None` when the URL does not parse or names no host at all. That absence is
/// not the empty host: an internal corpus document's `file://` URL genuinely
/// has none, and a URL nobody could read must not arrive wearing the same
/// value — one string with two meanings turns "unreadable" into "the
/// operator's own corpus" under an allowlist. A result whose host is `None` is
/// not offered as supply at all: it cannot be checked against a host policy,
/// so admitting it would put a source in the context window that no rule was
/// ever applied to. It stays in the sealed response, which is the record.
///
/// One trailing dot is stripped. `banned.example.` and `banned.example` are
/// the same name to DNS, and the operator's deny list holds the one they
/// wrote; the dot is left in place when stripping it would leave nothing.
pub(crate) fn host_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    Some(
        host.strip_suffix('.')
            .filter(|rest| !rest.is_empty())
            .unwrap_or(host)
            .to_owned(),
    )
}

/// Provider-specific fields, kept whole and namespaced under the provider's own
/// keys. Fields promoted into the envelope are removed so that one fact does
/// not appear twice with two names.
pub(crate) fn native_metadata(item: &Value, promoted: &[&str]) -> Value {
    let mut native = item.as_object().cloned().unwrap_or_default();
    for key in promoted {
        native.remove(*key);
    }
    Value::Object(native)
}

/// The fields an adapter reads off one result in a provider's results array,
/// in the provider's own field names, before the result becomes an envelope.
///
/// Each adapter owns exactly this mapping — which field is the URL, which is
/// the title, what counts as page text, which field is a supplier-declared
/// date, and which keys are promoted out of native metadata — and nothing
/// else about the envelope. The rules every provider shares are applied by
/// [`envelopes_from_results`] once, so no adapter can drift from them.
pub(crate) struct ResultFields {
    /// A supplier-declared licence reference; absent declarations stay unknown.
    pub(crate) licence: LicenceState,
    pub(crate) url: String,
    pub(crate) title: Option<String>,
    /// `None` when the provider returned no text or an empty string: "returned
    /// nothing" and "returned emptiness" are both a textless candidate, never
    /// invented content. The caller applies its own emptiness filter because
    /// which field is the text is the caller's mapping.
    pub(crate) text: Option<String>,
    pub(crate) declared_date: Option<DeclaredDate>,
    /// Keys removed from native metadata because their values were promoted
    /// into envelope fields, so one fact does not appear under two names.
    pub(crate) promoted: &'static [&'static str],
}

impl Default for ResultFields {
    fn default() -> Self {
        Self {
            url: String::new(),
            title: None,
            text: None,
            declared_date: None,
            licence: LicenceState::Unknown,
            promoted: &[],
        }
    }
}

/// One envelope per result `map` can read, in the order the provider ranked
/// them.
///
/// The rules applied here are the ones every content adapter holds to: a
/// result whose URL is absent or whose host cannot be read is not offered as
/// supply at all (see [`host_of`]); the content hash is over exactly the text
/// admitted; the licence is unknown unless the supplier declares a reference,
/// because search accessibility is not permission; the retrieval rank is
/// the result's 1-based position in the sequence `items` yields, so an adapter
/// that caps the offered candidates itself passes the capped iterator and the
/// rank still counts what was offered.
pub(crate) fn envelopes_from_results<'a, I, F>(items: I, map: F) -> Vec<ContextEnvelope>
where
    I: IntoIterator<Item = &'a Value>,
    F: Fn(&'a Value) -> Option<ResultFields>,
{
    items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let fields = map(item)?;
            Some(ContextEnvelope {
                host: host_of(&fields.url)?,
                content_hash: fields
                    .text
                    .as_deref()
                    .map(|text| sha256_digest(text.as_bytes())),
                title: fields.title,
                text: fields.text,
                licence: fields.licence,
                declared_date: fields.declared_date,
                native_metadata: native_metadata(item, fields.promoted),
                retrieval_rank: index as u32 + 1,
                source_url: fields.url,
            })
        })
        .collect()
}

/// A charge quoted from a provider's published price in the provider's own
/// unit, with no currency figure.
///
/// `money` stays `None`: that field is reserved for a currency charge the
/// provider reported on the response, and a quoted price is not one. `note`
/// names the published source the figure came from, because a quoted charge
/// is only interpretable alongside where it was read.
pub(crate) fn quoted_native(unit: &str, amount: Number, note: &str) -> AcquisitionCharge {
    AcquisitionCharge {
        money: None,
        native: Some(NativeCharge {
            unit: unit.to_owned(),
            amount,
            basis: ChargeBasis::Quoted,
            note: Some(note.to_owned()),
        }),
    }
}

/// A charge quoted from a provider's published price stated in a currency,
/// recorded both as comparable money and as the native figure.
///
/// `decimal` is the published price as decimal text (`"0.005"`), and both
/// representations are built from it so they cannot disagree; a price that is
/// already exact never acquires a rounding error on its way into the record.
///
/// # Panics
///
/// If `decimal` is not a JSON number. Every caller passes a literal.
pub(crate) fn quoted_money(currency: &str, decimal: &str, note: &str) -> AcquisitionCharge {
    let amount: Number =
        serde_json::from_str(decimal).expect("the published price is a valid JSON number");
    AcquisitionCharge {
        money: Money::from_decimal_str(currency, decimal),
        native: Some(NativeCharge {
            unit: currency.to_owned(),
            amount,
            basis: ChargeBasis::Quoted,
            note: Some(note.to_owned()),
        }),
    }
}

/// Percent-encode a value for a URL query string. The operator's query is a
/// sentence, and everything outside the unreserved set has to be escaped
/// before it can sit in a URL.
pub(crate) fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// A bounded excerpt of an error body, for the evidence record.
pub(crate) fn error_detail(body: &[u8]) -> String {
    const MAX: usize = 512;
    let text = String::from_utf8_lossy(body);
    let trimmed = text.trim();
    match trimmed.char_indices().nth(MAX) {
        Some((cut, _)) => format!("{}…", &trimmed[..cut]),
        None => trimmed.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn released(value: &str) -> credentials::Released {
        credentials::Released::new(
            "conn-1",
            "exa",
            "EXA_API_KEY",
            value,
            None,
            "2026-09-19T10:00:00.000Z",
        )
        .expect("exa's own variable is accepted")
    }

    // Catches: a hub release overriding the operator's shell or file; a
    // released value not reaching adapter construction; a cleared store still
    // supplying a value; a blank exported variable counted as a credential.
    #[test]
    fn the_environment_wins_then_the_release_then_the_variable_is_missing() {
        let store = credentials::ReleasedStore::new(std::time::Duration::from_secs(900));
        store.replace(vec![released("from-hub")]);
        let shell = |name: &str| (name == "EXA_API_KEY").then(|| "from-shell".to_owned());
        let blank = |name: &str| (name == "EXA_API_KEY").then(|| "  ".to_owned());
        let absent = |_: &str| None;

        let value = |environment: &dyn Fn(&str) -> Option<String>| {
            credential_for("EXA_API_KEY", Some(&store), environment)
        };
        assert_eq!(value(&shell).unwrap(), "from-shell");
        assert_eq!(value(&blank).unwrap(), "from-hub");
        assert_eq!(value(&absent).unwrap(), "from-hub");

        store.clear();
        assert!(matches!(
            value(&absent),
            Err(SupplyError::CredentialMissing { variable }) if variable == "EXA_API_KEY"
        ));
        assert!(matches!(
            credential_for("EXA_API_KEY", None, absent),
            Err(SupplyError::CredentialMissing { .. })
        ));
    }

    // Catches: a hub naming the corpus root or the skill catalogue, which
    // decide what this machine reads and executes; a variable that is not the
    // provider's own; a blank value held as a credential.
    #[test]
    fn a_release_is_held_only_for_a_remote_providers_own_variable() {
        let new = |provider: &str, variable: &str, value: &str| {
            credentials::Released::new("conn-1", provider, variable, value, None, "t")
        };
        assert!(new("exa", "EXA_API_KEY", "k").is_some());
        assert!(new("tavily", "TAVILY_API_KEY", "k").is_some());
        assert!(new("exa", "TAVILY_API_KEY", "k").is_none());
        assert!(new("exa", "EXA_API_KEY", " ").is_none());
        assert!(new("internal", CORPUS_VARIABLE, "/corpus").is_none());
        assert!(new("skill:audit", CATALOGUE_VARIABLE, "/skills.json").is_none());
        assert!(new("unknown", "UNKNOWN_API_KEY", "k").is_none());
    }

    // Catches: a released value in a debug rendering or in the names evidence
    // records; a shadowed variable reported as in use.
    #[test]
    fn the_store_reports_names_and_never_a_value() {
        let store = credentials::ReleasedStore::new(std::time::Duration::from_secs(900));
        assert!(store.replace(vec![released("exa-secret-value")]));
        assert!(!store.replace(vec![released("exa-secret-value")]));
        assert!(store.replace(vec![released("exa-rotated-value")]));

        let rendered = format!(
            "{store:?} {:?} {}",
            released("exa-secret-value"),
            serde_json::to_string(&store.names()).unwrap()
        );
        assert!(!rendered.contains("exa-secret-value"), "{rendered}");
        assert!(!rendered.contains("exa-rotated-value"), "{rendered}");
        assert!(rendered.contains("conn-1"), "{rendered}");

        let standing = store.standing_under(|_| false);
        assert_eq!(standing.in_use.len(), 1);
        assert!(standing.shadowed.is_empty());
        let standing = store.standing_under(|name| name == "EXA_API_KEY");
        assert!(standing.in_use.is_empty());
        assert_eq!(standing.shadowed[0].connection_id, "conn-1");
    }

    // Catches: a released value supplied after its maximum age, whoever
    // stopped confirming it; an expired set still named as in use, or not
    // named at all; a release after an expiry reported as unchanged; an
    // empty store reported as expired.
    #[test]
    fn a_set_no_fetch_has_confirmed_for_its_maximum_age_is_not_used() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let elapsed = std::sync::Arc::new(std::sync::Mutex::new(Duration::ZERO));
        let store = credentials::ReleasedStore::with_clock(Duration::from_secs(900), {
            let elapsed = std::sync::Arc::clone(&elapsed);
            std::sync::Arc::new(move || start + *elapsed.lock().unwrap())
        });
        let at = |seconds: u64| *elapsed.lock().unwrap() = Duration::from_secs(seconds);
        let absent = |_: &str| None;
        let value = || credential_for("EXA_API_KEY", Some(&store), absent);

        assert!(
            store.expired().is_empty(),
            "an empty store has nothing to expire"
        );
        store.replace(vec![released("from-hub")]);
        at(899);
        assert_eq!(value().unwrap(), "from-hub");
        assert_eq!(store.names().len(), 1);
        assert!(store.expired().is_empty());

        // A confirming fetch starts the age again.
        assert!(!store.replace(vec![released("from-hub")]));
        at(899 + 899);
        assert_eq!(value().unwrap(), "from-hub");

        at(899 + 900);
        assert!(matches!(
            value(),
            Err(SupplyError::CredentialMissing { variable }) if variable == "EXA_API_KEY"
        ));
        assert!(store.names().is_empty());
        let standing = store.standing_under(|name| name == "EXA_API_KEY");
        assert!(standing.in_use.is_empty() && standing.shadowed.is_empty());
        assert_eq!(standing.expired[0].connection_id, "conn-1");
        assert_eq!(standing.max_age, Duration::from_secs(900));
        assert!(!format!("{store:?}").contains("from-hub"));

        // The same value served again is a change: nothing was in use.
        assert!(store.replace(vec![released("from-hub")]));
        assert_eq!(value().unwrap(), "from-hub");
        assert!(store.expired().is_empty());
    }

    /// The provider list and the three lookups that answer for a provider
    /// must name the same set. A provider added to one and not the others
    /// surfaces as `configured: false`, or as a plan that cannot be built,
    /// with nothing failing at the point the name was added.
    ///
    /// `internal` and a skill provider are the two that name no remote
    /// origin: neither has one to dial and neither can be replayed from a
    /// recorded response.
    #[test]
    fn every_implemented_provider_is_answered_by_every_lookup() {
        for provider in IMPLEMENTED_PROVIDERS {
            assert!(
                required_variable(provider).is_some(),
                "{provider} is implemented and names no environment variable"
            );
            assert!(
                declared_provider_ref(provider).is_some(),
                "{provider} is implemented and declares no capabilities"
            );
            let remote = remote_adapter(provider, Some("http://127.0.0.1:1"), "placeholder");
            assert_eq!(
                remote.is_some(),
                provider != "internal",
                "{provider} is implemented but is wired for only one of the live and replay paths"
            );
        }

        let skill = format!("{SKILL_PROVIDER_PREFIX}anything");
        assert!(required_variable(&skill).is_some());
        assert!(declared_provider_ref(&skill).is_some());
        assert!(
            remote_adapter(&skill, Some("http://127.0.0.1:1"), "placeholder").is_none(),
            "a skill is a local execution, not a remote origin"
        );

        for unknown in ["", "not-a-provider", SKILL_PROVIDER_PREFIX] {
            assert!(required_variable(unknown).is_none());
            assert!(declared_provider_ref(unknown).is_none());
            assert!(remote_adapter(unknown, Some("http://127.0.0.1:1"), "placeholder").is_none());
        }
    }
}
