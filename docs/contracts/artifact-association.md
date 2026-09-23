---
title: Artifact associations and local snapshots
---

# Artifact associations and local snapshots

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

`commonmeasure artifact` records explicit session associations and captures saved
files or fixed Git trees. It exports an unsigned, self-contained JSON bundle
which can be verified without its original Edge, session directory or Hub.
A runnable synthetic example in `demo/artifacts/README.md` includes a saved file
and its exported bundle. The structural schema is `schema/artifact.v1.json`.
The runtime additionally checks canonical digests, reference equality, sorted
sets, evidence coverage, path safety and resource limits.

This first profile accepts agent and operator declarations. It does not observe
host edits, insert metadata into documents, sign snapshots, resolve remote records
or send evidence to Hub. Existing [batch C2PA signing](processor.md#independent-output-verification)
is a separate path.

## Local workflow

These command forms are exercised by the CLI integration tests. Choose a store
outside the file being captured. For a repository, keep its store beneath the
excluded root `.commonmeasure/` directory.

```sh
commonmeasure artifact init --store records
commonmeasure artifact associate --store records \
  --namespace urn:example:sessions --session session-one \
  --issuer urn:example:operator --edge local-edge \
  --actor urn:example:agent --basis agent_declaration --role drafting
commonmeasure artifact snapshot file answer.txt --store records
```

`init` returns a stable UUID URN. Repeating it reads the same identity; passing
`--artifact-id` with a different UUID URN refuses. `associate` returns a new record
UUID. Add another declaration for each session/role. `snapshot` returns
`snapshot_id`, `snapshot_digest` and the captured `snapshot` object. It selects
all declarations currently published in that store and retains previous snapshots.
The command records a bounded set, not a promise that later declarations are absent.

The following command forms require substituting the returned identifiers:

```sh
commonmeasure artifact snapshot file answer.txt --store records \
  --evidence ASSOCIATION_UUID=session.ndjson
commonmeasure artifact export --store records --snapshot SNAPSHOT_UUID \
  --output answer.txt.commonmeasure.json
commonmeasure artifact verify answer.txt.commonmeasure.json --file answer.txt \
  --expected-snapshot sha256:SNAPSHOT_DIGEST
```

Evidence selection is optional and repeatable. Each selection names an association
and a local NDJSON file. No session discovery or copying happens by default. Export
contains exactly the retained prefix bytes selected during capture; original log
paths are not retained in the bundle. Its contents can be private even if the
artifact is public. The caller chooses where to share that file. Existing export
files and symlinked parent directories are refused. Export uses bounded canonical
JSON and the same complete, synced, no-clobber publication as records; a successful
export fits the verifier's input limit. No automatic overwrite or network fallback
occurs.

For coding agents, use the same commands with a repository store:

```sh
commonmeasure artifact init --store .commonmeasure/artifacts/project
# Add declarations to that store with the associate command above.
commonmeasure artifact snapshot git . --revision HEAD \
  --store .commonmeasure/artifacts/project
commonmeasure artifact index --store .commonmeasure/artifacts/project > COMMONMEASURE.md
```

Export a returned snapshot using `artifact export`. Verify its bundle with
`artifact verify BUNDLE --repo . --revision HEAD`. Git must be installed and the
selected objects must be available locally. The supplied repository path must
name its root, not a subdirectory. Verification does not fetch missing
objects. The selected Git tree is the target; uncommitted work is outside scope.
The Markdown index is a generated view with canonical record paths, not another
source of authority. Regenerate it after new declarations or snapshots.

## Records and physical storage

Each record carries `record_version: "commonmeasure-artifact/1"`. Unknown versions
and unknown fields are refused by this deliberately small profile. A later schema
migration must create new records and retain original bytes; it cannot silently
rewrite an old signed or hash-pinned record.

```text
STORE/identity.json
STORE/associations/<record-uuid>.json
STORE/snapshots/<snapshot-uuid>.json
STORE/evidence/<sha256-hex>.ndjson
```

Identity contains the UUID URN and creation time. Association contains:

- `record_id`, `created_at`, `artifact_id`;
- `session.namespace`, exact original `session.id`, `session.edge.issuer` and
  `session.edge.installation_id`, with optional `session.host`;
- `role`: `research`, `drafting`, `review`, `transformation` or `unspecified`;
- `assertion.basis`: `agent_declaration` or `operator_declaration`, the declared
  `actor`, and fixed relation `session-associated-with-artifact`;
- `applies_to: {"kind":"working_artifact"}`. A snapshot explicitly selects it.

Issuer, namespace, installation ID and actor are declarations, not authenticated
identities. Use an explicitly local namespace when no registered identity exists.
Original Edge sessions, host tasks and Hub UUIDs remain distinct. The current
relay's UUID derivation does not include Edge identity; a bare Hub UUID is not a
globally unique portable session reference. Many records can name one session or
artifact. Reuse the same qualified session reference across different artifact
stores to represent one session contributing to several outputs.

Snapshot contains its UUID and capture time, artifact ID, `identity_digest`,
content binding, sorted `{record_id,digest}` association references and sorted
evidence references. `parents` is currently an empty array: automatic ancestry,
branch merges, retractions and region-specific relationships are not implemented.
Previous independent snapshots remain in the store. New declarations do not alter
an older snapshot's selected set.

Concurrent writers publish a complete, synced temporary inode using a no-clobber
hard link. A concurrent `init` reads the winning complete identity. Unsupported
filesystems or publication errors return failure. Immutable record names cannot
be overwritten by these commands; external edits are detected against retained
digests during export/verification. Symlinks, parent traversal and unexpected record
filenames are refused. Stores are ordinary local files, not protected against
another process with the same write privileges.

## Bindings and canonical bytes

Record digests use the existing [RFC 8785 canonicalisation](canonical-json.md)
and SHA-256. Duplicate JSON object keys, non-finite numbers and integers outside
the safe I-JSON range are refused in this profile, including evidence JSON. Strings
are preserved without Unicode normalisation. Arrays representing sets have an
explicit order: association UUID, evidence `(digest, association_id)` and inventory
path in case-sensitive UTF-8 byte order. Evidence bytes themselves are never
re-serialised for hashing.

`file-bytes/1` hashes every saved file byte, with `algorithm`, `digest` and
`byte_length`. It neither parses nor modifies a DOCX, PDF, image or other file.
Moving or renaming the same bytes preserves the binding. A save that changes ZIP
bookkeeping or only metadata still creates a different byte version. The store's
own files cannot be captured as their own artifact. Ordinary changes detected
during reading cause a refusal; no file-lock or adversarial-writer isolation is
claimed.

`repo-inventory/1` resolves a revision to one immutable tree and reads raw blobs.
Its SHA-256 preimage is the canonical JSON `inventory` object containing:

```json
{
  "scope": "repo-inventory/1",
  "excluded_prefixes": [".commonmeasure/"],
  "excluded_paths": ["COMMONMEASURE.md"],
  "entries": []
}
```

Each entry contains `path`, Git `mode`, `byte_length` and raw-blob SHA-256 `digest`.
All other tracked entries are enumerated. Added/deleted blobs, content changes
and executable-mode changes affect the digest. Symlinks, submodules, unresolved
Git LFS pointers, non-UTF-8/unsafe/colliding paths and empty subtrees are refused.
Replacement objects, filters, hooks and network transports are disabled.

The separate `source` object names Git object format and captured tree ID. It is
covered by the snapshot digest, but outside the inventory digest. Therefore adding
provenance files in a later commit can change the tree ID while preserving payload
integrity. Verify re-enumerates the supplied target tree and compares the entire
inventory and digest; it does not just check files already listed. The original
tree locator is not interpreted as the commit that will contain the snapshot.
If excluded paths hold executable or shipped product content, this scope is
unsuitable: it deliberately does not cover that content.

## Retained evidence and portable bundle

An evidence reference identifies its association, exact byte digest, scope
`evidence-bytes/1`, format `application/x-ndjson` and coverage. Coverage is a
nonempty complete prefix `[0,end)`, with `log_id`, record count, capture time and
`complete_lines: true`. Every line must be a JSON object with the matching
`session_id`, either directly or in the existing source-record `payload` envelope.
If both locations exist both must match. Missing/mismatched session IDs, truncated
lines or malformed JSON are refused. A detected append or edit during capture
also refuses; retry when the selected log is stable. One prefix per association
is supported.
The association binds the declared namespace and Edge; matching a log's session
ID does not independently authenticate its origin.

The bundle embeds `identity`, selected `associations`, `snapshot`, its external
`snapshot_digest`, and unique evidence objects `{digest,bytes}`. `bytes` is the
exact UTF-8 NDJSON represented as a JSON string. Verification checks its decoded
UTF-8 bytes, line/session constraints and coverage against the snapshot. All
referenced objects must exist and no extra association/evidence objects are allowed.
Deleting the original log, artifact store or Edge does not remove those exported
bytes. The artifact itself is supplied separately to verification.

No evidence selection means **not recorded**; it does not mean no sources were
used. A complete prefix proves only the supplied prefix, not session completeness,
absence of later events or truth of its contents. Nothing about exporting changes
licence, disclosure or retention permissions.

## Verification outcomes and limits

The report has `valid`, component checks, `signature: "absent"` and
`trust: "not_evaluated"`. A failed check returns a nonzero CLI status. Malformed,
unsupported or unavailable inputs fail explicitly. `--expected-snapshot` checks
a digest held separately from the bundle. Without such an external reference,
another writer can replace an unsigned bundle and all its digests consistently.
Neither case establishes an authenticated signer or declarant.

| Boundary | Limit |
|---|---|
| Encoded JSON bundle/input | 32 MiB |
| Individual stored JSON record | 1 MiB |
| Saved file | 100 MiB |
| Association/evidence references per snapshot | 1,024 |
| Evidence bytes | 8 MiB per object; 16 MiB per capture/bundle |
| Evidence lines | 100,000 per object; 1 MiB per line |
| Git tree listing | 10,000 entries including directory entries; 8 MiB output |
| Git raw blobs | 64 MiB each; 256 MiB total retained content |

Encoded-record/bundle limits can be reached before item limits, especially when
JSON escaping expands embedded evidence. The command refuses instead of truncating.
The v1 store reader also bounds the number of enumerated record files. File and
store paths must not contain symlink components; use a physical path when a system
temporary-directory alias is a symlink. No remote resolver, current revocation
check, automatic duplication/Save As detection or native Word save/export claim
is part of this slice.

Acquisition, admission, context entry, output reference, document insertion and
authorship remain distinct. These records associate sessions and bind supplied
bytes; they do not establish factual correctness, source support or licence rights.
