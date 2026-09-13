# The operator console

The console is where an operator reads their own Common Measure record: what content their
agents took in, under which policy, and what left the machine. It is served
by `commonmeasure serve` on loopback, rendered server-side from the local
evidence logs. It edits three declarations and nothing else: the policy mode,
a scope's denied hosts and the attribution rules, each under the conditions
`DECISIONS.md` §Execution and evidence sets. This directory holds its two
static assets; the pages themselves are produced by
`crates/commonmeasure-console`.

```sh
cargo run -p commonmeasure-cli -- serve                       # http://127.0.0.1:4173
cargo run -p commonmeasure-cli -- inspect demo/output/replay  # a run dossier, in the terminal
```

A measurement the record does not hold renders as the word "unknown", never
as zero and never as a blank (`docs/FAIL-POLICY.md` §7).

Terms like crossing, engagement, scope and policy mode are defined in
`docs/GLOSSARY.md`.

## Assets

The only script the console ships is the vendored `htmx.min.js` (htmx 2.0.7,
BSD Zero Clause licence), which drives fragment swaps; the console itself
contains no JavaScript. It is the upstream release unmodified, which a reader
with no network can confirm against the recorded hash:

```
source  https://unpkg.com/htmx.org@2.0.7/dist/htmx.min.js
size    51076 bytes
sha256  60231ae6ba9db3825eb15a261122d5f55921c4d53b66bf637dc18b4ee27c79f9
```

`styles.css` carries the product's design tokens in a light and a dark
theme; the token rules, the hub as the canonical palette and the test that
enforces both are in `DECISIONS.md` §Integration and ownership. Colour never
carries meaning alone; every state sits beside a text label.

## Serving it

Nothing here authenticates, so the console binds loopback and refuses to bind
anywhere else without `--allow-remote`. A non-loopback bind publishes every
recorded URL, host, cwd and session, the Compare form and the policy and
attribution write routes, to anyone who can reach the address; passing the
flag prints that as a warning. Requests naming
a `Host` other than the loopback console are refused as well, so a public
page cannot reach it by pointing its own name at 127.0.0.1, and a request
carrying a foreign `Origin` is refused.

`commonmeasure serve` derives an index (`~/.commonmeasure/telemetry.db`) from the
session evidence logs. The index refreshes before every answer, so a crossing
recorded while the console is open appears on the next reload. Deleting the
database loses nothing; it is rebuilt from the logs. The pages exist only
through the binary: there is no plain-file fallback, because the markup is
produced by the same code that answers the JSON API behind it.

## The five sections

The console is an app shell with a sidebar of five sections, each a
bookmarkable route. Every screen opens with its title and a line naming what
it was read from and when: the sessions directory the index was derived from
and the time of the read, or for Policy the policy file and its revision.
Labels and values carry the rest; a state with nothing to show is stated in
words, never left blank.

**Overview** (`/`) states the numbers first: crossings recorded, witnessed by
Common Measure, refused by policy, and cleared to leave the machine (the relay's
delivered count; with no receiver configured it reads 0). Beneath them, the
most recent crossings by host, and each engagement with its crossings,
attributed by the working directory each crossing ran in.

**Record** (`/app/record`; `/sessions` lands here) is a master-detail over
the sessions: a rail on the left, grouped by the engagement the attribution
rules (`~/.commonmeasure/attribution.json`) resolve for each session, and a pane
on the right that holds the chosen session. Each rail row is a link to
`/app/record?session=<id>`, which the server renders with the pane filled and
the row marked current, so a session is bookmarkable and reachable without a
script; with htmx the pane is swapped in place and its heading takes focus. A
rail row reads "N crossed", N being the observed and mediated crossings
together, with "N refused" and "N breached" beside it when either is
non-zero. A Claude Code session's mediated crossings are recorded under a
separate `local-<timestamp>-<pid>` session of the MCP server's own, beside the
host session's observed record, and the rail says so. The pane states the
session's facts (witnessed, reconstructed, grounded, refused, breached), then
each crossing as a card: its URL, grade badge, grounded badge, host and
licence, then the record's own fields as labelled rows, each present only when
the record carries it (`docs/contracts/session-evidence.md` §Crossing):

- *bytes received*: `retrieved_hash`, the hash of the body the origin served,
  which a mediated fetch that received a body carries and no other record;
- *text read*: `content_hash`, the hash of the text delivered to the agent,
  which every witnessed crossing carries;
- *status*: `http_status`, on a mediated fetch that was answered;
- *identity*: what the request presented, `signed as <key id>` for an
  enrolled edge or `unsigned` with the recorded reason, on a mediated crossing
  whose request left the machine.

A refused crossing is a marked card carrying the reason and none of these
fields, because nothing was fetched. Witnessed and reconstructed evidence are
never totalled together. A session that crossed nothing says so, and names
what it did record instead — turn boundaries, and lines the console could not
read — so no crossings is not read as nothing happened.

**Policy** (`/app/policy`) states what governs the next crossing, read from
the same `~/.commonmeasure/policy.json` the runtime reads and resolved by the
same loader: the mode in force (observe, prefer or strict) with its
consequence stated, and one row per declared scope with its mode (marked
inherited where the scope declares none), the engagement it governs, the
directories it has covered, its denied hosts and its access rules, headed by
how many engagements are cleared to leave the machine. A separate table
names every scope where the governing engagement the policy declares and
the reported engagement the attribution rules resolve differ, with both
names; neither overrides the other. With no policy file it says so, names
the path, and offers no policy form: the console never creates a policy
file.

The screen makes three edits. The mode dial and each scope row set a mode;
the block form adds a host to a scope's denied-host set; the attribution
editor replaces the ordered attribution rules. Every form carries the
revision of the file it was rendered from, and a save against a file that
changed underneath is refused with both revisions stated and nothing
written. A policy save goes through the runtime's own loader, so the console
cannot write a policy the runtime would refuse; a refused save leaves the
file as it was and states the loader's reason. A saved policy edit governs
the next crossing and nothing already recorded, and a mediated session
already running keeps the policy it loaded until it next starts; the page
says so on every save. A scope that inherits the top-level constraints keeps
them as its own list on its first denied host, and the save says the scope
has stopped inheriting. Fields that widen what may leave the machine or
enter the record (`allow_telemetry_egress`, `record_internal_prefixes`) are
not editable here.

Before a save, a draft can be forecast over the recorded history: the
candidate policy is built through the runtime's document and every recorded
crossing is put through the runtime's admission check under the standing
and the draft policy, and the table reports what the draft would newly
refuse, newly carry as a breach or newly admit, witnessed, reconstructed and
previously refused each on its own line and never totalled. A draft is a
mode for a scope, a denied host for a scope, or an access rule (a host
pattern and an action), for which the forecast shows the JSON to add to the
scope; the console does not write access rules. The forecast states the
principal it judged as, which is the console process's own, so a crossing
recorded under another principal is judged under this one's overlay and the
count of such crossings is stated; that reconstructed crossings carry no
working directory and are judged under the top-level policy; and how many
crossing records carried no URL and could not be judged. A forecast writes
nothing.

A write answers with its outcome's own status: 200 for a save or an edit
that changed nothing, 409 for a conflict or an undeclared policy, 400 for a
form or a candidate the loader refused. The page configures htmx to swap
the answer in on every status, so the notice is read whatever the outcome.

**Sources** (`/app/sources`) lists every provider that takes a key and
whether one is present in the operator's credentials. The console checks
presence and never fetches. A source says where content came from, never
that the operator has the right to use it; licence stays unknown unless a
provider states one.

**Compare** (`/app/compare`) runs one query live across the connected
providers, each answering with its results, cost and latency. These are
real, credit-spending calls, made only when the operator submits the form.
It records nothing and edits no declaration. When no provider holds a key
the screen says the query is unavailable and why, and sends nothing.

A published run directory is not rendered by the console: `commonmeasure
inspect <dir>` prints the dossier (`docs/READ-A-RUN.md`), and a run's spend
is read there; a session record carries only the observed charge a provider
reports for a mediated search.

## The guide

`/guide` serves the guide to context window optimisation, with its evidence
companion at `/guide/state-of-the-evidence`, rendered at first request from
`docs/guide/*.md` with `docs/guide/guide.css` inlined
(`crates/commonmeasure-console/src/guide.rs`): self-contained pages with no
scripts and no external assets. They are publications, not console screens;
the only relaxation the serving layer makes to its content-security policy is
admitting each page's own inline stylesheet by hash. `commonmeasure guide
<dir>` writes the public variant of the same pages, with every product panel
stripped and no product name in the output, for publication.
