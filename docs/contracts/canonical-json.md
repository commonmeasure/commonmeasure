---
title: Canonical JSON contract
---

# Canonical JSON contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

This contract defines the one serialisation every sealed hash and digest in
Common Measure is computed over: the JSON Canonicalization Scheme of RFC 8785.
Terms such as manifest, evidence log and policy identity are defined in
[`docs/GLOSSARY.md`](../GLOSSARY.md).

A hash over a JSON document is only reproducible when the bytes hashed are a
function of the value and nothing else. A general-purpose serialiser does not
give that: the order it writes object members in, and the way it prints a
number, are properties of the library and its build rather than of the
document. RFC 8785 settles each of those, so a reviewer holding the document
recomputes the hash with any conforming serialiser and SHA-256, and a hash
does not move because a dependency elsewhere in the build changed.

## The rule

- No whitespace between tokens.
- Object members sorted by the UTF-16 code units of their names, at every
  depth. This is not byte order: a name outside the Basic Multilingual Plane
  sorts by its surrogate pair, so an emoji sorts before `U+FB33`.
- Arrays in the order given. An array whose order carries no meaning is
  sorted by the caller before it is serialised; the pre-image says which
  arrays those are.
- Strings with `"`, `\` and the control characters below `U+0020` escaped —
  `\b`, `\t`, `\n`, `\f`, `\r`, and otherwise `\u00xx` in lower-case hex —
  and every other character written literally, the solidus included.
- Numbers as ECMAScript's `Number.prototype.toString` writes them: the
  shortest digits that read back as the same IEEE 754 double, plain notation
  for a magnitude from `1e-6` up to `1e21`, exponent notation with an
  explicit sign outside that range, and `0` for both zeros. A weight declared
  as `1.0` is sealed as `1`.

Every number is treated as a double, as RFC 8785 requires of an I-JSON
document. An integer beyond 2^53 loses precision here exactly as it does in
any JavaScript reader; nothing sealed carries one.

A digest is `sha256:` followed by the lower-case hex SHA-256 of the canonical
text in UTF-8.

## Where it applies

Every seal in the product, with no exception and no second form:

| Seal | Named in |
|---|---|
| the run manifest hash | [`docs/contracts/run-output.md`](run-output.md) §Manifest |
| a replay recording's response hash | [`docs/contracts/run-output.md`](run-output.md) §Replay binding |
| the relay's derived session and event ids | `conformance/README.md` |
| the policy digest | [`docs/contracts/fleet-status.md`](fleet-status.md) §Policy digest |
| the effective-policy identity | [`docs/contracts/fleet-status.md`](fleet-status.md) §Policy identity |
| the signed policy envelope's signature base | [`docs/contracts/policy-envelope.md`](policy-envelope.md) |

A hash over bytes that are not a JSON document — a retrieved source's text, a
skill's entrypoint, the evidence log file, a processor's rule set — is a hash
over those bytes as they stand and this contract does not reach it.

## Implementations

`crates/commonmeasure-types/src/canonical.rs` is the edge's, and the only one
in this repository: every sealing point calls it. The hub implements the same
rule in its own crate, from this contract and RFC 8785 rather than from a
shared library, because the two sides agreeing by construction would prove
nothing about either. Both carry the RFC's own test vectors, including the
worked example of §3.2.3, the member-ordering example, and the number
serialisation table of Appendix B.

The relay's derived ids are UUID version 5 over the canonical text of a
pre-image naming the record the id stands for, in the relay's fixed
namespace, so an id is recomputed the same way as a digest.

An edge key's id is the RFC 7638 JWK thumbprint, which hashes the key's
required members in lexicographic order with no whitespace. That is the form
this rule produces for that object, so the pre-image is built here too and
the thumbprint has no canonicaliser of its own on either side.

## Shared policy vectors

The edge digests a policy after its loader has parsed and re-serialised it;
the hub digests the policy as it was submitted. The two must produce the same
digest for the same document, whatever shape that document is in, and these
two vectors are what each side tests against. The second is the first with a
defaulted field left out, which is a different document and digests as one.

Written in full:

```json
{"policy_mode":"strict","constraints":[{"kind":"denied_source_host","host":"paywall.example"},{"kind":"maximum_acquisition_cost","amount":{"currency":"GBP","micros":2500000}}],"scopes":[{"match":"code/ozone","policy_mode":"observe","allow_telemetry_egress":false}],"allow_private_hosts":false,"record_internal_prefixes":["https://rag.example.internal/"]}
```

`sha256:2ad528b05af5e72e090362a5a608e1f8b72c42ba4ae64536f98695eaff2bd103`

With `allow_private_hosts` left out:

```json
{"policy_mode":"strict","constraints":[{"kind":"denied_source_host","host":"paywall.example"},{"kind":"maximum_acquisition_cost","amount":{"currency":"GBP","micros":2500000}}],"scopes":[{"match":"code/ozone","policy_mode":"observe","allow_telemetry_egress":false}],"record_internal_prefixes":["https://rag.example.internal/"]}
```

`sha256:e4d73e9d4be89337df7fe24c7acadb424daf22afea6ff1889271db7d8e8e0141`

A change to either digest is a change to the wire between the two products
and lands on both sides together.
