---
title: Documentation
domain: edge
audience: operator
section: get-started
---

# Documentation

Install and use Common Measure, read its source records, and integrate
hosts and content with it. Common Measure Hub is open by invitation
during the pilot ([waiting list](https://commonmeasure.ai/waitlist/?source=edge)). Once invited, follow
[Start here](https://commonmeasure.ai/docs/hub/start-here/) from sign-in to
your first delivery, or
[connect an existing edge](https://commonmeasure.ai/docs/hub/connect-commonmeasure/).
How these pages are written and published is `DOCUMENTATION.md` in the
repository.

Get started:

- [`docs/GETTING-STARTED.md`](GETTING-STARTED.md): from the installer to a
  recorded session, policy, the console, import, the relay and a build from
  source.
- [Installing and updating](INSTALL.md): release assets, updates and login services.

Use:

- [Sessions, policy and reporting](OPERATING.md): credentials, scopes, import,
  relay and directory enrolment.

- [`docs/CONSOLE.md`](CONSOLE.md): the operator console, section by section.

Integrate:

- [Write a sidecar checker](extensions/authoring.md): script, manifest, lifecycle checks and source-record evidence.
- [People Inc content](integrate/peopleinc.md): configure article search and
  named-URL retrieval, with billing and verification limits.
- [`docs/integrate/host.md`](integrate/host.md): connect an agent host to an
  edge through the MCP tools, over stdio or Streamable HTTP, and the hooks.
- [`docs/integrate/reports.md`](integrate/reports.md): receive usage reports
  about your content, match them to your logs, and run a receiver.
- [`docs/integrate/bot.md`](integrate/bot.md): recognise and verify
  `CommonMeasureBot`, and what to do about a key.

Reference:

- [`docs/GLOSSARY.md`](GLOSSARY.md): every defined term in one line.
- [`docs/FAIL-POLICY.md`](FAIL-POLICY.md): what the edge does when a
  dependency is missing, a write fails or a measurement is unknown.
- [`docs/contracts/source-policy.md`](contracts/source-policy.md) (Edge): the
  policy file, what the loader accepts and refuses, and the schema and
  vectors published beside it.
- [`docs/contracts/session-evidence.md`](contracts/session-evidence.md) (Edge):
  every record a session writes.
- [`docs/contracts/telemetry-projection.md`](contracts/telemetry-projection.md)
  (Network): what the relay sends to a receiver as Content Telemetry, under
  which clearances, and how delivery is retried.
- [`docs/contracts/onward-delivery.md`](contracts/onward-delivery.md)
  (Network): how the hub finds a content owner for an event and delivers it
  to the owner's endpoints.
- [`docs/contracts/bot-identity.md`](contracts/bot-identity.md) (Network):
  `CommonMeasureBot`: the per-edge key, signed requests, the key directory,
  the agent card and revocation.

The full contract set, every authoritative format with the domain that owns
it, is in [`docs/contracts/`](https://github.com/commonmeasure/commonmeasure/tree/main/docs/contracts)
on GitHub.

Guides, published at https://commonmeasure.ai/guides/:

- [Four fetches, two refused](guide/four-fetches.md): one agent's four
  recorded fetches, read from their evidence.
- [`docs/guide/context-window-optimisation.md`](guide/context-window-optimisation.md):
  what should enter a context window and how to know it helped.
- [`docs/guide/state-of-the-evidence.md`](guide/state-of-the-evidence.md): the
  evidence behind that guide, graded claim by claim.
- [How agents use outside content](guide/untrusted-context.md): permission,
  reliability, prompt injection and the records operators and publishers need.
- [`docs/guide/measurements/README.md`](guide/measurements/README.md): the original
  measurements the guides report, and how to rerun them.
