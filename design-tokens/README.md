---
domain: shared
audience: contributor
---

# Common Measure design tokens

The website repository owns `design-tokens/tokens.json`. Its versioned release
contains the palette, typography, spacing, geometry, focus and motion roles for
the website, Hub, Edge and embedded guides. This directory in Hub and Edge is a
vendored release; change the website source first. No package service or runtime
network request is involved. Node is needed to regenerate or verify CSS, not to
build or run the Rust console.

From any consumer repository root:

```sh
node design-tokens/generate.mjs
node design-tokens/generate.mjs --check
```

The generator owns the marked blocks in the consumer stylesheets. It rejects
local overrides of shared colours and foundations. Output records the release
version and SHA-256 of the exact source. Bump the version when changing tokens.

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
Application contrast gates cover text, controls, status fills and chart marks;
run them after a palette edit. Reduced-motion preferences override transitions.
