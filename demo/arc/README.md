# The demonstration's generated data store

The security demonstration (`docs/RUN-THE-DEMONSTRATION.md`) runs against a generated data store,
`demo/arc/home/`, not against the operator's real `~/.commonmeasure`,
because the real store contains client names, paths and URLs that must
not be screen-shared. `just demo-arc` (which runs `regenerate.sh` here) deletes
and rebuilds the store from scratch. Terms like mediated, observed,
policy mode and relay are defined in `docs/GLOSSARY.md`.

## What is real and what is scripted

The *story* is fictional: the vendor (Fictive Systems), the websites, the
two client projects (`fictive-systems`, `other-client`) and every page
served are invented, and all hosts use reserved `.example` names so the
records are obviously synthetic. The *records* are real: every one is
written by the shipped `commonmeasure` binary through the same code that
runs in production. Nothing is inserted into the store by hand, and no
stand-in replaces a shipped component:

- **The mediated sessions** (`arc-mediated-observe`,
  `arc-mediated-strict`) are the real MCP server (`commonmeasure mcp`)
  driven over stdio, the interface a coding agent drives, making
  real HTTP fetches from a local test server that serves
  `demo/injection/corpus/`. The policy check, the PII detector and the
  injection screen run as they do in a live session. Those components
  produce the observe-mode pass-through and the strict-mode block.
- **The policy switch between those two sessions** is a rewrite of
  `policy.json`, the same edit an operator makes; the console reads the
  declared policy and does not write it.
- **The relay step** is the shipped `commonmeasure relay` delivering to
  `receiver.py`, a small local server that answers the documented
  acceptance response and writes every batch it accepts to disk. It
  evidences which records left and under whose permission. It does not
  evidence an integration with any real receiver product.
- **The observed sessions** (`arc-fictive-observed`,
  `arc-other-client-observed`) are the shipped hook entry point
  (`commonmeasure hook post-tool-use`) fed the documented payload shape a
  coding agent would send (`plugin/README.md` §Observed). This step is
  scripted: no coding agent ran, and the store does not claim one did.
  In the live demonstration the presenter's real session supplies this
  part (`docs/RUN-THE-DEMONSTRATION.md` §3).

No credentials are read, no external server is contacted, nothing costs
money, and every connection is to another process on this machine.

## The story the store tells

One client project (`fictive-systems`) with recorded and policy-checked
work, beside a second project (`other-client`) whose records have no
permission to leave the machine:

1. ordinary recorded web reads, the baseline: witnessed after the fact,
   not stoppable;
2. a policy-checked session in **observe** mode: the clean page passes,
   the malicious page passes too but the injection screen's finding is
   recorded, naming the matched rules;
3. the policy switched to **strict** in `policy.json`;
4. the same fetch again under **strict**: fetched, then refused before its
   content enters the context, the rules named, the attack text not quoted
   anywhere in the record;
5. the relay delivering the permitted project's records to the local
   stand-in receiver; the test-server traffic and the unpermitted
   project's records do not leave, which `regenerate.sh` verifies;
6. the policy switched back to observe, so the live demonstration can
   perform the switch itself. The strict-mode block remains on the
   record, because policy acts at the moment content moves and a later
   change to the policy does not rewrite earlier records.

`demo/arc/home/scripted/` keeps the raw transcripts beside the store (the
tool responses, the relay report, the receiver's record and the two local
servers' startup logs)
so every step can be read back after the run. If a local service
cannot start, the generator prints the end of its log before failing.

## Files

- `regenerate.sh` — the generation script; it prints an error and exits
  if any step's evidence is missing. Ports: test server 8377, receiver 8378
  (both local-only).
- `receiver.py` — the local stand-in telemetry receiver.
- `session-prompt.txt` — the task message pasted into one live
  coding-agent session (`docs/RUN-THE-DEMONSTRATION.md` §3). The request
  to prefer the mediated tools is not pasted: the installed plugin's
  `SessionStart` hook delivers it to every session as a standing nudge
  (`plugin/README.md` §The standing nudge). The prompt asks for the
  mediated tools by name, because this task's pages are served from
  loopback and the standing wording asks about external web content.
- `home/` — the generated store (gitignored; wiped on every run).
