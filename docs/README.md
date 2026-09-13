---
title: Documentation
---

# Documentation

These pages are the one source of the product's documentation. They are read
in the repository and they are published as the documentation section of the
product's website, which is built from this directory alone. The rules that
make the second possible are stated at the end of this page.

## Pages

Start with the [door page](guide/what-an-agent-meets-at-the-door.md), which
follows one agent through four recorded fetches, then the
[getting started walkthrough](GETTING-STARTED.md), which takes a clean
checkout to a session that records crossings of your own.

Product:

- [`docs/GLOSSARY.md`](GLOSSARY.md): every defined term in one line.
- [`docs/GETTING-STARTED.md`](GETTING-STARTED.md): the tested walkthrough from
  a clean checkout through install, a recorded session, policy, the console,
  import and the egress boundary.
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

- [`docs/guide/what-an-agent-meets-at-the-door.md`](guide/what-an-agent-meets-at-the-door.md):
  four fetches under the product, read from their evidence.
- [`docs/guide/context-window-optimisation.md`](guide/context-window-optimisation.md):
  what should enter a context window and how to know it helped.
- [`docs/guide/state-of-the-evidence.md`](guide/state-of-the-evidence.md): the
  evidence behind the guide, graded claim by claim.
- [`docs/guide/measurements/README.md`](guide/measurements/README.md): the two
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

Internal, kept in the repository and not published:

- `docs/qa/`: the gate procedure and the open defect list.
- `docs/knowledge-base/`: provider verification, the supplier landscape,
  source declarations, the naming record and unverified assumptions.

## Rules for the website build

The website's documentation section is built from a copy of this directory
and nothing else, so every rule the build needs is here.

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
   measurements page describes.
6. **`docs/qa/` and `docs/knowledge-base/` are not published.** They are
   working documents for people changing the repository. The build excludes
   the two directories, and every page in them also carries `draft: true` in
   its front matter, so a build that takes the whole directory still leaves
   them out.
7. **Sidebar order** follows the groups above: the door page and the
   walkthrough first, then product, demonstrations, guide, contracts.
