---
domain: edge
audience: contributor
---

# Recorded pass presentation

Recorded on 15 October 2026 from the trusted news demo's stand-in
gatekeeper and stand-in Anchor (`src/gatekeeper.rs` and `src/passes.rs` in
the private repository `commonmeasure/trusted-news`), by its ignored test
`record_a_presentation`, run in a checkout of that repository with `EDGE`
naming a checkout of this one:

```sh
TRUSTED_NEWS_RECORD_PRESENTATION="$EDGE/crates/commonmeasure-cli/tests/recorded/pass-presentation" \
  cargo test record_a_presentation -- --ignored
```

The gatekeeper was reached at `127.0.0.1` and named that host in its
challenge. The edge's own client (`commonmeasure_harness::passes`) obtained
two passes for it from the stand-in Anchor, with a generator seeded at 9577,
and `wallet.json` is the wallet that refill left, with the Anchor's
loopback origin written as `http://anchor.recorded`. The exchanges with the
gatekeeper went through a proxy on loopback that wrote each as the bytes on
the wire: `NN.request` is what was sent and `NN.response` what the
gatekeeper answered.

- `01`: the article with no pass; the gatekeeper's 401 and its
  `PrivateToken` challenge.
- `02`: the article with the wallet's first pass; the release.
- `03`: the same pass again; the gatekeeper's refusal of a spent pass and
  a new challenge.
- `04`: a path the gatekeeper does not serve; its 404.

The article is the one written for the signed release fixture, so no
licensed text is committed, and the gatekeeper signed it with a key
generated for the run. The credential is the stand-in's test value and opens
nothing; the Anchor's key was generated for the run and the passes are
spendable nowhere else.

`crates/commonmeasure-cli/tests/pass_presentation.rs` serves these answers
from a loopback origin to the shipped binary, choosing each by what the
request carries as the gatekeeper did: no pass, the recorded pass the first
time, the recorded pass again, or another path.
