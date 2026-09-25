//! Local project selection and independently signed, edge-bound reporting approvals.
//! Directory selection never changes source policy or provider credentials.

use crate::{declaration, enrolment::EnrolmentRecord, managed::Deployment};
use chrono::{DateTime, Utc};
use commonmeasure_types::canonical::{canonical_digest, canonical_json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const FORMAT: &str = "commonmeasure-reporting-approvals/v1";
const FILE: &str = "reporting-approvals.json";

/// One explicitly selected root. Paths and OS identities remain on this edge.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub id: String,
    pub nonce: String,
    pub root: PathBuf,
    pub filesystem_id: String,
    pub name: String,
    pub os_user: u32,
    pub git_common: Option<PathBuf>,
    pub binding: String,
    pub reporting: bool,
}

/// Selected roots in a home that has adopted directory selection.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub projects: Vec<Project>,
}

/// Resolve a real directory; a filesystem root is never an agent project.
pub fn selected(path: &Path) -> Result<PathBuf, String> {
    let root = path
        .canonicalize()
        .map_err(|e| format!("cannot resolve directory {}: {e}", path.display()))?;
    if !root.is_dir() || root.parent().is_none() {
        return Err("select an existing project directory, not a filesystem root".into());
    }
    if root.to_str().is_none() {
        return Err("project directory must be UTF-8".into());
    }
    Ok(root)
}

/// Git's on-disk link identifies related worktrees without running repository code.
/// A malformed link is an error, so it cannot erase a restriction.
pub fn git_identity(path: &Path) -> Result<Option<(PathBuf, PathBuf)>, String> {
    for ancestor in path.ancestors() {
        let dot = ancestor.join(".git");
        if dot.is_dir() {
            return Ok(Some((
                dot.canonicalize().map_err(|e| e.to_string())?,
                ancestor.to_path_buf(),
            )));
        }
        if dot.is_file() {
            let text = std::fs::read_to_string(&dot).map_err(|e| e.to_string())?;
            let target = text
                .trim()
                .strip_prefix("gitdir: ")
                .ok_or("invalid .git link")?;
            let gitdir = ancestor
                .join(target)
                .canonicalize()
                .map_err(|e| e.to_string())?;
            let common = match std::fs::read_to_string(gitdir.join("commondir")) {
                Ok(text) => gitdir
                    .join(text.trim())
                    .canonicalize()
                    .map_err(|e| e.to_string())?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => gitdir,
                Err(e) => return Err(e.to_string()),
            };
            return Ok(Some((common, ancestor.to_path_buf())));
        }
    }
    Ok(None)
}

#[cfg(unix)]
fn filesystem_id(path: &Path) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn filesystem_id(_path: &Path) -> Result<String, String> {
    Err("this platform cannot bind a directory to an authenticated OS user".into())
}

fn private_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    crate::identity::write_private(&temp, bytes)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error.to_string());
    }
    #[cfg(unix)]
    std::fs::File::open(path.parent().ok_or("missing parent")?)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn binding(root: &Path, user: u32, git: &Option<PathBuf>, nonce: &str) -> String {
    canonical_digest(
        &json!({"schema": "commonmeasure-directory-binding/v1", "nonce": nonce, "filesystem_id": filesystem_id(root).ok(), "root": root, "os_user": user, "git_common": git}),
    )
}

impl Registry {
    pub fn read(home: &Path) -> Result<Option<Self>, String> {
        match std::fs::read(home.join("directories.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| format!("invalid directory registry: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Consent data disappearing must not fall back to scope
                // clearances. An empty selection withholds egress without
                // changing admission.
                let enabled = home
                    .join("directory-selection.json")
                    .try_exists()
                    .map_err(|e| e.to_string())?;
                Ok(enabled.then(Self::default))
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Update under an exclusive lock; repeated selection retains the project identity.
    pub fn enrol(home: &Path, root: &Path, name: &str, reporting: bool) -> Result<Project, String> {
        let root = selected(root)?;
        if name.trim().is_empty() || name.len() > 200 {
            return Err("project name must contain 1–200 bytes".into());
        }
        let user = crate::policy::trusted_os_user()
            .ok_or("this platform has no authenticated OS user for directory binding")?;
        let git = git_identity(&root)?.map(|(common, _)| common);
        std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
        let _lock = declaration::lock(&home.join("directories.lock"))
            .map_err(|e| format!("directory lock: {e:?}"))?;
        let mut registry = Self::read(home)?.unwrap_or_default();
        let index = registry
            .projects
            .iter()
            .position(|p| p.root == root && p.os_user == user);
        let nonce = index
            .map(|i| registry.projects[i].nonce.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut project = Project {
            id: uuid::Uuid::new_v4().to_string(),
            binding: binding(&root, user, &git, &nonce),
            nonce,
            filesystem_id: filesystem_id(&root)?,
            root,
            name: name.trim().into(),
            os_user: user,
            git_common: git,
            reporting,
        };
        if let Some(index) = index {
            if registry.projects[index].binding == project.binding {
                project.id.clone_from(&registry.projects[index].id);
            }
            registry.projects[index] = project.clone();
        } else {
            registry.projects.push(project.clone());
        }
        // Persist the mode before any directory can authorise reporting. This
        // marker survives registry loss and every supported opt-out operation.
        private_replace(
            &home.join("directory-selection.json"),
            b"{\"schema\":\"commonmeasure-directory-selection/v1\"}\n",
        )?;
        private_replace(
            &home.join("directories.json"),
            &serde_json::to_vec_pretty(&registry).map_err(|e| e.to_string())?,
        )?;
        Ok(project)
    }

    pub fn matching(&self, cwd: &str) -> Option<&Project> {
        let path = selected(Path::new(cwd)).ok()?;
        let user = crate::policy::trusted_os_user()?;
        let git = git_identity(&path).ok()?;
        self.projects
            .iter()
            .filter(|p| {
                let Ok(root) = selected(&p.root) else {
                    return false;
                };
                let Ok(root_git) = git_identity(&root) else {
                    return false;
                };
                let same_worktree = root_git == git;
                let root_git = root_git.map(|(common, _)| common);
                p.os_user == user
                    && root == p.root
                    && filesystem_id(&p.root).ok().as_deref() == Some(p.filesystem_id.as_str())
                    && path.starts_with(&p.root)
                    && p.git_common == root_git
                    && p.binding == binding(&p.root, user, &root_git, &p.nonce)
                    // A valid local-only ancestor vetoes nested repositories
                    // too. Positive inheritance must stay inside the same worktree.
                    && (!p.reporting || same_worktree)
            })
            .max_by_key(|p| (!p.reporting, p.root.components().count()))
    }
}

/// Verified reporting approvals use a separate revision space from source policy.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub registry: Option<Registry>,
    pub managed: bool,
    pub approvals: Vec<Value>,
}

impl Selection {
    pub fn read(home: &Path) -> Result<Self, String> {
        let registry = Registry::read(home)?;
        if registry.is_none() {
            return Ok(Self::default());
        }
        let managed = crate::managed::is_managed(home)?;
        let approvals = if managed && source_policy_applied(home) {
            // Approval failures remove reporting authority without replacing
            // the source policy with an unavailable or permissive admission mode.
            match snapshot(home) {
                Ok(Some(value)) if fresh(&value) => value["payload"]["approvals"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
                _ => vec![],
            }
        } else {
            vec![]
        };
        Ok(Self {
            registry,
            managed,
            approvals,
        })
    }

    pub fn permitted(&self, project: &Project) -> bool {
        project.reporting
            && (!self.managed
                || self
                    .approvals
                    .iter()
                    .any(|a| a["project_id"] == project.id && a["binding"] == project.binding))
    }
}

fn source_policy_applied(home: &Path) -> bool {
    let Ok(state) = crate::managed::State::read(home) else {
        return false;
    };
    if state.applied.is_none() {
        return false;
    }
    let expected = std::fs::read(crate::managed::State::last_known_good_path(home))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|v| {
            serde_json::from_value::<crate::policy::PolicyFile>(v["payload"]["policy"].clone()).ok()
        })
        .and_then(|p| serde_json::to_value(p).ok())
        .map(|p| canonical_digest(&p));
    let actual = crate::policy::PolicyDocument::read_file(&home.join("policy.json"))
        .ok()
        .and_then(|p| p.digest());
    expected.is_some() && expected == actual
}

fn snapshot(home: &Path) -> Result<Option<Value>, String> {
    match std::fs::read(home.join(FILE)) {
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            verify(home, &value, false)?;
            Ok(Some(value))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn fresh(value: &Value) -> bool {
    value["expires_at"]
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .is_some_and(|t| t > Utc::now())
}

fn hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return Err("invalid hex".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// Verify format, identity, stable payload digest, signature and bounded validity.
/// Old source-policy envelopes are never accepted here, or changed by this code.
pub fn verify(home: &Path, value: &Value, require_fresh: bool) -> Result<(), String> {
    let Deployment::Managed {
        signer,
        organisation,
        ..
    } = Deployment::read(home)?
    else {
        return Err("reporting approvals require managed enrolment".into());
    };
    let edge = EnrolmentRecord::load(home)?.ok_or("connect to a named hub first")?;
    let payload = &value["payload"];
    if signer.algorithm != "ed25519"
        || edge.revoked_at.is_some()
        || edge.organization.id != organisation
        || payload["schema"] != FORMAT
        || payload["organisation"] != organisation
        || payload["edge_key_id"] != edge.key_id
        || value["key_id"] != signer.key_id
        || payload["revision"].as_u64().is_none()
        || !payload["approvals"].is_array()
        || value["digest"] != canonical_digest(payload)
    {
        return Err("reporting approval identity, format or digest mismatch".into());
    }
    let issued = value["issued_at"]
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .ok_or("invalid reporting approval issue time")?;
    let expires = value["expires_at"]
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .ok_or("invalid reporting approval expiry")?;
    if issued > Utc::now() + chrono::Duration::minutes(5)
        || expires <= issued
        || expires - issued > chrono::Duration::days(1)
        || (require_fresh && !fresh(value))
    {
        return Err("reporting approval expired or invalid validity window".into());
    }
    let signature = hex(value["signature"]
        .as_str()
        .ok_or("missing reporting approval signature")?)?;
    let mut signed = value.clone();
    signed
        .as_object_mut()
        .ok_or("invalid reporting approval envelope")?
        .remove("signature");
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, hex(&signer.public_key)?)
        .verify(canonical_json(&signed).as_bytes(), &signature)
        .map_err(|_| "invalid reporting approval signature".to_owned())
}

/// Accept an independently numbered snapshot, refusing rollback and revision reuse.
pub fn accept(home: &Path, value: &Value) -> Result<(), String> {
    verify(home, value, true)?;
    let _lock = declaration::lock(&home.join("reporting-approvals.lock"))
        .map_err(|e| format!("reporting approval lock: {e:?}"))?;
    let path = home.join(FILE);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let old: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            let before = old["payload"]["revision"]
                .as_u64()
                .ok_or("invalid stored revision")?;
            let after = value["payload"]["revision"]
                .as_u64()
                .ok_or("invalid revision")?;
            if after < before || (after == before && old["digest"] != value["digest"]) {
                return Err("reporting approval rollback or revision reuse".into());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    private_replace(
        &path,
        &serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
}

/// Exchange enrolment metadata with the connected hub. Never return credentials or
/// include a canonical path in a request. Redirects are not followed.
fn hub_request(home: &Path, path: &str, body: Option<Value>) -> Result<Value, String> {
    let edge = EnrolmentRecord::load(home)?
        .ok_or("not connected: run commonmeasure connect <named-hub> --token <token> --managed")?;
    if edge.revoked_at.is_some() {
        return Err("edge enrolment is revoked".into());
    }
    let url = format!("{}{path}", edge.hub.trim_end_matches('/'));
    crate::managed::policy_url_accepted(&url)?;
    let key = edge.hub_ingest_key(home)?.ok_or(
        "connected ingest credential is missing: relay.json holds no key for the enrolled hub",
    )?;
    let mut request = match body {
        Some(body) => commonmeasure_http::Request::post(
            path,
            serde_json::to_vec(&body).map_err(|e| e.to_string())?,
            "application/json",
        ),
        None => commonmeasure_http::Request::get(path),
    };
    request.headers.set("X-API-Key", &key);
    let response = commonmeasure_http::send(&url, request)
        .map_err(|_| "directory enrolment hub is unreachable".to_owned())?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "directory enrolment hub returned HTTP {}; no reporting approval was changed",
            response.status
        ));
    }
    serde_json::from_slice(&response.body).map_err(|e| format!("invalid directory response: {e}"))
}

/// Submit or update this edge's request. Local opt-out is effective before this call.
pub fn request(home: &Path, project: &Project) -> Result<Value, String> {
    hub_request(
        home,
        "/api/v1/edge/directories",
        Some(
            json!({"project_id": project.id, "binding": project.binding, "name": project.name, "requested": project.reporting}),
        ),
    )
}

/// Refresh signed reporting approvals on demand and before relay. A failure
/// preserves the previous snapshot and its original expiry; 404 is never a
/// revocation.
pub fn sync(home: &Path) -> Result<(), String> {
    if Registry::read(home)?.is_none() || !crate::managed::is_managed(home)? {
        return Ok(());
    }
    let value = hub_request(home, "/api/v1/edge/reporting-approvals", None)?;
    accept(home, &value)
}

/// Shared CLI/MCP status reports local evidence independently from delivery.
pub fn status(home: &Path, root: &Path) -> Result<Value, String> {
    let root = selected(root)?;
    let cwd = root.to_str().ok_or("directory is not UTF-8")?;
    let document = crate::policy::PolicyDocument::read(home)?;
    let resolved = document.resolve(Some(cwd));
    let registry = Registry::read(home)?;
    let project = registry.as_ref().and_then(|r| r.matching(cwd));
    let enrolment = EnrolmentRecord::load(home)?;
    let management = crate::managed::management(home, Utc::now());
    let receiver = match std::fs::read(home.join("relay.json")) {
        Ok(bytes) => {
            serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string())?["receiver"].clone()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(e) => return Err(e.to_string()),
    };
    let mut witnessed = 0usize;
    for path in crate::SessionLog::list(home).map_err(|e| e.to_string())? {
        for record in crate::SessionLog::read(&path).map_err(|e| e.to_string())? {
            let payload = &record["payload"];
            if (record["event"] == "crossing_mediated" || record["event"] == "crossing_observed")
                && payload["cwd"]
                    .as_str()
                    .and_then(|p| std::fs::canonicalize(p).ok())
                    .is_some_and(|p| p.starts_with(&root))
            {
                witnessed += 1;
            }
        }
    }
    let approvals = match snapshot(home) {
        Ok(Some(value)) => {
            json!({"state": if fresh(&value) {"current"} else {"expired"}, "revision":value["payload"]["revision"], "digest":value["digest"], "expires_at":value["expires_at"]})
        }
        Ok(None) => json!({"state":"missing"}),
        Err(error) => json!({"state":"unavailable", "reason":error}),
    };
    let reporting = match project {
        None => "not_enrolled",
        Some(p) if !p.reporting => "local_only",
        Some(_) if receiver.is_null() => "receiver_missing",
        Some(_) if resolved.allows_telemetry_egress() => "permitted",
        Some(_) if resolved.describe()["fail_closed"].is_string() => "policy_refused",
        Some(_)
            if document.scopes().iter().any(|s| {
                Some(s.matcher.as_str()) == resolved.scope() && !s.allow_telemetry_egress
            }) =>
        {
            "policy_withheld"
        }
        Some(_) if management.mode == "managed" && !source_policy_applied(home) => {
            "managed_policy_unapplied"
        }
        Some(_) if approvals["state"] == "expired" => "approval_expired",
        Some(_) if approvals["state"] == "unavailable" => "approval_unavailable",
        Some(_) => "approval_pending",
    };
    Ok(json!({
        "directory": root, "coverage": "this canonical directory and descendants; related worktrees require separate enrolment",
        "project": project, "edge": enrolment,
        "deployment_mode": management.mode, "applied_revision": management.applied_revision,
        "policy_digest": document.digest(), "policy": resolved.describe(),
        "reporting": reporting, "receiver": receiver, "approvals": approvals,
        "historical_evidence": "reporting includes existing eligible witnessed evidence under this root; previously delivered evidence is not recalled",
        "first_evidence": {"state": if witnessed == 0 { "no_witnessed_crossing" } else { "witnessed_locally" }, "witnessed_crossings": witnessed, "delivery": "run commonmeasure relay --dry-run, then commonmeasure relay; delivery totals distinguish eligible and accepted events"},
        "connect": if enrolment.is_none() { Some("commonmeasure connect <named-hub> --token <token> --managed") } else { None },
    }))
}

/// Refresh source policy using this edge's existing key, then refresh reporting approvals.
pub fn sync_all(home: &Path) -> Result<(), String> {
    if crate::managed::is_managed(home)? {
        let key = crate::enrolment::enrolled_key_id(home)?
            .map(crate::fleet::EdgeIdentity::KeyId)
            .unwrap_or(crate::fleet::EdgeIdentity::Unknown {
                reason: "not enrolled".into(),
            });
        let result = crate::managed::sync(home, &key, Utc::now)?;
        if !result.converged() {
            return Err(format!(
                "source policy refresh did not converge: {}",
                result.to_record()
            ));
        }
    }
    sync(home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey};

    fn home() -> (tempfile::TempDir, SigningKey, Project) {
        let home = tempfile::tempdir().unwrap();
        let key = SigningKey::from_bytes(&[17; 32]);
        std::fs::write(home.path().join("deployment.json"),json!({"mode":"managed","signer":{"key_id":"test-signer","algorithm":"ed25519","public_key":crate::managed::encode_hex(&key.verifying_key().to_bytes())},"organisation":"test-org","policy_url":"http://127.0.0.1:9/api/v1/policy/desired"}).to_string()).unwrap();
        std::fs::write(home.path().join("enrolment.json"),json!({"hub":"http://127.0.0.1:9","organization":{"id":"test-org","name":"Test"},"name":"test-edge","key_id":"test-edge","identity":{"origin":"http://127.0.0.1:9","bot_page":"http://127.0.0.1:9/bot"},"enrolled_at":Utc::now()}).to_string()).unwrap();
        let root = home.path().join("project");
        std::fs::create_dir(&root).unwrap();
        let project = Registry::enrol(home.path(), &root, "Project", true).unwrap();
        std::fs::write(
            home.path().join("policy.json"),
            "{\"policy_mode\":\"strict\"}",
        )
        .unwrap();
        std::fs::create_dir(home.path().join("managed")).unwrap();
        std::fs::write(
            home.path().join("managed/last-known-good.json"),
            json!({"payload":{"policy":{"policy_mode":"strict"}}}).to_string(),
        )
        .unwrap();
        std::fs::write(home.path().join("managed/state.json"),json!({"applied":{"revision":1,"digest":"test","issued_at":Utc::now(),"expires_at":Utc::now()+chrono::Duration::days(7),"activated_at":Utc::now(),"signer_key_id":"test-signer"}}).to_string()).unwrap();
        (home, key, project)
    }
    fn signed(key: &SigningKey, revision: u64, approvals: Value, expired: bool) -> Value {
        let payload = json!({"schema":FORMAT,"organisation":"test-org","edge_key_id":"test-edge","revision":revision,"approvals":approvals});
        let issued = Utc::now() - chrono::Duration::hours(if expired { 25 } else { 1 });
        let mut value = json!({"payload":payload,"digest":canonical_digest(&payload),"key_id":"test-signer","issued_at":issued,"expires_at":issued+chrono::Duration::hours(24)});
        value["signature"] = json!(crate::managed::encode_hex(
            &key.sign(canonical_json(&value).as_bytes()).to_bytes()
        ));
        value
    }
    #[test]
    fn signed_approvals_bind_edge_org_project_and_preserve_monotonic_revocation() {
        let (home, key, project) = home();
        let approvals = json!([{"project_id":project.id,"binding":project.binding}]);
        let first = signed(&key, 1, approvals.clone(), false);
        accept(home.path(), &first).unwrap();
        assert!(Selection::read(home.path()).unwrap().permitted(&project));
        let mut wrong = first.clone();
        wrong["payload"]["organisation"] = json!("other");
        assert!(accept(home.path(), &wrong).is_err());
        wrong = first.clone();
        wrong["payload"]["edge_key_id"] = json!("other");
        assert!(accept(home.path(), &wrong).is_err());
        assert!(
            accept(home.path(), &signed(&key, 1, json!([]), false)).is_err(),
            "same revision cannot change meaning"
        );
        accept(home.path(), &signed(&key, 2, json!([]), false)).unwrap();
        assert!(!Selection::read(home.path()).unwrap().permitted(&project));
        assert!(
            accept(home.path(), &first).is_err(),
            "rollback cannot reinstate approval"
        );
        assert!(accept(home.path(), &signed(&key, 3, approvals.clone(), true)).is_err());
        let expired = signed(&key, 3, approvals, true);
        private_replace(
            &home.path().join(FILE),
            &serde_json::to_vec(&expired).unwrap(),
        )
        .unwrap();
        assert!(!Selection::read(home.path()).unwrap().permitted(&project));
        assert_eq!(
            crate::policy::SessionPolicy::load(home.path(), project.root.to_str())
                .unwrap()
                .describe()["mode"],
            "strict"
        );
        let before = std::fs::read(home.path().join(FILE)).unwrap();
        assert!(sync(home.path()).is_err());
        assert_eq!(
            before,
            std::fs::read(home.path().join(FILE)).unwrap(),
            "failed refresh cannot renew expiry"
        );
    }
    // EGR-07. `relay.json` re-pointed at another receiver holds that
    // receiver's key, and the approval refresh asks the enrolled hub. The
    // server records what it is sent; it is not a hub.
    #[test]
    fn an_approval_refresh_sends_the_hub_only_a_key_held_for_the_hub() {
        use std::sync::{Arc, Mutex};

        let (home, _, _) = home();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut hub = {
            let seen = seen.clone();
            commonmeasure_http::Server::bind("127.0.0.1:0")
                .unwrap()
                .spawn(move |request| {
                    seen.lock()
                        .unwrap()
                        .push(request.headers.get("x-api-key").map(str::to_owned));
                    commonmeasure_http::Response::json(503, "{}")
                })
                .unwrap()
        };
        let hub_url = hub.url().trim_end_matches('/').to_owned();
        let mut enrolment: Value =
            serde_json::from_slice(&std::fs::read(home.path().join("enrolment.json")).unwrap())
                .unwrap();
        enrolment["hub"] = json!(hub_url);
        std::fs::write(home.path().join("enrolment.json"), enrolment.to_string()).unwrap();

        std::fs::write(
            home.path().join("relay.json"),
            json!({"receiver": "http://127.0.0.1:9/api/v1/telemetry", "api_key": "other-key"})
                .to_string(),
        )
        .unwrap();
        let refusal = sync(home.path()).unwrap_err();
        assert!(
            refusal.contains("ingest credential is missing"),
            "{refusal}"
        );
        assert!(seen.lock().unwrap().is_empty(), "the hub was sent nothing");

        std::fs::write(
            home.path().join("relay.json"),
            json!({"receiver": format!("{hub_url}/api/v1/telemetry"), "api_key": "hub-key"})
                .to_string(),
        )
        .unwrap();
        let refusal = sync(home.path()).unwrap_err();
        assert!(refusal.contains("HTTP 503"), "{refusal}");
        hub.stop();
        assert_eq!(*seen.lock().unwrap(), vec![Some("hub-key".to_owned())]);
    }
    #[test]
    fn another_local_binding_or_recreated_folder_does_not_inherit_an_approval() {
        let (home, key, project) = home();
        accept(
            home.path(),
            &signed(
                &key,
                1,
                json!([{"project_id":project.id,"binding":"sha256:other"}]),
                false,
            ),
        )
        .unwrap();
        assert!(!Selection::read(home.path()).unwrap().permitted(&project));
        let registry = Registry::read(home.path()).unwrap().unwrap();
        std::fs::rename(&project.root, home.path().join("old-project")).unwrap();
        std::fs::create_dir(&project.root).unwrap();
        assert!(registry.matching(project.root.to_str().unwrap()).is_none());
    }
    // Pre-release formats have no reader: the renamed format is the only one
    // accepted, and a file left under the old name authorises nothing.
    #[test]
    fn only_the_reporting_approval_format_and_file_are_read() {
        let (home, key, project) = home();
        let approvals = json!([{"project_id":project.id,"binding":project.binding}]);
        let mut old = signed(&key, 1, json!([]), false);
        old["payload"] = json!({"schema":"commonmeasure-directory-grants/v1","organisation":"test-org","edge_key_id":"test-edge","revision":1,"grants":approvals.clone()});
        old["digest"] = json!(canonical_digest(&old["payload"]));
        old.as_object_mut().unwrap().remove("signature");
        old["signature"] = json!(crate::managed::encode_hex(
            &key.sign(canonical_json(&old).as_bytes()).to_bytes()
        ));
        assert!(accept(home.path(), &old).is_err());
        std::fs::write(
            home.path().join("directory-grants.json"),
            serde_json::to_vec(&signed(&key, 1, approvals, false)).unwrap(),
        )
        .unwrap();
        assert!(!Selection::read(home.path()).unwrap().permitted(&project));
        assert_eq!(
            status(home.path(), &project.root).unwrap()["approvals"]["state"],
            "missing"
        );
    }
}
