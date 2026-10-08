//! A payment or licence-server term the edge cannot meet refuses the
//! crossing in every policy mode (owner decision, 30 September 2026: no
//! edge holds a settlement rail, so the edge pays or does not fetch). These
//! cases close the routes round that rule:
//!
//! - a payment term is unmet unless the licence says it is free
//!   (`RslPayment::needs_settlement`): an unrecognised type, a case variant
//!   of a defined one, and no type beside a stated amount or terms all
//!   refuse;
//! - a licence with no usage `<permits>` covers AI input on its own terms,
//!   so its payment and licence server are ruled on;
//! - within one `<content>` the edge takes any offer it can meet, whatever
//!   the document order, and the record names the offer taken;
//! - every `License:` line in `robots.txt` is read, and each one's terms
//!   bind.
//!
//! Every case runs through `McpServer::tool_fetch` against a loopback
//! publisher in `observe`, `prefer` and `strict`. The assertions read the
//! agent's text and the source record only, so this file can be dropped onto
//! an earlier revision to show that it fails there. It is a child of
//! `mcp.rs`'s test module and uses its helpers (`server`, `bind_local`,
//! `crossings`).

use super::*;

const MODES: [&str; 3] = ["observe", "prefer", "strict"];

const ROBOTS: &str = "License: /license.xml\nUser-agent: *\nAllow: /\n";

/// The documents a publisher serves, by path.
type Documents = Vec<(&'static str, Vec<u8>)>;

/// `licence` served at `/license.xml`.
fn licence(licence: &[u8]) -> Documents {
    vec![("/license.xml", licence.to_vec())]
}

/// A publisher whose `robots.txt` is `robots` and which serves each of
/// `documents` at its path. Counts the requests for anything else but the
/// well-known probes: the pages.
fn publisher(
    robots: &'static str,
    documents: Documents,
) -> (
    commonmeasure_http::ServerHandle,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    let pages = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&pages);
    let site = bind_local()
        .spawn(move |request| {
            let target = request.target.as_str();
            if target == "/robots.txt" {
                return Response::text(200, robots);
            }
            if let Some((_, body)) = documents.iter().find(|(path, _)| *path == target) {
                // `{origin}` in a document is this publisher's origin, which
                // the port makes known only once it is bound. Any other
                // document is served as its bytes.
                if !body.windows(8).any(|window| window == b"{origin}") {
                    return Response::new(200, body.clone());
                }
                let origin = format!("http://{}", request.headers.get("Host").unwrap_or_default());
                let body = String::from_utf8_lossy(body).replace("{origin}", &origin);
                return Response::new(200, body.into_bytes());
            }
            if target.starts_with("/.well-known/") {
                return Response::text(404, "none");
            }
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Response::text(200, "the article")
        })
        .expect("spawn");
    (site, pages)
}

/// The page every case fetches unless it names another.
const PAGE: &str = "/article";

/// One fetch of `page` in `mode`: the result and the crossing recorded.
fn fetch(
    page: &str,
    robots: &'static str,
    documents: &Documents,
    mode: &str,
) -> (Result<String, String>, Value, usize) {
    let (mut site, pages) = publisher(robots, documents.clone());
    let (home, mut server) =
        server(&json!({"policy_mode": mode, "allow_private_hosts": true}).to_string());
    let result = server
        .tool_fetch(&json!({"url": format!("{}{page}", site.url())}))
        .map(|delivered| delivered.to_string());
    let recorded = crossings(home.path());
    assert_eq!(recorded.len(), 1, "{mode}: {recorded:?}");
    let pages = pages.load(std::sync::atomic::Ordering::SeqCst);
    site.stop();
    (result, recorded[0].clone(), pages)
}

/// In every mode the crossing is refused before the page is asked for, the
/// agent is told `told` and none of `withheld`, and the record's `refusal`
/// carries `on_record`. Returns each mode's crossing for further checks.
fn refused_in_every_mode(
    robots: &'static str,
    documents: Documents,
    told: &str,
    withheld: &[&str],
    on_record: &str,
) -> Vec<Value> {
    refused_at(PAGE, robots, documents, told, withheld, on_record)
}

/// [`refused_in_every_mode`] for `page`.
fn refused_at(
    page: &str,
    robots: &'static str,
    documents: Documents,
    told: &str,
    withheld: &[&str],
    on_record: &str,
) -> Vec<Value> {
    MODES
        .iter()
        .map(|mode| {
            let (result, crossing, pages) = fetch(page, robots, &documents, mode);
            let error = result.expect_err(mode);
            assert!(
                error.starts_with("refused before the crossing: "),
                "{mode}: {error}"
            );
            assert!(error.contains(told), "{mode}: {error}");
            for withheld in withheld {
                assert!(!error.contains(withheld), "{mode}: {error}");
            }
            assert_eq!(pages, 0, "{mode}: the page was never asked for");
            assert_eq!(crossing["event"], "crossing_refused", "{mode}");
            let payload = &crossing["payload"];
            assert_eq!(payload["grounded"], false, "{mode}: {payload}");
            assert!(
                payload["refusal"]
                    .as_str()
                    .is_some_and(|refusal| refusal.contains(on_record)),
                "{mode}: {payload}"
            );
            crossing
        })
        .collect()
}

/// In every mode the page is delivered and nothing is refused. Returns each
/// mode's crossing for further checks.
fn delivered_in_every_mode(robots: &'static str, documents: Documents) -> Vec<Value> {
    delivered_at(PAGE, robots, documents)
}

/// [`delivered_in_every_mode`] for `page`.
fn delivered_at(page: &str, robots: &'static str, documents: Documents) -> Vec<Value> {
    MODES
        .iter()
        .map(|mode| {
            let (result, crossing, pages) = fetch(page, robots, &documents, mode);
            let delivered = result.unwrap_or_else(|error| panic!("{mode}: {error}"));
            assert!(delivered.contains("the article"), "{mode}: {delivered}");
            assert_eq!(pages, 1, "{mode}");
            assert_eq!(crossing["event"], "crossing_mediated", "{mode}");
            assert_eq!(
                crossing["payload"]["declarations"]["effective"]["ai-input"], "allow",
                "{mode}: {crossing}"
            );
            crossing
        })
        .collect()
}

fn payment_of(crossing: &Value, licence: usize) -> &Value {
    &crossing["payload"]["declarations"]["licences"][licence]["terms"]["payment"]
}

// ---------------------------------------------------------------------------
// EDG-133: a payment is unmet unless it is known to be free.

/// A type RSL 1.0 §3.7 does not define, beside an amount. The agent is
/// told the type is one this edge does not recognise, never the type
/// itself, which is the source's text; the record keeps it whole.
#[test]
fn an_unrecognised_payment_type_refuses_in_every_mode() {
    const LICENCE: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<permits type="usage">ai-input</permits>
<payment type="per-token"><amount currency="USD">0.015</amount></payment>
</license></content></rsl>"#;
    for crossing in refused_in_every_mode(
        ROBOTS,
        licence(LICENCE),
        "The licence the source names permits AI input under a payment type this edge does \
         not recognise (0.015000 USD), and this edge holds no settlement rail, so the payment \
         term is unmet.",
        &["per-token"],
        "under payment type per-token (0.015 USD)",
    ) {
        assert_eq!(payment_of(&crossing, 0)["kind"], "per-token", "{crossing}");
    }
}

/// §3.7 gives the defined values as written, and XML attribute values are
/// case-sensitive, so `Use` is not `use`: a type this edge does not
/// recognise, which refuses.
#[test]
fn a_case_variant_of_a_monetary_type_refuses_in_every_mode() {
    const LICENCE: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<permits type="usage">ai-input</permits>
<payment type="Use"><amount currency="USD">0.015</amount></payment>
</license></content></rsl>"#;
    for crossing in refused_in_every_mode(
        ROBOTS,
        licence(LICENCE),
        "permits AI input under a payment type this edge does not recognise (0.015000 USD)",
        &["Use"],
        "under payment type Use (0.015 USD)",
    ) {
        assert_eq!(payment_of(&crossing, 0)["kind"], "Use", "{crossing}");
    }
}

/// §3.7 states no default type. A `<payment>` with no type that states an
/// amount quotes a price, which the edge does not read as free.
#[test]
fn a_payment_with_no_type_and_an_amount_refuses_in_every_mode() {
    const LICENCE: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<permits type="usage">ai-input</permits>
<payment><amount currency="USD">0.015</amount></payment>
</license></content></rsl>"#;
    refused_in_every_mode(
        ROBOTS,
        licence(LICENCE),
        "permits AI input under payment type unstated (0.015000 USD), and this edge holds no \
         settlement rail",
        &[],
        "under payment type unstated (0.015 USD)",
    );
}

/// The same with no amount and a `<standard>` licence it points at: terms
/// the edge has not read and cannot agree to.
#[test]
fn a_payment_with_no_type_and_a_standard_refuses_in_every_mode() {
    const LICENCE: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<permits type="usage">ai-input</permits>
<payment><standard>https://licences.example/collective</standard></payment>
</license></content></rsl>"#;
    refused_in_every_mode(
        ROBOTS,
        licence(LICENCE),
        "permits AI input under payment type unstated, and this edge holds no settlement rail",
        &["licences.example"],
        "under payment type unstated, and this edge holds no settlement rail",
    );
}

/// A `<payment>` with no type and nothing in it says no more than an
/// omitted one, which §3.7 reads as free; `free` and `attribution` ask for
/// nothing a rail would give. Each is delivered.
#[test]
fn a_payment_known_to_be_free_is_delivered_in_every_mode() {
    const EMPTY: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<permits type="usage">ai-input</permits><payment/>
</license></content></rsl>"#;
    const FREE: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<permits type="usage">ai-input</permits><payment type="free"/>
</license></content></rsl>"#;
    const ATTRIBUTION: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<permits type="usage">ai-input</permits>
<payment type="attribution"><standard>https://creativecommons.org/licenses/by/4.0/</standard></payment>
</license></content></rsl>"#;
    delivered_in_every_mode(ROBOTS, licence(EMPTY));
    delivered_in_every_mode(ROBOTS, licence(FREE));
    delivered_in_every_mode(ROBOTS, licence(ATTRIBUTION));
}

// ---------------------------------------------------------------------------
// EDG-26: a licence with no usage `<permits>` prices the crossing.

/// The EDG-130 probe (EDG-26, 30 September): a licence with no usage
/// `<permits>`, a `use` payment and a licence server, named in
/// `robots.txt`, with no `Content-Signal`. RSL 1.0 §3.5 restricts usage
/// only where a `<permits>` of that type exists, so the licence covers AI
/// input and its payment refuses.
#[test]
fn a_priced_licence_with_no_permits_refuses_in_every_mode() {
    const LICENCE: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/" server="https://licensing.example/"><license>
<payment type="use"><amount currency="USD">0.015</amount></payment>
</license></content></rsl>"#;
    const PAYMENT_ONLY: &[u8] =
        br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<payment type="use"><amount currency="USD">0.015</amount></payment>
</license></content></rsl>"#;
    for document in [LICENCE, PAYMENT_ONLY] {
        for crossing in refused_in_every_mode(
            ROBOTS,
            licence(document),
            "The licence the source names permits AI input under payment type use (0.015000 \
             USD), and this edge holds no settlement rail, so the payment term is unmet.",
            &[],
            "permits AI input under payment type use (0.015 USD)",
        ) {
            assert_eq!(payment_of(&crossing, 0)["kind"], "use", "{crossing}");
        }
    }
}

/// A licence server on a licence with no usage `<permits>`, with no
/// robots.txt signal that would make the combined preference Allow.
#[test]
fn a_licence_server_on_a_licence_with_no_permits_refuses_in_every_mode() {
    const LICENCE: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/" server="https://licensing.example/"><license>
<payment type="free"/>
</license></content></rsl>"#;
    refused_in_every_mode(
        ROBOTS,
        licence(LICENCE),
        "The licence the source names requires a licence obtained from a licence server \
         before access, and this edge holds no rail to do so.",
        &["licensing.example"],
        "names a licence server (https://licensing.example/)",
    );
}

/// The same priced licence with a usage `<prohibits>` covering AI input
/// does not cover it: the crossing is refused on the Disallow, and no
/// payment term is read for AI input.
#[test]
fn a_licence_with_no_permits_that_prohibits_ai_input_is_disallowed_not_priced() {
    const LICENCE: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<prohibits type="usage">ai-input</prohibits>
<payment type="use"><amount currency="USD">0.015</amount></payment>
</license></content></rsl>"#;
    for crossing in refused_in_every_mode(
        ROBOTS,
        licence(LICENCE),
        "The source disallows AI input (in its RSL licence).",
        &["payment"],
        "The source disallows AI input (",
    ) {
        let payload = &crossing["payload"];
        assert_eq!(
            payload["declarations"]["effective"]["ai-input"], "disallow",
            "{payload}"
        );
        assert!(payment_of(&crossing, 0).is_null(), "{payload}");
        assert!(
            !payload["refusal"].as_str().unwrap().contains("payment"),
            "{payload}"
        );
    }
}

const PRICED_OFFER: &str = r#"<license><permits type="usage">ai-input</permits>
<payment type="use"><amount currency="USD">0.015</amount></payment></license>"#;
const FREE_OFFER: &str =
    r#"<license><permits type="usage">ai-input</permits><payment type="free"/></license>"#;

fn offers(first: &str, second: &str) -> Documents {
    licence(
        format!(
            r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/">{first}{second}</content></rsl>"#
        )
        .as_bytes(),
    )
}

/// RSL 1.0 §3.1.1: the order of licences does not change what they mean,
/// and each is an offer. A free offer written after a priced one admits,
/// and the record names the offer taken.
#[test]
fn a_free_offer_behind_a_priced_one_is_taken_in_every_mode() {
    for crossing in delivered_in_every_mode(ROBOTS, offers(PRICED_OFFER, FREE_OFFER)) {
        let terms = &crossing["payload"]["declarations"]["licences"][0]["terms"];
        assert_eq!(terms["offer"], 2, "{terms}");
        assert_eq!(terms["payment"]["kind"], "free", "{terms}");
    }
}

/// A priced offer written after a free one changes nothing.
#[test]
fn a_priced_offer_behind_a_free_one_is_not_taken_in_every_mode() {
    for crossing in delivered_in_every_mode(ROBOTS, offers(FREE_OFFER, PRICED_OFFER)) {
        let terms = &crossing["payload"]["declarations"]["licences"][0]["terms"];
        assert_eq!(terms["offer"], 1, "{terms}");
        assert_eq!(terms["payment"]["kind"], "free", "{terms}");
    }
}

/// Two `License:` lines: each is a separate document, every one is read,
/// and the most restrictive combination of their terms binds (RSL 1.0
/// §4.4.3 and §4.9). A free licence first and a priced one second refuses
/// on the second, and the record holds both.
#[test]
fn every_robots_licence_is_read_and_a_priced_second_one_refuses_in_every_mode() {
    const FREE: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<permits type="usage">ai-input</permits><payment type="free"/>
</license></content></rsl>"#;
    const PRICED: &[u8] = br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"><license>
<permits type="usage">ai-input</permits>
<payment type="use"><amount currency="USD">0.015</amount></payment>
</license></content></rsl>"#;
    for crossing in refused_in_every_mode(
        "License: /free.xml\nLicense: /priced.xml\nUser-agent: *\nAllow: /\n",
        vec![
            ("/free.xml", FREE.to_vec()),
            ("/priced.xml", PRICED.to_vec()),
        ],
        "permits AI input under payment type use (0.015000 USD)",
        &[],
        "/priced.xml permits AI input under payment type use",
    ) {
        let licences = crossing["payload"]["declarations"]["licences"]
            .as_array()
            .expect("licences");
        let urls: Vec<&str> = licences
            .iter()
            .map(|licence| licence["url"].as_str().unwrap())
            .collect();
        assert_eq!(urls.len(), 2, "{urls:?}");
        assert!(urls[0].ends_with("/free.xml"), "{urls:?}");
        assert!(urls[1].ends_with("/priced.xml"), "{urls:?}");
        assert_eq!(payment_of(&crossing, 1)["kind"], "use");
    }
}

// ---------------------------------------------------------------------------
// A licence silent on usage is a grant only where no licence of the same
// `<content>` names AI input (RSL 1.0 §3.1.1: the more specific declaration
// takes precedence, a prohibition over a permit, and licences are read
// conservatively).

fn content(licences: &str) -> Documents {
    licence(
        format!(r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/">{licences}</content></rsl>"#)
            .as_bytes(),
    )
}

/// In every mode the crossing is refused on the licence's Disallow, the
/// agent is told none of `withheld`, and the prohibition is on the record:
/// in the declarations' statements and in the licence's own, naming
/// `licence` as the one that does not permit AI input. Returns each mode's
/// crossing for further checks.
fn disallowed_in_every_mode(documents: Documents, licence: &str, withheld: &[&str]) -> Vec<Value> {
    disallowed_at(PAGE, documents, licence, withheld)
}

/// [`disallowed_in_every_mode`] for `page`.
fn disallowed_at(page: &str, documents: Documents, licence: &str, withheld: &[&str]) -> Vec<Value> {
    let withheld: Vec<&str> = ["payment"]
        .into_iter()
        .chain(withheld.iter().copied())
        .collect();
    let crossings = refused_at(
        page,
        ROBOTS,
        documents,
        "The source disallows AI input (in its RSL licence).",
        &withheld,
        "The source disallows AI input (",
    );
    for crossing in &crossings {
        let declarations = &crossing["payload"]["declarations"];
        assert_eq!(
            declarations["effective"]["ai-input"], "disallow",
            "{declarations}"
        );
        let disallows = |statements: &Value| {
            statements.as_array().is_some_and(|statements| {
                statements.iter().any(|statement| {
                    statement["category"] == "ai-input"
                        && statement["preference"] == "disallow"
                        && statement["detail"].as_str().is_some_and(|detail| {
                            detail.ends_with(&format!(
                                "no licence permits usage ai-input (licence {licence})"
                            ))
                        })
                })
            })
        };
        assert!(disallows(&declarations["statements"]), "{declarations}");
        assert!(
            disallows(&declarations["licences"][0]["terms"]["statements"]),
            "{declarations}"
        );
        assert!(
            declarations["licences"][0]["terms"].get("offer").is_none(),
            "{declarations}"
        );
    }
    crossings
}

/// A sibling's `<prohibits type="usage">ai-input</prohibits>` beside a free
/// licence silent on usage: the prohibition is the specific term, so the
/// crossing is refused on it, as before the silent licence was read as a
/// grant.
#[test]
fn a_sibling_prohibition_of_ai_input_beside_a_silent_free_licence_refuses_in_every_mode() {
    disallowed_in_every_mode(
        content(
            r#"<license><prohibits type="usage">ai-input</prohibits></license><license><payment type="free"/></license>"#,
        ),
        "1",
        &[],
    );
}

/// The same with `ai-all`, beside an empty `<license/>`.
#[test]
fn a_sibling_prohibition_of_ai_all_beside_an_empty_licence_refuses_in_every_mode() {
    disallowed_in_every_mode(
        content(
            r#"<license><prohibits type="usage">ai-all</prohibits><payment type="free"/></license><license></license>"#,
        ),
        "1",
        &[],
    );
}

/// A licence that prices AI input by name beside an attribution licence
/// silent on usage: the priced term is the specific one, so the silent
/// licence is no offer and the payment refuses.
#[test]
fn a_priced_offer_by_name_beside_a_silent_free_licence_refuses_in_every_mode() {
    for crossing in refused_in_every_mode(
        ROBOTS,
        offers(
            PRICED_OFFER,
            r#"<license><payment type="attribution"/></license>"#,
        ),
        "The licence the source names permits AI input under payment type use (0.015000 \
         USD), and this edge holds no settlement rail, so the payment term is unmet.",
        &[],
        "permits AI input under payment type use (0.015 USD)",
    ) {
        let terms = &crossing["payload"]["declarations"]["licences"][0]["terms"];
        assert_eq!(terms["offer"], 1, "{terms}");
        assert_eq!(terms["payment"]["kind"], "use", "{terms}");
    }
}

/// A sibling that names only other usages (`search`) does not name AI input,
/// so a silent attribution licence beside it is still a grant.
#[test]
fn a_silent_licence_beside_a_sibling_naming_only_search_is_delivered_in_every_mode() {
    for crossing in delivered_in_every_mode(
        ROBOTS,
        content(
            r#"<license><permits type="usage">search</permits><payment type="free"/></license><license><payment type="attribution"><standard>https://creativecommons.org/licenses/by/4.0/</standard></payment></license>"#,
        ),
    ) {
        let terms = &crossing["payload"]["declarations"]["licences"][0]["terms"];
        assert_eq!(terms["offer"], 2, "{terms}");
    }
}

/// `Content-Signal: ai-input=no` beside a silent free licence: the signal's
/// Disallow wins the combination.
#[test]
fn a_content_signal_refusing_ai_input_beside_a_silent_free_licence_refuses_in_every_mode() {
    refused_in_every_mode(
        "License: /license.xml\nUser-agent: *\nAllow: /\nContent-Signal: ai-input=no\n",
        content(r#"<license><payment type="free"/></license>"#),
        "The source disallows AI input (in a Content-Signal line in its robots.txt).",
        &[],
        "The source disallows AI input (",
    );
}

// ---------------------------------------------------------------------------
// Several `<payment>` elements in one licence: it needs settlement if any
// one does, and the record keeps every one.

const PRICED_PAYMENT: &str =
    r#"<payment type="use"><amount currency="USD">0.015</amount></payment>"#;
const FREE_PAYMENT: &str = r#"<payment type="free"/>"#;

fn payments(first: &str, second: &str) -> Documents {
    content(&format!(
        r#"<license><permits type="usage">ai-input</permits>{first}{second}</license>"#
    ))
}

fn kinds(crossing: &Value) -> Vec<&str> {
    crossing["payload"]["declarations"]["licences"][0]["terms"]["payments"]
        .as_array()
        .map(|payments| {
            payments
                .iter()
                .map(|payment| payment["kind"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_priced_payment_then_a_free_one_in_one_licence_refuses_in_every_mode() {
    for crossing in refused_in_every_mode(
        ROBOTS,
        payments(PRICED_PAYMENT, FREE_PAYMENT),
        "permits AI input under payment type use (0.015000 USD), and this edge holds no \
         settlement rail",
        &[],
        "permits AI input under payment type use (0.015 USD)",
    ) {
        assert_eq!(payment_of(&crossing, 0)["kind"], "use", "{crossing}");
        assert_eq!(kinds(&crossing), ["use", "free"], "{crossing}");
    }
}

#[test]
fn a_free_payment_then_a_priced_one_in_one_licence_refuses_in_every_mode() {
    for crossing in refused_in_every_mode(
        ROBOTS,
        payments(FREE_PAYMENT, PRICED_PAYMENT),
        "permits AI input under payment type use (0.015000 USD), and this edge holds no \
         settlement rail",
        &[],
        "permits AI input under payment type use (0.015 USD)",
    ) {
        assert_eq!(payment_of(&crossing, 0)["kind"], "use", "{crossing}");
        assert_eq!(kinds(&crossing), ["free", "use"], "{crossing}");
    }
}

#[test]
fn two_free_payments_in_one_licence_are_delivered_in_every_mode() {
    for crossing in delivered_in_every_mode(
        ROBOTS,
        payments(FREE_PAYMENT, r#"<payment type="attribution"/>"#),
    ) {
        assert_eq!(payment_of(&crossing, 0)["kind"], "free", "{crossing}");
        assert_eq!(kinds(&crossing), ["free", "attribution"], "{crossing}");
    }
}

// ---------------------------------------------------------------------------
// A licence server binds whatever the offers (RSL 1.0 §3.7), a `<content>`
// with no `<license>` included.

/// In every mode the page is delivered with AI input unknown: nothing the
/// source states authorises it or refuses it.
fn delivered_unknown_in_every_mode(documents: Documents) -> Vec<Value> {
    MODES
        .iter()
        .map(|mode| {
            let (result, crossing, pages) = fetch(PAGE, ROBOTS, &documents, mode);
            let delivered = result.unwrap_or_else(|error| panic!("{mode}: {error}"));
            assert!(delivered.contains("the article"), "{mode}: {delivered}");
            assert_eq!(pages, 1, "{mode}");
            assert_eq!(crossing["event"], "crossing_mediated", "{mode}");
            let declarations = &crossing["payload"]["declarations"];
            assert!(
                declarations["effective"].get("ai-input").is_none()
                    || declarations["effective"]["ai-input"] == "unknown",
                "{mode}: {declarations}"
            );
            assert!(
                declarations["licences"][0]["terms"].get("offer").is_none(),
                "{mode}: {declarations}"
            );
            crossing
        })
        .collect()
}

#[test]
fn a_licence_server_on_a_content_entry_with_no_licence_refuses_in_every_mode() {
    refused_in_every_mode(
        ROBOTS,
        licence(
            br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/" server="https://licensing.example/"/></rsl>"#,
        ),
        "The licence the source names requires a licence obtained from a licence server \
         before access, and this edge holds no rail to do so.",
        &["licensing.example"],
        "names a licence server (https://licensing.example/)",
    );
}

/// A `<content>` with no `<license>` and no server makes no offer: the page
/// is delivered with AI input unknown, as before.
#[test]
fn a_content_entry_with_no_licence_and_no_server_makes_no_offer_in_every_mode() {
    delivered_unknown_in_every_mode(licence(
        br#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/"></content></rsl>"#,
    ));
}

// ---------------------------------------------------------------------------
// A licence restricted to a class of user or a region authorises nothing:
// the edge cannot show it is in the class.

/// A sibling that permits only `search` beside a free licence restricted to
/// education users: the restricted licence is no offer, so the sibling's
/// Disallow decides.
#[test]
fn a_user_restricted_silent_licence_beside_a_search_only_sibling_refuses_in_every_mode() {
    disallowed_in_every_mode(
        content(
            r#"<license><permits type="usage">search</permits></license><license><permits type="user">education</permits><payment type="free"/></license>"#,
        ),
        "1",
        &[],
    );
}

/// Alone, such a licence, free, leaves AI input unknown, not allowed, and
/// is recorded as restricted, whether the restriction is a class of user or
/// a region. A free payment term is not recorded: there is no offer.
#[test]
fn a_free_restricted_silent_licence_alone_leaves_ai_input_unknown_in_every_mode() {
    for restriction in [
        r#"<permits type="user">non-commercial</permits>"#,
        r#"<permits type="geo">GB</permits>"#,
    ] {
        for crossing in delivered_unknown_in_every_mode(content(&format!(
            r#"<license>{restriction}<payment type="free"/></license>"#
        ))) {
            let terms = &crossing["payload"]["declarations"]["licences"][0]["terms"];
            assert_eq!(terms["restricted"], json!([1]), "{terms}");
            assert!(terms.get("payment").is_none(), "{terms}");
        }
    }
}

/// The price a restricted licence asks is on the record although no offer
/// is: `payment` holds it and `offer` is absent.
fn priced_on_the_record(crossing: &Value, decimal: &str) {
    let terms = &crossing["payload"]["declarations"]["licences"][0]["terms"];
    assert_eq!(terms["payment"]["amount"]["currency"], "USD", "{terms}");
    assert_eq!(terms["payment"]["amount"]["decimal"], decimal, "{terms}");
    assert_eq!(terms["restricted"], json!([1]), "{terms}");
    assert!(terms.get("offer").is_none(), "{terms}");
}

/// A priced licence that permits AI input by name only within a region is
/// no offer this edge can take, and is not read as no term at all: for this
/// edge it permits no AI input, so the crossing refuses on the Disallow.
/// Its price stays on the record; the agent is told neither the region nor
/// the price.
#[test]
fn a_priced_licence_permitting_ai_input_only_in_a_region_refuses_in_every_mode() {
    for crossing in disallowed_in_every_mode(
        content(&format!(
            r#"<license><permits type="usage">ai-input</permits><permits type="geo">GB</permits>{PRICED_PAYMENT}</license>"#
        )),
        "1",
        &["GB", "0.015"],
    ) {
        priced_on_the_record(&crossing, "0.015");
        assert_eq!(payment_of(&crossing, 0)["kind"], "use", "{crossing}");
    }
}

/// A licence silent on usage, restricted to education users, at a price.
/// Inside the class the price binds and this edge cannot pay; outside it
/// nothing grants the page. Neither admits AI input, so the crossing
/// refuses on the Disallow, and the price is on the record.
#[test]
fn a_user_restricted_silent_priced_licence_refuses_in_every_mode() {
    for crossing in disallowed_in_every_mode(
        content(&format!(
            r#"<license><permits type="user">education</permits>{PRICED_PAYMENT}</license>"#
        )),
        "1",
        &["education", "0.015"],
    ) {
        priced_on_the_record(&crossing, "0.015");
        assert_eq!(payment_of(&crossing, 0)["kind"], "use", "{crossing}");
    }
}

/// The same restricted to a region.
#[test]
fn a_region_restricted_silent_priced_licence_refuses_in_every_mode() {
    for crossing in disallowed_in_every_mode(
        content(&format!(
            r#"<license><permits type="geo">GB</permits>{PRICED_PAYMENT}</license>"#
        )),
        "1",
        &["GB", "0.015"],
    ) {
        priced_on_the_record(&crossing, "0.015");
    }
}

/// A user prohibition restricts the licence too, and a `<payment>` with no
/// type and an amount asks for payment.
#[test]
fn a_user_prohibiting_silent_licence_with_an_untyped_amount_refuses_in_every_mode() {
    for crossing in disallowed_in_every_mode(
        content(
            r#"<license><prohibits type="user">commercial</prohibits><payment><amount currency="USD">0.015</amount></payment></license>"#,
        ),
        "1",
        &["commercial", "0.015"],
    ) {
        priced_on_the_record(&crossing, "0.015");
        assert!(payment_of(&crossing, 0).get("kind").is_none(), "{crossing}");
    }
}

// ---------------------------------------------------------------------------
// `<accepts>` with no type asks for payment.

/// RSL 1.0 §3.7: `<accepts>` lists the payment methods the source takes. A
/// source that lists them is asking for payment, so a `<payment>` with no
/// type and only `<accepts>` refuses.
#[test]
fn a_payment_with_no_type_and_only_accepts_refuses_in_every_mode() {
    for crossing in refused_in_every_mode(
        ROBOTS,
        content(
            r#"<license><permits type="usage">ai-input</permits><payment><accepts>card</accepts></payment></license>"#,
        ),
        "permits AI input under payment type unstated, and this edge holds no settlement rail",
        &["card"],
        "under payment type unstated, and this edge holds no settlement rail",
    ) {
        assert_eq!(payment_of(&crossing, 0)["accepts"], "card", "{crossing}");
    }
}

// ---------------------------------------------------------------------------
// Every usage `<permits>` and `<prohibits>` element of a licence is read.
// RSL 1.0 §3.5 allows one of each type; a document that writes more states
// each as a term, and their order does not change what they mean (§3.1).

/// The two orders of two usage elements of one kind.
fn both_orders(element: &str) -> [String; 2] {
    let one = |token: &str| format!(r#"<{element} type="usage">{token}</{element}>"#);
    [
        format!("{}{}", one("ai-input"), one("search")),
        format!("{}{}", one("search"), one("ai-input")),
    ]
}

/// A priced licence that permits AI input in one of two usage `<permits>`
/// names AI input, so a free licence silent on usage beside it is no offer,
/// and the payment refuses, in either element order.
#[test]
fn a_priced_licence_with_two_usage_permits_beside_a_silent_free_licence_refuses_in_every_mode() {
    for permits in both_orders("permits") {
        for crossing in refused_in_every_mode(
            ROBOTS,
            content(&format!(
                r#"<license>{permits}{PRICED_PAYMENT}</license><license>{FREE_PAYMENT}</license>"#
            )),
            "The licence the source names permits AI input under payment type use (0.015000 \
             USD), and this edge holds no settlement rail, so the payment term is unmet.",
            &[],
            "permits AI input under payment type use (0.015 USD)",
        ) {
            let terms = &crossing["payload"]["declarations"]["licences"][0]["terms"];
            assert_eq!(terms["offer"], 1, "{permits}: {terms}");
            assert_eq!(terms["payment"]["kind"], "use", "{permits}: {terms}");
        }
    }
}

/// The same licence alone refuses on the payment, as before.
#[test]
fn a_priced_licence_with_two_usage_permits_alone_refuses_in_every_mode() {
    for permits in both_orders("permits") {
        refused_in_every_mode(
            ROBOTS,
            content(&format!(r#"<license>{permits}{PRICED_PAYMENT}</license>"#)),
            "permits AI input under payment type use (0.015000 USD)",
            &[],
            "permits AI input under payment type use (0.015 USD)",
        );
    }
}

/// A free licence that prohibits AI input in one of two usage
/// `<prohibits>` prohibits it, in either element order.
#[test]
fn a_free_licence_with_two_usage_prohibits_refuses_in_every_mode() {
    for prohibits in both_orders("prohibits") {
        disallowed_in_every_mode(
            content(&format!(r#"<license>{prohibits}{FREE_PAYMENT}</license>"#)),
            "1",
            &[],
        );
    }
}

/// The same prohibitions beside a free licence silent on usage: the
/// prohibition names AI input, so the silent licence is no offer.
#[test]
fn a_licence_with_two_usage_prohibits_beside_a_silent_free_licence_refuses_in_every_mode() {
    for prohibits in both_orders("prohibits") {
        disallowed_in_every_mode(
            content(&format!(
                r#"<license>{prohibits}</license><license>{FREE_PAYMENT}</license>"#
            )),
            "1",
            &[],
        );
    }
}

/// A free licence permitting `ai-input` and `search` in two usage
/// `<permits>` is ruled the same in either order: delivered under it.
#[test]
fn a_free_licence_with_two_usage_permits_is_delivered_in_either_order_in_every_mode() {
    for permits in both_orders("permits") {
        for crossing in delivered_in_every_mode(
            ROBOTS,
            content(&format!("<license>{permits}{FREE_PAYMENT}</license>")),
        ) {
            let terms = &crossing["payload"]["declarations"]["licences"][0]["terms"];
            assert_eq!(terms["offer"], 1, "{permits}: {terms}");
        }
    }
}

// ---------------------------------------------------------------------------
// Two `<content>` entries of equal scope are read as one entry, their
// licences in document order (RSL 1.0 §3.1: order does not change what the
// document means).

/// `entries` as one document, each a `(url attributes, licences)` pair.
fn entries(entries: &[(&str, &str)]) -> Documents {
    let contents: String = entries
        .iter()
        .map(|(attributes, licences)| format!("<content {attributes}>{licences}</content>"))
        .collect();
    licence(format!(r#"<rsl xmlns="https://rslstandard.org/rsl">{contents}</rsl>"#).as_bytes())
}

const PROHIBITS_AI_INPUT: &str =
    r#"<license><prohibits type="usage">ai-input</prohibits></license>"#;
const SILENT_FREE: &str = r#"<license><payment type="free"/></license>"#;

/// A prohibition in one entry and a free licence silent on usage in another
/// of the same scope are siblings: the prohibition names AI input, so the
/// silent licence is no offer, in either order. The Disallow names the
/// prohibiting licence by its position across both entries.
#[test]
fn a_prohibition_and_a_silent_licence_in_two_equal_entries_refuse_in_either_order() {
    disallowed_in_every_mode(
        entries(&[
            (r#"url="/""#, PROHIBITS_AI_INPUT),
            (r#"url="/""#, SILENT_FREE),
        ]),
        "1",
        &[],
    );
    disallowed_in_every_mode(
        entries(&[
            (r#"url="/""#, SILENT_FREE),
            (r#"url="/""#, PROHIBITS_AI_INPUT),
        ]),
        "2",
        &[],
    );
}

/// A priced offer by name in one entry and a free one in another of the
/// same scope are two offers of one entry: the free one is taken in either
/// order, as in one `<content>`, and the record names it by its position
/// across both entries.
#[test]
fn a_priced_and_a_free_offer_in_two_equal_entries_are_ruled_alike_in_either_order() {
    for (first, second, taken) in [(PRICED_OFFER, FREE_OFFER, 2), (FREE_OFFER, PRICED_OFFER, 1)] {
        for crossing in delivered_in_every_mode(
            ROBOTS,
            entries(&[(r#"url="/""#, first), (r#"url="/""#, second)]),
        ) {
            let terms = &crossing["payload"]["declarations"]["licences"][0]["terms"];
            assert_eq!(terms["offer"], taken, "{terms}");
            assert_eq!(terms["payment"]["kind"], "free", "{terms}");
        }
    }
}

/// A licence server on the first of two equal entries binds the crossing,
/// whatever the second says.
#[test]
fn a_licence_server_on_the_first_of_two_equal_entries_refuses_in_every_mode() {
    refused_in_every_mode(
        ROBOTS,
        entries(&[
            (r#"url="/" server="https://licensing.example/""#, FREE_OFFER),
            (r#"url="/""#, FREE_OFFER),
        ]),
        "The licence the source names requires a licence obtained from a licence server \
         before access, and this edge holds no rail to do so.",
        &["licensing.example"],
        "names a licence server (https://licensing.example/)",
    );
}

/// An entry whose scope contains another matching entry's is set aside:
/// the narrower one governs alone, in either order, so a site-wide
/// prohibition does not reach a page a narrower entry licenses free.
#[test]
fn a_more_specific_entry_governs_alone_beside_a_less_specific_one_in_every_mode() {
    for order in [
        [
            (r#"url="/""#, PROHIBITS_AI_INPUT),
            (r#"url="/article""#, SILENT_FREE),
        ],
        [
            (r#"url="/article""#, SILENT_FREE),
            (r#"url="/""#, PROHIBITS_AI_INPUT),
        ],
    ] {
        for crossing in delivered_in_every_mode(ROBOTS, entries(&order)) {
            let licence = &crossing["payload"]["declarations"]["licences"][0];
            assert_eq!(licence["content"], "/article", "{licence}");
            assert_eq!(licence["terms"]["offer"], 1, "{licence}");
        }
    }
}

// ---------------------------------------------------------------------------
// Of two entries, the one whose scope lies within the other's governs alone
// (RSL 1.0 §3.1.1), whatever their lengths. Entries of the same scope are
// read as one, so `/` and `/*` are one entry.

const PRICED_REFUSAL: &str = "The licence the source names permits AI input under payment type \
                              use (0.015000 USD), and this edge holds no settlement rail, so the \
                              payment term is unmet.";
const PRICED_ON_RECORD: &str = "permits AI input under payment type use (0.015 USD)";

/// The two entries as one document, in both orders.
fn either_order(one: (&str, &str), other: (&str, &str)) -> [Documents; 2] {
    [entries(&[one, other]), entries(&[other, one])]
}

/// In every mode, `page` is refused on the payment of the one licence of
/// the entry `content`, which governs alone.
fn priced_alone_at(page: &str, documents: Documents, content: &str) {
    for crossing in refused_at(
        page,
        ROBOTS,
        documents,
        PRICED_REFUSAL,
        &[],
        PRICED_ON_RECORD,
    ) {
        let licence = &crossing["payload"]["declarations"]["licences"][0];
        assert_eq!(licence["content"], content, "{licence}");
        assert_eq!(licence["terms"]["offer"], 1, "{licence}");
        assert_eq!(licence["terms"]["payment"]["kind"], "use", "{licence}");
    }
}

/// `/p` is narrower than `/*` for `/page`, though both are two octets, so
/// its price binds and the site-wide free offer is not taken.
#[test]
fn a_priced_literal_path_governs_over_a_free_wildcard_of_its_length_in_either_order() {
    for documents in either_order((r#"url="/*""#, FREE_OFFER), (r#"url="/p""#, PRICED_OFFER)) {
        priced_alone_at("/page", documents, "/p");
    }
}

/// `/p`'s prohibition governs over a site-wide free offer on `/*`.
#[test]
fn a_prohibiting_literal_path_governs_over_a_free_wildcard_of_its_length_in_either_order() {
    for documents in either_order(
        (r#"url="/*""#, FREE_OFFER),
        (r#"url="/p""#, PROHIBITS_AI_INPUT),
    ) {
        for crossing in disallowed_at("/page", documents, "1", &[]) {
            let licence = &crossing["payload"]["declarations"]["licences"][0];
            assert_eq!(licence["content"], "/p", "{licence}");
        }
    }
}

/// `/p`'s free offer governs over a site-wide prohibition on `/*`, and the
/// record names `/p` and its one licence.
#[test]
fn a_free_literal_path_governs_over_a_prohibiting_wildcard_of_its_length_in_either_order() {
    for documents in either_order(
        (r#"url="/*""#, PROHIBITS_AI_INPUT),
        (r#"url="/p""#, FREE_OFFER),
    ) {
        for crossing in delivered_at("/page", ROBOTS, documents) {
            let licence = &crossing["payload"]["declarations"]["licences"][0];
            assert_eq!(licence["content"], "/p", "{licence}");
            assert_eq!(licence["terms"]["offer"], 1, "{licence}");
        }
    }
}

/// `/news*` covers what `/news` covers, so `/news/` is narrower and its
/// price binds `/news/1`.
#[test]
fn a_trailing_wildcard_does_not_make_a_free_scope_as_narrow_as_a_priced_one() {
    for documents in either_order(
        (r#"url="/news*""#, FREE_OFFER),
        (r#"url="/news/""#, PRICED_OFFER),
    ) {
        priced_alone_at("/news/1", documents, "/news/");
    }
}

/// `/a/` and `/*/` are one length and `/a/` lies within `/*/`, so its
/// price binds `/a/1`.
#[test]
fn a_literal_path_governs_over_an_inner_wildcard_of_its_length_in_either_order() {
    for documents in either_order((r#"url="/*/""#, FREE_OFFER), (r#"url="/a/""#, PRICED_OFFER)) {
        priced_alone_at("/a/1", documents, "/a/");
    }
}

/// `/` and `/*` cover every page, so they are one entry: the prohibition in
/// one names AI input, and the free licence silent on usage in the other is
/// no offer, in either order.
#[test]
fn a_prohibition_on_root_and_a_silent_licence_on_root_wildcard_refuse_in_either_order() {
    disallowed_in_every_mode(
        entries(&[
            (r#"url="/""#, PROHIBITS_AI_INPUT),
            (r#"url="/*""#, SILENT_FREE),
        ]),
        "1",
        &[],
    );
    disallowed_in_every_mode(
        entries(&[
            (r#"url="/*""#, SILENT_FREE),
            (r#"url="/""#, PROHIBITS_AI_INPUT),
        ]),
        "2",
        &[],
    );
}

/// A priced offer by name on `/` and a free one on `/*` are two offers of
/// one entry: the free one is taken in either order, as in one `<content>`,
/// and the record names the first entry and the offer by its position
/// across both.
#[test]
fn a_priced_offer_on_root_and_a_free_one_on_root_wildcard_are_ruled_alike_in_either_order() {
    for (first, second, content, taken) in [
        (
            (r#"url="/""#, PRICED_OFFER),
            (r#"url="/*""#, FREE_OFFER),
            "/",
            2,
        ),
        (
            (r#"url="/*""#, FREE_OFFER),
            (r#"url="/""#, PRICED_OFFER),
            "/*",
            1,
        ),
    ] {
        for crossing in delivered_in_every_mode(ROBOTS, entries(&[first, second])) {
            let licence = &crossing["payload"]["declarations"]["licences"][0];
            assert_eq!(licence["content"], content, "{licence}");
            assert_eq!(licence["terms"]["offer"], taken, "{licence}");
            assert_eq!(licence["terms"]["payment"]["kind"], "free", "{licence}");
        }
    }
}

/// A literal path within the wildcard governs alone: priced and prohibited
/// refuse, permitted is delivered.
#[test]
fn a_longer_literal_path_governs_alone_beside_a_wildcard_in_every_mode() {
    for documents in either_order((r#"url="/*""#, FREE_OFFER), (r#"url="/pa""#, PRICED_OFFER)) {
        priced_alone_at("/page", documents, "/pa");
    }
    for documents in either_order(
        (r#"url="/*""#, FREE_OFFER),
        (r#"url="/pa""#, PROHIBITS_AI_INPUT),
    ) {
        disallowed_at("/page", documents, "1", &[]);
    }
    for documents in either_order(
        (r#"url="/*""#, PROHIBITS_AI_INPUT),
        (r#"url="/pa""#, FREE_OFFER),
    ) {
        for crossing in delivered_at("/page", ROBOTS, documents) {
            let licence = &crossing["payload"]["declarations"]["licences"][0];
            assert_eq!(licence["content"], "/pa", "{licence}");
            assert_eq!(licence["terms"]["offer"], 1, "{licence}");
        }
    }
}

// ---------------------------------------------------------------------------
// Of two entries, the one whose scope lies within the other's governs alone;
// neither lying within the other, they are read as one entry. An absolute
// scope takes part by its path.

/// `/p*` and `/*p` are three octets each, and `/p*` lies within `/*p`, so
/// its prohibition governs `/p` over a silent free licence on `/*p`.
#[test]
fn a_prohibiting_scope_within_a_free_one_of_its_length_governs_in_either_order() {
    for documents in either_order(
        (r#"url="/p*""#, PROHIBITS_AI_INPUT),
        (r#"url="/*p""#, SILENT_FREE),
    ) {
        for crossing in disallowed_at("/p", documents, "1", &[]) {
            let licence = &crossing["payload"]["declarations"]["licences"][0];
            assert_eq!(licence["content"], "/p*", "{licence}");
        }
    }
}

/// `/p*`'s price binds `/p`, and the free offer on `/*p` is not taken.
#[test]
fn a_priced_scope_within_a_free_one_of_its_length_governs_in_either_order() {
    for documents in either_order((r#"url="/p*""#, PRICED_OFFER), (r#"url="/*p""#, FREE_OFFER)) {
        priced_alone_at("/p", documents, "/p*");
    }
}

/// `/abc$` names only `/abc`, which lies within `/ab*c` of the same length,
/// so its prohibition governs over `/ab*c`'s free offer by name.
#[test]
fn an_anchored_prohibition_within_a_free_wildcard_of_its_length_governs_in_either_order() {
    for documents in either_order(
        (r#"url="/abc$""#, PROHIBITS_AI_INPUT),
        (r#"url="/ab*c""#, FREE_OFFER),
    ) {
        for crossing in disallowed_at("/abc", documents, "1", &[]) {
            let licence = &crossing["payload"]["declarations"]["licences"][0];
            assert_eq!(licence["content"], "/abc$", "{licence}");
        }
    }
}

/// `/a*/x` and `/*b*x` are one length and neither lies within the other,
/// so they are one entry: the prohibition names AI input and the silent
/// licence is no offer. The Disallow names the prohibiting licence by its
/// position across both.
#[test]
fn a_prohibition_and_a_silent_licence_on_overlapping_scopes_of_one_length_refuse() {
    disallowed_at(
        "/ab/x",
        entries(&[
            (r#"url="/a*/x""#, SILENT_FREE),
            (r#"url="/*b*x""#, PROHIBITS_AI_INPUT),
        ]),
        "2",
        &[],
    );
    disallowed_at(
        "/ab/x",
        entries(&[
            (r#"url="/*b*x""#, PROHIBITS_AI_INPUT),
            (r#"url="/a*/x""#, SILENT_FREE),
        ]),
        "1",
        &[],
    );
}

/// `/a*` and `/*b` overlap at `/ab` and neither lies within the other.
#[test]
fn a_prohibition_on_a_prefix_and_a_silent_licence_on_a_suffix_of_one_length_refuse() {
    disallowed_at(
        "/ab",
        entries(&[
            (r#"url="/a*""#, PROHIBITS_AI_INPUT),
            (r#"url="/*b""#, SILENT_FREE),
        ]),
        "1",
        &[],
    );
    disallowed_at(
        "/ab",
        entries(&[
            (r#"url="/*b""#, SILENT_FREE),
            (r#"url="/a*""#, PROHIBITS_AI_INPUT),
        ]),
        "2",
        &[],
    );
}

/// An absolute scope takes part by its path, so the absolute root and `/*`
/// are one scope and are read as one entry in either order: the
/// prohibition refuses beside the silent licence, named by its position
/// across both, and the record names the first entry's `url`.
#[test]
fn a_prohibition_on_root_wildcard_and_a_silent_absolute_root_are_read_as_one_in_either_order() {
    let [wildcard_first, root_first] = either_order(
        (r#"url="/*""#, PROHIBITS_AI_INPUT),
        (r#"url="{origin}/""#, SILENT_FREE),
    );
    for crossing in disallowed_at("/page", wildcard_first, "1", &[]) {
        let licence = &crossing["payload"]["declarations"]["licences"][0];
        assert_eq!(licence["content"], "/*", "{licence}");
    }
    for crossing in disallowed_at("/page", root_first, "2", &[]) {
        let licence = &crossing["payload"]["declarations"]["licences"][0];
        let origin = crossing["payload"]["url"]
            .as_str()
            .and_then(|url| url.strip_suffix("page"))
            .expect("the page URL");
        assert_eq!(licence["content"], origin, "{licence}");
    }
}

/// `{origin}/` alone governs `/page`: the substituted origin is the scope
/// the record names, so the absolute scope in the test above is one that
/// matches the page.
#[test]
fn a_silent_free_licence_on_the_absolute_root_alone_is_delivered() {
    for crossing in delivered_at(
        "/page",
        ROBOTS,
        entries(&[(r#"url="{origin}/""#, SILENT_FREE)]),
    ) {
        let page = crossing["payload"]["url"].as_str().expect("the page");
        let root = page.strip_suffix("page").expect("the page under the root");
        let licence = &crossing["payload"]["declarations"]["licences"][0];
        assert_eq!(licence["content"], root, "{licence}");
    }
}

// ---------------------------------------------------------------------------
// EDG-159, EDG-158 and the owner's rule of 3 October 2026. Of the entries
// matching a page, each whose scope strictly contains another's is set aside
// whatever the lengths, an absolute scope taking part by its path, and the
// rest are read as one entry. A prohibition of AI input in any licence of
// that entry refuses the page, whatever another licence permits.

/// The licence each entry of a pair is given.
#[derive(Clone, Copy, Debug)]
enum Role {
    /// Prohibits AI input.
    Prohibits,
    /// Permits AI input by name at a price.
    Priced,
    /// Permits AI input by name, free.
    Free,
}

impl Role {
    fn licence(self) -> &'static str {
        match self {
            Self::Prohibits => PROHIBITS_AI_INPUT,
            Self::Priced => PRICED_OFFER,
            Self::Free => FREE_OFFER,
        }
    }
}

/// How every mode rules a page.
#[derive(Clone, Copy, Debug)]
enum Ruled {
    /// Refused on the licence's Disallow, which names the licence at this
    /// position across the entries read.
    Disallowed(usize),
    /// Refused on the payment of the priced offer.
    Priced,
    /// Delivered, under the offer at this position.
    Delivered(usize),
}

/// In every mode `page` under `documents` is ruled `ruled`, and the record's
/// `content` is `content`, with `{origin}` read as the publisher's origin.
/// A refusal is the record's `told`; a delivery carries none.
fn ruled_at(page: &str, documents: Documents, ruled: Ruled, content: &str) {
    for mode in MODES {
        let (result, crossing, pages) = fetch(page, ROBOTS, &documents, mode);
        let payload = &crossing["payload"];
        let declarations = &payload["declarations"];
        let licence = &declarations["licences"][0];
        let url = payload["url"].as_str().expect("the page URL");
        let origin = url.strip_suffix(page).expect("the page under its origin");
        let at = format!("{mode} {ruled:?}");
        assert_eq!(
            licence["content"],
            content.replace("{origin}", origin),
            "{at}: {licence}"
        );
        match ruled {
            Ruled::Delivered(offer) => {
                let delivered = result.unwrap_or_else(|error| panic!("{at}: {error}"));
                assert!(delivered.contains("the article"), "{at}: {delivered}");
                assert_eq!(pages, 1, "{at}");
                assert_eq!(crossing["event"], "crossing_mediated", "{at}");
                assert!(payload["told"].is_null(), "{at}: {payload}");
                assert_eq!(declarations["effective"]["ai-input"], "allow", "{at}");
                assert_eq!(licence["terms"]["offer"], offer, "{at}: {licence}");
                assert_eq!(licence["terms"]["payment"]["kind"], "free", "{at}");
            }
            Ruled::Disallowed(_) | Ruled::Priced => {
                let error = result.expect_err(&at);
                assert!(
                    error.starts_with("refused before the crossing: "),
                    "{at}: {error}"
                );
                assert_eq!(pages, 0, "{at}: the page was never asked for");
                assert_eq!(crossing["event"], "crossing_refused", "{at}");
                assert_eq!(payload["told"], error, "{at}: {payload}");
                if let Ruled::Disallowed(position) = ruled {
                    assert!(
                        error.contains("The source disallows AI input (in its RSL licence)."),
                        "{at}: {error}"
                    );
                    assert!(!error.contains("payment"), "{at}: {error}");
                    assert_eq!(declarations["effective"]["ai-input"], "disallow", "{at}");
                    assert!(licence["terms"].get("offer").is_none(), "{at}: {licence}");
                    let names = |statement: &Value| {
                        statement["category"] == "ai-input"
                            && statement["preference"] == "disallow"
                            && statement["detail"].as_str().is_some_and(|detail| {
                                detail.ends_with(&format!(
                                    "no licence permits usage ai-input (licence {position})"
                                )) || detail.contains(&format!(
                                    ": licence {position} prohibits usage ai-input"
                                ))
                            })
                    };
                    assert!(
                        licence["terms"]["statements"]
                            .as_array()
                            .is_some_and(|statements| statements.iter().any(names)),
                        "{at}: {licence}"
                    );
                } else {
                    assert!(error.contains(PRICED_REFUSAL), "{at}: {error}");
                    assert_eq!(licence["terms"]["payment"]["kind"], "use", "{at}");
                }
            }
        }
    }
}

/// `narrow` lies strictly within `broad`, so it governs `page` alone in
/// either order: its prohibition and price bind, and its free offer is
/// delivered whatever `broad` says.
fn narrower_governs_alone(narrow: &str, broad: &str, page: &str) {
    for (narrow_role, broad_role, ruled) in [
        (Role::Prohibits, Role::Free, Ruled::Disallowed(1)),
        (Role::Free, Role::Prohibits, Ruled::Delivered(1)),
        (Role::Priced, Role::Free, Ruled::Priced),
        (Role::Free, Role::Priced, Ruled::Delivered(1)),
    ] {
        let narrow_url = format!(r#"url="{narrow}""#);
        let broad_url = format!(r#"url="{broad}""#);
        for documents in either_order(
            (&narrow_url, narrow_role.licence()),
            (&broad_url, broad_role.licence()),
        ) {
            ruled_at(page, documents, ruled, narrow);
        }
    }
}

/// `one` and `other` both govern `page`, read as one entry in either
/// order, the first entry's `url` on the record: a prohibition in either
/// refuses beside a free offer in the other, and a free offer in either is
/// taken beside a price in the other.
fn read_as_one(one: &str, other: &str, page: &str) {
    for (one_role, other_role) in [
        (Role::Prohibits, Role::Free),
        (Role::Free, Role::Prohibits),
        (Role::Priced, Role::Free),
        (Role::Free, Role::Priced),
    ] {
        for (first, second) in [
            ((one, one_role), (other, other_role)),
            ((other, other_role), (one, one_role)),
        ] {
            let ruled = match (first.1, second.1) {
                (Role::Prohibits, _) => Ruled::Disallowed(1),
                (_, Role::Prohibits) => Ruled::Disallowed(2),
                (Role::Free, _) => Ruled::Delivered(1),
                (_, Role::Free) => Ruled::Delivered(2),
                _ => unreachable!("every row holds a prohibition or a free offer"),
            };
            let (first_url, second_url) = (
                format!(r#"url="{}""#, first.0),
                format!(r#"url="{}""#, second.0),
            );
            let documents = entries(&[
                (&first_url, first.1.licence()),
                (&second_url, second.1.licence()),
            ]);
            ruled_at(page, documents, ruled, first.0);
        }
    }
}

/// `/ab` lies within `/a*b` for `/abc`, though `/a*b` is the longer.
#[test]
fn a_shorter_scope_within_a_longer_wildcard_governs_alone_in_every_mode() {
    narrower_governs_alone("/ab", "/a*b", "/abc");
}

/// `/news/` lies within `/new*s/` for `/news/1`.
#[test]
fn a_literal_section_within_a_longer_wildcard_section_governs_alone_in_every_mode() {
    narrower_governs_alone("/news/", "/new*s/", "/news/1");
}

/// `/p` lies within `/*p` for `/p`.
#[test]
fn a_prefix_within_a_longer_suffix_pattern_governs_alone_in_every_mode() {
    narrower_governs_alone("/p", "/*p", "/p");
}

/// `/` and `/*` are one scope, read as one in either order.
#[test]
fn root_and_root_wildcard_are_read_as_one_in_every_mode() {
    read_as_one("/", "/*", "/page");
}

/// `/xy` and `/x*z` overlap for `/xyz` and neither lies within the other,
/// so they are read as one, though `/x*z` is the longer.
#[test]
fn overlapping_scopes_of_unequal_length_are_read_as_one_in_every_mode() {
    read_as_one("/xy", "/x*z", "/xyz");
}

/// One `<content>` with a licence that prohibits AI input and a sibling
/// that permits it by name, free: the prohibition refuses in either order.
#[test]
fn a_prohibition_beside_a_free_permit_by_name_in_one_content_refuses_in_every_mode() {
    for (licences, prohibiting) in [
        (format!("{PROHIBITS_AI_INPUT}{FREE_OFFER}"), 1),
        (format!("{FREE_OFFER}{PROHIBITS_AI_INPUT}"), 2),
    ] {
        ruled_at(
            PAGE,
            content(&licences),
            Ruled::Disallowed(prohibiting),
            "/",
        );
    }
}

/// The pairs the fix4 review found delivered with a free permit by name in
/// one entry beside a prohibition in an entry read with it: every one is
/// refused, either entry prohibiting, in either order.
#[test]
fn a_prohibition_in_an_entry_read_with_a_free_permit_by_name_refuses_in_every_mode() {
    for (one, other, page) in [
        ("/", "/*", "/page"),
        ("/p", "/p*", "/page"),
        ("/n", "/%6E", "/n/1"),
        ("/a*/x", "/*b/x", "/ab/x"),
        ("/a*/x", "/*b*x", "/ab/x"),
        ("/a*", "/*b", "/ab"),
        ("/ab", "/*c", "/abc"),
    ] {
        read_as_one(one, other, page);
    }
}

/// `/**`, `/*` and `/` are one scope: a prohibition on `/` refuses beside
/// free permits by name on the other two, wherever it is written.
#[test]
fn a_prohibition_on_root_beside_two_free_root_wildcards_refuses_in_every_mode() {
    let three = |first: (&str, &str), second: (&str, &str), third: (&str, &str)| {
        entries(&[first, second, third])
    };
    let root = (r#"url="/""#, PROHIBITS_AI_INPUT);
    let star = (r#"url="/*""#, FREE_OFFER);
    let stars = (r#"url="/**""#, FREE_OFFER);
    ruled_at("/page", three(root, star, stars), Ruled::Disallowed(1), "/");
    ruled_at(
        "/page",
        three(stars, root, star),
        Ruled::Disallowed(2),
        "/**",
    );
    ruled_at(
        "/page",
        three(stars, star, root),
        Ruled::Disallowed(3),
        "/**",
    );
}

/// A contained entry permitting by name beside a broader one prohibiting
/// is delivered: the broader entry is set aside, outside the scope that
/// governs the page.
#[test]
fn a_free_permit_on_a_contained_scope_beside_a_broader_prohibition_is_delivered() {
    for documents in either_order(
        (r#"url="/p""#, FREE_OFFER),
        (r#"url="/*""#, PROHIBITS_AI_INPUT),
    ) {
        ruled_at("/page", documents, Ruled::Delivered(1), "/p");
    }
}

/// The owner's example: `/blog/` permits AI input by name and `/*.pdf$`
/// prohibits it. For `/blog/a.pdf` neither lies within the other, so they
/// are read as one and the prohibition refuses, in either order and
/// whichever pattern is the longer; `/blog/a.html` is under `/blog/` alone
/// and is delivered.
#[test]
fn a_permit_on_a_section_and_a_prohibition_on_a_file_type_refuse_the_file_in_the_section() {
    for section in ["/blog/", "/blog/posts/"] {
        let permit = format!(r#"url="{section}""#);
        let permit = (permit.as_str(), FREE_OFFER);
        let prohibition = (r#"url="/*.pdf$""#, PROHIBITS_AI_INPUT);
        let file = format!("{section}a.pdf");
        ruled_at(
            &file,
            entries(&[permit, prohibition]),
            Ruled::Disallowed(2),
            section,
        );
        ruled_at(
            &file,
            entries(&[prohibition, permit]),
            Ruled::Disallowed(1),
            "/*.pdf$",
        );
        for documents in either_order(permit, prohibition) {
            ruled_at(
                &format!("{section}a.html"),
                documents,
                Ruled::Delivered(1),
                section,
            );
        }
    }
}

/// An absolute scope takes part by its path: the absolute root is one
/// scope with `/` and `/*`, and `{origin}/x` with `/x`, each pair read as
/// one in either order.
#[test]
fn an_absolute_scope_and_a_relative_entry_of_its_path_are_read_as_one_in_every_mode() {
    read_as_one("/*", "{origin}/", "/page");
    read_as_one("/", "{origin}/", "/page");
    read_as_one("/x", "{origin}/x", "/x/1");
}

/// `{origin}/x` lies within `/*` and `/`, and `/x` within the absolute
/// root: the narrower governs alone in either order.
#[test]
fn an_absolute_scope_and_a_relative_entry_one_within_the_other_govern_by_containment() {
    narrower_governs_alone("{origin}/x", "/*", "/x/1");
    narrower_governs_alone("{origin}/x", "/", "/x/1");
    narrower_governs_alone("/x", "{origin}/", "/x/1");
}

/// Within one licence a specific permit stands over a blanket `prohibits
/// all` (RSL 1.0 §3.1.1), so such a licence alone is delivered. Beside a
/// second licence that prohibits AI input it is refused, in either order.
#[test]
fn a_permit_over_a_blanket_prohibition_stands_alone_and_falls_beside_a_prohibition() {
    const SPECIFIC_OVER_BLANKET: &str = r#"<license><permits type="usage">ai-input</permits><prohibits type="usage">all</prohibits><payment type="free"/></license>"#;
    ruled_at(
        PAGE,
        content(SPECIFIC_OVER_BLANKET),
        Ruled::Delivered(1),
        "/",
    );
    for (licences, prohibiting) in [
        (format!("{SPECIFIC_OVER_BLANKET}{PROHIBITS_AI_INPUT}"), 2),
        (format!("{PROHIBITS_AI_INPUT}{SPECIFIC_OVER_BLANKET}"), 1),
    ] {
        ruled_at(
            PAGE,
            content(&licences),
            Ruled::Disallowed(prohibiting),
            "/",
        );
    }
}

/// A licence restricted to a class of user or a region that prohibits AI
/// input counts: the edge cannot show it is outside the restriction, so it
/// refuses beside a free permit by name, in either order.
#[test]
fn a_restricted_licence_that_prohibits_ai_input_refuses_beside_a_free_permit_by_name() {
    for restriction in [
        r#"<permits type="user">education</permits>"#,
        r#"<prohibits type="geo">GB</prohibits>"#,
    ] {
        let restricted = format!(
            r#"<license>{restriction}<prohibits type="usage">ai-input</prohibits></license>"#
        );
        for (licences, prohibiting) in [
            (format!("{restricted}{FREE_OFFER}"), 1),
            (format!("{FREE_OFFER}{restricted}"), 2),
        ] {
            ruled_at(
                PAGE,
                content(&licences),
                Ruled::Disallowed(prohibiting),
                "/",
            );
        }
    }
}

// ---------------------------------------------------------------------------
/// A free offer by name with a reporting demand this edge cannot meet: a
/// telemetry receiver, which a session with none configured does not have.
const REPORTING_FREE_OFFER: &str = r#"<license><permits type="usage">ai-input</permits><payment type="free"/><reporting type="telemetry" profile="https://contenttelemetry.org/profiles/spur" endpoint="https://telemetry.example.com/v1/events"><![CDATA[{"conformance_level":"grounding","privacy_level":"minimal"}]]></reporting></license>"#;

/// A recorded dependence, not a rule: the offer choice ranks payment, then
/// document order, and does not read reporting demands, so of two free
/// offers that differ only in a demand this edge cannot meet the first
/// written is taken. `/xy` and `/x*z` overlap and are read as one for
/// `/xyz`; with the demand first the page is refused for the demand, with
/// it second the page is delivered under the other offer. Whether reporting
/// enters the choice is the owner's to decide; this pins the order
/// dependence until then.
#[test]
fn two_free_offers_differing_only_in_a_reporting_demand_are_ruled_by_document_order_a_recorded_dependence()
 {
    let demanding = (r#"url="/xy""#, REPORTING_FREE_OFFER);
    let plain = (r#"url="/x*z""#, FREE_OFFER);
    for mode in MODES {
        let (result, crossing, pages) = fetch("/xyz", ROBOTS, &entries(&[demanding, plain]), mode);
        let error = result.expect_err(mode);
        assert!(
            error.contains("requires telemetry reporting") && error.contains("cannot be met"),
            "{mode}: {error}"
        );
        assert_eq!(pages, 0, "{mode}: the page was never asked for");
        let payload = &crossing["payload"];
        assert_eq!(payload["told"], error, "{mode}: {payload}");
        let licence = &payload["declarations"]["licences"][0];
        assert_eq!(licence["content"], "/xy", "{mode}: {licence}");
        assert_eq!(licence["terms"]["offer"], 1, "{mode}: {licence}");
    }
    for crossing in delivered_at("/xyz", ROBOTS, entries(&[plain, demanding])) {
        let licence = &crossing["payload"]["declarations"]["licences"][0];
        assert_eq!(licence["content"], "/x*z", "{licence}");
        assert_eq!(licence["terms"]["offer"], 1, "{licence}");
        assert!(licence["terms"].get("reporting").is_none(), "{licence}");
    }
}

// ---------------------------------------------------------------------------
// Selection is bounded by the work its containment tests do
// (`declarations::SELECTION_BUDGET`), not by a count of entries. Past the
// budget the licence is unread for the page, which refuses in every mode.

/// A page path long enough that each of `count` entries can be a distinct
/// prefix of it.
fn long_page(count: usize) -> String {
    format!("/{}", "7".repeat(count + 1))
}

/// `count` entries, each a distinct prefix of [`long_page`], from the
/// shortest, each with `licence`: each lies within the one before it.
fn prefixes(count: usize, licence: &str) -> Documents {
    let page = long_page(count);
    let entries: Vec<(String, &str)> = (0..count)
        .map(|n| (format!(r#"url="{}""#, &page[..n + 2]), licence))
        .collect();
    let entries: Vec<(&str, &str)> = entries
        .iter()
        .map(|(url, licence)| (url.as_str(), *licence))
        .collect();
    self::entries(&entries)
}

/// A page of random letters and `count` entries `/*w*`, each `w` a distinct
/// `len`-letter window of it, each with `licence` but the one at `odd_one`,
/// which has `odd`. Every entry matches the page and none lies within
/// another, so selection makes every containment test and every entry
/// governs.
fn windows(
    count: usize,
    len: usize,
    licence: &str,
    odd: Option<(usize, &str)>,
) -> (String, Documents) {
    let mut seed: u64 = 7;
    let page: String = (0..count + len)
        .map(|_| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            char::from(b'a' + u8::try_from((seed >> 33) % 26).expect("a letter"))
        })
        .collect();
    let urls: Vec<String> = (0..count)
        .map(|at| format!(r#"url="/*{}*""#, &page[at..at + len]))
        .collect();
    let entries: Vec<(&str, &str)> = urls
        .iter()
        .enumerate()
        .map(|(at, url)| {
            let licence = match odd {
                Some((odd_at, odd)) if odd_at == at => odd,
                _ => licence,
            };
            (url.as_str(), licence)
        })
        .collect();
    (format!("/{page}"), self::entries(&entries))
}

/// The work selection counts for [`windows`]`(count, len, ..)`, as
/// `declarations::selection_work` counts it: each form read once, and each
/// ordered pair of forms compared, the inner's general target and the
/// outer's form, plus 64 for the test.
fn windows_work(count: usize, len: usize) -> u64 {
    let (count, len) = (count as u64, len as u64);
    let (form, general) = (len + 3, len + 4);
    count * form + count * (count - 1) * (form + general + 64)
}

const OVER_BUDGET: &str = "choosing among the <content> entries that match this page takes \
                           more work than this edge does for one page";

/// In every mode `page` is refused as naming an unread licence, the agent
/// is told why in the edge's own words and nothing of the document (its
/// scopes, its terms or the page path the scopes are cut from), the
/// agent's sentence is the record's `told`, and the record holds the unread
/// licence and the reason. Nothing in the document is ruled.
fn unread_over_budget_at(page: &str, documents: Documents) {
    let fragment = &page[1..9];
    for mode in MODES {
        let (result, crossing, pages) = fetch(page, ROBOTS, &documents, mode);
        let error = result.expect_err(mode);
        assert!(
            error.starts_with("refused before the crossing: "),
            "{mode}: {error}"
        );
        assert!(
            error.contains(&format!(
                "The source names a licence its robots.txt names, which could not be read \
                 ({OVER_BUDGET}), so its terms, including any reporting demand, are unknown"
            )),
            "{mode}: {error}"
        );
        for withheld in [fragment, "<license", "free", "disallows", "payment"] {
            assert!(!error.contains(withheld), "{mode}: {withheld}: {error}");
        }
        assert_eq!(pages, 0, "{mode}: the page was never asked for");
        let payload = &crossing["payload"];
        assert_eq!(crossing["event"], "crossing_refused", "{mode}");
        assert_eq!(payload["told"], error, "{mode}: {payload}");
        assert!(
            payload["refusal"].as_str().is_some_and(|refusal| {
                refusal.contains(&format!("which could not be read ({OVER_BUDGET})"))
            }),
            "{mode}: {payload}"
        );
        let declarations = &payload["declarations"];
        let licence = &declarations["licences"][0];
        assert_eq!(licence["unread"], true, "{mode}: {licence}");
        assert_eq!(licence["unavailable"], OVER_BUDGET, "{mode}: {licence}");
        assert!(licence.get("content").is_none(), "{mode}: {licence}");
        assert!(licence.get("terms").is_none(), "{mode}: {licence}");
        assert!(licence.get("selection").is_none(), "{mode}: {licence}");
        assert_ne!(
            declarations["effective"]["ai-input"], "disallow",
            "{mode}: {declarations}"
        );
    }
}

/// 200 entries of 2,000-octet forms, all matching the page and none within
/// another, each a free licence silent on usage: their selection is past
/// the budget, so the licence is unread and the page refused, where any
/// selection would deliver it.
#[test]
fn a_licence_whose_selection_is_past_the_budget_is_unread_in_every_mode() {
    assert!(windows_work(200, 2_000) > crate::declarations::SELECTION_BUDGET);
    let (page, documents) = windows(200, 2_000, SILENT_FREE, None);
    unread_over_budget_at(&page, documents);
}

/// The floor: 256 matching entries of 64-octet forms, none within another,
/// are within the budget. All 256 govern, read as one entry, and the first
/// free offer is taken.
#[test]
fn two_hundred_and_fifty_six_matching_entries_of_64_octets_are_read_in_every_mode() {
    assert!(windows_work(256, 61) <= crate::declarations::SELECTION_BUDGET);
    let (page, documents) = windows(256, 61, SILENT_FREE, None);
    for crossing in delivered_at(&page, ROBOTS, documents) {
        let licence = &crossing["payload"]["declarations"]["licences"][0];
        assert_eq!(
            licence["content"],
            format!("/*{}*", &page[1..62]),
            "{licence}"
        );
        assert_eq!(licence["terms"]["offer"], 1, "{licence}");
    }
}

/// 257 nested prefixes of the page, which the count of 256 left unread,
/// are within the budget: the longest governs alone and the page is
/// delivered.
#[test]
fn more_than_256_matching_entries_within_the_budget_are_read_in_every_mode() {
    let page = long_page(257);
    for crossing in delivered_at(&page, ROBOTS, prefixes(257, SILENT_FREE)) {
        let licence = &crossing["payload"]["declarations"]["licences"][0];
        assert_eq!(licence["content"], page[..258], "{licence}");
        assert_eq!(licence["terms"]["offer"], 1, "{licence}");
    }
}

/// Entries that do not match the page do no selection work: of 300 literal
/// paths, the page matches `/p`, which governs over `/*` for `/page`.
#[test]
fn entries_that_do_not_match_the_page_are_not_counted() {
    let others: Vec<String> = (0..298).map(|n| format!(r#"url="/x{n}""#)).collect();
    for (first, second) in [
        ((r#"url="/p""#, PRICED_OFFER), (r#"url="/*""#, FREE_OFFER)),
        ((r#"url="/*""#, FREE_OFFER), (r#"url="/p""#, PRICED_OFFER)),
    ] {
        let entries: Vec<(&str, &str)> = others
            .iter()
            .map(|url| (url.as_str(), FREE_OFFER))
            .chain([first, second])
            .collect();
        priced_alone_at("/page", self::entries(&entries), "/p");
    }
}

/// One entry prohibits AI input and the rest permit it by name, first or
/// last in the document. Past the budget the licence is unread, so the page
/// is neither delivered nor refused on the Disallow. Within it every entry
/// governs, and the prohibition refuses.
#[test]
fn a_prohibition_among_matching_entries_is_ruled_only_within_the_budget() {
    for at in [0, 199] {
        let (page, documents) = windows(200, 2_000, FREE_OFFER, Some((at, PROHIBITS_AI_INPUT)));
        unread_over_budget_at(&page, documents);
    }
    for at in [0, 255] {
        let (page, documents) = windows(256, 61, FREE_OFFER, Some((at, PROHIBITS_AI_INPUT)));
        ruled_at(
            &page,
            documents,
            Ruled::Disallowed(at + 1),
            &format!("/*{}*", &page[1..62]),
        );
    }
}

/// A publisher whose `robots.txt` names `/license.xml`, whose final page
/// names `/link.xml` in a `Link` header, both free licences silent on
/// usage, and whose `/page` answers each request with `next` for the
/// request's count: `Some(path)` redirects there, `None` is the page.
fn redirecting_publisher(
    next: impl Fn(&str, usize) -> Option<String> + Send + Sync + 'static,
) -> commonmeasure_http::ServerHandle {
    let requests = std::sync::atomic::AtomicUsize::new(0);
    let document = format!(
        r#"<rsl xmlns="https://rslstandard.org/rsl"><content url="/">{SILENT_FREE}</content></rsl>"#
    );
    bind_local()
        .spawn(move |request| {
            let target = request.target.as_str();
            match target {
                "/robots.txt" => Response::text(200, ROBOTS),
                "/license.xml" | "/link.xml" => Response::new(200, document.clone().into_bytes()),
                _ if target.starts_with("/.well-known/") => Response::text(404, "none"),
                _ => {
                    let count = requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    match next(target, count) {
                        Some(location) => {
                            let mut response = Response::new(302, Vec::new());
                            response.headers.set("Location", &location);
                            response
                        }
                        None => {
                            let mut response = Response::text(200, "the article");
                            response.headers.set(
                                "Link",
                                r#"</link.xml>; rel="license"; type="application/rsl+xml""#,
                            );
                            response
                        }
                    }
                }
            }
        })
        .expect("spawn")
}

/// How many times a delivered crossing of `/page` at `site` selects a
/// `<content>` entry, and the licences its record holds.
fn selections_delivering(site: &commonmeasure_http::ServerHandle) -> (usize, Vec<Value>) {
    let (home, mut server) =
        server(&json!({"policy_mode": "strict", "allow_private_hosts": true}).to_string());
    crate::declarations::SELECTIONS.with(|selections| selections.set(0));
    let delivered = server
        .tool_fetch(&json!({"url": format!("{}/page", site.url())}))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(delivered.to_string().contains("the article"), "{delivered}");
    let selections = crate::declarations::SELECTIONS.with(std::cell::Cell::get);
    let recorded = crossings(home.path());
    let licences = recorded[0]["payload"]["declarations"]["licences"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    (selections, licences)
}

/// A crossing selects a licence body for a page once. Five redirect hops
/// back to the page itself read the `robots.txt` licence for the same page
/// six times and select once; the `Link` licence of the final response is
/// another body and selects once more. Five hops to five other pages are
/// six pages, each selected once, and the `Link` licence once.
#[test]
fn a_crossing_selects_a_licence_body_for_a_page_once_across_its_hops() {
    let mut site = redirecting_publisher(|_, count| (count < 5).then(|| "/page".to_owned()));
    let (selections, licences) = selections_delivering(&site);
    site.stop();
    assert_eq!(selections, 2, "{licences:?}");
    assert!(
        licences
            .iter()
            .any(|licence| licence["mechanism"] == "link-header"),
        "{licences:?}"
    );

    let mut site = redirecting_publisher(|target, _| {
        let hop: usize = match target {
            "/page" => 0,
            other => other.strip_prefix("/hop").and_then(|n| n.parse().ok())?,
        };
        (hop < 5).then(|| format!("/hop{}", hop + 1))
    });
    let (selections, licences) = selections_delivering(&site);
    site.stop();
    assert_eq!(selections, 7, "{licences:?}");
}
