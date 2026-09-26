//! Observed capture from browser answer surfaces: ChatGPT on the web, Google
//! AI Overviews and Bing Copilot Search.
//!
//! The model's own web search on these surfaces never passes through any tool
//! this product can offer, so the only evidence is what the page shows. The
//! browser extension in `browser/` reads it, one answer at a time, and sends
//! the binary one message per answer over Chrome native messaging (or one
//! message on stdin to `hook`); this module turns a message into crossings.
//!
//! What a message can support is narrow. A source the answer showed was seen by
//! the model as a search result or as a citation, so it is an observed
//! crossing, retrieved and never grounded: no surface exposes the page text the
//! model read, so nothing is hashed and no token count is estimated. The
//! session-evidence contract has no field for "cited", so a citation is
//! recorded as the retrieved crossing it also is, and the citation itself is
//! not recorded. No surface runs in a working directory, so no browser crossing
//! carries a `cwd`, and no crossing records a tool, because a page shows
//! sources and not the tool call that found each one.

use chrono::Utc;
use commonmeasure_types::LicenceState;
use serde::Deserialize;

use crate::grounding;
use crate::hook::HostSurface;
use crate::session::{Crossing, CrossingMode, SessionLog, safe_session};

/// One answer's sources from one surface, as the extension sends it
/// (`docs/contracts/host-integration.md` §2).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BrowserAnswer {
    /// `chatgpt-web`, `google-ai-overview` or `bing-copilot-search`.
    #[serde(default)]
    pub host: Option<String>,
    /// The surface's own session identity where it has one: ChatGPT's
    /// conversation id. Google and Bing give none.
    #[serde(default)]
    pub session: Option<String>,
    /// Every source the answer showed, search results and citations alike.
    #[serde(default)]
    pub retrieved: Vec<String>,
    /// The sources bound to the answer. Each is also retrieved; a citation
    /// missing from `retrieved` is still recorded as retrieved.
    #[serde(default)]
    pub cited: Vec<String>,
}

/// What recording one answer did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub session_id: String,
    pub crossings: usize,
}

impl BrowserAnswer {
    /// The surface this answer is recorded under: the message's own `host`
    /// where it names one, which must agree with the one the caller was told.
    pub fn surface(&self, told: Option<HostSurface>) -> Result<HostSurface, String> {
        let named = match self.host.as_deref() {
            Some(name) => Some(
                HostSurface::parse(name)
                    .filter(|surface| surface.is_browser())
                    .ok_or_else(|| format!("{name:?} is not a browser surface"))?,
            ),
            None => None,
        };
        match (named, told) {
            (Some(named), Some(told)) if named != told => Err(format!(
                "the message names {} and the command was told {}",
                named.id(),
                told.id()
            )),
            (Some(surface), _) | (None, Some(surface)) if surface.is_browser() => Ok(surface),
            (None, Some(surface)) => Err(format!("{} is not a browser surface", surface.id())),
            _ => Err("the message names no host".to_owned()),
        }
    }

    /// The session this answer belongs to: the surface's own identity, or
    /// `fallback` when the surface gave none. A page controls what the
    /// extension reads the identity from, so an identifier that is not a
    /// plain name under the rule every session log uses ([`safe_session`]) is
    /// refused rather than cleaned.
    pub fn session_id(&self, fallback: impl FnOnce() -> String) -> Result<String, String> {
        match self.session.as_deref() {
            Some(raw) => safe_session(raw)
                .map(str::to_owned)
                .ok_or_else(|| format!("session {raw:?} is not a plain identifier")),
            None => Ok(fallback()),
        }
    }
}

/// The observed crossings one answer evidences: one per distinct source,
/// retrieved and not grounded, under the privacy floor and the operator's
/// named internal prefixes as every observed crossing is.
pub fn capture(
    answer: &BrowserAnswer,
    surface: HostSurface,
    session_id: &str,
    internal_prefixes: &[String],
) -> Vec<Crossing> {
    let mut urls: Vec<&str> = answer
        .retrieved
        .iter()
        .chain(&answer.cited)
        .map(String::as_str)
        // A page link can be any scheme; only a web address is a source.
        .filter(|url| {
            url::Url::parse(url).is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
        })
        .filter(|url| grounding::recordable_under(url, internal_prefixes))
        .collect();
    urls.sort_unstable();
    urls.dedup();
    urls.into_iter()
        .map(|url| Crossing {
            session_id: session_id.to_owned(),
            timestamp: Utc::now(),
            requested_at: None,
            mode: CrossingMode::Observed,
            host: surface.id().to_owned(),
            client: None,
            tool: None,
            agent_type: None,
            agent_id: None,
            turn_id: None,
            cwd: None,
            url: url.to_owned(),
            host_name: grounding::host_of(url),
            internal: grounding::matches_internal_prefix(url, internal_prefixes),
            content_hash: None,
            retrieved_hash: None,
            estimated_tokens: None,
            delivered: None,
            grounded: false,
            // No surface reports rights, and being shown a source is not
            // permission.
            licence: LicenceState::Unknown,
            // Observed capture judges nothing and met no policy.
            refusal: None,
            policy_scope: None,
            principal: None,
            authentication_basis: None,
            breach: None,
            derived_from: None,
            http_status: None,
            failure: None,
            declarations: None,
            named_by: None,
            manifest_record: None,
            content_telemetry_id: None,
            supplier: None,
            identity: None,
            challenge: None,
            allowance: None,
        })
        .collect()
}

/// Record one answer in the operator home. `told` is the surface the command
/// was told, where it was told one; `fallback` names a session for a surface
/// that gave none. Returns what was recorded, or why nothing was.
pub fn record(
    home: &std::path::Path,
    answer: &BrowserAnswer,
    told: Option<HostSurface>,
    fallback: impl FnOnce() -> String,
) -> Result<Recorded, String> {
    let surface = answer.surface(told)?;
    let session_id = answer.session_id(fallback)?;
    // A policy that cannot be read lowers nothing: the floor holds, as on
    // every observed path.
    let internal_prefixes = crate::policy::SessionPolicy::load(home, None)
        .map(|policy| policy.internal_prefixes().to_vec())
        .unwrap_or_default();
    let crossings = capture(answer, surface, &session_id, &internal_prefixes);
    if crossings.is_empty() {
        return Ok(Recorded {
            session_id,
            crossings: 0,
        });
    }
    let mut log = SessionLog::open(home, &session_id)
        .map_err(|error| format!("cannot open the session log: {error}"))?;
    let mut written = 0;
    for crossing in &crossings {
        if log.record_crossing(crossing).is_ok() {
            written += 1;
        }
    }
    Ok(Recorded {
        session_id,
        crossings: written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(
        host: &str,
        session: Option<&str>,
        retrieved: &[&str],
        cited: &[&str],
    ) -> BrowserAnswer {
        BrowserAnswer {
            host: Some(host.to_owned()),
            session: session.map(str::to_owned),
            retrieved: retrieved.iter().map(|url| (*url).to_owned()).collect(),
            cited: cited.iter().map(|url| (*url).to_owned()).collect(),
        }
    }

    #[test]
    fn each_distinct_source_is_one_retrieved_crossing_and_none_grounds() {
        let message = answer(
            "chatgpt-web",
            Some("conv-1"),
            &["https://www.axios.com/a", "https://www.reuters.com/b"],
            &["https://www.reuters.com/b", "https://www.theguardian.com/c"],
        );
        let crossings = capture(&message, HostSurface::ChatgptWeb, "conv-1", &[]);
        assert_eq!(
            crossings.iter().map(|c| c.url.as_str()).collect::<Vec<_>>(),
            vec![
                "https://www.axios.com/a",
                "https://www.reuters.com/b",
                "https://www.theguardian.com/c",
            ],
            "a citation missing from retrieved is still recorded, once"
        );
        for crossing in &crossings {
            assert_eq!(crossing.mode, CrossingMode::Observed);
            assert_eq!(crossing.host, "chatgpt-web");
            assert!(!crossing.grounded);
            assert!(crossing.content_hash.is_none() && crossing.estimated_tokens.is_none());
            assert!(crossing.tool.is_none() && crossing.cwd.is_none());
        }
    }

    #[test]
    fn private_and_non_web_addresses_stay_out() {
        let message = answer(
            "google-ai-overview",
            None,
            &[
                "http://192.168.1.10/admin",
                "file:///etc/hosts",
                "javascript:alert(1)",
                "https://www.gov.uk/a",
            ],
            &[],
        );
        let crossings = capture(&message, HostSurface::GoogleAiOverview, "s", &[]);
        assert_eq!(crossings.len(), 1);
        assert_eq!(crossings[0].url, "https://www.gov.uk/a");
    }

    #[test]
    fn a_session_that_is_not_a_plain_name_is_refused() {
        for raw in ["../escape", "a/b", ".hidden", "", "conv id"] {
            let message = answer("chatgpt-web", Some(raw), &[], &[]);
            assert!(
                message.session_id(|| "fallback".to_owned()).is_err(),
                "{raw:?}"
            );
        }
        let message = answer(
            "chatgpt-web",
            Some("6a412eb6-c30c-83ea-b6fd-619b3a93db5c"),
            &[],
            &[],
        );
        assert_eq!(
            message.session_id(|| "fallback".to_owned()).unwrap(),
            "6a412eb6-c30c-83ea-b6fd-619b3a93db5c"
        );
        let message = answer("bing-copilot-search", None, &[], &[]);
        assert_eq!(
            message.session_id(|| "local-1-2".to_owned()).unwrap(),
            "local-1-2"
        );
    }

    #[test]
    fn the_surface_is_a_browser_surface_the_message_and_the_command_agree_on() {
        let google = answer("google-ai-overview", None, &[], &[]);
        assert_eq!(google.surface(None), Ok(HostSurface::GoogleAiOverview));
        assert_eq!(
            google.surface(Some(HostSurface::GoogleAiOverview)),
            Ok(HostSurface::GoogleAiOverview)
        );
        assert!(
            google
                .surface(Some(HostSurface::BingCopilotSearch))
                .is_err()
        );
        assert!(answer("claude-code", None, &[], &[]).surface(None).is_err());
        let unnamed = BrowserAnswer::default();
        assert_eq!(
            unnamed.surface(Some(HostSurface::ChatgptWeb)),
            Ok(HostSurface::ChatgptWeb)
        );
        assert!(unnamed.surface(None).is_err());
        assert!(unnamed.surface(Some(HostSurface::Cursor)).is_err());
    }
}
