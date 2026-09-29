---
domain: edge
audience: contributor
---

# Interactive Codex record

Recorded on 29 September 2026 with Codex CLI 0.157.1 (`codex-tui`) and
Common Measure 0.4.6. An interactive terminal session, resumed once, used
the installed `commonmeasure` MCP registration. This extract comes from
the resumed server: it refused Google's `/search` path under its recorded
robots declaration, then admitted 500 characters of the Common Measure
public page. Both crossings carry the same source policy identity.

The managed policy allowed public hosts in the selected directory and
required robots refusals in every policy mode. This is a source-declared
path refusal, not verification of an operator's denied-host rule.

The NDJSON retains only `client_identified`, `crossing_refused` and
`crossing_mediated`. Paths use `/operator`, `/work` and `/work/acceptance`;
the signing key identifier is `recorded-edge`. These substitutions preserve
neither a deployable configuration nor a verifiable signing identity.
Timestamps, session identifiers, URLs, hashes, declarations and counters
are unchanged. No prompt, answer, page body or credential is included.

`tool-results.json` contains selected metadata from the raw tool outputs
in the Codex transcript, with the same path substitutions. `delivered_hash`
was calculated as SHA-256 of the admitted tool output's `content` bytes
before omitting that text. It identifies the delivered 500-character part;
`content_hash` identifies the whole extracted page. The refusal text is
the host's tool error, rather than the assistant's account of it.

`recorded_sessions.rs` reads the extract through the real session command
and compares it with the independently captured tool metadata. This is a
record-reader regression fixture, not a transport replay or a Hub receipt.
Local relay state showed delivery of two content events and a refusal
count of one. The receiving Hub organisation was not independently read
back, so the Codex verification state remains `fixture-tested`.
