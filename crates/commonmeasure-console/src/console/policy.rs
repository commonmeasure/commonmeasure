//! The policy panel: what governs each engagement, read from the declared
//! artefact the runtime itself reads.
//!
//! Everything here is projection, and it is resolved by the enforcing
//! implementation rather than a parallel one: every recorded working directory
//! is put through
//! [`commonmeasure_harness::policy::PolicyDocument::resolve`], the same resolution a
//! session's own load performs, so the stance shown here can never drift from
//! the stance a crossing would meet. The console projects the declaration over
//! recorded history; admission in the runtime and egress at the relay are
//! where anything is enforced (`DECISIONS.md` §Session policy and egress).
//!
//! This panel is also the one place both engagement identities are in view.
//! Rows are keyed by the reported engagement the attribution rules resolve;
//! each stance carries the governing engagement its policy scope declared and
//! the telemetry clearance the relay enforces from it. Nothing can make the two
//! agree — the relay cannot read attribution rules, and attribution cannot
//! enforce anything — so where they differ the panel says so and prefers
//! neither (`DECISIONS.md` §Session policy and egress).
//!
//! The absent case is a stated absence naming the file, not an empty grid, and
//! a policy that did not parse is its error and no stance — a stance shown
//! under a file that did not load would look exactly like one read from a
//! healthy declaration. What the columns mean rides on the columns themselves
//! (`html::tip`), because a definition is not commentary but it is also not
//! something to read before the table.
//!
//! The panel's writes live in [`super::edit`]: the mode dial, a denied host
//! and the attribution rules, each saved back through the runtime's own
//! loader. The division is the one the whole surface is built on: this file
//! reads and projects, that one edits what was declared, and neither
//! enforces anything.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use commonmeasure_harness::policy::{PolicyDocument, SessionPolicy};
use serde_json::{Value, json};

use crate::attribution::Attribution;

/// The declared policy projected over the recorded working directories:
/// which stance governs each engagement's recorded work, right now.
///
/// Served verbatim at `/api/policy` and rendered by [`panel`], so the JSON
/// and the HTML cannot disagree. `cwd_facts` is [`crate::Store::cwd_facts`];
/// the reported engagement each directory belongs to is resolved by the same
/// attribution rules every other projection uses, and every stance carries the
/// governing engagement its scope declared beside it. `engagement_divergences`
/// names each scope where those two identities differ; the `engagement`
/// argument filters the table and never the divergences.
///
/// A policy file that does not load yields an `error` field and no stances:
/// a stance shown under a policy that did not parse would look exactly like
/// one read from a healthy declaration.
pub fn projection(
    home: &Path,
    cwd_facts: &Value,
    attribution: &Attribution,
    engagement: Option<&str>,
) -> Value {
    let source = home.join("policy.json");
    let source_text = source.display().to_string();
    if !source.exists() {
        return json!({"source": source_text, "declared": false});
    }
    // Read once and resolved per recorded directory, through the loader the
    // runtime uses. One read means every stance in one projection is resolved
    // against the same bytes, so a policy edited while the page is being built
    // cannot make two directories disagree about a file neither of them saw
    // whole; the next request re-reads, like every other answer this console
    // serves.
    let document = match PolicyDocument::read(home) {
        Ok(document) => document,
        Err(error) => {
            return json!({"source": source_text, "declared": true, "error": error});
        }
    };
    let top = document.resolve(None);

    // engagement → stance key → (stance, distinct cwds, sessions without cwd).
    // BTreeMaps keyed on names and serialised stances, so the projection is
    // deterministic without a second ordering rule.
    let mut rows: BTreeMap<String, BTreeMap<String, (Value, u64, u64)>> = BTreeMap::new();
    let mut governing_scopes: BTreeSet<String> = BTreeSet::new();
    // Which recorded work each declared scope actually governs, for the mode
    // dial: the reported engagements whose directories resolve to it, and how
    // many directories those are. Keyed by the scope's own matcher, and by the
    // empty string for the top-level policy, which governs everything no scope
    // claimed.
    let mut governed_work: BTreeMap<String, (BTreeSet<String>, u64)> = BTreeMap::new();
    let empty = Vec::new();
    for entry in cwd_facts
        .get("cwds")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
    {
        let Some(cwd) = entry.get("cwd").and_then(Value::as_str) else {
            continue;
        };
        let governed = document.resolve(Some(cwd));
        if let Some(scope) = governed.scope() {
            governing_scopes.insert(scope.to_owned());
        }
        let work = governed_work
            .entry(governed.scope().unwrap_or_default().to_owned())
            .or_default();
        work.0
            .insert(attribution.engagement_of(Some(cwd)).to_owned());
        work.1 += 1;
        let stance = stance_of(&governed);
        let slot = rows
            .entry(attribution.engagement_of(Some(cwd)).to_owned())
            .or_default()
            .entry(stance.to_string())
            .or_insert((stance, 0, 0));
        slot.1 += 1;
    }
    // A session that recorded no working directory is governed by the
    // top-level policy — no scope can match a directory that was never
    // reported — and attributed to no engagement, exactly as its aggregates
    // are.
    let sessions_without_cwd = cwd_facts
        .get("sessions_without_cwd")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if sessions_without_cwd > 0 {
        let stance = stance_of(&top);
        let slot = rows
            .entry(crate::attribution::UNATTRIBUTED.to_owned())
            .or_default()
            .entry(stance.to_string())
            .or_insert((stance, 0, 0));
        slot.2 += sessions_without_cwd;
    }

    // The two engagement identities, compared where they can be: the reported
    // one keying each row, the governing one the resolved scope declared. They
    // are expected to agree and nothing enforces that they do — the relay
    // cannot read attribution rules and the sink cannot enforce policy — so
    // this console, the only reader of both, states the disagreement rather
    // than preferring a name (`DECISIONS.md` §Session policy and egress). Computed over every row before
    // the engagement filter narrows the table, for the same reason unmatched
    // scopes are: a declaration nobody can see is the failure being fixed.
    let mut divergences: Vec<Value> = Vec::new();
    for (reported, stances) in &mut rows {
        for (stance, _, _) in stances.values_mut() {
            let governing = stance
                .get("governing_engagement")
                .and_then(Value::as_str)
                .filter(|governing| governing != reported)
                .map(str::to_owned);
            let Some(governing) = governing else {
                continue;
            };
            divergences.push(json!({
                "reported": reported,
                "governing": governing,
                "scope": stance.get("scope").cloned().unwrap_or(Value::Null),
                "allow_telemetry_egress": stance.get("allow_telemetry_egress")
                    == Some(&Value::Bool(true)),
            }));
            stance["engagement_diverges"] = json!(true);
        }
    }

    // Scopes declared but exercised by no recorded work. A matcher with a
    // typo would otherwise be visible nowhere: it governs nothing, refuses
    // nothing, and the operator believes it is standing.
    //
    // From the document already in hand, not a second read of the file: two
    // reads could straddle an edit and report stances from one version beside
    // unused scopes from another, and a file that became unreadable between
    // them reported no unused scopes rather than the error the panel is
    // otherwise careful to state.
    let unmatched_scopes: Vec<String> = document
        .scopes()
        .iter()
        .map(|scope| scope.matcher.clone())
        .filter(|matcher| !governing_scopes.contains(matcher))
        .collect();

    // What the mode dial edits: every declared scope in its declared order,
    // then the top-level policy, each with the mode it is running under and
    // whether that mode is its own or inherited. This is keyed by the scope
    // because the scope is the unit of the artefact — an engagement's work can
    // fall under two of them, and a scope can govern two engagements — so the
    // work each one governs is named beside it rather than implied.
    let top_mode = top.describe()["mode"].clone();
    let modes: Vec<Value> = document
        .scopes()
        .iter()
        .map(|scope| {
            let (engagements, directories) = governed_work
                .get(&scope.matcher)
                .map(|(names, count)| (names.iter().cloned().collect::<Vec<_>>(), *count))
                .unwrap_or_default();
            // The identity a session in this scope resolves to, for the
            // principal this console runs as: the scope's own matcher is a
            // directory it matches. Where an earlier scope also matches
            // that directory the earlier one governs, and this row says so
            // instead of showing the identity of a scope nothing reaches
            // through this matcher.
            let resolved = document.resolve(Some(&scope.matcher));
            let identity = match resolved.scope() {
                Some(governing) if governing == scope.matcher => {
                    json!(resolved.identity().digest)
                }
                Some(governing) => json!({"shadowed_by": governing}),
                None => Value::Null,
            };
            json!({
                "scope": scope.matcher,
                "mode": scope.policy_mode.map(|mode| json!(mode)).unwrap_or_else(|| top_mode.clone()),
                // False means the scope states no mode of its own and follows
                // the top-level policy; saving one here pins it.
                "declares_mode": scope.policy_mode.is_some(),
                // False means the scope's constraints are the top-level list,
                // inherited; the first constraint written into it ends that.
                "declares_constraints": scope.constraints.is_some(),
                "denied_hosts": denied_hosts(scope.constraints.as_deref().unwrap_or(&document.file().constraints)),
                "access_rules": access_rules(scope.constraints.as_deref().unwrap_or(&document.file().constraints)),
                "governing_engagement": scope.engagement,
                "reported_engagements": engagements,
                "distinct_cwds": directories,
                "policy_identity": identity,
            })
        })
        .chain(std::iter::once({
            let (engagements, directories) = governed_work
                .get("")
                .map(|(names, count)| (names.iter().cloned().collect::<Vec<_>>(), *count))
                .unwrap_or_default();
            json!({
                "scope": Value::Null,
                "mode": top_mode,
                "declares_mode": true,
                "declares_constraints": true,
                "denied_hosts": denied_hosts(&document.file().constraints),
                "access_rules": access_rules(&document.file().constraints),
                "governing_engagement": Value::Null,
                "reported_engagements": engagements,
                "distinct_cwds": directories,
                "sessions_without_cwd": sessions_without_cwd,
                "policy_identity": top.identity().digest,
            })
        }))
        .collect();

    let engagements: Vec<Value> = rows
        .into_iter()
        .filter(|(name, _)| engagement.is_none_or(|wanted| wanted == name))
        .map(|(name, stances)| {
            json!({
                "engagement": name,
                "stances": stances
                    .into_values()
                    .map(|(mut stance, distinct_cwds, without_cwd)| {
                        stance["distinct_cwds"] = json!(distinct_cwds);
                        if without_cwd > 0 {
                            stance["sessions_without_cwd"] = json!(without_cwd);
                        }
                        stance
                    })
                    .collect::<Vec<_>>(),
            })
        })
        .collect();

    json!({
        "source": source_text,
        "declared": true,
        // The token over the bytes every figure here was resolved from. The
        // mode dial states it back on a save, so an edit against a policy that
        // changed underneath is refused rather than taken.
        "revision": document.revision(),
        // What a session outside every scope — including one reporting no
        // working directory at all — runs under.
        "outside_scopes": stance_of(&top),
        "engagements": engagements,
        "modes": modes,
        // Each identity in `modes` is a resolution, and a resolution has a
        // principal: the one this console runs as. A session under another
        // principal resolves its own identity, which its crossings record.
        "identity_principal": {
            "name": top.principal(),
            "basis": top.authentication_basis().as_str(),
        },
        "unmatched_scopes": unmatched_scopes,
        "engagement_divergences": divergences,
    })
}

/// The denied-host set one constraint list declares, in declared order, for
/// the row that offers to add to it.
fn denied_hosts(constraints: &[commonmeasure_types::Constraint]) -> Vec<String> {
    constraints
        .iter()
        .filter_map(|constraint| match constraint {
            commonmeasure_types::Constraint::DeniedSourceHost { host } => Some(host.clone()),
            _ => None,
        })
        .collect()
}

/// The access rules one constraint list declares, in the order they are
/// read, each with its one-based position among the constraints so the row
/// matches the rule a refusal names.
fn access_rules(constraints: &[commonmeasure_types::Constraint]) -> Vec<Value> {
    constraints
        .iter()
        .enumerate()
        .filter_map(|(index, constraint)| match constraint {
            commonmeasure_types::Constraint::AccessRule { host, action } => Some(json!({
                "position": index + 1,
                "host": host.as_written(),
                "action": action.as_str(),
                "licence": match action {
                    commonmeasure_types::AccessAction::RequireLicence { licence } => Some(licence),
                    _ => None,
                },
            })),
            _ => None,
        })
        .collect()
}

/// The attribution rules as the editor renders them, with the revision a
/// save states back. An unreadable rule file still has a revision, so the
/// editor can replace it; its rules are then the error, not an empty list.
pub fn attribution_editor(home: &Path) -> Value {
    match crate::attribution::Attribution::read(home) {
        Ok(snapshot) => json!({
            "revision": snapshot.revision,
            "rules": match &snapshot.rules {
                Ok(rules) => rules
                    .pairs()
                    .into_iter()
                    .map(|(matcher, engagement)| json!({"match": matcher, "engagement": engagement}))
                    .collect::<Vec<_>>(),
                Err(_) => Vec::new(),
            },
            "error": snapshot.rules.as_ref().err().map(|error| format!("{error:#}")),
        }),
        Err(error) => json!({
            "revision": Value::Null,
            "rules": [],
            "error": format!("{error:#}"),
        }),
    }
}

/// One resolved policy as the projection carries it: the runtime's own
/// `describe()` minus the source path, which the projection states once.
fn stance_of(policy: &SessionPolicy) -> Value {
    let mut stance = policy.describe();
    if let Some(fields) = stance.as_object_mut() {
        fields.remove("source");
    }
    stance
}
