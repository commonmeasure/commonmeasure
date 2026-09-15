---
title: Source policy contract
description: The policy file every edge loads — its fields, what a scope and a principal replace, the order admission applies, every check the loader makes, and the schema and vectors a second implementation is held to.
---

# Source policy contract

This contract is licensed under CC-BY-4.0 (`docs/contracts/LICENSE`).

The source policy is the operator's policy file: which sources an agent may
take in, under what licence and at what cost, and which work may be reported
beyond the machine. This contract states what the file may contain, what
each field does, the order in which admission applies it, and every check
the loader makes before a policy is used. Terms such as scope, engagement,
principal and policy mode are defined in
[`docs/GLOSSARY.md`](../GLOSSARY.md).

There is one policy engine. The runtime's loader and admission check are the
implementation, and this contract describes them; it is not a second
implementation. Two artefacts published beside it let anything that writes
or checks a policy outside this repository be held to the same answers:

- `docs/contracts/source-policy.schema.json`, the JSON Schema of the file's
  structure (§The schema);
- `docs/contracts/source-policy-vectors.json`, accepted and refused
  documents and the rulings a policy produces (§The vectors).

`commonmeasure policy check <file>` loads a candidate through the loader and
prints what it accepted or the refusal, so the question of whether a policy
is valid can always be settled by the engine itself.

## The file

`$COMMONMEASURE_HOME/policy.json`, or `~/.commonmeasure/policy.json`. A JSON
object. An absent file is not an error: the mode is `observe`, every
crossing is recorded and nothing is refused. A file that is present and
cannot be loaded is an error, reported where the policy would be used, and
never replaced by a permissive default. That includes a field the installed
binary does not know, so a policy written for a newer binary refuses every
mediated crossing on an older one until that binary is upgraded.

On a managed edge the file is the desired policy the hub distributed, and
the edge replaces it only through
[`docs/contracts/policy-envelope.md`](policy-envelope.md).

## Fields

Top level:

| Field | Type | Default | What it does |
|---|---|---|---|
| `policy_mode` | `observe`, `prefer` or `strict` | `observe` | What happens to a breach (§Admission). |
| `constraints` | array of constraints | `[]` | The rules admission applies (§Constraints). |
| `scopes` | array of scopes | `[]` | Overlays selected by working directory (§Scopes and principals). |
| `principals` | array of principal bindings | `[]` | Overlays selected by operating-system user (§Scopes and principals). |
| `allow_private_hosts` | boolean | `false` | Whether the mediated tools may reach loopback and private-network addresses. |
| `refuse_on_pii` | boolean | `false` | Whether `strict` refuses a crossing for a personal-data finding on every source, not only on internal and private ones. |
| `record_internal_prefixes` | array of URL prefixes | `[]` | Internal prefixes whose crossings may be recorded (§Recording). |
| `terms` | array of terms | `[]` | Agreements the operator holds with named sources (§Terms). |

A scope:

| Field | Type | Default | What it does |
|---|---|---|---|
| `match` | string, required | | Matched as a substring of the session's working directory. |
| `principal` | string | none | The principal this scope belongs to. |
| `engagement` | string | none | The governing engagement: the name under whose clearance a crossing in this scope may leave the machine. |
| `allow_telemetry_egress` | boolean | `false` | Whether witnessed public crossings in this scope may enter a configured telemetry projection. |
| `policy_mode` | mode | inherited | Replaces the mode. |
| `constraints` | array of constraints | inherited | Replaces the constraint list; it is not merged with it. |
| `allow_private_hosts` | boolean | inherited | Replaces the setting. |
| `refuse_on_pii` | boolean | inherited | Replaces the setting. |
| `terms` | array of terms | inherited | Replaces the terms list. |

A principal binding:

| Field | Type | Default | What it does |
|---|---|---|---|
| `principal` | string, required | | The name the binding gives the identity. |
| `os_user` | integer, required | | The effective numeric operating-system user id the binding applies to. |
| `require_scope` | boolean | `false` | Whether this principal may work only in a scope bound to it. |
| `allowances` | array | `[]` | Periodic spending limits, at most one per period: `{"period": "day" or "month", "amount": {"currency", "micros"}, "timezone": "<IANA zone>"}`. |
| `policy_mode` | mode | inherited | Replaces the mode. |
| `constraints` | array of constraints | inherited | Replaces the constraint list. |
| `allow_private_hosts` | boolean | inherited | Replaces the setting. |

An optional field of a scope or a principal binding given as `null` is the
field left out. An unknown field at the top level, in a scope, in a
principal binding, in terms, in an allowance or in an amount refuses the
whole file. An unknown member inside a constraint is dropped, because a
constraint's kind decides its members and every member a kind needs is
required.

## Constraints

```json
{"kind": "denied_source_host", "host": "tracker.example"}
{"kind": "allowed_source_host", "host": "www.gov.uk"}
{"kind": "required_licence", "licence": "cc-by-4.0"}
{"kind": "maximum_acquisition_cost", "amount": {"currency": "GBP", "micros": 2500000}}
{"kind": "maximum_context_tokens", "tokens": 8000}
{"kind": "maximum_latency_ms", "milliseconds": 30000}
{"kind": "allowed_provider", "provider": "exa"}
{"kind": "denied_provider", "provider": "exa"}
{"kind": "access_rule", "host": "*.example.com", "action": "refuse"}
```

Every kind but one is a set, and the order of its entries means nothing.
`access_rule` is the exception. Access rules are read in the order written
and the first whose `host` matches the source's host decides; a later rule
for the same host is never reached.

```json
{
  "constraints": [
    {"kind": "access_rule", "host": "docs.example.com", "action": "allow"},
    {"kind": "access_rule", "host": "*.example.com", "action": "refuse"},
    {"kind": "access_rule", "host": "publisher.example", "action": "require_licence", "licence": "rsl:publisher/2026"},
    {"kind": "access_rule", "host": "*", "action": "refuse"}
  ]
}
```

A host pattern is one of three forms. An exact host matches that host
alone. `*.example.com` matches `example.com` and every host beneath it.
`*` matches every source that has a host. A source with no host, such as a
document from the operator's own corpus, matches no pattern. A pattern is
compared in lower case, IDNA-encoded and without a trailing dot, whatever
case it was written in. A wildcard anywhere but the front refuses the file.

The actions:

- `allow` passes the host, including past an `allowed_source_host` list that
  does not name it. A required licence still applies.
- `refuse` makes the source a breach.
- `require_licence` admits the source only when the supplier declared
  exactly the named licence; an undeclared licence is unknown, never
  permitted.
- `require_mediation` passes every crossing admission sees, because every
  such crossing is one Common Measure ruled on before it happened. An
  observed crossing is recorded without being checked against it. It states
  an intent in the file; a host that must not be read at all takes
  `refuse`.

A refusal names the rule that decided it as `access rule N (pattern)`, where
`N` is the rule's position in its constraint list, counted from one and
counting every constraint in that list, not only access rules.

## Scopes and principals

A session's effective policy is resolved once, from the policy, the
process's operating-system user and the working directory:

1. **Principal.** Where `principals` is not empty, the binding whose
   `os_user` is the process's effective user id overlays the top level: its
   `policy_mode`, `constraints` and `allow_private_hosts` replace the top
   level's where present. The user id is read from the process, not from
   `USER`, `LOGNAME`, `COMMONMEASURE_PRINCIPAL` or any other variable the
   process could set; `COMMONMEASURE_PRINCIPAL` is shown as an asserted label
   for diagnosis and selects nothing. An operating-system user with no
   binding holds no authority: every mediated crossing is refused and the
   refusal names the reason, whatever the mode. Declaring one principal
   therefore changes what happens to every other user of the machine, service
   and CI accounts included.
   Windows has no user id the runtime reads, so a policy declaring
   `principals` refuses every mediated crossing there.
2. **Scope.** The first scope whose `match` appears anywhere in the working
   directory is selected; no match leaves the top level. A match is a
   substring, not a path prefix: `/matters` also selects
   `/home/op/matters-archive`, so a narrower scope is listed before a
   broader one.
3. **Ownership.** A selected scope that names a principal other than the
   session's is not passed over for a later one: the session is refused
   there. A principal with `require_scope: true` whose session selects no
   scope bound to it is refused.
4. **Overlay.** The selected scope's `policy_mode`, `constraints`,
   `allow_private_hosts`, `refuse_on_pii` and `terms` replace the values
   resolved so far where present. Its `engagement` and
   `allow_telemetry_egress` apply to that scope alone and are never
   inherited.

## Admission

For each source a crossing would take in, admission applies, in this order:

1. a refusal from resolution (an unbound user, a scope owned by another
   principal), whatever the mode;
2. the `denied_source_host` set, which no rule allows past;
3. the access rules, first match deciding;
4. the `allowed_source_host` set, where one is declared and no access rule
   allowed the host;
5. the `required_licence` set, where one is declared.

Each step either passes the source to the next or produces a breach. The
mode decides what a breach does: `strict` refuses the crossing before any
bytes reach the model; `observe` and `prefer` admit it with the breach on
the record. Nothing is ever left unrecorded because of the mode.

## Recording

- `allow_private_hosts` lets the mediated tools reach loopback and private
  addresses. It does not affect observed capture, which never records
  localhost, private networks or `file://` whatever this says.
- `record_internal_prefixes` names internal prefixes, each an absolute URL
  ending in `/`, whose crossings are recorded, observed and mediated alike,
  and marked `internal` so no projection sends them. Observed capture records
  no internal or private address the list does not match; the mediated tools
  reach and record one only under `allow_private_hosts`.
- `refuse_on_pii`: the personal-data detector's finding is recorded on every
  mediated crossing. `strict` refuses one on an internal or private source;
  on a public source it admits the crossing with the finding recorded, because
  a public page's published contact details are not the personal data the
  detector exists to keep out of a model. With `refuse_on_pii` it refuses one
  on every source.
- A witnessed public crossing leaves the machine only where its scope names
  an `engagement` and sets `allow_telemetry_egress: true`; an absent policy,
  an unmatched directory or a scope without both keeps it local. What leaves
  is [`docs/contracts/session-evidence.md`](session-evidence.md). The scope's
  `engagement` is the governing engagement; the name the console reports
  work under is resolved separately, from the attribution rules.

## Terms

```json
{
  "terms": [
    {"host": "publisher.example", "reference": "agreement-42", "requires_reporting": true,
     "access_context": [{"scheme": "ror", "value": "https://ror.org/013meh722"}]}
  ]
}
```

A terms entry states that the operator holds an agreement with the source at
`host`, by a reference the operator chooses. The runtime never checks it.
Where one names the host of a mediated fetch it governs over any preference
the source publishes, and the crossing's licence is the reference.
`requires_reporting` says the agreement requires usage reporting;
`access_context` names the institution identifiers the agreement attributes
usage to, never a person. The host is compared as a host pattern's is, and
matched exactly: a subdomain needs its own entry.

The current entry has no fields for issuer authority, agreement version,
covered content or permitted uses, or an operator-assessed exception. It
therefore records the operator's host-level declaration without checking
those limits. The scoped basis-for-use extension is planned in `ROADMAP.md`
WP-30; its fields are not part of this contract or accepted by the loader.

## What the loader refuses

A policy is refused in two stages, and the whole file is refused at the
first fault.

**Structure.** The document must parse into the fields above: no unknown
field where §Fields says one refuses, every required member present, every
value of its type, a known mode, a known constraint kind, a known action, a
host pattern that names a host. The refusal is `<file> is not a valid
policy: ` followed by the parser's message, whose wording is not part of
this contract.

**Checks.** A document that parses is then checked. Each check refuses with
the sentence below, where `<file>` names the file as the loader was given
it and `<list>` is `the top-level policy`, `scope "<match>"` or
`principal "<name>"`:

| Check | Refused when | Refusal |
|---|---|---|
| `internal_prefix_not_absolute` | a recordable prefix is not an absolute URL | `<file>: record_internal_prefixes entry "<prefix>" is not an absolute URL prefix and would never match anything` |
| `internal_prefix_unterminated` | a recordable prefix does not end in `/` | `<file>: record_internal_prefixes entry "<prefix>" must end with "/" — without it the prefix also matches hosts and paths it merely starts, which would record more than the operator named` |
| `scope_match_empty` | a scope's `match` is empty | `<file> has a scope with an empty "match", which would govern everything` |
| `scope_match_duplicate` | two scopes have one `match` | `<file> declares two scopes matching "<match>"; only the first would govern or take an edit, so the second must be merged into it or renamed` |
| `principal_name_empty` | a principal's name is empty | `<file> has a principal with an empty name` |
| `principal_name_duplicate` | two bindings share a name | `<file> declares principal "<name>" twice, for OS users <first> and <second>` |
| `principal_os_user_duplicate` | two bindings share a user id | `<file> binds OS user <id> to both "<first>" and "<second>"; one identity cannot hold two principals` |
| `principal_scope_missing` | `require_scope` is set and no scope names the principal | `<file> requires principal "<name>" to work in a directory scope bound to it and declares none, so nothing could ever be admitted for it; bind a scope or drop require_scope` |
| `allowance_timezone_unknown` | an allowance names an unknown zone | `<file> principal "<name>" <period> allowance names timezone "<zone>", which is not an IANA zone this build recognises` |
| `allowance_period_duplicate` | one principal has two allowances for one period | `<file> principal "<name>" declares two <period> allowances of <first> and <second>; one period holds one amount, and the second would silently never bind` |
| `access_rule_licence_empty` | a `require_licence` rule names no licence | `<file> <list> constraint <N>: a require_licence access rule must name the licence it requires` |
| `scope_engagement_empty` | a scope's `engagement` is empty | `<file> has a scope with an empty "engagement"` |
| `scope_egress_without_engagement` | a scope clears egress and names no engagement | `<file> scope "<match>" allows telemetry egress without naming an engagement` |
| `scope_principal_undeclared` | a scope names a principal no binding declares | `<file> scope "<match>" names undeclared principal "<name>"` |
| `terms_host_empty` | a terms entry names no host | `<file> <list> terms entry <N> names no host` |
| `terms_reference_empty` | a terms entry names no reference | `<file> <list> terms entry <N> for host "<host>" names no reference; the reference is what the record and the wire carry` |
| `terms_host_duplicate` | one list has two entries for one host | `<file> <list> declares terms for host "<host>" twice; one host holds one agreement` |
| `terms_identifier_incomplete` | an institution identifier lacks a scheme or a value | `<file> <list> terms entry <N> for host "<host>" has an access_context identifier without a scheme or a value` |

Amounts in `allowance_period_duplicate` are written as the major unit with
six decimal places (`1.000000`). A value that is only spaces counts as empty
for the licence, the reference and an identifier's scheme and value.

With this saved as `refused.json`:

```json
{"policy_mode": "strict", "scopes": [{"match": "/matters", "allow_telemetry_egress": true}]}
```

```shell
$ commonmeasure policy check refused.json
commonmeasure: refused.json scope "/matters" allows telemetry egress without naming an engagement
```

The command exits non-zero on a refusal and prints nothing else.

## The loader's form

The loader's form of a policy is the policy as the loader writes it back
after parsing:

- `policy_mode`, `constraints`, `scopes`, `allow_private_hosts` and
  `record_internal_prefixes` are always present;
- `principals` and `terms` are present only when not empty, and
  `refuse_on_pii` only when true;
- a scope always carries `allow_telemetry_egress`, and carries its other
  optional fields only when set; a binding always carries `require_scope`,
  and `allowances` only when not empty; a terms entry always carries
  `requires_reporting`, and `access_context` only when not empty;
- a host pattern is written in its compared form and a currency in upper
  case;
- an unknown member inside a constraint is gone.

The digest of a policy is the SHA-256 of the canonical JSON of its loader's
form ([`docs/contracts/canonical-json.md`](canonical-json.md)), which is the
digest a fleet-status document reports for the file in force.
`commonmeasure policy check` prints it; here `policy.json` holds the `policy`
of the vectors' second accepted document:

```shell
$ commonmeasure policy check policy.json
accepted      policy.json
mode          strict
constraints   4
scopes        2
  /matters/confidential- — engagement client-confidential, telemetry egress not cleared
  /matters — engagement matters, telemetry egress cleared
principals    0
terms         0
digest        sha256:7e0249949df8ae01662b97eb21ef7e877945aa8bda144383dbf29367bf1a86ed
```

## The schema

`docs/contracts/source-policy.schema.json` is JSON Schema 2020-12, derived
from the types the loader parses with and printed by
`commonmeasure policy schema`. A test fails when the committed file is not
what the binary prints.

The schema decides structure and nothing after it. It accepts a host pattern
with a misplaced wildcard, which the parser refuses, and it accepts every
document refused by a check in §What the loader refuses. A document the
schema accepts is therefore not a document the loader accepts; the vectors
record the schema's verdict and the loader's separately, and the difference
between them is this section.

## The vectors

`docs/contracts/source-policy-vectors.json` holds three lists.

`validation`: each entry has a `name`, a `policy`, `schema_valid` (the
schema's verdict) and `accepted` (the loader's). An accepted entry carries
`loader_form`. A refused entry carries `check`, one of `structure` or the
check names in §What the loader refuses, and, for every check but
`structure`, the `refusal` the loader produces for a file named by the
top-level `source_name`. Every check has at least one refused entry.

`ruling_policies` names the policies the rulings use, and `rulings` holds
one entry per ruling:

| Member | Meaning |
|---|---|
| `policy` | a key of `ruling_policies` |
| `cwd` | the session's working directory |
| `url` | the source |
| `licence` | the licence the supplier declared, or `null` for none |
| `scope` | the `match` of the scope that governs, or `null` |
| `mode` | the mode that governs |
| `ruling` | `allowed`, `refused` or `allowed_with_breach` |
| `reason` | the runtime's sentence for a breach, or `null` |

No ruling policy declares `principals`, because a principal is resolved from
the user running the check and a vector must give one answer everywhere.

An implementation that writes, checks or explains policies outside this
repository runs the vectors and reproduces `schema_valid`, `accepted`,
`loader_form`, `check`, `refusal`, `scope`, `mode` and `ruling`. `reason` is
the sentence an explanation can quote. The vectors are checked against the
binary and the runtime in `crates/commonmeasure-cli/tests/source_policy_contract.rs`.

## Policy written for many machines

A policy is written for one machine or distributed to many from a hub, and
two of its fields mean something different on each machine they reach:

- `os_user` is a number each machine assigns. The same id names a different
  person on each machine, and on a personal workstation it is usually that
  machine's only account. A binding distributed to many machines applies
  its constraints and allowances to whoever holds that id on each of them,
  and every crossing their sessions record carries the binding's name.
- `match` is compared with each machine's own directory layout. A
  distributed scope governs the same work on every machine only where the
  organisation lays its directories out alike.

A policy meant for many machines is therefore written without `principals`
until an identity that names the same person on every machine exists, and
its scopes are written against a directory layout the organisation keeps.
[`docs/contracts/policy-envelope.md`](policy-envelope.md) §What a distributed
policy carries states the same for the hub.


## Directory selection and managed reporting

Directory enrolment does not edit this schema or a managed policy file.
A separate local root selection and signed edge-bound reporting grant narrow
egress. Every winning false scope vetoes reporting, including an omitted bool;
grants never replace admission rules. Absolute existing directory matchers
also recognise canonical targets, and conflicting symlink/worktree scopes fail
closed. Resolver version 2 records these semantics. See
[directory enrolment](directory-enrolment.md).
