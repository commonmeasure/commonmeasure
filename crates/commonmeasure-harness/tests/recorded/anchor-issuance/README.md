---
domain: edge
audience: contributor
---

# Recorded Anchor issuance

Recorded on 15 October 2026 from the trusted news demo's stand-in Anchor
(`src/passes.rs` in the private repository `commonmeasure/trusted-news`),
by its ignored test `record_an_issuance`, run in a checkout of that
repository with `EDGE` naming a checkout of this one:

```sh
TRUSTED_NEWS_RECORD_ISSUANCE="$EDGE/crates/commonmeasure-harness/tests/recorded/anchor-issuance" \
  cargo test record_an_issuance -- --ignored
```

The stand-in ran with a ration of three passes per credential. The edge's
own client (`commonmeasure_harness::passes`) asked it for four passes for
the host in `recording.json`, through a proxy on loopback that wrote each
exchange as the bytes on the wire: `NN.request` is what the client sent and
`NN.response` what the stand-in answered. `01` is the issuer directory,
`02` to `04` are passes, and `05` is the ration refusal (429), as the
stand-in stated it.

The client blinded with a generator seeded from `recording.json`, so a
client seeded the same way sends these requests again byte for byte, apart
from the loopback port in `Host`. The credential is the stand-in's test
value and opens nothing. The key was generated for the run.

`crates/commonmeasure-harness/tests/anchor_issuance.rs` serves these bytes
from a loopback origin to the edge's client, and
`crates/commonmeasure-cli/tests/passes_cli.rs` serves the directory and the
refusal to the shipped binary.
