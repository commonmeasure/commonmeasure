# Common Measure

Input control and audit for AI agents.

Common Measure sits between an AI agent and everything it reads. It decides,
before the agent reads a page, whether the agent may read it under your
rules on sources, licences and spend. It writes down where every piece of
content came from, what it cost and a hash of exactly what the model saw,
so a person or a regulator can check the record without trusting a
dashboard or reading a transcript. It measures whether the content helped,
so you can tell what the agent needed and did not get, and which route to
use next time.

It is one component you build into your own agent system. It is not a
harness, not an agent framework, not a model gateway and not a content
marketplace ([PRODUCT.md](PRODUCT.md)). The binary is `commonmeasure`.

## Install

Three routes. Each ends with the binary on your machine and registered
with your host. No account is needed. The record stays on your machine
unless you configure somewhere for it to go.

**From a release, with no toolchain.** The installer downloads the binary
for your platform, checks its checksum and puts it in `~/.local/bin`.

```sh
curl -fsSL https://github.com/commonmeasure/commonmeasure/releases/latest/download/install.sh | sh
commonmeasure install claude
```

**As a Claude Code plugin.** The plugin registers the same hooks and MCP
server through Claude Code's plugin mechanism. It carries no binary, so run
the installer's first line above, then:

```sh
claude plugin marketplace add commonmeasure/commonmeasure
claude plugin install commonmeasure@commonmeasure
```

**From source.** Rust 1.97 or later.

```sh
git clone https://github.com/commonmeasure/commonmeasure.git
cd commonmeasure
cargo install --path crates/commonmeasure-cli
commonmeasure install claude
```

Register Claude Code by one route or the other. A machine that holds both
registrations records every fetch twice, because each carries the same
hooks; a plugin-only install, or a direct registration alone, records each
fetch once. `commonmeasure install codex` and `commonmeasure install pi`
register the tools with those hosts. `commonmeasure doctor` shows what each
host has. The full walkthrough is
[docs/GETTING-STARTED.md](docs/GETTING-STARTED.md).

## One fetch, end to end

An agent in Claude Code is asked to read a page. It uses the tool Common
Measure provides instead of its built-in fetch:

```text
context_fetch https://fsl.software/FSL-1.1-ALv2.template.md
```

Your policy is checked first. Nothing excluded this page, so the tool
returns its text and two hashes: one over the bytes the site served, one
over the text the model saw. The result, with the page text left out:

```json
{
  "url": "https://fsl.software/FSL-1.1-ALv2.template.md",
  "content_hash": "sha256:36b6082235c0a2105174927fc57cc6ae9c41f45a08af2bdcaee18a8dace56177",
  "retrieved_hash": "sha256:36b6082235c0a2105174927fc57cc6ae9c41f45a08af2bdcaee18a8dace56177",
  "http_status": 200,
  "policy": "Admitted; no constraint excluded it.",
  "recorded_in": "~/.commonmeasure/sessions/local-1789308747565-2206.ndjson"
}
```

The two hashes are equal here because the page is plain text. For an HTML
page the first is over the markup and the second over the extracted text.
You read the same fetch back from the record:

```sh
commonmeasure session local-1789308747565-2206
```

```text
crossings  0 observed, 3 mediated, 0 refused, 0 reconstructed
  mediated   grounded  https://fsl.software/FSL-1.1-ALv2.template.md
```

A crossing is one moment content enters the agent. "Mediated" means it was
checked before it happened. "Observed" means a hook saw it afterwards and
could not have stopped it. `commonmeasure serve` opens a local web page
that shows the same record.

## What it does

- **Rules.** Fixed rules, applied before content moves: which sites are
  allowed, which licences are required, a cost cap per job, a spend limit
  per person per day and month.
- **The record.** Every fetch goes into an append-only log with its grade.
  A batch run also keeps the exact bytes it received and a report in which
  every claim points at its evidence. Missing data is marked missing, never
  written as zero.
- **Measurement.** Three fixed checks score each source: did the answer
  really cite it, how much of the question did it cover, how recent is it.
- **Add-ons.** Extra checks run on the same stream: a prompt-injection
  screen, a personal-data detector, a context optimiser and an
  answer-fidelity check. Every add-on built into the binary runs; a switch
  to turn one off per operator is not built yet. Others can be written to
  the same contract ([docs/contracts/processor.md](docs/contracts/processor.md)).
- **Sources.** Fifteen adapters: twelve web search providers, your own
  document folder, one licensed supplier bought by quote, one licensed
  publisher corpus. A missing key or gateway gives a clear "unavailable"
  result naming what is missing.
- **Sending data out.** Nothing leaves your machine by default. One command
  sends a cleared subset of the record to a receiver you name, in the
  Content Telemetry standard. In an organisation managed from the hub, the
  owner clears it in the policy the hub distributes to each machine
  ([docs/HUB.md](docs/HUB.md); the hub's documentation, at
  `/docs/policy-distribution` on the hub).

## Build and test from a checkout

```sh
cargo fetch                       # once, online
cargo test --workspace --offline
cargo run -p commonmeasure-cli -- run demo/jobs/energy-price-cap.json --output /tmp/commonmeasure-run
cargo run -p commonmeasure-cli -- inspect /tmp/commonmeasure-run
```

The tests make no network calls but bind local sockets. The example run
needs no keys; it reports what it could not do and why. `just --list`
shows the other commands.

## Documentation

- [PRODUCT.md](PRODUCT.md): what it is, who it is for, what it is not.
- [ARCHITECTURE.md](ARCHITECTURE.md): the parts and how data moves.
- [ROADMAP.md](ROADMAP.md): what is built and what is not.
- [DECISIONS.md](DECISIONS.md): the rules the product is held to.
- [AGENTS.md](AGENTS.md): rules for changing this repository.
- [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md).
- [docs/GLOSSARY.md](docs/GLOSSARY.md): every defined term in one line.
- [docs/GETTING-STARTED.md](docs/GETTING-STARTED.md): the tested walkthrough.
- [docs/HUB.md](docs/HUB.md): the hub: what leaves a machine, how a machine
  joins and how policy arrives.
- [docs/RELEASE.md](docs/RELEASE.md): releases and the installer.
- [docs/contracts/](docs/contracts/): the formats and interfaces.
- [docs/READ-A-RUN.md](docs/READ-A-RUN.md) and the other `docs/READ-*.md` pages: how to read the committed demonstrations under [demo/](demo/).
- [plugin/README.md](plugin/README.md): the Claude Code plugin and policy.
- [console/README.md](console/README.md): the local web page.

## Licence

Source-available under the Functional Source License, FSL-1.1-Apache-2.0
([LICENSE.md](LICENSE.md); SPDX identifier FSL-1.1-ALv2). You may use, copy,
change and share it for any purpose except offering it, or a substitute for
it, as a commercial product or service. Each release becomes Apache-2.0 two
years after it is made. It is not open source. The contracts under
[docs/contracts/](docs/contracts/) are CC-BY-4.0, so anyone may implement
them. Contributions need the agreement in
[CONTRIBUTING.md](CONTRIBUTING.md).

Copyright 2026 Common Measure Ltd.
