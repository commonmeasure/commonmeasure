---
title: The hosted edge
draft: true
---

# The hosted edge

The design of an edge that runs as a service and serves the three mediated
tools (`context_fetch`, `context_search`, `context_status`) over HTTPS to
hosts that reach MCP only remotely. The hosts, and why the local binary
cannot serve them, are
[`docs/knowledge-base/host-surfaces.md`](host-surfaces.md) §Order. The
roadmap package is `ROADMAP.md` §The hosted edge.

Nothing on this page is built. Every element of the design is `planned`.
External facts carry one of the grades the host-surfaces page uses and are
never collapsed upwards:

- `spec-verified`: read in the vendor's or the protocol's published
  documentation, cited by URL. Every page cited here was read on 14
  September 2026; where the page carries its own date, that date is given
  too.
- `planned`: nothing is proven.

Terms such as crossing, principal, scope, engagement and key id are defined
in [`docs/GLOSSARY.md`](../GLOSSARY.md).

## What it is

- The same `commonmeasure` binary, started in a service mode, listening on
  HTTPS, serving the mediated tools to many people in one organisation.
- Enrolled with the hub as one edge (`commonmeasure connect`), so it holds
  one key id, signs its fetches as `CommonMeasureBot` under that key, and
  delivers its cleared projection under one ingest key.
- One hosted edge per organisation. It serves no other organisation.

The hosted edge MUST keep these invariants, which are the ones every edge
keeps (`PRODUCT.md` §Edge and hub, `DECISIONS.md` §Product):

- The hub is never in the decision path of a crossing. No hub call sits
  between a tool call and its ruling.
- The hub's absence never relaxes policy. A hub that cannot be reached
  leaves the policy in force governing and can only make the tools less
  available, never more permissive.
- The policy engine, the admission rules, the refusal semantics, the record
  format and the relay are the existing ones, unchanged. There is no second
  policy engine, no second record format and no second egress path.

What is new is confined to four things: transport (HTTP in place of stdio),
identity (a signed-in person in place of an operating-system user),
tenancy (many people and many concurrent sessions in one process), and
where the record lives (a disk on a server in place of the operator's own
machine). Each section below changes one of those and says what it leaves
alone.

## Transport

**The protocol revisions in play.** The Model Context Protocol has
published revisions 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25 and
2026-07-28 (`spec-verified`: <https://modelcontextprotocol.io/specification/2026-07-28>).
The two latest differ on exactly what a hosted server needs:

- 2025-11-25 has protocol sessions. The server "MAY assign a session ID at
  initialization time, by including it in an `MCP-Session-Id` header on the
  HTTP response containing the `InitializeResult`"; the client "MUST include
  it" on every later request; a server that ends a session "MUST respond to
  requests containing that session ID with HTTP 404 Not Found", after which
  the client starts a new session; a client "SHOULD send an HTTP DELETE" to
  end one (`spec-verified`:
  <https://modelcontextprotocol.io/specification/2025-11-25/basic/transports>).
- 2026-07-28 removes protocol sessions, the `Mcp-Session-Id` header, the
  `initialize` handshake and resumable streams. "MCP is a stateless
  protocol"; `clientInfo` moves into each request's `_meta` and is marked
  optional (`spec-verified`:
  <https://modelcontextprotocol.io/specification/2026-07-28/basic/index> and
  <https://modelcontextprotocol.io/specification/2026-07-28/changelog>).
- A server may serve both on one endpoint: an `initialize` request "selects
  legacy semantics, scoped to … the session (HTTP)" (`spec-verified`:
  <https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning>).

No target host documents the 2026-07-28 revision. Claude "Supports the
2025-03-26, 2025-06-18, and 2025-11-25 auth specifications" (`spec-verified`:
<https://claude.com/docs/connectors/building>, no date shown). The local
server serves 2025-03-26, 2025-06-18 and 2025-11-25 and answers a client
with the revision it asks for where it serves it (`PROTOCOL_VERSIONS` in
`crates/commonmeasure-harness/src/mcp.rs`), and the clients probed locally
asked for 2025-03-26 (host-surfaces-2 §Junie), 2025-06-18 and 2025-11-25
(host-surfaces §Claude Desktop, §Codex beyond the CLI).

**The design.**

- The hosted edge serves Streamable HTTP at 2025-06-18 and 2025-11-25, with
  protocol sessions: one POST endpoint per host registration, a session id
  minted at `initialize`, `404` for an unknown or expired session, `DELETE`
  honoured, the `MCP-Protocol-Version` header checked, and the `Origin`
  header validated as the protocol requires ("If the `Origin` header is
  present and invalid, servers MUST respond with HTTP 403 Forbidden", same
  page).
- Responses are single JSON objects. The server sends no server-initiated
  requests and no progress notifications today, so it never needs to open an
  event stream; the protocol lets a server answer a POST with either form.
  `GET` on the endpoint answers `405`, which the transport allows for a
  server that offers no standing stream.
- The 2026-07-28 revision is added when a target host sends it, not before.
  What a session is under that revision is §Risks and open questions, item
  2.
- The legacy HTTP+SSE transport of 2024-11-05 is not built. It is
  "Deprecated … New implementations SHOULD NOT adopt it" (`spec-verified`:
  <https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http>),
  and no target host requires it: Claude "supports both Streamable HTTP and
  the legacy HTTP+SSE transport" (<https://claude.com/docs/connectors/building>);
  ChatGPT's developer mode supports "SSE and streaming HTTP"
  (<https://developers.openai.com/api/docs/guides/developer-mode>, no date
  shown); Copilot Studio "no longer supports SSE for MCP after August 2025"
  (<https://learn.microsoft.com/en-us/microsoft-copilot-studio/mcp-add-existing-server-to-agent>,
  last updated 28 May 2026); the Copilot cloud agent accepts `"http"` or
  `"sse"`
  (<https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/configure-mcp-servers>,
  no date shown). All `spec-verified`.

**What changes in `mcp.rs`.** A second transport behind the same tool
definitions and the same `McpServer`:

- `McpServer` already handles one JSON-RPC message at a time
  (`handle_line`, `handle_request`) and holds one session: its log, its
  resolved policy, its host word, its client identity and its fetcher
  identity. The stdio loop (`serve`) is one way of feeding it messages; an
  HTTP handler that routes each POST to the `McpServer` of its session is
  the other. Tool definitions, the fetch, the search, the status, the
  refusal messages and the record writes are shared, not copied.
- What `serve_mcp` in `crates/commonmeasure-cli/src/main.rs` does once per
  process today (refresh managed policy, resolve the policy, open the log,
  record `edge_identity` and `credentials_loaded`) becomes the start of a
  session, run once per protocol session by either transport. Credentials
  are still applied once, at process start, before any thread exists, and
  each session records what was loaded. The stdio path keeps its behaviour
  exactly.
- Two additions reach both transports: the three tools declare
  `readOnlyHint: true`, because ChatGPT treats a tool without it as a write
  and asks for confirmation before each call (host-surfaces §ChatGPT); and a
  session's host word comes from the endpoint it was opened on, where stdio
  takes it from `--host`.

## Authentication and the principal

**What the protocol requires.** A protected MCP server "acts as an OAuth 2.1
resource server"; it "MUST implement OAuth 2.0 Protected Resource Metadata
(RFC9728)" naming at least one authorisation server; it "MUST validate that
access tokens were issued specifically for them as the intended audience,
according to RFC 8707 Section 2"; it "MUST NOT accept or transit any other
tokens" and "MUST NOT pass through the token it received"; clients "MUST
implement PKCE" with `S256`; "All redirect URIs MUST be either `localhost`
or use HTTPS". Dynamic Client Registration (RFC 7591) is "MAY" in
2025-11-25 and deprecated in 2026-07-28 in favour of Client ID Metadata
Documents, which authorisation servers and clients "SHOULD support"
(`spec-verified`:
<https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/index.md>
and its client-registration and security-considerations pages).

**What each target host requires.** All `spec-verified`.

| Host | Client registration | Redirect URI | Audience | Other requirements |
|---|---|---|---|---|
| Claude custom connector (claude.ai, Claude Desktop, Cowork, mobile) | Client ID Metadata Document and DCR "Supported out of the box"; a static client id and optional secret under "Advanced settings"; Claude picks CIMD only when the metadata advertises `client_id_metadata_document_supported: true` and `none` among token endpoint auth methods; DCR registers "a new client on every fresh connection" | `https://claude.ai/api/mcp/auth_callback` | Claude sends the RFC 8707 `resource` parameter set to the canonical server URL "including any path component"; the resource metadata's `resource` "must match your MCP server URL exactly" | PKCE `S256` on every request; `offline_access` appended when listed; a `401` with `WWW-Authenticate`, never a `200`; discovery, registration and token endpoints answer within 10 seconds; only the first listed authorisation server is used; IPv4 only; traffic from `160.79.104.0/21`; "Every connection requires user consent" |
| ChatGPT app (developer mode, workspace apps) | static credentials if given, else CIMD (`client_id` `https://chatgpt.com/oauth/client.json`), else DCR "once per MCP server connection" | `https://chatgpt.com/connector_platform_oauth_redirect` when the authorisation server returns `iss` (RFC 9207), otherwise a per-callback URI | ChatGPT appends `resource=` to authorisation and token requests; "copy that value into the access token (commonly the aud claim)" | `code_challenge_methods_supported` must include `S256`; without `offline_access` access may lapse; no machine-to-machine grants; egress ranges published in `chatgpt-connectors.json` and they change |
| Microsoft 365 Copilot, Copilot Studio | OAuth 2.0 "Dynamic discovery" (DCR with discovery), "Dynamic" or "Manual"; also "None" and "API key" | "A callback URL appears" after creation; its value is not documented | not documented | Streamable HTTP only; generative orchestration on; access "relies on Power Platform connectors", so data policies apply |
| Microsoft 365 Copilot, declarative agent | DCR "Supported", but "DCR without a client secret isn't supported yet"; the Teams Developer Portal "doesn't support DCR yet"; manual OAuth registration in the Developer Portal otherwise | `https://teams.microsoft.com/api/platform/v1.0/oAuthRedirect`, "the same for every plugin and provider" | not documented | "DCR isn't available for an MCP server that's protected by Microsoft Entra ID" |
| Copilot cloud agent on GitHub | none: it does "not currently support remote MCP servers that leverage OAuth" | none | none | headers carrying repository secrets named `COPILOT_MCP_*`; "The firewall … does not apply to Model Context Protocol (MCP) servers"; tools are used "autonomously" without approval |

Sources: Claude
<https://claude.com/docs/connectors/building/authentication>,
<https://claude.com/docs/connectors/building/troubleshooting.md> and
<https://platform.claude.com/docs/en/api/ip-addresses> (no dates shown),
<https://support.claude.com/en/articles/11175166-getting-started-with-custom-connectors-using-remote-mcp>
(dated 11 August 2026); ChatGPT
<https://developers.openai.com/plugins/build/auth> and
<https://developers.openai.com/api/docs/guides/ip-addresses> (no dates
shown); Microsoft
<https://learn.microsoft.com/en-us/microsoft-copilot-studio/mcp-add-existing-server-to-agent>
(28 May 2026),
<https://learn.microsoft.com/en-us/microsoft-copilot-studio/agent-extend-action-mcp>
(26 August 2026),
<https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/plugin-authentication>
(21 August 2026),
<https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/plugin-authentication-oauth>
and
<https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/plugin-authentication-dynamic-client-registration>
(both 31 August 2026); GitHub
<https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/configure-mcp-servers>,
<https://docs.github.com/en/copilot/concepts/agents/cloud-agent/mcp-and-cloud-agent>
and
<https://docs.github.com/en/copilot/how-tos/copilot-on-github/customize-copilot/customize-the-firewall>
(no dates shown).

**Which identity provider the first organisation uses: the hub's own
sign-in.** The hub is the authorisation server, it signs a person in with
the sign-in it already has (email link, and Google where configured), and it
issues access tokens only to members of the organisation that owns the
hosted edge. A firm's own identity provider is not placed directly in front
of the hosted edge, and federation from the hub's sign-in to a firm's
provider is a later hub change that the hosted edge does not see. The
argument:

- **One authorisation server can meet all four hosts; a firm's provider
  cannot be relied on to.** The hosts disagree on registration: Claude and
  ChatGPT accept CIMD with a public client, Microsoft's declarative agents
  need DCR that issues a secret, Copilot Studio's callback URL is known only
  after creation. Microsoft's own documents say DCR is unavailable for a
  server protected by Microsoft Entra ID, and Claude's troubleshooting page
  says Entra needs the full MCP URL added as an Application ID URI. A
  hosted edge that trusted each firm's provider directly would inherit a
  per-firm, per-host matrix of what works; the hub meets the matrix once, in
  code Common Measure Ltd tests.
- **Membership already lives at the hub.** Who belongs to the organisation,
  and who may enrol its edges, is hub state. An authorisation server that is
  not the hub would need that list copied to it or would authorise people
  the organisation has not admitted.
- **The edge stays a pure resource server.** It verifies a signature,
  `iss`, `aud`, `exp` and the organisation claim, and holds no client
  registrations, refresh tokens or sign-in state. Everything stateful about
  OAuth lives in the hub, which already has a database and a public origin.
- **Federation later changes nothing at the edge.** When a firm requires
  its own provider, the hub's sign-in delegates to it; the issuer, the
  audience and the subject the edge sees stay the hub's.

The strongest case against: a regulated firm's access control, multi-factor
policy and leaver process live in its own provider, and a hub email link is
weaker than that. The answer is to federate at the hub before that firm is
onboarded, not to point the edge at the firm's provider; until then a
leaver's access ends when their hub membership is removed and their
current access token expires.

The cost: the hub becomes a dependency of the tools' availability. A
person whose access token has expired cannot obtain a new one while the hub
is down, and their tool calls then answer `401`. That is the tools
becoming unavailable, never policy becoming weaker, and no crossing's
ruling consults the hub; both invariants hold. The edge fetches the hub's
signing keys (JWKS) from the issuer pinned at enrolment, caches them on its
disk, and refetches only for a key id it does not hold, so a hub outage
does not stop tokens already issued from verifying.

**The token.** The hub issues a signed JWT access token carrying `iss` (the
hub's origin), `aud` (the canonical URL of the endpoint the host registered,
path included, exactly as the resource metadata names it), `sub` (the hub's
stable, opaque user id), the organisation id, `exp` (an hour), `client_id`
and `scope`, with a refresh token when `offline_access` is asked for. The
edge refuses, with `401` and a `WWW-Authenticate` header naming the resource
metadata, a token whose signature, issuer, audience, expiry or organisation
does not match. The organisation MUST equal the one in the edge's
`enrolment.json`, so a member of another organisation cannot use this edge
even with a valid hub token.

**How the subject becomes the principal.** Today the mediated path reads
the principal from the process (`Principal::current` in
`crates/commonmeasure-harness/src/policy.rs`):

- the effective user id from the kernel becomes the name `os-user:<uid>`
  with `authentication_basis: "os_user"`;
- a policy binding `{"principal": "<label>", "os_user": <uid>}` renames it to
  the label and applies its overlay and allowances;
- a platform with no readable user records `unauthenticated` with basis
  `unavailable`, and a policy that declares principals refuses every
  crossing there;
- `COMMONMEASURE_PRINCIPAL` is kept as an asserted label and never selects
  a binding.

On the hosted edge:

- The principal comes from the verified token and from nothing else. The
  service process's own user id is never read as a principal, and the
  asserted-label variable is not read.
- `sub` becomes the name `subject:<sub>` with a new basis,
  `authentication_basis: "oauth_subject"`. The basis travels with the name,
  as it does today, so a reader can tell a kernel-authenticated principal
  from a token-authenticated one.
- A binding may name `subject` in place of `os_user`:
  `{"principal": "research-lead", "subject": "<hub user id>"}`. A binding
  names exactly one of `os_user`, `subject` and `edge_token` (below), and
  the loader refuses one that names more or none. Selection, overlay,
  `require_scope`, allowances and the fail-closed rule are unchanged.
- The policy identity's pre-image already carries `principal` as
  `{"name", "basis"}` (`docs/contracts/fleet-status.md` §Policy identity),
  and still leaves out the authenticated subject id. The schema and the
  resolver version do not move: no existing policy document resolves
  differently, and a test holds the existing identity vectors unchanged.
- The host with no OAuth (the Copilot cloud agent) authenticates with a
  bearer token the edge itself issues for one named principal and stores
  only as a hash (§Per host, the registration step). Its basis is
  `edge_token` and its name `token:<label>`; a binding names it with
  `edge_token`. It is recorded as a weaker basis because the secret sits in
  a repository's settings, readable by that repository's administrators.

**What a firm sees as a result.**

- In its private operator record on the hosted edge: every mediated
  crossing and refusal carries `principal` and `authentication_basis`, so
  each is attributable to one hub user, or to the label its binding gives.
  The record carries the opaque hub user id, never an email address; the
  mapping to a person is the organisation's member list at the hub.
- In the hub's Fleet evidence: nothing about the person. The projection
  carries the agent identifier, which is the edge's key id read from the
  session's `edge_identity` record, the host word as `contextops-host-tool`,
  a session identifier and the refused count
  (`crates/commonmeasure-relay/src/project.rs`), and no principal. All the
  organisation's people therefore appear as one agent, the hosted edge,
  with their sessions apart. This follows the existing decision that no
  engagement name and no per-crossing authority detail crosses the wire
  (`DECISIONS.md` §Session policy and egress), and it is kept: putting a
  principal on the Content Telemetry wire would need a contract
  conversation with the standard's home and a privacy decision of its own.

## Tenancy

One hosted edge per organisation: one enrolment, one key id, one policy,
many principals, many concurrent sessions.

- **The session.** One protocol session is one session in the record and
  one file, `sessions/<session-id>.ndjson`, as today
  ([`docs/contracts/session-evidence.md`](../contracts/session-evidence.md)
  §Where). Under the protocol revisions the hosts speak, the server assigns
  the session id and the client supplies none, so the edge mints one at
  `initialize`: `hosted-<milliseconds>-<128 random bits in hex>`, the local
  form `local-<milliseconds>-<pid>` with the process id replaced by a value
  the protocol asks to be "globally unique and cryptographically
  secure". The same value is the `Mcp-Session-Id` header and the record's
  `session_id`, so a request in a web server's log and a record in the
  session file are joined without a lookup table.
- **A session belongs to one principal.** The edge binds the session to the
  token's subject at `initialize` and answers `404` to a request that
  carries the session id with another subject's token. The protocol says a
  session id is not authentication; this is the check that makes it so.
- **Policy per session.** Each session refreshes managed policy and
  resolves the policy once, at `initialize`, for its own principal, exactly
  as a local server does at process start
  ([`docs/contracts/session-evidence.md`](../contracts/session-evidence.md)
  §Policy identity); every crossing in the session names that identity.
  Many sessions of many principals run under one policy document, each with
  its own resolved overlay.
- **The working directory.** The recorded `cwd` stays the fact it is
  today: the directory the serving process runs in, one value for every
  session. Scopes match on it as they do on any edge, so a hosted edge has
  one governing scope and one engagement, and principals differ by their
  binding overlays. Per-host or per-engagement scopes on one hosted edge are
  §Risks and open questions, item 3.
- **Concurrency.** Sessions run concurrently; the messages of one session
  are handled one at a time, which is what `McpServer` assumes today. Each
  session file has one writer. The shared files (the allowance ledger, the
  declaration and manifest caches, the managed state) are already written
  under the exclusive file lock in
  `crates/commonmeasure-runtime/src/declaration.rs`, because Claude Desktop
  already runs two servers against one home.
- **Session end.** A session ends at `DELETE`, after an idle interval with
  no request (thirty minutes, one constant), or when the process stops.
  After that the id answers `404` and the host starts a new session, as the
  protocol directs. A session that served no tool call leaves no file, as
  today.
- **The `host` word and the `client`.** `host` is the registration's word,
  taken from the endpoint path the host was registered with:
  `claude-connector`, `chatgpt`, `m365-copilot`, `copilot-cloud-agent`. An
  unknown path answers `404`, which is the HTTP form of the stdio server
  refusing an unknown `--host`. `client` is recorded from `clientInfo` at
  `initialize` as today. What each host sends is not documented by any of
  them: Claude's pages name no client, OpenAI's name none (a community
  forum mentions a user agent, which is not `clientInfo`), and Microsoft's
  and GitHub's name none. Each is `planned` until the first `initialize`
  from that host is recorded, and the record then says it exactly. Claude
  Desktop's local chat client sends `claude-ai` (host-surfaces §Claude
  Desktop), so a connector call and a local call from the same person are
  told apart by `host`, never by `client` alone.

## Where the record lives

The session logs, the allowance ledger, the managed-policy state, the
spool, the enrolment record, the edge's private key and the caches are
files under the operator home. Their guarantees rest on a POSIX file
system: every append fsynced before it returns
([`docs/contracts/session-evidence.md`](../contracts/session-evidence.md)
§Where), atomic rename for policy saves, and an exclusive advisory lock for
the ledger and the policy file.

**The smallest change that keeps `session-evidence.md` true is no change to
the files: a durable POSIX disk, one machine, one serving process.**
`$COMMONMEASURE_HOME` points at a directory on that disk and every contract
reads as written. A store behind the record (a database or object storage)
would rewrite the session log, the ledger, the spool and the managed state,
and every guarantee above would have to be re-proven; that is a later
package if hosted edges become numerous, not the first one.

That rules out Cloud Run's volume options, which is why §Deployment departs
from Cloud Run. `spec-verified`, pages dated 10 September 2026 unless noted:

- Cloud Storage FUSE: "does not provide concurrency control for multiple
  writes (file locking)", "is not a fully POSIX-compliant file system"
  (<https://docs.cloud.google.com/run/docs/configuring/services/cloud-storage-volume-mounts>);
  with streaming writes, its default, "the fsync operation will not finalize
  the object" and "Relying on fsync for data durability with streaming
  writes enabled is not recommended"
  (<https://github.com/GoogleCloudPlatform/gcsfuse/blob/master/docs/semantics.md>,
  no date shown).
- NFS on Cloud Run: "Cloud Run does not support NFS locking. NFS volumes are
  automatically mounted in no-lock mode"
  (<https://docs.cloud.google.com/run/docs/configuring/services/nfs-volume-mounts>).
- One instance is not guaranteed: "Cloud Run might, for a short period of
  time, create more instances than are specified in the maximum instances
  setting", "a few times per week", and a deploy starts the new revision
  before directing traffic to it
  (<https://docs.cloud.google.com/run/docs/about-instance-autoscaling>, 9
  September 2026).

Two processes in two instances, each holding a lock that locks nothing, can
both write the allowance ledger. That is a double spend, and it is the
failure the ledger's lock exists to prevent.

**The index.** The console's SQLite index (`telemetry.db`) is derived and
can be rebuilt; on the hosted edge it sits on the same disk, read by one
console process on the same machine, so SQLite's locking is the local
file system's and holds.

**Backups.** The disk is snapshotted on a schedule (§Deployment). A
restored snapshot is the record as of that time, and the sessions written
after it are lost. The hub may then hold delivered events for sessions the
restored record no longer has; the operator record stays the authoritative
one, and the restore is written down with the window it lost.

**The operator console.** `commonmeasure serve` has no credential and
refuses a non-loopback address without `--allow-remote`
(`crates/commonmeasure-cli/src/main.rs`). On the hosted edge:

- It is not replaced by Fleet evidence. Fleet evidence is the cleared
  projection: no refused URL, no reason, no principal, no scope. A
  compliance owner answering "which person's agent was refused which source
  under which rule" needs the operator record, and only the console reads
  it.
- It runs as a second process on the same machine, reading the same home,
  listening with `--allow-remote` on a port the machine's firewall admits
  only from the load balancer, on its own hostname.
- It is protected by Identity-Aware Proxy on that backend, granted to named
  accounts: the organisation's reviewers, and Common Measure Ltd's
  operators only with the organisation's agreement. "You can enable IAP on
  a Compute Engine backend service"; with the Google-managed OAuth client
  "Only users within the organization can access the IAP-enabled
  application", so reviewers outside Common Measure Ltd's Google
  organisation need IAP's external-identity setup (`spec-verified`:
  <https://cloud.google.com/iap/docs/enabling-compute-howto>, 10 September
  2026). The console does not itself check the proxy's identity header, so
  the machine's firewall admits the console's port from the load balancer
  alone. A reviewer without a Google account is §Risks and open questions,
  item 9.
- Its policy writes behave as on any managed edge: a local edit shows as
  divergent and the next synchronisation writes the desired policy back
  ([`docs/contracts/policy-envelope.md`](../contracts/policy-envelope.md)
  §Validation). Policy for a hosted edge is authored at the hub.

## Policy

- **Managed.** The hosted edge is a managed edge
  ([`docs/contracts/policy-envelope.md`](../contracts/policy-envelope.md)
  §Deployment mode): `deployment.json` pins the hub's policy signer, every
  envelope is verified and activated through the one loader, rollback and
  expiry are refused, and the last-known-good policy governs through every
  failure. Nothing here changes.
- **Cadence.** The contract's two refresh moments apply: at session start
  (each protocol session, with the three-second budget, recorded as
  `policy_sync` with trigger `server_start`) and before relay. The service
  runs the relay itself on a fixed interval, so a hosted edge refreshes at
  least once per interval with no session open. The contract gains one
  sentence saying so and no new outcome.
- **Enrolment.** `connect` needs an enrolment token an owner mints in the
  hub, single-use and short-lived. For a service:
  1. An owner of the organisation mints a token in the hub, naming the edge
     (for example "hosted edge").
  2. The operator runs, once, on the service machine, as the service's user
     and against the service's home, `commonmeasure connect
     https://hub.commonmeasure.ai --token <token> --managed`, with the
     service stopped. The private key is generated on that disk and never
     leaves it. The token is typed into that one command and is spent by it.
  3. `connect` writes `edge-key.json`, `enrolment.json`, `relay.json` and
     `deployment.json`, makes a first synchronisation and a first relay run,
     and exits non-zero if the signer could not be read or no policy was
     activated.
  4. The service is started. It refuses to start in service mode without an
     enrolment record and a managed deployment, because a hosted edge that
     is not enrolled would sign nothing and deliver nothing, and one that
     is `local` would take its policy from a file nobody at the organisation
     can see.
- **The issuer pin.** The same enrolment reads the hub's token issuer (its
  origin and JWKS URL) and writes it to the service configuration beside
  `deployment.json`, not into it, so the deployment-mode contract and its
  unknown-field rule are untouched. The hub already returns its policy
  signer to an enrolled edge and may return it in the enrolment answer
  ([`docs/contracts/policy-envelope.md`](../contracts/policy-envelope.md)
  §The hub side); it returns the issuer beside it.
- **Re-pinning.** Today a new signer can be pinned only by `disconnect`
  followed by `connect --managed` with a fresh token, which mints a new key
  id: the edge becomes a different agent to publishers and in Fleet
  evidence. That is acceptable for a signer compromise and wrong for a
  routine rotation. A command that re-reads the signer and issuer for the
  existing enrolment, keeping the key id, is §Risks and open questions,
  item 8; it serves every managed edge, not only the hosted one.

## Deployment

**Compute Engine beside the hub, not Cloud Run.** The hosted edge runs in
Google Cloud beside the hub, as its own workload with its own service
account, never inside the hub's process, following the shape of the hub's
own hosted deployment: the same region, Secret Manager for
secrets, an external HTTPS load balancer holding a Google-managed
certificate, and the operator's acts written as scripts that print what
they did. It departs from that shape in one place: the compute is a single
Compute Engine virtual machine with a zonal persistent disk, not a Cloud Run
service, for the reasons in §Where the record lives. Cloud Run becomes the
right choice when the record moves to a store with its own concurrency
control, and not before.

**Project.** One Google Cloud project per organisation's hosted edge, in
Common Measure Ltd's organisation. Access to an organisation's operator
record is then an IAM grant on one project, billing is attributable, and
closing the organisation's hosted edge is deleting one project.

**Address.** `https://<label>.edge.commonmeasure.ai/mcp/<host>`, one path
per host word. `<label>` is an opaque identifier, not the organisation's
name, because every certificate issued for a hostname is published in
Certificate Transparency logs and a consultancy's or a regulated firm's use
of the product is not Common Measure Ltd's to publish. The protected
resource metadata is served at the path-suffixed well-known location for
each endpoint, and its `resource` is the endpoint URL exactly as a host
canonicalises it: lower-case scheme and host, no trailing slash, path kept.
The console is `https://console.<label>.edge.commonmeasure.ai`. The address
has an IPv4 record, which Claude requires.

**Certificate.** A Google-managed certificate on the load balancer for the
two hostnames, issued once DNS resolves to the load balancer's address, as
the hub's is. The DNS records are created in the `commonmeasure.ai` zone by
the owner of that zone.

**Secrets.** The edge's private key, `relay.json` (the ingest key) and the
enrolment and deployment records are written by `connect` onto the
persistent disk, mode `0600`, owned by the service's user, and never copied
off it. Provider credentials (search and supply keys) are held in Secret
Manager and written at boot to `<home>/credentials.env`, mode `0600`, so the
server loads them through the existing loader and records
`credentials_loaded` with the file's digest and variable names, never a
value (`DECISIONS.md` §Integration and ownership). Who can read the disk
is who holds compute or disk rights on the organisation's project.

**Scale: one machine, one process, on purpose.**

- Protocol sessions are held in the process's memory; a second process
  would not know them and would answer `404` to half the requests.
- The record's guarantees need one writer per session file and a lock that
  locks, which one machine gives and two do not.
- The relay and the policy refresh run in the process on an interval.
- A tool call is a few outbound HTTP requests and a few appends, so one
  small machine is expected to serve a first organisation. No measurement
  of that exists; the first organisation's load is the measurement, and the
  machine type is the one thing to change if it fails.
- The machine is a stateful managed instance group of size one: "stateful
  disks are always preserved on instance autohealing, update, and
  recreation operations", and an update uses the replacement method
  `RECREATE`, which requires "maxSurge to be 0", so an update never starts a
  second machine beside the first
  (<https://cloud.google.com/compute/docs/instance-groups/configuring-stateful-disks-in-migs>
  and
  <https://cloud.google.com/compute/docs/instance-groups/rolling-out-updates-to-managed-instance-groups>,
  both 3 September 2026, `spec-verified`). No page read says autohealing
  never runs two machines at once, so the service also takes an exclusive
  lock on the operator home at start and refuses to serve without it.
- The binary is installed from the release on a stock image and run by the
  init system; no container is needed for a single static binary, and
  Compute Engine's container startup agent "is deprecated"
  (<https://cloud.google.com/compute/docs/containers/deploying-containers>,
  3 September 2026, `spec-verified`).
- The load balancer's backend service timeout "default value is 30
  seconds" and covers the whole response
  (<https://cloud.google.com/load-balancing/docs/backend-service>, 9
  September 2026, `spec-verified`). It is set above the longest tool call a
  host waits for: Claude's timeout is 300 seconds (its building page above).
- Availability is one zone. A machine failure is recovered by recreating the
  machine on the same disk; a zone loss is recovered from the last snapshot,
  with the window since that snapshot lost. Snapshot schedules run "hourly,
  daily, or weekly"
  (<https://cloud.google.com/compute/docs/disks/scheduled-snapshots>, 3
  September 2026, `spec-verified`). That is §Risks and open questions,
  item 10.

**Cost, order of magnitude, before traffic.** Tens of pounds a month per
organisation. At the list prices the pricing pages show by default
(Iowa; London is higher), `spec-verified`:

- the machine, an `e2-small`, "$0.016752855 / 1 hour", about $12 a month
  (<https://cloud.google.com/products/compute/pricing/general-purpose>, no
  date shown);
- a 20 GiB balanced disk at "$0.000136986 / 1 gibibyte hour", about $2, and
  incremental snapshots at "$0.000068493 / 1 gibibyte hour", about $1
  (<https://cloud.google.com/compute/disks-image-pricing>, no date shown);
- the load balancer's forwarding rule, "First 5 forwarding rules | $0.025 /
  1 hour", about $18 (<https://cloud.google.com/vpc/network-pricing>, no date
  shown).

About $35 a month in all. For comparison, the smallest Basic HDD Filestore
share is 1 TiB at $0.16 per GiB-month, about $164 a month
(<https://cloud.google.com/filestore/pricing>, no date shown, and
<https://docs.cloud.google.com/filestore/docs/service-tiers>, 3 September
2026), and Cloud Run would still mount it without locks. The hub's
authorisation server adds no infrastructure: it runs in the hub's existing
server.

**When a firm runs its own.** The design does not depend on Common Measure
Ltd operating the machine; it depends on an enrolment, a managed
deployment, a POSIX disk and a public HTTPS address. A firm that runs its
own hosted edge in its own cloud:

- holds its operator record in its own infrastructure, so Common Measure
  Ltd holds none of it and the console's access is the firm's own;
- still enrols with the hub and still takes managed policy from it, or runs
  `local` with a policy file it maintains on that disk;
- can keep the hub as the authorisation server (the hub issues tokens whose
  audience is the firm's endpoint) or place its own provider in front,
  taking on the per-host registration matrix in §Authentication and the
  principal;
- runs the same binary; nothing in the service mode is specific to Google
  Cloud except the recipe.

## Per host, the registration step

Every step below is `planned`: the hosted edge does not exist. For each
host:

- a step becomes `spec-verified` when the hosted edge and the hub's
  authorisation server meet every requirement the cited page states for that
  step, checked by a test that reads the served metadata and the token
  exchange against those stated rules;
- it becomes `live-verified` when an administrator has performed it against
  the deployed hosted edge and a session through that host has recorded a
  mediated crossing, committed as the other hosts' sessions are.

The sequences are the documentation's, cited in §Authentication and the
principal; the endpoint names the hosted edge's address.

### Claude custom connector

For a Team or Enterprise organisation ("only Owners can add them"):

1. Organization settings > Connectors > Add > Custom > Web.
2. Enter `https://<label>.edge.commonmeasure.ai/mcp/claude-connector`.
3. Leave the OAuth client fields under Advanced settings empty, so Claude
   uses its published client metadata document, or chooses "Use Claude's
   published identity" where the two-step dialog is offered. Authentication
   settings "can't be changed after a connector is added"; changing them
   means removing and re-adding it, and every member reconnects.
4. Add.
5. Each member: Customize > Connectors > Connect, signs in at the hub,
   consents, and returns to Claude.

An individual account adds the connector for itself; custom connectors are
available "for users on Free, Pro, Max, Team, and Enterprise plans", and
"Free users are limited to one custom connector". The connector then serves
claude.ai, Claude Desktop and Cowork alike: Claude "connects to your remote
MCP server from Anthropic's cloud infrastructure … across every Claude
client".

### ChatGPT

For a Business, Enterprise or Education workspace
(<https://help.openai.com/en/articles/12584461>, "Updated: 24 days ago"):

1. The admin enables developer mode: Workspace Settings > Permissions &
   Roles > Connected Data > Developer mode (Business: each admin or owner
   for themselves; Enterprise and Education: by role).
2. Workspace settings > Apps > Create; the endpoint
   `https://<label>.edge.commonmeasure.ai/mcp/chatgpt`; authentication OAuth,
   client registration by metadata document.
3. Scan Tools; Create. The app appears under Drafts.
4. Enterprise and Education: Configure Actions and Configure Access.
5. Publish. ChatGPT keeps "a 'frozen' snapshot" of the tools, so a change to
   the tool definitions reaches members only when an admin publishes again;
   on Business an app "cannot be updated after publishing" and is recreated.
6. Each member connects the app and signs in at the hub.

The help page says Pro accounts "can connect MCPs with read/fetch
permissions in developer mode", which the three read-only tools fit; the
developer-mode page also lists Plus. The two pages disagree, and the first
live registration settles which applies.

### Microsoft 365 Copilot

Through Copilot Studio, first, because it takes the edge by URL and
discovers the authorisation server itself:

1. The Power Platform admin confirms the organisation's data policy allows
   the connector, and that generative AI features are enabled.
2. In the agent: Tools > Add a tool > New tool > Model Context Protocol;
   server name, description, and the URL
   `https://<label>.edge.commonmeasure.ai/mcp/m365-copilot`.
3. Authentication: OAuth 2.0, Dynamic discovery. Create. With dynamic
   discovery the client registers itself; the page says only that "a
   callback URL might appear", and whether it then needs registering at the
   authorisation server is not documented. The first live registration
   settles it.
4. Create a new connection, sign in at the hub, Add to agent. Generative
   orchestration on.
5. Channels > Teams and Microsoft 365 Copilot > Make agent available in
   Microsoft 365 Copilot > Add channel; Availability options > Show to
   everyone in my org > Submit for admin approval
   (<https://learn.microsoft.com/en-us/microsoft-copilot-studio/publication-add-bot-to-microsoft-teams>,
   17 August 2026).
6. The Teams administrator approves the agent in the Teams admin center.

The declarative-agent route (Microsoft 365 Agents Toolkit: Declarative
agent > Add an Action > Start with an MCP Server > the URL > the
authentication type > Provision) needs either DCR that issues a client
secret or a manual OAuth client registration in the Teams Developer Portal
with the redirect above, "Any Teams app" selected and PKCE enabled. One
Microsoft page says the Developer Portal does not support DCR and another
points DCR configuration at it; the Copilot Studio route avoids the
conflict.

### Copilot cloud agent on GitHub

No OAuth, so no person signs in:

1. The operator issues an edge token on the hosted edge for one named
   principal (for example `ci:<repository>`), which prints the token once
   and stores only its hash; the policy binds that label.
2. A repository administrator adds an Agents secret named
   `COPILOT_MCP_COMMONMEASURE_TOKEN` holding it; "Only Agents secrets and
   variables with names prefixed with COPILOT_MCP_ will be available to your
   MCP configuration".
3. Settings > Copilot > MCP servers, saves:

   ```json
   {"mcpServers": {"commonmeasure": {"type": "http",
     "url": "https://<label>.edge.commonmeasure.ai/mcp/copilot-cloud-agent",
     "headers": {"Authorization": "Bearer $COPILOT_MCP_COMMONMEASURE_TOKEN"},
     "tools": ["context_fetch", "context_search", "context_status"]}}}
   ```

4. Assigns an issue to Copilot, opens the session, and reads the "Start MCP
   Servers" step.

Revoking the token at the edge ends the repository's access at its next
call. The cloud agent's firewall "does not apply to Model Context Protocol
(MCP) servers", so it neither stops the agent reaching the hosted edge nor
limits what the edge fetches; the edge's policy is the rule on both.

## What it does not do

- It sees only what the host routes to it. The host's model decides, per
  call, whether to use `context_fetch` or its own tools. The host's own web
  search and fetch never cross the hosted edge: they run in the vendor's
  infrastructure, and an MCP server receives only the tool calls addressed
  to it. The same holds for the cloud agent's own shell.
- It records no observed crossings, no turn boundaries, no prompt sources
  and no context snapshots: none of the four hosts delivers a lifecycle
  event to a remote server. The cloud agent's hooks run shell commands
  inside its own sandbox, where there is no operator home. `named_by` on every crossing is `unknown`, because no
  prompt is recorded
  ([`docs/contracts/host-integration.md`](../contracts/host-integration.md)
  §2).
- It issues no nudge: no host runs a session-start hook against it.
- The only complement for what the host fetches itself is the observed
  path: a browser adapter on the person's own machine, which records after
  the fact and cannot refuse. That is a separate package and is not
  designed here.

## Risks and open questions

1. **The hub as a sign-in dependency.** When the hub is down, nobody can
   obtain or refresh a token, and after at most an hour every call answers
   `401`. Recommendation: accept it; it makes the tools unavailable and never
   weakens policy. Keep the access-token lifetime at an hour so a leaver's
   access also ends within one.
2. **The 2026-07-28 protocol revision.** Under it there is no protocol
   session, so the record's session has to be defined by the edge.
   Recommendation: build 2025-06-18 and 2025-11-25 now; when a target host
   sends 2026-07-28, define a session as the run of requests from one
   principal, one host word and one client name with no gap longer than the
   idle interval, minted and ended by the edge, and resolve policy at its
   first request. Rejected: a session per principal per day, which would
   hold a policy resolved in the morning against a revision published at
   noon.
3. **One scope per hosted edge.** One working directory means one matching
   scope, one governing engagement and one telemetry clearance for the whole
   organisation. A consultancy with an engagement per client cannot express
   that on one hosted edge. Recommendation: accept it for the first
   organisation. If a second needs more, let the service configuration
   declare several endpoints per host, one per engagement, each with a
   directory recorded as its sessions' `cwd`, stated in the contract as the
   declared directory of a hosted endpoint rather than a process's
   directory. Do not key scopes on principals, which would be a second
   resolver.
4. **The cloud metadata server and the private network.** On a Google
   Cloud machine `metadata.google.internal` and `169.254.169.254` serve the
   machine's service-account credentials, and the machine's network reaches
   the project's private addresses. The private-address floor refuses both
   today, but `allow_private_hosts: true` lifts it, and a managed policy can
   carry that field. Recommendation: in service mode the floor holds whatever
   the policy says: `allow_private_hosts` is not honoured, `context_status`
   says so, and link-local addresses and the metadata hostname are refused
   even under a named `record_internal_prefixes` entry. Declining a
   permissive field is stricter than the policy, never looser.
5. **Two registrations for one person.** A person with Claude Desktop's local
   entry and the organisation's connector sees two sets of tools with the
   same names. The record keeps them apart by `host` (`claude-desktop`
   against `claude-connector`), but the model may call either.
   Recommendation: the organisation chooses one per surface; `doctor`
   reports the local entry, and the connector's description says which it
   is.
6. **DCR registrations accumulate.** Claude registers a client on every
   fresh connection under DCR. Recommendation: the hub advertises CIMD with
   `none` so Claude and ChatGPT use metadata documents, keeps DCR for
   Microsoft, and deletes a dynamically registered client that has held no
   grant for thirty days.
7. **Undocumented Microsoft behaviour.** Neither Copilot Studio's callback
   URL, nor whether it sends `resource`, nor its egress ranges are
   documented. Recommendation: register Microsoft 365 Copilot third, after
   Claude and ChatGPT have proven the authorisation server, and record what
   it sends at the first live registration.
8. **Signer and issuer rotation.** Covered in §Policy. Recommendation: add a
   command that re-reads the signer and the issuer for the existing
   enrolment and keeps the key id, before a second organisation is onboarded;
   it serves every managed edge.
9. **Console access for the firm's reviewers.** IAP's default client admits
   only Common Measure Ltd's own Google organisation, and IAP names Google
   identities; a firm on Microsoft accounts has no reviewer IAP can name
   without the external-identity setup. Recommendation: for the first
   organisation, IAP with the external-identity setup and named accounts;
   when a firm cannot use it, the console signs in through the hub's
   authorisation server as an OAuth client, which is a console change and a
   separate box.
10. **One zone, one machine.** A zone outage stops the tools and loses the
    record written since the last snapshot. Recommendation: hourly
    snapshots, accepted for the first organisation; state the recovery point
    in the organisation's agreement. A regional disk is the next step if an
    organisation requires more.
11. **Common Measure Ltd holds a firm's operator record.** On a local edge
    the private record never leaves the firm's machines, and the hub sees
    only the projection. On a hosted edge Common Measure Ltd operates the
    machine the record lives on. Recommendation: say so in the decision and
    in the organisation's agreement; keep the record in a project of its
    own with access only by named grant; delete the project on the
    organisation's instruction, and thirty days after it closes its hosted
    edge, matching the hub's retention rule for delivered telemetry.
12. **The static token for the cloud agent.** It is a secret in repository
    settings, and the cloud agent uses tools without approval.
    Recommendation: register this host last; one token per repository, one
    principal per token, revocable at the edge; the record's basis says
    `edge_token`.

## Build sequence

Each step is the smallest change that the next depends on, with the test
that proves it and an estimate in days of one worker's focused work.

| # | Change | Test that proves it | Days |
|---|---|---|---|
| 1 | Principal from an authenticated subject: `oauth_subject` and `edge_token` bases, the `subject` and `edge_token` binding keys (exactly one of the three per binding), resolution unchanged otherwise | unit tests in `crates/commonmeasure-harness/src/policy.rs` for binding selection, the both-or-neither refusal and fail-closed; the existing policy identity vectors digest unchanged | 1 |
| 2 | Session start separated from the stdio loop: the per-process work of `serve_mcp` becomes a session constructor both transports call; the host word is a session property | `crates/commonmeasure-cli/tests/mediated_e2e.rs` and `crates/commonmeasure-cli/tests/managed_policy.rs` pass unchanged | 1 |
| 3 | Streamable HTTP, 2025-06-18 and 2025-11-25, loopback listener: POST per host path, session id minted and bound, `404`, `DELETE`, `405` on `GET`, `Origin` and protocol-version checks, JSON responses, `readOnlyHint` on the three tools | a new real-binary test, `hosted_mcp.rs`, driving the server over HTTP against a loopback origin with the assertions of `mediated_e2e.rs` (refusal before the crossing, content hash, client identified once, no file for an idle session), plus two concurrent sessions leaving two files and an unknown path and a bad `Origin` refused | 2.5 |
| 4 | The resource server: protected resource metadata per endpoint, `401` with `WWW-Authenticate`, JWT verification against a JWKS from the pinned issuer with a disk cache, audience and organisation checks, the session bound to its subject; edge tokens issued, hashed and revoked | `hosted_mcp.rs` against a loopback issuer that serves a JWKS and signs tokens, as `managed_policy.rs` runs the edge against a loopback endpoint verifying as the hub does: wrong audience, issuer, organisation, expiry, another subject's session and a revoked edge token each refused by name; a crossing records `oauth_subject` and the bound label | 2.5 |
| 5 | Service mode: a service configuration (origin, issuer, enabled host words), refusal to start unenrolled, unmanaged or without the home's lock, the relay and policy refresh on an interval, the private-address floor held whatever the policy says, `doctor` and `status` reporting the mode | real-binary tests: the relay delivers to a loopback receiver with no `relay` command run; fetches of `169.254.169.254`, `metadata.google.internal` and a private address refused under `allow_private_hosts: true`; an unenrolled home, and a second process on a locked home, refuse to start | 1.5 |
| 6 | Contracts: `docs/contracts/host-integration.md` §1, §5 and §6 carry the hosted edge; `docs/contracts/session-evidence.md` carries the bases and the hosted session id; `docs/contracts/policy-envelope.md` §Cadence and staleness carries the interval refresh | `crates/commonmeasure-cli/tests/docs_paths.rs`; every quoted command run | 1 |
| 7 | The hub's authorisation server, on the hub's own board: authorisation-server metadata with `S256`, CIMD with `none`, DCR issuing secrets, the `iss` response parameter, `offline_access` and refresh, sign-in and membership check, the resource-to-organisation registry, JWKS, the issuer in the enrolment answer | the hub's own tests, and the ignored live test of step 9 | 5 |
| 8 | Deployment for the first organisation, kept outside this repository with the other deployment recipes: project, machine and disk, snapshot schedule, load balancer host rules and certificate, IAP on the console, Secret Manager, the enrolment act | the hosted endpoint answers `401` with resource metadata over HTTPS; the console answers only through IAP | 2 |
| 9 | One recorded mediated crossing through one host: the Claude custom connector, first, because it takes CIMD and DCR "out of the box", is open to individual accounts without a workspace administrator, and reaches the edge from a published range | a real session committed in a directory under `demo/host-sessions/` named for the host, read by `crates/commonmeasure-cli/tests/recorded_sessions.rs`: `host: claude-connector`, the client's own name, `authentication_basis: oauth_subject`, one `crossing_mediated` with its content hash; an ignored live test against the running hub's authorisation server | 1 |

Edge: 10.5 days (steps 1 to 6 and 9). Hub: 5. Operations: 2. The first
organisation's registration of ChatGPT and Microsoft 365 Copilot follows
step 9 at about one day each, and the cloud agent after them.

Step 7 is the longest single dependency and sits on another board. If it
slips, the Copilot cloud agent can take step 9's place: it authenticates
with an edge token (step 4), needs no authorisation server, and a recorded
crossing through it proves the transport, the tenancy, the record's
placement and the deployment. It needs a GitHub plan whose cloud agent may
use MCP servers, and it moves the edge-token basis, the weaker one, to the
front.

## The Claude Desktop bridge

Claude Desktop announces the local server's tools to its device bridge, and
a claude.ai chat in the browser on the same account cannot call them
(host-surfaces §Claude in the browser and Claude in Chrome, `live-verified`),
so the hosted edge is the path for the browser and for Claude in Chrome.
