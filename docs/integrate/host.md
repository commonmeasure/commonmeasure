---
title: Connect an agent host
domain: edge
audience: integrator
section: integrate
---

# Connect an agent host

A host reaches the edge in two ways. The **mediated** tools are an MCP
server: the agent calls them instead of its own fetch and search, so policy
rules on each crossing before the content moves. The **hooks** record what
the host's own tools did after the call; they cannot refuse. A host can use
either or both. The formats are in
[`docs/contracts/host-integration.md`](../contracts/host-integration.md);
terms such as crossing, host and edge are in [`docs/GLOSSARY.md`](../GLOSSARY.md).

You need `commonmeasure` 0.4.0 or later on `PATH`
([`docs/INSTALL.md`](../INSTALL.md)). The release download and
`commonmeasure --version` were checked on 0.4.4 on macOS (Apple silicon). The integration commands below were run on 0.4.0
with loopback servers standing in for the web. Claude Code registration
and `doctor claude` were rerun on 0.4.6 (29 September 2026), in a scratch
home. Other host and integration commands retain their 0.4.0 verification
baseline and have not been rerun here.

## 1. A host the binary already knows

For Claude Code, Codex, Pi, Claude Desktop, Cursor, the Copilot CLI and VS
Code the binary writes its own registration and reads it back. The host
words, files and capabilities are in the host-integration contract, §1 and §2.

### Register with the host

```sh
commonmeasure install claude
```

This writes the five hooks (`SessionStart`, `PostToolUse`,
`UserPromptSubmit`, `Stop`, `SessionEnd`) into `~/.claude/settings.json` and the MCP
server at user scope into `~/.claude.json`, each naming the binary by its
resolved absolute path, because a hook runs outside your login shell's
`PATH`. In the same settings it pre-approves `context_fetch` and
`context_search` (`mcp__commonmeasure__context_fetch` and
`mcp__commonmeasure__context_search` in `permissions.allow`), so a session
does not ask before each fetch or search; `context_enrol` and
`context_status` still ask. Only this product's entries are written;
everything else in those files is kept. Replacing the binary at that path
changes what the next session runs, with no reinstall. `commonmeasure
uninstall claude` removes exactly those entries, keeps a pre-approval you
had written yourself, and leaves `~/.commonmeasure/` alone.

The Claude Code plugin in `plugin/` is the other registration route, for
marketplace and archive installs; it declares the same hooks and carries no
binary (`plugin/README.md`). With the binary installed, the repository
itself is a marketplace, and the plugin's launcher finds the binary on
`PATH` or in `~/.local/bin`:

```sh
claude plugin marketplace add commonmeasure/commonmeasure
claude plugin install commonmeasure@commonmeasure
```

Use one route or the other: `install claude`
refuses while the plugin is enabled, because two registrations record every
crossing twice. `commonmeasure install codex` and `commonmeasure install
pi` register the mediated tools with those hosts the same way
(`plugin/README.md` §Codex and Pi); the Codex table also carries the
approval mode Codex needs to call the tools without asking, and it serves
the Codex CLI, the ChatGPT desktop app and the Codex IDE extension alike.
`commonmeasure install claude-desktop` and `commonmeasure install cursor`
register with those two applications (`plugin/README.md` §Claude Desktop
and Cursor). Neither sends a session-end event, so a source whose licence
demands usage reporting is refused in their sessions until a background
relay runs on the Edge home; `install claude-desktop` says so and names the
command ([Relay without a session end](../INSTALL.md#relay-without-a-session-end)). `commonmeasure install copilot`
and `commonmeasure install vscode` register with the Copilot CLI and VS
Code (`plugin/README.md` §GitHub Copilot and VS Code). `commonmeasure install chrome` registers the binary
for the browser extension in `browser/`, which records the sources ChatGPT
on the web and Bing Copilot Search show (`browser/README.md`).

The host-registration command supports every host listed above.

### Check what arrived

```sh
commonmeasure doctor claude
```

`doctor` prints one report: the Edge home (whether the sessions directory
can be written, whether the policy file loads), the console (whether one
answers and where its Policy page is), the relay, and then the host: which
of the five hooks are registered and in which file, whether the MCP server
is registered and its command, the binary each names and the version that
binary reports when run, which of the two pre-approvals the settings hold,
and whether a plugin is installed beside the registration. Each finding is marked `✓`, `!` (something to act on), `?`
(could not be determined) or `·` (a fact), and the report ends with what
needs attention. A registration whose binary has gone is reported as not
found and marked `!`, which is the one state in which every hook exits
without recording and nothing in the session says so. `commonmeasure doctor`
with no host reports every host; `--json` prints the same findings as a
document for a script or a support thread; `--color never` drops the colour
in a terminal.

If every fetch is refused with a name that "resolves to a local or private
address", `commonmeasure doctor --resolve <name>` looks the name up and says
which range answered. `198.18.0.0/15` means a fake-IP proxy (Clash, Surge,
sing-box) is answering names on this machine: set it to return real addresses
to the machine running the edge. `100.64.0.0/10` is a tailnet or carrier-grade
NAT; [source policy §Recording](../contracts/source-policy.md#recording) says how
to allow one host.

## 2. The mediated tools over stdio

Start the server as a child process of the host, in the directory the work
belongs to. It resolves the policy scope from that directory once, at start.

```sh
commonmeasure mcp --host claude-code --session <your session id>
```

- `--host` is one of `claude-code`, `codex`, `pi`, `claude-desktop`,
  `cursor`, `copilot-cli` or `vscode`, and is recorded on every record. Any
  other word is refused.
- `--session` names the session log. Without it the server takes
  `AGENT_SESSION_ID` from its environment, or mints `local-<millis>-<pid>`.
  To keep one log, pass the host's own session id, the one its hook
  payloads carry.
- The server records the `clientInfo` name and version your host sends in
  `initialize`. Send them: they are what tells one host from another under
  the same `--host` word.

It offers `context_fetch`, `context_search`, `context_status` and
`context_enrol`. To try it without a host, use a throwaway operator home,
serve a page on loopback and allow the edge to reach private addresses,
which it refuses by default
([source policy §Recording](../contracts/source-policy.md#recording) says
which addresses are private, and how to allow a single tailnet host):

```sh
export COMMONMEASURE_HOME=$(mktemp -d)
mkdir -p site && printf '<title>Opening hours</title><p>The library opens at nine.</p>\n' > site/hours.html
python3 -m http.server 8401 --bind 127.0.0.1 --directory site &
cat > "$COMMONMEASURE_HOME/policy.json" <<'EOF'
{"policy_mode": "strict", "allow_private_hosts": true,
 "constraints": [{"kind": "denied_source_host", "host": "localhost"}]}
EOF
```

`localhost` is a second name for the same server, denied so that one fetch
is admitted and one refused. Then send the server one JSON-RPC message per
line:

```sh
{
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"my-host","version":"1.0"}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/initialized"}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"context_fetch","arguments":{"url":"http://127.0.0.1:8401/hours.html"}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"context_fetch","arguments":{"url":"http://localhost:8401/hours.html"}}}'
} | commonmeasure mcp --host claude-code --session demo-session
```

The admitted fetch returns one text item holding a JSON object: the page's
readable text in `content`, `content_hash` (the SHA-256 of exactly that
text), `retrieved_hash` (the bytes the origin served), the policy ruling and
the source's declarations. Abridged:

```json
{"url":"http://127.0.0.1:8401/hours.html","content_hash":"sha256:9e5ddf53…","http_status":200,
 "policy":"Admitted; no constraint excluded it.","content":"Opening hours\nThe library opens at nine."}
```

The refused fetch is a tool error, and the page is never requested:

```json
{"content":[{"type":"text","text":"{\"error\":\"refused before the crossing: The job denies host localhost. (operator policy in <home>/policy.json)\"}"}],"isError":true}
```

With no policy file, as in a real home that has none, the mode is `observe`:
the edge records the operator's own rules and enforces none of them. The
source's terms still refuse in every mode
([fail policy](../FAIL-POLICY.md) §6).

## 3. The mediated tools over Streamable HTTP

For a host that reaches MCP servers only from its vendor's cloud. This path
needs an edge enrolled with Common Measure Hub (`commonmeasure connect
<hub-url>`), because the hub is the issuer whose OAuth
tokens it accepts. The exchange below ran on loopback against an enrolment
record written the way the test suite writes one, not against a hub; a
deployed edge sits behind TLS at its public origin and runs as
`commonmeasure hosted service` (contract §1, The hosted path).

The endpoints are `<origin>/mcp/claude-connector`, `/mcp/chatgpt`,
`/mcp/m365-copilot` and `/mcp/copilot-cloud-agent`. A host with no OAuth
presents a token the edge issues:

```sh
TOKEN=$(commonmeasure hosted token issue ci-agent --host copilot-cloud-agent)
commonmeasure hosted serve --listen 127.0.0.1:8765 --origin http://127.0.0.1:8765 &
```

```text
commonmeasure: issued edge token for "ci-agent"; only its hash is kept in <home>/hosted-tokens.json
commonmeasure: hosted edge for <organisation> at http://127.0.0.1:8765, endpoints http://127.0.0.1:8765/mcp/claude-connector … http://127.0.0.1:8765/mcp/copilot-cloud-agent, issuer <hub>
listening on http://127.0.0.1:8765
```

The token is printed once, on standard output. `initialize` returns the
session in `Mcp-Session-Id`; send it on every later request, and `DELETE`
to end the session:

```sh
E=http://127.0.0.1:8765/mcp/copilot-cloud-agent
SESSION=$(curl -s -D - -o /dev/null -X POST $E -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"my-host","version":"1.0"}}}' \
  | tr -d '\r' | sed -n 's/^Mcp-Session-Id: //p')
curl -s -X POST $E -H "Authorization: Bearer $TOKEN" -H "Mcp-Session-Id: $SESSION" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"context_fetch","arguments":{"url":"http://127.0.0.1:8401/hours.html"}}}'
curl -s -X DELETE $E -H "Authorization: Bearer $TOKEN" -H "Mcp-Session-Id: $SESSION"
```

A request without a token is `401` with `WWW-Authenticate` naming the
endpoint's protected resource metadata. The hosted endpoint does not offer
`context_enrol`. The fetch is recorded with `principal: token:ci-agent` and
`authentication_basis: edge_token`.

## 4. The hooks

The host runs `commonmeasure hook <event> --host <word>` with its hook
payload as JSON on stdin. The command always exits zero and never blocks the
agent; a payload it cannot read records nothing.

| Event | When the host runs it | What it records |
|---|---|---|
| `session-start` | a session begins | the host process; prints the standing nudge on stdout for the host to add to context |
| `post-tool-use` | after a fetch, search or third-party MCP call | an observed crossing |
| `user-prompt-submit` | a prompt is submitted | the turn start and the URLs the prompt named |
| `stop` | a turn ends | the turn end and a context snapshot |
| `session-end` | the session ends | the session end, and starts a relay run where a receiver is configured |

The minimum for an observed crossing is `session_id`, `cwd`, `tool_name`,
`tool_input` and `tool_response`. In Claude Code's shape:

```sh
printf '%s' '{"session_id":"host-session-1","cwd":"/work/acme","hook_event_name":"PostToolUse","tool_name":"WebFetch","tool_input":{"url":"https://www.example.org/opening-hours"},"tool_response":{"result":"The library opens at nine."}}' \
  | commonmeasure hook post-tool-use --host claude-code
```

The hook reads the payload shapes of the hosts `--host` names (contract §2
has each host's fields). A tool name it does not recognise as a fetch, a
search or an MCP result yields no crossing. Loopback and private addresses
are not recorded by the hooks unless the policy names them in
`record_internal_prefixes`.

## 5. See the result

```sh
commonmeasure session host-session-1
```

Among its lines:

```text
records    1
crossings  1 observed, 0 mediated, 0 refused, 0 reconstructed
grounded   1 put page text into the model's context

crossings
  observed   grounded  https://www.example.org/opening-hours
```

For the stdio session of §2, `commonmeasure session demo-session` lists
`client     my-host 1.0 via host claude-code`, one mediated crossing and one
refused. The raw records are one JSON object per line in
`~/.commonmeasure/sessions/<session id>.ndjson`
([`docs/contracts/session-evidence.md`](../contracts/session-evidence.md)),
and `commonmeasure serve` shows them in the console. To check that a record
matches what the model saw, hash the text:

```sh
printf '%s' 'The library opens at nine.' | shasum -a 256
```

```text
da217a1f19b6935706d042384fd65fe4523d1139526d017e7b0ac68ab2f68346  -
```

That is the `content_hash` the hook recorded for the crossing.

## Reporting from a host

A source whose licence demands usage reporting is admitted only where the
session's events leave without anyone running a command. Among local hosts
on their own, that is Claude Code alone, through its `session-end` hook; a session under
`--host claude-code` whose `clientInfo` name is not `claude-code` does not
count. Elsewhere such a source is refused unless `commonmeasure hosted
service` or a background relay (`commonmeasure relay --every`, or
`commonmeasure service install relay` on macOS) runs on the same home. In
every case the operator must have agreed to reporting (`commonmeasure
consent agree`) and `relay.json` must name a receiver; the session's policy
scope need not clear telemetry egress
([`docs/contracts/session-evidence.md`](../contracts/session-evidence.md)
§Source declarations and §Reporting consent).
