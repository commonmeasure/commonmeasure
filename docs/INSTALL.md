---
title: Installing and updating
domain: edge
audience: operator
section: get-started
---

# Installing and updating

For a first session, follow [Getting started](GETTING-STARTED.md).
Release-download examples were checked on 0.4.4 on macOS (Apple silicon).
They pin that release deliberately and have not been rerun here. Build and
batch-run examples were checked on 0.4.0; the current toolchain requirement
is given below. Commands that replace binaries or login services are shown
for an operator to run, not claimed as newly verified.

## From a release, with no toolchain

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
that directory is not on `PATH`. It checks both checksum and version before
placement, leaving an installed binary intact if either check fails. `commonmeasure update`
runs the copy of this script compiled into the binary.

```sh
curl -fsSL https://github.com/commonmeasure/commonmeasure/releases/latest/download/install.sh | sh
```

From an empty home directory it prints, with the release's version in place
of `<version>`:

```text
installed ~/.local/bin/commonmeasure: commonmeasure <version>, checksum verified
~/.local/bin is not on PATH. This installer does not edit shell profiles; add this line to yours:
  export PATH="~/.local/bin:$PATH"
Next: commonmeasure install claude (or codex, pi, claude-desktop, cursor, copilot, vscode, chrome) to register with your host, then work a session, then run 'commonmeasure session' to see what it recorded and 'commonmeasure serve' for the console on loopback. Session records stay on this machine, and no use of a source is reported until relay.json names a telemetry receiver and a policy scope clears egress.
```

The home directory is written as `~` above; the installer prints it in full.
With a terminal, the installer also asks for reporting consent between the
`PATH` lines and `Next:` (below).

### Connect during installation

`--connect HUB` connects the machine to Common Measure Hub during the
install. After installing, it runs `commonmeasure first-run`,
registers detected hosts, connects under managed policy, checks directory
reporting, installs the background relay on macOS
([below](#relay-without-a-session-end)), makes one governed fetch, relays
and prints the evidence addresses. To connect, it prints a code and the hub
page to approve it on, opens the page where the desktop has an opener, and
waits until you approve the code signed in to the hub. `--token TOKEN`, from
the Hub's **Enrol this machine** card, connects with a token instead, which
is how an owner enrols someone else's machine. The token is single-use and
short-lived; it is not printed or stored.

Run the line in the project directory. A fresh directory remains unenrolled
until you confirm history with `commonmeasure enrol --name NAME --reporting
hub --include-history`. The home directory and `/` are never enrolled. See
[Getting started](GETTING-STARTED.md) for the steps and remedies.

`--token` needs `--connect`, neither may be empty or whitespace-only, and
`--connect` with `--update` is refused, all before any download (exit 2). A
code refused in the hub, one that expires and an unreachable hub stop the
connect step with the reason. If a
first-run step fails, the installer exits 3 with the binary still installed
and names the failed step. Steps already completed stand; later steps do not
run. Reporting consent remains the operator's choice, as below.

### Reporting consent

Some sources license their content only if each use of it is reported. They
are refused until you agree to report to them. When the Edge home records no
answer and the installer has a terminal, it shows the consent text and asks
once; the answer is read from the terminal, so the `curl | sh` form above
asks too:

```text
reporting consent: not given; sources whose licence demands reporting are refused. Agree with: commonmeasure consent agree

Consent text 1:
Some sources license their content only if each use of it is reported. If you agree, Common Measure admits those sources in every policy scope and reports each use of them through the telemetry receiver named in relay.json: the page address, when it was retrieved and entered context, its licence, its content hash and token estimate, and this edge's key id. Prompts, answers and page text are not sent. If you do not agree, sources that demand reporting are refused. Withdrawing applies to later fetches; uses already admitted are still reported.

Agree to report to sources that require it? [y/N] y
reporting consent agreed at 2026-09-29T10:00:00Z (consent text 1), recorded in ~/.commonmeasure/consent.json
no receiver is configured in ~/.commonmeasure/relay.json, so sources that demand reporting stay refused until one is (commonmeasure connect)
```

The closing `Next:` line ends by what the recorded answer lets leave:

- agreed: "Session records stay on this machine. With reporting consent
  agreed, each use of a source whose licence demands reporting is reported
  to the telemetry receiver relay.json names, once one is named."
- no answer recorded: the line shown above.
- withdrawn, or a `consent.json` that does not count: "Session records stay
  on this machine. Without reporting consent agreed, sources whose licence
  demands reporting are refused; a use admitted while consent was agreed is
  still reported to the telemetry receiver relay.json names, and no other
  use is reported until a policy scope clears egress."

Any answer but `y` or `yes` records nothing, and the installer prints the
command that agrees later. An install with no terminal, or with a non-empty
`CI` value, records nothing and prints the same line, unless it is given
`--agree-reporting` or `COMMONMEASURE_REPORTING_CONSENT=agree`, which record
agreement without asking. A home that already records an answer is not asked
again, and `--update` never asks. The same answer is shown, given and
withdrawn at any time with one command:

```sh
commonmeasure consent            # the answer recorded and the text it refers to
commonmeasure consent agree
commonmeasure consent withdraw
```

The answer is yours alone: `deployment.json` and a hub-managed policy cannot
set it, and a scope's `allow_telemetry_egress` is not read as it. It counts
only from `consent.json` as a regular file in the Edge home, owned by the
user who runs Common Measure and writable by no one else. A symbolic link, a
file owned by another user and a file writable by its group or by others are
refused, and the refusal names which, as it names the error of a file that
does not read. `consent agree` and the installer write the file at mode
0600 whatever the umask, replacing a link rather than writing through it. A
home that records no answer, an existing one included, has no consent until
you give it; until you agree, `commonmeasure status`, `commonmeasure doctor`
and the console's Overview list the sources refused for want of it. What
leaves under consent is in [Reporting](OPERATING.md#egress-only-when-you-ask-for-it).

To pin a release, download that release's installer into a directory of its
own (a checkout has an `install.sh` of its own at the root) and name the
tag. `--dir` installs somewhere other than `~/.local/bin`, and `--plugin`
also downloads, verifies and unpacks the plugin archive, then prints the two
commands that install it into Claude Code:

```sh
curl -fsSL -o /tmp/commonmeasure-release/install.sh --create-dirs \
  https://github.com/commonmeasure/commonmeasure/releases/download/v0.4.4/install.sh
sh /tmp/commonmeasure-release/install.sh --tag v0.4.4 --plugin ~/commonmeasure-plugin
```

To check a download by hand, fetch `SHA256SUMS` beside it:

```sh
curl -fsSLO https://github.com/commonmeasure/commonmeasure/releases/download/v0.4.4/SHA256SUMS
curl -fsSLO https://github.com/commonmeasure/commonmeasure/releases/download/v0.4.4/commonmeasure-darwin-arm64
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

## Updating

If your installed updater refuses while hosts run the binary, run the
installer once instead; it replaces the binary in place.

```sh
curl -fsSL https://github.com/commonmeasure/commonmeasure/releases/latest/download/install.sh | sh
```

`update` does not ask you to close hosts.

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
  asked only after `update` has tried to start the console and relay
  services again, whether or not that succeeded, and given 15 seconds and
  4 KiB of output.
  Past either, it and the processes it started in its process group are
  stopped; a process that leaves that group is not stopped. Past either, or
  when it exits non-zero, the version is not known and `update` prints the
  command that shows it. A non-zero exit stops nothing it left running;
- whether the binary was replaced is not known, because it could not be
  read, or changed or was replaced while it was read, with the command that
  shows its version.

The version check’s 15-second bound covers the wait for output and exit,
not the system calls that spawn and reap the child or the whole `update`
command.

The comparison reads the binary twice, before and after the installer; it
is not a snapshot. A process writing the binary at the same time can change
it between or during those reads without being told apart from the
installer, and nothing locks the binary against other writers.

A failure restarting the console or relay service is described below. It replaces the
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

`update` does not stop other processes that run the binary, and does not
refuse because of them. The installer moves the new binary into place by
renaming it over the old file, so a process already running keeps the file
it started from: an MCP server a host started, a hook in progress,
`commonmeasure hosted service`, or a `serve` or `relay` started by hand
runs the old release until it exits or its host starts it again. After
every successful install, including a reinstall of the release already
installed, `update` lists these processes; when the version changed it then
says what new sessions run. Output shape, not rerun here; versions, process
ids and paths below are placeholders:

```text
These processes run commonmeasure <old-version>, or a release installed before it, until their host restarts them:
  pid <pid>  mcp  COMMONMEASURE_HOME not set or empty
  pid <pid>  serve  COMMONMEASURE_HOME=/home/op/edge-a

Hosts start the MCP server and hooks from this binary: sessions opened from now on run <new-version>.
```

Each process is shown by pid, subcommand (such as `mcp`) and
`COMMONMEASURE_HOME`, or `COMMONMEASURE_HOME unknown` when its environment
cannot be read. macOS withholds the environment of a restricted process, so
an empty one is reported as unknown rather than unset, as is an environment
of zero bytes on Linux, and one with an empty entry followed by more entries
that do not include `COMMONMEASURE_HOME`. On macOS, where the kernel's own
strings follow the environment, it is also unknown when `COMMONMEASURE_HOME`
appears only past where the environment was taken to end (`ARCHITECTURE.md`
§What runs where). An empty or whitespace-only `COMMONMEASURE_HOME` is
reported as `not set or empty`, as is an environment that does not set it;
these use the default home. No other argument or environment variable is
printed, since either can hold a credential. A process whose executable the
system does not identify is listed by its process name when that name is
`commonmeasure`: on macOS, a process that still runs a binary whose file has
since been removed. When the process table cannot be read, `update` says so
in one line and the update stands.

The list leaves out other users' processes, copies of `commonmeasure` at
other paths, and processes started while the installer ran. Until the
listed processes restart, processes of two releases share the Edge home
(`ARCHITECTURE.md` §What runs where). To finish on one release, start a new
session in each host, and stop and start `commonmeasure hosted service` and
any `serve` or `relay` started by hand; `commonmeasure service status`
names a console on the service's port that the service did not start.

The console service and the background relay service (below) are handled as
follows, the console first:

- A loaded service running this binary is stopped before the installer runs,
  and started again afterwards with the configuration in its plist: its
  address or interval, Edge home, log and working directory, whatever the
  shell running `update` selects.
- Stopping waits up to 10 seconds: for the console's port to free, and for
  the background relay's loop, which finishes the run in progress after it
  is stopped, to release `relay-loop.lock` in its Edge home. Starting again
  waits up to 10 seconds for the console to answer on its port, or for the
  new loop to hold `relay-loop.lock`. A console whose port stays held is
  started again at once and nothing is installed. A relay loop still holding
  its lock after the wait stops the update before anything is installed; the
  console, if it was stopped, is started again, and `update` prints the
  command that starts the relay service once the loop has exited.
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

## Keep the console running (macOS)

Service commands describe 0.4.6. They were not rerun here; the original
walkthrough did not record a checked release for service installation.

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
with the Edge home and log in its plist ([Updating](#updating)); after an upgrade by
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

That unit is transient: systemd does not restart it if it exits, and it
does not start at login. For both, write a unit under
`~/.config/systemd/user` with `Restart=on-failure` and enable it.

## Relay without a session end

Relay-service commands describe 0.4.6. They were not rerun here; the
original walkthrough did not record a checked release for these commands.

Claude Desktop, Codex, Cursor, the Copilot CLI and VS Code
send no session-end event. The background relay relays the Edge home on an
interval instead, to the receiver in `relay.json`:

```sh
commonmeasure service install relay       # macOS, every 300 s; --every to change
commonmeasure service status relay
commonmeasure service uninstall relay
```

After a successful `connect --managed`, macOS offers to install the relay if
none is installed for this Edge home and neither a relay loop nor a hosted
service holds it. Installation requires an explicit yes at the terminal. If
the installed relay serves another home, the offer names that home and asks
whether to move it, defaulting to no. Moving it stops that home's background
reporting; hosts without a session-end event there refuse sources requiring
usage reporting until another automatic relay runs.

With any non-empty `CI` value, non-terminal stdin or stdout, or
`--no-relay-offer`, the finding and command are printed without prompting or
installing. Other platforms print `commonmeasure relay --every 300` for your
service manager. If an accepted installation fails, enrolment still succeeds
and the finding names the failure and the command to retry.

The Hub's one-command line ([Connect during
installation](#connect-during-installation)) asks nothing: on macOS its
first run installs the relay before its governed fetch, where none is
installed for this Edge home and neither a relay loop nor a hosted service
holds it, because the first run's own session also sends no session-end
event. A relay installed for another home is left in place. If the
installation fails, the first run names the failure and the command to
retry, goes on, and the fetch is refused.

`install relay` writes `~/Library/LaunchAgents/ai.commonmeasure.relay.plist`,
which runs `commonmeasure relay --every 300` at login, logging to
`logs/relay.log` in the Edge home, and waits until the loop holds the home.
Launchd restarts the loop if it crashes, at most once in five minutes. A loop
that finds no receiver in `relay.json`, or a `relay.json` that does not load,
says so in the log, exits and stays stopped until you install it again;
`status` and `doctor` then say the agent is installed but not holding the
home, and name its log. `install relay` refuses when `relay.json` names no
receiver, and when a background relay you started by hand already holds the
home. `commonmeasure disconnect` removes `relay.json` and names `service
uninstall relay` while the agent is installed. `commonmeasure update` stops
the agent before it replaces the binary and starts it again with the Edge
home, interval and log in its plist ([Updating](#updating)). On Linux, run `commonmeasure
relay --every 300` under your own service manager; `service` names a
`systemd-run` command, whose unit is transient (not restarted, not started at
login), as for the console above. While it runs, `status` and `doctor` show
`background relay: running`, and a Claude Desktop session may use a source
whose licence demands usage reporting, where you have agreed to reporting
([Reporting consent](#reporting-consent)) and `relay.json` names a receiver. A
second one on the same home is refused. Each run sends what a session-end run
would send, no more.

To review each run before it leaves, create the empty file
`~/.commonmeasure/relay/manual`. Nothing then relays at a session end or on
the hosted service's or the background relay's interval, and a source whose
licence demands usage reporting is refused while the file is there. Delete
it to relay automatically again.

## If you build from source

From a checkout, the test suite, a build of the
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
have placed, which then registers with a host as in [Connect an agent host](integrate/host.md):

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
