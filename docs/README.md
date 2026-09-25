---
title: Documentation
domain: edge
audience: operator
section: get-started
---

# Documentation

Install and use Common Measure, read its source records, and integrate
hosts and content with it. For Common Measure Hub, follow
[Start here](https://commonmeasure.ai/docs/hub/start-here/) from sign-in to
your first delivery, or
[connect an existing edge](https://commonmeasure.ai/docs/hub/connect-commonmeasure/).
How these pages are written and published is `DOCUMENTATION.md` in the
repository.

Get started:

- [`docs/GETTING-STARTED.md`](GETTING-STARTED.md): from the installer to a
  recorded session, policy, the console, import, the relay and a build from
  source.

Use:

- [`docs/CONSOLE.md`](CONSOLE.md): the operator console, section by section.

Integrate:

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

Contracts, the authoritative formats. Each names the domain that owns it:

- [`docs/contracts/artifact-association.md`](contracts/artifact-association.md)
  (Edge): session declarations, saved-file/Git snapshots and portable offline
  bundles.
- [`docs/contracts/canonical-json.md`](contracts/canonical-json.md) (Edge): the
  one serialisation every hash is computed over.
- [`docs/contracts/comparison-export.md`](contracts/comparison-export.md)
  (Edge): the offline report bundle the console exports from a retrieval
  comparison.
- [`docs/contracts/experiment.md`](contracts/experiment.md) (Edge): the
  experiment declaration.
- [`docs/contracts/host-integration.md`](contracts/host-integration.md) (Edge):
  what a host must supply to integrate.
- [`docs/contracts/run-output.md`](contracts/run-output.md) (Edge): the run
  directory.
- [`docs/contracts/session-evidence.md`](contracts/session-evidence.md) (Edge):
  every record a session writes.
- [`docs/contracts/simpleqa-benchmark.md`](contracts/simpleqa-benchmark.md)
  (Edge): the local SimpleQA benchmark.
- [`docs/contracts/source-policy.md`](contracts/source-policy.md) (Edge): the
  policy file, what the loader accepts and refuses, and the schema and
  vectors published beside it.
- [`docs/contracts/directory-enrolment.md`](contracts/directory-enrolment.md)
  (Hub): how an edge and a directory selection enrol with one organisation.
- [`docs/contracts/enrolment.md`](contracts/enrolment.md) (Hub): `connect`
  and `disconnect`: the token exchange, key registration, the directory
  proof, revocation.
- [`docs/contracts/fleet-status.md`](contracts/fleet-status.md) (Hub): what
  an edge reports about the policy it applies.
- [`docs/contracts/policy-envelope.md`](contracts/policy-envelope.md) (Hub):
  signed policy distribution.
- [`docs/contracts/bot-identity.md`](contracts/bot-identity.md) (Network):
  `CommonMeasureBot`: the per-edge key, signed requests, the key directory,
  the agent card and revocation.
- [`docs/contracts/grant.md`](contracts/grant.md) (Network): the grant object
  and its entitlement binding. Planned: nothing implements it.
- [`docs/contracts/instance-registration.md`](contracts/instance-registration.md)
  (Network): registration, renewal and closure of a working instance.
- [`docs/contracts/onward-delivery.md`](contracts/onward-delivery.md)
  (Network): how the hub finds a content owner for an event and delivers it
  to the owner's endpoints.
- [`docs/contracts/supplier-credentials.md`](contracts/supplier-credentials.md)
  (Network): custody and release of an organisation's supplier key to a
  hosted edge. `contracts/supplier-credentials-release-vector.json` is one
  signed release request fixed by value, which the edge's signer and the
  hub's verifier are each tested against.
- [`docs/contracts/telemetry-projection.md`](contracts/telemetry-projection.md)
  (Network): what the relay sends to a receiver as Content Telemetry, under
  which clearances, and how delivery is retried.
- [`docs/contracts/extension-manifest.md`](contracts/extension-manifest.md)
  (Marketplace): the manifest the hub's Extension Studio accepts. No edge
  reads it yet.
- [`docs/contracts/processor.md`](contracts/processor.md) (Extensions): the
  processor contract, a stage around a ContextJob.
- [`docs/contracts/provider.md`](contracts/provider.md) (Extensions): supply
  adapter capabilities.
