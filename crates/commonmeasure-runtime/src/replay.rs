//! Recorded replay as a run mode.
//!
//! A replay run serves committed, redacted provider captures from real
//! loopback origins and lets the production adapters, transport, parsers,
//! policy and evidence trail run unchanged — the same substitution the
//! integration tests make (`crates/commonmeasure-supply/tests/recorded_replay.rs`).
//! What replay adds over the tests is provenance: a committed manifest names each served response's recon
//! source, capture date, redactions and permitted use, and every served byte
//! is checked against the manifest's hash before an origin will serve it.
//!
//! The mode has no weaker fallback. A provider without a verified recording
//! fails the run before it starts; nothing here can reach a live endpoint,
//! because every adapter is constructed against a loopback origin and no
//! credential is ever read.

use commonmeasure_types::canonical::sha256_digest;
use std::collections::BTreeMap;
use std::path::{Component, Path};

use commonmeasure_http::{Response, Server, ServerHandle};
use commonmeasure_supply::{SupplyAdapter, SupplyError};
use commonmeasure_types::canonical::canonical_json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::run::SupplyResolver;

/// The manifest a replay directory must hold, beside the captures it names.
pub const MANIFEST_FILE: &str = "replay-manifest.json";

/// The manifest shape this runtime reads.
pub const MANIFEST_VERSION: &str = "contextops-replay/v1";

/// Sent where an adapter requires a credential header. The loopback origin
/// ignores it; it is deliberately not secret-shaped, and the capture's own
/// redacted header records which variable authenticated the original call.
const PLACEHOLDER_CREDENTIAL: &str = "replay-no-credential";

/// The recorded legs a quote-then-buy provider's replay needs, in wire
/// order, named by the manifest's `capability` column. One routed origin
/// serves all five, answering each JSON-RPC request with its own leg's
/// verified bytes — the shape the real gateway has.
const QUOTE_LEGS: [&str; 5] = ["initialize", "balance", "inspect", "quote", "confirm"];

/// The leg whose bytes are the plan's main sealed response: the settlement,
/// which is what the envelopes are derived from.
const SETTLEMENT_LEG: &str = "confirm";

/// One recorded response the manifest offers for replay, with the provenance
/// replay requires: where the bytes came from, when, what was redacted, and
/// what serving them is for.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recording {
    pub provider: String,
    /// What the recording answers: an acquisition capability in the
    /// vocabulary of `docs/contracts/provider.md` (`search` for the open-web
    /// providers), or — for a quote-then-buy provider — one of the five leg
    /// names in [`QUOTE_LEGS`].
    pub capability: String,
    /// Path of the capture, relative to the replay directory.
    pub source: String,
    /// The date the original call was made, from the capture's own evidence.
    pub captured_at: String,
    /// The URL the original call was made to. The replay origin is loopback;
    /// this records what the recorded response is a response *to*.
    pub recorded_endpoint: String,
    /// SHA-256 of the exact bytes an origin serves: the capture's
    /// `response.body`, canonically serialised. Checked before serving, so a
    /// capture that changed since the manifest was derived refuses to replay.
    pub response_sha256: String,
    /// What was removed from the capture before it was committed.
    pub redactions: String,
    /// What serving these bytes is for. Recon captures are retained evidence,
    /// not licensed content for reuse.
    pub permitted_use: String,
}

/// A manifest is refused, not repaired: an unknown field is a version this
/// runtime does not read, and a missing recording is a run that must not
/// start. The same posture as the session policy loader.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayManifest {
    manifest_version: String,
    /// Where the captures came from, for a human reader. Accepted and never
    /// read, because an unknown field refuses the manifest.
    #[serde(rename = "derived_from")]
    _derived_from: String,
    recordings: Vec<Recording>,
}

/// What a replay run records about itself: the manifest it served from and
/// the recording behind each provider. Carried in `RunOptions` so the runtime
/// can bind every sealed response to its recorded source.
#[derive(Debug, Clone)]
pub struct ReplayContext {
    /// The manifest path as the operator named it, for the published record.
    pub manifest_path: String,
    /// The recording behind each provider's main sealed response, by
    /// provider name: the single acquisition recording for an open-web
    /// provider, the settlement (`confirm`) leg for a quote-then-buy one.
    pub recordings: BTreeMap<String, Recording>,
    /// Every recording served per provider, in wire order — the same single
    /// recording for an open-web provider, all five legs for a
    /// quote-then-buy one. This is what the published `replay.json` lists,
    /// so the quote legs' provenance is inspectable too.
    pub served: BTreeMap<String, Vec<Recording>>,
}

#[derive(Debug)]
pub enum ReplayError {
    ManifestUnreadable {
        path: String,
        detail: String,
    },
    ManifestInvalid {
        path: String,
        detail: String,
    },
    ManifestVersion {
        path: String,
        found: String,
    },
    UnknownProvider {
        provider: String,
    },
    RecordingMissing {
        provider: String,
        capability: String,
    },
    SourceEscapes {
        source: String,
    },
    CaptureUnreadable {
        path: String,
        detail: String,
    },
    CaptureInvalid {
        path: String,
        detail: String,
    },
    HashMismatch {
        source: String,
        expected: String,
        actual: String,
    },
    EndpointMismatch {
        source: String,
        declared: String,
        recorded: String,
    },
    Origin {
        detail: String,
    },
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ManifestUnreadable { path, detail } => write!(
                f,
                "cannot read the replay manifest {path}: {detail}. A replay run serves only \
                 what a manifest names; there is no fallback to live acquisition"
            ),
            Self::ManifestInvalid { path, detail } => {
                write!(f, "the replay manifest {path} is not readable: {detail}")
            }
            Self::ManifestVersion { path, found } => write!(
                f,
                "the replay manifest {path} declares {found}; this runtime reads {version}",
                version = MANIFEST_VERSION
            ),
            Self::UnknownProvider { provider } => write!(
                f,
                "the suite names provider {provider}, which has no implemented adapter"
            ),
            Self::RecordingMissing {
                provider,
                capability,
            } => write!(
                f,
                "the replay manifest holds no {capability} recording for provider {provider}. \
                 A replay run serves only committed recorded responses; it never falls back to \
                 live or local acquisition, so this provider cannot run in replay mode"
            ),
            Self::SourceEscapes { source } => write!(
                f,
                "recording source {source} points outside the replay directory, and a replay \
                 run reads nothing the manifest's own directory does not contain"
            ),
            Self::CaptureUnreadable { path, detail } => {
                write!(f, "cannot read the recorded capture {path}: {detail}")
            }
            Self::CaptureInvalid { path, detail } => {
                write!(f, "the recorded capture {path} is not a capture: {detail}")
            }
            Self::HashMismatch {
                source,
                expected,
                actual,
            } => write!(
                f,
                "the recorded capture {source} does not match the manifest ({expected} declared, \
                 {actual} on disk). Either the capture changed since the manifest was derived or \
                 the manifest is stale; recon artefacts are evidence and a replay refuses to \
                 serve bytes it cannot vouch for"
            ),
            Self::EndpointMismatch {
                source,
                declared,
                recorded,
            } => write!(
                f,
                "the recorded capture {source} answers {recorded}, and the manifest declares it \
                 as {declared}. The endpoint a run publishes must be the one the bytes came \
                 from, so a replay refuses rather than seal a response against a URL nobody \
                 called"
            ),
            Self::Origin { detail } => write!(f, "cannot start a loopback origin: {detail}"),
        }
    }
}

impl std::error::Error for ReplayError {}

/// The loopback origins of one replay run, alive for as long as this value is.
///
/// Dropping it stops the origins, so it must outlive the `execute` call that
/// resolves suppliers against it.
pub struct ReplaySupply {
    /// Held for their `Drop`: each handle stops its origin.
    _origins: Vec<ServerHandle>,
    /// Provider name → loopback base URL.
    bases: BTreeMap<String, String>,
    context: ReplayContext,
}

impl ReplaySupply {
    /// Verify and serve the recordings `providers` need from `directory`.
    ///
    /// Every provider must resolve to a hash-verified recording before any
    /// origin starts; a missing or altered recording fails here, not midway
    /// through a run.
    pub fn start(directory: &Path, providers: &[String]) -> Result<Self, ReplayError> {
        Self::start_with(directory, providers, false)
    }

    /// [`Self::start`] for a suite: a suite that names a `fetch_target` is a
    /// fetch comparison, so each open-web provider's `fetch` recording is
    /// served instead of its `search` one — the capability the run will
    /// dispatch (`crate::run`).
    pub fn for_suite(directory: &Path, suite: &crate::Suite) -> Result<Self, ReplayError> {
        Self::start_with(directory, &suite.providers, suite.fetch_target.is_some())
    }

    fn start_with(
        directory: &Path,
        providers: &[String],
        fetch: bool,
    ) -> Result<Self, ReplayError> {
        let manifest_path = directory.join(MANIFEST_FILE);
        let shown = manifest_path.display().to_string();
        let bytes =
            std::fs::read(&manifest_path).map_err(|error| ReplayError::ManifestUnreadable {
                path: shown.clone(),
                detail: error.to_string(),
            })?;
        let manifest: ReplayManifest =
            serde_json::from_slice(&bytes).map_err(|error| ReplayError::ManifestInvalid {
                path: shown.clone(),
                detail: error.to_string(),
            })?;
        if manifest.manifest_version != MANIFEST_VERSION {
            return Err(ReplayError::ManifestVersion {
                path: shown,
                found: manifest.manifest_version,
            });
        }

        let mut origins = Vec::new();
        let mut bases = BTreeMap::new();
        let mut recordings = BTreeMap::new();
        let mut served = BTreeMap::new();
        let find = |provider: &str, capability: &str| {
            manifest
                .recordings
                .iter()
                .find(|recording| {
                    recording.provider == provider && recording.capability == capability
                })
                .cloned()
                .ok_or_else(|| ReplayError::RecordingMissing {
                    provider: provider.to_owned(),
                    capability: capability.to_owned(),
                })
        };
        for provider in providers {
            if recordings.contains_key(provider) {
                continue;
            }
            let origin = if quote_provider(provider)? {
                // Every leg must verify before the origin starts: a
                // quote-then-buy replay with a missing settlement would run
                // four legs and fail mid-plan, which is exactly the weaker
                // mode replay refuses to have.
                let mut legs = BTreeMap::new();
                let mut in_order = Vec::new();
                for leg in QUOTE_LEGS {
                    let recording = find(provider, leg)?;
                    legs.insert(leg.to_owned(), verified_capture(directory, &recording)?);
                    in_order.push(recording);
                }
                recordings.insert(
                    provider.clone(),
                    in_order
                        .iter()
                        .find(|recording| recording.capability == SETTLEMENT_LEG)
                        .expect("QUOTE_LEGS contains the settlement leg")
                        .clone(),
                );
                served.insert(provider.clone(), in_order);
                serve_routed(legs)?
            } else {
                let capability = planned_capability(provider, fetch)?;
                let recording = find(provider, capability)?;
                let capture = verified_capture(directory, &recording)?;
                recordings.insert(provider.clone(), recording.clone());
                served.insert(provider.clone(), vec![recording]);
                serve(capture.status, capture.body)?
            };
            bases.insert(provider.clone(), origin.url());
            origins.push(origin);
        }

        Ok(Self {
            _origins: origins,
            bases,
            context: ReplayContext {
                manifest_path: shown,
                recordings,
                served,
            },
        })
    }

    /// The resolver a replay run hands to [`crate::RunOptions`]: every
    /// provider resolves to its real adapter aimed at a loopback origin. A
    /// provider without a loaded recording is refused — `start` already
    /// guarantees none is asked for, and this is the backstop that keeps the
    /// guarantee even for a caller that bypassed it.
    pub fn resolver(&self) -> SupplyResolver {
        let bases = self.bases.clone();
        Box::new(move |provider| {
            let base = bases.get(provider).ok_or_else(|| SupplyError::Transport {
                detail: format!(
                    "no recording is loaded for provider {provider}; a replay run never \
                         falls back to live acquisition"
                ),
            })?;
            adapter_against(provider, base).ok_or_else(|| SupplyError::Transport {
                detail: format!(
                    "provider {provider} takes no HTTP origin, so it cannot be replayed"
                ),
            })
        })
    }

    /// The provenance a run records: which recording stands behind each
    /// provider, and where the manifest that vouches for them lives.
    pub fn context(&self) -> ReplayContext {
        self.context.clone()
    }

    /// Run options for a replay run: recorded suppliers, no external
    /// acquisition authority, and the inference gateway from the environment —
    /// replay fixes the supply side of the experiment, never the model route.
    pub fn run_options(&self, output: std::path::PathBuf) -> crate::RunOptions {
        crate::RunOptions {
            allow_external_acquisition: false,
            output,
            suppliers: self.resolver(),
            backend: commonmeasure_inference::backend_from_environment(),
            replay: Some(self.context()),
            // A replayed purchase moves no money, so it must not enter the
            // real spend ledger; the quote record states the absence.
            allowance: None,
            provenance_signing: crate::processor::provenance::SigningIdentity::Unconfigured,
        }
    }
}

/// Does `provider` acquire through the quote gate? A quote-declaring
/// provider replays all five recorded legs behind one routed origin.
fn quote_provider(provider: &str) -> Result<bool, ReplayError> {
    use commonmeasure_types::ProviderCapability;
    let provider_ref = commonmeasure_supply::declared_provider_ref(provider).ok_or_else(|| {
        ReplayError::UnknownProvider {
            provider: provider.to_owned(),
        }
    })?;
    Ok(provider_ref
        .capabilities
        .contains(&ProviderCapability::Quote))
}

/// The acquisition capability a run would plan for `provider` — the same
/// choice `supply_plans` makes, so the recording looked up here is the one the
/// plan's step will exercise.
fn planned_capability(provider: &str, fetch: bool) -> Result<&'static str, ReplayError> {
    use commonmeasure_types::ProviderCapability;
    let provider_ref = commonmeasure_supply::declared_provider_ref(provider).ok_or_else(|| {
        ReplayError::UnknownProvider {
            provider: provider.to_owned(),
        }
    })?;
    if fetch {
        // A fetch comparison dispatches `fetch` to every provider, so the
        // recording looked up is the fetch one; a provider that declares no
        // fetch has no such recording, and the manifest lookup names that.
        return Ok("fetch");
    }
    if provider_ref
        .capabilities
        .contains(&ProviderCapability::Search)
    {
        Ok("search")
    } else {
        // The internal corpus lands here: it declares `query`, a local read
        // with no recorded acquisition, so the manifest lookup will refuse it
        // by name rather than this function guessing.
        Ok("query")
    }
}

/// One verified recorded response, ready to serve.
struct VerifiedCapture {
    status: u16,
    body: Vec<u8>,
    /// The `mcp-session-id` response header the capture records, where one
    /// exists — the committed redaction marker, which the routed origin
    /// re-issues so an adapter's session discipline replays end to end.
    session: Option<String>,
}

/// Read a capture and prove it is the one the manifest describes.
///
/// The bytes hashed are the bytes served, in whichever of the two committed
/// capture shapes the file uses: the open-web recon nests `response.body` as
/// a JSON value (served canonically re-serialised), the Redpine recon keeps
/// the wire body verbatim as a top-level `body` string (served byte for
/// byte). Either way the served bytes are what the adapter receives and the
/// run seals, which is what lets a published run bind its sealed response to
/// the manifest's hash by equality.
fn verified_capture(
    directory: &Path,
    recording: &Recording,
) -> Result<VerifiedCapture, ReplayError> {
    let source = Path::new(&recording.source);
    if source.is_absolute()
        || source
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ReplayError::SourceEscapes {
            source: recording.source.clone(),
        });
    }
    let path = directory.join(source);
    let shown = path.display().to_string();
    let bytes = std::fs::read(&path).map_err(|error| ReplayError::CaptureUnreadable {
        path: shown.clone(),
        detail: error.to_string(),
    })?;
    let capture: Value =
        serde_json::from_slice(&bytes).map_err(|error| ReplayError::CaptureInvalid {
            path: shown.clone(),
            detail: format!("not valid JSON: {error}"),
        })?;
    let (status, body, session) = if capture["response"].is_object() {
        let status = capture["response"]["status"]
            .as_u64()
            .and_then(|status| u16::try_from(status).ok())
            .ok_or_else(|| ReplayError::CaptureInvalid {
                path: shown.clone(),
                detail: "no response.status".to_owned(),
            })?;
        let body = &capture["response"]["body"];
        if body.is_null() {
            return Err(ReplayError::CaptureInvalid {
                path: shown,
                detail: "no response.body".to_owned(),
            });
        }
        // The canonical form of the recorded body, which is what the
        // recording's declared hash covers and what the loopback origin
        // serves: the same bytes for one recording whatever serialiser or
        // map implementation a build happens to link
        // (`crates/commonmeasure-types/src/canonical.rs`).
        let body = canonical_json(body).into_bytes();
        (status, body, None)
    } else {
        let status = capture["status"]
            .as_u64()
            .and_then(|status| u16::try_from(status).ok())
            .ok_or_else(|| ReplayError::CaptureInvalid {
                path: shown.clone(),
                detail: "no status".to_owned(),
            })?;
        let body = capture["body"]
            .as_str()
            .ok_or_else(|| ReplayError::CaptureInvalid {
                path: shown,
                detail: "no body string".to_owned(),
            })?
            .as_bytes()
            .to_vec();
        let session = capture["headers"]["mcp-session-id"]
            .as_str()
            .map(str::to_owned);
        (status, body, session)
    };
    let actual = sha256_digest(&body);
    if actual != recording.response_sha256 {
        return Err(ReplayError::HashMismatch {
            source: recording.source.clone(),
            expected: recording.response_sha256.clone(),
            actual,
        });
    }
    // The manifest's `recorded_endpoint` is what a replayed plan publishes as
    // the URL it acquired from, so it must be the URL the capture itself says
    // it called. An HTTP-shaped capture records it under `request.url` and a
    // JSON-RPC leg under `url`; a capture that records neither is served on
    // the manifest's word, which is the only claim there is.
    if let Some(recorded) = capture["request"]["url"]
        .as_str()
        .or_else(|| capture["url"].as_str())
        && recorded != recording.recorded_endpoint
    {
        return Err(ReplayError::EndpointMismatch {
            source: recording.source.clone(),
            declared: recording.recorded_endpoint.clone(),
            recorded: recorded.to_owned(),
        });
    }
    Ok(VerifiedCapture {
        status,
        body,
        session,
    })
}

/// Serve one verified capture exactly as it was received: same status, same
/// body bytes, on every request. Every committed capture is JSON, and the
/// content type says so because the adapters' parsers expect to say why a
/// body is unreadable rather than guess at its framing.
fn serve(status: u16, body: Vec<u8>) -> Result<ServerHandle, ReplayError> {
    Server::bind("127.0.0.1:0")
        .and_then(|server| {
            server.spawn(move |_| {
                let mut response = Response::new(status, body.clone());
                response.headers.set("Content-Type", "application/json");
                response
            })
        })
        .map_err(|error| ReplayError::Origin {
            detail: format!("{error:#}"),
        })
}

/// Serve a quote-then-buy provider's verified legs from one origin, routing
/// each JSON-RPC request to its own leg's bytes the way the real gateway
/// does, and re-issuing the recorded session header where the leg carries
/// one. A request no recorded leg answers is refused with a response naming
/// that — the adapter then surfaces the provider's words — never answered
/// with some other leg's bytes.
fn serve_routed(legs: BTreeMap<String, VerifiedCapture>) -> Result<ServerHandle, ReplayError> {
    Server::bind("127.0.0.1:0")
        .and_then(|server| {
            server.spawn(move |request| {
                let parsed: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
                let leg = match parsed["method"].as_str() {
                    Some("initialize") => Some("initialize"),
                    Some("tools/call") => match parsed["params"]["name"].as_str() {
                        Some("get_balance") => Some("balance"),
                        Some("inspect-tool") => Some("inspect"),
                        Some("preview") => Some("quote"),
                        Some("confirm") => Some("confirm"),
                        _ => None,
                    },
                    _ => None,
                };
                match leg.and_then(|leg| legs.get(leg)) {
                    Some(capture) => {
                        let mut response = Response::new(capture.status, capture.body.clone());
                        response.headers.set("Content-Type", "application/json");
                        if let Some(session) = &capture.session {
                            response.headers.set("mcp-session-id", session);
                        }
                        response
                    }
                    None => Response::text(
                        404,
                        "the replay origin holds no recorded leg for this request; a replay \
                         serves only what its manifest names",
                    ),
                }
            })
        })
        .map_err(|error| ReplayError::Origin {
            detail: format!("{error:#}"),
        })
}

/// The real adapter for `provider`, constructed against a loopback origin.
///
/// One call into `commonmeasure_supply::remote_adapter`, which is also what a
/// live run builds from, so a provider cannot be wired for live acquisition
/// and left unreplayable, or the other way round. `None` for a provider with
/// no HTTP origin to substitute (the internal corpus), which `start` has
/// already refused by then.
fn adapter_against(provider: &str, base_url: &str) -> Option<Box<dyn SupplyAdapter>> {
    commonmeasure_supply::remote_adapter(provider, Some(base_url), PLACEHOLDER_CREDENTIAL)
}
