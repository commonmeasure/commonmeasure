---
domain: edge
audience: contributor
---

# Signed release record

Recorded on 13 October 2026 from the trusted news demo's stand-in
gatekeeper (`src/gatekeeper.rs` in the private repository
`commonmeasure/trusted-news`), by its ignored test `record_a_release`, run
in a checkout of that repository with `EDGE` naming a checkout of this one:

```sh
TRUSTED_NEWS_RECORD_RELEASE="$EDGE/crates/commonmeasure-cli/tests/recorded/signed-release" \
  cargo test record_a_release -- --ignored
```

`body.json` is the body of the gatekeeper's answer for a release, byte for
byte, and `content-type` its `Content-Type` value. The gatekeeper signed
the text with a key generated for the run and served the answer through
the same function that answers `/article`. Two parts of the live path are
left out: the pass check, and the fetch from Wikipedia. The article was
written for this fixture, so no licensed text is committed. The `source`
and `attribution` fields still name Wikipedia, because the stand-in writes
them that way for every release.

The release format and its media type are a working format, not a
standard (`crates/commonmeasure-types/src/release.rs`).
`crates/commonmeasure-cli/tests/release_e2e.rs` serves these bytes from a
loopback origin to the shipped binary.
