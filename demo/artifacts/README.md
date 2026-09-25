---
domain: edge
audience: contributor
---

# Portable artifact example

A synthetic saved file and the bundle `commonmeasure artifact` emitted for it.
The bundle has two explicit session declarations (drafting and review), one
copied synthetic NDJSON prefix and no signature. It makes no claim of real source use,
authorship or authenticated identities.

From the repository root, run:

```sh
cargo run --locked --offline -p commonmeasure-cli -- artifact verify \
  demo/artifacts/answer.txt.commonmeasure.json --file demo/artifacts/answer.txt \
  --expected-snapshot sha256:f529fb5e986f374a677aeef9681290390d93c3e43dc2266c63be68dc77804c9c
```

The result is `valid: true`, `signature: "absent"`, `trust: "not_evaluated"`.
The check reads only these two files, not the store or log that produced them,
so it passes with both copied elsewhere and their new paths supplied. Changing
the answer or the evidence makes it fail. The separately retained digest above
also detects replacement of the whole unsigned bundle; it authenticates nobody.

The bundle uses canonical compact JSON. Use a JSON viewer to inspect its
`associations`, `snapshot` and retained `evidence` entries. The saved file and
bundle contain no machine-local paths or private source records.

See [the contract](../../docs/contracts/artifact-association.md) for creation
commands, fixed-Git-tree capture, optional evidence selection and limits.
