# Live briefs

A brief is a bounded task an agent can land in one change. It serves one
roadmap package and carries no status: the package's boxes in `ROADMAP.md`
record what is built, and the brief is deleted when it lands. When this
directory holds only this file, nothing is queued beyond the packages
themselves.

Every brief is a markdown file in this directory with these second-level
headings, in this order:

1. `## Package`: the `WP-nn` heading in `ROADMAP.md` the brief serves, and
   the boxes it closes.
2. `## Goal`: what is true when the brief has landed, in one paragraph.
3. `## Files`: the repository paths the change touches.
4. `## Done when`: the tests, artefacts or commands that show it, named
   exactly.
5. `## Out of scope`: what the brief leaves to another brief or package.

A brief states current fact, follows `AGENTS.md` §Writing, and names no
path outside this repository. `crates/commonmeasure-cli/tests/work_briefs.rs`
checks the headings and the package reference.
