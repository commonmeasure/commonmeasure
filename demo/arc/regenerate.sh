#!/bin/sh
# Regenerate the demonstration evidence home (demo/arc/home) from the arc's
# own scripted sessions, against the shipped binaries and nothing else.
#
# The demonstration never shares the operator's real ~/.commonmeasure store,
# whose recorded cwds and URLs cross engagements. This script builds the store
# the console will show instead: every record in it is written by the shipped
# `commonmeasure` binary exercising its real code path — the hook entry point
# with the documented host payload shape, the mediated MCP server over stdio
# with real HTTP fetches from a loopback origin, the policy mode switched by
# rewriting policy.json between sessions, the relay against a loopback
# receiver. What is scripted is the
# driving, and demo/arc/README.md states exactly which moments a live harness
# session replaces; no record claims a harness ran when none did (the observed
# sessions record tool payloads this script composed, under obviously
# fictional .example hosts).
#
# Offline: no credential is read, no external host is contacted, and nothing
# is billable. The only sockets are loopback: the origin serving
# demo/injection/corpus and the telemetry receiver.
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
home="$repo/demo/arc/home"
origin_port=8377
receiver_port=8378
origin="http://127.0.0.1:$origin_port"
receiver="http://127.0.0.1:$receiver_port"
bin="$repo/target/debug/commonmeasure"

fail() { echo "demo-arc: $1" >&2; exit 1; }

pids=""
cleanup() {
    for pid in $pids; do kill "$pid" 2>/dev/null || true; done
}
trap cleanup EXIT INT TERM

wait_for() {
    tries=0
    until curl -s -o /dev/null "$1"; do
        tries=$((tries + 1))
        if [ "$tries" -ge 50 ]; then
            if [ -n "${3:-}" ] && [ -s "$3" ]; then
                echo "demo-arc: $2 startup log ($3):" >&2
                tail -n 20 "$3" >&2
            fi
            fail "$2 did not answer at $1"
        fi
        sleep 0.1
    done
}

# A port already answering belongs to something else on this machine; refusing
# is better than scripting against a stranger.
for url in "$origin" "$receiver"; do
    if curl -s -o /dev/null --max-time 1 "$url" 2>/dev/null; then
        fail "$url is already in use; stop what is listening there first"
    fi
done

echo "==> building the shipped binary (target/debug/commonmeasure)"
cargo build -p commonmeasure-cli --offline --quiet

echo "==> wiping and declaring the demonstration home: $home"
rm -rf "$home"
mkdir -p "$home/workspace/fictive-systems" "$home/workspace/other-client" "$home/scripted"
export COMMONMEASURE_HOME="$home"

# The declared policy the arc starts under: observe mode, the loopback
# origin's prefix named as recordable internal supply (consent in writing,
# marked internal so egress never projects it), and two engagement scopes —
# only the demonstration engagement is cleared to leave the machine.
cat > "$home/policy.json" <<EOF
{
  "policy_mode": "observe",
  "record_internal_prefixes": ["$origin/"],
  "scopes": [
    {
      "match": "demo/arc/home/workspace/fictive-systems",
      "engagement": "fictive-systems",
      "allow_telemetry_egress": true
    },
    {
      "match": "demo/arc/home/workspace/other-client",
      "engagement": "other-client"
    }
  ]
}
EOF

# Read-time attribution for the console: the same two engagements the policy
# scopes govern, so the policy panel shows the identities agreeing.
cat > "$home/attribution.json" <<EOF
{
  "rules": [
    {"match": "demo/arc/home/workspace/fictive-systems", "engagement": "fictive-systems"},
    {"match": "demo/arc/home/workspace/other-client", "engagement": "other-client"}
  ]
}
EOF

echo "==> beat 1: the observed floor, through the shipped hook entry point"
# The hook binary is driven with the documented PostToolUse payload shape
# (plugin/README.md §Observed). In the live demonstration these records come
# from the presenter's real harness session; here the driving is scripted and
# every host is a reserved .example name, so the store is obviously synthetic.
observed_hook() {
    printf '%s' "$1" | "$bin" hook post-tool-use
}
fictive_cwd="$home/workspace/fictive-systems"
other_cwd="$home/workspace/other-client"
observed_hook "{\"session_id\":\"arc-fictive-observed\",\"cwd\":\"$fictive_cwd\",\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"WebFetch\",\"tool_input\":{\"url\":\"https://docs.fictive.example/connecting-a-data-source\"},\"tool_response\":{\"result\":\"Fictive Systems documentation: point the connector at the data source and run validation from the Orchestrator console before enabling scheduled syncs.\"}}"
observed_hook "{\"session_id\":\"arc-fictive-observed\",\"cwd\":\"$fictive_cwd\",\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"WebSearch\",\"tool_input\":{\"query\":\"fictive orchestrator validation failing\"},\"tool_response\":{\"results\":[{\"title\":\"Connecting a data source\",\"url\":\"https://docs.fictive.example/connecting-a-data-source\"},{\"title\":\"Community thread: validation keeps failing\",\"url\":\"https://community.fictive.example/thread/4821\"}]}}"
observed_hook "{\"session_id\":\"arc-fictive-observed\",\"cwd\":\"$fictive_cwd\",\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"WebFetch\",\"tool_input\":{\"url\":\"https://community.fictive.example/thread/4821\"},\"tool_response\":{\"result\":\"Community thread: most validation failures are an unreachable endpoint or a credential without read access; re-run validation after fixing either.\"}}"
observed_hook "{\"session_id\":\"arc-other-client-observed\",\"cwd\":\"$other_cwd\",\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"WebFetch\",\"tool_input\":{\"url\":\"https://kb.other-client.example/change-freeze-policy\"},\"tool_response\":{\"result\":\"Other Client knowledge base: production changes are frozen during the quarterly close; emergency fixes need a named approver.\"}}"
[ -f "$home/sessions/arc-fictive-observed.ndjson" ] || fail "the observed session was not recorded"
[ -f "$home/sessions/arc-other-client-observed.ndjson" ] || fail "the other-client session was not recorded"

echo "==> beat 2: the mediated session under observe, over a real loopback origin"
origin_log="$home/scripted/origin.log"
python3 -m http.server "$origin_port" --bind 127.0.0.1 \
    --directory "$repo/demo/injection/corpus" >"$origin_log" 2>&1 &
pids="$pids $!"
wait_for "$origin/corpus.json" "the loopback origin" "$origin_log"

# The shipped MCP server over stdio, exactly as a harness drives it. Observe
# mode carries both fetches: the clean page admitted, the hostile page carried
# with the injection screen's findings recorded as a breach.
mediated_session() {
    session="$1"
    (
        cd "$fictive_cwd"
        {
            printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"demo-arc","version":"0"}}}'
            printf '%s\n' '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"context_status","arguments":{}}}'
            printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"context_fetch\",\"arguments\":{\"url\":\"$origin/connecting-a-data-source.md\"}}}"
            printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"context_fetch\",\"arguments\":{\"url\":\"$origin/community-answer-thread.md\"}}}"
        } | "$bin" mcp --session "$session" 2>>"$home/scripted/mcp-server.log"
    )
}
mediated_session arc-mediated-observe > "$home/scripted/arc-mediated-observe.jsonl"
grep -q 'content_hash' "$home/scripted/arc-mediated-observe.jsonl" \
    || fail "the observe-mode mediated fetch returned no content hash"
grep -q 'crossing_mediated' "$home/sessions/arc-mediated-observe.ndjson" \
    || fail "no mediated crossing was recorded under observe"
grep -q 'injection-screen' "$home/sessions/arc-mediated-observe.ndjson" \
    || fail "the injection screen did not record its invocation under observe"
grep -q 'instruction_override' "$home/sessions/arc-mediated-observe.ndjson" \
    || fail "the hostile page's carried breach does not name the matched rule"

echo "==> beat 3: the policy switched from observe to strict in policy.json"
# The operator switches the mode by editing the declared policy; the console
# reads it and does not write it. The rewrite keeps every other declaration.
set_mode() {
    wanted="$1"
    python3 - "$home/policy.json" "$wanted" <<'PY'
import json, sys
path, wanted = sys.argv[1], sys.argv[2]
with open(path) as f:
    policy = json.load(f)
policy["policy_mode"] = wanted
with open(path, "w") as f:
    json.dump(policy, f, indent=2)
    f.write("\n")
PY
    grep -q "\"policy_mode\": \"$wanted\"" "$home/policy.json" \
        || fail "policy.json does not declare $wanted after the edit"
}
set_mode strict

echo "==> beat 4: the same fetch again, refused before its content enters the context under strict"
# A mediated session loads its policy at startup, so the strict fetch is a new
# server process; a session already running keeps the policy it loaded.
mediated_session arc-mediated-strict > "$home/scripted/arc-mediated-strict.jsonl"
grep -q 'refused before the content entered the context' "$home/scripted/arc-mediated-strict.jsonl" \
    || fail "strict mode did not refuse the hostile fetch"
grep -q 'crossing_refused' "$home/sessions/arc-mediated-strict.ndjson" \
    || fail "no refusal was recorded under strict"
grep -q 'crossing_mediated' "$home/sessions/arc-mediated-strict.ndjson" \
    || fail "the clean page should still be admitted under strict"

echo "==> beat 5: the relay's projection to a loopback receiver"
python3 "$repo/demo/arc/receiver.py" "$receiver_port" "$home/scripted/receiver-received.ndjson" \
    > "$home/scripted/receiver.log" 2>&1 &
pids="$pids $!"
wait_for "$receiver/" "the loopback receiver" "$home/scripted/receiver.log"

"$bin" relay --receiver "$receiver" > "$home/scripted/relay-report.txt" 2>&1 \
    || { cat "$home/scripted/relay-report.txt" >&2; fail "the relay did not deliver"; }
grep -q 'under governing engagement fictive-systems' "$home/scripted/relay-report.txt" \
    || fail "the relay report does not name the demonstration engagement's clearance"
grep -q 'withheld' "$home/scripted/relay-report.txt" \
    || fail "the relay report does not state the withheld sessions"
grep -q 'fictive.example' "$home/scripted/receiver-received.ndjson" \
    || fail "the cleared engagement's crossings did not reach the receiver"
if grep -q '127.0.0.1' "$home/scripted/receiver-received.ndjson"; then
    fail "an internal loopback crossing left the machine"
fi
if grep -q 'other-client' "$home/scripted/receiver-received.ndjson"; then
    fail "an uncleared engagement's crossing left the machine"
fi

echo "==> beat 6: the policy back to observe, ready to be switched live"
# The strict refusal stays on the record (policy is capture-time, not
# retroactive); the declared mode returns to observe so a live walk of the
# arc can perform the observe -> strict switch itself.
set_mode observe

cleanup
trap - EXIT INT TERM

cat <<EOF

demo-arc: the demonstration home is regenerated at $home
  sessions            $home/sessions/          (arc-* ids only)
  scripted transcripts $home/scripted/          (tool results, relay report,
                                                receiver log)
  declared policy     $home/policy.json        (observe; strict was declared for
                                                beat 4 and reverted)
Read it back:
  COMMONMEASURE_HOME=$home $bin session arc-mediated-strict
  COMMONMEASURE_HOME=$home $bin serve --listen 127.0.0.1:4180
Pre-share check (docs/RUN-THE-DEMONSTRATION.md): review every host, cwd and
session id this store will render before any screen is shared.
EOF
