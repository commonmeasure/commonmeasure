//! The real adapters, over the real transport, against the bytes four
//! providers actually returned on 1 August 2026.
//!
//! Each test starts a loopback origin serving one recorded response from
//! `demo/recon/`, points the adapter's base URL at it, and asserts on both
//! directions: the request the adapter put on the wire, and the envelopes and
//! charge its parser derived from the reply. Nothing in the path is a stand-in
//! — the client, the framing, the parser and the charge rules are the ones a
//! live call uses. Only the origin is local, which is the substitution
//! `AGENTS.md` permits and replay builds on.
//!
//! The recorded page text is elided in the capture (a 120-character lead plus a
//! marker); its shape, type and true length survive, which is what a parser
//! test needs and is why no page body is committed.

use commonmeasure_http::Response;
use commonmeasure_supply::{
    ExaAdapter, FirecrawlAdapter, ParallelAdapter, SupplyAdapter, SupplyError, TavilyAdapter,
    TollbitAdapter,
};
use commonmeasure_types::{ChargeBasis, LicenceState, ProviderCapability};
mod common;

use common::{Origin, read_capture, verify_capture};

#[test]
fn exa_search_parses_its_recorded_response_and_reads_cost_from_the_total() {
    let origin = Origin::replaying("exa/exa-search.json");
    let adapter = ExaAdapter::new(&origin.url(), "test-key");

    let acquisition = adapter
        .search(
            "EU AI Act general purpose AI code of practice obligations",
            2,
            &[],
        )
        .expect("the recorded response should parse");

    let request = origin.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/search");
    assert_eq!(request.header("x-api-key"), Some("test-key"));
    assert_eq!(request.json_body()["numResults"], 2);

    assert_eq!(acquisition.provider, "exa");
    assert_eq!(acquisition.http_status, Some(200));
    assert_eq!(
        acquisition.provider_request_id.as_deref(),
        Some("414c17b47f628c776e84b6a16b11e717")
    );
    assert_eq!(acquisition.envelopes.len(), 2);

    let first = &acquisition.envelopes[0];
    assert_eq!(first.host, "ai-act-service-desk.ec.europa.eu");
    assert_eq!(first.retrieval_rank, 1);
    assert!(first.text.is_some(), "Exa returns page text with search");
    assert!(
        first
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:"),
        "admitted text must be hashed at ingestion"
    );
    // No Exa response carries a licence field. Search accessibility is not
    // permission, so the envelope must say unknown rather than infer one.
    assert_eq!(first.licence, LicenceState::Unknown);
    // Neither recorded search result carries a `publishedDate`, so neither
    // envelope may carry a date: the recorded truth is that Exa declared
    // none here, and an invented one would be exactly the fabricated
    // measurement the freshness evaluator exists to refuse.
    for envelope in &acquisition.envelopes {
        assert!(
            envelope.declared_date.is_none(),
            "the recorded Exa results declare no date, so none may be mapped"
        );
    }
    // Provider-specific fields stay namespaced rather than promoted.
    assert!(first.native_metadata.get("favicon").is_some());
    assert!(first.native_metadata.get("text").is_none());

    // costDollars.total, exactly, not the unreliable per-category breakdown.
    let money = acquisition.charge.money.expect("Exa reports currency");
    assert_eq!(money.currency(), "USD");
    assert_eq!(money.micros, 7_000);
    let native = acquisition.charge.native.expect("Exa reports a charge");
    assert_eq!(native.unit, "USD");
    assert_eq!(native.basis, ChargeBasis::Observed);
}

/// Exa documents `publishedDate` on results, in the same optional metadata
/// family as the `author` the recorded contents call shows arriving null —
/// but neither recorded search result carried one, so the mapping is
/// spec-verified only (`docs/knowledge-base/unverified-assumptions.md` E3)
/// and this test exercises it over the documented shape rather than recorded
/// bytes. A result without the field must stay undated.
#[test]
fn exa_maps_a_published_date_where_a_result_declares_one_and_none_where_it_does_not() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "results": [
                    {"url": "https://dated.example/a", "title": "t", "text": "body",
                     "publishedDate": "2026-07-01T09:00:00.000Z"},
                    {"url": "https://undated.example/b", "title": "t", "text": "body"}
                ]
            }))
            .unwrap(),
        )
    });

    let acquisition = ExaAdapter::new(&origin.url(), "test-key")
        .search("anything", 2, &[])
        .expect("documented shape");
    let dated = acquisition.envelopes[0]
        .declared_date
        .as_ref()
        .expect("the declared date travels on the envelope");
    assert_eq!(dated.date, "2026-07-01T09:00:00.000Z");
    assert!(
        dated.provenance.contains("supplier-declared") && dated.provenance.contains("Exa"),
        "the provenance must name who declared the date: {}",
        dated.provenance
    );
    // Promoted, so the date does not appear twice under two names.
    assert!(
        acquisition.envelopes[0]
            .native_metadata
            .get("publishedDate")
            .is_none()
    );
    assert!(
        acquisition.envelopes[1].declared_date.is_none(),
        "a result that declares nothing stays undated, never dated by its neighbour"
    );
}

/// A provider that returns an empty text field returned emptiness, not
/// nothing, and the envelope keeps the two apart
/// (`commonmeasure_types::ContextEnvelope::text`). Folding them loses the one
/// fact that says whether the source had no text or the adapter was never
/// given it, which is the difference between a page worth refetching and a
/// page that is genuinely blank.
#[test]
fn an_empty_text_field_is_not_an_absent_one() {
    let exa = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "results": [
                    {"url": "https://empty.example/a", "title": "t", "text": ""},
                    {"url": "https://absent.example/b", "title": "t"}
                ]
            }))
            .unwrap(),
        )
    });
    let acquisition = ExaAdapter::new(&exa.url(), "test-key")
        .search("anything", 2, &[])
        .expect("documented shape");
    assert_eq!(acquisition.envelopes[0].text.as_deref(), Some(""));
    assert_eq!(acquisition.envelopes[1].text, None);

    let tavily = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "results": [
                    {"url": "https://empty.example/a", "title": "t", "content": ""},
                    {"url": "https://absent.example/b", "title": "t"}
                ]
            }))
            .unwrap(),
        )
    });
    let acquisition = TavilyAdapter::new(&tavily.url(), "test-key")
        .search("anything", 2, &[])
        .expect("documented shape");
    assert_eq!(acquisition.envelopes[0].text.as_deref(), Some(""));
    assert_eq!(acquisition.envelopes[1].text, None);

    let firecrawl = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "data": {"web": [
                    {"url": "https://empty.example/a", "title": "t", "description": ""},
                    {"url": "https://absent.example/b", "title": "t"}
                ]}
            }))
            .unwrap(),
        )
    });
    let acquisition = FirecrawlAdapter::new(&firecrawl.url(), "test-key")
        .search("anything", 2, &[])
        .expect("documented shape");
    assert_eq!(acquisition.envelopes[0].text.as_deref(), Some(""));
    assert_eq!(acquisition.envelopes[1].text, None);
}

/// A scrape that read a page as empty admits the empty page; a scrape that
/// returned no page at all admits nothing. The two are different outcomes and
/// the adapter keeps them apart.
#[test]
fn an_empty_scrape_is_an_empty_page_and_a_missing_one_is_no_page() {
    let empty = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "data": {"markdown": "", "metadata": {"sourceURL": "https://empty.example/a"}}
            }))
            .unwrap(),
        )
    });
    let acquisition = FirecrawlAdapter::new(&empty.url(), "test-key")
        .fetch("https://empty.example/a")
        .expect("documented shape");
    assert_eq!(acquisition.envelopes.len(), 1);
    assert_eq!(acquisition.envelopes[0].text.as_deref(), Some(""));

    let missing = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "data": {"metadata": {"sourceURL": "https://absent.example/b"}}
            }))
            .unwrap(),
        )
    });
    let acquisition = FirecrawlAdapter::new(&missing.url(), "test-key")
        .fetch("https://absent.example/b")
        .expect("documented shape");
    assert!(acquisition.envelopes.is_empty());
}

/// A job's allowed source hosts scope the Exa search: when the operator names
/// hosts, the adapter must ask Exa for only those (`includeDomains`), so the
/// run does not pay to fetch sources admission would reject. An unscoped search
/// must send no such field, so an ordinary open-web run is byte-identical to
/// before.
#[test]
fn exa_scopes_the_search_to_the_jobs_allowed_hosts_and_omits_the_field_when_there_are_none() {
    let scoped = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    ExaAdapter::new(&scoped.url(), "test-key")
        .search("anything", 2, &["www.dailymail.co.uk", "reddit.com"])
        .expect("documented shape");
    assert_eq!(
        scoped.only_request().json_body()["includeDomains"],
        serde_json::json!(["www.dailymail.co.uk", "reddit.com"]),
        "named hosts must reach Exa as includeDomains"
    );

    let unscoped = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    ExaAdapter::new(&unscoped.url(), "test-key")
        .search("anything", 2, &[])
        .expect("documented shape");
    assert!(
        unscoped
            .only_request()
            .json_body()
            .get("includeDomains")
            .is_none(),
        "an unscoped search must not send includeDomains at all"
    );
}

/// The same contract for Tavily, whose field is `include_domains`.
#[test]
fn tavily_scopes_the_search_to_the_jobs_allowed_hosts_and_omits_the_field_when_there_are_none() {
    let scoped = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    TavilyAdapter::new(&scoped.url(), "test-key")
        .search("anything", 2, &["www.dailymail.co.uk"])
        .expect("documented shape");
    assert_eq!(
        scoped.only_request().json_body()["include_domains"],
        serde_json::json!(["www.dailymail.co.uk"]),
        "named hosts must reach Tavily as include_domains"
    );

    let unscoped = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({"results": []})).unwrap(),
        )
    });
    TavilyAdapter::new(&unscoped.url(), "test-key")
        .search("anything", 2, &[])
        .expect("documented shape");
    assert!(
        unscoped
            .only_request()
            .json_body()
            .get("include_domains")
            .is_none(),
        "an unscoped search must not send include_domains at all"
    );
}

/// Tavily's `fetch` capability is `/extract`: it names one URL and returns the
/// full page body as `raw_content`.
#[test]
fn tavily_fetch_extracts_a_named_url() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "results": [{
                    "url": "https://www.ft.com/content/x",
                    "title": "A headline",
                    "raw_content": "The full extracted page body.",
                    "images": []
                }],
                "failed_results": [],
                "request_id": "req_abc"
            }))
            .unwrap(),
        )
    });
    let acquisition = TavilyAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.ft.com/content/x")
        .expect("documented shape");
    let request = origin.only_request();
    assert_eq!(request.target, "/extract");
    assert_eq!(request.header("Authorization"), Some("Bearer test-key"));
    assert_eq!(
        request.json_body()["urls"],
        serde_json::json!(["https://www.ft.com/content/x"])
    );
    assert_eq!(acquisition.capability, ProviderCapability::Fetch);
    assert_eq!(acquisition.provider_request_id.as_deref(), Some("req_abc"));
    assert_eq!(acquisition.envelopes.len(), 1);
    assert_eq!(acquisition.envelopes[0].host, "www.ft.com");
    assert_eq!(
        acquisition.envelopes[0].text.as_deref(),
        Some("The full extracted page body.")
    );
    // The extract response carries no cost field and Tavily's published rate
    // is per five successful extractions with no single-URL rule, so a
    // successful extract records an unknown charge: neither money nor a
    // native figure, never a rounded credit and never zero.
    assert_eq!(acquisition.charge.money, None);
    assert_eq!(acquisition.charge.native, None);
}

/// A URL Tavily could not extract lands in `failed_results` with `results`
/// empty: an acquisition with no envelope, and the one documented price —
/// "you never get charged if a URL extraction fails" — quoted as zero credits.
#[test]
fn tavily_fetch_of_a_failed_url_yields_no_envelope_and_the_documented_zero_charge() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "results": [],
                "failed_results": [{
                    "url": "https://www.telegraph.co.uk/news/x",
                    "error": "Failed to extract content"
                }],
                "request_id": "req_def"
            }))
            .unwrap(),
        )
    });
    let acquisition = TavilyAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.telegraph.co.uk/news/x")
        .expect("a per-URL failure is a documented 200");
    assert!(acquisition.envelopes.is_empty());
    assert_eq!(acquisition.charge.money, None);
    let native = acquisition
        .charge
        .native
        .expect("a failed extraction has a documented price");
    assert_eq!(native.unit, "credits");
    assert_eq!(native.amount.as_u64(), Some(0));
    assert_eq!(native.basis, ChargeBasis::Quoted);
    assert!(
        native
            .note
            .as_deref()
            .is_some_and(|note| note.contains("failed extraction is never charged")),
        "the note names the documented rule"
    );
}

/// Exa's `fetch` capability is `/contents`: it names one URL and returns its
/// page text, served here from the recorded exchange over loopback. This is a
/// `fixture-tested` claim over the 1 August 2026 capture, not `replay-tested`:
/// `exa/exa-contents.json` is not in `demo/recon/replay-manifest.json` and no
/// run serves it. The result shape is a search result, so text, host and the
/// currency charge come out the same way.
#[test]
fn exa_fetch_replays_its_recorded_contents_response() {
    let origin = Origin::replaying("exa/exa-contents.json");
    let acquisition = ExaAdapter::new(&origin.url(), "test-key")
        .fetch("https://digital-strategy.ec.europa.eu/en/policies/contents-code-gpai")
        .expect("the recorded response should parse");

    let request = origin.only_request();
    assert_eq!(request.target, "/contents");
    assert_eq!(request.header("x-api-key"), Some("test-key"));
    assert!(request.json_body()["urls"].is_array());

    assert_eq!(acquisition.capability, ProviderCapability::Fetch);
    assert_eq!(acquisition.envelopes.len(), 1);
    let e = &acquisition.envelopes[0];
    assert!(e.text.is_some(), "contents returns page text");
    assert!(e.content_hash.as_deref().unwrap().starts_with("sha256:"));
    // costDollars.total is present on /contents (0.001 in the capture).
    let money = acquisition.charge.money.expect("Exa reports currency");
    assert_eq!(money.currency(), "USD");
    assert_eq!(money.micros, 1_000);
}

/// Firecrawl's `fetch` capability is `/v2/scrape`: it returns the full page
/// markdown and bills the credit at `data.metadata.creditsUsed`, served here
/// from the recorded exchange over loopback. This is a `fixture-tested` claim
/// over the 1 August 2026 capture, not `replay-tested`:
/// `firecrawl/firecrawl-scrape.json` is not in `demo/recon/replay-manifest.json`
/// and no run serves it.
#[test]
fn firecrawl_fetch_replays_its_recorded_scrape_response() {
    let origin = Origin::replaying("firecrawl/firecrawl-scrape.json");
    let acquisition = FirecrawlAdapter::new(&origin.url(), "test-key")
        .fetch("https://digital-strategy.ec.europa.eu/en/policies/contents-code-gpai")
        .expect("the recorded response should parse");

    let request = origin.only_request();
    assert_eq!(request.target, "/v2/scrape");
    assert_eq!(request.header("Authorization"), Some("Bearer test-key"));
    assert_eq!(
        request.json_body()["formats"],
        serde_json::json!(["markdown"])
    );

    assert_eq!(acquisition.capability, ProviderCapability::Fetch);
    assert_eq!(acquisition.envelopes.len(), 1);
    let e = &acquisition.envelopes[0];
    assert!(e.text.is_some(), "scrape returns page markdown");
    assert!(e.content_hash.as_deref().unwrap().starts_with("sha256:"));
    // Markdown is promoted out into the envelope text; the rest of the scrape
    // data (its `metadata` block) stays namespaced.
    assert!(e.native_metadata.get("markdown").is_none());
    assert!(e.native_metadata.pointer("/metadata/sourceURL").is_some());
    assert_eq!(e.host, "digital-strategy.ec.europa.eu");
    // Credits, never currency, read from data.metadata.creditsUsed.
    assert!(acquisition.charge.money.is_none());
    let native = acquisition
        .charge
        .native
        .expect("Firecrawl reports credits");
    assert_eq!(native.unit, "credits");
    assert_eq!(native.basis, ChargeBasis::Observed);
}

/// Parallel is `live-verified` for search and fetch: the sealed responses are
/// in the publisher-ingestion audit runs of 20 August 2026 held outside this
/// repository (`docs/knowledge-base/provider-verification.md`). No recon
/// capture is committed under `demo/recon/`, so there is nothing to serve from
/// a recording and the parser is exercised over Parallel's documented response
/// shape through the real transport and a loopback origin.
/// The request must carry the objective and query, and the reply's `excerpts`
/// array must become the envelope's text, `publish_date` its date, `search_id`
/// the request id, and the `sku_search` usage count its observed charge.
#[test]
fn parallel_search_parses_its_documented_shape() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "search_id": "search_8a911eb27c7a4afaa20d0d9dc98d07c0",
                "results": [
                    {
                        "url": "https://www.dailymail.co.uk/news/royals/article-1/x.html",
                        "title": "A royal headline",
                        "publish_date": "2026-08-15",
                        "excerpts": ["First excerpt of the page.", "Second excerpt."]
                    },
                    {
                        "url": "https://www.dailymail.co.uk/news/royals/article-2/y.html",
                        "title": "Another headline",
                        "publish_date": null,
                        "excerpts": []
                    }
                ],
                "warnings": null,
                "usage": [{"name": "sku_search", "count": 1}],
                "session_id": "session_8a911eb27c7a4afaa20d0d9dc98d07c0"
            }))
            .unwrap(),
        )
    });

    let acquisition = ParallelAdapter::new(&origin.url(), "test-key")
        .search("royal family news", 5, &[])
        .expect("documented shape");

    let request = origin.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v1/search");
    assert_eq!(request.header("x-api-key"), Some("test-key"));
    assert_eq!(request.json_body()["objective"], "royal family news");
    assert_eq!(
        request.json_body()["search_queries"][0],
        "royal family news"
    );

    assert_eq!(acquisition.provider, "parallel");
    assert_eq!(
        acquisition.provider_request_id.as_deref(),
        Some("search_8a911eb27c7a4afaa20d0d9dc98d07c0")
    );
    assert_eq!(acquisition.envelopes.len(), 2);

    let first = &acquisition.envelopes[0];
    assert_eq!(first.host, "www.dailymail.co.uk");
    assert_eq!(first.retrieval_rank, 1);
    // The excerpts array is joined into the one text field, and hashed.
    assert_eq!(
        first.text.as_deref(),
        Some("First excerpt of the page.\n\nSecond excerpt.")
    );
    assert!(
        first
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(first.licence, LicenceState::Unknown);
    let dated = first.declared_date.as_ref().expect("publish_date maps");
    assert_eq!(dated.date, "2026-08-15");
    assert!(
        dated.provenance.contains("supplier-declared") && dated.provenance.contains("Parallel")
    );
    // A result with an empty excerpts array carries no text and no date.
    assert!(acquisition.envelopes[1].text.is_none());
    assert!(acquisition.envelopes[1].declared_date.is_none());
    // Promoted fields do not reappear under their own names.
    assert!(first.native_metadata.get("excerpts").is_none());
    assert!(first.native_metadata.get("publish_date").is_none());

    // Usage is a billing SKU, not currency: the count is observed, no money is
    // derived.
    assert!(acquisition.charge.money.is_none());
    let native = acquisition
        .charge
        .native
        .expect("sku_search count is recorded");
    assert_eq!(native.unit, "sku_search");
    assert_eq!(native.amount.as_u64(), Some(1));
    assert_eq!(native.basis, ChargeBasis::Observed);
}

/// A job's allowed hosts scope the Parallel search through
/// `advanced_settings.source_policy.include_domains`; with none named the
/// settings object is absent entirely. The offered candidates are also capped
/// to the job's result limit, since the provider-side count field is not wired.
#[test]
fn parallel_scopes_via_source_policy_and_caps_offered_results_to_the_limit() {
    let many = serde_json::json!({
        "search_id": "s",
        "results": (0..5).map(|i| serde_json::json!({
            "url": format!("https://www.dailymail.co.uk/a/{i}.html"),
            "title": "t",
            "excerpts": ["body"]
        })).collect::<Vec<_>>(),
        "usage": [{"name": "sku_search", "count": 1}]
    });

    let scoped = Origin::serving({
        let many = many.clone();
        move |_| Response::new(200, serde_json::to_vec(&many).unwrap())
    });
    let acquisition = ParallelAdapter::new(&scoped.url(), "test-key")
        .search("anything", 2, &["www.dailymail.co.uk"])
        .expect("documented shape");
    assert_eq!(
        scoped.only_request().json_body()["advanced_settings"]["source_policy"]["include_domains"],
        serde_json::json!(["www.dailymail.co.uk"]),
        "named hosts must reach Parallel as source_policy.include_domains"
    );
    // Five returned, two requested: the adapter offers only two, and the sealed
    // response still holds all five.
    assert_eq!(acquisition.envelopes.len(), 2);
    assert!(String::from_utf8_lossy(&acquisition.raw_response).contains("/a/4.html"));

    let unscoped = Origin::serving(move |_| Response::new(200, serde_json::to_vec(&many).unwrap()));
    ParallelAdapter::new(&unscoped.url(), "test-key")
        .search("anything", 2, &[])
        .expect("documented shape");
    assert!(
        unscoped
            .only_request()
            .json_body()
            .get("advanced_settings")
            .is_none(),
        "an unscoped search must send no advanced_settings at all"
    );
}

/// Parallel's Extract API is the `fetch` capability: it names one URL and
/// returns the article body. A successful extract carries `full_content`, which
/// becomes the envelope's text; the request names the URL and asks for the full
/// content; the charge is the `sku_extract_excerpts` count.
#[test]
fn parallel_fetch_extracts_a_named_url_into_one_envelope() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "extract_id": "extract_abc",
                "results": [{
                    "url": "https://www.ft.com/content/x",
                    "title": "A headline",
                    "publish_date": "2026-08-15",
                    "excerpts": ["lead excerpt"],
                    "full_content": "The full article body of the piece."
                }],
                "errors": [],
                "usage": [{"name": "sku_extract_excerpts", "count": 1}],
                "session_id": "session_abc"
            }))
            .unwrap(),
        )
    });

    let acquisition = ParallelAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.ft.com/content/x")
        .expect("documented shape");

    let request = origin.only_request();
    assert_eq!(request.target, "/v1/extract");
    assert_eq!(request.header("x-api-key"), Some("test-key"));
    assert_eq!(
        request.json_body()["urls"],
        serde_json::json!(["https://www.ft.com/content/x"])
    );
    assert_eq!(
        request.json_body()["advanced_settings"]["full_content"],
        true
    );

    assert_eq!(acquisition.capability, ProviderCapability::Fetch);
    assert_eq!(
        acquisition.provider_request_id.as_deref(),
        Some("extract_abc")
    );
    assert_eq!(acquisition.envelopes.len(), 1);
    let e = &acquisition.envelopes[0];
    assert_eq!(e.host, "www.ft.com");
    // Full content is the text, not the lead excerpt.
    assert_eq!(
        e.text.as_deref(),
        Some("The full article body of the piece.")
    );
    assert!(e.content_hash.as_deref().unwrap().starts_with("sha256:"));
    assert_eq!(e.declared_date.as_ref().unwrap().date, "2026-08-15");
    assert_eq!(e.licence, LicenceState::Unknown);
    let native = acquisition.charge.native.expect("extract SKU is charged");
    assert_eq!(native.unit, "sku_extract_excerpts");
    assert_eq!(native.amount.as_u64(), Some(1));
    assert_eq!(native.basis, ChargeBasis::Observed);
}

/// An extract often returns `full_content` as an empty string while populating
/// `excerpts`. An empty body is no body: the adapter must fall back to the
/// excerpts rather than admit a textless source beside content the provider did
/// return.
#[test]
fn parallel_fetch_falls_back_to_excerpts_when_full_content_is_empty() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "extract_id": "e",
                "results": [{
                    "url": "https://www.theguardian.com/world/x",
                    "title": "t",
                    "excerpts": ["The excerpt body that survived."],
                    "full_content": ""
                }],
                "errors": [],
                "usage": [{"name": "sku_extract_excerpts", "count": 1}]
            }))
            .unwrap(),
        )
    });
    let acquisition = ParallelAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.theguardian.com/world/x")
        .expect("documented shape");
    assert_eq!(acquisition.envelopes.len(), 1);
    assert_eq!(
        acquisition.envelopes[0].text.as_deref(),
        Some("The excerpt body that survived.")
    );
}

/// The finding this whole exercise exists to seal: a fetch the origin refused.
/// Parallel answers `200` with the per-URL `403` inside `errors` and `results`
/// empty. That must yield no admissible envelope and no fabricated body, with
/// the provider's own error sealed in the raw response — and no charge, because
/// the empty `usage` says the blocked call was not billed.
#[test]
fn parallel_fetch_carries_an_origin_refusal_without_inventing_a_body() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "extract_id": "extract_def",
                "results": [],
                "errors": [{
                    "url": "https://www.ft.com/content/paywalled",
                    "error_type": "http_error",
                    "http_status_code": 403,
                    "content": null
                }],
                "usage": [],
                "session_id": "session_def"
            }))
            .unwrap(),
        )
    });

    let acquisition = ParallelAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.ft.com/content/paywalled")
        .expect("a 200 with a per-URL error is not a transport failure");

    assert!(
        acquisition.envelopes.is_empty(),
        "an origin refusal admits no content"
    );
    assert!(
        String::from_utf8_lossy(&acquisition.raw_response).contains("403"),
        "the origin's refusal is sealed in the response, not thrown away"
    );
    assert!(
        acquisition.charge.money.is_none() && acquisition.charge.native.is_none(),
        "a blocked fetch carried no usage line, so the charge stays unknown, never zero"
    );
}

#[test]
fn firecrawl_search_parses_its_recorded_response_and_charges_in_credits() {
    let origin = Origin::replaying("firecrawl/firecrawl-search.json");
    let adapter = FirecrawlAdapter::new(&origin.url(), "test-key");

    let acquisition = adapter
        .search("EU AI Act", 2, &[])
        .expect("recorded response");

    let request = origin.only_request();
    assert_eq!(request.target, "/v2/search");
    assert_eq!(request.header("Authorization"), Some("Bearer test-key"));

    assert_eq!(acquisition.envelopes.len(), 2);
    assert_eq!(
        acquisition.envelopes[0].host,
        "digital-strategy.ec.europa.eu"
    );
    assert!(acquisition.envelopes[0].text.is_some());
    // No recorded Firecrawl search result carries any date field, and none
    // is documented for the search endpoint; the adapter maps no date at all.
    for envelope in &acquisition.envelopes {
        assert!(envelope.declared_date.is_none());
    }

    // Credits, never currency: no USD figure is returned and the conversion
    // depends on plan, so a dollar amount here would be derived, not observed.
    assert!(
        acquisition.charge.money.is_none(),
        "Firecrawl reports no currency and must not be given one"
    );
    let native = acquisition
        .charge
        .native
        .expect("Firecrawl reports credits");
    assert_eq!(native.unit, "credits");
    assert_eq!(native.amount.as_u64(), Some(2));
    assert_eq!(native.basis, ChargeBasis::Observed);
}

#[test]
fn tavily_search_parses_its_recorded_response_and_marks_its_charge_quoted() {
    let origin = Origin::replaying("tavily/tavily-search.json");
    let adapter = TavilyAdapter::new(&origin.url(), "test-key");

    let acquisition = adapter
        .search("EU AI Act", 2, &[])
        .expect("recorded response");

    assert_eq!(origin.only_request().target, "/search");
    assert_eq!(
        acquisition.provider_request_id.as_deref(),
        Some("299af699-9a7e-45ba-b521-ab26e93535ea")
    );
    assert_eq!(acquisition.envelopes.len(), 2);
    // The provider's own relevance score stays in native metadata; it is not
    // comparable with another provider's.
    assert!(acquisition.envelopes[0].native_metadata["score"].is_number());
    // Tavily declares no date on the general-topic search this adapter
    // performs (its documented `published_date` exists only for news-topic
    // searches), and the recorded response carries none; the adapter maps
    // no date at all.
    for envelope in &acquisition.envelopes {
        assert!(envelope.declared_date.is_none());
    }

    // Tavily reports no charge on any endpoint and its usage counters did not
    // move across a billed search, so the only honest figure is the published
    // price, marked quoted.
    assert!(acquisition.charge.money.is_none());
    let native = acquisition
        .charge
        .native
        .expect("a quoted charge is recorded");
    assert_eq!(native.basis, ChargeBasis::Quoted);
    assert!(native.note.is_some(), "a quoted charge must say why");
}

/// TollBit search returns candidates with no excerpt at all. The adapter must
/// carry that absence through rather than invent text, because a candidate with
/// no text cannot ground an answer.
#[test]
fn tollbit_search_returns_candidates_without_text_or_a_licence() {
    let origin = Origin::replaying("tollbit/tollbit-search.json");
    let adapter = TollbitAdapter::new(&origin.url(), "test-key");

    let acquisition = adapter
        .search("climate", 2, &[])
        .expect("recorded response");

    let request = origin.only_request();
    assert_eq!(request.method, "GET");
    assert!(request.target.starts_with("/dev/v2/search?q="));
    assert_eq!(request.header("TollbitKey"), Some("test-key"));

    assert_eq!(acquisition.envelopes.len(), 2);
    for envelope in &acquisition.envelopes {
        assert!(envelope.text.is_none(), "v2 search returns no excerpt");
        assert!(envelope.content_hash.is_none(), "no text, nothing to hash");
        // `publisher` names who owns the page; it is not a machine-readable
        // licence, and no licence reference was ever obtained from TollBit.
        assert_eq!(envelope.licence, LicenceState::Unknown);
        assert!(envelope.native_metadata.get("publisher").is_some());
    }
    // TollBit is the one provider in the set whose recorded search bytes
    // declare a date at all: `publishedDate` arrived on every item, and the
    // envelope carries it verbatim with provenance naming the field. It is
    // the supplier's claim, never verified against the page.
    let declared: Vec<_> = acquisition
        .envelopes
        .iter()
        .map(|envelope| {
            envelope
                .declared_date
                .as_ref()
                .expect("every recorded TollBit item declares publishedDate")
        })
        .collect();
    assert_eq!(declared[0].date, "2026-07-09T12:12:04");
    assert_eq!(declared[1].date, "2025-07-14T00:00:00");
    for date in &declared {
        assert!(
            date.provenance.contains("supplier-declared") && date.provenance.contains("TollBit"),
            "the provenance must name who declared the date: {}",
            date.provenance
        );
    }
    // Promoted into the envelope, so the fact does not appear twice under
    // two names.
    assert!(
        acquisition.envelopes[0]
            .native_metadata
            .get("publishedDate")
            .is_none()
    );
    assert!(
        acquisition.charge.money.is_none() && acquisition.charge.native.is_none(),
        "search is not a priced TollBit operation; unknown is not free"
    );
}

/// TollBit answered `403` on every rate lookup tried, including two pages it had
/// marked `readyToLicense`. The provider's own words are the evidence, so a
/// refusal must arrive as a status error carrying them, never as an empty
/// result set that reads like "this provider found nothing".
#[test]
fn a_provider_refusal_is_carried_with_its_own_explanation() {
    let recorded = read_capture("tollbit/tollbit-rates.json");
    let status = recorded["response"]["status"].as_u64().unwrap() as u16;
    let body = serde_json::to_vec(&recorded["response"]["body"]).unwrap();
    let origin = Origin::serving(move |_| Response::new(status, body.clone()));

    let error = TollbitAdapter::new(&origin.url(), "test-key")
        .search("climate", 2, &[])
        .expect_err("a 403 is not a successful acquisition");

    match error {
        SupplyError::Status { status, detail, .. } => {
            assert_eq!(status, 403);
            assert!(
                detail.contains("disallowed access"),
                "the provider's own reason must survive: {detail}"
            );
        }
        other => panic!("expected a status error, got {other:?}"),
    }
}

/// Every capture this suite serves is declared in a manifest, and an altered
/// one is refused here exactly as the runtime's replay path refuses it
/// (`an_altered_capture_refuses_to_serve`,
/// `crates/commonmeasure-runtime/tests/replay_mode.rs`). Without this a
/// capture could be edited into a passing parser test and a failing run.
#[test]
fn an_altered_or_undeclared_capture_is_refused() {
    let capture = read_capture("exa/exa-search.json");
    verify_capture("exa/exa-search.json", &capture).expect("the committed capture verifies");

    let mut altered = capture.clone();
    altered["response"]["body"]["results"][0]["title"] = serde_json::json!("edited");
    let refusal = verify_capture("exa/exa-search.json", &altered)
        .expect_err("an edited body must not verify");
    assert!(
        refusal.contains("declared sha256:") && refusal.contains("computed sha256:"),
        "the refusal must name both hashes: {refusal}"
    );

    let refusal = verify_capture("tollbit/tollbit-search-wide.json", &capture)
        .expect_err("a capture no manifest declares must not be served");
    assert!(
        refusal.contains("is in no manifest"),
        "the refusal must say the capture is undeclared: {refusal}"
    );

    // A capture that says it answered another URL is refused on that alone,
    // the same rule the runtime's replay path applies
    // (`a_capture_answering_another_url_refuses_to_serve`).
    let mut moved = capture.clone();
    moved["request"]["url"] = serde_json::json!("https://api.exa.ai/contents");
    let refusal = verify_capture("exa/exa-search.json", &moved)
        .expect_err("a capture answering another URL must not be served");
    assert!(
        refusal.contains("https://api.exa.ai/contents")
            && refusal.contains("https://api.exa.ai/search"),
        "the refusal must name both URLs: {refusal}"
    );
}

/// A body the parser cannot read is an error, not an empty result. Silently
/// returning no sources would make a broken parser look like a quiet provider.
#[test]
fn an_unreadable_body_is_an_error_rather_than_an_empty_result() {
    let origin = Origin::serving(|_| Response::new(200, b"<html>not json</html>".to_vec()));

    let error = ExaAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("an unparseable body must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
}

/// An unconfigured provider produces a named credential gap, and never a
/// request. This is the difference between "we could not ask" and "we asked and
/// got nothing".
#[test]
fn a_missing_credential_names_the_variable_and_sends_nothing() {
    // The state under test is "unconfigured", so the test establishes it
    // rather than hoping for it: a developer with EXA_API_KEY exported would
    // otherwise be running a different test from the one written here. No
    // other test in this binary reads or writes the environment.
    unsafe { std::env::remove_var("EXA_API_KEY") };

    let error = commonmeasure_supply::supplier_from_environment("exa")
        .err()
        .expect("an unconfigured provider must not build an adapter");
    assert_eq!(
        error,
        SupplyError::CredentialMissing {
            variable: "EXA_API_KEY".to_owned()
        }
    );
}

/// A source URL's host is what a deny or allow list is compared against, so
/// the two must be the same name. An absolute DNS name carries a trailing dot
/// that no operator writes into a policy: leaving it on admitted a denied host
/// one character away from the one the job named.
#[test]
fn a_trailing_dot_is_not_a_different_host() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "results": [{"url": "https://banned.example./a", "title": "t", "text": "body"}]
            }))
            .unwrap(),
        )
    });

    let acquisition = ExaAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect("recorded shape");
    assert_eq!(acquisition.envelopes.len(), 1);
    assert_eq!(acquisition.envelopes[0].host, "banned.example");
}

/// A result whose URL has no readable host cannot be checked against a source
/// policy, so it is not offered as supply. It must not arrive carrying the
/// empty host either: that value already means "an internal corpus document",
/// and one string with two meanings decides host policy wrongly whichever way
/// the job is written. The response itself is still sealed.
#[test]
fn a_result_with_no_readable_host_is_not_offered_as_supply() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "results": [
                    {"url": "not a url at all", "title": "t", "text": "body"},
                    {"url": "https://real.example/a", "title": "t", "text": "body"}
                ]
            }))
            .unwrap(),
        )
    });

    let acquisition = ExaAdapter::new(&origin.url(), "test-key")
        .search("anything", 2, &[])
        .expect("recorded shape");
    let hosts: Vec<&str> = acquisition
        .envelopes
        .iter()
        .map(|envelope| envelope.host.as_str())
        .collect();
    assert_eq!(hosts, ["real.example"]);
    assert!(
        String::from_utf8_lossy(&acquisition.raw_response).contains("not a url at all"),
        "the provider's own answer is still sealed whole"
    );
}

/// A charge too fine for the ledger's micro precision is a disclosed price,
/// not an absent one. Recording nothing here made a provider that priced the
/// call indistinguishable from one that disclosed no price at all, and the
/// operator's acquisition cost cap silently stopped being enforced.
#[test]
fn a_charge_finer_than_micro_precision_is_recorded_with_its_reason() {
    let origin = Origin::serving(|_| {
        Response::new(
            200,
            serde_json::to_vec(&serde_json::json!({
                "results": [{"url": "https://real.example/a", "title": "t", "text": "body"}],
                "costDollars": {"total": 0.0075001}
            }))
            .unwrap(),
        )
    });

    let acquisition = ExaAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect("recorded shape");
    assert!(
        acquisition.charge.money.is_none(),
        "no comparable amount can be derived at micro precision"
    );
    let native = acquisition
        .charge
        .native
        .expect("the disclosed price is still recorded");
    assert_eq!(native.unit, "USD");
    assert_eq!(native.basis, ChargeBasis::Observed);
    assert_eq!(native.amount.to_string(), "0.0075001");
    assert!(
        native
            .note
            .expect("the absence of money says why")
            .contains("0.0075001"),
        "the reason names the figure that could not be held"
    );
}

/// A `200` whose body lacks the results array is a changed provider
/// shape, not an empty search. Every adapter fails naming the field, so the run
/// records a gap and builds no acquisition — and therefore no charge — rather
/// than a successful empty result with a quoted price against it.
#[test]
fn a_200_without_a_results_array_is_an_error_naming_the_field_not_an_empty_search() {
    fn empty_object() -> Origin {
        Origin::serving(|_| Response::new(200, b"{}".to_vec()))
    }
    fn assert_names(error: SupplyError, field: &str) {
        assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
        assert!(error.to_string().contains(field), "{error}");
    }

    let origin = empty_object();
    let error = ExaAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `results` must fail");
    assert_names(error, "results");
    let origin = empty_object();
    let error = ExaAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.ft.com/content/x")
        .expect_err("a contents body without `results` must fail");
    assert_names(error, "results");

    let origin = empty_object();
    let error = TavilyAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `results` must fail");
    assert_names(error, "results");
    let origin = empty_object();
    let error = TavilyAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.ft.com/content/x")
        .expect_err("an extract body without `results` must fail");
    assert_names(error, "results");

    let origin = empty_object();
    let error = ParallelAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `results` must fail");
    assert_names(error, "results");
    let origin = empty_object();
    let error = ParallelAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.ft.com/content/x")
        .expect_err("an extract body without `results` must fail");
    assert_names(error, "results");

    // Firecrawl answers under `data.web` (search) and `data` (scrape);
    // TollBit under `items`. The field named is the one that is missing.
    let origin = empty_object();
    let error = FirecrawlAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `data.web` must fail");
    assert_names(error, "data/web");
    let origin = empty_object();
    let error = FirecrawlAdapter::new(&origin.url(), "test-key")
        .fetch("https://www.ft.com/content/x")
        .expect_err("a scrape body without `data` must fail");
    assert_names(error, "data");
    let origin = empty_object();
    let error = TollbitAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("a body without `items` must fail");
    assert_names(error, "items");
}

/// The converse: a present but empty array is a legitimate empty
/// result, and a present field of the wrong type is named for what it is.
#[test]
fn an_empty_results_array_is_an_empty_search_and_a_mistyped_one_is_named() {
    let origin = Origin::serving(|_| Response::new(200, br#"{"results": []}"#.to_vec()));
    let acquisition = ExaAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect("an empty array is an empty result");
    assert!(acquisition.envelopes.is_empty());

    let origin = Origin::serving(|_| Response::new(200, br#"{"results": "none"}"#.to_vec()));
    let error = ExaAdapter::new(&origin.url(), "test-key")
        .search("anything", 1, &[])
        .expect_err("a string where an array belongs must fail");
    assert!(matches!(error, SupplyError::Malformed { .. }), "{error:?}");
    let message = error.to_string();
    assert!(
        message.contains("results") && message.contains("a string"),
        "{message}"
    );
}
