# Portable artifact example

Synthetic saved-file example emitted by `commonmeasure artifact` on 22 September
2026. It has two explicit session declarations (drafting and review), one copied
synthetic NDJSON prefix and no signature. It makes no claim of real source use,
authorship or authenticated identities.

From the repository root, run:

```sh
cargo run --locked --offline -p commonmeasure-cli -- artifact verify \
  demo/artifacts/answer.txt.commonmeasure.json --file demo/artifacts/answer.txt \
  --expected-snapshot sha256:f529fb5e986f374a677aeef9681290390d93c3e43dc2266c63be68dc77804c9c
```

The result is `valid: true`, `signature: "absent"`, `trust: "not_evaluated"`.
The original store and log were deleted before this example was verified. Copy
both files elsewhere and supply their new paths to run the same check. Changing
the answer or evidence causes failure. The separately retained digest above
also detects replacement of the whole unsigned bundle; it authenticates nobody.

The bundle uses canonical compact JSON. Use a JSON viewer to inspect its
`associations`, `snapshot` and retained `evidence` objects. The saved file and
bundle contain no machine-local paths or private source records.

See [the contract](../../docs/contracts/artifact-association.md) for creation
commands, fixed-Git-tree capture, optional evidence selection and limits.
