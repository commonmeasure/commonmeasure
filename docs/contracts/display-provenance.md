---
title: Display provenance
domain: edge
audience: integrator
section: reference
---

# Display provenance

This document is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

A display that `context_show` renders against a signed release
([host integration](host-integration.md) §Displaying a reply against a
release) is a second-order knowledge product: made from first-order
products, the releases it was rendered against, and read by someone who
may use it under terms of its own. In the session path the edge never sees
the model's free text; what it delivers is the display. This contract
defines the one C2PA manifest the edge signs for each display, where the
manifest is kept, what the source record says of it, and how the terms of
each release are carried and combined.

The `output-provenance` processor ([processor](processor.md#status)) signs
both the batch path's answer labels and these display manifests. Only the
shared code is the same; the batch path's behaviour is unchanged.

## The manifest

One C2PA manifest per rendered display, built from the record alone and
signed with the certificate the operator configures:

- **Hard binding.** `c2pa.hash.data` over the NFC-normalised plain text of
  the display, the second text block of the `context_show` result, with no
  exclusion. The text is never in the manifest, the record or the
  sidecar; `display_text_sha256` on the `display_rendered` record is the
  SHA-256 of the same normalised text, so the binding can be checked
  against a text without the text being kept.
- **Actions.** One `c2pa.created` action, `digitalSourceType`
  `http://cv.iptc.org/newscodes/digitalsourcetype/trainedAlgorithmicMedia`,
  the edge binary as `softwareAgent`.
- **Ingredients.** One `c2pa.ingredient` per release the display was
  rendered against (one per display today): `title` the URL the release
  was served at, `format` `application/x.signed-release+json`,
  `relationship` `inputTo`, `instance_id` the release's `content_hash`
  (SHA-256 of its text), `description` the grade the crossing was recorded
  at (`mediated` for every `context_fetch` delivery), and in `metadata`
  `ai.commonmeasure.sidecar_digest`, the canonical digest of the sidecar as
  served, so the signed fields are bound beside the text. Where the release
  carried its own C2PA store and the release verifier found it `untrusted`
  or `trusted` (§The release's own manifest), the ingredient carries that
  store as its manifest data with the reader's validation results; the
  source's identity and assertions travel inside it without the edge
  re-asserting them. An `invalid` store is not carried.
- **Training and data mining.** A `cawg.training-mining` assertion
  (version 1.1) derived from the ingredients (§Terms).
- **Terms.** An `ai.commonmeasure.terms` assertion (version 1) carrying
  each ingredient's terms and the statement derived for the display
  (§Terms).
- **No identity.** The manifest carries no `cawg.identity` assertion. The
  reader of the display adds nothing that names them, and the operator
  adds nothing either: the batch path's `cawg.identity` under
  `cawg.producer` is not used here. CAWG identity belongs to incoming
  content, inside an ingredient's own manifest. The claim is still signed,
  as C2PA requires, and the signing certificate names the claim generator
  (§Signing).

The manifest definition is recorded whole on the `output-provenance`
record, so the assertions can be re-derived from the record; the signed
bytes are not deterministic (a fresh manifest label and signature).

## Where it is kept

The manifest store is written as a sidecar beside the session log, never in
the display text:

```text
<home>/sessions/<session>.ndjson                  the session log
<home>/sessions/<session>.displays/<n>.c2pa       one display's manifest store
```

`<n>` is the `seq` of the `integrity-checker` record the display was
rendered from, which `display_rendered` names as `processor_record`. A
`seq` is the record's line in the log, counted under the append lock, so
two servers on one session never write two records, or two stores, under
one `<n>` ([session evidence](session-evidence.md) §Where). The store is
written whole to a temporary file of its own in the same directory and
linked into place, so a reader never finds part of one and two writers
never share a temporary file. A store already at the name is never
replaced: the link fails, the record abstains with a
`capability_unavailable` gap saying a store is already there, the
earlier store is left as it was, and the display is rendered without a
manifest. A store no record names is removed.

## Records

`context_show` appends, in order, the `integrity-checker` record, the
`output-provenance` record for the display and `display_rendered`
([session evidence](session-evidence.md) §Events). The provenance record is
written before `display_rendered` so that `display_rendered` can name it by
`seq`.

The `output-provenance` record has the shape every processor invocation
has (`contextops-processor-invocation/v1`,
[processor](processor.md#evidence)), under the display rules'
configuration digest:

| Field | Value |
|---|---|
| `decision` | `admit` where the manifest was signed, read back and written; `abstain` otherwise |
| `inputs` | the display (`<session>.ndjson#processor_invoked/<n>` with `display_text_sha256`), each release by URL with its `content_hash`, and each release's sidecar (`<url>#sidecar`) with its digest |
| `outputs` | where written: the sidecar reference `<session>.displays/<n>.c2pa`, relative to the `sessions` directory, with the SHA-256 of the store bytes |
| `detail.manifest_definition` | the definition signed |
| `detail.manifest_label`, `detail.manifest_bytes` | the active manifest's label and the store's length |
| `detail.signing` | `basis`, `certificate`, `key` (file references, never the key), `alg` (`ES256`) |
| `detail.signature` | the signature information the reader reports: issuer, common name, serial, time |
| `detail.validation` | `state` (`trusted`, `valid` or `invalid`), `trusted`, `summary`, `trust_list`, `trust_anchors_hash`, `trust_scope`, `checked_at`, `verifier` and every status the reader reported, from reading the store back against the display text before it is recorded |
| `detail.binding` | `method` (`c2pa.hash.data`), `normalisation` (`NFC`), `display_text_sha256` |
| `gaps` | on `abstain`, one `capability_unavailable` gap naming what is missing; where configured trust anchors could not be read, a `capability_unavailable` gap beginning `trust anchors unavailable` (§Signing) |

`display_rendered` gains `provenance_record`, the `seq` of that record
(null where none could be written), and `display_text_sha256`.

The `context_show` payload gains `provenance`:

| Field | Meaning |
|---|---|
| `record` | the `seq` of the `output-provenance` record |
| `manifest` | `reference` (the sidecar, relative to the `sessions` directory), `sha256` and `bytes`; null where no manifest was written |
| `validation` | `state` as the record has it, or `unavailable` where no manifest was written |

The store's bytes never travel in the tool result.

Neither record is a crossing. Neither is cleared for egress or projected
to any receiver ([telemetry projection](telemetry-projection.md)), so
nothing of a manifest leaves the machine through the relay. Nothing in the
fetch path, the anonymous pass exchange or the request to the source
changes: the manifest is written to the session home and named to the
reader only.

## Signing

The display manifest is signed locally with the certificate chain and
PKCS#8 ECDSA P-256 key the operator names in
`COMMONMEASURE_PROVENANCE_CERTIFICATE` and `COMMONMEASURE_PROVENANCE_KEY`,
the variables the batch path reads. The certificate names the claim
generator, never an operator or a person. The remote signer
(`COMMONMEASURE_PROVENANCE_SIGNER`) is not sent a display.

`COMMONMEASURE_PROVENANCE_TRUST_ANCHORS` names a PEM bundle of trust
anchors for reading manifests back: the display's own at write time and a
release's own store at the fetch. Unset, a manifest whose signature
verifies is `valid` and untrusted, and `trust_list` is `none configured`.
Set to a file that is missing, cannot be read or is not UTF-8, the
manifests are read without anchors, so none is `trusted`; the record
carries a `capability_unavailable` gap beginning `trust anchors
unavailable` that names the variable, the path and the error, and the
display record's `trust_list` is `configured trust anchors unavailable`.
The release verifier's record at the fetch carries the same gap. The
display is rendered either way.

Without a usable signing identity, or where the store cannot be signed,
read back or written, the record abstains with a `capability_unavailable`
gap naming the variables or the reason, the unsigned definition stays on
the record, no sidecar is left, the payload's `manifest` is null with the
state `unavailable`, and the display is rendered and returned regardless.

**Open design question: the production certificate.** This edge leaves
the certificate operator-configured. The candidate for a production edge is
a certificate the hub issues to the edge's enrolled key id at enrolment,
with that key id as its subject and nothing else, so that a manifest names
an edge, never an operator or a person, and a verifier holding the hub's
anchor can tell it is one. That belongs to the Anchor's design and is not
built.

## The release's own manifest

A signed release ([processor](processor.md#status) `release-verifier`) may
carry its own C2PA manifest store in its sidecar, outside the
Ed25519-signed fields:

- `sidecar.c2pa`: a standard base64 C2PA manifest store whose hard binding
  is `c2pa.hash.data` over the NFC-normalised release text, signed by the
  source with its own certificate, with the source's `cawg.identity`
  (X.509) and, where it declares one, its `cawg.training-mining`.

The release verifier reads the store offline against the release text,
under the operator's trust anchors where configured, and records on the
crossing's `release` record `c2pa`: `state` (`invalid`, `untrusted` or
`trusted`), `reason` where `invalid`, `manifest_sha256`, `manifest_label`,
`binding` (`matches`, whether the data hash matches the text, and
`same_text_as_signed`, whether that text is the one `text_sha256` names),
`signer` (common name, issuer, algorithm), `identities` (each
`cawg.identity` summarised: credential kind, roles, subject, issuer and
serial), `training_mining` as the source signed it, `statuses` and
`verifier`. The verifier's own record says `absent` where the sidecar
carries no store; the crossing's `release` then has no `c2pa`. No text is
recorded.

The C2PA state neither admits nor refuses the release: the Ed25519
signature does. A release whose store is `invalid` is still the verified
release its signature admits, and its display's ingredient carries no
source manifest.

## Terms

Each ingredient carries the terms its release was admitted under, and the
manifest carries a statement derived for the display. Three inputs are
read:

1. **The release's own terms.** `licence` (a string) and `terms` (free
   text), and, where the release states one, a structured `usage` block
   among the signed fields:

   ```json
   {"ai_input": "allow" | "disallow",
    "onward_commercial_product": "allow" | "disallow",
    "reporting": {"required": true, "destination": "<url or reference>"}}
   ```

   `reporting` is optional. A release without `usage` signs, reads and
   verifies as before.
2. **The source's declarations on the crossing.** The effective answer per
   category (RSL licences, `Content-Usage` and Content-Signal statements,
   most restrictive first; [source policy](source-policy.md)), the RSL
   licences read and the `Content-Usage` header.
3. **The operator's terms for the host.** The source policy's terms
   reference for the host and whether it requires reporting. Never the
   institution identifiers or the Anchor.

Per ingredient, each use is the sources' statements together: `disallow`
where any statement disallows, `allow` where one allows and none
disallows, `unknown` where none states anything. AI input reads the release's
`usage.ai_input` and the declarations' AI input answer; onward commercial
product reads `usage.onward_commercial_product` alone; reporting is
required where the release, an RSL licence or the operator's terms require
it, with every destination named.

An ingredient's declarations and host terms are those of the crossing
that delivered the release the display names. The server holds each
delivery with what its crossing recorded
([host integration](host-integration.md) §Releases held for display):
the same release bytes at the same URL fetched again under another
`Content-Usage` header or other terms is a second held delivery with its
own `acquisition_id`, and a display that names that handle signs that
crossing's terms; the earlier handle keeps its own. A `content_hash` that
matches more than one held delivery is refused as ambiguous: nothing is
displayed or signed for it, and the refusal lists the handles.

For the display, each use is the most restrictive over the ingredients:
`disallow` beats `unknown` beats `allow`. Unknown is never read as
permitted. Reporting is required where any ingredient requires it, with
every destination listed once.

### The terms assertion

`ai.commonmeasure.terms`, version 1:

```json
{
  "ingredients": [{
    "instance_id": "sha256:<release text>",
    "licence": "<the release's licence>",
    "source_terms": {"usage": {…} | null, "terms": ["…"]},
    "declarations": {"effective": {…}, "licences": ["<rsl url>"], "content_usage": "<header>" | null},
    "host_policy_terms": {"reference": "<policy terms reference>", "requires_reporting": false} | null,
    "usage": {"ai_input": "allow|disallow|unknown",
              "onward_commercial_product": "allow|disallow|unknown",
              "reporting": {"required": true, "destinations": ["…"]}}
  }],
  "derived": {"ai_input": "…", "onward_commercial_product": "…",
              "reporting": {"required": true, "destinations": ["…"]}}
}
```

The `ai.commonmeasure.` namespace is a working format this edge writes and
offers to the open standards bodies (SPUR and OA) through the commons. It
is not a standard and not Common Measure's to define; where a standard
gains a word for one of these terms, the assertion gives way to it.

### Mapping to RSL, C2PA and CAWG

| Term | RSL 1.0 | C2PA 2.x / CAWG 1.x | Carried as |
|---|---|---|---|
| AI input | usage token `ai-input` | `cawg.training-mining` entry `cawg.ai_inference` | the derived `ai_input`: `allow` as `allowed`, `disallow` as `notAllowed`, `unknown` as `constrained` |
| AI training | usage token `ai-train` | entries `cawg.ai_training` and `cawg.ai_generative_training` | the declarations' training answer, combined over the ingredients the same way |
| Data mining | no usage token | entry `cawg.data_mining` | no release term or declaration states it today, so `constrained` |
| Onward commercial product | the user type for commercial use; no usage token | none | `ai.commonmeasure.terms` only (gap) |
| Reporting duty and destination | the licence's reporting element and endpoint | none | `ai.commonmeasure.terms` only (gap) |
| Licence | the licence a site names | none in `cawg.training-mining` | `ai.commonmeasure.terms` only |

Every `constrained` entry's `constraint_info` is
`self#jumbf=c2pa.assertions/ai.commonmeasure.terms`, the terms assertion
in the same manifest.

Gaps by name, today: "onward commercial product" and "reporting duty and
destination" have no CAWG word; `cawg.training-mining` speaks of training,
mining and inference only, so a use of the display itself, such as selling
it onwards, cannot be stated in a standard assertion.

## Reading a display's manifest back

- `commonmeasure inspect <session>.ndjson` prints each display with its
  `<path>:<line>` citation, mode and non-zero counts, the integrity
  checker's record, and the manifest's reference, hash, length and
  validation state at write time from the `output-provenance` record, or
  the gap where none was written.
- `commonmeasure provenance <session>.ndjson` reads each display's sidecar
  back with the C2PA reader and prints, as JSON, the claim, the signer,
  the ingredients, the actions, the identity assertion (none), the
  training-and-mining and terms assertions and the validation, citing both
  records by `<path>:<line>`. The store's hash is checked against the
  record. Without `--text`, the claim and signature are validated and the
  hard binding is reported as verified at write time (the record's state)
  and not re-verified now. `--text <file>`, the display text the host
  kept, re-verifies the binding of the display whose `display_text_sha256`
  it matches. A sidecar that is missing, unreadable, does not hash to its
  record or does not verify fails the command, as does a `--text` that
  matches no display; `--trust-anchors` and `--require-trusted` apply as
  for a labelled output. With `--require-trusted`, a display without a
  manifest (none recorded, or the record abstained) fails the command, as
  does a log that records no display.
- Both commands resolve a display's records by the `seq` it names, among
  records of the processor it names. In a log written before a record's
  `seq` was its line, two writers could leave two records under one `seq`;
  such a display is reported as ambiguous with both citations, its
  manifest is not read, and `provenance` fails.
