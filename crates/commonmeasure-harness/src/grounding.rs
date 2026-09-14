//! What a tool result put into the model's context, and what may be claimed
//! about it.
//!
//! Raided from AI Content Diet's `diet-capture::grounding`, which had already
//! paid for the details below.

use serde_json::Value;

/// Per-tool grounding policy. The central semantic guard: a returned tool
/// result means content was *available in model context*, never that the answer
/// relied on it.
///
/// - `WebFetch` — retrieved **and** grounded: the page text entered context.
/// - `WebSearch` — retrieved only. The model saw titles and snippets, not the
///   pages, so claiming grounding would overstate it.
/// - `Mcp` — retrieved only. An MCP result can carry external content, but the
///   protocol has no standard marker tying a returned URL to the bytes that
///   entered context.
/// - `SelfMediated` — one of our own MCP tools. The mediated path already
///   recorded this crossing properly, before it happened; observing it again
///   would double it, and the second copy would be the weaker grade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    WebFetch,
    WebSearch,
    Mcp,
    SelfMediated,
    Other,
}

/// How this plugin's own MCP server appears in a host's tool namespace.
/// There are two spellings, and both are ours: a server declared
/// directly in `.mcp.json` is exposed as `mcp__<server>__<tool>`, while the
/// same server installed through the plugin is namespaced by Claude Code as
/// `mcp__plugin_<plugin>_<server>__<tool>` — plugin and server are both
/// named `commonmeasure`. The production install is the plugin, so recognising
/// only the direct spelling excluded nothing that actually runs.
pub const SELF_TOOL_PREFIXES: &[&str] = &[
    "mcp__commonmeasure__",
    "mcp__plugin_commonmeasure_commonmeasure__",
    // Sessions recorded under the earlier plugin name carry these prefixes.
    "mcp__contextops__",
    "mcp__plugin_contextops_contextops__",
];

/// The mediated tools by their own names: how Pi registers them, and what
/// follows Cursor's `MCP:` prefix.
pub const OWN_TOOL_NAMES: [&str; 3] = ["context_fetch", "context_search", "context_status"];

impl ToolKind {
    pub fn classify(tool_name: &str) -> Self {
        match tool_name {
            "WebFetch" => ToolKind::WebFetch,
            "WebSearch" => ToolKind::WebSearch,
            name if OWN_TOOL_NAMES.contains(&name) => ToolKind::SelfMediated,
            // A host that names an MCP tool by its server and tool under
            // any spelling (`MCP:context_fetch`, `commonmeasure/context_fetch`,
            // `commonmeasure_context_fetch`) still ends in our tool's name
            // after a separator; the mediated server recorded that call
            // already. A third-party tool that happens to share one of the
            // three names is excluded with it, and the exclusion says so.
            name if ends_with_own_tool(name) => ToolKind::SelfMediated,
            // Cursor names an MCP tool `MCP:<tool>` in its hook payloads,
            // without the server.
            name if name.starts_with("MCP:") => ToolKind::Mcp,
            // Our own tools are recognised before the general MCP case: a
            // mediated crossing is recorded by the server that carried it, and
            // recording it again from the hook would put the same fetch in the
            // log twice under two different modes. The observed copy would also
            // scrape every URL out of the page body we just returned.
            name if SELF_TOOL_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix)) =>
            {
                ToolKind::SelfMediated
            }
            // Claude Code exposes MCP tools as `mcp__<server>__<tool>`, and
            // other MCP-capable runtimes use the same namespaced convention, so
            // the prefix is the client-agnostic signal that a result came from
            // an MCP server rather than a built-in tool.
            name if name.starts_with("mcp__") => ToolKind::Mcp,
            _ => ToolKind::Other,
        }
    }
}

/// Whether a tool name ends in one of our tools' names after `:`, `_` or
/// `/`, the separators hosts put between a server and its tool.
fn ends_with_own_tool(name: &str) -> bool {
    OWN_TOOL_NAMES.iter().any(|tool| {
        name.strip_suffix(tool).is_some_and(|prefix| {
            prefix
                .chars()
                .last()
                .is_some_and(|separator| matches!(separator, ':' | '_' | '/'))
        })
    })
}

/// The host a URL names, or empty when it names none.
///
/// URL semantics live here, so every path that records a crossing reads the
/// host the same way.
pub fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The text a tool result put into model context.
///
/// The hash must cover the bytes as ingested, not a transport wrapper. Claude
/// Code delivers a `WebFetch` result to the hook as
/// `{"bytes": …, "code": …, "result": "<text>", …}` while the model's context
/// carries only `result`; hashing the serialised wrapper makes the stored hash
/// unmatchable against the transcript text, which breaks any hash-anchored
/// citation check. Verified against a live payload: the `result` string is
/// byte-identical to the transcript's tool-result text.
pub fn ingested_text(response: &Value) -> String {
    match response {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        Value::Object(fields) => {
            if let Some(result) = fields.get("result").and_then(Value::as_str) {
                return result.to_owned();
            }
            if let Some(blocks) = fields.get("content").and_then(Value::as_array) {
                let texts: Vec<&str> = blocks
                    .iter()
                    .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect();
                if !texts.is_empty() {
                    return texts.join("");
                }
            }
            response.to_string()
        }
        other => other.to_string(),
    }
}

/// Rough token estimate at roughly four characters per token.
///
/// Named an estimate everywhere it is recorded, for the same reason the run
/// path names its whitespace-word basis: a number labelled "tokens" that no
/// tokeniser produced is borrowed precision.
pub const TOKEN_BASIS: &str = "characters/4";

pub fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// Every distinct `http(s)` URL anywhere in a JSON value. Used to pull result
/// links out of a `WebSearch` or MCP response whose exact shape varies.
pub fn extract_urls(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    walk(value, &mut found);
    found.sort();
    found.dedup();
    found
}

fn walk(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => extract_urls_from_text(text, out),
        Value::Array(items) => items.iter().for_each(|item| walk(item, out)),
        Value::Object(fields) => fields.values().for_each(|field| walk(field, out)),
        _ => {}
    }
}

fn extract_urls_from_text(text: &str, out: &mut Vec<String>) {
    let mut rest = text;
    // The *earliest* of the two schemes, not the first one that matches.
    // `find("http://").or_else(|| find("https://"))` looks equivalent and is
    // not: on "see https://a and http://b" it returns the offset of the plain
    // http URL, and the scan then restarts past it, silently dropping every
    // https URL that preceded it. Inherited from the raided implementation,
    // where the tests happened to use one scheme at a time.
    while let Some(start) = [rest.find("http://"), rest.find("https://")]
        .into_iter()
        .flatten()
        .min()
    {
        let tail = &rest[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '"' | '<' | '>' | '|'))
            .unwrap_or(tail.len());
        let candidate = tail[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
        if candidate.len() > "https://".len() {
            out.push(candidate.to_owned());
        }
        rest = &tail[end.max(1)..];
    }
}

/// Whether a URL may be recorded at all.
///
/// A privacy floor, not a policy engine: an operator's localhost, private
/// network and `file://` traffic is their own business and never enters an
/// evidence record. Job-level source policy is a separate thing, decided by
/// `commonmeasure_runtime::policy` against the job's declared constraints.
pub fn recordable(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    !commonmeasure_types::address::is_private(&parsed)
}

/// The floor with the operator's named exceptions applied.
///
/// The floor is a default, not a ceiling: an operator whose most valuable
/// supply is internal — a RAG corpus, an intranet, a document store — may name
/// specific prefixes in `policy.json` whose crossings are recorded like public
/// traffic. The lowering is per-prefix and explicit, never by omission:
/// naming one corpus is not consent for localhost at large, so anything the
/// list does not match stays out exactly as [`recordable`] decides.
pub fn recordable_under(raw: &str, internal_prefixes: &[String]) -> bool {
    recordable(raw) || matches_internal_prefix(raw, internal_prefixes)
}

/// Whether the operator has named this URL's prefix as recordable internal
/// supply.
///
/// Judged on the parsed URL rather than the spelling that arrived, for the
/// same reason the privacy floor judges parsed addresses: `https://host:443/p`,
/// `HTTPS://HOST/p` and `https://host/./p` are one page under three spellings,
/// and a verbatim comparison stamps two of them public. That is not a missed
/// record but an egress decision — the `internal` marking is what keeps a
/// named corpus on the operator's machine — so the spelling must not be able
/// to change the answer. Both sides are normalised, because the operator's
/// prefix is a spelling too.
pub fn matches_internal_prefix(raw: &str, internal_prefixes: &[String]) -> bool {
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    internal_prefixes
        .iter()
        .filter_map(|prefix| url::Url::parse(prefix).ok())
        .any(|prefix| parsed.as_str().starts_with(prefix.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn webfetch_hashes_the_text_that_entered_context_not_the_wrapper() {
        let response = json!({
            "bytes": 559, "code": 200, "codeText": "OK",
            "result": "The page title is \"Example Domain\".",
            "durationMs": 1486, "url": "https://example.com/"
        });
        assert_eq!(
            ingested_text(&response),
            "The page title is \"Example Domain\"."
        );
    }

    #[test]
    fn mcp_content_blocks_join_their_text() {
        let response = json!({"content": [
            {"type": "text", "text": "first "},
            {"type": "image", "data": "…"},
            {"type": "text", "text": "second"}
        ]});
        assert_eq!(ingested_text(&response), "first second");
    }

    #[test]
    fn urls_are_found_inside_nested_text() {
        let response = json!({"content": [{
            "type": "text",
            "text": "Read https://docs.rs/serde and then https://tokio.rs/."
        }]});
        assert_eq!(
            extract_urls(&response),
            vec!["https://docs.rs/serde", "https://tokio.rs/"]
        );
    }

    /// The regression that found the inherited scanner bug: a plain-http URL
    /// later in the text made every earlier https URL disappear.
    #[test]
    fn a_later_plain_http_url_does_not_swallow_earlier_https_ones() {
        let response = json!({"result":
            "Try https://www.ofgem.gov.uk/a and https://www.gov.uk/b and http://example.org/c"});
        assert_eq!(
            extract_urls(&response),
            vec![
                "http://example.org/c",
                "https://www.gov.uk/b",
                "https://www.ofgem.gov.uk/a",
            ]
        );
    }

    #[test]
    fn private_and_local_addresses_are_never_recorded() {
        for private in [
            "http://localhost:3000/x",
            "http://127.0.0.1/x",
            "https://build.internal/x",
            "file:///home/alex/notes.md",
            "http://192.168.1.4/x",
            "http://172.20.0.1/x",
            // The probes the 2 Aug gate recorded through the old string
            // prefixes: loopback is a /8, IPv6 loopback arrives bracketed,
            // and the unspecified address is nobody's public host.
            "http://127.0.0.2/secret",
            "http://127.255.255.254/x",
            "http://[::1]:8080/admin",
            "http://0.0.0.0/x",
            "http://[::]/x",
            // Link-local, IPv4 and IPv6, and IPv6 unique-local.
            "http://169.254.1.1/x",
            "http://[fe80::1]/x",
            "http://[fd12:3456:789a::1]/x",
            // An IPv4 private address does not turn public by being written
            // as an IPv6-mapped address.
            "http://[::ffff:192.168.1.4]/x",
        ] {
            assert!(!recordable(private), "{private} should not be recordable");
        }
        assert!(recordable("https://www.ofgem.gov.uk/energy-price-cap"));
        // 172.32 is outside the private range and is ordinary public space.
        assert!(recordable("http://172.32.0.1/x"));
    }

    /// The floor guards addresses, not spellings: a public domain that merely
    /// starts with digits is not a private network, and dropping it would
    /// silently thin the record of real public supply.
    #[test]
    fn public_domains_that_look_like_private_prefixes_are_recordable() {
        for public in [
            "https://10.example.com/x",
            "https://192.168.example.org/x",
            "https://127.0.0.1.nip.example/x",
        ] {
            assert!(recordable(public), "{public} should be recordable");
        }
    }

    /// The floor lowers only where the operator named a prefix, and a named
    /// prefix is not consent for the private space around it.
    #[test]
    fn a_named_prefix_is_recordable_and_its_neighbours_are_not() {
        let prefixes = vec![
            "https://rag.corp.internal/".to_owned(),
            "file:///corp/kb/".to_owned(),
        ];
        assert!(recordable_under(
            "https://rag.corp.internal/doc/42",
            &prefixes
        ));
        assert!(recordable_under("file:///corp/kb/handbook.md", &prefixes));
        // Same private space, different prefix: still out.
        assert!(!recordable_under("https://wiki.corp.internal/x", &prefixes));
        assert!(!recordable_under("http://localhost:3000/x", &prefixes));
        assert!(!recordable_under("file:///home/alex/notes.md", &prefixes));
        // Public traffic is unaffected by the list being present.
        assert!(recordable_under("https://www.gov.uk/x", &prefixes));
    }

    /// The trailing slash the policy loader enforces is what makes a prefix a
    /// boundary: a host that merely starts with the named host, a userinfo
    /// trick reaching another host, and a longer final IP octet all fail the
    /// verbatim match at the "/" the operator wrote.
    #[test]
    fn a_well_formed_prefix_stops_at_the_component_the_operator_named() {
        let prefixes = vec![
            "https://rag.corp.internal/".to_owned(),
            "http://10.0.0.9/".to_owned(),
        ];
        assert!(!recordable_under(
            "https://rag.corp.internal.evil.internal/x",
            &prefixes
        ));
        assert!(!recordable_under(
            "https://rag.corp.internal@wiki.corp.internal/secret",
            &prefixes
        ));
        assert!(!recordable_under("http://10.0.0.99/x", &prefixes));
        assert!(recordable_under("http://10.0.0.9/x", &prefixes));
    }

    /// Live through the hook and the relay, `https://intranet.example.com:443/…`,
    /// `HTTPS://INTRANET.EXAMPLE.COM/…` and `https://intranet.example.com/./…`
    /// were each stamped `internal=false` and delivered across the egress
    /// boundary while the literal spelling of the same page was correctly held
    /// back. The address floor beside this one already judged all three alike.
    #[test]
    fn equivalent_spellings_of_a_named_url_match_the_prefix_that_named_it() {
        let prefixes = vec!["https://intranet.example.com/private/".to_owned()];
        for spelling in [
            "https://intranet.example.com/private/handbook",
            "https://intranet.example.com:443/private/handbook",
            "HTTPS://INTRANET.EXAMPLE.COM/private/handbook",
            "https://intranet.example.com/./private/handbook",
            "https://intranet.example.com/other/../private/handbook",
        ] {
            assert!(
                matches_internal_prefix(spelling, &prefixes),
                "{spelling} names the page the operator named"
            );
        }
        // The prefix is a spelling too, and normalising one side only would
        // leave the same gap facing the other way.
        let spelled = vec!["HTTPS://Intranet.Example.com:443/private/".to_owned()];
        assert!(matches_internal_prefix(
            "https://intranet.example.com/private/handbook",
            &spelled
        ));
    }

    /// Normalisation must not widen the prefix. A dot segment cannot climb out
    /// of the named path, and a neighbouring host is still a different host
    /// however it is spelled.
    #[test]
    fn normalisation_does_not_admit_anything_the_prefix_did_not_name() {
        let prefixes = vec!["https://intranet.example.com/private/".to_owned()];
        for outside in [
            "https://intranet.example.com/private/../public/handbook",
            "https://intranet.example.com:8443/private/handbook",
            "HTTPS://INTRANET.EXAMPLE.COM.EVIL.TEST/private/handbook",
            "http://intranet.example.com/private/handbook",
        ] {
            assert!(
                !matches_internal_prefix(outside, &prefixes),
                "{outside} is not what the operator named"
            );
        }
    }

    /// An empty list is the default, and the default is exactly the floor.
    #[test]
    fn an_empty_prefix_list_changes_nothing() {
        for url in [
            "http://localhost:3000/x",
            "https://build.internal/x",
            "file:///home/alex/notes.md",
        ] {
            assert_eq!(recordable_under(url, &[]), recordable(url));
        }
    }

    /// Our own tools are not somebody else's MCP server.
    #[test]
    fn our_own_mcp_tools_are_recognised_as_already_mediated() {
        assert_eq!(
            ToolKind::classify("mcp__commonmeasure__context_fetch"),
            ToolKind::SelfMediated
        );
        assert_eq!(ToolKind::classify("mcp__exa__search"), ToolKind::Mcp);
    }
}
