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

`styles.css` opens with the generated block from the design release vendored
in `design-tokens/`: the token declarations and, inside `@layer components`,
the application chrome Hub and Edge share (the shell and its collapsed rail,
the header and Menu disclosure at 52rem and below, the appearance control,
badges, alerts, cards, data tables, buttons, fields and read groups). The
rules after that block are Edge's own screens; being unlayered they win over
the chrome where the console departs from it. `design-tokens/README.md` lists
the classes and the sync with the website source; `node
design-tokens/generate.mjs --check` (also `just tokens`, part of `just gates`)
detects drift and runs the release's contrast gate.
`crates/commonmeasure-console/tests/design_tokens.rs` checks offline that the
embedded CSS is that release. Rust builds need no Node or network access.

Light mode is the default. Light, Dark and System are explicit choices under
Appearance in the sidebar and in the small-screen header, remembered in local
storage by `/theme.js`; if storage is unavailable, the choice lasts for that
page. The collapse button reduces the sidebar to a 4rem rail that keeps the
navigation icons and the active-page marker, with each icon's label on hover
or focus; the console remembers the collapsed state the same way. Navigation
works without JavaScript; saving a preference requires the local script.
Console-served guides share the theme preference through one Dark mode button.
Standalone guide exports stay script-free and light.

The console serves Geist and Geist Mono from embedded WOFF2 files under
`/fonts/`; `fonts/Geist-OFL.txt` carries the licence. Logo, favicon, CSS, fonts
and the htmx and appearance scripts work offline. The content security policy permits images and fonts
only from the same origin. The console-served guide uses the same local fonts;
standalone exported guides retain system fallbacks.

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

## Shared policy form

`policy-form/` owns the reusable form and reproducible package;
[`policy-form/README.md`](policy-form/README.md) specifies its adapter and Hub
pinning. `policy-host.mjs` is the Edge adapter. Both are served as same-origin
modules under the existing CSP. No Node runtime is needed by the binary.

The visual fixture defaults to synthetic directory policies. Set
`CM_DESIGN_PREVIEW_POLICY` to `empty`, `one`, `many`, `managed`, `absent`,
`malformed` or `deployment-error` to exercise other states. These declarations
exist only in the fixture's temporary home. `CM_DESIGN_PREVIEW_PORT` chooses
the loopback port.

After building the preview, the browser acceptance script uses an installed
Playwright module (no browser download is performed):

```sh
PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs node console/test/policy-browser.mjs
```

It starts only temporary synthetic fixtures and writes screenshots and results
to the ignored `target/policy-ux-evidence/` directory. It checks both themes at
390px and 1280px, error/read-only states, the real local edit/review/save path,
stale and invalid drafts, preservation and historical forecasting.
