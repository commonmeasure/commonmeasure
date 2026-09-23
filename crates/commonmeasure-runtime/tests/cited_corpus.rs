//! Replay of the 4 August 2026 cited run, the live evidence behind the two
//! evaluator defects the fixture README describes, through the fixed evaluator.
//!
//! The fixture (`recon/cited-run-2026-08-04/`, with a README; the
//! public repository omits it) carries the four real model answers byte for
//! byte, the window parts as `ContextPart` fields in order, and the verdicts
//! grounding 0.3.0 published — wrong reasons and all. The `published_evaluation` documents
//! the defect; nothing here matches it except to assert it differs.
//!
//! The answers were produced against the OLD directive (placeholder
//! `SOURCE_URL`). They remain the corpus for the parser half of the seam:
//! their CITATION lines cite real window headers in the collision form — the
//! `SOURCE ` label, the copied hash, the template echo — which are exactly
//! the shapes the 0.4.0 parser must now read or refuse correctly, whatever
//! the current directive teaches.
//!
//! What each plan witnesses:
//! - `no-context`: the directive's template line copied verbatim. 0.3.0
//!   published it `uncovered` — a window fact about a placeholder. It is
//!   `unavailable`.
//! - `exa-only`: two whole window headers, hash included and no `::`. Never
//!   parseable as citations, and never `uncovered`.
//! - `firecrawl-only`: two real window URLs behind the `SOURCE ` label, with
//!   the template quote. 0.3.0 published both `uncovered` while its own
//!   `inputs` listed the URLs. They are `unavailable` template echoes, and a
//!   URL genuinely in the window is never `uncovered`.
//! - `tavily-only`: two real window URLs behind the label, with genuine
//!   quotes; citation 0 differs from its source only in the case of its
//!   first word. 0.3.0 published both `uncovered`. Both are `supported`,
//!   with spans into the retained text.

use commonmeasure_inference::ContextPart;
use commonmeasure_runtime::evaluate::{EvaluationInput, grounding};
use serde_json::Value;

fn fixture() -> Value {
    let path = std::path::Path::new(env!("COMMONMEASURE_EVIDENCE_DIR"))
        .join("recon/cited-run-2026-08-04/cited-run-2026-08-04.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("the committed corpus fixture"))
        .expect("the fixture is valid JSON")
}

fn plan<'a>(fixture: &'a Value, id: &str) -> &'a Value {
    fixture["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .find(|plan| plan["id"] == id)
        .unwrap_or_else(|| panic!("plan {id} is in the corpus"))
}

/// Replay one plan's recorded answer against its recorded window, exactly as
/// the run evaluated it. The window parts deserialise straight into
/// `ContextPart` — the fixture carries the production fields, not a test's
/// reconstruction of them.
fn replay(plan: &Value) -> (Value, Vec<ContextPart>) {
    let parts: Vec<ContextPart> = serde_json::from_value(plan["window"].clone())
        .expect("window parts carry ContextPart's fields in order");
    let record = grounding(&EvaluationInput {
        answer: Some(plan["answer"].as_str().expect("a recorded answer")),
        cited_answer_required: true,
        parts: &parts,
    });
    (record, parts)
}

/// The brief's headline correction, over every plan at once: no citation in
/// this corpus names a URL outside its window — the models cited what they
/// were sent, or echoed the template — so no replayed verdict may be
/// `uncovered`. 0.3.0 published five.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the cited-run corpus: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn no_replayed_citation_is_uncovered() {
    for plan in fixture()["plans"].as_array().expect("plans") {
        let (record, _) = replay(plan);
        assert_eq!(
            record["verdict_counts"]["uncovered"], 0,
            "plan {} still publishes uncovered:\n{}",
            plan["id"], record["citations"]
        );
        assert_eq!(
            record["unevaluated"],
            Value::Null,
            "every corpus plan has a checkable answer"
        );
    }
}

/// Plan `no-context`: the directive's template line, copied verbatim by a
/// model with an empty window. Nothing was cited, so no verdict about the
/// window belongs on the line.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the cited-run corpus: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn the_template_echo_is_unavailable_where_0_3_0_published_uncovered() {
    let fixture = fixture();
    let plan = plan(&fixture, "no-context");
    let (record, _) = replay(plan);

    let citations = record["citations"].as_array().expect("citations");
    assert_eq!(citations.len(), 1, "the recorded line is still read");
    assert_eq!(citations[0]["verdict"], "unavailable");
    assert!(
        citations[0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("placeholder")),
        "the reason names the template, not a window fact"
    );

    // The published record is the defect, preserved: 0.3.0 called the
    // evaluator's own teaching material a URL outside the window.
    assert_eq!(
        plan["published_evaluation"]["citations"][0]["verdict"], "uncovered",
        "the fixture documents the 0.3.0 verdict this replay corrects"
    );
}

/// Plan `exa-only`: two whole window headers, hash included, with no `::` at
/// all. They never parse as citations — `unavailable`, with the lines
/// carried verbatim — and the parser still reads both recorded lines.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the cited-run corpus: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn whole_header_lines_without_a_quote_are_unavailable_not_uncovered() {
    let fixture = fixture();
    let (record, _) = replay(plan(&fixture, "exa-only"));

    assert_eq!(record["verdict_counts"]["unavailable"], 2);
    assert_eq!(record["verdict_counts"]["uncovered"], 0);
    assert_eq!(record["citations"].as_array().map(Vec::len), Some(2));
}

/// Plan `firecrawl-only`: real window URLs behind the `SOURCE ` label, with
/// the template quote. The URLs are genuinely in the window — 0.3.0's
/// `uncovered` was false on its own record's evidence — and the template
/// quote means nothing was quoted, so the lines are `unavailable` echoes
/// rather than `contradicted` accusations.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the cited-run corpus: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn labelled_urls_with_the_template_quote_are_unavailable_where_0_3_0_published_uncovered() {
    let fixture = fixture();
    let plan = plan(&fixture, "firecrawl-only");
    let (record, _) = replay(plan);

    assert_eq!(record["verdict_counts"]["uncovered"], 0);
    assert_eq!(record["verdict_counts"]["contradicted"], 0);
    assert_eq!(record["verdict_counts"]["unavailable"], 2);

    for (index, published) in plan["published_evaluation"]["citations"]
        .as_array()
        .expect("published citations")
        .iter()
        .enumerate()
    {
        assert_eq!(
            published["verdict"], "uncovered",
            "the fixture documents the 0.3.0 verdict this replay corrects"
        );
        assert_ne!(
            record["citations"][index]["verdict"], published["verdict"],
            "citation {index} must no longer publish the 0.3.0 verdict"
        );
    }
}

/// Plan `tavily-only`: the run's only genuine quotes. Citation 0 lowercased
/// its first word to seat the quote mid-sentence — the W2 witness — and both
/// URLs wore the window's `SOURCE ` label — the W1 witness. 0.3.0 published
/// both `uncovered`; both are `supported`, and citation 0's recorded span
/// excises from the retained text to the quoted words.
#[test]
#[cfg_attr(
    not(evidence_recon),
    ignore = "needs the cited-run corpus: recon/ under COMMONMEASURE_PRIVATE_EVIDENCE"
)]
fn the_genuine_tavily_quotes_are_supported_with_spans_into_the_retained_text() {
    let fixture = fixture();
    let plan = plan(&fixture, "tavily-only");
    let (record, parts) = replay(plan);

    assert_eq!(record["verdict_counts"]["supported"], 2);
    assert_eq!(record["verdict_counts"]["uncovered"], 0);

    for (index, citation) in record["citations"]
        .as_array()
        .expect("citations")
        .iter()
        .enumerate()
    {
        assert_eq!(citation["verdict"], "supported", "citation {index}");
        // The record names the bare URL its `inputs` names — the labelling
        // the model copied is not part of the source's identity.
        let cited = citation["url"].as_str().expect("a URL");
        assert!(
            parts.iter().any(|part| part.source_ref == cited),
            "citation {index} must resolve to a window part: {cited}"
        );
        assert_eq!(
            plan["published_evaluation"]["citations"][index]["verdict"], "uncovered",
            "the fixture documents the 0.3.0 verdict this replay corrects"
        );
    }

    // Citation 0 differs from its source only in first-word case; its span
    // must excise from the retained text to the quote, case aside.
    let matched = &record["citations"][0]["matched"];
    let (start, end) = (
        matched["start"].as_u64().expect("a span start") as usize,
        matched["end"].as_u64().expect("a span end") as usize,
    );
    let part = parts
        .iter()
        .find(|part| part.content_hash == matched["content_hash"].as_str().expect("a hash"))
        .expect("the span names a window part by hash");
    let excised = &part.text[start..end];
    let quote = record["citations"][0]["quote"].as_str().expect("a quote");
    assert!(
        excised.eq_ignore_ascii_case(quote),
        "the excised span must be the quote, case aside:\n span: {excised}\nquote: {quote}"
    );
    assert_ne!(
        excised, quote,
        "the corpus witnesses a real case difference; if these are byte-equal the \
         fixture no longer exercises W2"
    );
}
