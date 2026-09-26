# Changelog

Each release's section is also its GitHub release notes: the release
workflow publishes the section whose heading names the tag's version.
Versions follow [Semantic Versioning](https://semver.org/); before 1.0.0 a
minor version may break compatibility. `RELEASING.md` §Release notes says how
a section is written.

## 0.4.2 (26 September 2026)

**Upgrading:**

- 0.4.2 refuses a `relay.json` whose `receiver` URL carries
  credentials (`https://user:key@…`), a query or a fragment. Until the file is
  corrected, automatic relay is off: the session-end hook starts no relay run,
  `commonmeasure relay` and the hosted service send nothing, `status` and
  `doctor` report the load error, and reporting demands are ruled unmet. To
  correct it, remove the credentials, query or fragment from
  `receiver` and give the key as `api_key`
  ([one receiver](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/telemetry-projection.md#one-receiver)).
  `commonmeasure connect` now refuses a hub URL carrying any of them.
- Where `robots.txt` refuses the manifest, 0.4.2 does not request it and
  records the outcome `refused`. A 0.4.1 process on the same Edge home treats
  that record as absent and requests the manifest without checking
  `robots.txt`. Stop every `commonmeasure` process that uses the Edge home,
  including the console service and `commonmeasure hosted service`, before
  upgrading, and do not run 0.4.1 on that home afterwards
  ([one release per Edge home](https://github.com/commonmeasure/commonmeasure/blob/main/ARCHITECTURE.md#what-runs-where)).
- 0.4.2 refuses an enrolment whose stored hub URL is `http` to a host other
  than `127.0.0.1`, `localhost` or `[::1]`, or carries credentials, a query
  or a fragment. Up to and including 0.4.1, `commonmeasure connect` stored a
  cleartext hub URL and a hub URL with credentials. Until the edge is
  reconnected, nothing is sent to the hub: relay, standing checks, directory
  proofs, managed policy, reporting approvals, instance requests and
  supplier credentials are refused, and `commonmeasure hosted serve` and
  `hosted service` do not start. `status` and `doctor` state the refusal as
  `hub URL refused:`, `status --json` gives the key standing `cleartext_hub`
  or `unusable_hub_url`, and the console's Overview and a session's
  Reporting block on Agents give the reason and the remedy; a session that
  ran under such an enrolment raises an Attention item on Agents. To
  reconnect, run `commonmeasure disconnect`, revoke
  the edge key and the ingest key it names on the hub's API keys page,
  replace any key the URL carried (a 0.4.1 hosted edge on that home
  published it), and run `commonmeasure connect` with an https hub, by its
  address alone
  ([enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/enrolment.md#the-exchange)).
- After upgrading, act on the entries below that apply to your home: revoke
  or rotate a credential 0.4.1 or earlier exposed (a hub URL `connect`
  stored, a `policy_url` in `deployment.json`, a manifest or supplier URL
  signed through Encypher, a relay receiver's key); run `chmod 600` on the
  relay state files and the existing lock files in the Edge home; and delete
  any leftover `hosted-tokens.json.tmp` or `relay/*.json.tmp`.

### Added

- Search Wikipedia or arXiv through Dataville with `DATAVILLE_API_KEY`, recording one result per search, any declared licence and the reported USD charge ([provider contract](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/provider.md)).
- Send a supplier's own telemetry receiver only that supplier's events: set `suppliers` in `relay.json`, such as `["ozone"]`, and the receiver gets the supplier's retrievals and any grounding recorded for what it served, with no turn boundaries, no other source and no refused count. An empty list sends nothing, a reporting demand on a fetched page, from its licence or the operator's terms, is unmet while the list is set, and `relay.json` is refused when the list is not a list of names or the receiver URL has a query, fragment or credentials. Mediated search records a supplier's results as retrievals only, so from mediated search the receiver gets retrieval events and no grounding ([supplier scope](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/telemetry-projection.md#supplier-scope)).
- `commonmeasure enrol` reports `relay_config_invalid` for a directory chosen for hub reporting when the relay would refuse `relay.json`, with the load error in `relay_config_error` and the policy's own clearance beside it ([directory enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/directory-enrolment.md)).
- `commonmeasure service install console [--listen <addr>]` runs the console as a macOS LaunchAgent (`ai.commonmeasure.console`): the installing binary by absolute path, `RunAtLoad` and `KeepAlive`, logging to `logs/console.log` in the Edge home (`~/.commonmeasure`, or the installing shell's `COMMONMEASURE_HOME`, which the plist sets for the service). It is loaded with `launchctl bootstrap` and started with `launchctl kickstart`. A non-loopback `--listen` is refused, and so is an address another process already listens on, naming its pid. Installing again rewrites the plist from the invoking shell and restarts the console ([console service](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#keep-the-console-running-macos)).
- `commonmeasure service status` reports whether the service is installed, loaded and listening; the binary, version, Edge home, log and port it runs with; and the `commonmeasure` on `PATH`. It names the restart command when the running console's version differs from the binary on `PATH` or its binary changed after it started. It names a console on the port that the service did not start. It says so when launchd cannot be asked. It also says when the plist has been edited since installation ([console service](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#keep-the-console-running-macos)).
- `commonmeasure service uninstall console` boots the agent out and removes the plist ([console service](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#keep-the-console-running-macos)).
- On Linux and other platforms apart from macOS, `commonmeasure service` refuses and names the equivalent `systemd-run --user` command ([console service](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#keep-the-console-running-macos)).
- The console answers `GET /api/version` with its version and process id ([console service](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#keep-the-console-running-macos)).
- `commonmeasure service status` asks the `commonmeasure` on `PATH` for its version within 15 seconds and 4 KiB of output, as `update` does for a binary a failed install replaced. When it does not answer in time, prints too much or prints no version, `status` says the version is unknown and why. The bound covers only the wait for version output and exit ([console service](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#keep-the-console-running-macos)).
- `commonmeasure update` installs the latest release, or `--tag vX.Y.Z`, over the binary it runs as, through the installer compiled into it, on macOS and Linux; `--check` reports the installed and latest versions and changes nothing. A release build ignores `COMMONMEASURE_RELEASE_URL`. 0.4.1 has no `update`: rerun the release installer ([updating](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#updating)).
- Release tags must be `vX.Y.Z`; `update` fails on a latest tag of another form. `--tag` may name a lower version ([updating](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#updating)).
- Before stopping anything or downloading release assets, `update` refuses while other processes of yours run the same binary file, and lists each by pid, executable, subcommand and `COMMONMEASURE_HOME`; stop them and run it again. It also refuses for a process whose executable the system does not identify but whose name is the binary's, for a macOS process whose owner and name cannot be read (listed as not inspected, with the `ps` command that checks it), and when the console service restarts during the check. The check does not find other copies of the binary, processes started after it or other users' processes ([updating](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#updating)).
- A loaded console service running that binary is stopped before the installer runs and started again afterwards, whether or not the install succeeded. When the installer fails, `update` says whether the binary is unchanged, was replaced (with its version) or cannot be told. When the service cannot be started again, `update` fails, says whether the binary was replaced, and prints the command that reinstalls the service ([updating](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#updating)).
- `update` refuses a loaded console service when its plist is missing, unreadable, edited since `service install` wrote it or different from the job launchd loaded; when launchd's report of its environment cannot be read, or sets the job's own `COMMONMEASURE_HOME` empty with none inherited; or when it inherits `COMMONMEASURE_HOME` from launchd's domain. The refusal names the commands that remove or reinstall the service ([updating](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#updating)).
- On macOS, `update` refuses a binary reached through a symbolic link. On Linux it does not detect the link and replaces the file the link points to. Nothing checks for updates in the background ([updating](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#updating)).
- The checksum shows the binary matches the release's `SHA256SUMS`. Releases are not signed, so it does not show who published them ([updating](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#updating)).
- A mediated fetch's crossing records `requested_at`, when the page request was handed to the transport after any `Crawl-delay` wait; `timestamp` stays the time the record was written. Crossings where no request was attempted, and records written by earlier versions, have none ([session evidence](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/session-evidence.md#crossing)).

### Changed

- The edge's own requests follow your `robots.txt`: it requests your Content Telemetry manifest, a licence that only a page's `Link` header names, and any redirect from either only where `robots.txt` at that URL's origin allows it for `CommonMeasureBot`, in every policy mode. A redirect from either is followed only in its host's `Crawl-delay` turn; where that turn does not fit the fetch, the redirect is not followed. After a 404 the manifest is asked once more, at the registrable domain (`example.com` for `news.example.com`); that second request never goes to a public suffix such as `co.uk` or `pages.dev` ([bot identity](https://github.com/commonmeasure/commonmeasure/blob/main/docs/integrate/bot.md)).
- A licence a `License:` line in `robots.txt` names at another origin is checked against `robots.txt` at that origin. Where that file refuses it, the licence is unread and the page is refused before it is requested, in every policy mode, unless the source policy holds an applicable assessment of the host's terms. Up to and including 0.4.1, such a licence was requested without that check. A licence named at the page's own origin is still requested without it; one named at the origin the page's `robots.txt` redirects to is checked there ([session evidence](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/session-evidence.md), [terms](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/source-policy.md#terms)).
- `context_fetch` returns a page's text in parts of at most 60,000 characters. A result that stops short says so (`truncated`) and names the `offset` to ask for next; pass `max_chars` for a larger part, up to 200,000. Each part is a new request to the site, recorded as its own crossing with the hash of the part delivered. An `offset` above 0 at or past the end of the text returns an error and grounds nothing ([host integration](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/host-integration.md#the-parts-of-a-fetch)).
- On the console's session, engagement and internal-use rows (`/api/sessions`, `/api/budget` and `/api/internal`), `estimated_tokens`, `estimated_tokens_by_basis`, `crossings_without_estimate`, `token_basis` and `total_withheld` now cover witnessed crossings only, observed and mediated; for a session with a context snapshot, that is the figure `commonmeasure session` states. Up to and including 0.4.1 they also counted reconstructed crossings, whose estimates are of the text in an imported transcript. Those are now counted in the same fields with a `reconstructed_` prefix, shown apart on the Budget screen ([console](https://github.com/commonmeasure/commonmeasure/blob/main/docs/CONSOLE.md#budget)).

### Removed

- `GET /api/sessions/<id>` serves a session-log line the console cannot parse, such as one torn by an interrupted write, as `{"event": "unreadable", "line": <number>, "bytes": <length>}` and no longer returns its text as `raw`; read the line in the log by its `line`. The log and the console index keep the line as written ([console](https://github.com/commonmeasure/commonmeasure/blob/main/docs/CONSOLE.md)).

### Fixed

- `install.sh` checks the downloaded binary's version before placing it. Up to and including 0.4.1, it checked after replacing the installed binary and, on a mismatch, removed it, leaving no binary. `--update` leaves out the first-install next steps ([updating](https://github.com/commonmeasure/commonmeasure/blob/main/docs/GETTING-STARTED.md#updating)).
- Enrolment changes and removal now take an exclusive `enrolment.lock`, readable by its owner only, against the stored record, so an older copy cannot overwrite a newer revocation, disconnect or enrolment. A change that has waited 10 seconds for the lock refuses, changes nothing and names the file. Readers take no lock, and a relay run whose hub says the key stands takes it only to withdraw a refusal the record holds; concurrent `connect` commands are not serialised as a whole and can leave mismatched files that prevent signing ([enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/enrolment.md)).
- A successful standing answer withdraws a refusal learnt from a `401` only when the request began in a later millisecond than the recorded refusal. An answer requested in the same millisecond leaves the refusal in force; a revocation stated by the hub remains permanent ([enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/enrolment.md)).
- Enrolment files are replaced through a temporary file unique to each writer. A crashed writer can leave `<file>.*.tmp` beside the home’s files; for `edge-key.json` and `relay.json`, these are credential copies created with mode `0600`. Neither later writes nor `disconnect` removes them. Remove these temporary files only after every `commonmeasure` process has stopped, and keep `enrolment.lock` ([enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/enrolment.md)).
- A page on a host written with a trailing dot (`publisher.example.`) asks for `robots.txt` and the content-telemetry manifest at the host without the dot, and shares their cached answers with it. Up to and including 0.4.1, such a page's manifest was rejected because its `id` names the host without the dot ([session evidence](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/session-evidence.md)).
- The console's context footprint no longer counts refused crossings. A crossing refused after the page was fetched, by the page's declarations or a screen, records the estimate of the text it withheld, and up to and including 0.4.1 the console added it to the session's, the engagement's and the internal-use figures; `commonmeasure session` did not ([console](https://github.com/commonmeasure/commonmeasure/blob/main/docs/CONSOLE.md#budget)).
- A session's crossing card in the console labels `content_hash` by how the crossing was recorded: *text extracted* on a mediated fetch, *text supplied* on a search result, whose hash is the supplier's, *text in context* on an observed crossing, whose hash is of what the host's tool returned, and *text in transcript* on a reconstructed one ([console](https://github.com/commonmeasure/commonmeasure/blob/main/docs/CONSOLE.md#record)).
- On a directory proof upload and on the standing check, at a relay run or session start, an answer the hub did not write, such as an ingress, proxy or firewall page, is reported as the hub not reached and changes nothing: a `401`, `404` or `409` of that kind no longer marks the key unlisted with the page's text shown as the hub's reason, and a `401` of that kind no longer records the key as revoked or stops signing. The hub's own refusal is recognised with or without a `code` beside its `detail`, and a key the edge concludes is unlisted after the hub's own `404` or `409` is shown as the edge's conclusion, not as `hub: <text>` ([directory proof](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/enrolment.md#the-directory-proof-and-its-renewal), [standing](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/enrolment.md#standing-and-revocation)).
- Where the spool is refused, the console's Overview states what it still owes in the words `status` prints, and a count that is incomplete reads as unknown ([console](https://github.com/commonmeasure/commonmeasure/blob/main/docs/CONSOLE.md#overview)).
- Where `enrolment.json` exists but cannot be read, the console's Overview and `status` and `doctor` say the enrolment record is unreadable, with the error. Up to and including 0.4.1, Overview said the edge was not enrolled ([console](https://github.com/commonmeasure/commonmeasure/blob/main/docs/CONSOLE.md#overview)).
- Where `enrolment.json` does not parse, the error names the kind of fault (syntax, data, end of file or I/O) and its line and column, and quotes nothing from the file. The parser's own message could quote a value, such as a hand-edited hub URL with credentials in it.
- A `429`, `500` or `503` in the hub's own shape for a directory proof upload is described as `the hub could not take the proof (<status>): <detail>; nothing was concluded from it`, where it read as the hub refusing the proof. Only the wording changes; such an answer has never changed the listing ([enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/enrolment.md#the-directory-proof-and-its-renewal)).
- A request to a host just out of back-off that gets no HTTP answer, such as a refused or reset connection, no longer holds every other request to that host for up to 30 seconds, including the next request of the same fetch. It does not count as a failure or extend back-off ([session evidence](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/session-evidence.md#source-declarations)).
- On an own edge, a fetch refused while another request to the same host holds its turn says that request is under way and when its turn ends. It no longer reports back-off lasting until that time with the status of a back-off period that had already ended; the back-off and its status are named only while that period lasts. A refusal before the crossing states its reason once: a back-off refusal and a `robots.txt` this edge did not read repeated it in parentheses, and a refusal at the hub's origin named the operator's policy file, which cannot lift it.
- When a host answers a page and this edge cannot record the answer in its back-off store, the crossing keeps the answer's `http_status` and its `failure` says the host answered and the answer was not used; `commonmeasure session` shows it as an answer not used rather than no answer, and the console labels the field Failure. Where the fault is the store's directory (the lock file or the record cannot be created, written or removed there), the remedy names the directory; it named the host's back-off file, which may not exist ([fail policy](https://github.com/commonmeasure/commonmeasure/blob/main/docs/FAIL-POLICY.md#14-a-hosts-failure-paces-its-next-request)).
- A revoked edge token no longer verifies again after a `commonmeasure hosted token issue` run at the same time as the `revoke`. From 0.3.5 up to and including 0.4.1, an issue that read `hosted-tokens.json` before the revoke wrote it could write the token back as standing. `issue` and `revoke` now hold `hosted-tokens.lock` beside the file, and one refused after two seconds behind another says no token was issued or revoked ([host integration](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/host-integration.md)).
- On an edge serving sessions over HTTP, a refusal caused by a fault in the crawl-delay and back-off store, such as a lock file that cannot be created, names the store's files under `crawl-delay`, as in `crawl-delay/example.com.lock`. Up to and including 0.4.1, the cause gave the operator's full path to the tenant, in the tool error, the crossing's `failure` and the back-off event's `reason` ([session evidence](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/session-evidence.md)).
- On an edge serving sessions over HTTP, every answer, body and headers, names the operator's files relative to the operator home, and a refusal names "the operator's policy" with no path: `recorded_in` and `context_status`'s `evidence` are `sessions/<id>.ndjson`, and its `policy.source` and `credentials.path` are `policy.json` and `credentials.env`. What a supplier sent is served as received, so a page that quotes the operator home still matches its `content_hash`. The edge does not quote back a tool name, a JSON-RPC method, an `MCP-Protocol-Version` it does not serve, an `Origin` header that is not an origin, or any value from a token refused before its signature is checked. Up to and including 0.4.1, these answers named the operator's full paths to the tenant. An internal corpus kept under the home is still served by its full `file://` URLs, so keep it outside the home; the service's standard error names the token file in full, and an edge served over stdio names each path in full ([session evidence](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/session-evidence.md#source-declarations)).
- A file this edge replaces whole, such as the source policy, the attribution rules, the enrolment record or a cache under the home, is no longer left unsaved when a temporary file a crashed process left behind has the name the save drew, and that leftover file is no longer deleted by the failed save. The save takes another name. If eight names in a row are taken, it refuses, changes nothing, and names the directory whose leftover `.tmp` files to remove.
- `commonmeasure hosted token issue` and `revoke` sync the home directory after writing `hosted-tokens.json`, so a revocation that returned is not lost if the machine crashes in the seconds after. A write that fails no longer leaves `hosted-tokens.json.tmp` behind, and a leftover `hosted-tokens.json.tmp` readable by other users no longer makes `hosted-tokens.json` readable by them when it replaces it. Delete any `hosted-tokens.json.tmp` in the home; the next `issue` or `revoke` makes `hosted-tokens.json` owner-only ([host integration](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/host-integration.md)).
- A hosted edge that cannot write its issuer key cache, `hosted-jwks.json`, leaves the earlier cache whole rather than part-written. The failure is now logged on standard error (`issuer keys held in memory only; the key cache was not written`); the next process still starts from the earlier cache until its first refetch. Verification is unaffected: it uses the keys just fetched ([host integration](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/host-integration.md)).
- These lock files in the home are created readable by their owner only: `policy.lock`, `attribution.lock`, `directories.lock`, `reporting-approvals.lock`, `hosted-service.lock`, `import.lock`, `allowance/ledger.lock`, `instances/.lock`, `relay/spool/delivery.lock` and the lock files under `crawl-delay`. Up to and including 0.4.1, they were created readable by every local user, and one who could reach the home could hold a lock and make every save behind that lock refuse as busy, keep `hosted service` from starting, or stop every import or relay run. Lock files that already exist keep their mode; to narrow one, run `chmod 600` on it or delete it while nothing is running.
- A lock another process holds and a lock file that exists but cannot be opened, such as one a `sudo` run created, now read differently. The busy refusal names the lock file and says another process holds it; the other says to make that lock file readable and writable by this user, or to remove it while no process holds it. The back-off store gave both faults the remedy of making its directory writable, and a crawl-delay turn refused behind a busy lock asked for a writable home ([fail policy](https://github.com/commonmeasure/commonmeasure/blob/main/docs/FAIL-POLICY.md#14-a-hosts-failure-paces-its-next-request)).
- Where `hosted-service.lock` cannot be opened, as when another user runs `status` or `doctor` against a home whose service created it readable by its owner only, they say whether the service is running cannot be read, with the reason; they said `configured, not running`. A session there is still treated as having no automatic delivery, so a source whose licence demands usage reporting is refused ([host integration](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/host-integration.md)).
- The relay's state files in `relay/` (`receipts.json`, `session-agents.json`, `refused-delivered.json` and `skipped-sessions.json`) are each written through a temporary file of their own and are readable by their owner only, whatever mode the file they replace had. Up to and including 0.4.1, they were staged through a fixed `<file>.json.tmp` that two writers could share and whose leftover passed its mode to the file. A leftover of that name is no longer used or removed; delete it.

### Security

- A `relay.json` the relay refuses is named by its receiver's origin alone (scheme, host and port) in load errors, status and reporting rulings, and `status` and `doctor` name the receiver of the last recorded delivery the same way. Up to and including 0.4.1, `commonmeasure enrol` and MCP `context_enrol` showed the `receiver` URL as written, a reporting ruling recorded it in the source record, and `relay/receipts.json` kept it, including any key in its credentials or query; `status`, `doctor` and the console printed it from there where `relay.json` named no receiver, and `doctor` also where it named another ([one receiver](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/telemetry-projection.md#one-receiver)).
- An enrolment whose stored hub URL carries credentials, a query or a fragment is sent nothing, and the hosted edge does not start for it. The refusal, `disconnect`, `connect`'s refusal on an enrolled edge and directory status (`commonmeasure enrol` and MCP `context_enrol`) name the hub by its origin alone. Up to and including 0.4.1, `connect` stored a hub URL with credentials, and the hosted edge published them in its protected resource metadata, which needs no token, and in its 401 answer to any token naming another issuer, which includes every token the hub issued; its start line wrote them to the service journal, and `disconnect` and directory status printed them ([enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/enrolment.md#the-exchange)).
- Host allow and deny rules and access rules now read a host written with a trailing dot (`denied.example.`) as the same host as without it. Up to and including 0.4.1, a URL from the agent, a page link, a redirect target or a named discovery URL could get past a host denial by adding the dot, with no change to the policy file ([source policy](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/source-policy.md)).
- The private-address floor and `record_internal_prefixes` now read a host written with trailing dots (`host.internal.`, `corp.example.`) as the same host; `record_internal_prefixes` and an RSL licence's absolute `<content>` scope now match a URL whatever credentials it carries; and the absolute scope now matches a page whatever its host's case, trailing dots or explicit default port (`:443`, `:80`). Up to and including 0.4.1, a URL from the agent, a page link or a redirect target could, by that spelling alone, be recorded as public supply by observed capture, and where the operator had enabled telemetry egress be sent to the telemetry receiver; or fall outside its licence's scope, so that strict policy admitted a page the licence prohibits and the licence's reporting demand was not ruled on. Neither needed a change to the policy file. A mediated fetch still refused a private name once it resolved to a private address. Where a licence's `<content>` entries overlap, the entry constraining the longer path now governs, an absolute scope governs a relative entry constraining a path of the same length, and a scope that is not a valid URL ranks below every valid scope and every path entry; up to and including 0.4.1 an absolute scope, valid or not, whose whole URL was written longer than a narrower relative entry outranked it, so strict policy could admit a page that entry prohibits, or skip its reporting demand ([session evidence](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/session-evidence.md)). A licence reference sent to the telemetry receiver, including one already queued, no longer carries credentials from the URL of the page it was found for; up to and including 0.4.1 it did ([telemetry projection](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/telemetry-projection.md)). A page URL or licence reference whose credentials are written in a form other than `scheme://user:pass@host` that a URL parser still reads as credentials (`https:/user:pass@host/`, `http:\\user:pass@host/`) is now queued for the receiver without them; up to and including 0.4.1 it was sent with them, and a page URL in that form queued before the upgrade is still sent as queued.
- A session's `edge_identity` record names the hub by its origin alone (scheme, host and port), or `null` where the stored hub URL has none, and the console's Agents view and `GET /api/sessions/<id>` reduce a record written by an earlier version to its origin when they read it; session logs are not rewritten. Up to and including 0.4.1, the record held the hub URL as stored; where 0.4.1's `connect` stored one with credentials, every session record held them, and the Agents view and the session records API showed them. The session records API also reduces a `policy_sync` record's policy URL to its origin, because 0.4.1 built that URL from the stored hub URL where the hub gave a policy path and no policy URL; up to and including 0.4.1 the API served it whole ([session evidence](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/session-evidence.md)).
- Up to and including 0.4.1, `GET /api/sessions/<id>` served a session-log line the console could not parse whole, as `raw`, so a torn `policy_sync` or `edge_identity` line could show a credentialed hub or policy URL to anyone who could reach the console, including under `serve --allow-remote`. Such a line is now served as its line number and length alone ([console](https://github.com/commonmeasure/commonmeasure/blob/main/docs/CONSOLE.md)).
- If 0.4.1's `commonmeasure connect` stored your hub URL with credentials in it, revoke that ingest key on the hub's API keys page if you have not already. Those credentials are in every session log and console index written since, and the console served them to anyone who could reach it, including over the network under `serve --allow-remote`.
- If you signed output through Encypher with a credential in a manifest or supplier URL, rotate that credential. Encypher output signing sends each source reference with any credentials removed (`https://user:key@host/path` is sent as `https://host/path`), in the ingredient assertions and the source record, including a reference that does not parse as a URL but is written `scheme://user:key@host`. From 0.3.5 up to and including 0.4.1, the adapter sent references as recorded to `api.encypher.com`, so credentials in an operator manifest or supplier URL reached Encypher and the credential it signed. The run's own record keeps each reference as recorded, and `commonmeasure provenance --run` does not count a reference's credentials as a difference. The Encypher processor's version and configuration digest change with this fix ([processor contract](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/processor.md#optional-encypher-signing)).
- A managed edge's policy refresh names the policy URL by its origin alone in the reason it records, and refuses a `policy_url` that does not parse with the parser's reason, such as `policy_url is not a URL: invalid port number`. The console withholds a `policy_sync` reason and serves a note in its place where the reason or the record's policy URL holds an `@`, where the policy URL carries a query or does not parse, where the reason holds the record's policy URL verbatim and that URL has a path other than `/`, a query or a fragment, and where the reason of an `unavailable` record holds a `"`. Session logs are not rewritten. From 0.3.1 up to and including 0.4.1, a name resolution that did not finish within the budget (3 seconds at session start) and a refused `policy_url` recorded the policy URL whole, with any credentials in it, in the session log's `policy_sync` reason, the timeout also in the managed state file, and `status --json` and the console showed it: the console's session records API, record page and Agents view, to anyone who could reach it. If a `policy_url` in your `deployment.json` held credentials, revoke that credential on the hub and remove it from the file ([session evidence](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/session-evidence.md#policy-synchronisation)).
- Up to and including 0.4.1, the relay wrote `relay/receipts.json`, `session-agents.json`, `refused-delivered.json` and `skipped-sessions.json` under the umask, so under the usual `022` any local user who could reach the home could read them, and a leftover `<file>.json.tmp` passed its own mode on. `receipts.json` holds the receiver URL last delivered to, with any key in its credentials or query. Each file becomes owner-only at its next write; `receipts.json` at the next delivery attempt. If your receiver URL carries a key and other local users can reach the home, run `chmod 600 ~/.commonmeasure/relay/*.json` (or the same files in your Edge home) now, delete any `relay/*.json.tmp`, and rotate the key.

## 0.4.1 (25 September 2026)

**Upgrading:** 0.4.1 upgrades from 0.4.0 only. It refuses a relay spool that
holds `relay/spool/outbound.ack` or queue lines without an index, which 0.3.4
and earlier wrote, and sends nothing from it. On such a home:

1. Stop every relay, including `commonmeasure hosted service`
   ([one release per Edge home](https://github.com/commonmeasure/commonmeasure/blob/main/ARCHITECTURE.md#what-runs-where)).
2. Upgrade.
3. Run `commonmeasure status` and read its `refused spool` line
   (`egress.refused_spool` with `--json`): how many indexed batches are still
   queued or dead, and whether that count is complete.
4. Move `relay/spool` aside, keeping it and `relay/delivered.idx`.
5. Run `commonmeasure relay` without `--session`, or restart the hosted
   service, whose interval run does the same. It projects outstanding events
   again from every session log under `sessions/`, under the approvals and
   policy in force, and queues no event that `relay/delivered.idx` records as
   accepted. The one exception is an event resent under its own id to carry a
   session's changed refused count. Pass `--run` again for each published run
   whose events were outstanding.

An outstanding event whose session log or run input is gone is not projected
again: it stays in the saved spool, undelivered
([delivery state](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/telemetry-projection.md#delivery-state)).

### Added

- Reported telemetry now includes `data.commonmeasure-crawl-delay` on mediated retrievals with a recorded, honoured `Crawl-delay` above zero, capped at 60 seconds; it is absent when that evidence is missing ([telemetry projection](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/telemetry-projection.md)).

### Changed

- The edge reports its release with every directory proof, and the hub leaves a key unlisted when the last accepted upload reported no release or one below its floor. Upgrade the edge to at least the hub's floor: the key stays unlisted until that edge uploads a proof. After an upgrade, the edge uploads on its next relay run; a failed upload due only to the release change waits an hour before another attempt ([enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/enrolment.md)).
- Failing hosts put later mediated requests into back-off, shared across sessions and restarts on one edge: 429 and 503 honour a usable `Retry-After` up to one hour; other 5xx responses, and 429 or 503 without a usable header, start at 10 seconds and double for consecutive failure periods up to 15 minutes. Failed requests are never resent automatically; later requests wait within their remaining budget or are refused, and one sender goes first after back-off ends ([failure policy](https://github.com/commonmeasure/commonmeasure/blob/main/docs/FAIL-POLICY.md)).
- Hosted back-off refusals say only that the host is in back-off, except that they may give the end when the host supplied it as an uncapped `Retry-After` HTTP-date; other tenants' response status, failure count and timing remain private ([session evidence](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/session-evidence.md)).

### Removed

- The relay no longer imports acknowledgements from `outbound.ack` or reads
  a queue line by its position. It refuses such a spool and names the file
  or line and the remedy; `status` and `doctor` say how many batches in its
  indexed lines are still queued or dead, and whether that count is complete
  ([delivery state](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/telemetry-projection.md#delivery-state)).
- A queued batch without `directory_selection` is no longer sent as if no
  directory selection applied: the line is reported as damaged. An
  undelivered batch that 0.3.4 or earlier queued is held and never sent
  ([directory enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/directory-enrolment.md)).
- `doctor` no longer reads a `relay/receipts.json` that names no accepting
  receiver as a delivery to the configured one; it says the file is from
  0.3.4 or earlier and that the next delivery rewrites it.
- The fleet-status classifier no longer compares an edge that reports no
  envelope digest (0.3.0) on its loader digest; it reads as `unknown`
  ([fleet status](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/fleet-status.md#what-a-receiver-concludes)).

### Fixed

- Directory-proof failures retain non-JSON error bodies up to 2 KiB on a character boundary, with a truncation marker giving the original byte count; JSON `detail` and `message` strings remain whole ([enrolment](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/enrolment.md)).
- Telemetry receivers can acknowledge a batch with any `2xx` response, including an empty body; when the response supplies no valid count of newly recorded events, the relay reports that count as unknown ([receiving reports](https://github.com/commonmeasure/commonmeasure/blob/main/docs/integrate/reports.md)).
- A relay run uses the reporting approvals it has just refreshed, so an approval renewed after expiry allows reporting during that run without a separate enrolment sync ([telemetry projection](https://github.com/commonmeasure/commonmeasure/blob/main/docs/contracts/telemetry-projection.md)).

## 0.4.0 (23 September 2026)

`robots.txt` now binds in every policy mode. An operator who ran `observe`
or `prefer` and relied on the edge fetching a disallowed page will see that
crossing refused.

### robots.txt

- A `Disallow` addressed to `CommonMeasureBot`, or to `*` where no group
  names it, refuses the crossing before the page is fetched in every policy
  mode: `strict`, `observe` and `prefer` alike. In earlier releases
  `observe` and `prefer` fetched the page and recorded the rule.
- A `robots.txt` that cannot be read follows RFC 9309 §2.3.1 in every mode.
  A 4xx other than 429 means no rules. A 429, a 5xx, a timeout, or a name,
  TLS or connection failure means unreachable: the last answer the host
  gave rules, however old; with none held, the crossing is refused as a
  complete disallow. A failed probe never replaces the last answer held.
- A `robots.txt` larger than 512 KiB is parsed up to its last complete line
  within that bound (RFC 9309 §2.5), and the record carries `truncated` with
  the file's size and the bytes read. Earlier releases discarded an
  oversized file and fetched with no rules, so padding the file switched its
  `Disallow` off.
- A `robots.txt` redirect the edge declines to follow (to an address it does
  not mediate, to the hub's origin, or to a host the operator's policy
  refuses) counts as unreachable. More than five redirects is unreachable.
- A robots refusal names the source's `robots.txt`, not the operator's
  policy file. The record carries the address of the file that ruled, so a
  redirected `robots.txt` is named by where it was read from.
- A `robots.txt` request that the edge cut short (the call's time limit left
  no time for it, or it could not be signed) refuses the crossing in every
  mode and is not held as the host's failure.

## Earlier releases

- **0.3.5** (22 September 2026): registered network instances with a
  delivery duty, hub-released supplier credentials, a hosted MCP transport,
  Agents and Compare views in the console, and a publisher's reporting
  demand, `Crawl-delay` and licence honoured before a page is read.
- **0.3.4** (16 September 2026): `commonmeasure disconnect` works against a
  hosted hub.
- **0.3.3** (15 September 2026): project-directory reporting consent, and
  source record, budget and policy views in the console.
- **0.3.2** (14 September 2026): managed-policy synchronisation and
  session-file path fixes, and compatibility with more agent hosts.
- **0.3.1** (14 September 2026): three more hosts registered by the binary,
  one-command hub enrolment for a managed machine, and strict policy's
  handling of personal data found on a public page.
- **0.3.0** (13 September 2026): the first public release, downloadable and
  usable without an account.
- **0.2.0** (6 September 2026): tagged before the first public release.
