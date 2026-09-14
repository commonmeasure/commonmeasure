---
title: Run the security demonstration
---

# Run the security demonstration

This walkthrough performs the security demonstration from a clean shell. It
shows a malicious web page being blocked before its content reaches the
model, with the block recorded and explained, using the real product end to
end rather than a staged recording. Terms like crossing, engagement,
clearance and admission are defined in [`docs/GLOSSARY.md`](GLOSSARY.md).

Keep the claim accurate throughout: the injection screen is a deterministic
pattern matcher that names the exact rule it matched. It is screening, not a
complete defence, and a page it does not match is "clean under these rules",
not "safe".

Every fenced command below was run, in this order, from the repository root,
while the document was written. Two steps are performed live by the
presenter, a real coding-agent session and the policy switch, and §3
explains both, each with a scripted fallback that already exercises the same
code. Nothing here needs credentials, contacts any external server, or costs
money; every network connection is to another process on this machine.

## 0. Generate the demonstration data

You cannot screen-share your real console: your record in `~/.commonmeasure`
contains real client names, paths and URLs. So the demonstration uses a
separate, generated data store, `demo/arc/home/`, built around a fictional
story (a made-up vendor, two made-up client engagements). The story is
fictional and the records are real: each record is produced by running the
shipped `commonmeasure` binary, not by writing output-shaped files.
`demo/arc/README.md` states what is real and what is scripted.

```sh
just demo-arc
```

The script performs six steps and checks each one produced its expected
record, and fails if not, so a broken demonstration is caught at preparation
time. In order: it simulates an agent's ordinary web reads so the console has
content; fetches a page containing a planted prompt-injection attack in
`observe` mode, which lets it through but records the finding; switches the
policy to `strict` by rewriting `policy.json`, the same edit an operator
makes; fetches the same malicious page again, which this time is refused
before its content enters the context, with the matched rules named; sends the one cleared
engagement's records to a local stand-in receiver, proving the other
engagement's records stay put; and switches the policy back so the presenter
can perform the switch live.

## 1. The block, read back

```sh
COMMONMEASURE_HOME=demo/arc/home cargo run -q -p commonmeasure-cli -- session arc-mediated-strict
```

Two fetches: the clean page went through and into the model's context; the
malicious page was refused, and the record names the three matched pattern
rules (`instruction_override`, `role_reassignment`, `tool_directive`). The
point to make aloud: the record names the rules but never quotes the attack
text itself, so the evidence cannot become a copy of the payload.

## 2. The console and the run report

```sh
COMMONMEASURE_HOME=demo/arc/home cargo run -q -p commonmeasure-cli -- serve --listen 127.0.0.1:4180
```

Open http://127.0.0.1:4180/. The Overview counts the generated crossings;
the Record lists the four generated sessions, with the content the agent
read per URL and per engagement; the Policy section shows the two
engagements, of which only `fictive-systems` is cleared for its records to
leave the machine. The committed batch run in which the same malicious page
was blocked is read in the terminal, not the console:

```sh
cargo run -q -p commonmeasure-cli -- inspect demo/output/injection
```

Read it per [`docs/READ-A-RUN.md`](READ-A-RUN.md): the block resolves to the named rules,
every integrity hash is recomputed at inspect time, and the original page
bytes are preserved on disk while no report quotes them.

## 3. The two live steps

Everything these two steps demonstrate has already been exercised by the
generated data, so if a live step fails on stage, fall back to showing the
generated record instead of improvising.

**A real coding-agent session.** First check that the registration runs the
current build: `commonmeasure doctor claude` names the binary the hooks run
and the version it reports, and the registration names the binary by path,
so a rebuilt binary at that path is what the next session runs. (The
`SessionStart` hook asks every session to prefer the Common Measure fetch
and search tools, the standing nudge of `plugin/README.md` §The standing
nudge, so that request does not need pasting.) Then start a
real Claude Code session and paste `demo/arc/session-prompt.txt` as the first
message: it states the task, and repeats the request for these pages only
because they are served from loopback, which the standing wording ("external
web content") does not clearly name. With the local test server from
`just demo-arc` still running, the session produces live what step 2 of the
script produced synthetically. *Fallback:* the `arc-mediated-observe`
session in the console, plus its raw transcript in `demo/arc/home/scripted/`.

**The policy switch.** Edit `demo/arc/home/policy.json` and set
`"policy_mode": "strict"`, the same edit the script made, then repeat the
malicious fetch in a fresh session: refused before its content enters the
context, rules named.
Say the limitation the Policy section states: a session that is already
running keeps the policy it started with, and picks up the new mode when it
restarts. *Fallback:* the `arc-mediated-strict` session, §1 above.

## 4. What leaves the machine

The final claim is about reporting: records leave the machine only for
engagements with explicit clearance, and only through the relay. The
generated data includes a relay delivery to a local stand-in receiver; its
report and the receiver's own record of what arrived are both in the store:

```sh
cat demo/arc/home/scripted/relay-report.txt
```

One engagement's session was delivered, six events, each attributed to the
cleared engagement; one session was withheld because nothing in it was
cleared, and the local test-server traffic was never eligible. The
receiver's file shows the delivered bytes themselves:

```sh
grep -o 'fictive\.example' demo/arc/home/scripted/receiver-received.ndjson | wc -l
grep -o 'other-client' demo/arc/home/scripted/receiver-received.ndjson | wc -l
```

Six and zero: every delivered event belongs to the cleared engagement, and
the other engagement appears nowhere in what arrived.

## 5. The hand-out

Attendees can take the plugin away and install it with no repository access
and no Rust toolchain. Every release publishes the archive, and the installer
fetches it with its checksum verified ([`docs/RELEASE.md`](RELEASE.md)). To build it from
this checkout instead:

```sh
sh plugin/build.sh
sh plugin/package.sh
```

Verify it the way a recipient would install it, against a throwaway Claude
Code configuration so this machine's real installation is not touched:

```sh
export CLAUDE_CONFIG_DIR=$(mktemp -d)
tar xzf dist/commonmeasure-plugin-*.tar.gz -C "$CLAUDE_CONFIG_DIR"
cd "$CLAUDE_CONFIG_DIR/commonmeasure-plugin"
claude plugin marketplace add ./
claude plugin install commonmeasure@commonmeasure
claude plugin uninstall commonmeasure@commonmeasure
claude plugin marketplace remove commonmeasure
cd - && unset CLAUDE_CONFIG_DIR
```

Without any credentials a recipient can reproduce everything in this
walkthrough: the install, recording of their own sessions, mediated fetch and
search with policy checks, the console, the generated demonstration data, and
the committed run's report. What they cannot reproduce: live purchasing from
content providers (needs their own provider accounts), model inference (needs
a configured model route), and the licensed-content evidence, which
deliberately never leaves this machine. The archive's own `INSTALL.md` says
the same, along with the privacy defaults: no telemetry destination is
configured, the plugin makes no network calls of its own, and the record
stays on the recipient's machine and deliberately survives an uninstall.

## 6. Before you share a screen

Review everything the console will display from the store: hosts, working
directories, session ids:

```sh
grep -rhoE '"(url|cwd|session_id)": *"[^"]*"' demo/arc/home/sessions/ | sort -u
```

The expected list is short: four `arc-*` session ids, the local test server,
the fictional `.example` hosts, and two workspace directories. The
directories are the one place your real environment shows: they are absolute
paths under this checkout, so they include your username. Decide before the
screen is up whether that is acceptable, and regenerate with `just demo-arc`
after any experiment that put other values into the store.
