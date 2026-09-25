---
domain: edge
audience: contributor
---

# Demo skill inputs

This directory holds two different things: a skill bundle under review (the
job's target) and the operator's skill catalogue (the routes). Terms like skill, run, plan
and coverage are defined in `docs/GLOSSARY.md`.

## `geo-lookup/` — the target, not a supplier

A skill bundle **under review**. It is demo *input*, in the same position as
`demo/corpus/`: the thing the job asks a question about, not a route the
job could take. Nothing here is invoked.

It carries one deliberate and realistic fault: its frontmatter declares
`version: 1.2.0`. The Agent Skill format has no version key (the allowed
frontmatter properties are `name`, `description`, `license`,
`allowed-tools` and `metadata`), so a bundle that declares one is not
publishable as written. Authors make this mistake because most other
packaging formats carry a version. Because the format has no version key,
`acquisition.invocation.declared_version` is null in every skill invocation
record.

The bundle is also not packaged for distribution: it has no
`.codex-plugin/plugin.json`. That is a second, different finding about the
same directory. The comparison exists to show which of the two each route
reports.

## `catalogue.json` — the operator's declaration, machine-specific

The skill catalogue for the recorded run (`COMMONMEASURE_SKILL_CATALOGUE`). It
declares the two real third-party bundles the run invokes: what to execute,
with which absolute interpreter, with which arguments, under what timeout and
output cap.

**It names absolute paths on an operator's machine**, so `just
skills-example` runs only where those paths exist; the committed catalogue
names a neutral home (`/home/op`), and a run record shows the paths of the
machine that produced it. The bundles are licensed third-party content and
are not copied into this repository; an operator regenerating the run edits
`root` for each entry to where their own installation sits. A skill is
supply the operator admits from their own machine, not content a supplier
serves, so the catalogue names the operator's own paths. The recipe passes
`--live`, executes both skills locally, and writes to `output/skills` under
`COMMONMEASURE_PRIVATE_EVIDENCE`, refusing to run without it
(`CONTRIBUTING.md`).

The two skills, both installed as Codex system skills:

| Catalogue name | Entrypoint | What it answers |
|---|---|---|
| `skill-creator` | `scripts/quick_validate.py` | Is this a valid Agent Skill? Frontmatter verdict, exit 0 or 1 |
| `plugin-creator` | `scripts/validate_plugin.py` | Is this a distributable plugin? Packaging verdict, exit 0 or 1 |

Both execute offline, need no credential, and receive **identical input
bytes**: the job's prompt is the target path, substituted into each skill's
declared argument vector at the one `{job}` position. They differ only in
procedure, so they are two routes to one job and not one route run twice.

## The rubric, and when it was written

The suite's coverage rubric (`demo/jobs/skill-publishable.json`) was written
before either skill was run against `geo-lookup/`: it is the operator's
statement of what a complete answer to "is this publishable, and what must
change?" would contain. Its phrases were chosen with knowledge of the two
validators' general vocabulary, because a rubric in words no candidate
could produce would measure nothing. It does not name either skill's actual
output for this target.

Two of its six items (`remedy-stated`, `verdict-on-publishability`) were
expected to go uncovered by every route, and did: neither skill states the
remedy in words or reaches an overall verdict on publishability. The
uncovered items are a measured result about the routes, not a run failure.
