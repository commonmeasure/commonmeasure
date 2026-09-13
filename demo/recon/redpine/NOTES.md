# Redpine reconnaissance notes, 4 August 2026

Probed 4 August 2026, the day the API key arrived. Twelve MCP requests over
two sessions; one trial query consumed; no currency charged. Captures in this
directory carry the exchanges, with the licensed transcript text truncated —
the permitted use of fetched content is not machine-readable anywhere in the
responses, so the committed record keeps structure and provenance and drops
the licensed words.

Redaction covers every carrier: the human-readable `content[0].text` and
the machine-readable duplicate in `structuredContent.content` hold the same
truncated text byte for byte, and the `mcp-session-id` header carries the
issued-by marker on both request and response sides.
`crates/commonmeasure-cli/tests/recon_captures.rs` sweeps every carrier of every
committed capture, including strings that are themselves serialised JSON.

## The first licensed content fetch, full stop

`preview` → `confirm` on `media--sample` returned three individual podcast
transcript mentions from the "Redpine × AllEars data partnership"
(`redpine-confirm-sample.json`). This is the repository's first successful
licensed content fetch against any provider. It ran under a free trial: the
receipt says `cost_charged: "0.000000"`, and the trial meter observably moved
from 5 of 5 to 4 of 5 (`redpine-balance-before.json`,
`redpine-balance-after.json`). A charge in currency has still never been
observed; the trial is a meter, not a price.

## Quote-then-buy is the protocol surface, not a convention

The gateway is six meta-tools (`get_balance`, `find-tools`, `inspect-tool`,
`preview`, `confirm`, `call-tool`) in front of the actual integrations.
`preview` executes the target tool, returns a teaser plus a billing note that
names what confirming will consume, and issues a `preview_id`; `confirm`
unlocks the full result and returns `cost_charged` and `balance_remaining`.
That is a quote → decision → settlement loop at the API boundary — the shape
Common Measure's admission path wants, offered natively by the supplier. TollBit's
equivalent (rate → token → content) has never got past its first step on our
key.

## Prices are machine-readable, and there are two of them

`inspect-tool` on `media--sample` returns a pricing annotation:
`{cost_credits: 0.03, cost_type: "exact"}`, and the prose says "$0.03/query"
(`redpine-inspect-sample-2.json`). The confirmed payload's own `_meta` says
`cost_usd: 0.0015`. Twenty-fold apart. Plausibly list price versus data-partner
cost, but nothing in the responses says which one a funded account is charged.
Recorded as an open assumption; settled only by a post-trial query with a cash
balance.

## Session discipline

The server is `redpine-connect` (Streamable HTTP, JSON-RPC). `tools/list`
without a session answers "Missing session ID — call initialize first"; the
session id arrives as an `mcp-session-id` response header on `initialize`.
`inspect-tool`'s parameter is `tool_name`, not `name` — guessing `name` earns
a polite error. One `initialize` timed out at 60 s and succeeded on retry;
every priced-path call answered in under a second.

## Not exercised

The REST surface from the getting-started page
(`POST /api/v1/search/query` with `{collection, query}`) was not called: it
needs a collection name and nothing in the MCP surface named one. The
`call-tool` direct path (bypassing preview) was not called. Four trial
queries remained after this probe; they were the budget for adapter
integration, not for more reconnaissance (three remain after the
pre-registered live run below).

## Pre-registered: the one live adapter run (written 6 August 2026, before any call)

The adapter is green over these captures (loopback tests, the committed
replay run `demo/output/redpine-replay/`), so the live acceptance run may
spend **exactly one** of the four remaining trial queries — one `confirm`;
`initialize`, `get_balance`, `inspect-tool` and `preview` are free. What the
run must observe, decided before it runs:

- the confirm receipt's `cost_charged` and `balance_remaining`, verbatim —
  the R1/R2 observation. A trial-covered zero is recorded as trial-covered
  (unit `trial_queries`), never as free;
- the balance and trial meter before purchase, from the in-quote
  `get_balance` (expected 4 of 5 after the recon's spend), and the meter
  movement the receipt implies (4 → 3);
- the preview's `billing_status`/`billing_note` shape on a *non-fresh* trial
  (the recorded one said "5 of 5"; whether the note re-counts correctly is
  new evidence), and whether any non-trial billing field appears (R5);
- whether the pricing annotation still reads `cost_credits: 0.03,
  cost_type: "exact"` and the payload still carries `_meta.cost_usd`
  twenty-fold apart (R1 stays open for a funded account either way — the
  trial observation narrows it to "a trial account is charged trial
  queries, not currency");
- the same argument shape as the recording (`filter.keyword: "NVIDIA"`,
  `sort: "recent"`, `limit: 3`): R6 is deliberately *not* probed.

Failure discipline: a failure before `confirm` spends nothing — re-run
freely. A failure between `preview` and `confirm`'s response leaves the
meter state unknown; check it with a free `get_balance` before any re-run,
and record what it says. The run itself is gitignored
(`demo/output/live-redpine/`, the live convention) and its receipt lands in
the register rows R1/R2 and the verification record.

**Outcome (same day):** run `b8f0bcf2`, one attempt, no failures, one trial
query spent. Every pre-registered observation landed: receipt
`cost_charged: "0.000000"` / `balance_remaining: "0.00"`, meter 4 → 3, the
non-fresh-trial billing note re-counted correctly ("4 of 5 … 3 would
remain"), the pricing annotation and `_meta.cost_usd` returned unchanged
and still twenty-fold apart, the recorded argument shape used and R6 left
unprobed. Register rows R1/R2/R5 and
`docs/knowledge-base/provider-verification.md` carry the record. Three
trial queries remain.
