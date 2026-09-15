# Directory enrolment implementation handoff

Snapshot: focused implementation evidence from 14 September 2026. Integrated
on main during the 15 September checkpoint; [integration status](directory-enrolment.md)
and the product roadmap own current acceptance.
The [directory enrolment contract](../docs/contracts/directory-enrolment.md)
owns the behaviour. Nothing was pushed or deployed, no agents were spawned,
and no shared operator home or actual host registration was changed.

## Implementation

- CLI: `commonmeasure enrol`, explicit directory/name/local-or-hub choice,
  historical-evidence acknowledgement, status, sync and local opt-out.
- Claude plugin: `/commonmeasure:enrol`, included in standalone packaging.
- Codex: `$commonmeasure-enrol`, installed/upgraded/removed by the existing
  host-registration commands under the user's `.agents/skills` directory.
  Foreign skills and custom metadata are preserved; installation refuses a
  conflicting file before changing the host configuration.
- Local MCP: `context_enrol`, explicit host/project-directory agreement, and
  the `commonmeasure_enrol` prompt. Status includes process cwd. A process
  started at `/` cannot claim to govern a selected project.
- Local project records bind canonical roots, OS user, filesystem identity and
  Git common directory to UUIDs and nonce-protected hashes. Root/descendant
  matching excludes siblings and separately identifies related worktrees.
- Managed path: private edge requests, owner approval/revocation page, signed
  edge/organisation-bound grant snapshots with independent revisions/digests
  and a maximum 24-hour validity window. Signed empty snapshots revoke;
  failed fetches never clear or extend snapshots.
- Admission remains the organisation's source policy. False winning scopes,
  including omitted flags, veto reporting. Local consent, grant validity and
  source/privacy rules are rechecked for queued deliveries too. Persistent local
  selection mode and spool consent provenance prevent registry loss from
  restoring legacy queued reporting. Valid local-only ancestors veto nested
  repositories and submodules across their distinct Git identities.
- Existing keys, signed source-policy bytes, provider credentials and evidence
  are preserved. Effective-policy resolver identity advances to version `2`;
  source-policy schema, document digests and envelope payloads do not change.

## Checks and results

Commands ran in the named repository. Cargo used offline dependencies. The
local Postgres URL below is a local test service; SQLx created isolated test
databases, applied migrations and removed them. No application database was
used for fixture writes.

### Edge

```sh
cargo fmt --all --check
cargo clippy -p commonmeasure-cli -p commonmeasure-harness -p commonmeasure-relay --all-targets --offline -- -D warnings
cargo test -p commonmeasure-harness -p commonmeasure-relay --offline --quiet
cargo test -p commonmeasure-cli --test directory_enrolment --test install_e2e --test managed_policy --test mediated_e2e --test relay_e2e --test source_policy_contract --test fleet_status --test docs_paths --offline --quiet
```

Formatting and Clippy passed. Harness/relay suites: **254 passed**. Focused
CLI/contract suites: **120 passed, one existing live oa-server test ignored**.
The final directory suite has **9 tests**, including missing registry with and
without legacy scopes, loss of both registry and mode marker across repeated
retries, nested Git/submodule ancestor vetoes and old spool compatibility.
Harness/relay tests and Clippy passed again after these corrections.
One managed-policy hanging-Hub test failed once at MCP child exit during a
combined run; its isolated rerun and the next combined run passed. The broader
relay suite caught lock-file creation during dry-run; the delivery lock now
applies only to actual sends. The affected directory, relay, source-policy and
documentation suites were rerun after that correction. `git diff --check` passed.

The skill-creator validator passed using an isolated temporary Python
virtualenv with PyYAML. A temporary copy of the real plugin was packaged with
the newly built binary: the archive included the command, Codex skill and
invocation metadata, and its launcher enrolled an isolated local-only project.
The installed public 0.3.2 binary also accepted the unchanged source-policy
shape through `policy check` in an isolated home.

### Hub

```sh
cargo fmt --all --check
cargo clippy -p commonmeasure-hub-server --all-targets --offline -- -D warnings
DATABASE_URL="$TEST_DATABASE_URL" cargo test -p commonmeasure-hub-server --test directories --test policy --test enrolment --offline --quiet
DIRECTORY_TEST_EDGE_BINARY="$TEST_EDGE_BINARY" DATABASE_URL="$TEST_DATABASE_URL" cargo test -p commonmeasure-hub-server --test directories cli_enrol --offline -- --ignored --nocapture
```

The commands above use placeholders for the local validation environment. Set
`TEST_DATABASE_URL` to an isolated local test Postgres database and
`TEST_EDGE_BINARY` to the newly built edge executable before rerunning them.
Formatting and Clippy passed. Cargo still reports existing manifest warnings
for unused workspace dependencies `clap` and `csv`; there were no Rust Clippy
warnings. Handler suites: **50 passed**, with the cross-repository test excluded
by default and **passed explicitly** with the second command.

A separate `initdb` attempt under a temporary directory was refused by the
sandbox's shared-memory permission (`shmget`). No permission controls were
bypassed. Validation used the already-running loopback Postgres service instead.

### Frontend

```sh
npm run check
npx prettier --check 'src/routes/(app)/directories' src/lib/components/AppShell.svelte
npx eslint 'src/routes/(app)/directories' src/lib/components/AppShell.svelte
npm test -- --project server 'src/routes/(app)/directories/page.test.ts'
```

All passed: no type errors/warnings and **2 page tests passed**. The tests check
owner approval/revocation form transitions, reviewed binding, and absence of
controls for members/withdrawn requests. Real handler/database tests establish
server authorisation; these page tests do not claim a live browser walkthrough.

## End-to-end evidence

The explicit Hub CLI test starts the real router on loopback and drives the
new CLI/MCP as child processes with isolated homes. It verifies:

1. Managed connection once; repeated directory requests preserve keys and
   source-policy bytes, and status remains pending until owner approval.
2. Owner approval, signed grant acceptance, and reporting permitted under the
   actual applied strict source policy.
3. A successful permitted local MCP fetch and a denied-source refusal.
   The private retrieval produces **zero eligible wire events**.
4. Synthetic public WebFetch input through the production hook produces
   **two events accepted by the real Hub**. A repeated relay creates none.
5. A queued batch remains withheld after owner revocation.
6. An expired signed snapshot plus a failed refresh authorises neither queued
   nor new events and its saved expiry is unchanged.
7. Local opt-out leaves keys, signed policy and evidence intact.

The public hook input is synthetic evidence. The test establishes real
recording/relay/Hub integration, not a live external provider acquisition or
interactive agent-host invocation.

## Independent review corrections

Lead review reproduced two blocking faults: missing `directories.json` skipped
queued consent revalidation, and Git identity filtering discarded valid
local-only ancestors before applying their veto. Both are corrected:

- Enrolment writes `directory-selection.json` before a project can authorise
  reporting. Missing registry plus retained mode means empty selection, so
  queued and new reporting stays local without relaxing admission.
- Local spool entries carry `directory_selection` provenance outside the wire
  document. Even if both consent files disappear, a tagged pending batch
  returns an explicit error and remains pending on repeated retries. This
  prevents acknowledgement followed by historical reprojection under legacy
  scopes. Old entries without the field retain legacy behaviour in homes that
  have never adopted directory selection.
- Ancestor vetoes validate the ancestor's own canonical root, UID, filesystem
  identity, binding and Git identity. Positive reporting inheritance still
  requires a matching current Git identity.

The lead independently confirmed both original reproductions now withhold
reporting, and reported passing the rebuilt edge's complete cross-repository
CLI/Hub test, 50 Hub handler tests, 2 approval page tests, frontend checks and
2 documentation-path checks. These reports supplement the implementation's
local validation above. No hosted acceptance or live host invocation is claimed.

## Remaining limits and lead integration work

- Interactive Claude/Codex invocation is **unverified**. Plugin/Codex discovery
  is specification-verified; packaging and CLI/MCP boundaries are tested.
  MCP prompt exposure is host-specific. No hosted policy/session was changed.
- Windows directory enrolment returns unavailable because the runtime has no
  authenticated OS-user basis there. Non-Git folders are supported on Unix.
- Older binaries ignore the new local consent registry. Upgrade every binary
  sharing an operator home before using directory selection; mixed old/new
  enforcement is unsupported. Other old edges retain unchanged source policy.
- An offline grant lasts only to its original expiry, at most 24 hours. Local
  opt-out completes after an in-progress delivery and blocks later ones.
- Canonical paths must remain resolvable at projection. Removed or replaced
  directories cannot clear historical evidence. A related worktree resolving
  a different main-worktree policy scope fails closed until policy is aligned.
- Source-policy and provider configuration remain session-scoped outside an
  explicit MCP enrol/sync operation. This implementation adds no live-refresh
  service, entitlement distribution or hosted acceptance.
