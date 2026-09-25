---
domain: edge
audience: contributor
---

# Documentation rules

How the pages under `docs/` are written so the website can publish them.
The website imports `docs/` for the edge documentation at
https://commonmeasure.ai/docs/, beside the Hub guides, and the pages under
`docs/guide/` as its guides section at https://commonmeasure.ai/guides/.
Two tests hold the rules below:
`crates/commonmeasure-cli/tests/docs_front_matter.rs` (front matter) and
`crates/commonmeasure-cli/tests/docs_paths.rs` (paths and links).

1. **Every Markdown document carries front matter.** A YAML block at the
   top names `domain`, the domain that owns the document (`edge`, `hub`,
   `network`, `marketplace`, `extensions` or `shared`), and `audience`
   (`operator`, `integrator`, `contributor`, `internal` or `reader`). A page
   under `docs/`, outside `docs/guide/`, whose audience is `operator` or
   `integrator` also carries `section`: `get-started`, `use`, `integrate` or
   `reference`. Contracts and the glossary are `reference`. No other
   document carries `section`. `docs_front_matter.rs` lists the exempt
   files, each with its reason.
2. **The audience decides publication.** Under `docs/`, `operator` and
   `integrator` pages are published under `/docs/`; under `docs/guide/`,
   `reader` pages are published under `/guides/`. A `contributor` or
   `internal` page is not published, so it does not belong under `docs/`.
3. **The title is the first heading** unless the front matter sets
   `title`. The site renders the title itself and drops a first heading
   that repeats it; readers of the repository keep the heading.
4. **`docs/README.md` is the index, not a page.** The site serves its own
   landing page at `/docs/`. The order in which `docs/README.md` links the
   pages is the order within each sidebar group; the groups are the
   `section` values.
5. **Links between pages are relative Markdown links to `.md` files**: the
   glossary is linked as `GLOSSARY.md` from a page in `docs/` and as
   `../GLOSSARY.md` from one level down. Each link resolves to a file
   inside `docs/`. The site maps each target to its route, lower-cased, so
   `GETTING-STARTED.md` becomes `/docs/getting-started/`. Link text is often
   the repository path in code style, so readers of either surface see the
   same name.
6. **A page in `docs/` names a file outside `docs/` in a code span**, not
   a link: `ARCHITECTURE.md`, `crates/…` and `demo/…` are repository paths.
   `docs_paths.rs` checks that each one a document names exists.
7. **Only `.md` files are pages.** `guide/guide.css` is the stylesheet the
   console inlines when it serves a guide; `guide/measurements/` holds the
   scripts and results its page describes;
   `contracts/source-policy.schema.json` and
   `contracts/source-policy-vectors.json` are published by the source policy
   contract beside it.
8. **A guide's address is its file name.** `guide/four-fetches.md` is
   published at `/guides/four-fetches/`, and `guide/measurements/README.md`
   at `/guides/measurements/`. A link to a guide from any page resolves to
   that address.
