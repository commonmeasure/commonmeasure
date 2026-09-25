---
domain: shared
audience: contributor
---

# Contributing

Common Measure is source-available under the Functional Source License
(`LICENSE.md`). It does not accept outside contributions for now: pull
requests and patches from outside Common Measure Ltd are closed without
review.

Bug reports are welcome as GitHub issues. Say what you ran, what you
expected and what happened, and include the version (`commonmeasure
--version`). Security problems go to hello@commonmeasure.ai (`SECURITY.md`),
not to a public issue.

[ARCHITECTURE.md](ARCHITECTURE.md), [docs/contracts/](docs/contracts/) and
[AGENTS.md](AGENTS.md) describe the components, formats and conventions for
anyone reading or building the code. [DOCUMENTATION.md](DOCUMENTATION.md)
sets how the pages under `docs/` are written so the website can publish
them, and [RELEASING.md](RELEASING.md) how a release is cut.

## Recorded fixtures (optional, for maintainers)

Some tests read recorded fixtures that this repository does not carry:
third-party provider responses (`recon/`), committed runs (`output/`) and
recorded host sessions (`host-sessions/`). Without them those tests report
as ignored, with the missing directory named, and `just gates` still passes.

A maintainer who holds the fixtures sets `COMMONMEASURE_PRIVATE_EVIDENCE` to
the directory that contains them, as an absolute path or one relative to the
repository root:

```sh
COMMONMEASURE_PRIVATE_EVIDENCE=/path/to/evidence just gates
```

The fixture tests then run, and the `just` recipes that replay recorded
responses or regenerate a committed run read and write that directory.
Changing the variable rebuilds the crates that read it.
