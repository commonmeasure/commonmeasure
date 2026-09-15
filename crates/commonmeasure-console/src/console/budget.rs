//! The Budget screen at `/app/budget` renders `/api/budget`: recorded context
//! footprint by engagement, declared acquisition caps and principal allowances.
//!
//! Session logs supply the footprint and crossing counts, attributed at read
//! time. They carry no per-engagement acquisition charge. Mediated purchases
//! account for spend in the principal's allowance ledger; run charges carry
//! no engagement. The projection states why no engagement spend total exists.
//!
//! Caps come from the Policy screen's projection and are labelled declared.
//! Principal allowances come from the policy loader and the runtime's own
//! ledger, the same sources enforcement reads. A job cap bounds one purchase;
//! a periodic allowance bounds a principal's spending over its declared period.

use std::collections::BTreeMap;
use std::path::Path;

use commonmeasure_harness::policy::PolicyDocument;
use commonmeasure_runtime::allowance::{Account, Ledger};
use serde_json::{Value, json};

use super::html::text;

/// Every declared principal allowance beside its ledger standing, read for
/// the screen's allowance section. The declarations and the spend come from
/// the same places enforcement reads — the parsed policy document and the
/// runtime's ledger — and an unreadable half is stated, never rendered as an
/// empty allowance (`docs/FAIL-POLICY.md` §7).
pub fn allowance_state(home: &Path) -> Value {
    let document = match PolicyDocument::read(home) {
        Ok(document) => document,
        Err(error) => return json!({"available": false, "reason": error}),
    };
    let ledger = Ledger::in_home(home);
    let principals: Vec<Value> = document
        .principals()
        .iter()
        .filter(|binding| !binding.allowances.is_empty())
        .map(|binding| {
            let account = Account {
                principal: &binding.principal,
                declarations: &binding.allowances,
            };
            match ledger.state(&account, chrono::Utc::now()) {
                Ok(periods) => json!({
                    "principal": binding.principal,
                    "periods": periods,
                }),
                Err(error) => json!({
                    "principal": binding.principal,
                    "error": error,
                }),
            }
        })
        .collect();
    json!({
        "available": true,
        "declared": !principals.is_empty(),
        "principals": principals,
    })
}

/// The per-engagement budget projection: store facts joined to the declared
/// policy, served verbatim at `/api/budget` and rendered by [`super::app::budget_page`].
///
/// `policy` is [`super::policy::projection`]'s output — not a second read of
/// `policy.json`, so the cap shown here is the cap the policy panel resolved.
/// `budgets` is [`crate::Store::engagement_budgets`]; `allowances` is
/// [`allowance_state`]'s output, carried whole so the JSON reader sees what
/// the panel renders.
pub fn projection(
    policy: &Value,
    budgets: &Value,
    allowances: &Value,
    engagement: Option<&str>,
) -> Value {
    let empty = Vec::new();
    let rows: Vec<Value> = budgets
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|row| {
            let name = text(row.get("engagement").unwrap_or(&Value::Null)).unwrap_or("");
            engagement.is_none_or(|wanted| wanted == name)
        })
        .map(|row| {
            let name = text(row.get("engagement").unwrap_or(&Value::Null)).unwrap_or("");
            let mut entry = row.clone();
            entry["declared_cap"] = declared_cap(policy, name);
            entry
        })
        .collect();
    json!({
        "engagements": rows,
        // Stated as a field, not left for the renderer to invent: the JSON
        // reader and the page reader are told the same thing about why every
        // charge cell is empty.
        "acquisition_charge": {
            "recorded": false,
            "reason": "No per-engagement acquisition spend total is available. Crossings may record \
                       quoted prices and allowance settlements; spend is accounted per principal \
                       in the allowance ledger. Quotes are not settled charges.",
        },
        "allowances": allowances,
    })
}

/// The acquisition cap governing one engagement, read out of the policy
/// projection's resolved stances.
///
/// An engagement whose directories fall under two scopes has two stances, and
/// they may declare different caps. That disagreement is reported rather than
/// resolved: picking one would be this console choosing an enforcement the
/// runtime never performed.
fn declared_cap(policy: &Value, engagement: &str) -> Value {
    if let Some(error) = text(policy.get("error").unwrap_or(&Value::Null)) {
        return json!({"state": "unreadable", "detail": error});
    }
    if policy.get("declared") != Some(&Value::Bool(true)) {
        return json!({"state": "no_policy"});
    }
    let empty = Vec::new();
    let stances = policy
        .get("engagements")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .find(|entry| text(entry.get("engagement").unwrap_or(&Value::Null)) == Some(engagement))
        .and_then(|entry| entry.get("stances"))
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    // Keyed by the serialised amount so two scopes declaring the same cap read
    // as one, and deterministically ordered without a second sort rule.
    let mut caps: BTreeMap<String, Value> = BTreeMap::new();
    for stance in stances {
        for constraint in stance
            .get("constraints")
            .and_then(Value::as_array)
            .unwrap_or(&empty)
        {
            if text(constraint.get("kind").unwrap_or(&Value::Null))
                == Some("maximum_acquisition_cost")
            {
                let amount = constraint.get("amount").cloned().unwrap_or(Value::Null);
                caps.insert(amount.to_string(), amount);
            }
        }
    }
    match caps.len() {
        0 => json!({"state": "none_declared"}),
        1 => json!({"state": "declared", "amount": caps.into_values().next()}),
        _ => json!({
            "state": "conflicting",
            "amounts": caps.into_values().collect::<Vec<_>>(),
        }),
    }
}
