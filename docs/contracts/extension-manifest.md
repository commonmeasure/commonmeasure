---
title: Extension manifest contract
domain: marketplace
audience: integrator
section: reference
---

# Extension manifest contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

- **Owner:** Marketplace.
- **Producer:** an extension's author, through the hub's Extension Studio.
- **Consumer:** the hub, which validates, stores and digests each version,
  and the public catalogue, which serves published versions. No edge reads
  the manifest.
- **State:** built in the hub, with its own tests on a real database.
  Nothing on the edge reads a manifest, activates an extension or receives
  settings: activation and settings are planned (MKT-2), and distribution to
  edges is planned (MKT-4).

An extension is anything distributed through the Marketplace: a processor, a
supplier adapter, a host integration or a service. The manifest describes
one version of it. It is metadata only: the hub stores no code, installs
nothing and runs nothing.

The manifest is a different document from the edge's own manifests: the
processor manifest compiled into the binary and sealed into a run
([`docs/contracts/processor.md`](processor.md)), the run manifest
([`docs/contracts/run-output.md`](run-output.md)) and the Content Telemetry
discovery manifest a source publishes
([session evidence §Manifest discovery](session-evidence.md#manifest-discovery)).

## The document

A version is submitted as `{"manifest": {…}}`:

```json
{"manifest": {
  "id": "pii-redactor",
  "name": "PII redactor",
  "version": "1.2.0",
  "summary": "Removes personal data from fetched text before admission.",
  "kind": "processor",
  "purposes": ["privacy"],
  "provider": "Example Ltd",
  "maintainer": "extensions@example.com",
  "runtime_binding": "in-process processor, admission stage",
  "permissions": ["read fetched text"],
  "requirements": ["edge 0.4 or later"],
  "documentation_url": "https://example.com/docs/pii-redactor"
}}
```

The wrapper and the manifest reject unknown members. Every member is
required except `documentation_url`, which may be omitted or `null`.

| Member | Rule |
|---|---|
| `id` | 1 to 80 bytes of `a-z`, `0-9` and `-`, not starting or ending with `-`. Unique within the organisation, and equal for every version of one extension |
| `name` | text, at most 160 bytes |
| `version` | `MAJOR.MINOR.PATCH`: three parts of 1 to 10 digits, no leading zero except `0` itself; no pre-release or build suffix |
| `summary` | text, at most 2,000 bytes |
| `kind` | `processor`, `supplier_connector`, `host_integration` or `service` |
| `purposes` | at most 16 texts of at most 512 bytes |
| `provider` | text, at most 160 bytes |
| `maintainer` | text, at most 160 bytes |
| `runtime_binding` | text, at most 512 bytes; how the extension is meant to run, as the author states it |
| `permissions` | at most 16 texts of at most 512 bytes |
| `requirements` | at most 16 texts of at most 512 bytes |
| `documentation_url` | an `https` URL with a host and no user name or password, at most 2,048 bytes; stored, never fetched |

A text is not empty after trimming, is measured in UTF-8 bytes and contains
no control character. Arrays may be empty; duplicates are not checked.

The kinds map to the edge's contracts: `processor` to
[`docs/contracts/processor.md`](processor.md), `supplier_connector` (a supply
adapter) to [`docs/contracts/provider.md`](provider.md) and
`host_integration` to [`docs/contracts/host-integration.md`](host-integration.md).
`service` has no edge contract. The manifest does not bind an extension to
any of these contracts: `runtime_binding`, `permissions` and `requirements`
are the author's statements and the hub checks none of them.

A manifest that breaks a rule is refused with `400` and a JSON body
`{"detail": "…"}`, for most rules `invalid or oversized extension manifest`.
A missing, unknown or mistyped member is refused with `422` and the
framework's own message, before validation. The manifest is validated before
the caller's authority is checked, so a member who is not an author is
refused `400` for an invalid manifest and `403` for a valid one.

## Size

The manifest's canonical text (§Digest) is at most 48 KiB
(49,152 bytes): `400`, `manifest exceeds 48 KiB`. The request body is at most
64 KiB. The member limits keep an ordinary manifest well under 48 KiB; only
text dense in characters JSON escapes reaches it.

## Digest

The hub computes two digests; an author cannot supply either. Each is
SHA-256 over RFC 8785 canonical JSON, written with the `sha256:` prefix, as
[`docs/contracts/canonical-json.md`](canonical-json.md) defines:

- the **manifest-version digest**, over the manifest as parsed, with an
  omitted `documentation_url` as `null`. It is computed when a version is
  created. Whitespace and member order in the submission do not change it.
  Studio versions, the platform's review list and the public catalogue
  return it.
- the **approved-configuration digest**, over the configuration an
  organisation owner approves or withdraws with (§Lifecycle), computed when
  the decision is recorded and stored with it. Studio versions return it
  with the version's latest decision.

Arrays keep their submitted order in both, so reordering
`allowed_edge_ids` or `purposes` changes the digest.

[`docs/contracts/extension-manifest-vector.json`](extension-manifest-vector.json)
holds the example manifest above and the example configuration below, each
with its canonical text and digest. The edge's test
`crates/commonmeasure-types/tests/extension_manifest_vector.rs` recomputes
them and checks that they match the examples on this page; the hub pins a
copy. A change to either digest lands on both sides together.

## Versions

A version is immutable once created: the hub has no route that changes or
deletes a version, a decision, a submission, a review or an admission. Any
unused version number may be added, in any order, and every member but `id`
may change between versions, `kind` included. A new version starts with no
decision and no review. An extension's listing shows the name, summary and
kind of its most recently created version, not its highest version number.

An organisation may hold 100 extensions and 100 versions in all (`409` past
either).

## Lifecycle

Versions, decisions, submissions, reviews and admissions are append-only, and
the latest decision, review and admission decide. Publication is one current
state per version, and authorship is granted and removed.

| Step | Who |
|---|---|
| create an extension with its first version, or add a version | an organisation owner, or a member the owner has made an author |
| grant or revoke authorship | an organisation owner |
| approve or withdraw approval of a version for the organisation, with the edges and operations it may use and its data scope | an organisation owner |
| submit a version for public review | an author or owner; a submission cannot be withdrawn |
| admit the provider organisation | a platform administrator who is not a member of, author in or submitter from that organisation |
| review a submitted version | a platform administrator who is not a member of, author in or submitter from the provider organisation |
| publish or withdraw a version | an organisation owner; publishing needs an approved review and an admitted provider (`409` otherwise) |

Each step is taken by a signed-in person, never an API key or an impersonated
session. An organisation's approval is pinned to one version and therefore to
its digest. Its configuration (`allowed_edge_ids`, each an active edge key of
the organisation, at most 100; `allowed_operations`, at most 32 texts of at
most 512 bytes; `data_scope`, a text of at most 2,000 bytes) is required to
approve and to withdraw, and is stored beside the approval with its own
digest (§Digest). It is not part of the manifest or the manifest's digest.
Nothing sends it to an edge. An example:

```json
{"allowed_edge_ids": ["edge-example-1"], "allowed_operations": ["redact fetched text"], "data_scope": "fetched text of admitted sources"}
```

A version's review status is `private`, `submitted`, `approved` or
`rejected`. It is available where it is published, its latest review is
approved and its provider's latest admission stands; revoking either removes
it from the catalogue.

## Public catalogue

`GET /api/v1/marketplace/extensions` needs no sign-in. It lists up to 200
available versions, newest first, every available version of an extension
included. Each row carries the extension's hub identifier, the provider
organisation's name, the whole manifest as submitted, its manifest-version
digest and the version's identifier. The manifest is served as the hub
parsed and stored it: an omitted `documentation_url` appears as `null`, and
member order is not kept. A version is available only while its provider
organisation is open. The whole manifest is public: an author must not put
credentials or private detail in it.

## Planned

- **Compatibility.** No member states the edge versions, hosts or contract
  versions an extension works with. `requirements` is free text.
- **Settings.** No member declares a settings schema. MKT-2 adds per-scope
  activation and settings, and mandates carried in the signed policy envelope
  ([`docs/contracts/policy-envelope.md`](policy-envelope.md)).
- **Distribution and execution.** MKT-4: an approved version reaches the edges
  in scope and runs there. Until then the edge runs only what is compiled into
  it.
