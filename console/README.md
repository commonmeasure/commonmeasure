# The operator console

The console is where an operator reads their own Common Measure record: what content their
agents took in, under which policy, and what left the machine. It is served
by `commonmeasure serve` on loopback, rendered server-side from the local
evidence logs. It edits three declarations and nothing else: the policy mode,
a scope's denied hosts and the attribution rules. Each edit is validated by
the runtime's own policy loader and saved atomically behind a revision
check. This directory holds its stylesheet, scripts, brand SVGs and fonts; the pages themselves are produced by
`crates/commonmeasure-console`.

```sh
cargo run -p commonmeasure-cli -- serve                 # http://127.0.0.1:4173
cargo run -p commonmeasure-cli -- inspect <run-directory>  # a run dossier, in the terminal
```

A measurement the record does not hold renders as the word "unknown", never
as zero and never as a blank (`docs/FAIL-POLICY.md` §7).

Terms like crossing, engagement, scope and policy mode are defined in
`docs/GLOSSARY.md`.

## Assets

The vendored `htmx.min.js` (htmx 2.0.7, BSD Zero Clause licence) drives
fragment swaps. A separate local `theme.js` handles appearance and sidebar preferences. The htmx
file is the unmodified upstream release, verifiable offline against this hash:

```
source  https://unpkg.com/htmx.org@2.0.7/dist/htmx.min.js
size    51076 bytes
sha256  60231ae6ba9db3825eb15a261122d5f55921c4d53b66bf637dc18b4ee27c79f9
```

`styles.css` carries the Hub's canonical application palette: white grounds,
navy text, pale-grey panels and cobalt actions in light mode; ink grounds,
pale-blue links and coral actions in dark mode. Data surfaces remain white
in light mode. The inset sidebar switches the Edge mark between the
website's approved colourways. Controls use 10px corners, small details 6px and panels 14px.
Primary actions have a 44px minimum height; compact fields use 36px. Status colours retain labels and remain distinct from actions.

Hub and Edge share a 15rem vertical sidebar. At 52rem and below, a Menu
control opens the same vertical links above the content. The active page has
a filled row and a left marker. The collapse button reduces the sidebar to a
4rem rail, keeping navigation icons and the active-page marker. Hover or focus
an icon for its label. The console remembers the collapsed state locally.
Appearance stays at the bottom
left in both sizes, or after the links inside the mobile Menu. Long navigation
scrolls above the footer. Navigation works without JavaScript; collapsing and
saving preferences require the local script.

The first visit uses light mode. The appearance button switches light and dark,
remembered in local storage by `/theme.js`; if storage is unavailable, the choice
lasts for that page. Console-served guides share this control and preference.
Standalone guide exports stay script-free and light.

The console serves Geist and Geist Mono from embedded WOFF2 files under
`/fonts/`; `fonts/Geist-OFL.txt` carries the licence. Logo, favicon, CSS, fonts
and the htmx and appearance scripts work offline. The content security policy permits images and fonts
only from the same origin. The console-served guide uses the same local fonts;
standalone exported guides retain system fallbacks.

The website owns `design-tokens/tokens.json`. The vendored release in this
repository contains the generator and values for the console and guides.
`node design-tokens/generate.mjs --check` (also `just tokens`) detects drift;
`just gates` includes it. See `design-tokens/README.md` for cross-repository sync.
`crates/commonmeasure-console/tests/design_tokens.rs` checks contrast and the
embedded palettes against that source offline. Rust builds need no Node or
network access.

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

**Compare** (`/app/compare`) sends one query to the providers the operator
selects, sequentially, requesting up to five results each. No provider is
selected initially. `internal` uses the configured local corpus; remote calls
require supplier access/network and may incur charges. Configuration presence
is not proof of access or remaining credit. Credentials are read from the
launching environment and the selected Edge home's `credentials.env` at startup;
restart after changing them. Invalid credential files stop startup.

**Next comparison setup** shows the Edge home, directory and principal used
to resolve policy. Each
comparison resolves the process's OS principal and current local source policy,
then uses the same allowance gate, admission and text screens as mediated MCP
search. It neither enrols with Hub nor changes source permissions. Managed
policy validity is checked by the existing loader; Compare does not initiate a
Hub policy refresh. Hosted Word's separate service and Hub-held credentials do
not become available merely by opening this local console.

The local record retains the query/hash, policy identity and effective policy,
principal binding, requested limit, supplier outcomes, result provenance,
refusals, charge and latency. Query and result text are private operator data:
queries are retained locally; returned snippets are screened but omitted from
the comparison summary, and the existing private-address recording floor still
applies. The source record contains response hashes, not archived supplier
response bodies, so this is not a sealed replay dataset. Missing cost and
latency remain unknown. Charges apply to the whole request, including refused
results; admission is after supplier acquisition. Unknown-price allowance
behaviour is unchanged and does not promise a hard spend ceiling.

Records are stored in `<home>/comparisons/<id>.ndjson`, outside the relay's
session scan. The history links reopen results; **Source record** expands the
underlying policy, processor and crossing evidence. **Download private source
record** saves the same evidence. Started-only records show an incomplete
attempt and possible charges. An evidence write
failure stops subsequent providers. No automatic retry or Hub export occurs.
This is a retrieval probe; controlled answer-quality evaluation remains the
separate batch run path.

**Export results** downloads a separate ZIP with a self-contained HTML report,
summary/case CSV tables and a JSON manifest, using only permitted fields from
retained evidence. Query text is excluded unless **Include query text** is
selected; review that exact text before sharing. Internal-source comparisons,
including internal or withheld private results returned by remote providers,
cannot be exported. The report preserves unknown measurements and evidence or
allowance gaps. It needs no Hub account and makes no supplier calls. See the
[export contract](../docs/contracts/comparison-export.md) for the format and
`GET /api/compare/export` endpoint.

`POST /api/compare` accepts `{"query":"...","providers":["internal"]}`;
HTML forms repeat `provider` fields. Empty, repeated or unknown selections are
refused before dispatch. `GET /api/compare` lists local identifiers, and
`GET /api/compare?comparison=<id>` reads a record. These are unauthenticated
local console routes under the existing Host/Origin guards, not a remote
organisation execution API.

A published run directory is not rendered by the console: `commonmeasure
inspect <dir>` prints the dossier (`docs/contracts/run-output.md`), and a run's spend
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

## Local visual fixture

`cargo run -p commonmeasure-console --example design_preview` serves the real
console at `http://127.0.0.1:4187` with three synthetic sessions in a temporary
directory. It has no provider runner, credentials or relay. Policy-form writes
affect only that temporary directory. Stop the process when finished. This
fixture establishes rendering and local interactions, not provider integration.
