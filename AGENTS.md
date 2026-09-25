---
domain: shared
audience: contributor
---

# Repository conventions

Conventions for anyone changing this repository, person or agent. Common
Measure is source-available under the Functional Source License
(`LICENSE.md`); `CONTRIBUTING.md` covers pull requests and the contributor
licence agreement.

## Where things are

- `README.md`: what the product is and the quick start.
- `ARCHITECTURE.md`: the components and how a run moves through them.
- `docs/contracts/`: the formats a change must keep to.
- `docs/GLOSSARY.md`: the defined terms. Look a term up rather than
  guessing its meaning.
- `docs/FAIL-POLICY.md`: what the product does when a dependency or a
  measurement is missing.

## Build and check

Rust 1.97 or later. Fetch dependencies once online; the checks then run
offline.

```sh
cargo fetch
cargo build --workspace --locked
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test -p <crate>          # the crates a change affects
cargo test --workspace --locked
node --test browser/test/      # browser extension, node 22 or later
```

`just` lists the same commands as recipes; `just gates` runs formatting,
Clippy, the tests, the browser tests and the design-token check.

Formatting, Clippy with warnings denied and the affected crates' tests must
pass before a change is proposed. Report the commands run and their results.

Some tests read recorded fixtures (provider responses, committed runs, host
sessions) that this repository does not carry. They report as ignored, with
the missing directory named, and are not failures. Maintainers who hold the
fixtures run them by setting `COMMONMEASURE_PRIVATE_EVIDENCE`
(`CONTRIBUTING.md`).

## Code

- Idiomatic, self-documenting Rust. Document public APIs and non-obvious
  invariants. Comments explain why, not what the code does.
- An unknown measurement stays unknown. Missing cost, latency, token, route
  or evaluation evidence is never recorded as zero or as a requested value
  (`docs/FAIL-POLICY.md` §7).
- A missing dependency produces an explicit unavailable result that names
  the gap, never a weaker successful mode (`docs/FAIL-POLICY.md` §5).
- A fake provider, inference backend, ledger or receiver does not prove that
  an integration works. Replay tests serve recorded bytes from a loopback
  origin through the production transport, parsing and execution path.
- Focused test doubles, injected clocks and fault injection are fine for
  unit tests and failure handling. Keep test seams small, and do not build a
  parallel implementation of the product for testing.
- Prefer the smallest real vertical path to speculative traits, parallel
  type systems or copied subsystems. Every abstraction needs a current
  consumer or an external protocol boundary.
- Deterministic policy runs before learned optimisation.
- A processor is a stage around a ContextJob (`docs/contracts/processor.md`);
  a harness plugin integrates the product with a host (`plugin/`). Do not use
  the bare word "plugin" for the first.
- Supply adapters implement the capabilities a supplier has. Do not treat
  content and skill suppliers as having the same search, fetch, invocation,
  provenance, terms or payment semantics.
- Format identifiers inside sealed evidence keep the `contextops-` namespace.
  Do not rename them.
- Never commit credentials, tokens, customer prompts or licensed content.
- Behaviour, contracts and documentation change together.

## Writing

These rules apply to docs, UI copy, comments and commit messages.

- Plain British English, concrete verbs and stable product terms. In prose
  the product is **Common Measure**; the evidence a session or run leaves is
  the **source record**; the operator's policy file is the **source
  policy**; the hosted tier is **Common Measure Hub**.
- State what happens and, when useful, why. Keep necessary uncertainty.
- No mannered prose: aphorisms, rhetorical questions, dramatic fragments,
  slogans, or sentences shaped for effect. Avoid "This isn't X. It's Y." when
  a direct statement would do.
- No filler or self-praise: "seamless", "powerful", "robust", "elegant",
  "at its core", "importantly". Describe the behaviour or the evidence.
- Do not narrate the UI. A page needs controls, data and consequences, not a
  paragraph announcing what the page does. UI text earns its place by
  helping someone choose, act or recover.
- Keep implementation detail out of product copy unless the user needs it
  to complete the task.
- A commit message states the change and its reason.

| Remove | Write instead |
|---|---|
| "This isn't just a log. It's trust, made visible." | "The log records each source and the policy decision." |
| "Seamlessly take control of your source ecosystem." | "Choose which sources your agents may use." |
| "This page lets you view and manage your API keys." | Use the heading "API keys" and the relevant controls. |
| "Click Save to save your changes." | Use the button label "Save". |

Common Measure's own agents: the internal working rules are in the private
company repository.
