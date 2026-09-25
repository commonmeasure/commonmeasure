---
domain: edge
audience: operator
---

# Common Measure

Secure, safe, legal and transparent AI agent content usage.

Common Measure applies your rules on sources, licences and spend to the
content an AI agent reads. It records where each piece came from, its cost
or an explicit unknown, and a hash of the text that entered the agent's
context, and it measures whether that content helped.

It is one binary that runs on your machine and registers with Claude Code,
Codex, Pi, Claude Desktop, Cursor, the Copilot CLI, VS Code and Chrome. The
record stays on your machine unless you configure a receiver for it.

- **Rules before content moves**: allowed and refused sites, required
  licences, a cost cap per job, spend limits per person per day and month.
  A fetch Common Measure carries honours the site's `robots.txt` in every
  policy mode.
- **An append-only record** of every fetch and search, graded: *mediated*
  when Common Measure carried and checked it, *observed* when a host hook
  saw it afterwards. Missing data is recorded as missing, never as zero.
- **Measurement** of whether an answer cited a source, how much of the
  question the source covered, and how recent it is.
- **Checks at admission** for prompt injection and personal data.

## Install

From a release, with no toolchain. The installer downloads the binary for
your platform, verifies its checksum and puts it in `~/.local/bin`:

```sh
curl -fsSL https://github.com/commonmeasure/commonmeasure/releases/latest/download/install.sh | sh
commonmeasure install claude
```

As a Claude Code plugin, after the installer's first line above:

```sh
claude plugin marketplace add commonmeasure/commonmeasure
claude plugin install commonmeasure@commonmeasure
```

Use one route or the other for Claude Code: a machine registered both ways
records every fetch twice.

From source, with Rust 1.97 or later:

```sh
git clone https://github.com/commonmeasure/commonmeasure.git
cd commonmeasure
cargo install --locked --path crates/commonmeasure-cli
commonmeasure install claude
```

`commonmeasure install <host>` registers the other hosts, and
`commonmeasure doctor` shows what each host has registered.

## First use

Start a new Claude Code session and ask it to read a web page. Then:

```sh
commonmeasure session      # what the most recent session recorded
commonmeasure serve        # the local console, on 127.0.0.1
```

A session that fetched one page prints, among other lines:

```text
records    3
crossings  1 observed, 0 mediated, 0 refused, 0 reconstructed
grounded   1 put page text into the model's context
```

To refuse sources before they are read, write a policy file and check it:

```sh
commonmeasure policy check ~/.commonmeasure/policy.json
```

[docs/GETTING-STARTED.md](docs/GETTING-STARTED.md) walks through
registration, policy, the console, importing earlier sessions, sending
records to a receiver and enrolling a project.

## Documentation

- [docs/README.md](docs/README.md): the documentation index.
- [docs/GETTING-STARTED.md](docs/GETTING-STARTED.md): the tested walkthrough.
- [docs/GLOSSARY.md](docs/GLOSSARY.md): every defined term in one line.
- [ARCHITECTURE.md](ARCHITECTURE.md): the components and how data moves.
- [docs/contracts/](docs/contracts/): the record formats and interfaces.
- [docs/FAIL-POLICY.md](docs/FAIL-POLICY.md): what happens when a dependency
  is missing or a measurement is unknown.
- [docs/CONSOLE.md](docs/CONSOLE.md): the operator console.
- [Common Measure Hub guides](https://commonmeasure.ai/docs/hub/start-here/):
  joining an organisation's hub, and what leaves a machine that does.
- [plugin/README.md](plugin/README.md): the Claude Code plugin and each
  host's registration.
- [CHANGELOG.md](CHANGELOG.md): what each release changed.

## Contributing

Build, test and pull-request rules are in [CONTRIBUTING.md](CONTRIBUTING.md)
and [AGENTS.md](AGENTS.md). Report security problems as
[SECURITY.md](SECURITY.md) describes, not in a public issue.

## Licence

Source-available under the Functional Source License, FSL-1.1-Apache-2.0
([LICENSE.md](LICENSE.md)). You may use, copy, change and share it for any
purpose except offering it, or a substitute for it, as a commercial product
or service. Each release becomes Apache-2.0 two years after it is made. The
contracts under [docs/contracts/](docs/contracts/) are CC-BY-4.0, so anyone
may implement them.

Copyright 2026 Common Measure Ltd.
