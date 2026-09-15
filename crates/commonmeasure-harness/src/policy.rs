//! The operator's standing policy for a harness session.
//!
//! A session has no ContextJob, but an operator's source rules are the same
//! rules, so they are declared in the same vocabulary (`commonmeasure_types::Constraint`)
//! and enforced by the same functions (`commonmeasure_runtime::policy`). There is no
//! second policy engine, and there is nothing a session can express that a job
//! cannot.
//!
//! No policy file means observe: record everything, refuse nothing. That is the
//! adoption on-ramp, and it is the state in which the mediated tools are still
//! useful, because recording is the half that needs no configuration.

use std::path::{Path, PathBuf};

use crate::declaration;
use commonmeasure_runtime::policy::{self, Ruling, normalised_host};
/// The vocabulary a policy is written in, re-exported here: a caller that
/// reads or edits a policy needs the mode words, and only this module says
/// which type carries them.
pub use commonmeasure_types::PolicyMode;
use commonmeasure_types::canonical::{canonical_digest, canonical_json};
use commonmeasure_types::{AllowanceDeclaration, Constraint, ContextEnvelope};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Unknown fields are load errors, here above all: every field has a
/// permissive default, so a misspelled `"contraints"` would otherwise load as
/// a strict policy enforcing nothing — and `context_status` would report the
/// mode the operator wrote over the constraints they lost.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyFile {
    #[serde(default = "observe")]
    pub policy_mode: PolicyMode,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    /// Scoped overlays, resolved against the session's working directory when
    /// the server starts. Ordered; the first scope whose `match` is a
    /// substring of the cwd applies, and no match falls through to the
    /// top-level policy. Strict for a client engagement and observe for
    /// personal work cannot be a read-time projection — refusal happens at
    /// the crossing — which is why this lives here and not in the sink's
    /// attribution rules.
    #[serde(default)]
    pub scopes: Vec<PolicyScope>,
    /// Delegated authority keyed by an authenticated operating-system user.
    /// An empty list leaves the policy scoped by directory alone, resolving
    /// exactly as a policy with no principals.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub principals: Vec<PrincipalBinding>,
    /// Whether the mediated tools may reach loopback and private-network
    /// addresses. Off by default.
    ///
    /// The asymmetry with observed capture is deliberate. Passive capture sees
    /// everything the agent does and never asks, so it holds a hard floor: an
    /// operator's localhost, private network and `file://` traffic stays out of
    /// the record whatever this says. A mediated call is the agent naming one
    /// URL and asking for it, which an operator running a local documentation
    /// server may legitimately want governed and recorded — so that is theirs
    /// to allow, explicitly.
    #[serde(default)]
    pub allow_private_hosts: bool,
    /// Whether `strict` refuses a crossing for a PII finding on every
    /// source. Off by default: a finding on a public source is recorded on
    /// the crossing and the crossing is admitted, because a public page's
    /// published contact details are not the personal data the detector
    /// exists to keep out of a model, and `strict` refuses a finding only
    /// on an internal or private source. An operator that wants the
    /// refusal everywhere sets this (`DECISIONS.md` §Execution and
    /// evidence). Left out of the file when false, so the loader's form of
    /// a policy that does not set it is unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refuse_on_pii: bool,
    /// Internal prefixes whose crossings may be recorded despite the privacy
    /// floor, for observed and mediated capture alike.
    ///
    /// The floor is a default, not a ceiling. An enterprise operator's most
    /// valuable supply is internal — a RAG corpus, an intranet, a document
    /// store — and the questions this record exists to answer apply to that
    /// supply more than to the open web. Each entry here is consent in
    /// writing for one named prefix (`"https://rag.example.internal/"`,
    /// `"file:///corp/kb/"`); everything unmatched stays out of the record
    /// exactly as if the list were absent. The floor is never lowered by
    /// omission.
    #[serde(default)]
    pub record_internal_prefixes: Vec<String>,
    /// Terms the operator holds with named sources: an agreement with a
    /// publisher, a subscription, a licence bought outside this runtime. A
    /// statement of fact about the agreement, referenced by an identifier the
    /// operator chooses; the runtime records it as governing over any
    /// preference the source publishes for that host and never checks it
    /// (`DECISIONS.md` §Source declarations).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terms: Vec<TermsDeclaration>,
}

/// One agreement the operator holds with a source, keyed by host.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TermsDeclaration {
    /// The host the agreement covers, exactly; a subdomain needs its own
    /// entry.
    pub host: String,
    /// The operator's reference for the agreement. Recorded as the licence
    /// reference on every crossing the terms govern and projected as
    /// `license_ref`.
    pub reference: String,
    /// Whether the agreement itself requires usage reporting to the source.
    /// A reporting duty comes from the source or from terms like these,
    /// never from policy alone.
    #[serde(default)]
    pub requires_reporting: bool,
    /// The institution identifiers the agreement attributes usage to (a ROR
    /// or ISNI where one exists, otherwise the agreement id under an opaque
    /// scheme). Present only where the terms require it; they name the
    /// operator as an institution, never a person
    /// (`DECISIONS.md` §Session policy and egress).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access_context: Vec<InstitutionIdentifier>,
}

/// One typed institution identifier, in the standard's `access_context`
/// shape: a scheme (`ror`, `isni`, `saml_entity_id`, or an operator-chosen
/// one) and a value in that scheme's own format.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstitutionIdentifier {
    pub scheme: String,
    pub value: String,
}

/// One scoped overlay. A field left out inherits the top-level value, so a
/// scope states only what it changes.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyScope {
    /// Substring matched against the session's working directory.
    #[serde(rename = "match")]
    pub matcher: String,
    /// When present, this directory overlay is authority belonging only to
    /// the named principal. A working directory cannot authenticate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    /// The **governing** engagement: the name under whose clearance a crossing
    /// in this scope may leave the machine. Optional for older
    /// enforcement-only scopes; telemetry egress cannot be cleared without it.
    ///
    /// This is capture-time enforcement identity and not the name the console
    /// reports work under — that is the reported engagement, resolved at read
    /// time from the sink's attribution rules, which nothing below `commonmeasure-console`
    /// can read. The two are expected to agree and nothing here can check that
    /// they do; the console reads both and reports where they differ
    /// (`DECISIONS.md` §Session policy and egress).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engagement: Option<String>,
    /// Whether witnessed public crossings governed by this scope may enter a
    /// configured telemetry projection. False by default, including for every
    /// client engagement, and meaningful only beside a named `engagement`.
    #[serde(default)]
    pub allow_telemetry_egress: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_mode: Option<PolicyMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<Vec<Constraint>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_private_hosts: Option<bool>,
    /// Whether `strict` refuses a PII finding on every source in this
    /// scope; inherits the top-level value when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refuse_on_pii: Option<bool>,
    /// Terms this scope holds, replacing the top-level list when present, as
    /// `constraints` does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms: Option<Vec<TermsDeclaration>>,
}

/// One authenticated principal's overlay. `os_user` is the effective numeric
/// user id, read from the process rather than an environment variable.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrincipalBinding {
    pub principal: String,
    pub os_user: u32,
    /// Require the working directory to match a scope explicitly bound to this
    /// principal. No match is a refusal, not top-level inheritance.
    #[serde(default)]
    pub require_scope: bool,
    /// Periodic monetary allowances for this principal, at most one per
    /// calendar period. Declaring one here is allocation — desired state —
    /// and enforcement is the runtime's ledger, which reserves and
    /// reconciles at the purchase (`DECISIONS.md` §Delegated authority and fleet management). An empty
    /// list declares no cumulative limit, exactly as before allowances
    /// existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowances: Vec<AllowanceDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_mode: Option<PolicyMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<Vec<Constraint>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_private_hosts: Option<bool>,
}

/// How an identity was established, and therefore what the identity is worth
/// as evidence. The basis travels with the principal everywhere the principal
/// is recorded: a name is worth exactly what authenticated it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationBasis {
    /// The effective operating-system user of this process, read from the
    /// kernel rather than from anything the process can write.
    OsUser,
    /// This platform offers no identity this runtime can read from outside
    /// the process's own control. Nothing was authenticated.
    Unavailable,
}

impl AuthenticationBasis {
    /// The name recorded on a crossing and in `context_status`
    /// (`docs/contracts/session-evidence.md` §Crossing).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OsUser => "os_user",
            Self::Unavailable => "unavailable",
        }
    }
}

/// What a process with no authenticated identity is called. It names the
/// absence, and no binding can ever select it: bindings are keyed on the
/// authenticated subject, which such a process does not have.
const UNAUTHENTICATED: &str = "unauthenticated";

/// Identity established at the process boundary. The asserted label is kept
/// for diagnostics only and never participates in binding selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    name: String,
    /// The authenticated subject bindings are keyed on, absent when the basis
    /// is [`AuthenticationBasis::Unavailable`].
    os_user: Option<u32>,
    basis: AuthenticationBasis,
    asserted_label: Option<String>,
}

impl Principal {
    pub fn current() -> Self {
        Self::resolved(
            trusted_os_user(),
            std::env::var("COMMONMEASURE_PRINCIPAL").ok(),
        )
    }

    /// One place decides what an authenticated subject — or its absence — is
    /// called, so a test principal and a real one cannot drift apart.
    fn resolved(os_user: Option<u32>, asserted_label: Option<String>) -> Self {
        match os_user {
            Some(os_user) => Self {
                name: format!("os-user:{os_user}"),
                os_user: Some(os_user),
                basis: AuthenticationBasis::OsUser,
                asserted_label,
            },
            None => Self {
                name: UNAUTHENTICATED.to_owned(),
                os_user: None,
                basis: AuthenticationBasis::Unavailable,
                asserted_label,
            },
        }
    }

    #[cfg(test)]
    fn for_test(os_user: Option<u32>, asserted_label: Option<&str>) -> Self {
        Self::resolved(os_user, asserted_label.map(str::to_owned))
    }
}

/// The credential this process cannot rewrite about itself, when the platform
/// has one this runtime can read.
///
/// Unix keeps the effective uid in the kernel, which is why it is the first
/// shipped basis (`docs/contracts/session-evidence.md` §Crossing).
#[cfg(unix)]
pub(crate) fn trusted_os_user() -> Option<u32> {
    // SAFETY: `geteuid` takes no arguments and has no memory-safety
    // preconditions. Effective uid is the credential governing this
    // process, unlike the freely writable USER/LOGNAME environment.
    Some(unsafe { libc::geteuid() })
}

/// Windows identifies a process by an access token carrying a SID rather than
/// by a numeric uid, and this runtime does not read one yet. `USERNAME` is
/// writable by the process it describes, so returning it would dress a label
/// as a credential. No basis means no authority rather than assumed
/// authority: a policy file declaring principals fails closed on the Windows
/// binary this project ships, and one declaring none behaves exactly as it did
/// before principals existed.
#[cfg(not(unix))]
pub(crate) fn trusted_os_user() -> Option<u32> {
    None
}

fn observe() -> PolicyMode {
    PolicyMode::Observe
}

impl Default for PolicyFile {
    fn default() -> Self {
        Self {
            policy_mode: PolicyMode::Observe,
            constraints: Vec::new(),
            scopes: Vec::new(),
            principals: Vec::new(),
            allow_private_hosts: false,
            refuse_on_pii: false,
            record_internal_prefixes: Vec::new(),
            terms: Vec::new(),
        }
    }
}

pub struct SessionPolicy {
    /// The effective policy, after any matched scope's overlay was applied.
    file: PolicyFile,
    /// The `match` string of the scope that applied, when one did. This is
    /// what a crossing records as `policy_scope`.
    scope: Option<String>,
    /// Governing engagement and its explicit telemetry clearance for the
    /// matched scope. An unmatched cwd has neither, so it cannot leave the
    /// machine.
    governing_engagement: Option<String>,
    allow_telemetry_egress: bool,
    principal: Principal,
    /// The matched binding's periodic allowances, empty for an unbound or
    /// unauthenticated session. Declaration only: the ledger that enforces
    /// them lives in the runtime and is consulted where money moves.
    allowances: Vec<AllowanceDeclaration>,
    fail_closed: Option<String>,
    source: PathBuf,
    loaded: bool,
}

/// `<home>/policy.json` as read, parsed and validated once.
///
/// Reading and resolving are separate operations because the callers that
/// resolve many working directories — the relay over a session's crossings,
/// the console over every recorded cwd — would otherwise reopen and reparse
/// the file once per directory. Holding one document also makes a single
/// invocation reproducible: every decision it takes is against the same bytes,
/// so a policy edited while it runs cannot change its later answers.
pub struct PolicyDocument {
    selection: crate::directory::Selection,
    file: PolicyFile,
    source: PathBuf,
    loaded: bool,
    /// A token over the bytes `file` was parsed from, so an editor can state
    /// what it read before asking to replace it. [`declaration::ABSENT`] when
    /// there is no file.
    revision: String,
    principal: Principal,
}

impl PolicyDocument {
    /// Read and validate `<home>/policy.json`.
    ///
    /// An unreadable or malformed policy file does **not** fall back to
    /// permissive silently: it is reported to the caller, which surfaces it
    /// rather than starting a session whose enforcement nobody chose. A scope
    /// with an empty `match` is malformed — it would silently govern every
    /// session on the machine.
    pub fn read(home: &Path) -> Result<Self, String> {
        let source = home.join("policy.json");
        let selection = crate::directory::Selection::read(home)?;
        let encoded = match std::fs::read(&source) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    file: PolicyFile::default(),
                    selection,
                    source,
                    loaded: false,
                    revision: declaration::ABSENT.to_owned(),
                    principal: Principal::current(),
                });
            }
            Err(error) => return Err(format!("cannot read {}: {error}", source.display())),
        };
        let mut document = Self::from_bytes(&encoded, source)?;
        document.selection = selection;
        Ok(document)
    }

    /// Read a named policy through the ordinary loader. An explicitly named
    /// file must exist; absence is an error rather than the default policy.
    pub fn read_file(source: &Path) -> Result<Self, String> {
        let encoded = std::fs::read(source)
            .map_err(|error| format!("cannot read {}: {error}", source.display()))?;
        Self::from_bytes(&encoded, source.to_path_buf())
    }

    fn from_bytes(encoded: &[u8], source: PathBuf) -> Result<Self, String> {
        Ok(Self {
            file: parse(encoded, &source)?,
            selection: crate::directory::Selection::default(),
            revision: declaration::revision_of(encoded),
            source,
            loaded: true,
            principal: Principal::current(),
        })
    }

    /// Whether a policy file exists at all. False is the adoption on-ramp —
    /// observing, refusing nothing — and not a policy the console may edit:
    /// there is no declaration to change, and inventing one from an editor
    /// would put a stance on every session on the machine that the operator
    /// never wrote.
    pub fn declared(&self) -> bool {
        self.loaded
    }

    /// The token over the bytes this document was parsed from.
    pub fn revision(&self) -> &str {
        &self.revision
    }

    /// This document with one policy mode changed: the scope whose `match` is
    /// `scope`, or the top-level policy when `scope` is `None`. Everything else
    /// in the declaration — constraints, engagements, egress clearance,
    /// recordable prefixes, scope order — is carried through untouched.
    ///
    /// A scope this document does not declare is an error rather than a new
    /// scope. Creating one from the mode dial would silently start governing
    /// directories the operator never named; declaring scopes is the editor's
    /// job, not the dial's.
    pub fn with_mode(&self, scope: Option<&str>, mode: PolicyMode) -> Result<PolicyFile, String> {
        let mut file = self.file.clone();
        match scope {
            None => file.policy_mode = mode,
            Some(matcher) => {
                let target = file
                    .scopes
                    .iter_mut()
                    .find(|scope| scope.matcher == matcher)
                    .ok_or_else(|| {
                        format!(
                            "{} declares no scope matching {matcher:?}",
                            self.source.display()
                        )
                    })?;
                target.policy_mode = Some(mode);
            }
        }
        Ok(file)
    }

    /// This document with one more denied source host, in the scope whose
    /// `match` is `scope`, or in the top-level policy when `scope` is `None`.
    ///
    /// A denylist is a set and this adds one member: the host is normalised
    /// the way admission normalises it, and a host the target already denies
    /// is left alone rather than written twice. Nothing else in the
    /// declaration is touched, and no scope is created — the same rule the
    /// mode dial follows, for the same reason.
    ///
    /// **A scope that stated no constraints of its own inherits the top-level
    /// list**, so writing one constraint into it would replace inheritance
    /// with a list of exactly one and silently drop every top-level
    /// constraint from that scope. The inherited set is materialised first and
    /// the new host appended to it, which keeps enforcement identical the
    /// moment after the save; what changes is that the scope now states its
    /// own list and later top-level edits no longer reach it. Callers are
    /// expected to say so — [`Self::denies_by_inheritance`] answers whether a
    /// save is about to do this.
    pub fn with_denied_host(
        &self,
        scope: Option<&str>,
        host: &str,
    ) -> Result<(PolicyFile, bool), String> {
        let host = normalised_host(host);
        if host.is_empty() {
            return Err("a denied host cannot be empty".to_owned());
        }
        let mut file = self.file.clone();
        let denied = Constraint::DeniedSourceHost { host: host.clone() };
        let already = |constraints: &[Constraint]| {
            constraints.iter().any(|constraint| {
                matches!(constraint, Constraint::DeniedSourceHost { host: declared }
                    if normalised_host(declared) == host)
            })
        };
        match scope {
            None => {
                if already(&file.constraints) {
                    return Ok((file, false));
                }
                file.constraints.push(denied);
            }
            Some(matcher) => {
                let inherited = file.constraints.clone();
                let target = file
                    .scopes
                    .iter_mut()
                    .find(|scope| scope.matcher == matcher)
                    .ok_or_else(|| {
                        format!(
                            "{} declares no scope matching {matcher:?}",
                            self.source.display()
                        )
                    })?;
                let constraints = target.constraints.get_or_insert(inherited);
                if already(constraints) {
                    return Ok((file, false));
                }
                constraints.push(denied);
            }
        }
        Ok((file, true))
    }

    /// Whether `scope` currently takes its constraints from the top-level
    /// policy, so a first constraint written into it ends that inheritance.
    /// `None` — the top-level policy — inherits from nothing.
    pub fn denies_by_inheritance(&self, scope: Option<&str>) -> bool {
        scope.is_some_and(|matcher| {
            self.file
                .scopes
                .iter()
                .any(|scope| scope.matcher == matcher && scope.constraints.is_none())
        })
    }

    /// Replace `<home>/policy.json` with `file`, atomically.
    ///
    /// The candidate bytes are put back through [`parse`] — the same
    /// validation [`Self::read`] performs — before anything is written, so no
    /// caller can leave an operator with a policy this runtime would refuse to
    /// load, and every session on the machine locked out of a file that no
    /// longer parses.
    ///
    /// The file is rewritten canonically: the serialised form of what was
    /// declared, not the operator's own formatting.
    pub fn save(home: &Path, file: &PolicyFile) -> Result<(), String> {
        let target = home.join("policy.json");
        let encoded = serde_json::to_vec_pretty(file)
            .map_err(|error| format!("cannot serialise the policy: {error}"))?;
        parse(&encoded, &target)?;
        declaration::replace(&target, &encoded)
    }

    /// Hold this home's policy file exclusively, across processes, for as long
    /// as the returned guard lives. A revision check and the rename that
    /// follows it have to be one step against any other writer.
    pub fn lock(home: &Path) -> Result<declaration::Lock, declaration::LockRefused> {
        declaration::lock(&home.join("policy.lock"))
    }

    /// The scopes as declared, in their declared order.
    ///
    /// For the readers that need to say something about a scope no working
    /// directory matched — the console names them, because a matcher with a
    /// typo governs nothing and would otherwise be visible nowhere. Without
    /// this they reopened and reparsed the file, which reintroduced per-read
    /// disagreement inside one answer and turned an unreadable policy into an
    /// empty scope list.
    pub fn scopes(&self) -> &[PolicyScope] {
        &self.file.scopes
    }

    /// Every declared principal binding, for the surfaces that report the
    /// whole file rather than one resolved session — the console's allowance
    /// panel reads each binding's declarations here, from the same parsed
    /// document the stances came from.
    pub fn principals(&self) -> &[PrincipalBinding] {
        &self.file.principals
    }
}

/// A candidate declaration: what an editor builds from a document before
/// asking to save it, and what a forecast judges in place of the document.
impl PolicyDocument {
    /// The declaration as parsed. Reading it is not editing it; an editor
    /// clones it to build a candidate, and nothing here writes.
    pub fn file(&self) -> &PolicyFile {
        &self.file
    }

    /// This document with `file` in place of its declaration and everything
    /// else kept: the source path, the revision of the bytes on disk and the
    /// principal. A candidate built this way resolves and admits exactly as it
    /// would once saved, so a forecast over it is the runtime's own answer and
    /// not a second policy engine's. Nothing is written.
    pub fn with_file(&self, file: PolicyFile) -> Self {
        Self {
            file,
            source: self.source.clone(),
            selection: self.selection.clone(),
            loaded: self.loaded,
            revision: self.revision.clone(),
            principal: self.principal.clone(),
        }
    }

    /// This document with one access rule appended to the scope whose `match`
    /// is `scope`, or to the top-level policy when `scope` is `None`.
    ///
    /// Appended, not inserted: the rules are read in order and the first
    /// match decides, so a rule added here is read after every rule already
    /// declared. A rule that has to come first is an edit to the file. A
    /// scope that inherited the top-level constraints materialises them
    /// first, exactly as [`Self::with_denied_host`] does and for the same
    /// reason; no scope is created.
    pub fn with_access_rule(
        &self,
        scope: Option<&str>,
        host: commonmeasure_types::HostPattern,
        action: commonmeasure_types::AccessAction,
    ) -> Result<PolicyFile, String> {
        action.validate()?;
        let mut file = self.file.clone();
        let rule = Constraint::AccessRule { host, action };
        match scope {
            None => file.constraints.push(rule),
            Some(matcher) => {
                let inherited = file.constraints.clone();
                let target = file
                    .scopes
                    .iter_mut()
                    .find(|scope| scope.matcher == matcher)
                    .ok_or_else(|| {
                        format!(
                            "{} declares no scope matching {matcher:?}",
                            self.source.display()
                        )
                    })?;
                target.constraints.get_or_insert(inherited).push(rule);
            }
        }
        Ok(file)
    }
}

/// Policy bytes as a validated [`PolicyFile`]. `source` only names the file in
/// the error, so a reader is told which file to fix.
///
/// The one validity rule, shared by [`PolicyDocument::read`] and
/// [`PolicyDocument::save`], so nothing can write a policy a load would refuse.
fn parse(encoded: &[u8], source: &Path) -> Result<PolicyFile, String> {
    let file: PolicyFile = serde_json::from_slice(encoded)
        .map_err(|error| format!("{} is not a valid policy: {error}", source.display()))?;
    // A prefix that cannot parse as an absolute URL can never match a
    // crossing, and an entry that silently matches nothing would leave the
    // operator believing their corpus is on the record when it is not.
    // The trailing slash is the consent boundary: prefixes match the raw
    // URL string, so "https://rag.internal" would also match
    // "https://rag.internal.evil.internal/x" and
    // "https://rag.internal@other-host/y" — hosts the operator never
    // named. Requiring the "/" makes every match end at a component the
    // operator wrote.
    for prefix in &file.record_internal_prefixes {
        if url::Url::parse(prefix).is_err() {
            return Err(format!(
                "{}: record_internal_prefixes entry {prefix:?} is not an absolute URL prefix and \
                 would never match anything",
                source.display()
            ));
        }
        if !prefix.ends_with('/') {
            return Err(format!(
                "{}: record_internal_prefixes entry {prefix:?} must end with \"/\" — without it \
                 the prefix also matches hosts and paths it merely starts, which would record \
                 more than the operator named",
                source.display()
            ));
        }
    }
    if file.scopes.iter().any(|scope| scope.matcher.is_empty()) {
        return Err(format!(
            "{} has a scope with an empty \"match\", which would govern everything",
            source.display()
        ));
    }
    // Two scopes with one matcher are one name on every keyed surface:
    // `resolve` applies the first, `with_mode` edits the first, and the
    // console keys a scope's governed work and dial row by the matcher
    // string — so the duplicate renders as two rows with one name whose
    // controls both edit the first, while the shadowed second is editable
    // from no surface.
    for (index, scope) in file.scopes.iter().enumerate() {
        if file.scopes[..index]
            .iter()
            .any(|earlier| earlier.matcher == scope.matcher)
        {
            return Err(format!(
                "{} declares two scopes matching {:?}; only the first would govern or take an \
                 edit, so the second must be merged into it or renamed",
                source.display(),
                scope.matcher
            ));
        }
    }
    for (index, binding) in file.principals.iter().enumerate() {
        if binding.principal.is_empty() {
            return Err(format!(
                "{} has a principal with an empty name",
                source.display()
            ));
        }
        // Each collision is reported as the specific pair it is. "Something
        // here is ambiguous" leaves the operator to find the other half of a
        // conflict this loop is already holding.
        if let Some(earlier) = file.principals[..index]
            .iter()
            .find(|earlier| earlier.principal == binding.principal)
        {
            return Err(format!(
                "{} declares principal {:?} twice, for OS users {} and {}",
                source.display(),
                binding.principal,
                earlier.os_user,
                binding.os_user
            ));
        }
        if let Some(earlier) = file.principals[..index]
            .iter()
            .find(|earlier| earlier.os_user == binding.os_user)
        {
            return Err(format!(
                "{} binds OS user {} to both {:?} and {:?}; one identity cannot hold two principals",
                source.display(),
                binding.os_user,
                earlier.principal,
                binding.principal
            ));
        }
        // A principal required to work inside a scope bound to it, with no
        // such scope declared, can never be admitted anywhere. That is a
        // deleted scope or a typo far more often than a deliberate denial,
        // and a policy nobody can use is not a policy to start a server on.
        if binding.require_scope
            && !file
                .scopes
                .iter()
                .any(|scope| scope.principal.as_ref() == Some(&binding.principal))
        {
            return Err(format!(
                "{} requires principal {:?} to work in a directory scope bound to it and declares none, so nothing could ever be admitted for it; bind a scope or drop require_scope",
                source.display(),
                binding.principal
            ));
        }
        for (allowance_index, declared) in binding.allowances.iter().enumerate() {
            if let Err(reason) = declared.validate() {
                return Err(format!(
                    "{} principal {:?} {} allowance {reason}",
                    source.display(),
                    binding.principal,
                    declared.period.as_str()
                ));
            }
            // One period holds one amount: the ledger checks a purchase
            // against "the day allowance", so a second day declaration would
            // silently never bind.
            if let Some(earlier) = binding.allowances[..allowance_index]
                .iter()
                .find(|earlier| earlier.period == declared.period)
            {
                return Err(format!(
                    "{} principal {:?} declares two {} allowances of {} and {}; one period \
                     holds one amount, and the second would silently never bind",
                    source.display(),
                    binding.principal,
                    declared.period.as_str(),
                    earlier.amount.as_decimal_string(),
                    declared.amount.as_decimal_string()
                ));
            }
        }
    }
    // Access rules. The host pattern refused itself while parsing, so what is
    // left to check is the action; the top-level list, every scope's own
    // list and every principal's are checked alike, because each is the list
    // admission reads for some session.
    let lists = std::iter::once(("the top-level policy".to_owned(), &file.constraints))
        .chain(file.scopes.iter().filter_map(|scope| {
            scope
                .constraints
                .as_ref()
                .map(|constraints| (format!("scope {:?}", scope.matcher), constraints))
        }))
        .chain(file.principals.iter().filter_map(|binding| {
            binding
                .constraints
                .as_ref()
                .map(|constraints| (format!("principal {:?}", binding.principal), constraints))
        }));
    for (owner, constraints) in lists {
        for (index, constraint) in constraints.iter().enumerate() {
            if let Constraint::AccessRule { action, .. } = constraint
                && let Err(reason) = action.validate()
            {
                return Err(format!(
                    "{} {owner} constraint {}: {reason}",
                    source.display(),
                    index + 1
                ));
            }
        }
    }
    for scope in &file.scopes {
        if scope.engagement.as_deref() == Some("") {
            return Err(format!(
                "{} has a scope with an empty \"engagement\"",
                source.display()
            ));
        }
        if scope.allow_telemetry_egress && scope.engagement.is_none() {
            return Err(format!(
                "{} scope {:?} allows telemetry egress without naming an engagement",
                source.display(),
                scope.matcher
            ));
        }
        if let Some(principal) = &scope.principal
            && !file
                .principals
                .iter()
                .any(|binding| &binding.principal == principal)
        {
            return Err(format!(
                "{} scope {:?} names undeclared principal {:?}",
                source.display(),
                scope.matcher,
                principal
            ));
        }
    }
    // Terms: the top-level list and every scope's own. A host that appears
    // twice in one list would make the recorded reference depend on order,
    // and an entry naming no host or no reference could govern nothing.
    let terms_lists = std::iter::once(("the top-level policy".to_owned(), &file.terms)).chain(
        file.scopes.iter().filter_map(|scope| {
            scope
                .terms
                .as_ref()
                .map(|terms| (format!("scope {:?}", scope.matcher), terms))
        }),
    );
    for (owner, terms) in terms_lists {
        for (index, declared) in terms.iter().enumerate() {
            let host = normalised_host(&declared.host);
            if host.is_empty() {
                return Err(format!(
                    "{} {owner} terms entry {} names no host",
                    source.display(),
                    index + 1
                ));
            }
            if declared.reference.trim().is_empty() {
                return Err(format!(
                    "{} {owner} terms entry {} for host {host:?} names no reference; the \
                     reference is what the record and the wire carry",
                    source.display(),
                    index + 1
                ));
            }
            if terms[..index]
                .iter()
                .any(|earlier| normalised_host(&earlier.host) == host)
            {
                return Err(format!(
                    "{} {owner} declares terms for host {host:?} twice; one host holds one \
                     agreement",
                    source.display()
                ));
            }
            for identifier in &declared.access_context {
                if identifier.scheme.trim().is_empty() || identifier.value.trim().is_empty() {
                    return Err(format!(
                        "{} {owner} terms entry {} for host {host:?} has an access_context \
                         identifier without a scheme or a value",
                        source.display(),
                        index + 1
                    ));
                }
            }
        }
    }
    Ok(file)
}

impl PolicyDocument {
    /// Resolve this document's scopes against one working directory: first
    /// scope whose `match` is a substring of the cwd wins, no match falls
    /// through to the top-level policy.
    ///
    /// Infallible by construction — [`Self::read`] already refused everything
    /// malformed, so resolution has nothing left to reject.
    pub fn resolve(&self, cwd: Option<&str>) -> SessionPolicy {
        let canonical = cwd.and_then(|p| std::fs::canonicalize(p).ok());
        let actual = canonical.as_ref().and_then(|p| p.to_str()).or(cwd);
        let mut resolved = self.resolve_source(actual);
        if cwd != actual
            && self.resolve_source(cwd).scope != resolved.scope
            && self.resolve_source(cwd).scope.is_some()
        {
            resolved.fail_closed = Some(
                "symlink and selected directory resolve different source-policy scopes".into(),
            );
        }
        if let Some(path) = canonical.as_ref() {
            match crate::directory::git_identity(path) {
                Ok(Some((common, root))) if common != root.join(".git") => {
                    let original = common
                        .parent()
                        .map(|main| main.join(path.strip_prefix(&root).unwrap_or(Path::new(""))));
                    if let Some(original) = original {
                        let main = self.resolve_source(original.to_str());
                        if main.scope != resolved.scope && main.scope.is_some() {
                            resolved.fail_closed = Some("linked Git worktree differs from its main worktree source-policy scope; align policy before retrieval".into());
                        }
                    }
                }
                Err(error) => {
                    resolved.fail_closed =
                        Some(format!("Git directory identity unavailable: {error}"))
                }
                _ => {}
            }
        }
        if let Some(registry) = &self.selection.registry {
            let project = actual.and_then(|cwd| registry.matching(cwd));
            // Every false winning scope vetoes a grant, including an omitted bool.
            let veto = resolved.scope.is_some() && !resolved.allow_telemetry_egress;
            let permitted = project.is_some_and(|p| !veto && self.selection.permitted(p));
            resolved.allow_telemetry_egress = permitted;
            if permitted && resolved.governing_engagement.is_none() {
                resolved.governing_engagement = project.map(|p| p.name.clone());
            }
        }
        if resolved.fail_closed.is_some() {
            resolved.allow_telemetry_egress = false;
        }
        resolved
    }

    fn resolve_source(&self, cwd: Option<&str>) -> SessionPolicy {
        let mut file = self.file.clone();
        let mut principal = self.principal.clone();
        // An unauthenticated process has no subject to match on, so nothing
        // selects a binding and the fail-closed branch below governs it.
        let binding = principal.os_user.and_then(|os_user| {
            file.principals
                .iter()
                .find(|binding| binding.os_user == os_user)
                .cloned()
        });
        let mut fail_closed = None;
        if let Some(binding) = &binding {
            principal.name.clone_from(&binding.principal);
            if let Some(mode) = binding.policy_mode {
                file.policy_mode = mode;
            }
            if let Some(constraints) = &binding.constraints {
                file.constraints.clone_from(constraints);
            }
            if let Some(allow) = binding.allow_private_hosts {
                file.allow_private_hosts = allow;
            }
        } else if !file.principals.is_empty() {
            fail_closed = Some(match principal.os_user {
                Some(os_user) => format!("effective OS user {os_user} has no principal binding"),
                None => format!(
                    "this platform authenticates no identity (basis {}), so no principal binding can be resolved",
                    principal.basis.as_str()
                ),
            });
        }
        // The directory alone selects the scope, exactly as it did before
        // principals existed: first match wins.
        let matched = cwd.and_then(|cwd| {
            let position = file.scopes.iter().position(|scope| {
                cwd.contains(&scope.matcher)
                    || (Path::new(&scope.matcher).is_absolute()
                        && std::fs::canonicalize(&scope.matcher)
                            .ok()
                            .is_some_and(|root| Path::new(cwd).starts_with(root)))
            })?;
            Some(file.scopes.swap_remove(position))
        });
        // Ownership is then checked, never searched past. Walking on to the
        // next matching scope would hand this session an overlay the operator
        // wrote for a broader directory, which is very often the looser one:
        // adding `"principal"` to a narrow strict scope would then quietly
        // relax every other principal standing in it. A scope that names an
        // owner is that owner's authority, and standing in the directory is
        // not a claim on it.
        let matched = match matched {
            Some(scope) => match &scope.principal {
                Some(owner) if owner != &principal.name => {
                    // An identity that could not be authenticated at all has
                    // already recorded the more fundamental reason.
                    fail_closed.get_or_insert_with(|| {
                        format!(
                            "principal {:?} may not use directory scope {:?}, which belongs to principal {:?}",
                            principal.name, scope.matcher, owner
                        )
                    });
                    None
                }
                _ => Some(scope),
            },
            None => None,
        };
        if let Some(binding) = &binding
            && binding.require_scope
            && !matched
                .as_ref()
                .is_some_and(|scope| scope.principal.as_deref() == Some(&binding.principal))
        {
            fail_closed.get_or_insert_with(|| {
                format!(
                    "principal {:?} requires a matching principal-bound directory scope",
                    binding.principal
                )
            });
        }
        let mut governing_engagement = None;
        let mut allow_telemetry_egress = false;
        let scope = matched.map(|overlay| {
            if let Some(mode) = overlay.policy_mode {
                file.policy_mode = mode;
            }
            if let Some(constraints) = overlay.constraints {
                file.constraints = constraints;
            }
            if let Some(allow) = overlay.allow_private_hosts {
                file.allow_private_hosts = allow;
            }
            if let Some(refuse) = overlay.refuse_on_pii {
                file.refuse_on_pii = refuse;
            }
            if let Some(terms) = overlay.terms {
                file.terms = terms;
            }
            governing_engagement = overlay.engagement;
            allow_telemetry_egress = overlay.allow_telemetry_egress;
            overlay.matcher
        });
        SessionPolicy {
            file,
            scope,
            governing_engagement,
            allow_telemetry_egress,
            allowances: binding
                .map(|binding| binding.allowances)
                .unwrap_or_default(),
            principal,
            fail_closed,
            source: self.source.clone(),
            loaded: self.loaded,
        }
    }
}

impl SessionPolicy {
    /// Read `<home>/policy.json` and resolve it against one working directory.
    ///
    /// The one-shot form, for the callers that resolve a single cwd. Callers
    /// resolving many should read a [`PolicyDocument`] once and resolve
    /// against it.
    pub fn load(home: &Path, cwd: Option<&str>) -> Result<Self, String> {
        Ok(PolicyDocument::read(home)?.resolve(cwd))
    }

    /// The `match` string of the scope governing this session, if one applied.
    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    pub fn principal(&self) -> &str {
        &self.principal.name
    }

    /// How [`Self::principal`] was established. `unavailable` is a real
    /// answer rather than a missing one, and is recorded as such.
    pub fn authentication_basis(&self) -> AuthenticationBasis {
        self.principal.basis
    }

    /// The governing engagement the matched scope declared, if any: the name
    /// under whose clearance a crossing here may be projected. Never the name
    /// a console reports this work under — see [`PolicyScope::engagement`].
    pub fn governing_engagement(&self) -> Option<&str> {
        self.governing_engagement.as_deref()
    }

    /// Whether this resolved governing engagement may leave through telemetry.
    /// Unmatched, unnamed and undeclared scopes all return false.
    pub fn allows_telemetry_egress(&self) -> bool {
        self.governing_engagement.is_some() && self.allow_telemetry_egress
    }

    pub fn mode(&self) -> PolicyMode {
        self.file.policy_mode
    }

    /// Whether `strict` refuses a PII finding on every source, rather than
    /// on internal and private sources only ([`PolicyFile::refuse_on_pii`]).
    pub fn refuse_on_pii(&self) -> bool {
        self.file.refuse_on_pii
    }

    pub fn constraints(&self) -> &[Constraint] {
        &self.file.constraints
    }

    /// The resolved principal's periodic allowances, empty when no binding
    /// matched. What the operator allocated; the runtime's ledger is what
    /// enforces it.
    pub fn allowances(&self) -> &[AllowanceDeclaration] {
        &self.allowances
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Check one already-normalised source against the standing policy.
    pub fn admit(&self, envelope: &ContextEnvelope) -> Ruling {
        if let Some(reason) = &self.fail_closed {
            return Ruling::Refused {
                reason: format!("Principal authority refused: {reason}."),
                gap: commonmeasure_types::Gap::new(
                    commonmeasure_types::GapReason::PolicyRefused,
                    "No authenticated principal policy authorised this crossing.",
                ),
            };
        }
        policy::source_admission(&crate::mcp::policy_job(self), envelope)
    }

    /// Check a bare URL before its bytes exist.
    pub fn admit_host(&self, url: &str) -> Ruling {
        self.admit(&crate::mcp::envelope_for(url))
    }

    /// Whether the mediated tools may reach this address at all. Separate from
    /// admission: this is the privacy floor, not the operator's source policy.
    /// A named internal prefix is narrower consent than `allow_private_hosts`
    /// and lifts the floor for that prefix alone.
    pub fn mediates_address(&self, url: &str) -> bool {
        self.file.allow_private_hosts || self.records_address(url)
    }

    /// Whether a crossing to this address may enter the record at all: the
    /// privacy floor, lowered only where the operator named an internal
    /// prefix. This is what observed capture consults, so passive capture
    /// records an internal corpus only when `policy.json` says so in writing.
    pub fn records_address(&self, url: &str) -> bool {
        crate::grounding::recordable_under(url, &self.file.record_internal_prefixes)
    }

    /// The internal prefixes the operator named as recordable.
    pub fn internal_prefixes(&self) -> &[String] {
        &self.file.record_internal_prefixes
    }

    /// The terms the operator holds for one host, where the effective policy
    /// declares any. Matched on the normalised host, exactly.
    pub fn terms_for(&self, host: &str) -> Option<&TermsDeclaration> {
        let wanted = normalised_host(host);
        if wanted.is_empty() {
            return None;
        }
        self.file
            .terms
            .iter()
            .find(|declared| normalised_host(&declared.host) == wanted)
    }

    pub fn describe(&self) -> Value {
        json!({
            "mode": self.file.policy_mode,
            "source": if self.loaded {
                self.source.display().to_string()
            } else {
                format!("{} (absent; observing, refusing nothing)", self.source.display())
            },
            // Which scoped overlay governs this session, so the agent reading
            // its own status sees the same name every crossing will record.
            "scope": self.scope,
            "principal": self.principal.name,
            "authentication_basis": self.principal.basis.as_str(),
            "authenticated_subject": self.principal.os_user.map(|uid| format!("uid:{uid}")),
            "asserted_principal": self.principal.asserted_label,
            "fail_closed": self.fail_closed,
            // Named for what it is, because a reader of this document may also
            // be reading a reported engagement resolved by different rules.
            "governing_engagement": self.governing_engagement,
            "allow_telemetry_egress": self.allow_telemetry_egress,
            // Allocation as declared; the remaining amounts live in the
            // runtime's ledger, not here, so this surface cannot drift from
            // what enforcement reads.
            "allowances": self.allowances,
            "constraints": self.file.constraints,
            "allow_private_hosts": self.file.allow_private_hosts,
            "refuse_on_pii": self.file.refuse_on_pii,
            // On the status surface deliberately: the console must be able to
            // say what internal supply is being recorded and on whose
            // authority. An empty list is the floor holding everywhere.
            "record_internal_prefixes": self.file.record_internal_prefixes,
            // The digest of the effective policy this session resolved, so a
            // reader of the status can match it against the identity every
            // mediated crossing records.
            "policy_identity": self.identity().to_value(),
            // The agreements the operator declared, references and
            // identifiers only; what governs a crossing is recorded on it.
            "terms": self.file.terms,
        })
    }
}

// ---------------------------------------------------------------------------
// Canonical effective policy and its identity
// ---------------------------------------------------------------------------

/// The version of the pre-image [`SessionPolicy::canonical`] builds. It moves
/// when a field is added to or removed from the pre-image, because every
/// digest changes with it and a reader comparing digests across edges has
/// to know they were computed over the same shape.
pub const IDENTITY_SCHEMA: &str = "contextops-policy-identity/v2";

/// The version of the resolution [`PolicyDocument::resolve`] performs: which
/// scope wins, how a principal overlays, when a session fails closed. It
/// moves when that algorithm changes, so two edges resolving one policy
/// document differently because they run different resolvers do not hash
/// equal.
pub const RESOLVER_VERSION: &str = "2";

/// The value the add-on slot of the pre-image carries until the active
/// add-on set is part of policy. Every processor compiled into the binary
/// runs today, so there is no active set to represent; the slot is present
/// and says so rather than being absent, and it becomes the set with its
/// settings when add-on management lands, under the next schema version.
pub const ADDONS_UNKNOWN: &str = "unknown";

/// The identity of one resolved policy: the digest over its canonical
/// pre-image, with the schema and resolver versions the digest was computed
/// under. What a mediated crossing records, what the fleet-status document
/// reports as the applied identity, and what a reviewer recomputes from the
/// pre-image (`docs/contracts/fleet-status.md` §Policy identity).
///
/// This is drift evidence: two edges reporting one digest resolved the same
/// effective policy. It is not proof that an uncompromised edge enforced it,
/// because the edge computes and reports it about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyIdentity {
    pub schema: String,
    pub resolver: String,
    /// `sha256:<hex>` over [`canonical_json`] of the pre-image.
    pub digest: String,
}

impl PolicyIdentity {
    pub fn to_value(&self) -> Value {
        json!({
            "schema": self.schema,
            "resolver": self.resolver,
            "digest": self.digest,
        })
    }
}

/// One constraint as the pre-image carries it: hosts normalised the way
/// admission normalises them, so two spellings admission treats as one host
/// hash as one.
fn canonical_constraint(constraint: &Constraint) -> Value {
    match constraint {
        Constraint::AllowedSourceHost { host } => json!({
            "kind": "allowed_source_host", "host": normalised_host(host),
        }),
        Constraint::DeniedSourceHost { host } => json!({
            "kind": "denied_source_host", "host": normalised_host(host),
        }),
        other => serde_json::to_value(other).unwrap_or(Value::Null),
    }
}

impl SessionPolicy {
    /// The canonical pre-image of this resolved policy: every fact the
    /// resolution produced that changes what a crossing meets, and nothing
    /// that does not. The field set is fixed by [`IDENTITY_SCHEMA`] and
    /// listed in `docs/contracts/fleet-status.md` §Policy identity.
    ///
    /// Lists whose order carries no meaning (the set-semantics constraints,
    /// prefixes, allowances) are sorted, so reordering a policy file is not
    /// drift. Access rules are the exception: the first matching rule
    /// decides, so their order is the policy, and they are carried in
    /// declaration order with their position (`access_rules`), apart from
    /// the sorted set (`constraints`). The operator's terms are carried
    /// too, because they govern over a source's published preference, with
    /// hosts normalised the way `terms_for` matches them. The source path,
    /// the asserted label and the authenticated subject id are left out: they describe the
    /// machine and the process, not the policy. A scope that did not match
    /// this session is left out too, so an edit to an unrelated scope leaves
    /// this identity unchanged.
    pub fn canonical(&self) -> Value {
        let mut constraints: Vec<String> = self
            .file
            .constraints
            .iter()
            .filter(|constraint| !matches!(constraint, Constraint::AccessRule { .. }))
            .map(|constraint| canonical_json(&canonical_constraint(constraint)))
            .collect();
        constraints.sort();
        constraints.dedup();
        let constraints: Vec<Value> = constraints
            .iter()
            .map(|text| serde_json::from_str(text).unwrap_or(Value::Null))
            .collect();
        let access_rules: Vec<Value> = self
            .file
            .constraints
            .iter()
            .filter(|constraint| matches!(constraint, Constraint::AccessRule { .. }))
            .enumerate()
            .map(|(position, rule)| {
                let mut rule = serde_json::to_value(rule).unwrap_or(Value::Null);
                rule["position"] = json!(position);
                rule
            })
            .collect();
        let mut prefixes = self.file.record_internal_prefixes.clone();
        prefixes.sort();
        prefixes.dedup();
        let mut allowances: Vec<String> = self
            .allowances
            .iter()
            .map(|declared| {
                canonical_json(&json!({
                    "period": declared.period.as_str(),
                    "amount": declared.amount,
                    "timezone": declared.timezone,
                }))
            })
            .collect();
        allowances.sort();
        let allowances: Vec<Value> = allowances
            .iter()
            .map(|text| serde_json::from_str(text).unwrap_or(Value::Null))
            .collect();
        let mut terms: Vec<String> = self
            .file
            .terms
            .iter()
            .map(|declared| {
                let mut declared = serde_json::to_value(declared).unwrap_or(Value::Null);
                declared["host"] = json!(normalised_host(
                    declared["host"].as_str().unwrap_or_default()
                ));
                canonical_json(&declared)
            })
            .collect();
        terms.sort();
        terms.dedup();
        let terms: Vec<Value> = terms
            .iter()
            .map(|text| serde_json::from_str(text).unwrap_or(Value::Null))
            .collect();
        json!({
            "schema": IDENTITY_SCHEMA,
            "resolver": RESOLVER_VERSION,
            "mode": self.file.policy_mode,
            "constraints": constraints,
            "access_rules": access_rules,
            "allow_private_hosts": self.file.allow_private_hosts,
            "refuse_on_pii": self.file.refuse_on_pii,
            "record_internal_prefixes": prefixes,
            "scope": self.scope,
            "governing_engagement": self.governing_engagement,
            "allow_telemetry_egress": self.allow_telemetry_egress,
            "principal": {
                "name": self.principal.name,
                "basis": self.principal.basis.as_str(),
            },
            "fail_closed": self.fail_closed,
            "allowances": allowances,
            "addons": ADDONS_UNKNOWN,
            "terms": terms,
        })
    }

    /// The digest of [`Self::canonical`], with the versions it was taken
    /// under.
    pub fn identity(&self) -> PolicyIdentity {
        PolicyIdentity {
            schema: IDENTITY_SCHEMA.to_owned(),
            resolver: RESOLVER_VERSION.to_owned(),
            digest: canonical_digest(&self.canonical()),
        }
    }
}

/// The JSON Schema of the policy file, derived from the types a load parses
/// with, so the schema an author validates against cannot drift from what the
/// loader accepts.
///
/// It is the structural half of the policy's contract and no more: the checks
/// [`parse`] makes after deserialising — an empty scope match, egress cleared
/// without an engagement, a recordable prefix that ends at no component — are
/// not expressible in a schema and are stated in the contract and enforced by
/// the loader alone (`docs/contracts/source-policy.md`).
///
/// The derived descriptions are removed. They are this crate's doc comments,
/// written for someone reading the loader; the contract is what explains a
/// field to an author, and a published schema that moved with every comment
/// edit would report structural change where there was none.
pub fn schema() -> Value {
    fn without_descriptions(value: Value) -> Value {
        match value {
            Value::Object(members) => Value::Object(
                members
                    .into_iter()
                    // Only a string-valued `description` is annotation; a
                    // property named `description` would carry a schema object.
                    .filter(|(key, member)| !(key == "description" && member.is_string()))
                    .map(|(key, member)| (key, without_descriptions(member)))
                    .collect(),
            ),
            Value::Array(items) => {
                Value::Array(items.into_iter().map(without_descriptions).collect())
            }
            other => other,
        }
    }
    let mut schema = without_descriptions(
        serde_json::to_value(schemars::schema_for!(PolicyFile))
            .expect("a derived schema serialises to JSON"),
    );
    schema["title"] = json!("Common Measure source policy");
    schema
}

impl PolicyDocument {
    /// Parse and validate candidate policy bytes exactly as a load does,
    /// naming `source` in the refusal.
    ///
    /// For callers that hold a document rather than an operator home: the
    /// `policy check` command, and anything holding a policy before it is
    /// installed anywhere.
    pub fn check(encoded: &[u8], source: &Path) -> Result<PolicyFile, String> {
        parse(encoded, source)
    }

    /// The one validity rule, applied to a policy before it is written by
    /// anything other than [`Self::save`]: the same rule a load enforces.
    pub fn validate(file: &PolicyFile) -> Result<(), String> {
        let encoded = serde_json::to_vec(file)
            .map_err(|error| format!("cannot serialise the policy: {error}"))?;
        parse(&encoded, Path::new("policy.json")).map(|_| ())
    }

    /// The digest of the declaration as parsed, over its canonical
    /// serialisation rather than its bytes, so a reformatted file and a
    /// distributed copy of the same declaration digest equal. `None` when
    /// there is no file: an absent declaration has no digest to compare with
    /// a desired one. [`Self::revision`] is the byte-level token the editor
    /// uses and stays distinct.
    pub fn digest(&self) -> Option<String> {
        self.loaded
            .then(|| canonical_digest(&serde_json::to_value(&self.file).unwrap_or(Value::Null)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_in(json: &str, cwd: Option<&str>) -> SessionPolicy {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join("policy.json"), json).unwrap();
        // Leak the directory for the test's lifetime: the policy holds only its
        // path and reads nothing further.
        let policy = SessionPolicy::load(directory.path(), cwd).expect("valid policy");
        std::mem::forget(directory);
        policy
    }

    /// `os_user` is `None` where the platform authenticates nothing, which is
    /// what [`Principal::current`] resolves to off unix.
    fn policy_for(json: &str, cwd: Option<&str>, os_user: Option<u32>) -> SessionPolicy {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join("policy.json"), json).unwrap();
        let mut document = PolicyDocument::read(directory.path()).expect("valid policy");
        document.principal = Principal::for_test(os_user, Some("someone-else"));
        document.resolve(cwd)
    }

    #[test]
    fn authenticated_principal_resolves_before_its_directory_scope() {
        let policy = policy_for(
            r#"{"policy_mode":"observe","principals":[{"principal":"alice","os_user":1001,"require_scope":true,"policy_mode":"strict"}],"scopes":[{"match":"alice-work","principal":"alice","constraints":[{"kind":"allowed_source_host","host":"allowed.example"}]}]}"#,
            Some("/work/alice-work"),
            Some(1001),
        );
        assert_eq!(policy.principal(), "alice");
        assert_eq!(policy.scope(), Some("alice-work"));
        assert_eq!(policy.mode(), PolicyMode::Strict);
        assert!(policy.admit_host("https://denied.example/").is_refusal());
        assert_eq!(policy.describe()["asserted_principal"], "someone-else");
    }

    #[test]
    fn changing_cwd_and_asserted_label_cannot_acquire_another_principals_scope() {
        let policy = policy_for(
            r#"{"principals":[{"principal":"alice","os_user":1001,"require_scope":true},{"principal":"bob","os_user":1002,"require_scope":true}],"scopes":[{"match":"alice-work","principal":"alice"},{"match":"bob-work","principal":"bob","policy_mode":"observe"}]}"#,
            Some("/work/bob-work"),
            Some(1001),
        );
        assert_eq!(policy.principal(), "alice");
        assert_eq!(policy.scope(), None);
        assert!(policy.admit_host("https://example.com/").is_refusal());
        assert!(
            policy.describe()["fail_closed"]
                .as_str()
                .unwrap()
                .contains("may not use directory scope")
        );
    }

    /// A scope naming an owner is that owner's authority, and standing in the
    /// directory is not a claim on it. The refusal matters most where the
    /// scope beneath is looser: skipping past an owned scope to the next
    /// match would let an operator relax policy for everyone else by writing
    /// what reads as a restriction — adding `"principal": "bob"` to a strict
    /// `/work/secret` scope would move every other principal working there to
    /// the broader, unconstrained `/work` scope.
    #[test]
    fn another_principals_scope_refuses_rather_than_falling_through_to_a_looser_one() {
        let policy = policy_for(
            r#"{"policy_mode":"observe","principals":[{"principal":"alice","os_user":1001},{"principal":"bob","os_user":1002}],"scopes":[{"match":"/work/secret","principal":"bob","policy_mode":"strict","constraints":[{"kind":"allowed_source_host","host":"allowed.example"}]},{"match":"/work","policy_mode":"observe"}]}"#,
            Some("/work/secret/notes"),
            Some(1001),
        );
        assert_eq!(policy.principal(), "alice");
        assert_eq!(policy.scope(), None);
        assert!(policy.admit_host("https://anything.example/").is_refusal());
        assert!(
            policy.describe()["fail_closed"]
                .as_str()
                .unwrap()
                .contains("may not use directory scope")
        );
    }

    /// The other fail-closed reason stays distinct: this principal owns a
    /// scope, and is working outside every directory bound to it.
    #[test]
    fn a_required_scope_no_directory_matches_fails_closed_naming_the_requirement() {
        let policy = policy_for(
            r#"{"principals":[{"principal":"alice","os_user":1001,"require_scope":true}],"scopes":[{"match":"alice-work","principal":"alice"}]}"#,
            Some("/work/elsewhere"),
            Some(1001),
        );
        assert_eq!(policy.scope(), None);
        assert!(policy.admit_host("https://example.com/").is_refusal());
        assert!(
            policy.describe()["fail_closed"]
                .as_str()
                .unwrap()
                .contains("requires a matching principal-bound directory scope")
        );
    }

    /// The Windows binary this project ships has no uid for
    /// [`Principal::current`] to read (`plugin/build.sh`). An operator's
    /// principal bindings must not quietly stop applying there.
    #[test]
    fn a_platform_that_authenticates_nothing_fails_closed_under_principal_policy() {
        let policy = policy_for(
            r#"{"policy_mode":"observe","principals":[{"principal":"alice","os_user":1001}],"scopes":[{"match":"alice-work","principal":"alice"}]}"#,
            Some("/work/alice-work"),
            None,
        );
        assert_eq!(policy.principal(), "unauthenticated");
        assert_eq!(
            policy.authentication_basis(),
            AuthenticationBasis::Unavailable
        );
        assert_eq!(policy.scope(), None);
        assert!(policy.admit_host("https://example.com/").is_refusal());
        assert!(policy.describe()["authenticated_subject"].is_null());
        assert!(
            policy.describe()["fail_closed"]
                .as_str()
                .unwrap()
                .contains("authenticates no identity")
        );
    }

    /// The other half of that promise: where no principal is declared, an
    /// absent basis changes nothing. Directory scoping resolves exactly as it
    /// did before principals existed, on every platform.
    #[test]
    fn directory_only_policy_is_unaffected_by_an_absent_basis() {
        let policy = policy_for(
            r#"{"policy_mode":"strict","scopes":[{"match":"ozone","constraints":[{"kind":"allowed_source_host","host":"allowed.example"}]}]}"#,
            Some("/work/ozone"),
            None,
        );
        assert_eq!(policy.scope(), Some("ozone"));
        assert_eq!(policy.mode(), PolicyMode::Strict);
        assert!(!policy.admit_host("https://allowed.example/").is_refusal());
        assert!(policy.admit_host("https://denied.example/").is_refusal());
        assert!(policy.describe()["fail_closed"].is_null());
    }

    fn load_error(json: &str) -> String {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join("policy.json"), json).unwrap();
        PolicyDocument::read(directory.path())
            .err()
            .expect("the policy should be rejected")
    }

    #[test]
    fn duplicate_os_user_bindings_are_rejected_at_load() {
        let error = load_error(
            r#"{"principals":[{"principal":"alice","os_user":1001},{"principal":"bob","os_user":1001}]}"#,
        );
        assert!(
            error.contains(r#"binds OS user 1001 to both "alice" and "bob""#),
            "the error names both halves of the collision: {error}"
        );
    }

    #[test]
    fn duplicate_principal_names_are_rejected_at_load() {
        let error = load_error(
            r#"{"principals":[{"principal":"alice","os_user":1001},{"principal":"alice","os_user":1002}]}"#,
        );
        assert!(
            error.contains(r#"declares principal "alice" twice, for OS users 1001 and 1002"#),
            "the error names both halves of the collision: {error}"
        );
    }

    /// A binding that can never admit anything is a deleted scope or a typo,
    /// and it is caught before the server starts rather than at the first
    /// crossing of a session that will refuse every one of them.
    #[test]
    fn a_required_scope_with_no_bound_directory_is_rejected_at_load() {
        let error = load_error(
            r#"{"principals":[{"principal":"alice","os_user":1001,"require_scope":true}],"scopes":[{"match":"shared"}]}"#,
        );
        assert!(
            error.contains("bind a scope or drop require_scope"),
            "the error says what to do about it: {error}"
        );
    }

    #[test]
    fn two_allowances_for_one_period_are_rejected_at_load() {
        let error = load_error(
            r#"{"principals":[{"principal":"alice","os_user":1001,"allowances":[
                {"period":"day","amount":{"currency":"USD","micros":20000},"timezone":"UTC"},
                {"period":"day","amount":{"currency":"USD","micros":50000},"timezone":"UTC"}]}]}"#,
        );
        assert!(
            error.contains("two day allowances of 0.020000 and 0.050000"),
            "the error names both halves of the collision: {error}"
        );
    }

    /// Each terms rule refuses at load with the entry named: no host, no
    /// reference, one host declared twice, and an identifier missing its
    /// scheme or value.
    #[test]
    fn malformed_terms_are_rejected_at_load_with_the_entry_named() {
        let no_host = load_error(r#"{"terms":[{"host":"","reference":"a-1"}]}"#);
        assert!(no_host.contains("terms entry 1 names no host"), "{no_host}");

        let no_reference = load_error(r#"{"terms":[{"host":"pub.example","reference":" "}]}"#);
        assert!(
            no_reference.contains("terms entry 1 for host \"pub.example\" names no reference"),
            "{no_reference}"
        );

        let twice = load_error(
            r#"{"terms":[{"host":"pub.example","reference":"a-1"},
                          {"host":"PUB.example.","reference":"a-2"}]}"#,
        );
        assert!(
            twice.contains("declares terms for host \"pub.example\" twice"),
            "the second spelling is the same host: {twice}"
        );

        let bare_identifier = load_error(
            r#"{"terms":[{"host":"pub.example","reference":"a-1",
                           "access_context":[{"scheme":"ror","value":""}]}]}"#,
        );
        assert!(
            bare_identifier.contains("access_context identifier without a scheme or a value"),
            "{bare_identifier}"
        );

        let in_scope = load_error(
            r#"{"scopes":[{"match":"client","terms":[{"host":"pub.example","reference":""}]}]}"#,
        );
        assert!(
            in_scope.contains("scope \"client\" terms entry 1"),
            "a scope's own list is checked and named: {in_scope}"
        );
    }

    /// A scope's `terms` replace the top-level list, as `constraints` do; a
    /// scope without its own inherits, and a host no list names has none.
    #[test]
    fn scope_terms_replace_the_top_level_list_and_absence_inherits() {
        const TERMS: &str = r#"{
            "terms":[{"host":"pub.example","reference":"top-1"}],
            "scopes":[
                {"match":"client","terms":[{"host":"other.example","reference":"client-7",
                                            "requires_reporting":true,
                                            "access_context":[{"scheme":"ror","value":"https://ror.org/013meh722"}]}]},
                {"match":"personal"}
            ]}"#;
        let client = policy_in(TERMS, Some("/work/client"));
        assert!(
            client.terms_for("pub.example").is_none(),
            "replaced, not merged"
        );
        let terms = client
            .terms_for("OTHER.example")
            .expect("normalised host match");
        assert_eq!(terms.reference, "client-7");
        assert!(terms.requires_reporting);
        assert_eq!(terms.access_context[0].scheme, "ror");

        let personal = policy_in(TERMS, Some("/work/personal"));
        assert_eq!(
            personal
                .terms_for("pub.example")
                .map(|t| t.reference.as_str()),
            Some("top-1"),
            "a scope without terms inherits the top-level list"
        );
        assert!(personal.terms_for("nobody.example").is_none());
        assert_eq!(
            personal.describe()["terms"][0]["reference"],
            "top-1",
            "the status surface shows the effective list"
        );
    }

    #[test]
    fn an_allowance_in_an_unknown_timezone_is_rejected_at_load() {
        let error = load_error(
            r#"{"principals":[{"principal":"alice","os_user":1001,"allowances":[
                {"period":"month","amount":{"currency":"USD","micros":1},"timezone":"Mars/Olympus"}]}]}"#,
        );
        assert!(
            error.contains(r#""alice""#) && error.contains("Mars/Olympus"),
            "the error names the principal and the zone: {error}"
        );
    }

    /// The resolved session carries its own principal's allowances and
    /// nobody else's; a policy declaring none resolves exactly as before.
    #[test]
    fn resolution_carries_the_bound_principals_allowances_only() {
        let policy = policy_for(
            r#"{"principals":[
                {"principal":"me","os_user":1001,"allowances":[
                    {"period":"day","amount":{"currency":"USD","micros":20000},"timezone":"UTC"}]},
                {"principal":"other","os_user":1002,"allowances":[
                    {"period":"day","amount":{"currency":"USD","micros":1},"timezone":"UTC"}]}]}"#,
            None,
            Some(1001),
        );
        assert_eq!(policy.allowances().len(), 1);
        assert_eq!(policy.allowances()[0].amount.micros, 20000);
        assert_eq!(
            policy.describe()["allowances"][0]["amount"]["micros"],
            20000
        );
        let none = policy_in(r#"{"policy_mode":"observe"}"#, None);
        assert!(none.allowances().is_empty());
    }

    fn policy(json: &str) -> SessionPolicy {
        policy_in(json, None)
    }

    #[test]
    fn no_policy_file_observes_and_refuses_nothing() {
        let directory = tempfile::tempdir().expect("tempdir");
        let policy = SessionPolicy::load(directory.path(), None).expect("absent is fine");
        assert_eq!(policy.mode(), PolicyMode::Observe);
        assert!(!policy.admit_host("https://anything.example/x").is_refusal());
    }

    #[test]
    fn a_strict_denylist_refuses_before_the_crossing() {
        let policy = policy(
            r#"{"policy_mode":"strict",
                "constraints":[{"kind":"denied_source_host","host":"tracker.example"}]}"#,
        );
        assert!(
            policy
                .admit_host("https://tracker.example/beacon")
                .is_refusal()
        );
        assert!(!policy.admit_host("https://www.gov.uk/x").is_refusal());
    }

    #[test]
    fn a_strict_allowlist_refuses_everything_it_does_not_name() {
        let policy = policy(
            r#"{"policy_mode":"strict",
                "constraints":[{"kind":"allowed_source_host","host":"www.gov.uk"}]}"#,
        );
        assert!(!policy.admit_host("https://www.gov.uk/x").is_refusal());
        assert!(policy.admit_host("https://example.com/x").is_refusal());
    }

    /// Observe records the same breach it declines to enforce. The mode changes
    /// what happens to the crossing, never whether it is on the record.
    #[test]
    fn observe_mode_records_the_breach_it_does_not_enforce() {
        let policy = policy(
            r#"{"policy_mode":"observe",
                "constraints":[{"kind":"denied_source_host","host":"tracker.example"}]}"#,
        );
        let ruling = policy.admit_host("https://tracker.example/beacon");
        assert!(!ruling.is_refusal());
        assert!(ruling.gap().is_some());
    }

    /// The privacy floor holds unless the operator lifts it deliberately.
    #[test]
    fn private_addresses_are_mediated_only_when_explicitly_allowed() {
        let closed = policy(r#"{"policy_mode":"observe"}"#);
        assert!(!closed.mediates_address("http://127.0.0.1:8080/docs"));
        assert!(closed.mediates_address("https://www.gov.uk/x"));

        let opened = policy(r#"{"policy_mode":"observe","allow_private_hosts":true}"#);
        assert!(opened.mediates_address("http://127.0.0.1:8080/docs"));
    }

    /// A named prefix lifts the floor for itself and nothing beside it, for
    /// recording and mediation alike; an unlisted private address is exactly
    /// as unrecordable as it was before the list existed.
    #[test]
    fn a_named_internal_prefix_is_recorded_and_mediated_and_nothing_else_is() {
        let policy = policy(
            r#"{"policy_mode":"observe",
                "record_internal_prefixes":["https://rag.corp.internal/"]}"#,
        );
        assert!(policy.records_address("https://rag.corp.internal/doc/7"));
        assert!(policy.mediates_address("https://rag.corp.internal/doc/7"));
        assert!(!policy.records_address("https://wiki.corp.internal/x"));
        assert!(!policy.mediates_address("https://wiki.corp.internal/x"));
        assert!(!policy.records_address("http://localhost:3000/x"));
        assert_eq!(
            policy.describe()["record_internal_prefixes"],
            serde_json::json!(["https://rag.corp.internal/"]),
            "the console must be able to say what is being recorded and why"
        );
    }

    /// An empty list is the default: the floor holds exactly as it did.
    #[test]
    fn an_empty_prefix_list_is_the_unchanged_floor() {
        let policy = policy(r#"{"policy_mode":"observe","record_internal_prefixes":[]}"#);
        assert!(!policy.records_address("http://192.168.1.4/x"));
        assert!(!policy.mediates_address("file:///corp/kb/handbook.md"));
        assert!(policy.records_address("https://www.gov.uk/x"));
    }

    /// A prefix that could never match is a broken consent record, not a
    /// silently inert one.
    #[test]
    fn a_prefix_that_is_not_an_absolute_url_is_a_load_error() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            directory.path().join("policy.json"),
            r#"{"record_internal_prefixes":["rag.corp.internal/"]}"#,
        )
        .unwrap();
        assert!(SessionPolicy::load(directory.path(), None).is_err());
    }

    /// A prefix without its trailing slash would also match hosts and paths
    /// it merely starts ("https://rag.internal" matches
    /// "https://rag.internal.evil.internal/x"), so the loader refuses it
    /// rather than recording more than the operator named.
    #[test]
    fn a_prefix_without_a_trailing_slash_is_a_load_error() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            directory.path().join("policy.json"),
            r#"{"record_internal_prefixes":["https://rag.corp.internal"]}"#,
        )
        .unwrap();
        let error = match SessionPolicy::load(directory.path(), None) {
            Err(error) => error,
            Ok(_) => panic!("a slash-less prefix must not load"),
        };
        assert!(error.contains("must end with \"/\""), "got: {error}");
    }

    /// Demonstrated against the real MCP binary: a misspelled `"contraints"`
    /// must not load as a strict policy enforcing nothing while
    /// `context_status` reports the mode without the constraints. Every
    /// field defaults, so unknown keys must refuse to load — top-level and
    /// inside a scope alike.
    #[test]
    fn a_misspelled_policy_field_is_a_load_error() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            directory.path().join("policy.json"),
            r#"{"policy_mode":"strict",
                "contraints":[{"kind":"allowed_source_host","host":"www.gov.uk"}]}"#,
        )
        .unwrap();
        let error = match SessionPolicy::load(directory.path(), None) {
            Err(error) => error,
            Ok(_) => panic!("a misspelled field must not load"),
        };
        assert!(error.contains("contraints"), "got: {error}");

        std::fs::write(
            directory.path().join("policy.json"),
            r#"{"scopes":[{"match":"client","policy_mod":"strict"}]}"#,
        )
        .unwrap();
        assert!(SessionPolicy::load(directory.path(), None).is_err());
    }

    /// A policy file nobody can read must not quietly become a permissive one.
    #[test]
    fn a_malformed_policy_is_an_error_rather_than_a_silent_default() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join("policy.json"), "{ not json").unwrap();
        assert!(SessionPolicy::load(directory.path(), None).is_err());
    }

    const SCOPED: &str = r#"{
        "policy_mode": "observe",
        "scopes": [
            {"match": "code/ozone", "policy_mode": "strict",
             "constraints": [{"kind": "allowed_source_host", "host": "docs.ozone.example"}]},
            {"match": "common-measure", "policy_mode": "observe"}
        ]}"#;

    #[test]
    fn a_scope_matching_the_cwd_overlays_the_top_level_policy() {
        let governed = policy_in(SCOPED, Some("/home/operator/code/ozone/feature"));
        assert_eq!(governed.scope(), Some("code/ozone"));
        assert_eq!(governed.mode(), PolicyMode::Strict);
        assert!(governed.admit_host("https://example.com/x").is_refusal());
        assert!(
            !governed
                .admit_host("https://docs.ozone.example/x")
                .is_refusal()
        );
    }

    #[test]
    fn no_matching_scope_falls_through_to_the_top_level_policy() {
        let ungoverned = policy_in(SCOPED, Some("/home/operator/exploration"));
        assert_eq!(ungoverned.scope(), None);
        assert_eq!(ungoverned.mode(), PolicyMode::Observe);
        assert!(!ungoverned.admit_host("https://example.com/x").is_refusal());

        let no_cwd = policy_in(SCOPED, None);
        assert_eq!(no_cwd.scope(), None);
        assert_eq!(no_cwd.mode(), PolicyMode::Observe);
    }

    #[test]
    fn telemetry_egress_requires_a_named_engagement_and_explicit_clearance() {
        let cleared = policy_in(
            r#"{"scopes":[{"match":"code/personal","engagement":"personal","allow_telemetry_egress":true}]}"#,
            Some("/home/operator/code/personal"),
        );
        assert_eq!(cleared.governing_engagement(), Some("personal"));
        assert!(cleared.allows_telemetry_egress());

        let client_default = policy_in(
            r#"{"scopes":[{"match":"code/client","engagement":"client-a"}]}"#,
            Some("/home/operator/code/client"),
        );
        assert_eq!(client_default.governing_engagement(), Some("client-a"));
        assert!(!client_default.allows_telemetry_egress());

        let unmatched = policy_in(
            r#"{"scopes":[{"match":"code/personal","engagement":"personal","allow_telemetry_egress":true}]}"#,
            Some("/home/operator/code/elsewhere"),
        );
        assert_eq!(unmatched.governing_engagement(), None);
        assert!(!unmatched.allows_telemetry_egress());
    }

    #[test]
    fn telemetry_clearance_without_an_engagement_is_a_load_error() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            directory.path().join("policy.json"),
            r#"{"scopes":[{"match":"code","allow_telemetry_egress":true}]}"#,
        )
        .unwrap();
        let error = match SessionPolicy::load(directory.path(), Some("/code")) {
            Ok(_) => panic!("unnamed egress cannot be audited"),
            Err(error) => error,
        };
        assert!(error.contains("without naming an engagement"), "{error}");
    }

    /// Ordered rules, first match wins — the same discipline as the sink's
    /// attribution file, so one mental model covers both.
    #[test]
    fn the_first_matching_scope_wins() {
        let policy = policy_in(
            r#"{"scopes": [
                {"match": "code", "policy_mode": "strict"},
                {"match": "code/ozone", "policy_mode": "observe"}
            ]}"#,
            Some("/home/operator/code/ozone"),
        );
        assert_eq!(policy.scope(), Some("code"));
        assert_eq!(policy.mode(), PolicyMode::Strict);
    }

    /// A scope states only what it changes; everything else is inherited.
    #[test]
    fn a_scope_without_overrides_inherits_the_top_level_values() {
        let policy = policy_in(
            r#"{"policy_mode": "strict",
                "constraints": [{"kind": "denied_source_host", "host": "tracker.example"}],
                "scopes": [{"match": "common-measure"}]}"#,
            Some("/home/operator/common-measure"),
        );
        assert_eq!(policy.scope(), Some("common-measure"));
        assert_eq!(policy.mode(), PolicyMode::Strict);
        assert!(
            policy
                .admit_host("https://tracker.example/beacon")
                .is_refusal()
        );
    }

    /// An empty match would silently govern every session on the machine —
    /// malformed, and an error like any other malformed policy.
    #[test]
    fn a_scope_with_an_empty_match_is_an_error() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            directory.path().join("policy.json"),
            r#"{"scopes": [{"match": "", "policy_mode": "strict"}]}"#,
        )
        .unwrap();
        assert!(SessionPolicy::load(directory.path(), Some("/anywhere")).is_err());
    }

    /// Two scopes with one matcher would be two dial rows with one name,
    /// both of whose controls edit the first while the second is editable
    /// from nowhere — a false surface, refused at load like the empty match.
    #[test]
    fn two_scopes_with_the_same_match_are_an_error_naming_the_matcher() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            directory.path().join("policy.json"),
            r#"{"scopes": [{"match": "code/ozone", "policy_mode": "strict"},
                           {"match": "code/ozone", "policy_mode": "observe"}]}"#,
        )
        .unwrap();
        let error = SessionPolicy::load(directory.path(), Some("/anywhere"))
            .err()
            .expect("a duplicate matcher must refuse to load");
        assert!(
            error.contains("\"code/ozone\""),
            "the refusal names the duplicated matcher: {error}"
        );
        assert!(
            error.contains("only the first"),
            "the refusal says what the duplicate would silently do: {error}"
        );
    }

    const DECLARED: &str = r#"{
        "policy_mode": "observe",
        "constraints": [{"kind": "denied_source_host", "host": "tracker.example"}],
        "record_internal_prefixes": ["https://rag.corp.internal/"],
        "scopes": [
            {"match": "code/ozone", "engagement": "ozone",
             "allow_telemetry_egress": true, "policy_mode": "prefer"},
            {"match": "code/spur"}
        ]}"#;

    /// A mode edit changes the mode and nothing else. Everything an operator
    /// declared around it — the engagement a scope governs, its egress
    /// clearance, the constraints, the recordable prefixes, the order of the
    /// scopes — has to survive a save, or the dial quietly widens or narrows
    /// what it never showed.
    #[test]
    fn changing_one_scope_mode_carries_the_whole_declaration_through() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("policy.json"), DECLARED).unwrap();
        let document = PolicyDocument::read(home.path()).unwrap();
        let edited = document
            .with_mode(Some("code/ozone"), PolicyMode::Strict)
            .expect("a declared scope");
        PolicyDocument::save(home.path(), &edited).expect("a valid policy saves");

        let reread = PolicyDocument::read(home.path()).expect("the saved policy loads");
        let governed = reread.resolve(Some("/home/op/code/ozone/api"));
        assert_eq!(governed.mode(), PolicyMode::Strict);
        assert_eq!(governed.governing_engagement(), Some("ozone"));
        assert!(governed.allows_telemetry_egress());
        assert!(
            governed
                .admit_host("https://tracker.example/beacon")
                .is_refusal(),
            "the constraint the operator declared survived the mode edit"
        );
        assert_eq!(
            reread.resolve(None).internal_prefixes(),
            ["https://rag.corp.internal/"]
        );
        let matchers: Vec<&str> = reread
            .scopes()
            .iter()
            .map(|scope| scope.matcher.as_str())
            .collect();
        assert_eq!(matchers, ["code/ozone", "code/spur"], "order is precedence");

        // And the untouched scope still inherits, rather than being pinned to
        // whatever the top level said at the moment of the save.
        assert!(reread.scopes()[1].policy_mode.is_none());
    }

    #[test]
    fn the_top_level_mode_is_editable_and_the_scopes_are_left_alone() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("policy.json"), DECLARED).unwrap();
        let document = PolicyDocument::read(home.path()).unwrap();
        let edited = document.with_mode(None, PolicyMode::Strict).unwrap();
        PolicyDocument::save(home.path(), &edited).unwrap();

        let reread = PolicyDocument::read(home.path()).unwrap();
        assert_eq!(reread.resolve(None).mode(), PolicyMode::Strict);
        assert_eq!(
            reread.resolve(Some("/home/op/code/ozone")).mode(),
            PolicyMode::Prefer,
            "a scope that declared its own mode keeps it"
        );
        assert_eq!(
            reread.resolve(Some("/home/op/code/spur")).mode(),
            PolicyMode::Strict,
            "a scope that inherited follows the new top level"
        );
    }

    /// The dial edits declarations; it does not create scopes. A matcher
    /// nothing declares would start governing directories the operator never
    /// named, from a control that showed no such scope.
    #[test]
    fn a_mode_for_an_undeclared_scope_is_refused() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("policy.json"), DECLARED).unwrap();
        let document = PolicyDocument::read(home.path()).unwrap();
        let refusal = document
            .with_mode(Some("code/invented"), PolicyMode::Strict)
            .expect_err("an undeclared scope is not a new one");
        assert!(refusal.contains("code/invented"));
    }

    /// The console must not be able to write a policy this loader would then
    /// refuse: an operator locked out of their own policy file cannot repair
    /// it from the surface that broke it.
    #[test]
    fn a_save_validates_with_the_rule_a_load_enforces() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("policy.json"), DECLARED).unwrap();
        let before = PolicyDocument::read(home.path()).unwrap().revision;

        let mut invalid = PolicyDocument::read(home.path()).unwrap().file;
        invalid.record_internal_prefixes = vec!["not a url".to_owned()];
        let refusal =
            PolicyDocument::save(home.path(), &invalid).expect_err("the loader's rule applies");
        assert!(refusal.contains("record_internal_prefixes"));

        assert_eq!(
            PolicyDocument::read(home.path()).unwrap().revision,
            before,
            "a refused save never touched the file"
        );
    }

    #[test]
    fn an_absent_policy_reads_as_undeclared_with_an_absent_revision() {
        let home = tempfile::tempdir().unwrap();
        let document = PolicyDocument::read(home.path()).unwrap();
        assert!(!document.declared());
        assert_eq!(document.revision(), declaration::ABSENT);

        std::fs::write(home.path().join("policy.json"), DECLARED).unwrap();
        let declared = PolicyDocument::read(home.path()).unwrap();
        assert!(declared.declared());
        assert_ne!(declared.revision(), declaration::ABSENT);

        // The token follows the bytes, which is what makes a stale save
        // detectable.
        let edited = declared.with_mode(None, PolicyMode::Strict).unwrap();
        PolicyDocument::save(home.path(), &edited).unwrap();
        assert_ne!(
            PolicyDocument::read(home.path()).unwrap().revision(),
            declared.revision()
        );
    }

    // -----------------------------------------------------------------------
    // Canonical effective policy and its identity
    // -----------------------------------------------------------------------

    /// The pre-image field set is the contract. A field added here without
    /// a schema move would change every digest silently.
    #[test]
    fn the_pre_image_carries_exactly_the_documented_fields() {
        let policy = policy_in(SCOPED, Some("/home/operator/code/ozone"));
        let pre_image = policy.canonical();
        let mut keys: Vec<&str> = pre_image
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "access_rules",
                "addons",
                "allow_private_hosts",
                "allow_telemetry_egress",
                "allowances",
                "constraints",
                "fail_closed",
                "governing_engagement",
                "mode",
                "principal",
                "record_internal_prefixes",
                "refuse_on_pii",
                "resolver",
                "schema",
                "scope",
                "terms",
            ]
        );
        assert_eq!(pre_image["schema"], IDENTITY_SCHEMA);
        assert_eq!(pre_image["resolver"], RESOLVER_VERSION);
        assert_eq!(pre_image["addons"], ADDONS_UNKNOWN);
        assert_eq!(pre_image["scope"], "code/ozone");
        assert_eq!(pre_image["principal"]["basis"], "os_user");
    }

    /// A reviewer recomputes the digest from the printed pre-image with
    /// nothing but a canonical serialiser and sha256.
    #[test]
    fn the_identity_is_the_sha256_of_the_canonical_pre_image() {
        use sha2::{Digest, Sha256};
        let policy = policy_in(SCOPED, Some("/home/operator/code/ozone"));
        let text = canonical_json(&policy.canonical());
        let recomputed = format!("sha256:{:x}", Sha256::digest(text.as_bytes()));
        assert_eq!(policy.identity().digest, recomputed);
        assert_eq!(policy.describe()["policy_identity"]["digest"], recomputed);
        // Keys are in byte order at every depth and nothing is padded.
        assert!(
            text.starts_with(
                r#"{"access_rules":[],"addons":"unknown","allow_private_hosts":false"#
            )
        );
        assert!(!text.contains(": "));
        assert_eq!(
            canonical_json(&json!({"b": {"z": 1, "a": [3, {"y": 2, "x": 1}]}, "a": null})),
            r#"{"a":null,"b":{"a":[3,{"x":1,"y":2}],"z":1}}"#
        );
    }

    /// The vectors of `docs/contracts/canonical-json.md` §Shared policy
    /// vectors. The hub digests a submitted policy with its own
    /// implementation of the same rule and the edge compares the two, so a
    /// digest that moved on one side alone would be a silent
    /// `digest_mismatch` on every managed edge. The second vector is the
    /// first with a defaulted field left out: a different document, which
    /// digests as one, and each side must agree on both.
    #[test]
    fn the_edge_and_hub_digest_one_policy_equally() {
        const WRITTEN_IN_FULL: &str = r#"{"policy_mode":"strict","constraints":[{"kind":"denied_source_host","host":"paywall.example"},{"kind":"maximum_acquisition_cost","amount":{"currency":"GBP","micros":2500000}}],"scopes":[{"match":"code/ozone","policy_mode":"observe","allow_telemetry_egress":false}],"allow_private_hosts":false,"record_internal_prefixes":["https://rag.example.internal/"]}"#;
        const DEFAULT_LEFT_OUT: &str = r#"{"policy_mode":"strict","constraints":[{"kind":"denied_source_host","host":"paywall.example"},{"kind":"maximum_acquisition_cost","amount":{"currency":"GBP","micros":2500000}}],"scopes":[{"match":"code/ozone","policy_mode":"observe","allow_telemetry_egress":false}],"record_internal_prefixes":["https://rag.example.internal/"]}"#;

        for (text, expected) in [
            (
                WRITTEN_IN_FULL,
                "sha256:2ad528b05af5e72e090362a5a608e1f8b72c42ba4ae64536f98695eaff2bd103",
            ),
            (
                DEFAULT_LEFT_OUT,
                "sha256:e4d73e9d4be89337df7fe24c7acadb424daf22afea6ff1889271db7d8e8e0141",
            ),
        ] {
            let submitted: Value = serde_json::from_str(text).expect("the vector is json");
            assert_eq!(canonical_digest(&submitted), expected);
        }

        // A firm's policy as an owner writes it: the digest an envelope
        // names it by, and the digest the fleet-status document reports
        // once the loader's form is on disk.
        const OWNER_FORM: &str = r#"{"policy_mode":"strict","constraints":[{"kind":"access_rule","host":"www.legislation.gov.uk","action":"allow"},{"kind":"access_rule","host":"www.gov.uk","action":"allow"},{"kind":"access_rule","host":"www.lexisnexis.co.uk","action":"require_licence","licence":"marlow-reid/lexisnexis-subscription-2026"},{"kind":"access_rule","host":"*.theguardian.com","action":"allow"},{"kind":"access_rule","host":"*","action":"refuse"}],"scopes":[{"match":"/matters/confidential-","engagement":"client-confidential","allow_telemetry_egress":false,"constraints":[{"kind":"access_rule","host":"*","action":"refuse"}]},{"match":"/matters/MR-2026-014","engagement":"MR-2026-014","allow_telemetry_egress":true}]}"#;
        let owner_form: Value = serde_json::from_str(OWNER_FORM).expect("the vector is json");
        assert_eq!(
            canonical_digest(&owner_form),
            "sha256:3e12b0ad164279bbda19308614de6f0ace54837b087d6b1390437f7140efa379"
        );
        let loaded: PolicyFile = serde_json::from_value(owner_form).expect("a policy");
        assert_eq!(
            canonical_digest(&serde_json::to_value(&loaded).expect("serialises")),
            "sha256:0e7be44df686c8d1e0ad04a1d8c915e48fe3829bbb14345e02539647b982aacd",
            "the loader's form adds only the two defaulted top-level fields"
        );

        // The written-in-full vector is the loader's own save form, so a
        // policy that arrives in it digests the same after the round trip
        // the edge performs before it compares.
        let parsed: PolicyFile =
            serde_json::from_str(WRITTEN_IN_FULL).expect("the vector is a policy");
        assert_eq!(
            canonical_digest(&serde_json::to_value(&parsed).expect("a policy serialises")),
            "sha256:2ad528b05af5e72e090362a5a608e1f8b72c42ba4ae64536f98695eaff2bd103",
            "the loader's serialisation must be the form the vector is written in"
        );
    }

    /// Editing a scope this session is not in is not drift for this
    /// session; editing the scope it is in is.
    #[test]
    fn an_unrelated_scope_edit_leaves_the_identity_unchanged_and_a_related_one_moves_it() {
        let before = policy_in(SCOPED, Some("/home/operator/code/ozone"));
        let unrelated_edit = SCOPED.replace(
            r#"{"match": "common-measure", "policy_mode": "observe"}"#,
            r#"{"match": "common-measure", "policy_mode": "strict", "constraints": [{"kind": "denied_source_host", "host": "x.example"}]}"#,
        );
        assert_ne!(unrelated_edit, SCOPED, "the edit applied");
        let after_unrelated = policy_in(&unrelated_edit, Some("/home/operator/code/ozone"));
        assert_eq!(before.identity(), after_unrelated.identity());

        let related_edit = SCOPED.replace(
            r#"{"match": "code/ozone", "policy_mode": "strict","#,
            r#"{"match": "code/ozone", "policy_mode": "prefer","#,
        );
        assert_ne!(related_edit, SCOPED, "the edit applied");
        let after_related = policy_in(&related_edit, Some("/home/operator/code/ozone"));
        assert_ne!(before.identity(), after_related.identity());

        // The other session, in the scope that was edited, sees the change.
        let other_before = policy_in(SCOPED, Some("/home/operator/common-measure"));
        let other_after = policy_in(&unrelated_edit, Some("/home/operator/common-measure"));
        assert_ne!(other_before.identity(), other_after.identity());
    }

    /// Reordering, respelling a host, or listing a constraint twice changes
    /// nothing admission does, so it changes nothing here either.
    #[test]
    fn constraint_order_host_case_and_duplicates_are_not_drift() {
        let one = policy(
            r#"{"policy_mode":"strict","constraints":[
                {"kind":"denied_source_host","host":"Tracker.Example."},
                {"kind":"allowed_source_host","host":"www.gov.uk"}]}"#,
        );
        let two = policy(
            r#"{"policy_mode":"strict","constraints":[
                {"kind":"allowed_source_host","host":"www.gov.uk"},
                {"kind":"denied_source_host","host":"tracker.example"},
                {"kind":"denied_source_host","host":"tracker.example"}]}"#,
        );
        assert_eq!(one.identity(), two.identity());
        assert_eq!(one.canonical()["constraints"].as_array().unwrap().len(), 2);

        let three = policy(
            r#"{"policy_mode":"strict","constraints":[
                {"kind":"allowed_source_host","host":"www.gov.uk"}]}"#,
        );
        assert_ne!(one.identity(), three.identity());
    }

    /// Access rules are read in order and the first match decides, so two
    /// policies that differ only in rule order enforce differently and
    /// must not hash equal; the set-semantics constraints beside them stay
    /// order-free.
    #[test]
    fn reordering_access_rules_moves_the_identity_and_reordering_sets_does_not() {
        let first = policy(
            r#"{"policy_mode":"strict","constraints":[
                {"kind":"access_rule","host":"docs.example.com","action":"allow"},
                {"kind":"access_rule","host":"*.example.com","action":"refuse"},
                {"kind":"denied_source_host","host":"tracker.example"},
                {"kind":"denied_source_host","host":"beacon.example"}]}"#,
        );
        let sets_reordered = policy(
            r#"{"policy_mode":"strict","constraints":[
                {"kind":"denied_source_host","host":"beacon.example"},
                {"kind":"access_rule","host":"docs.example.com","action":"allow"},
                {"kind":"denied_source_host","host":"tracker.example"},
                {"kind":"access_rule","host":"*.example.com","action":"refuse"}]}"#,
        );
        assert_eq!(first.identity(), sets_reordered.identity());
        let rules_reordered = policy(
            r#"{"policy_mode":"strict","constraints":[
                {"kind":"access_rule","host":"*.example.com","action":"refuse"},
                {"kind":"access_rule","host":"docs.example.com","action":"allow"},
                {"kind":"denied_source_host","host":"tracker.example"},
                {"kind":"denied_source_host","host":"beacon.example"}]}"#,
        );
        assert_ne!(first.identity(), rules_reordered.identity());
        let rules = &first.canonical()["access_rules"];
        assert_eq!(rules[0]["position"], 0);
        assert_eq!(rules[0]["host"], "docs.example.com");
        assert_eq!(rules[1]["position"], 1);
        assert_eq!(rules[1]["action"], "refuse");
        assert_eq!(
            first.canonical()["constraints"].as_array().unwrap().len(),
            2
        );
    }

    /// The principal and its basis are part of what was resolved: the same
    /// document resolved for two principals is two identities, and a
    /// session that failed closed is not the identity of one that did not.
    #[test]
    fn the_principal_and_a_fail_closed_resolution_are_part_of_the_identity() {
        const BOUND: &str = r#"{"principals":[{"principal":"alice","os_user":1001,"policy_mode":"strict"},{"principal":"bob","os_user":1002}]}"#;
        let alice = policy_for(BOUND, None, Some(1001));
        let bob = policy_for(BOUND, None, Some(1002));
        let nobody = policy_for(BOUND, None, Some(1003));
        assert_ne!(alice.identity(), bob.identity());
        assert_ne!(alice.identity(), nobody.identity());
        assert!(nobody.canonical()["fail_closed"].is_string());
        assert_eq!(alice.canonical()["principal"]["name"], "alice");
        assert_eq!(alice.canonical()["allowances"], json!([]));
    }

    /// The declaration's digest follows what was declared, not how it was
    /// typed, and an absent declaration has none.
    #[test]
    fn the_document_digest_ignores_formatting_and_is_absent_without_a_file() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(PolicyDocument::read(home.path()).unwrap().digest(), None);

        std::fs::write(home.path().join("policy.json"), DECLARED).unwrap();
        let typed = PolicyDocument::read(home.path()).unwrap();
        let reformatted = typed.with_mode(None, PolicyMode::Observe).unwrap();
        PolicyDocument::save(home.path(), &reformatted).unwrap();
        let saved = PolicyDocument::read(home.path()).unwrap();
        assert_ne!(saved.revision(), typed.revision(), "the bytes changed");
        assert_eq!(saved.digest(), typed.digest(), "the declaration did not");

        let edited = typed.with_mode(None, PolicyMode::Strict).unwrap();
        PolicyDocument::save(home.path(), &edited).unwrap();
        assert_ne!(
            PolicyDocument::read(home.path()).unwrap().digest(),
            typed.digest()
        );
    }

    /// The access rules are ordered and the first match decides, so an
    /// exception written before a wildcard refusal stands and the same
    /// exception written after it is never reached. Scoped to the scope that
    /// declares them, like every other constraint.
    #[test]
    fn access_rules_are_read_in_order_and_the_first_match_decides() {
        let policy = policy_in(
            r#"{"policy_mode":"strict","scopes":[{"match":"client",
                "constraints":[
                    {"kind":"access_rule","host":"docs.example.com","action":"allow"},
                    {"kind":"access_rule","host":"*.example.com","action":"refuse"},
                    {"kind":"access_rule","host":"tracker.example","action":"allow"}]}]}"#,
            Some("/work/client/api"),
        );
        assert!(
            !policy
                .admit_host("https://docs.example.com/guide")
                .is_refusal(),
            "the exception before the wildcard stands"
        );
        assert!(
            policy.admit_host("https://api.example.com/v1").is_refusal(),
            "the wildcard refuses the rest of the domain"
        );
        assert!(
            policy.admit_host("https://example.com/").is_refusal(),
            "a wildcard covers the apex"
        );
        assert!(
            !policy.admit_host("https://other.example/").is_refusal(),
            "a host no rule names is left to the rest of the policy"
        );
        let outside = policy_in(
            r#"{"policy_mode":"strict","scopes":[{"match":"client",
                "constraints":[{"kind":"access_rule","host":"*","action":"refuse"}]}]}"#,
            Some("/work/personal"),
        );
        assert!(
            !outside
                .admit_host("https://api.example.com/v1")
                .is_refusal(),
            "a scope's rules govern that scope alone"
        );
    }

    /// The pattern is normalised the way admission normalises a host, so a
    /// rule written with capitals or a trailing dot names the host the record
    /// carries.
    #[test]
    fn an_access_rule_pattern_is_compared_as_the_runtime_spells_hosts() {
        let policy = policy(
            r#"{"policy_mode":"strict","constraints":[
                {"kind":"access_rule","host":"*.Tracker.Example.","action":"refuse"}]}"#,
        );
        assert!(
            policy
                .admit_host("https://cdn.tracker.example/pixel")
                .is_refusal()
        );
        assert_eq!(
            policy.describe()["constraints"][0]["host"],
            "*.tracker.example",
            "the status surface shows the normalised pattern"
        );
    }

    /// A refusal names the rule by position and pattern, so an operator can
    /// find the line that produced it.
    #[test]
    fn an_access_rule_refusal_names_the_rule_that_produced_it() {
        let policy = policy(
            r#"{"policy_mode":"strict","constraints":[
                {"kind":"denied_source_host","host":"other.example"},
                {"kind":"access_rule","host":"*.example.com","action":"refuse"}]}"#,
        );
        let ruling = policy.admit_host("https://api.example.com/v1");
        let reason = ruling.reason().expect("a refusal has a reason");
        assert!(
            reason.contains("access rule 2 (*.example.com)"),
            "the rule is named by position and pattern: {reason}"
        );
    }

    /// Under observe the same rule is carried with the breach recorded: the
    /// mode changes what happens to the crossing, never whether the rule is
    /// applied.
    #[test]
    fn observe_mode_carries_an_access_rule_breach_and_records_it() {
        let policy = policy(
            r#"{"policy_mode":"observe","constraints":[
                {"kind":"access_rule","host":"*","action":"refuse"}]}"#,
        );
        let ruling = policy.admit_host("https://anything.example/");
        assert!(!ruling.is_refusal());
        assert!(ruling.gap().is_some());
    }

    /// The denied-host list is a set and is applied before any access rule,
    /// so an allow rule cannot reach past a denial.
    #[test]
    fn an_allow_rule_cannot_pass_a_denied_host() {
        let policy = policy(
            r#"{"policy_mode":"strict","constraints":[
                {"kind":"denied_source_host","host":"tracker.example"},
                {"kind":"access_rule","host":"*","action":"allow"}]}"#,
        );
        assert!(
            policy
                .admit_host("https://tracker.example/beacon")
                .is_refusal()
        );
        assert!(!policy.admit_host("https://www.gov.uk/x").is_refusal());
    }

    /// An allow rule passes host policy, including an allowed-host list that
    /// does not name the host, because the rule is the operator's later and
    /// more specific word about that host.
    #[test]
    fn an_allow_rule_passes_a_host_the_allowed_list_does_not_name() {
        let policy = policy(
            r#"{"policy_mode":"strict","constraints":[
                {"kind":"allowed_source_host","host":"www.gov.uk"},
                {"kind":"access_rule","host":"docs.rs","action":"allow"}]}"#,
        );
        assert!(!policy.admit_host("https://docs.rs/serde").is_refusal());
        assert!(!policy.admit_host("https://www.gov.uk/x").is_refusal());
        assert!(policy.admit_host("https://example.com/x").is_refusal());
    }

    /// A licence required for one host is judged on what the supplier
    /// declared for that source; before any bytes exist the licence is
    /// unknown, and unknown is never permitted.
    #[test]
    fn a_require_licence_rule_refuses_an_unknown_licence_and_admits_the_declared_one() {
        let policy = policy(
            r#"{"policy_mode":"strict","constraints":[
                {"kind":"access_rule","host":"publisher.example","action":"require_licence","licence":"rsl:publisher/2026"}]}"#,
        );
        let unknown = policy.admit_host("https://publisher.example/article");
        assert!(unknown.is_refusal());
        assert!(
            unknown
                .reason()
                .unwrap()
                .contains("requires licence \"rsl:publisher/2026\""),
            "{unknown:?}"
        );

        let mut declared = crate::mcp::envelope_for("https://publisher.example/article");
        declared.licence = commonmeasure_types::LicenceState::Declared {
            reference: "rsl:publisher/2026".to_owned(),
        };
        assert!(!policy.admit(&declared).is_refusal());

        declared.licence = commonmeasure_types::LicenceState::Declared {
            reference: "cc-by-4.0".to_owned(),
        };
        let other = policy.admit(&declared);
        assert!(other.is_refusal());
        assert!(
            other.reason().unwrap().contains("declared \"cc-by-4.0\""),
            "the refusal states what was declared instead: {other:?}"
        );
        assert!(
            !policy.admit_host("https://other.example/").is_refusal(),
            "the requirement is about one host"
        );
    }

    /// Admission is the governed crossing a `require_mediation` rule demands,
    /// so a mediated crossing meets it; the rule is carried on the status
    /// surface for the capture path that cannot.
    #[test]
    fn a_require_mediation_rule_is_met_at_admission_and_shown_in_status() {
        let policy = policy(
            r#"{"policy_mode":"strict","constraints":[
                {"kind":"allowed_source_host","host":"www.gov.uk"},
                {"kind":"access_rule","host":"publisher.example","action":"require_mediation"}]}"#,
        );
        assert!(
            !policy
                .admit_host("https://publisher.example/article")
                .is_refusal()
        );
        assert_eq!(
            policy.describe()["constraints"][1],
            serde_json::json!({"kind":"access_rule","host":"publisher.example","action":"require_mediation"})
        );
    }

    /// A pattern that names no host, and a licence requirement that names no
    /// licence, are refused by the loader before any session starts on them.
    #[test]
    fn a_malformed_access_rule_is_a_load_error_naming_the_rule() {
        let error =
            load_error(r#"{"constraints":[{"kind":"access_rule","host":"","action":"refuse"}]}"#);
        assert!(error.contains("cannot be empty"), "{error}");

        let error = load_error(
            r#"{"constraints":[{"kind":"access_rule","host":"a.*.example","action":"refuse"}]}"#,
        );
        assert!(
            error.contains("wildcard stands only at the front"),
            "{error}"
        );

        let error = load_error(
            r#"{"constraints":[{"kind":"access_rule","host":"example.com","action":"require_licence","licence":" "}]}"#,
        );
        assert!(
            error.contains("the top-level policy constraint 1")
                && error.contains("must name the licence"),
            "{error}"
        );

        let error = load_error(
            r#"{"scopes":[{"match":"client","constraints":[
                {"kind":"denied_source_host","host":"x.example"},
                {"kind":"access_rule","host":"example.com","action":"require_licence","licence":""}]}]}"#,
        );
        assert!(error.contains("scope \"client\" constraint 2"), "{error}");

        let error = load_error(
            r#"{"constraints":[{"kind":"access_rule","host":"example.com","action":"escalate"}]}"#,
        );
        assert!(
            error.contains("escalate"),
            "an unknown action names itself: {error}"
        );
    }

    /// A save puts the rules back through the loader, and a valid set of
    /// rules survives the round trip in order.
    #[test]
    fn access_rules_survive_a_save_in_order() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("policy.json"),
            r#"{"policy_mode":"strict","scopes":[{"match":"client","policy_mode":"strict","constraints":[
                {"kind":"access_rule","host":"docs.example.com","action":"allow"},
                {"kind":"access_rule","host":"*.example.com","action":"refuse"}]}]}"#,
        )
        .unwrap();
        let document = PolicyDocument::read(home.path()).unwrap();
        let edited = document.with_mode(None, PolicyMode::Observe).unwrap();
        PolicyDocument::save(home.path(), &edited).unwrap();
        let reread = PolicyDocument::read(home.path()).unwrap();
        let governed = reread.resolve(Some("/work/client"));
        assert!(
            !governed
                .admit_host("https://docs.example.com/")
                .is_refusal()
        );
        assert!(governed.admit_host("https://api.example.com/").is_refusal());
    }

    /// Terms govern over a source's published preference, so a policy that
    /// declares them enforces differently from one that does not and must not
    /// hash equal; a respelt host or a repeated entry changes nothing
    /// `terms_for` finds, so it changes nothing here.
    #[test]
    fn terms_move_the_identity_and_a_respelt_host_does_not() {
        let without = policy(r#"{"policy_mode":"strict"}"#);
        let with = policy(
            r#"{"policy_mode":"strict","terms":[
                {"host":"pub.example","reference":"agreement-7","requires_reporting":true}]}"#,
        );
        assert_ne!(without.identity(), with.identity());
        assert_eq!(
            with.canonical()["terms"],
            json!([{"host":"pub.example","reference":"agreement-7","requires_reporting":true}])
        );
        let respelt = policy(
            r#"{"policy_mode":"strict","terms":[
                {"host":"PUB.Example.","reference":"agreement-7","requires_reporting":true}]}"#,
        );
        assert_eq!(with.identity(), respelt.identity());
        let other_reference = policy(
            r#"{"policy_mode":"strict","terms":[
                {"host":"pub.example","reference":"agreement-8","requires_reporting":true}]}"#,
        );
        assert_ne!(with.identity(), other_reference.identity());
    }

    /// A candidate built from a document resolves as the saved file would,
    /// and building it writes nothing.
    #[test]
    fn a_candidate_declaration_resolves_like_the_saved_one_and_writes_nothing() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("policy.json"), DECLARED).unwrap();
        let document = PolicyDocument::read(home.path()).unwrap();
        let before = document.revision().to_owned();

        let candidate = document
            .with_access_rule(
                Some("code/spur"),
                commonmeasure_types::HostPattern::parse("*.example.com").unwrap(),
                commonmeasure_types::AccessAction::Refuse,
            )
            .unwrap();
        let draft = document.with_file(candidate);
        assert_eq!(
            draft.revision(),
            before,
            "the candidate names the bytes it was built from"
        );
        let governed = draft.resolve(Some("/home/op/code/spur"));
        assert!(
            governed
                .admit_host("https://api.example.com/")
                .gap()
                .is_some(),
            "the draft rule applies under the draft"
        );
        assert!(
            governed
                .admit_host("https://tracker.example/beacon")
                .gap()
                .is_some(),
            "the inherited constraints were kept when the scope stopped inheriting"
        );
        assert!(
            document
                .resolve(Some("/home/op/code/spur"))
                .admit_host("https://api.example.com/")
                .gap()
                .is_none(),
            "the document itself is unchanged"
        );
        assert_eq!(
            PolicyDocument::read(home.path()).unwrap().revision(),
            before,
            "nothing was written"
        );
        assert!(
            document
                .with_access_rule(
                    Some("code/invented"),
                    commonmeasure_types::HostPattern::parse("*").unwrap(),
                    commonmeasure_types::AccessAction::Allow,
                )
                .is_err(),
            "no scope is created"
        );
    }
}
