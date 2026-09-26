---
title: "Getting started: from the installer to your own evidence"
domain: edge
audience: operator
section: get-started
---

# Getting started: from the installer to your own evidence

From the installer to an operator console showing the first crossing your
own agent made. Only §8 needs a checkout or a toolchain. The release
downloads and version checks in §1 were run against release 0.4.1 on macOS
(Apple silicon), from empty home directories. The remaining walkthrough
commands and quoted output were checked on 0.4.0; the update and
login-service instructions require 0.4.2. Terms such as crossing, run,
admission and engagement are defined in [`docs/GLOSSARY.md`](GLOSSARY.md).

- **Nothing needs configuring first.** With no credentials and no policy
  file, the hooks record everything a session retrieves. Credentials and
  policy add capability; where one is absent, the output says so.
- **The model is never Common Measure's.** A host session uses the host's
  model. A batch run from a checkout needs
  `COMMONMEASURE_INFERENCE_ENDPOINT` before any plan can produce an answer
  (§8).
- **No Hub account is needed** for local policy, supplier comparison or the
  record. Supplier calls need that supplier's credentials and may cost
  money (§3).

## 1. Install the binary and register it with Claude Code

The installer ends with `commonmeasure` on your `PATH`; the route from a
checkout (§8) ends in the same place, and both then register with Claude
Code the same way.

### From a release, with no toolchain

Each release on GitHub holds:

- `commonmeasure-linux-x64`, `commonmeasure-linux-arm64` (static),
  `commonmeasure-darwin-arm64`, `commonmeasure-darwin-x64` and
  `commonmeasure-win-x64.exe`: the binaries;
- `commonmeasure-plugin-<version>.tar.gz`: the Claude Code plugin archive,
  which bundles those five binaries and is its own marketplace;
- `install.sh`: the installer;
- `SHA256SUMS`: the SHA-256 of every other asset.

The installer needs `curl` and `sha256sum` or `shasum`, and no account or
token. It downloads the binary for your platform, verifies it against
`SHA256SUMS` and places it in `~/.local/bin`, printing the line to add when
that directory is not on `PATH`. The 0.4.1 installer checks the version after
placement; a mismatch removes the installed binary. The 0.4.2 installer
checks both checksum and version before placement, leaving an installed
binary intact if either check fails. In 0.4.2, `commonmeasure update` runs
the copy of this script compiled into the binary.

```sh
curl -fsSL https://github.com/commonmeasure/commonmeasure/releases/latest/download/install.sh | sh
```

From an empty home directory it prints, with the release's version in place
of `<version>`:

```text
installed ~/.local/bin/commonmeasure: commonmeasure <version>, checksum verified
~/.local/bin is not on PATH. This installer does not edit shell profiles; add this line to yours:
  export PATH="~/.local/bin:$PATH"
Next: commonmeasure install claude (or codex, pi, claude-desktop, cursor, copilot, vscode, chrome) to register with your host, then work a session, then run 'commonmeasure session' to see what it recorded and 'commonmeasure serve' for the console on loopback. Nothing leaves this machine.
```

The home directory is written as `~` above; the installer prints it in full.

To pin a release, download that release's installer into a directory of its
own (a checkout has an `install.sh` of its own at the root) and name the
tag. `--dir` installs somewhere other than `~/.local/bin`, and `--plugin`
also downloads, verifies and unpacks the plugin archive, then prints the two
commands that install it into Claude Code:

```sh
curl -fsSL -o /tmp/commonmeasure-release/install.sh --create-dirs \
  https://github.com/commonmeasure/commonmeasure/releases/download/v0.4.1/install.sh
sh /tmp/commonmeasure-release/install.sh --tag v0.4.1 --plugin ~/commonmeasure-plugin
```

To check a download by hand, fetch `SHA256SUMS` beside it:

```sh
curl -fsSLO https://github.com/commonmeasure/commonmeasure/releases/download/v0.4.1/SHA256SUMS
curl -fsSLO https://github.com/commonmeasure/commonmeasure/releases/download/v0.4.1/commonmeasure-darwin-arm64
shasum -a 256 -c --ignore-missing SHA256SUMS
```

```text
commonmeasure-darwin-arm64: OK
```

The installer stops and says why when `curl` or a checksum tool is missing,
the release does not exist, or the release has no binary for your platform;
in the last case it lists the binaries the release holds.
`COMMONMEASURE_RELEASE_URL` points it at a mirror with the same asset names
and address shape; a release build of `commonmeasure update` ignores it
(Updating, below).

The one-line form and the pinned form with `--plugin` have been run on macOS
(Apple silicon). Each release is also installed by its own workflow in a
clean Linux container with no toolchain and no credential (`RELEASING.md`).
The installer's refusals are each driven against a loopback origin standing
where the release stands (`crates/commonmeasure-cli/tests/installer.rs`).
The Windows binary is built and checksummed by the release run; the
installer has not been run on Windows.

### Updating

Requires 0.4.2. For 0.4.1, rerun the release installer above.

```sh
commonmeasure update --check        # the installed and latest versions; changes nothing
commonmeasure update                # install the latest release over this binary
commonmeasure update --tag vX.Y.Z   # a named release, older or newer
```

`update` runs the installer compiled into the binary, so the checks are the
installer's: the checksum from the release's `SHA256SUMS` and the new
binary's version, both before it replaces this one. A failure before the new
binary is moved into place leaves this binary as it was. The installer can
also fail after that, for instance when it cannot print its last message, so
when it exits non-zero `update` compares the binary with the device, inode,
size and SHA-256 it recorded before the installer ran, and says one of:

- the binary is unchanged;
- the binary was replaced, with the version the new binary reports. It is
  asked only after `update` has tried to start the console service again,
  whether or not that succeeded, and given 15 seconds and 4 KiB of output.
  Past either, it and the processes it started in its process group are
  stopped; a process that leaves that group is not stopped. Past either, or
  when it exits non-zero, the version is not known and `update` prints the
  command that shows it. A non-zero exit stops nothing it left running;
- whether the binary was replaced is not known, because it could not be
  read, or changed or was replaced while it was read, with the command that
  shows its version.

The version check’s five-second bound covers the wait for output and exit,
not the system calls that spawn and reap the child or the whole `update`
command.

The comparison reads the binary twice, before and after the installer; it
is not a snapshot. A process writing the binary at the same time can change
it between or during those reads without being told apart from the
installer, and nothing locks the binary against other writers.

A failure restarting the console service is described below. It replaces the
binary it was run as. On macOS it refuses when it was run through a symbolic
link, which belongs to whatever installed it. On Linux it does not detect
the link: the system reports the file the link points to, and `update`
replaces that file. It contacts the release location only when run; nothing
checks for updates in the background. macOS and Linux only.

The first line `update` prints is the release origin. A release build always
uses `https://github.com/commonmeasure/commonmeasure/releases`, ignores
`COMMONMEASURE_RELEASE_URL` and passes that origin to the installer, so an
inherited variable cannot redirect an update. A debug build honours the
variable; the tests use it to serve a release from loopback. `install.sh`
run by hand honours it in either case.

The checksum shows that the binary matches the `SHA256SUMS` published beside
it on the same origin. It does not show who published them: releases are not
signed, and `update` trusts the release origin exactly as the installer run
by hand does. Whoever can replace both files on the origin, or answer for it
with a certificate this machine trusts, can make `update` run their binary,
which it does when the installer checks the binary's version.

Release tags have the form `vX.Y.Z`: three numbers, no suffix. When the tag
the origin's `latest` points to has any other form, `update` and
`update --check` exit non-zero and change nothing. `--tag` takes the same
form and installs the named release even when its version is lower than
this binary's.

Before stopping anything or downloading release assets, `update` refuses
while other processes of yours run the same binary file, and lists each
one's pid, its executable, its subcommand (such as `mcp`) and its
`COMMONMEASURE_HOME`, or `COMMONMEASURE_HOME unknown` when its environment
cannot be read. macOS withholds the environment of a restricted process, so
an empty one is reported as unknown rather than unset, as is an environment
of zero bytes on Linux, and one with an empty entry followed by more entries
that do not include `COMMONMEASURE_HOME`. On macOS, where the kernel's own
strings follow the environment, it is also unknown when `COMMONMEASURE_HOME`
appears only past where the environment was taken to end (`ARCHITECTURE.md`
§What runs where). It prints no other argument or environment variable,
since either can hold a credential. The console service's own process is
left out only when launchd names the same pid before and after the check,
the process is launchd's child, and it started before `update` first asked
launchd. A process whose executable the system does not identify is listed
by its process name when that name is `commonmeasure`: on macOS, a process
that still runs a binary whose file has since been removed. Close them and
run `update` again: quit the host app that started an MCP server, and stop
`commonmeasure hosted service` and `commonmeasure relay`. It does not stop
them itself and has no option to skip the check.

Two other cases also refuse, with their own remedy:

- On macOS, a process the system refuses to describe, whose owner and name
  cannot be read either, is counted, since nothing shows it does not run the
  binary. It is listed as not inspected, with the `ps` command that checks
  it; run `update` again once it has exited.
- When the console service restarts or stops while `update` checks it,
  `update` cannot tell its process from another one running the binary. It
  says the console restarted during the check; run `update` again.

Looking up the latest release tag can precede this check. An empty or
whitespace-only `COMMONMEASURE_HOME` is reported as `not set or empty`,
as is an environment that does not set it; these use the default home.

The check does not find:

- processes running another copy of `commonmeasure` on the same Edge home;
- a copy under another name whose executable the system does not identify,
  such as one whose file has since been removed;
- a process started after the check and before the new binary is moved into
  place;
- other users' processes.

Those remain the operator's to stop (one release per Edge home,
`ARCHITECTURE.md` §What runs where).

A console service (§5) is handled as follows:

- A loaded service running this binary is stopped before the installer runs,
  and started again afterwards with the configuration in its plist: its
  address, Edge home, log and working directory, whatever the shell running
  `update` selects.
- It is started again whenever it was stopped: on the new binary after a
  successful install, and on whatever binary is then in place when the
  installer fails. The message names which, as above. A replacement is
  asked for its version only after this restart has been attempted, also
  when it fails.
- When it cannot be started again, `update` exits non-zero, says whether the
  new binary was already moved into place and which version it is, and
  prints the command that installs the service again with its own Edge home.
- `update` refuses before stopping anything or downloading release assets,
  and names the commands that remove or reinstall the service, when launchd
  cannot be asked about the service, or the service is loaded and:
  - its plist is missing or not in the form `service install` writes;
  - its plist has been edited since `service install` wrote it, including
    settings `update` does not read, such as `KeepAlive`, which starting it
    again would overwrite;
  - its plist differs from the job launchd loaded;
  - launchd's report of the job's environment cannot be read, so its Edge
    home is not known;
  - launchd passes it `COMMONMEASURE_HOME` from the domain's environment
    (`launchctl setenv`), which `service install` never does. The recovery
    removes the domain variable first (`launchctl unsetenv
    COMMONMEASURE_HOME`), then installs the service again with the Edge home
    it runs with: its plist's own value when the plist sets one, since
    launchd applies the job's environment over the inherited one, or else
    the inherited value. When neither gives a home, such as an inherited
    value that is empty and no value of its own, `update` names the values
    it saw and, after removing the domain variable, asks you to choose the
    home.
- When the service cannot be stopped, nothing is installed.
- A service running another binary is left running.

### Register with the host

On either path, the binary writes its own registration:

```sh
commonmeasure install claude
```

This writes the five hooks (`SessionStart`, `PostToolUse`,
`UserPromptSubmit`, `Stop`, `SessionEnd`) into `~/.claude/settings.json` and the MCP
server at user scope into `~/.claude.json`, each naming the binary by its
resolved absolute path, because a hook runs outside your login shell's
`PATH`. Only this product's entries are written; everything else in those
files is kept. Replacing the binary at that path changes what the next
session runs, with no reinstall. `commonmeasure uninstall claude` removes
exactly those entries and leaves `~/.commonmeasure/` alone.

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
and Cursor), and `commonmeasure install copilot` and `commonmeasure install
vscode` with the Copilot CLI and VS Code (`plugin/README.md` §GitHub
Copilot and VS Code). `commonmeasure install chrome` registers the binary
for the browser extension in `browser/`, which records the sources ChatGPT
on the web and Bing Copilot Search show (`browser/README.md`).

The installer registers every host listed above.

### Check what arrived

```sh
commonmeasure doctor claude
```

`doctor` prints, for the host: which of the five hooks are registered and in
which file, whether the MCP server is registered and its command, the binary
each names and the version that binary reports when run, whether a plugin is
installed beside the registration, whether the sessions directory can be
written, and whether the policy file loads. A registration whose binary has
gone is reported as not found, which is the one state in which every hook
exits without recording and nothing in the session says so.

## 2. Record a session of your own work

Open a new Claude Code session and work. No other step is needed: the
observed half needs no credentials and no configuration. A `PostToolUse` hook
fires after every `WebFetch`, `WebSearch` and third-party MCP result, and
records the crossing; for `WebFetch`, with the SHA-256 of exactly the text
that entered the model's context, which makes the row checkable against the
transcript later. Search results are recorded as retrieved but not grounded:
the model saw titles and snippets, not pages, and the record does not claim
more.

Read it back at any time:

```sh
commonmeasure session            # the most recent session
commonmeasure session <id>       # a named one
commonmeasure status             # the policy in force, by digest, and this edge's fleet status
```

A session that fetched one page prints, among other lines:

```text
records    3
crossings  1 observed, 0 mediated, 0 refused, 0 reconstructed
grounded   1 put page text into the model's context
```

Each completed turn also records a context snapshot: the host transcript's
own usage counters for the turn's last model call, printed with that basis
named, and an inventory of the window by category estimated from the host's
own transcript records, printed with its basis named. What no basis can see
(a context limit, the free capacity, the resident tool schemas) is said to
be unavailable rather than estimated; the witnessed acquisition is shown
beside the counters as a lower bound; and between two boundaries the report
shows the API-reported change beside the estimated growth of scaffolding,
conversation and acquired content and the remainder it cannot attribute
([`docs/contracts/session-evidence.md`](contracts/session-evidence.md) §Context snapshots).

Observed capture can only watch: the hook fires after the crossing, so
nothing can be refused. The refusable grade of evidence exists only when the
agent asks Common Measure to carry the crossing, and agents use their built-in
tools unless told otherwise. The plugin's `SessionStart` hook therefore adds
one paragraph to every session's context asking the agent to prefer
`context_fetch` and `context_search` over `WebFetch` and `WebSearch`, and to
respect a refusal rather than retrying it with a built-in tool
(`plugin/README.md` §The standing nudge). Each delivery is recorded in the
session log as `nudge_issued` under the wording's versioned identity. It is
a request, not enforcement: nothing blocks the built-in tools, so expect
mediated crossings alongside observed ones.

A mediated fetch checks the operator's source policy first, records the
crossing either way, and hands the agent the bytes plus the hash it just
recorded. Of the in-process processors, the PII detector and
the injection screen run at this crossing, judging the text before the model
sees it, and each invocation is recorded as a `processor_invoked` event
beside the crossing it judged ([`docs/contracts/processor.md`](contracts/processor.md)); the context
optimiser runs at the batch runner's transform stage, the support governor
only in batch runs whose job declares `governance`, the fidelity verifier on
every batch answer, the fidelity judge only in batch runs whose job declares
`fidelity_judge`, and the output provenance labeller only in batch runs
whose job declares `output_provenance`. One limitation:
the host does not tell the MCP server its session id, so mediated crossings
are recorded under a session of the server's own (`local-<timestamp>-<pid>`),
one per server process, alongside the host session's observed record. Both
appear in the console. The plugin's own tools are excluded from observed
capture, so a mediated fetch is never double-recorded.

## 3. Credentials, when you want provider search

`context_search` and the batch runner's `--live` need provider credentials,
which live in `~/.commonmeasure/credentials.env` (`$COMMONMEASURE_HOME/credentials.env`)
beside `policy.json`:

```sh
cat > ~/.commonmeasure/credentials.env <<'EOF'
EXA_API_KEY=…
TAVILY_API_KEY=…
EOF
chmod 600 ~/.commonmeasure/credentials.env
commonmeasure credentials     # which providers are configured, and from where
```

The MCP server loads the file at start, so the mediated tools work in a
session launched from anywhere. The launching environment wins; nothing is
read from the working directory; the file is parsed, never sourced; and a
loaded file is recorded as `credentials_loaded` with path, digest and
variable names. `commonmeasure credentials` lists every provider the
adapters know and the variable each reads.

An unconfigured provider reports

```text
unavailable: exa needs EXA_API_KEY, which is not configured. Set it in
/home/op/.commonmeasure/credentials.env (KEY=VALUE lines, chmod 600), or export
it in the environment that launches the harness — the environment wins where
both name it. No search was attempted.
```

rather than a search that returned nothing without saying why. A configured
search records each result against policy and reports the provider's
observed charge in its own unit (an Exa search reports, for example,
USD 0.007).

One provider needs no credential: `context_search` with
`"provider": "internal"` queries the operator's own corpus, the directory
`COMMONMEASURE_INTERNAL_CORPUS` names, set in the launching environment or in
the same credentials file. Results carry the licence and effective dates
declared in the corpus's `corpus.json`, and the crossings are recorded only
when the corpus's `file://` prefix is named in `record_internal_prefixes`
(§4); without it the agent still gets the result and the record is withheld.

The replay mode needs none of this (§8). A skill needs no credential either,
and is not reachable from a session tool: it is invoked only by a run, only
with `--live`, and only when `COMMONMEASURE_SKILL_CATALOGUE` names a catalogue
declaring it (`demo/skills/README.md` is a worked example;
[`docs/contracts/provider.md`](contracts/provider.md) §Skill supply is the contract).

## 4. Policy: refusing a crossing before it happens

Absent policy means observe: record everything, refuse nothing. To let the
operator refuse, put a source policy at `~/.commonmeasure/policy.json` (or
`$COMMONMEASURE_HOME/policy.json`). This one reproduces the four fetches of
the example, [Four fetches, two refused](guide/four-fetches.md); a checkout
holds the same file as `demo/policy/four-fetches.json`.

```sh
cat > ~/.commonmeasure/policy.json <<'EOF'
{
  "policy_mode": "strict",
  "constraints": [
    {"kind": "access_rule", "host": "www.gov.uk", "action": "allow"},
    {"kind": "access_rule", "host": "*.people.com", "action": "allow"},
    {"kind": "access_rule", "host": "*.theguardian.com", "action": "allow"},
    {"kind": "access_rule", "host": "*", "action": "refuse"}
  ]
}
EOF
commonmeasure policy check ~/.commonmeasure/policy.json
```

```text
accepted      /home/op/.commonmeasure/policy.json
mode          strict
constraints   4
scopes        0
principals    0
terms         0
digest        sha256:3a68c94502daf23f81776924da50f4b3d60c92f1ff81ae50a47f2905891d0052
```

Four ordered access rules in strict mode: three named hosts are allowed and
the last rule refuses every other host. Under it, the four fetches of the
example came out as follows. They were sent as `context_fetch` calls
straight to `commonmeasure mcp --host claude-code --session
door-walkthrough`, the way [`docs/integrate/host.md`](integrate/host.md) §2
shows, so the session's `client` line below names that test client rather
than Claude Code. Page text and the `robots` object
are elided, and `/home/op` stands for your home directory. The edge was not
enrolled with a hub, so every request went out unsigned (§7).

`https://www.gov.uk/government/organisations`, admitted:

```json
{
 "url": "https://www.gov.uk/government/organisations",
 "content_hash": "sha256:d2ea91637775a3cf18125d57a89a0d0728933cf5e393853fb9b1e591eec873ad",
 "retrieved_hash": "sha256:1c1c35426e233c1a8a0e6f28968714804336c6777cd906b3a67c26be57aa3d0b",
 "estimated_tokens": 10150,
 "token_basis": "characters/4",
 "http_status": 200,
 "licence": {"state": "unknown"},
 "declarations": {"effective": {"train-ai": "unknown", "ai-input": "unknown", "ai-index": "unknown", "search": "unknown"},
                  "statements": [], "governing": "statements", "assessment_decision": null, "terms": null,
                  "robots_group": "*", "robots": {…, "outcome": "allowed"}, "redirects": [], "licences": []},
 "policy": "Admitted; no constraint excluded it.",
 "breach": null,
 "named_by": "unknown",
 "content_telemetry_id": null,
 "allowance": null,
 "recorded_in": "/home/op/.commonmeasure/sessions/door-walkthrough.ndjson",
 "content": "Cookies on GOV.UK\nWe use some essential cookies to make this website work.\n…"
}
```

`https://www.people.com/` was admitted the same way; its result names the
address the site redirected to, `https://people.com/`.

The Guardian article the example names, refused before any request for
it, as a tool error:

```text
refused before the crossing: The licence https://theguardian.com/license.xml permits AI input under payment type subscription, and this edge holds no settlement rail, so the payment term is unmet. (operator policy in /home/op/.commonmeasure/policy.json; the source's declarations are recorded in /home/op/.commonmeasure/sessions/door-walkthrough.ndjson)
```

`https://www.economist.com/`, refused by the operator's own rule, as a tool
error:

```text
refused before the crossing: access rule 4 (*) refuses host www.economist.com. (operator policy in /home/op/.commonmeasure/policy.json)
```

The publishers' pages change, so a later run gives different hashes and
token estimates from those above and in the example; the two refusals
depend only on the policy and the Guardian's licence.

Read the session back:

```sh
commonmeasure session door-walkthrough
```

```text
session    /home/op/.commonmeasure/sessions/door-walkthrough.ndjson
joined     nothing: no other log names host process claude (pid 32907, started 2026-09-23T20:59:28Z)
records    14
crossings  0 observed, 2 mediated, 2 refused, 0 reconstructed
grounded   2 put page text into the model's context
host-observed (grade: observed): 0 context entries across 0 acquisitions; unknown output associations; 0 output observations
client     walk 1.0 via host claude-code

policy identity (mediated crossings and boundaries)
  sha256:de10447c95ffc94073aed12fb258409c594f8b9f3ed9780c1fadc0e52f27fb1a  4 record(s)

crossings
  mediated   grounded  https://www.gov.uk/government/organisations
      identity: unsigned — this edge is not enrolled with a hub, so it holds no key a publisher can verify: no enrolment record at /home/op/.commonmeasure/enrolment.json
  mediated   grounded  https://people.com/
      identity: unsigned — this edge is not enrolled with a hub, so it holds no key a publisher can verify: no enrolment record at /home/op/.commonmeasure/enrolment.json
  mediated   refused   https://www.theguardian.com/politics/2026/sep/09/mr-congeniality-burnham-advises-badenoch-to-ditch-the-point-scoring-over-defence-spending
      refused: The licence https://theguardian.com/license.xml permits AI input under payment type subscription, and this edge holds no settlement rail, so the payment term is unmet.
  mediated   refused   https://www.economist.com/
      refused: access rule 4 (*) refuses host www.economist.com.
```

A refusal appears in three places: the tool error the agent sees, quoted
above; the session log, as a `crossing_refused` record with the reason and
no content hash, because nothing was fetched; and the console, where the
Record's rail row for the session counts it under "refused" and the
session's detail pane shows a refused card carrying that reason. An admitted
fetch carries `"policy": "Admitted; no constraint excluded it."` on the
result. Only mediated crossings can be refused; observed capture still
records everything the agent's built-in tools did. Delete the file to
return to observe mode. A policy file that cannot be parsed is an error,
not a silent fallback to permissive. The two publishers' pages and
declarations are theirs to change, so a later run may differ from the
quotes above; `crates/commonmeasure-cli/tests/demo_policy.rs` holds the
same policy to a loopback publisher whose licence does not change.

### Scopes, engagements, principals and allowances

The same file carries the rest of the declared policy:

- **Scopes** bind a working directory to an engagement and to the rules
  that govern work there; the mode, constraints and clearance of the
  matching scope apply, and the top level applies where nothing matches
  ([`docs/contracts/source-policy.md`](contracts/source-policy.md) §Scopes and
  principals).
- **Engagements** are the names records, policy and reporting are grouped
  under. `allow_telemetry_egress: true` on a scope is the clearance for that
  engagement's witnessed crossings to leave the machine (§7); without it
  nothing leaves.
- **Principals** are authenticated identities: a `principals` list names
  who may exercise the policy, resolved from an authentication basis the
  process cannot rewrite, and a principal that cannot be authenticated is
  refused rather than degraded ([`docs/contracts/source-policy.md`](contracts/source-policy.md)
  §Scopes and principals). That basis is
  the process's operating-system user and is unix-only, so a policy that
  declares `principals` refuses every mediated crossing on Windows; a
  policy declaring none is unaffected there.
- **Allowances** bound a principal's cumulative spend over a declared
  period, enforced from a local transactional ledger before any priced
  acquisition is dispatched ([`docs/contracts/run-output.md`](contracts/run-output.md)
  §`acquisition.quote.allowance`).

For a firm the natural shape is one principal per solicitor and one scope
per matter. Each solicitor works as their own operating-system user on
their own machine, so the `principals` list binds a policy name to that
user's numeric id (`id -u` prints it); each matter is an engagement kept in
its own directory, and a scope matches that directory, names the matter as
the engagement and carries the clearance for its records to leave. The
policy for one solicitor and one matter:

```json
{
  "policy_mode": "strict",
  "constraints": [
    {"kind": "access_rule", "host": "www.legislation.gov.uk", "action": "allow"},
    {"kind": "access_rule", "host": "*", "action": "refuse"}
  ],
  "principals": [
    {"principal": "solicitor-a", "os_user": 501, "require_scope": true}
  ],
  "scopes": [
    {
      "match": "/home/solicitor-a/matters/2026-014",
      "principal": "solicitor-a",
      "engagement": "matter-2026-014",
      "allow_telemetry_egress": true
    }
  ]
}
```

The scope declares no constraints of its own, so the two access rules at
the top level govern work in the matter directory. `require_scope: true`
means this solicitor's agent can work only inside a matter directory the
policy names: outside one, every mediated crossing is refused rather than
falling back to the top-level rules. A second solicitor is a second
`principals` entry with their own user id, and a second matter a second
scope; a directory holding client-confidential material gets a scope whose
`constraints` refuse every host. What each field does, and what declaring
any principal changes for every other user of the machine, is
[`docs/contracts/source-policy.md`](contracts/source-policy.md).

[`docs/GLOSSARY.md`](GLOSSARY.md) defines each term in one line.

Where the file comes from a hub rather than your editor, because
`deployment.json` pins the hub's signing key, the edge refreshes it at every
session start and before every relay run, and `commonmeasure doctor` adds a
line naming the revision in force and its expiry. An envelope that has
expired keeps enforcing its policy and that line says `stale since` when;
[`docs/contracts/policy-envelope.md`](contracts/policy-envelope.md) §Cadence
and staleness has the rest.

## 5. The console

```sh
commonmeasure serve
```

serves the operator console on `http://127.0.0.1:4173`. `--listen` moves it
to another loopback address; it binds nothing else without
`--allow-remote`. A crossing recorded while the console is open appears on
reload. The door-walkthrough session is under **Record**, with its two
refusals as refused cards carrying the reasons quoted above. Each section
is described in [The console](CONSOLE.md).

### Keep the console running (macOS)

Requires 0.4.2.

```sh
commonmeasure service install console     # --listen 127.0.0.1:4173 by default
commonmeasure service status
commonmeasure service uninstall console
```

`install` writes a LaunchAgent to
`~/Library/LaunchAgents/ai.commonmeasure.console.plist` that runs this binary's
`serve` by its absolute path at login, with `RunAtLoad` and `KeepAlive`,
logging to `logs/console.log` in the Edge home: `~/.commonmeasure` unless the
installing shell sets `COMMONMEASURE_HOME`, in which case the plist sets it
for the service too. It loads the agent with
`launchctl bootstrap gui/<uid>`, starts it with `launchctl kickstart`, and
waits for the console to answer. It refuses a `--listen` that is not loopback,
and refuses when another process already listens on the address, naming its
pid and command; stop that process and install again. `uninstall` runs
`launchctl bootout` and removes the plist; the log stays.

Installing again rewrites the plist and restarts the console. That is how the
service picks up an upgrade: `KeepAlive` restarts a console that exits, not
one whose binary was replaced while it ran. Installing again takes the Edge
home from the shell that runs it. `commonmeasure update` stops the service
before it replaces the binary the service runs and starts it again afterwards
with the Edge home and log in its plist (§1, Updating); after an upgrade by
any other route, `status` names the command.

`status` reports whether the plist is installed, the Edge home and log it
names, whether launchd has it loaded and its pid, what listens on the port
and the version it reports (`GET /api/version`), and the `commonmeasure` on
`PATH` and its version, asked within 15 seconds and 4 KiB of output; past
either, or when it does not answer with a version, `status` says the version
is unknown and why. This time bound covers the wait for version output and
exit, not the system calls that spawn and reap the child or the rest of
`status` (launchd, `lsof`, `ps`). When launchd cannot be asked, it says so
rather than reporting the service as not loaded. It names the restart
command when the version the service runs differs from the one on `PATH`, or
when its binary changed after it started. It also names a console on the
port that the service did not start, such as one left running from a
terminal, with the `kill` that stops it. For a console that does not report
its version, status says so and does not guess one.

On macOS 26.4 on battery power, launchd was seen leaving the agent pending
("pended nondemand spawn" in `launchctl print`) instead of starting it at load
or after it exited. `install` starts it explicitly for that reason; if
`status` says the service is loaded but not running, run `install` again.

On Linux and other platforms `commonmeasure service` refuses and names the
equivalent systemd command:

```sh
systemd-run --user --unit=commonmeasure-console "$(command -v commonmeasure)" serve --listen 127.0.0.1:4173
```

## 6. Import history from before Common Measure

Work done before the plugin was installed left host transcripts behind.
Always look before importing:

```sh
commonmeasure import --dry-run
```

which reports, per host, what a real import would do (`<host>  N new
crossings from M transcripts`) and names the hosts it will not import from,
with reasons: Codex transcripts hold shell command text, and a URL inside a
command is not evidence that anything was retrieved.

```sh
commonmeasure import
```

imports what the transcripts evidence. Every imported row is recorded as
`reconstructed`, read back after the fact with nothing watching at the time,
and never counted with witnessed evidence. Import is idempotent: a second run
imports `0 crossings`.

## 7. Egress, only when you ask for it

Everything so far stays on your machine. The one command that can change that
is the relay, and on a fresh install it refuses:

```sh
commonmeasure relay
```

```text
commonmeasure: no telemetry receiver is configured: pass --receiver or set one
in ~/.commonmeasure/relay.json; nothing was projected and nothing was sent
```

There is no default destination and no default hub. A receiver is named in
`~/.commonmeasure/relay.json`, by `commonmeasure connect`, by hand, or for
one run with `--receiver`. The relay then projects witnessed crossings into
Content Telemetry v1.0 batches, spools them durably and delivers them.
Reconstructed crossings never leave, and a refused crossing leaves only as a
count on its session. A session's crossings leave only when the scope that
matched their working directory carries `allow_telemetry_egress: true`
(§4). What may leave, under whose clearance, and the wire format are
[`docs/contracts/telemetry-projection.md`](contracts/telemetry-projection.md).
For a supplier's own receiver, `"suppliers": ["<name>"]` in `relay.json`
sends it only that supplier's events
([§Supplier scope](contracts/telemetry-projection.md#supplier-scope)).
Requires 0.4.2: 0.4.1 refuses a `relay.json` with `suppliers` and sends
nothing.

The first run reads every session log in the home, including sessions
recorded before a receiver was configured, and decides each crossing under
the policy in force at that run. A later run under a wider policy sends
what the wider policy clears and never sends a delivered event twice.

To see what a run would send, without sending, refreshing policy or
changing files:

```sh
commonmeasure relay --dry-run --receiver http://127.0.0.1:9/events
```

```text
dry run: nothing was sent; no state was changed
would deliver 0 events in 0 batches to http://127.0.0.1:9/events (new at the receiver: unknown)
projected 0 of 1 sessions and 0 runs; 0 events would be newly spooled
  1 withheld: no crossing cleared to leave
hosts that would leave: none
forecast policy: /home/op/.commonmeasure/policy.json, as it stands on disk
a real run syncs managed policy and directory grants first, which can change what is cleared and what leaves
```

The door-walkthrough session is withheld because the §4 policy clears no
scope. `--policy <file>` forecasts a draft policy through the same loader.

Each run delivers only the batches that are due. `commonmeasure status` and
`commonmeasure doctor` show queued and dead batches, and
`commonmeasure relay requeue` starts another schedule for dead ones.

Once `relay.json` names a receiver, the relay also runs by itself when a
Claude Code session ends: the `SessionEnd` hook starts `commonmeasure relay`
in the background and returns without waiting. The other hosts send no
session-end event, so with them run `commonmeasure relay` yourself, and a
source whose licence demands usage reporting is refused there. On a managed
home, `commonmeasure hosted service` relays every session in the home on an
interval while it runs, which meets that demand. `commonmeasure doctor`
prints the last delivery and how automatic relaying is set up.

To review each run before it leaves, create the empty file
`~/.commonmeasure/relay/manual`. Nothing then relays at a session end or on
the hosted service's interval, and a source whose licence demands usage
reporting is refused while the file is there. Delete it to relay
automatically again.

### Joining Common Measure Hub

An organisation's machines can enrol with Common Measure Hub, so that one
owner publishes a policy every machine applies and sees the cleared
evidence each one delivers. Setting that up is in the Hub guides:
[Start here](https://commonmeasure.ai/docs/hub/start-here/) takes a new
organisation from sign-in to its first delivery, and
[Connect a Common Measure edge](https://commonmeasure.ai/docs/hub/connect-commonmeasure/)
covers enrolling a machine, taking the organisation's policy and enrolling
a project.

On the edge, `commonmeasure connect <hub-url> --token <token>` writes the
receiver and ingest key into `relay.json`, mints the edge's signing key
(`edge-key.json`, which never leaves the machine) and records the hub's key
id in `enrolment.json`, then makes a first relay run. With `--managed` it
also pins the hub's policy signer in `deployment.json` and makes a first
policy sync.
`commonmeasure disconnect` revokes both keys at the hub when it can reach it
and removes the three files. The exchange is
[`docs/contracts/enrolment.md`](contracts/enrolment.md).

## 8. If you build from source

No other section needs a checkout. From one, the test suite, a build of the
binary, the batch runner and the replay mode are available:

```sh
cargo fetch                      # once, online: the dependency crates
cargo test --workspace --offline
```

The toolchain is rustc 1.97 or later (`Cargo.toml` `rust-version`). After the
one fetch the workspace builds and the tests pass offline: tests that need
a network boundary start a real loopback origin and drive the production
adapters over it, so no external service is involved. Tests that need
recorded fixtures absent from a public checkout are reported as ignored,
with the missing directory named. The first build takes
a few minutes. The same checkout builds the binary the installer would
have placed, which then registers with a host exactly as in §1:

```sh
cargo install --locked --path crates/commonmeasure-cli
```

Then publish a run with nothing configured:

```sh
cargo run -p commonmeasure-cli -- run demo/jobs/energy-price-cap.json --output /tmp/commonmeasure-empty
cargo run -p commonmeasure-cli -- inspect /tmp/commonmeasure-empty
```

`inspect` prints the run dossier: the mode (`no-external-acquisition`), the
sealed manifest hash, and one block per supply plan, every claim ending with
a citation into the artefact it was read from
([`docs/contracts/run-output.md`](contracts/run-output.md) defines the
directory). Every plan here is `unavailable`, each with a gap
naming what is missing:

- the provider plans: acquisition was not authorised, because external calls
  cost money and happen only under `--live` with a credential;
- every plan: no inference gateway is configured; the gap names
  `COMMONMEASURE_INFERENCE_ENDPOINT`.

The replay mode (`run --replay <dir>`,
[`docs/contracts/run-output.md`](contracts/run-output.md)) runs the whole
acquisition path over recorded provider responses: the recorded bytes are
served from loopback origins through the real adapters, parsers and policy,
no external call is made and no credential is read, and a provider without a
hash-verified recording fails the run. It needs a directory of recordings
and its manifest. The recordings the project's own replay tests use are
third-party provider responses and are not in the public repository, so
those tests are ignored in a public checkout.

With `COMMONMEASURE_INFERENCE_ENDPOINT` set (`demo/gateway/tensorzero/` records
the reference sidecar), a run completes real inference over the acquired
context.

## 9. Enrol a project directory

A project directory can be set to record locally only, or to report
through the relay: to the hub on an enrolled edge, otherwise to the receiver
`relay.json` names. With the Claude Code plugin, invoke
`/commonmeasure:enrol`; with Codex, `commonmeasure install codex` adds the
`$commonmeasure-enrol` skill. The agent shows the directory it is working in
and the edge's enrolment, then asks for a project name and a reporting
choice. The same from the command line, in the current directory or a named
one:

```sh
commonmeasure enrol                                   # show the current enrolment
commonmeasure enrol --name "My project" --reporting local
commonmeasure enrol --directory /path/to/project --name "My project" --reporting hub --include-history
```

Each prints the directory's enrolment as JSON. Its `reporting` field is the
state: `not_enrolled`, `local_only`, `relay_config_invalid` (reporting
chosen, and `relay.json` is one the relay refuses; `relay_config_error` says
why; requires 0.4.2), `receiver_missing` (reporting chosen, no receiver configured),
`permitted`, or a reason reporting is held.

- Hub reporting covers the directory and everything below it, including
  eligible witnessed evidence recorded there before; `--include-history`
  acknowledges that. Sibling directories and other Git worktrees of the
  same repository are separate. A local-only ancestor blocks reporting
  below it. Source policy and confidential exclusions still apply.
- On a managed edge the request waits for an owner's approval on the hub's
  **Project reporting** page. `commonmeasure enrol --sync` then fetches the
  policy and the signed approval; a pending or expired approval, or a
  policy that withholds reporting, is reported as such.
- The MCP server resolves its directory when it starts. If it started in
  another directory, restart the host session in the project.
- `commonmeasure enrol --remove` stops reporting without deleting evidence,
  removing keys or disconnecting the edge. Events already delivered stay at
  the receiver.

Expiry and worktree handling are in
[`docs/contracts/directory-enrolment.md`](contracts/directory-enrolment.md).
