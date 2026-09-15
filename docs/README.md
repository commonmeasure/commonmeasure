---
title: Documentation
---

# Documentation

Install and use Common Measure, understand its source records, and connect
your machines to Common Measure Hub.

## Pages

Start with the example, [Four fetches, two refused](guide/four-fetches.md),
which follows one agent through four recorded fetches, then the
[getting started walkthrough](GETTING-STARTED.md), which takes an
installed binary to a session that records crossings of your own.

For Common Measure Hub, follow [Start here](https://commonmeasure.ai/docs/hub/start-here/)
from sign-in to your first delivery, or [connect an existing edge](https://commonmeasure.ai/docs/hub/connect-commonmeasure/).

Product:

- [`docs/GLOSSARY.md`](GLOSSARY.md): every defined term in one line.
- [`docs/GETTING-STARTED.md`](GETTING-STARTED.md): the tested walkthrough from
  the installer through registration, a recorded session, policy, the
  console, import, the egress boundary and a build from source.
- [`docs/HUB.md`](HUB.md): the hub: what it is, what leaves a machine and
  what never does, how a machine joins and how policy arrives.
- [`docs/RELEASE.md`](RELEASE.md): how a release is cut, what it holds, and the
  installer.
- [`docs/FAIL-POLICY.md`](FAIL-POLICY.md): what the product does when a
  dependency is missing, a write fails or a measurement is unknown.

Reading the committed demonstrations:

- [`docs/READ-A-RUN.md`](READ-A-RUN.md): a run directory read in five minutes.
- [`docs/READ-A-COMPARISON.md`](READ-A-COMPARISON.md): the commerce comparison.
- [`docs/READ-A-HOLDOUT.md`](READ-A-HOLDOUT.md): the routing experiment and its
  holdout.
- [`docs/RUN-THE-DEMONSTRATION.md`](RUN-THE-DEMONSTRATION.md): the security
  demonstration, performed from a clean shell.

Guide:

- [`docs/guide/four-fetches.md`](guide/four-fetches.md): the example, four
  fetches under the product, read from their evidence.
- [`docs/guide/context-window-optimisation.md`](guide/context-window-optimisation.md):
  what should enter a context window and how to know it helped.
- [`docs/guide/state-of-the-evidence.md`](guide/state-of-the-evidence.md): the
  evidence behind the guide, graded claim by claim.
- [How agents use outside content](guide/untrusted-context.md): permission,
  reliability, prompt injection and the records operators and publishers need.
- [`docs/guide/measurements/README.md`](guide/measurements/README.md): the original
  measurements the guide reports, and how to rerun them.

Contracts, the authoritative formats:

- [`docs/contracts/host-integration.md`](contracts/host-integration.md): what a
  host must supply to integrate.
- [`docs/contracts/session-evidence.md`](contracts/session-evidence.md): every
  record a session writes.
- [`docs/contracts/run-output.md`](contracts/run-output.md): the run directory.
- [`docs/contracts/provider.md`](contracts/provider.md): supply adapter
  capabilities.
- [`docs/contracts/processor.md`](contracts/processor.md): the add-on contract.
- [`docs/contracts/experiment.md`](contracts/experiment.md): the experiment
  declaration.
- [`docs/contracts/canonical-json.md`](contracts/canonical-json.md): the one
  serialisation every hash is computed over.
- [`docs/contracts/fleet-status.md`](contracts/fleet-status.md): what an edge
  reports about the policy it applies.
- [`docs/contracts/policy-envelope.md`](contracts/policy-envelope.md): signed
  policy distribution.
- [`docs/contracts/source-policy.md`](contracts/source-policy.md): the policy
  file, what the loader accepts and refuses, and the schema and vectors
  published beside it.

Internal, kept in the repository and not published:

- `docs/qa/`: the gate procedure and the open defect list.
- `docs/knowledge-base/`: provider verification, the supplier landscape,
  source declarations, the naming record and unverified assumptions.

## Rules for the website build

The website imports this directory for edge documentation and the
Common Measure Hub repository's `commonmeasure-hub/docs/client/` for Hub guides. Both use the
same navigation and search at https://commonmeasure.ai/docs/. The rules
below govern this repository's pages.

1. **Every page is Markdown with front matter.** Each `.md` file opens with a
   YAML block carrying `title`, and optionally `description`. The first line
   after the block is a level-one heading repeating the title; the build drops
   that heading, because the site renders the title itself, and readers of
   the repository keep it.
   A page that lacks the block takes its level-one heading as its title.
2. **This page is the section's index.** `docs/README.md` maps to the root of
   the documentation section.
3. **Links between pages are relative Markdown links to `.md` files**, for
   example `` [`docs/GLOSSARY.md`](GLOSSARY.md) `` from a page in this
   directory and `` (../GLOSSARY.md) `` from a page one level down. Every such link resolves
   to a file inside this directory; the offline test suite checks that. The
   build maps each `.md` target to that page's route, lower-cased as the site
   generator names routes, so `GETTING-STARTED.md` becomes the
   `getting-started` page. A link's text is often the repository path in
   code style; that is deliberate, so a reader of either surface sees the
   same name.
4. **A code span naming a repository file outside this directory is not a
   link.** `ROADMAP.md`, `crates/…`, `demo/…` and the rest name files in the
   repository; the site links to the repository once and does not resolve
   them.
5. **Only `.md` files are pages.** Everything else here is source for a page
   or for the console, and the build ignores it: `guide/guide.css` is the
   stylesheet the console inlines when it serves the guide;
   `guide/measurements/` holds the two scripts and the two JSON results the
   measurements page describes; `contracts/source-policy.schema.json` and
   `contracts/source-policy-vectors.json` are the schema and vectors the
   source policy contract publishes.
6. **`docs/qa/` and `docs/knowledge-base/` are not published.** They are
   working documents for people changing the repository. The build excludes
   the two directories, and every page in them also carries `draft: true` in
   its front matter, so a build that takes the whole directory still leaves
   them out.
7. **Sidebar order** follows the groups above: the example and the
   walkthrough first, then product, demonstrations, guide, contracts.
