---
domain: shared
audience: contributor
---

# Shared source policy form

`commonmeasure-policy-form/v1` is an offline ES module and stylesheet.
`docs/contracts/source-policy.schema.json` owns the vocabulary; the form reads
mode controls from its definitions. The focused controls edit default and
scope modes and add denied hosts. Full policy JSON retains access to the rest
of the schema without a second handwritten form. Server validation is
mandatory for JSON drafts, review and commit. The browser is not an admission
or policy validation engine. Documents containing integers outside JavaScript’s
exact integer range are refused by the form, with file editing as recovery;
they must never be rounded by an unrelated edit.

The form keeps a complete snapshot and an ordered draft, renders edit and
structured before/after review, and calls explicit host adapters. Read views,
authorisation, historical forecasts and Hub source checks belong to the host.
No framework, CDN, external font, inline script, HTML injection or eval is used.
Rust embeds these source files and the authoritative schema directly.

## Build and pin

From the product checkout:

```sh
node --test console/policy-form/test/
node console/policy-form/package.mjs --check
node console/policy-form/package.mjs --output target/policy-form
```

After a reviewed source change, `node console/policy-form/package.mjs --write`
updates the manifest. The artifact identity is its contract plus the SHA-256
of the JSON-encoded ordered file entries in `manifest.json`. Individual file
hashes include the schema. Packaging copies bytes without compiling them and
adds the checkout revision to `UPSTREAM.json`. Run packaging from the committed
revision being pinned; the reported source revision must match those bytes.

Hub should vendor the three output files and `UPSTREAM.json` together using
its source-policy pinning convention. Verify each file against its SHA-256
and keep the exact Edge source revision in the pin. Import the vendored module,
serve the stylesheet under the existing CSP, and load the pinned schema as
JSON. Do not reimplement the controls in Svelte. In a Svelte component, call
`mount` after its root exists and call the returned `destroy()` on unmount.
The Hub adapter and its real integration tests are owned by the Hub lane;
this Edge round does not establish that integration.

## Host interface

```js
import { mount, contract } from './policy-form.mjs';
const editor = await mount(element, {
  schema,
  adapter: {
    async load() { return { document, revision, writable, reason }; },
    async check({ document, revision }) { /* validate without committing */ },
    async commit({ document, revision }) { /* validate, check revision, commit */ }
  },
  commitLabel: 'Save', // Hub supplies 'Publish'
  consequence: 'The host supplies the consequence of this commit.',
  onCancel() {},
  onCommitted(result) {}
});
```

Adapters reject with an `Error` whose message is safe to display as plain text.
Revision is an opaque host token and remains the loaded revision throughout
the draft. `check` and `commit` must enforce host authorisation; `commit` must
also check current state and revision atomically. A rejected operation keeps
the draft and focuses its error. Hub supplies its existing authenticated load,
validation and publication routes, permission checks and publish consequence;
Edge supplies `/api/policy`, `/api/policy/check` and `/api/policy/save`.
The latter accept JSON string fields `revision` and `document`; responses use
the existing outcome object (`kind`, `status`, `notice`, `revision`). Edge
checks drafts through the runtime loader in a temporary directory under the
selected home, removed on completion, and saves under the policy lock.

Use the existing design tokens `--edge`, `--surface`, `--action`, `--on-action`
and inherited text/font colours in both hosts. Buttons are at least 44px.
