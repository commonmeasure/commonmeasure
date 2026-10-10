//! The provenance line: one line of fixed shape that opens every
//! `context_fetch` result and each `context_search` result, built from the
//! crossing record the call just wrote (`docs/contracts/host-integration.md`
//! §1).
//!
//! The line is the one in-session channel every host has: a host that shows
//! the model nothing but tool text still carries it, and the agent repeats
//! what the tool said when asked about its sources, so the fields it can
//! repeat are the fields the record holds. Each field is this edge's own
//! word or a value drawn from a fixed vocabulary; nothing in it is a string
//! the source chose except a host name, which is cut as agent-facing text
//! cuts one (EDG-120) and shown only where the agent already reads that host
//! (EDG-116, EDG-129). A field the record does not hold reads `unknown`,
//! never blank or zero (`docs/FAIL-POLICY.md` §7).

use commonmeasure_types::{LicenceState, PolicyMode};

use crate::declarations::{Category, Preference, StatementSource};
use crate::discovery::Declarations;
use crate::session::{Crossing, CrossingMode};
use crate::source_text;

/// How every provenance line starts, so a reader (the Claude Code router,
/// a test) can tell the line's text block from the payload's.
pub const PREFIX: &str = "Common Measure · ";

/// The separator between fields.
const SEPARATOR: &str = " · ";

/// How many hex digits of the content hash the line shows. Enough to match
/// the line to its crossing in the record by eye; the record keeps the
/// whole hash.
const HASH_DIGITS: usize = 8;

/// The line for `crossing`, recorded under `mode`. `asked_host` is the host
/// of the URL the agent asked `context_fetch` for, and `None` for a search
/// result.
///
/// The host is shown where the agent already reads it: a delivered crossing
/// names the URL it was read at, and the agent wrote the host it asked for.
/// A refused or failed crossing at any other host, which a redirect chose,
/// or a refused search result, which the agent knows by position alone,
/// reads `host withheld`.
pub fn line(crossing: &Crossing, mode: PolicyMode, asked_host: Option<&str>) -> String {
    let delivered = crossing.refusal.is_none()
        && crossing.failure.is_none()
        && (crossing.delivered.is_some() || crossing.delivered_file.is_some());
    let mut fields = vec![
        "Common Measure".to_owned(),
        format!("host {}", host(crossing, delivered, asked_host)),
        format!("terms {}", terms(crossing)),
    ];
    let ruling = format!(
        "ruling {} ({})",
        if crossing.refusal.is_some() {
            "refused"
        } else if delivered {
            "delivered"
        } else {
            "failed"
        },
        mode_word(mode)
    );
    match &crossing.supplier {
        // A search result names who served it in place of the ruling; a
        // refused one keeps the ruling too, since that is what happened.
        Some(supplier) => {
            fields.push(format!("supplier {supplier}"));
            if crossing.refusal.is_some() {
                fields.push(ruling);
            }
        }
        None => fields.push(ruling),
    }
    fields.push(format!("cost {}", cost(crossing.declarations.as_ref())));
    fields.push(format!("grade {}", grade(crossing.mode)));
    fields.push(format!("receipt {}", receipt(crossing, delivered)));
    fields.push(hash(crossing.content_hash.as_deref()));
    fields.join(SEPARATOR)
}

fn host(crossing: &Crossing, delivered: bool, asked_host: Option<&str>) -> String {
    let named = crossing.host_name.as_str();
    if named.is_empty() {
        return "unknown".to_owned();
    }
    if delivered || asked_host == Some(named) {
        source_text::quoted_host(named)
    } else {
        "withheld".to_owned()
    }
}

/// What the source or the operator declared about the terms of use, in this
/// edge's vocabulary: the governing RSL licence's AI-input grant, payment
/// type and reporting demand; else the operator's recorded agreement; else
/// the combined AI-input preference and the mechanism that stated it; else,
/// for a search result, whether its supplier declared a licence.
fn terms(crossing: &Crossing) -> String {
    let Some(declarations) = &crossing.declarations else {
        return match (&crossing.licence, &crossing.supplier) {
            (LicenceState::Declared { .. }, Some(_)) => "declared by supplier".to_owned(),
            (LicenceState::Declared { .. }, None) => "declared".to_owned(),
            (LicenceState::Unknown, _) => "unknown".to_owned(),
        };
    };
    if let Some((_, terms)) = declarations.licence_terms() {
        let mut parts = Vec::new();
        let ai_input = terms
            .statements
            .iter()
            .filter(|statement| statement.category == Category::AiInput)
            .map(|statement| statement.preference);
        let ai_input: Vec<Preference> = ai_input.collect();
        if ai_input.contains(&Preference::Disallow) {
            parts.push("no ai-input");
        } else if ai_input.contains(&Preference::Allow) {
            parts.push("ai-input");
        }
        if let Some(payment) = &terms.payment {
            parts.push(
                payment
                    .kind
                    .as_deref()
                    .and_then(source_text::defined_payment_kind)
                    .unwrap_or("payment"),
            );
        }
        if !terms.reporting.is_empty() {
            parts.push("report");
        }
        return if parts.is_empty() {
            "RSL".to_owned()
        } else {
            format!("RSL: {}", parts.join(", "))
        };
    }
    if let Some(agreement) = &declarations.terms {
        return if agreement.requires_reporting {
            "operator agreement, report".to_owned()
        } else {
            "operator agreement".to_owned()
        };
    }
    let preference = match declarations.ai_input() {
        crate::declarations::Effective::Allow => Preference::Allow,
        crate::declarations::Effective::Disallow => Preference::Disallow,
        crate::declarations::Effective::Unknown => return "unknown".to_owned(),
    };
    let said_by = declarations
        .statements
        .iter()
        .find(|statement| {
            statement.category == Category::AiInput && statement.preference == preference
        })
        .map_or("preference", |statement| mechanism(statement.source));
    match preference {
        Preference::Allow => format!("{said_by}: ai-input"),
        Preference::Disallow => format!("{said_by}: no ai-input"),
    }
}

/// The mechanism a statement was read from, by its own name.
fn mechanism(source: StatementSource) -> &'static str {
    match source {
        StatementSource::ContentUsageHeader | StatementSource::RobotsContentUsage => {
            "Content-Usage"
        }
        StatementSource::RobotsContentSignal => "Content-Signal",
        StatementSource::RslLicence | StatementSource::PastedRsl => "RSL",
        StatementSource::PastedContentUsage => "Content-Usage",
        StatementSource::PastedC2pa => "C2PA",
    }
}

/// The price a governing licence quoted for AI input, which no rail pays
/// here; `free` where it states the free payment type; else unknown. A
/// search result's share of a search's charge is not recorded, so it is
/// unknown too.
fn cost(declarations: Option<&Declarations>) -> String {
    match quoted_cost(declarations) {
        Cost::Quoted(price) => format!("{} quoted", money_text(&price)),
        Cost::Free => "free".to_owned(),
        Cost::Unknown => "unknown".to_owned(),
    }
}

/// A crossing's cost as the record holds it.
enum Cost {
    Unknown,
    Free,
    Quoted(commonmeasure_types::Money),
}

fn quoted_cost(declarations: Option<&Declarations>) -> Cost {
    let Some(declarations) = declarations else {
        return Cost::Unknown;
    };
    let licences = declarations.governing_licences();
    let payments = || {
        licences
            .iter()
            .filter_map(|(_, terms)| terms.payment.as_ref())
    };
    let quoted = payments()
        .filter(|payment| payment.needs_settlement())
        .find_map(|payment| payment.amount.as_ref());
    if let Some(amount) = quoted {
        let code =
            amount.currency.len() == 3 && amount.currency.bytes().all(|b| b.is_ascii_uppercase());
        let price = commonmeasure_types::Money::from_decimal_str(
            amount.currency.as_str(),
            amount.decimal.trim(),
        );
        return match price {
            Some(price) if code => Cost::Quoted(price),
            _ => Cost::Unknown,
        };
    }
    if payments().any(|payment| payment.needs_settlement()) {
        return Cost::Unknown;
    }
    if payments().any(|payment| payment.kind.as_deref() == Some("free")) {
        return Cost::Free;
    }
    Cost::Unknown
}

/// `0.015 USD`: the amount with trailing zeros cut, then the currency code.
fn money_text(price: &commonmeasure_types::Money) -> String {
    let decimal = price.as_decimal_string();
    let decimal = decimal.trim_end_matches('0').trim_end_matches('.');
    format!("{decimal} {}", price.currency())
}

fn grade(mode: CrossingMode) -> &'static str {
    match mode {
        CrossingMode::Observed => "observed",
        CrossingMode::Mediated => "mediated",
        CrossingMode::Reconstructed => "reconstructed",
    }
}

fn mode_word(mode: PolicyMode) -> &'static str {
    match mode {
        PolicyMode::Observe => "observe",
        PolicyMode::Prefer => "prefer",
        PolicyMode::Strict => "strict",
    }
}

/// Whether a usage report is owed to the source for this crossing: `owed`
/// where a reporting demand was met and the content delivered, `unmet` where
/// a demand was not met (the crossing was refused for it), `none` where the
/// declarations were read whole and carried no demand or nothing was
/// delivered, and `unknown` where a demand could not have been read.
fn receipt(crossing: &Crossing, delivered: bool) -> &'static str {
    let Some(declarations) = &crossing.declarations else {
        return "unknown";
    };
    if let Some(ruling) = &declarations.reporting {
        return match (ruling.met, delivered) {
            (false, _) => "unmet",
            (true, true) => "owed",
            (true, false) => "none",
        };
    }
    if !delivered {
        return "none";
    }
    let unread = declarations.robots.unavailable.is_some()
        || declarations
            .licences
            .iter()
            .any(|licence| licence.terms.is_none());
    if unread { "unknown" } else { "none" }
}

/// The content hash's prefix, where the record holds a SHA-256 in this
/// edge's form. A search result's hash is the supplier's value, so any
/// other form reads unknown rather than echoing it.
fn hash(content_hash: Option<&str>) -> String {
    let digest = content_hash.and_then(|hash| hash.strip_prefix("sha256:"));
    match digest {
        Some(hex)
            if hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) =>
        {
            format!("sha256:{}…", &hex[..HASH_DIGITS])
        }
        _ => "sha256 unknown".to_owned(),
    }
}

/// What a run of session records says about the sources it crossed, in
/// the line's vocabulary: one entry per crossing record, in record order,
/// with the totals the Claude Code mod's sources line shows
/// (`commonmeasure session --json`, `docs/contracts/host-integration.md` §6).
///
/// Each entry's fields follow the line's rules: a host is shown, cut and
/// quoted, where the crossing was delivered or the agent's own tool made it
/// (observed, reconstructed), and reads `withheld` for a refused or failed
/// one; a crossing the edge did not rule on (observed, reconstructed) reads
/// `ruling not ruled`. `url` is the record's own, for matching an answer's
/// citations; it is not agent-facing text.
///
/// The total `cost` covers carried crossings only, since a refused one
/// delivered nothing. It is `unknown` when any carried crossing's cost is
/// unknown, because a sum that left one out would understate it
/// (`docs/FAIL-POLICY.md` §7); else the quoted prices summed per currency,
/// or `free`; `none` when nothing was carried.
pub fn sources<'a>(records: impl IntoIterator<Item = &'a serde_json::Value>) -> serde_json::Value {
    let mut entries = Vec::new();
    let mut owed = 0u64;
    let mut carried = Vec::new();
    for record in records {
        let grade = match record["event"].as_str() {
            Some("crossing_observed") => "observed",
            Some("crossing_mediated") => "mediated",
            Some("crossing_refused") => "refused",
            Some("crossing_reconstructed") => "reconstructed",
            _ => continue,
        };
        let Ok(crossing) = serde_json::from_value::<Crossing>(record["payload"].clone()) else {
            entries.push(serde_json::json!({"grade": grade, "unreadable": true}));
            if grade != "refused" {
                carried.push(Cost::Unknown);
            }
            continue;
        };
        let ruled = crossing.mode == CrossingMode::Mediated;
        let delivered = crossing.refusal.is_none()
            && crossing.failure.is_none()
            && (!ruled || crossing.delivered.is_some() || crossing.delivered_file.is_some());
        let ruling = if crossing.refusal.is_some() {
            "refused"
        } else if !ruled {
            "not ruled"
        } else if delivered {
            "delivered"
        } else {
            "failed"
        };
        let receipt = receipt(&crossing, delivered);
        if receipt == "owed" {
            owed += 1;
        }
        if crossing.refusal.is_none() {
            carried.push(quoted_cost(crossing.declarations.as_ref()));
        }
        // A delivered crossing whose context entry waits on the host has no
        // grounding answer yet; the summariser keeps it apart the same way.
        let grounded = (record["payload"]["context_observation"] != "host_required")
            .then_some(crossing.grounded);
        entries.push(serde_json::json!({
            "grade": grade,
            "url": crossing.url,
            "host": host(&crossing, delivered, None),
            "terms": terms(&crossing),
            "ruling": ruling,
            "supplier": crossing.supplier,
            "cost": cost(crossing.declarations.as_ref()),
            "receipt": receipt,
            "grounded": grounded,
            "hash": hash(crossing.content_hash.as_deref()),
        }));
    }
    serde_json::json!({
        "crossings": entries,
        "cost": total_cost(&carried),
        "receipts_owed": owed,
    })
}

/// The cost of the carried crossings together: `none` for none, `unknown`
/// where any one is unknown or the quoted prices of one currency cannot be
/// summed, else the quoted prices summed per currency, else `free`.
fn total_cost(carried: &[Cost]) -> String {
    if carried.is_empty() {
        return "none".to_owned();
    }
    let mut quoted: Vec<commonmeasure_types::Money> = Vec::new();
    for cost in carried {
        match cost {
            Cost::Unknown => return "unknown".to_owned(),
            Cost::Free => {}
            Cost::Quoted(price) => {
                match quoted
                    .iter_mut()
                    .find(|total| total.currency() == price.currency())
                {
                    Some(total) => match total.try_add(price) {
                        Some(sum) => *total = sum,
                        None => return "unknown".to_owned(),
                    },
                    None => quoted.push(price.clone()),
                }
            }
        }
    }
    if quoted.is_empty() {
        return "free".to_owned();
    }
    let parts: Vec<String> = quoted.iter().map(money_text).collect();
    format!("{} quoted", parts.join(" + "))
}

/// The payload text of a tool answer's `result`: its first text block that
/// is not the provenance lines. Every tool's payload is JSON; the lines,
/// where the call recorded a crossing, are the block before it.
pub fn payload_text(result: &serde_json::Value) -> Option<&str> {
    result["content"]
        .as_array()?
        .iter()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .find(|text| !text.starts_with(PREFIX))
}

/// The provenance lines opening a tool answer's `result`, one per crossing
/// the call recorded, in record order. Empty where it recorded none.
pub fn lines(result: &serde_json::Value) -> Vec<&str> {
    result["content"][0]["text"]
        .as_str()
        .filter(|text| text.starts_with(PREFIX))
        .map(|text| text.lines().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The total is unknown where any carried crossing's cost is, sums
    /// quoted prices per currency, and reads free or none otherwise.
    #[test]
    fn the_cost_total_never_drops_an_unknown() {
        use commonmeasure_types::Money;
        let usd = |micros| Cost::Quoted(Money::new("USD", micros));
        assert_eq!(total_cost(&[]), "none");
        assert_eq!(total_cost(&[Cost::Free, Cost::Free]), "free");
        assert_eq!(
            total_cost(&[usd(15_000), Cost::Free, usd(5_000)]),
            "0.02 USD quoted"
        );
        assert_eq!(
            total_cost(&[usd(1_000_000), Cost::Quoted(Money::new("GBP", 500_000))]),
            "1 USD + 0.5 GBP quoted"
        );
        assert_eq!(total_cost(&[usd(15_000), Cost::Unknown]), "unknown");
        assert_eq!(total_cost(&[usd(u64::MAX), usd(1)]), "unknown");
    }

    /// Only a SHA-256 in this edge's form is shown, by prefix; a supplier's
    /// hash in any other form is not echoed.
    #[test]
    fn only_an_edge_form_hash_is_shown() {
        let hex = "3f9a5b6c".repeat(8);
        assert_eq!(hash(Some(&format!("sha256:{hex}"))), "sha256:3f9a5b6c…");
        for other in [
            None,
            Some("sha256:1"),
            Some("md5:abc"),
            Some(&*format!("sha256:{}", hex.to_uppercase())),
        ] {
            assert_eq!(hash(other), "sha256 unknown", "{other:?}");
        }
    }

    /// The payload is the first text block after the lines, and a result
    /// without lines has none.
    #[test]
    fn the_payload_block_follows_the_lines() {
        let result = serde_json::json!({"content": [
            {"type": "text", "text": "Common Measure · host a · …\nCommon Measure · host b · …"},
            {"type": "text", "text": "{\"ok\":true}"},
        ]});
        assert_eq!(payload_text(&result), Some("{\"ok\":true}"));
        assert_eq!(lines(&result).len(), 2);
        let bare = serde_json::json!({"content": [{"type": "text", "text": "{}"}]});
        assert_eq!(payload_text(&bare), Some("{}"));
        assert!(lines(&bare).is_empty());
    }
}
