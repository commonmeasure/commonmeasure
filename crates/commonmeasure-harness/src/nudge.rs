//! The standing mediation nudge.
//!
//! Agents default to their built-in tools unless asked otherwise, and the
//! refusable, policy-enforcing grade of evidence exists only when the agent
//! calls the mediated tools. Nobody pastes a per-session request into
//! ordinary work, so the plugin itself asks: its `SessionStart` hook prints
//! this text on stdout, and the host adds a `SessionStart` hook's stdout to
//! the session's context — on startup, on resume, and on the rebuilds after
//! `/clear` and compaction.
//!
//! It is a nudge, not enforcement, and nothing here may claim otherwise:
//! the text blocks nothing, the agent is free to ignore the request, and
//! observed capture keeps recording built-in use either way. The same text
//! comes from the plugin and from the settings hooks `install claude`
//! writes. On a Claude Code that loads the plugin's router
//! (`plugin/hooks/register.js`) with the edge's server connected, the
//! built-in web tools are answered through the mediated tools whether or not
//! the agent obeys, and refused where the edge refuses; elsewhere they run
//! natively. The hook cannot tell which holds, so the text names what the
//! model can see, a result or refusal that says it went through Common
//! Measure, instead of a host version. What this
//! runtime witnesses is emission; injection into context is the host's act,
//! which is why [`BASIS`] says exactly that on every issuance record.

/// Versioned like an evaluator identity: a `nudge_issued` record naming
/// `mediation-nudge/5` states exactly which wording was in force in that
/// session. Change [`TEXT`], bump this.
pub const IDENTITY: &str = "mediation-nudge/5";

/// What every session is asked. One short paragraph, deliberately: the cost
/// of a standing instruction is paid into every session's context, so the
/// wording stays as short as it stays honest.
///
/// Three conditions, kept apart because the right next move differs in each
/// and a single "if it does not work" would collapse them. An unconfigured
/// provider is an honest dead end the built-in tools may cover. A policy
/// refusal retried through a built-in tool would defeat the mediation the
/// operator asked for. A site refusing the request itself is neither: the page
/// may well be reachable with a built-in tool, and taking it is a decision
/// about whether to fetch as something other than the declared fetcher, which
/// the agent should take knowingly and which the record keeps either way.
///
/// The agent repeats this to a person, so it is told to report what the tool
/// said rather than why the site refused. Only a `cf-mitigated` answer
/// establishes that the refusal was about who asked; a bare 403 is equally a
/// paywall or a block, and an agent guessing between them states something
/// about a third party that nothing supports.
pub const TEXT: &str = "Common Measure standing instruction, from its SessionStart hook: for \
external web content, prefer this session's mediated tools — call context_fetch instead of \
WebFetch, and context_search instead of WebSearch — so operator policy can rule on each crossing \
before it happens and the record carries the refusable grade of evidence. A WebFetch or WebSearch \
whose result or refusal says it went through Common Measure was answered by these tools and has \
no native fallback; one that does not say so ran natively and is recorded after the fact. If a \
mediated tool reports unavailable (for example an unconfigured search provider), a built-in tool \
that runs natively is the fallback; if policy refuses a crossing, respect the refusal rather than \
retrying it with a built-in tool; if a site refuses the request itself, report what the tool said \
and no more, because a built-in tool that runs natively fetches as something other than the \
declared fetcher and is recorded as your decision. This is a nudge, not enforcement: it blocks \
nothing itself.\n";

/// The claim an issuance record can honestly make.
pub const BASIS: &str = "emitted on the SessionStart hook's stdout for the host to add to the \
session's context; injection is the host's act and is not witnessed";

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole request: both replacements by name, and the three conditions
    /// kept apart — an unavailable tool (fall back), a policy refusal (respect
    /// it) and a site refusing the request (report what the tool said; the
    /// fallback is a recorded decision).
    #[test]
    fn the_nudge_names_both_replacements_and_separates_the_three_conditions() {
        for phrase in [
            "context_fetch",
            "WebFetch",
            "context_search",
            "WebSearch",
            "unavailable",
            "respect the refusal",
            "refuses the request itself",
            "and no more",
            "your decision",
        ] {
            assert!(TEXT.contains(phrase), "the nudge must mention {phrase:?}");
        }
    }

    /// The box's own condition: honest about what it cannot guarantee. The
    /// text must call itself a nudge and must not claim anything is blocked
    /// or enforced.
    #[test]
    fn the_nudge_claims_no_enforcement() {
        assert!(TEXT.contains("This is a nudge, not enforcement"));
        assert!(TEXT.contains("it blocks nothing itself"));
    }

    /// The same text is emitted by the plugin and by the settings hooks
    /// `install claude` writes, with the router loaded or not, so it names
    /// what the model can see in a result rather than a host version, and
    /// promises no fallback or absence of blocking that the router's
    /// refusals would contradict.
    #[test]
    fn the_nudge_names_the_condition_the_model_can_see() {
        assert_eq!(IDENTITY, "mediation-nudge/5");
        assert!(TEXT.contains("says it went through Common Measure"));
        assert!(TEXT.contains("has no native fallback"));
        for claim in [
            "2.1.287",
            "installed commonmeasure plugin",
            "the plugin routes",
            "nothing blocks the built-in tools",
            "the built-in tools remain the fallback",
        ] {
            assert!(!TEXT.contains(claim), "the nudge must not say {claim:?}");
        }
    }

    /// The text enters every session's context, so growth is a per-session
    /// cost. This bound is the reviewable budget: it holds the three
    /// conditions and their reasons plus the one sentence on the router, and
    /// raising it is a decision, not drift (950 bytes held nudge/3; nudge/4
    /// added the router sentence; nudge/5 rewrote it within the same bound).
    #[test]
    fn the_nudge_stays_one_short_paragraph() {
        assert!(
            TEXT.len() <= 1050,
            "{} bytes is no longer a nudge",
            TEXT.len()
        );
        assert_eq!(
            TEXT.trim_end().lines().count(),
            1,
            "one paragraph, emitted as one line"
        );
    }
}
