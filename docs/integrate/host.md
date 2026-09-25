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
([`docs/GETTING-STARTED.md`](../GETTING-STARTED.md) §1). Every command below
was run against 0.4.0 on macOS with loopback servers standing in for the web.

## 1. A host the binary already knows

For Claude Code, Codex, Pi, Claude Desktop, Cursor, the Copilot CLI and VS
Code the binary writes its own registration and reads it back:

```sh
commonmeasure install claude
commonmeasure doctor claude
```

```text
claude-code: five hooks (SessionStart, PostToolUse, UserPromptSubmit, Stop, SessionEnd) registered in ~/.claude/settings.json, each naming ~/.local/bin/commonmeasure
claude-code: MCP server commonmeasure registered at user scope in ~/.claude.json, naming the same binary
claude-code: a running session picks this up on its next start
```

The output prints the home directory in full where this shows `~`. The
host words, the files each `install` writes and what each host supplies are
in the contract, §1 and §2. The rest of this page is what those
registrations do, for wiring a host by hand.

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
which it refuses by default:

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

With no policy file, as in a real home that has none, the edge records
everything and refuses nothing.

## 3. The mediated tools over Streamable HTTP

For a host that reaches MCP servers only from its vendor's cloud. This path
needs an edge enrolled with Common Measure Hub (`commonmeasure connect
<hub-url> --token <token>`), because the hub is the issuer whose OAuth
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
that is Claude Code alone, through its `session-end` hook; a session under
`--host claude-code` whose `clientInfo` name is not `claude-code` does not
count. Elsewhere such a source is refused unless `commonmeasure hosted
service` runs on the same home ([`docs/contracts/session-evidence.md`](../contracts/session-evidence.md)
§Source declarations).
