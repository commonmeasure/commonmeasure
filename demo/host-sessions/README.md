# Recorded host sessions

Session records written by the real hook binary and the real MCP server
during real host sessions, committed as they were written. Each file is a
session evidence log in the shape `docs/contracts/session-evidence.md`
states: identifiers, URLs, hashes, counters, record kinds and the host's
own turn identifiers. No prompt, answer or page text is in any of them,
because the log never holds those. The one edit made after recording is
that the `cwd` and `transcript` fields carry a neutral home directory in
place of the recording machine's; every hash, identifier, URL and counter
is as written. Terms like
crossing, boundary and mediated are defined in `docs/GLOSSARY.md`.

`crates/commonmeasure-cli/tests/recorded_sessions.rs` copies each directory
into a home of its own and runs `commonmeasure session` over it, so the
report's figures over a real session are pinned in the tree.

## `claude-code/`

One Claude Code session on host version 2.1.263, registered with
`commonmeasure install claude`, driven headless (`claude -p`, then `claude
-p --resume <id>` for each later turn) with a build of this tree named by
the registration. Four turns and a `/compact` between the third and the
fourth:

1. a `WebFetch` of `https://www.iana.org/help/example-domains`;
2. `context_status` then `context_fetch` of `https://example.com/`;
3. a `WebSearch`;
4. one question with no tool call, after the compaction.

`0e680a01-b902-4e86-85dc-4130d88ff14e.ndjson` is the hook session: six
`nudge_issued` (one per start, resume and compaction rebuild), four
`turn_started` and four `turn_completed` each carrying the host's
`prompt_id` and `privacy_level: minimal`, nine `crossing_observed` (the
grounded `WebFetch` and eight `WebSearch` results), and four
`context_snapshot` with the inventory of the window at each boundary. The
last boundary follows the compaction: its inventory counts one compaction
and its footprints start again from the rebuilt window.

`local-1788698405319-2755715.ndjson` is the MCP server's own session for the
same conversation: the `context_fetch` of `https://example.com/` as
`crossing_mediated` with its two `processor_invoked` records. The host
starts the server without a session id, so the mediated crossing sits in
the server's own log.

`crates/commonmeasure-harness/tests/recorded/claude-code-transcript.jsonl`
is the same session's host transcript with every text replaced by a run of
the same number of characters (`redact-transcript.py`, which keeps record
kinds, identifiers, tool, skill and agent names, the model, timestamps and
counters). The snapshot reader is tested over it, so its category filing,
its capability states and its compaction count are pinned against real
host records rather than hand-written ones.

## `pi/`

One Pi session on Pi 0.85.0, registered with `commonmeasure install pi`,
driven headless (`pi -p --mode json`, stdin closed) with a build of this
tree named by the extension. One turn: `context_status` then
`context_fetch` of `https://example.com/`.

`01a076b1-f309-7744-9398-6b4cad94412e.ndjson` is the server's log under
Pi's own session id, which the extension passes to the server: one
`crossing_mediated` as `host: pi` with its two `processor_invoked` records.
Pi has no web tool of its own, so there is nothing observed and no turn
boundary.

## Recording another

```sh
commonmeasure install claude
COMMONMEASURE_HOME=/tmp/cm-record claude -p --output-format json \
  --allowedTools "WebFetch,WebSearch,ToolSearch,mcp__commonmeasure__context_fetch,mcp__commonmeasure__context_status" \
  "Use the built-in WebFetch tool to read https://www.iana.org/help/example-domains and answer in one sentence."
commonmeasure uninstall claude
```

The session id is in the JSON the first turn prints; later turns take
`--resume <id>`. `commonmeasure session <id>` prints the report over
`/tmp/cm-record`. The Pi session is the same with `install pi` and `pi -p
--mode json "…" </dev/null`.
