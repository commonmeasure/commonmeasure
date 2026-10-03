---
domain: shared
audience: contributor
---

# Common Measure design release

The website repository owns `design-tokens/tokens.json` and `chrome.css`. Its
versioned release contains the palette, typography, spacing, geometry, focus
and motion roles for the website, Hub, Edge and embedded guides, and the
application chrome Hub and Edge render the same way. This directory in Hub and
Edge is a vendored release; change the website source first. No package
service or runtime network request is involved. Node is needed to regenerate
or verify CSS, not to build or run the Rust console.

From any consumer repository root:

```sh
node design-tokens/generate.mjs
node design-tokens/generate.mjs --check
node design-tokens/generate.mjs --check --verbose   # every contrast pairing
```

The generator owns the marked blocks in the consumer stylesheets. It rejects
local overrides of shared colours and foundations. Output records the release
version and the SHA-256 of each source file. Bump the version when changing
tokens or chrome.

The contrast gate runs first, on `tokens.json`, in both themes: primary text
at 7:1 on every ground, secondary text, links and the state colours at 4.5:1
including on their own tints, interactive borders and chart marks at 3:1. A
palette that fails an obligation is never written into a stylesheet, so
consumers carry no contrast checker of their own.

From the website checkout, synchronise explicitly named local repositories:

```sh
node design-tokens/generate.mjs --sync /path/to/hub-checkout /path/to/edge-checkout
node design-tokens/generate.mjs --check --compare /path/to/hub-checkout /path/to/edge-checkout
```

`--compare` verifies the source, generator and generated CSS in each consumer.
Each repository's normal check also verifies its own vendored release. The
consumer's `consumer.json` selects its adapters and stays local during sync.
Commit the source release and all matching consumer updates together across
repositories, and record the matching revisions.

## Roles

- `surface`: pale-grey grouped panels in light mode; ink panels in dark mode.
- `data`: white data/chart surfaces in light mode; ink in dark mode.
- `inset`: wells, table headings and subtle hover backgrounds.
- `edge`: decorative rules; `edge-strong`: visible control boundaries.
- `action` / `on-action`: primary control and label; `accent` / `on-accent`:
  links, focus and selected fills. They are independent in dark mode.
- `type-label`: 12px; `type-caption`: 13px; `type-body-compact`: 14px;
  `type-control`: 15px; `type-body`: 16px. `text-xs` means 13px everywhere.
- `radius-detail`: 6px; `radius-control`: 10px; `radius-panel`: 14px.
- `control-height`: 44px; `control-height-compact`: 36px. Compact application
  controls and dense data rows are explicit, while marketing keeps its larger
  headings and page spacing.

Status and chart palettes have their own roles. Colour never replaces labels.
Reduced-motion preferences override transitions.

## Themes

Every application stylesheet gets the same three blocks: `:root` and
`[data-theme='light']` carry the light palette, `[data-theme='dark']` the dark
one, and `:root[data-theme='system']` follows the operating system. A new
visitor is light; Light, Dark and System are explicit saved choices in both
applications.

## Chrome

`chrome.css` is inlaid into the Hub and Edge stylesheets inside
`@layer components`, so each application's own unlayered rules win where the
two differ. It holds the shell (`.app-shell`, `.shell-sidebar`, `.shell-nav`,
the collapsed rail under `:root[data-sidebar='collapsed']`, the header and
Menu disclosure at 52rem and below, `.rail-tooltip`), the appearance control
(`.theme-choice`), badges, alerts, cards, data tables, buttons, fields and
read groups (`.record-group`). Classes, not elements, carry the styles, so
either application opts in per element. Hub's Svelte primitives and Edge's
maud markup emit the same class names.
