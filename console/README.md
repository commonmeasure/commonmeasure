---
domain: edge
audience: contributor
---

# The operator console

This directory holds the console's stylesheet, scripts, brand SVGs and
fonts. The pages are produced by `crates/commonmeasure-console` and exist
only through the binary: there is no plain-file fallback, because the same
code renders the markup and answers the JSON API behind it. What each
section shows and does is `docs/CONSOLE.md`.

```sh
cargo run -p commonmeasure-cli -- serve                    # http://127.0.0.1:4173
cargo run -p commonmeasure-cli -- inspect <run-directory>  # a run dossier, in the terminal
```

## Assets

The vendored `htmx.min.js` (htmx 2.0.7, BSD Zero Clause licence) drives
fragment swaps. The local `theme.js` handles appearance and sidebar preferences. The htmx
file is the unmodified upstream release, verifiable offline against this hash:

```
source  https://unpkg.com/htmx.org@2.0.7/dist/htmx.min.js
size    51076 bytes
sha256  60231ae6ba9db3825eb15a261122d5f55921c4d53b66bf637dc18b4ee27c79f9
```

`styles.css` carries the Hub's canonical application palette: white grounds,
navy text, pale-grey panels and cobalt actions in light mode; ink grounds,
pale-blue links and coral actions in dark mode. Data surfaces remain white
in light mode. The inset sidebar switches the Edge mark between the
website's approved colourways. Controls use 10px corners, small details 6px and panels 14px.
Primary actions have a 44px minimum height; compact fields use 36px. Status colours retain labels and remain distinct from actions.

Hub and Edge share a 15rem vertical sidebar. At 52rem and below, a Menu
control opens the same vertical links above the content. The active page has
a filled row and a left marker. The collapse button reduces the sidebar to a
4rem rail, keeping navigation icons and the active-page marker. Hover or focus
an icon for its label. The console remembers the collapsed state locally.
Appearance stays at the bottom
left in both sizes, or after the links inside the mobile Menu. Long navigation
scrolls above the footer. Navigation works without JavaScript; collapsing and
saving preferences require the local script.

The first visit uses light mode. The appearance button switches light and dark,
remembered in local storage by `/theme.js`; if storage is unavailable, the choice
lasts for that page. Console-served guides share this control and preference.
Standalone guide exports stay script-free and light.

The console serves Geist and Geist Mono from embedded WOFF2 files under
`/fonts/`; `fonts/Geist-OFL.txt` carries the licence. Logo, favicon, CSS, fonts
and the htmx and appearance scripts work offline. The content security policy permits images and fonts
only from the same origin. The console-served guide uses the same local fonts;
standalone exported guides retain system fallbacks.

The website owns `design-tokens/tokens.json`. The vendored release in this
repository contains the generator and values for the console and guides.
`node design-tokens/generate.mjs --check` (also `just tokens`) detects drift;
`just gates` includes it. See `design-tokens/README.md` for cross-repository sync.
`crates/commonmeasure-console/tests/design_tokens.rs` checks contrast and the
embedded palettes against that source offline. Rust builds need no Node or
network access.

## The guide

`/guide` and `/guide/state-of-the-evidence` are rendered at first request
from `docs/guide/*.md` with `docs/guide/guide.css` inlined
(`crates/commonmeasure-console/src/guide.rs`). The only relaxation the
serving layer makes to its content security policy is admitting each
page's own inline stylesheet by hash. `commonmeasure guide <dir>` writes
the public variant, with every product panel stripped and no product name
in the output.

## Local visual fixture

`cargo run -p commonmeasure-console --example design_preview` serves the real
console at `http://127.0.0.1:4187` with three synthetic sessions in a temporary
directory. It has no provider runner, credentials or relay. Policy-form writes
affect only that temporary directory. Stop the process when finished. This
fixture establishes rendering and local interactions, not provider integration.
