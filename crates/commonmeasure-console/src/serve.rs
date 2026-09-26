//! The operator console over loopback HTTP.
//!
//! One binary serves everything: the maud-rendered console pages, the session
//! detail fragment htmx loads into the Record view, and the JSON API over the
//! telemetry index. The index is refreshed incrementally before every answer —
//! the reader's request is the poll, so there is no watcher thread to fall
//! behind and no answer served from a staler index than the logs on disk.
//!
//! Loopback by intent: this console is the operator reading their own record,
//! and nothing here authenticates. The binding enforces that rather than
//! assuming it — a non-loopback `--listen` is refused unless the operator
//! passes `--allow-remote`; a request naming a foreign `Host` is refused, so a
//! public page cannot reach a loopback console by pointing its own name at
//! 127.0.0.1; and a write carrying a foreign `Origin` is refused, so a page
//! that simply knows the port cannot post a form at it either.
//!
//! The console's writes are `POST /app/compare`, a query the operator submits
//! and the injected [`SearchRunner`] runs through governed acquisition for the
//! selected providers, retaining local comparison evidence; and the three declaration edits the Policy screen
//! offers, `POST /app/policy/mode`, `POST /app/policy/deny` and
//! `POST /app/attribution`, each revision-checked and saved through the
//! artefact's own loader (`console::edit`). `GET /app/policy/forecast` is the
//! dry run over recorded history for a draft rule and writes nothing.
//! `GET /app/agents` and `/api/agents` read the host sessions, with liveness
//! from the injected [`LivenessProbe`]. Every
//! write accepts form or JSON input and answers JSON when requested, HTML
//! otherwise. `/api/` aliases always answer JSON. Policy answers carry the
//! outcome and revision from a fresh read, including where nothing was saved.

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use commonmeasure_http::{Request, Response, Server};
use serde_json::{Value, json};

use crate::attribution::Attribution;
use crate::console;
use crate::console::form::query_param;
use crate::store::Store;

const STYLES_CSS: &str = include_str!("../../../console/styles.css");
const HTMX_JS: &str = include_str!("../../../console/htmx.min.js");

/// Executes an explicit comparison through the caller's governed acquisition
/// path and returns its durable local record. The console holds no credentials.
pub type SearchRunner = std::sync::Arc<
    dyn Fn(&commonmeasure_harness::compare::CompareRequest) -> Result<Value, String> + Send + Sync,
>;

pub use crate::console::agents::LivenessProbe;

pub struct ServeOptions {
    /// `host:port`; port 0 lets the OS choose and the bound address is printed.
    pub listen: String,
    /// Permit a `listen` that is reachable from off this machine. Refused
    /// without it: the console carries no credential, so a non-loopback bind
    /// publishes the whole evidence record and the write routes: the Compare
    /// query, the policy mode, a denied host and the attribution rules.
    pub allow_remote: bool,
    /// The Common Measure home holding `sessions/` and the index database.
    pub home: PathBuf,
    /// The content providers and whether each is connected, computed by the
    /// caller from the operator's own credentials. The sink never holds a key;
    /// each entry is `{name, connected}`, for the Sources screen.
    pub providers: Vec<Value>,
    /// Runs a live comparison search for the Compare screen. `None` disables
    /// it; the screen says so. Making real, credit-spending calls is why this
    /// is injected rather than built into the sink.
    pub search: Option<SearchRunner>,
    /// Answers whether a recorded host process is in the process table now,
    /// for the Agents screen. `None` when the caller wired no probe, which
    /// the screen states as liveness unavailable. Injected because reading
    /// the process table is the harness's, not the console's.
    pub liveness: Option<LivenessProbe>,
}

/// Serve until the process is stopped.
pub fn serve(options: ServeOptions) -> Result<()> {
    start(options)?.wait();
    Ok(())
}

/// Bind and serve on a background thread, returning the handle, whose
/// address a caller that asked for port 0 reads. [`serve`] is this and a
/// wait; a test that injects a probe starts the console here.
pub fn start(options: ServeOptions) -> Result<commonmeasure_http::ServerHandle> {
    let store = Store::open(&options.home.join("telemetry.db"))?;
    let home = options.home.clone();
    let sessions_dir = options.home.join("sessions");
    let state = Mutex::new(store);

    // Checked before the socket exists, so a refused exposure never listens
    // for even one request.
    let exposed = exposed_addresses(&options.listen)?;
    if !exposed.is_empty() && !options.allow_remote {
        bail!(
            "--listen {} is reachable from outside this machine ({}). This console carries no \
             credential of any kind, so that publishes every recorded URL, host, cwd and session, \
             and the policy and attribution write routes, to anyone who can reach the address. \
             Bind a loopback address, or pass --allow-remote to do this deliberately.",
            options.listen,
            exposed
                .iter()
                .map(SocketAddr::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        );
    }

    let server =
        Server::bind(&options.listen).with_context(|| format!("bind {}", options.listen))?;
    let address = server.local_addr()?;
    if !exposed.is_empty() {
        eprintln!(
            "warning: {address} is reachable from outside this machine and nothing here \
             authenticates. Every recorded URL, host, cwd and session is readable, and the \
             attribution rules, the declared policy mode and its denied hosts are writable, by \
             anyone who can reach it."
        );
    }
    // Parsed by tests and read by people; printed before serving so a caller
    // that started us with port 0 can learn where we landed.
    println!("listening on http://{address}");
    println!("evidence   {}", sessions_dir.display());
    std::io::stdout().flush().ok();

    let providers = options.providers.clone();
    let search = options.search.clone();
    let liveness = options.liveness.clone();
    let guard = HostGuard {
        address,
        allow_remote: options.allow_remote,
    };
    let handle = server.spawn(move |request| {
        let injected = Injected {
            providers: &providers,
            search: search.as_ref(),
            liveness: liveness.as_ref(),
        };
        route(&state, &home, &sessions_dir, &injected, &guard, &request)
    })?;
    Ok(handle)
}

/// The addresses `listen` resolves to that are not loopback. Empty means the
/// console will only be reachable from this machine.
pub fn exposed_addresses(listen: &str) -> Result<Vec<SocketAddr>> {
    use std::net::ToSocketAddrs;
    let resolved: Vec<SocketAddr> = listen
        .to_socket_addrs()
        .with_context(|| format!("resolve listen address {listen}"))?
        .collect();
    if resolved.is_empty() {
        bail!("listen address {listen} resolved to no address");
    }
    Ok(resolved
        .into_iter()
        .filter(|address| !address.ip().is_loopback())
        .collect())
}

/// What the console answers to. A loopback bind is only private if the name in
/// the request is a loopback name as well: a page anywhere on the internet can
/// point its own hostname at 127.0.0.1 and script a browser into reading this
/// console, and the `Host` header is the only thing that separates that from
/// the operator's own tab.
struct HostGuard {
    address: SocketAddr,
    allow_remote: bool,
}

impl HostGuard {
    fn permits(&self, host: Option<&str>) -> bool {
        // `--allow-remote` is about the address this console binds, not the
        // names it answers to. On a loopback bind the operator asked for no
        // reach at all, so the name guard stays on: the flag is not a way to
        // let a public page rebind its own hostname onto 127.0.0.1.
        if self.allow_remote && !self.address.ip().is_loopback() {
            return true;
        }
        // No Host at all is HTTP/1.0 or a hand-rolled client, never a browser
        // under rebinding — there is no forged name to refuse.
        let Some(host) = host else { return true };
        let (name, port) = split_host(host);
        if port.is_some_and(|port| port != self.address.port()) {
            return false;
        }
        matches!(name, "localhost" | "127.0.0.1" | "::1") || name == self.address.ip().to_string()
    }

    /// Whether a state-changing request may proceed, given its `Origin` and
    /// `Sec-Fetch-Site`.
    ///
    /// The `Host` guard stops DNS rebinding, but a public page never needed to
    /// rebind: it can post a form straight at `localhost` and the browser
    /// sends the very `Host` this console accepts, so the write lands. Only
    /// the browser's own account of where the request came from separates the
    /// operator's tab from a page that merely knows the port. `Sec-Fetch-Site:
    /// same-origin` is that account where the browser sends it, and no page
    /// can forge it. Otherwise `Origin` decides: a browser sets it on every
    /// cross-origin write, and its absence means the request came from no page
    /// at all. An `Origin` of `null` is what a sandboxed or opaque-origin page
    /// sends, so it is refused unless the fetch-site header vouches for it.
    fn permits_origin(&self, origin: Option<&str>, fetch_site: Option<&str>) -> bool {
        if fetch_site.is_some_and(|site| site.trim().eq_ignore_ascii_case("same-origin")) {
            return true;
        }
        let Some(origin) = origin else { return true };
        // `--allow-remote` is about the address the console binds, so it lifts
        // this check only for a bind that is genuinely remote. On loopback the
        // flag leaves the cross-origin boundary where it was: a page the
        // operator happens to visit is not one of this console's own.
        if self.allow_remote && !self.address.ip().is_loopback() {
            return true;
        }
        let Some(rest) = origin.trim().strip_prefix("http://") else {
            return false;
        };
        let (name, port) = split_host(rest);
        port.unwrap_or(80) == self.address.port()
            && (matches!(name, "localhost" | "127.0.0.1" | "::1")
                || name == self.address.ip().to_string())
    }
}

/// A `Host` value as (name, port). IPv6 literals carry their own brackets.
///
/// Text after the name that is not a port makes the whole value the name, so
/// the comparison refuses it. Dropping it instead compared a prefix and let
/// `127.0.0.1:99999`, `localhost:abc` and `[localhost]evil.example` through
/// the port check the name check is paired with.
fn split_host(host: &str) -> (&str, Option<u16>) {
    if let Some(rest) = host.strip_prefix('[') {
        let Some((name, tail)) = rest.split_once(']') else {
            return (host, None);
        };
        if tail.is_empty() {
            return (name, None);
        }
        return match tail.strip_prefix(':').and_then(|port| port.parse().ok()) {
            Some(port) => (name, Some(port)),
            None => (host, None),
        };
    }
    match host.rsplit_once(':') {
        Some((name, port)) if !port.is_empty() => match port.parse() {
            Ok(port) => (name, Some(port)),
            Err(_) => (host, None),
        },
        _ => (host, None),
    }
}

/// The id a path segment names, percent-decoded.
///
/// Plan ids come from a published summary and session ids are transcript file
/// stems, so neither is URL-safe by construction: a plan called `exa+brave mix`
/// or a session file `s 1.ndjson` is ordinary. The console's own links encode
/// them with [`console::form::encode_component`], and reading them back is the
/// other half of that — without it the page listed an item whose own link
/// answered that no such item existed.
fn path_id(segment: &str) -> std::result::Result<String, String> {
    console::form::percent_decode(segment).map_err(|error| format!("{error:#}"))
}

/// What the caller injected through [`ServeOptions`], passed to the router
/// as one value.
struct Injected<'a> {
    providers: &'a [Value],
    search: Option<&'a SearchRunner>,
    liveness: Option<&'a LivenessProbe>,
}

fn route(
    state: &Mutex<Store>,
    home: &std::path::Path,
    sessions_dir: &std::path::Path,
    injected: &Injected<'_>,
    guard: &HostGuard,
    request: &Request,
) -> Response {
    let Injected {
        providers,
        search,
        liveness,
    } = *injected;
    if !guard.permits(request.headers.get("Host")) {
        return json_error(
            403,
            "this console answers only to the loopback name it was bound to",
        );
    }
    let path = request.target.split('?').next().unwrap_or("/");
    if request.method == "POST" {
        if !guard.permits_origin(
            request.headers.get("Origin"),
            request.headers.get("Sec-Fetch-Site"),
        ) {
            return write_refusal(
                home,
                request,
                403,
                &format!(
                    "this console takes writes only from its own pages; a write with Origin \
                     {} was refused",
                    request.headers.get("Origin").unwrap_or("absent")
                ),
            );
        }
        return match path {
            "/app/compare" | "/api/compare" => post_compare(search, request, providers, home),
            "/app/policy/mode" | "/app/policy/deny" | "/app/attribution" | "/api/policy/mode"
            | "/api/policy/deny" | "/api/attribution" => {
                policy_write(state, home, sessions_dir, path, request)
            }
            _ => json_error(
                405,
                "the console's writes are a comparison query, the policy mode, a denied host \
                 and the attribution rules; nothing here executes anything else",
            ),
        };
    }
    if request.method != "GET" {
        return json_error(
            405,
            "the console edits the attribution rules, the declared policy mode and its denied \
             hosts; nothing here executes",
        );
    }
    // The engagement filter, shared by the aggregate routes. The rules are
    // re-read per answer, like the index itself: an edited rule re-attributes
    // on the next read, and a malformed rule file is an error the reader sees
    // rather than a console that quietly shows everything unattributed.
    let engagement = query_param(&request.target, "engagement");
    // The Record screen renders one session into its pane when a rail link
    // is followed as a page, so the pane is reachable without a script.
    let session = query_param(&request.target, "session");
    let attribution = || Attribution::load(home);
    match path {
        // The console: a maud app shell routed by section, each view
        // bookmarkable. `/` lands on the Overview.
        "/" | "/index.html" | "/app" => app_page(
            state,
            home,
            sessions_dir,
            engagement.as_deref(),
            console::app::Section::Overview,
            providers,
            session.as_deref(),
        ),
        "/app/record" => app_page(
            state,
            home,
            sessions_dir,
            engagement.as_deref(),
            console::app::Section::Record,
            providers,
            session.as_deref(),
        ),
        "/app/policy" => app_page(
            state,
            home,
            sessions_dir,
            engagement.as_deref(),
            console::app::Section::Policy,
            providers,
            session.as_deref(),
        ),
        "/app/sources" => app_page(
            state,
            home,
            sessions_dir,
            engagement.as_deref(),
            console::app::Section::Sources,
            providers,
            session.as_deref(),
        ),
        "/api/compare/export" => export_compare(home, request),
        "/app/compare" | "/api/compare" => get_compare(home, request, providers, search.is_some()),
        // The Agents screen, its JSON twin and its detail fragment, all
        // rendered from one projection (`console::agents::projection`).
        "/app/agents" | "/api/agents" => agents_route(
            state,
            home,
            sessions_dir,
            liveness,
            AgentsView::of(path, query_param(&request.target, "agent")),
        ),
        _ if path.starts_with("/app/fragments/agent/") => {
            match path_id(&path["/app/fragments/agent/".len()..]) {
                Ok(id) if !id.is_empty() => agents_route(
                    state,
                    home,
                    sessions_dir,
                    liveness,
                    AgentsView::Fragment(id),
                ),
                Ok(_) => json_error(404, "no such route"),
                Err(error) => html_error(400, &error),
            }
        }
        "/app/policy/forecast" | "/api/policy/forecast" => {
            forecast_route(state, home, sessions_dir, request)
        }
        "/api/providers" => json_value(200, &json!(providers)),
        // Read by `commonmeasure service status` to tell a console still
        // running an old binary from one running the binary on PATH.
        "/api/version" => json_value(
            200,
            &json!({
                "product": "commonmeasure",
                "version": env!("CARGO_PKG_VERSION"),
                "pid": std::process::id(),
            }),
        ),
        "/app/budget" => app_page(
            state,
            home,
            sessions_dir,
            engagement.as_deref(),
            console::app::Section::Budget,
            providers,
            None,
        ),
        "/favicon.svg" => asset(
            include_str!("../../../console/brand/favicon.svg"),
            "image/svg+xml",
        ),
        "/brand/edge.svg" => asset(
            include_str!("../../../console/brand/edge.svg"),
            "image/svg+xml",
        ),
        "/brand/edge-light.svg" => asset(
            include_str!("../../../console/brand/edge-light.svg"),
            "image/svg+xml",
        ),
        "/fonts/Geist-Variable.woff2" => binary_asset(
            include_bytes!("../../../console/fonts/Geist-Variable.woff2"),
            "font/woff2",
        ),
        "/fonts/GeistMono-Variable.woff2" => binary_asset(
            include_bytes!("../../../console/fonts/GeistMono-Variable.woff2"),
            "font/woff2",
        ),
        "/styles.css" => asset(STYLES_CSS, "text/css; charset=utf-8"),
        "/theme.js" => asset(
            include_str!("../../../console/theme.js"),
            "text/javascript; charset=utf-8",
        ),
        "/htmx.min.js" => asset(HTMX_JS, "text/javascript; charset=utf-8"),
        "/guide" => guide_page("context-window-optimisation"),
        "/guide/state-of-the-evidence" => guide_page("state-of-the-evidence"),
        "/guide/untrusted-context" => guide_page("untrusted-context"),
        "/sessions" | "/sessions.html" => app_page(
            state,
            home,
            sessions_dir,
            engagement.as_deref(),
            console::app::Section::Record,
            providers,
            session.as_deref(),
        ),
        // The egress block is read fresh per answer, so a delivery that
        // happened after the console started is reported, not staled over.
        "/api/status" => {
            let egress = commonmeasure_relay::egress_report(home);
            match attribution() {
                Ok(rules) => with_store(state, sessions_dir, move |store| {
                    store
                        .status(egress, &rules, engagement.as_deref())
                        .map(|status| (200, status))
                }),
                Err(error) => json_error(500, &format!("{error:#}")),
            }
        }
        // The same projection the Budget screen renders, verbatim. The policy
        // projection is built unfiltered and the budget projection narrows it,
        // so the cap in this answer is the cap the policy panel resolved.
        "/api/budget" => match attribution() {
            Ok(rules) => {
                let home = home.to_path_buf();
                with_store(state, sessions_dir, move |store| {
                    Ok((
                        200,
                        budget_projection(store, &home, &rules, engagement.as_deref())?,
                    ))
                })
            }
            Err(error) => json_error(500, &format!("{error:#}")),
        },
        // The same projection the policy panel renders, verbatim: the HTML
        // and the JSON views cannot disagree.
        "/api/policy" => match attribution() {
            Ok(rules) => {
                let home = home.to_path_buf();
                with_store(state, sessions_dir, move |store| {
                    let facts = store.cwd_facts()?;
                    Ok((
                        200,
                        console::policy::projection(&home, &facts, &rules, engagement.as_deref()),
                    ))
                })
            }
            Err(error) => json_error(500, &format!("{error:#}")),
        },
        "/api/sessions" => match attribution() {
            Ok(rules) => with_store(state, sessions_dir, move |store| {
                store
                    .sessions(&rules, engagement.as_deref())
                    .map(|sessions| (200, sessions))
            }),
            Err(error) => json_error(500, &format!("{error:#}")),
        },
        "/api/content" => match attribution() {
            Ok(rules) => with_store(state, sessions_dir, move |store| {
                store
                    .content(console::CONTENT_CAP, &rules, engagement.as_deref())
                    .map(|content| (200, content))
            }),
            Err(error) => json_error(500, &format!("{error:#}")),
        },
        // The same rows the internal-use panel renders, verbatim: the HTML
        // and the JSON views cannot disagree.
        "/api/internal" => match attribution() {
            Ok(rules) => with_store(state, sessions_dir, move |store| {
                store
                    .internal_use(console::INTERNAL_CAP, &rules, engagement.as_deref())
                    .map(|internal| (200, internal))
            }),
            Err(error) => json_error(500, &format!("{error:#}")),
        },
        _ => {
            // The console's detail pane: one session's crossings,
            // loaded into the Record screen when a rail row is chosen.
            if let Some(id) = path.strip_prefix("/app/fragments/session/") {
                let id = match path_id(id) {
                    Ok(id) if !id.is_empty() => id,
                    Ok(_) => return json_error(404, "no such route"),
                    Err(error) => return html_error(400, &error),
                };
                return with_store_html(state, sessions_dir, move |store| {
                    store.session_records(&id).map(|records| {
                        let records = records.unwrap_or_else(
                            || json!({"error": format!("no session {id} in the index")}),
                        );
                        console::app::session_detail(&id, &records)
                    })
                });
            }
            match path.strip_prefix("/api/sessions/") {
                Some(id) if !id.is_empty() => {
                    let id = match path_id(id) {
                        Ok(id) => id,
                        Err(error) => return json_error(400, &error),
                    };
                    with_store(state, sessions_dir, move |store| {
                        store.session_records(&id).map(|records| match records {
                            Some(records) => (200, records),
                            // The HTML fragment answers 200 so htmx swaps the
                            // reason into the page. The API has no such reason:
                            // a client reading the status must not record an
                            // error object as the session it asked for.
                            None => (
                                404,
                                json!({"error": format!("no session {id} in the index")}),
                            ),
                        })
                    })
                }
                _ if path.starts_with("/api/") => json_error(404, "no such route"),
                // A person who followed a bad link reads a page; a client
                // that asked the API reads JSON.
                _ => html_error(404, "no such route"),
            }
        }
    }
}

/// Which representation of the agents projection a request asked for.
enum AgentsView {
    Json,
    Page(Option<String>),
    Fragment(String),
}

impl AgentsView {
    fn of(path: &str, agent: Option<String>) -> Self {
        if path.starts_with("/api/") {
            Self::Json
        } else {
            Self::Page(agent)
        }
    }
}

/// The Agents screen, `/api/agents` and the detail fragment. The projection
/// is built once per answer from the store's host sessions, the Policy
/// screen's unfiltered projection, the relay's egress report and the
/// injected probe, so the three representations cannot disagree. The
/// policy projection needs attribution rules only for engagement names,
/// which this screen does not show, so a malformed rule file does not take
/// the screen down.
fn agents_route(
    state: &Mutex<Store>,
    home: &std::path::Path,
    sessions_dir: &std::path::Path,
    liveness: Option<&LivenessProbe>,
    view: AgentsView,
) -> Response {
    let rules = Attribution::read(home)
        .and_then(|snapshot| snapshot.rules)
        .unwrap_or_else(|_| Attribution::none());
    let egress = commonmeasure_relay::egress_report(home);
    let build = |store: &mut Store| -> Result<Value> {
        let facts = store.cwd_facts()?;
        let policy = console::policy::projection(home, &facts, &rules, None);
        Ok(console::agents::projection(
            &store.host_sessions()?,
            &policy,
            &egress,
            liveness,
            chrono::Utc::now(),
        ))
    };
    match view {
        AgentsView::Json => with_store(state, sessions_dir, |store| Ok((200, build(store)?))),
        AgentsView::Page(agent) => {
            let sessions_path = sessions_dir.display().to_string();
            let at = read_time();
            with_store_html(state, sessions_dir, |store| {
                Ok(console::agents::page(
                    &build(store)?,
                    agent.as_deref(),
                    console::app::ReadFrom {
                        sessions: &sessions_path,
                        at: &at,
                    },
                ))
            })
        }
        AgentsView::Fragment(id) => with_store_html(state, sessions_dir, |store| {
            Ok(console::agents::fragment(&build(store)?, &id))
        }),
    }
}

/// API paths serve JSON; page paths negotiate an explicit JSON Accept value.
fn negotiated_error(request: &Request, status: u16, detail: &str) -> Response {
    if wants_json(request) {
        json_error(status, detail)
    } else {
        html_error(status, detail)
    }
}

fn wants_json(request: &Request) -> bool {
    request.target.starts_with("/api/")
        || request.headers.get("Accept").is_some_and(|accept| {
            accept.split(',').any(|entry| {
                let mut parts = entry.split(';');
                parts
                    .next()
                    .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
                    && !parts.any(|part| {
                        part.trim()
                            .strip_prefix("q=")
                            .is_some_and(|q| q.parse::<f32>().unwrap_or(0.0) <= 0.0)
                    })
            })
        })
}

/// Decode either representation into the editor's ordered fields. Attribution
/// JSON uses a rules array because order determines which rule wins.
fn request_fields(request: &Request) -> Result<Vec<(String, String)>> {
    if !request.headers.get("Content-Type").is_some_and(|kind| {
        kind.split(';')
            .next()
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
    }) {
        return console::form::parse_form(&request.body);
    }
    let value: Value = serde_json::from_slice(&request.body)?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("expected a JSON object"))?;
    let mut fields = Vec::new();
    for (key, value) in object {
        if key == "providers" {
            for provider in value
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("providers must be an array"))?
            {
                fields.push((
                    "provider".to_owned(),
                    provider
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("providers must contain strings"))?
                        .to_owned(),
                ));
            }
        } else if key == "rules" {
            for rule in value
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("rules must be an array"))?
            {
                for key in ["match", "engagement"] {
                    let text = rule
                        .get(key)
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("each rule needs a string {key}"))?;
                    fields.push((key.to_owned(), text.to_owned()));
                }
            }
        } else {
            let text = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{key} must be a string"))?;
            fields.push((key.clone(), text.to_owned()));
        }
    }
    Ok(fields)
}

fn write_revision(home: &std::path::Path, request: &Request) -> Value {
    if request
        .target
        .split('?')
        .next()
        .is_some_and(|path| path.ends_with("/attribution"))
    {
        console::policy::attribution_editor(home)["revision"].clone()
    } else if request.target.contains("/policy/") {
        commonmeasure_harness::policy::PolicyDocument::read(home)
            .map(|document| json!(document.revision()))
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    }
}

fn write_refusal(home: &std::path::Path, request: &Request, status: u16, notice: &str) -> Response {
    if wants_json(request) {
        json_value(
            status,
            &json!({"kind": "refused", "status": status, "notice": notice,
            "revision": write_revision(home, request)}),
        )
    } else {
        html_error(status, notice)
    }
}

/// Identify the inspected home and the policy for the next comparison.
fn comparison_context(home: &std::path::Path, result: &mut Value, available: bool) {
    result["revision"] = Value::Null;
    result["home"] = json!(home);
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.display().to_string());
    result["next_cwd"] = json!(cwd);
    result["next_policy"] = match commonmeasure_harness::policy::SessionPolicy::load(
        home,
        cwd.as_deref(),
    ) {
        Ok(policy) => {
            json!({"mode": policy.mode(), "principal": policy.principal(), "scope": policy.scope()})
        }
        Err(_) => {
            json!({"error": "Policy unavailable; correct this Edge home's policy before comparing."})
        }
    };
    result["runner_available"] = json!(available);
    match commonmeasure_harness::compare::list(home) {
        Ok(ids) => result["history"] = json!(ids),
        Err(_) => result["history_error"] = json!("Local comparison history could not be read."),
    }
}

fn export_compare(home: &std::path::Path, request: &Request) -> Response {
    let Some(id) = query_param(&request.target, "comparison") else {
        return json_error(400, "Name a retained comparison to export.");
    };
    let include_query = match query_param(&request.target, "include_query").as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        _ => return json_error(400, "include_query must be true or false."),
    };
    let record = match commonmeasure_harness::compare::read(home, &id) {
        Ok(record) => record,
        Err(_) => return json_error(404, "Comparison record unavailable."),
    };
    match crate::compare_export::archive(&record, include_query) {
        Ok(bytes) => {
            let mut response = binary_asset(&bytes, "application/zip");
            response.headers.set(
                "Content-Disposition",
                "attachment; filename=commonmeasure-comparison.zip",
            );
            response.headers.set("Cache-Control", "no-store");
            response
        }
        Err(error) => json_error(422, &error),
    }
}

fn get_compare(
    home: &std::path::Path,
    request: &Request,
    providers: &[Value],
    available: bool,
) -> Response {
    let mut result = if let Some(id) = query_param(&request.target, "comparison") {
        match commonmeasure_harness::compare::read(home, &id) {
            Ok(value) => value,
            Err(_) => {
                json!({"kind": "unavailable", "status": 404, "notice": "Comparison record unavailable."})
            }
        }
    } else {
        json!({"status": 200})
    };
    comparison_context(home, &mut result, available);
    compare_answer(request, &result, providers)
}

fn post_compare(
    search: Option<&SearchRunner>,
    request: &Request,
    providers: &[Value],
    home: &std::path::Path,
) -> Response {
    let parsed = request_fields(request)
        .map_err(|e| e.to_string())
        .and_then(|fields| {
            let request = commonmeasure_harness::compare::CompareRequest {
                query: fields
                    .iter()
                    .find(|(key, _)| key == "query")
                    .map(|(_, v)| v.trim().to_owned())
                    .unwrap_or_default(),
                providers: fields
                    .iter()
                    .filter(|(key, _)| key == "provider")
                    .map(|(_, v)| v.clone())
                    .collect(),
            };
            request.validate()?;
            Ok(request)
        });
    let mut result = match parsed {
        Err(reason) => json!({"kind": "refused", "status": 400, "notice": reason}),
        Ok(query) => match search {
            None => {
                json!({"kind": "unavailable", "status": 503, "notice": "No comparison runner is connected; no query was sent."})
            }
            Some(runner) => match runner(&query) {
                Ok(value) => value,
                Err(reason) => {
                    json!({"kind": "unavailable", "status": 503, "notice": reason, "query": query.query})
                }
            },
        },
    };
    if !wants_json(request)
        && let Some(id) = result["comparison_id"].as_str()
    {
        // Refreshing the result page must never repeat a paid POST.
        let mut response = json_value(303, &json!({}));
        response
            .headers
            .set("Location", &format!("/app/compare?comparison={id}"));
        return response;
    }
    comparison_context(home, &mut result, search.is_some());
    compare_answer(request, &result, providers)
}

fn compare_answer(request: &Request, result: &Value, providers: &[Value]) -> Response {
    let status = result["status"].as_u64().unwrap_or(500) as u16;
    if wants_json(request) {
        json_value(status, result)
    } else {
        html(
            status,
            &console::app::compare_answer_page(result, providers),
        )
    }
}

fn budget_projection(
    store: &Store,
    home: &std::path::Path,
    rules: &Attribution,
    engagement: Option<&str>,
) -> Result<Value> {
    let facts = store.cwd_facts()?;
    let policy = console::policy::projection(home, &facts, rules, None);
    let budgets = store.engagement_budgets(rules)?;
    Ok(console::budget::projection(
        &policy,
        &budgets,
        &console::budget::allowance_state(home),
        engagement,
    ))
}

/// The rebuilt console (maud app shell), rendered for one section. Reads the
/// same status and content projections the old console does — presentation is
/// all that changed. A malformed rule file is stated rather than shown as a
/// console that silently attributes everything to nothing.
fn app_page(
    state: &Mutex<Store>,
    home: &std::path::Path,
    sessions_dir: &std::path::Path,
    engagement: Option<&str>,
    section: console::app::Section,
    providers: &[Value],
    session: Option<&str>,
) -> Response {
    use console::app::Section;
    // The Policy screen carries the editor that repairs a broken rule file,
    // so it renders with no attribution rather than refusing to render.
    if section == Section::Policy {
        return match policy_screen(state, home, sessions_dir, None, false) {
            Ok(markup) => html(200, &markup),
            Err(error) => html_error(500, &format!("{error:#}")),
        };
    }
    match Attribution::read(home).and_then(|snapshot| snapshot.rules) {
        Ok(rules) => {
            let engagement = engagement.map(str::to_owned);
            let home = home.to_path_buf();
            let egress = commonmeasure_relay::egress_report(&home);
            let providers = providers.to_vec();
            let session = session.map(str::to_owned);
            // What the screen was read from and when, stated on the screen.
            let sessions_path = sessions_dir.display().to_string();
            let at = read_time();
            with_store_html(state, sessions_dir, move |store| {
                let read = console::app::ReadFrom {
                    sessions: &sessions_path,
                    at: &at,
                };
                Ok(match section {
                    Section::Overview => {
                        let status = store.status(egress, &rules, engagement.as_deref())?;
                        let content =
                            store.content(console::CONTENT_CAP, &rules, engagement.as_deref())?;
                        console::app::overview_page(&status, &content, read)
                    }
                    Section::Record => {
                        let sessions = store.sessions(&rules, engagement.as_deref())?;
                        // A session named in the query is rendered into the
                        // pane; one the index does not hold is said so in
                        // the pane, the same answer the fragment gives.
                        let selected = match session.as_deref() {
                            Some(id) => Some((
                                id,
                                store.session_records(id)?.unwrap_or_else(
                                    || json!({"error": format!("no session {id} in the index")}),
                                ),
                            )),
                            None => None,
                        };
                        console::app::record_page(
                            &sessions,
                            selected.as_ref().map(|(id, records)| (*id, records)),
                            read,
                        )
                    }
                    Section::Policy => {
                        let facts = store.cwd_facts()?;
                        let policy = console::policy::projection(
                            &home,
                            &facts,
                            &rules,
                            engagement.as_deref(),
                        );
                        let editor = console::policy::attribution_editor(&home);
                        console::app::policy_page(&policy, &editor, None, &at)
                    }
                    Section::Budget => console::app::budget_page(
                        &budget_projection(store, &home, &rules, engagement.as_deref())?,
                        read,
                    ),
                    Section::Sources => console::app::sources_page(&providers),
                    Section::Compare => console::app::compare_page(None, &[], &providers),
                    // `/app/agents` is answered by `agents_route`, which
                    // needs the probe; this arm serves the same screen with
                    // liveness unavailable should a caller route here.
                    Section::Agents => console::agents::page(
                        &console::agents::projection(
                            &store.host_sessions()?,
                            &console::policy::projection(&home, &store.cwd_facts()?, &rules, None),
                            &egress,
                            None,
                            chrono::Utc::now(),
                        ),
                        None,
                        read,
                    ),
                })
            })
        }
        Err(error) => html_error(500, &format!("{error:#}")),
    }
}

/// One of the Policy screen's writes. The form is decoded, the edit is made
/// under `console::edit`'s conditions, and the screen comes back rendered
/// from a fresh read with the outcome stated at the top and the outcome's
/// own status: 200 saved or unchanged, 409 conflict, 400 refused. The page
/// configures htmx to swap the answer in on every status
/// (`console::app::shell`), so an htmx post reads the same notice a plain
/// post does.
fn policy_write(
    state: &Mutex<Store>,
    home: &std::path::Path,
    sessions_dir: &std::path::Path,
    path: &str,
    request: &Request,
) -> Response {
    use console::edit::{self, Outcome};
    let fields = match request_fields(request) {
        Ok(fields) => fields,
        Err(error) => {
            let outcome = Outcome::Refused {
                status: 400,
                notice: format!("Not saved: the form could not be read. {error:#}"),
            };
            return policy_answer(state, home, sessions_dir, request, &outcome);
        }
    };
    let field = |name: &str| {
        fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let revision = field("revision").unwrap_or_default();
    let outcome = match path {
        "/app/policy/mode" | "/api/policy/mode" => {
            match edit::mode_of(field("mode").unwrap_or_default()) {
                Ok(mode) => edit::set_mode(home, revision, edit::scope_field(field("scope")), mode),
                Err(reason) => Outcome::Refused {
                    status: 400,
                    notice: format!("Not saved: {reason}."),
                },
            }
        }
        "/app/policy/deny" | "/api/policy/deny" => edit::deny_host(
            home,
            revision,
            edit::scope_field(field("scope")),
            field("host").unwrap_or_default(),
        ),
        _ => match attribution_pairs(&fields) {
            Ok(pairs) => edit::set_attribution(home, revision, &pairs),
            Err(reason) => Outcome::Refused {
                status: 400,
                notice: format!("Not saved: {reason}."),
            },
        },
    };
    policy_answer(state, home, sessions_dir, request, &outcome)
}

/// The attribution editor's rows as ordered pairs. Row order is precedence.
/// A row with both fields blank is the empty row the editor offers and is
/// dropped; an engagement left blank beside a match is a rule that names
/// nothing, which the rule file's own validity rule refuses. A `match` with
/// no `engagement` after it, or the reverse, is a form this console did not
/// render, and is refused by name rather than dropped.
fn attribution_pairs(fields: &[(String, String)]) -> Result<Vec<(String, String)>> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut pending: Option<&str> = None;
    for (key, value) in fields {
        match key.as_str() {
            "match" => {
                if pending.is_some() {
                    bail!(
                        "a \"match\" field was followed by another \"match\" and not by its \"engagement\""
                    );
                }
                pending = Some(value);
            }
            "engagement" => {
                let Some(matcher) = pending.take() else {
                    bail!("an \"engagement\" field arrived with no \"match\" before it");
                };
                if !(matcher.is_empty() && value.is_empty()) {
                    pairs.push((matcher.to_owned(), value.clone()));
                }
            }
            _ => {}
        }
    }
    if pending.is_some() {
        bail!("the last \"match\" field has no \"engagement\" after it");
    }
    Ok(pairs)
}

/// The Policy screen after a write, from a fresh read of both declarations.
fn policy_answer(
    state: &Mutex<Store>,
    home: &std::path::Path,
    sessions_dir: &std::path::Path,
    request: &Request,
    outcome: &console::edit::Outcome,
) -> Response {
    if wants_json(request) {
        return json_value(
            outcome.status(),
            &json!({
                "kind": outcome.kind(), "status": outcome.status(), "notice": outcome.notice(),
                "revision": write_revision(home, request),
            }),
        );
    }
    let fragment = request
        .headers
        .get("HX-Request")
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    let status = outcome.status();
    let notice = (outcome.kind(), outcome.notice().to_owned());
    let home = home.to_path_buf();
    let rendered = policy_screen(state, &home, sessions_dir, Some(notice), fragment);
    match rendered {
        Ok(markup) => html(status, &markup),
        Err(error) => html_error(500, &format!("{error:#}")),
    }
}

/// The Policy screen, page or fragment, from one read of the policy and one
/// of the attribution rules. A rule file that does not parse is shown in its
/// editor with the error, and the policy is projected with no attribution,
/// so the operator can repair the rules from the page that reports them
/// rather than being locked out of it.
fn policy_screen(
    state: &Mutex<Store>,
    home: &std::path::Path,
    sessions_dir: &std::path::Path,
    notice: Option<(&'static str, String)>,
    fragment: bool,
) -> Result<String> {
    let editor = console::policy::attribution_editor(home);
    let rules = Attribution::read(home)
        .and_then(|snapshot| snapshot.rules)
        .unwrap_or_else(|_| Attribution::none());
    let mut store = state
        .lock()
        .map_err(|_| anyhow::anyhow!("the index lock was poisoned"))?;
    store.ingest_sessions(sessions_dir)?;
    let facts = store.cwd_facts()?;
    let policy = console::policy::projection(home, &facts, &rules, None);
    let notice = notice.as_ref().map(|(kind, text)| (*kind, text.as_str()));
    Ok(if fragment {
        console::app::policy_fragment(&policy, &editor, notice)
    } else {
        console::app::policy_page(&policy, &editor, notice, &read_time())
    })
}

/// The dry run for a draft rule: the candidate is built through the runtime's
/// own document and judged over every recorded crossing by the runtime's own
/// admission check (`console::forecast`). A read: nothing is written, and the
/// fragment says so.
fn forecast_route(
    state: &Mutex<Store>,
    home: &std::path::Path,
    sessions_dir: &std::path::Path,
    request: &Request,
) -> Response {
    use commonmeasure_harness::policy::PolicyDocument;
    use console::edit;
    let param = |name: &str| query_param(&request.target, name);
    let scope = param("scope");
    let scope = edit::scope_field(scope.as_deref());
    let host = param("host").unwrap_or_default();
    let action = param("action").unwrap_or_else(|| "deny".to_owned());
    let named = match scope {
        Some(matcher) => format!("scope \"{matcher}\""),
        None => "the top-level policy".to_owned(),
    };
    let answer = |value: Value| {
        if wants_json(request) {
            json_value(200, &value)
        } else {
            html(
                200,
                &console::app::forecast_fragment(&value, &value["draft"]),
            )
        }
    };
    let failed = |reason: String| answer(json!({"error": reason, "saved": false}));
    let document = match PolicyDocument::read(home) {
        Ok(document) if document.declared() => document,
        Ok(_) => {
            return failed(format!(
                "no policy is declared at {}, so there is no declaration to draft against",
                home.join("policy.json").display()
            ));
        }
        Err(error) => return failed(error),
    };
    let (candidate, draft) = if action == "mode" {
        let mode = match edit::mode_of(param("mode").as_deref().unwrap_or_default()) {
            Ok(mode) => mode,
            Err(reason) => return failed(reason),
        };
        match document.with_mode(scope, mode) {
            Ok(file) => (
                file,
                json!({"description": format!("{named} running in {} mode", edit::mode_word(mode))}),
            ),
            Err(reason) => return failed(reason),
        }
    } else if action == "deny" {
        match document.with_denied_host(scope, &host) {
            Ok((file, _)) => (
                file,
                json!({"description": format!(
                    "denying host {} in {named}",
                    commonmeasure_runtime::policy::normalised_host(&host)
                )}),
            ),
            Err(reason) => return failed(reason),
        }
    } else {
        let pattern = match edit::host_pattern(&host) {
            Ok(pattern) => pattern,
            Err(reason) => return failed(reason),
        };
        let action = match edit::access_action(&action, param("licence").as_deref()) {
            Ok(action) => action,
            Err(reason) => return failed(reason),
        };
        let rule = commonmeasure_types::Constraint::AccessRule {
            host: pattern.clone(),
            action: action.clone(),
        };
        let encoded = serde_json::to_string(&rule).unwrap_or_default();
        match document.with_access_rule(scope, pattern.clone(), action.clone()) {
            Ok(file) => (
                file,
                json!({
                    "description": format!("the rule {pattern} → {} in {named}", action.as_str()),
                    "json": encoded,
                }),
            ),
            Err(reason) => return failed(reason),
        }
    };
    let mut store = match state.lock() {
        Ok(store) => store,
        Err(_) => return negotiated_error(request, 500, "the index lock was poisoned"),
    };
    if let Err(error) = store.ingest_sessions(sessions_dir) {
        return negotiated_error(request, 500, &format!("ingest failed: {error:#}"));
    }
    let facts = match store.crossing_facts() {
        Ok(facts) => facts,
        Err(error) => return negotiated_error(request, 500, &format!("{error:#}")),
    };
    let mut forecast = console::forecast::forecast(&document, candidate, &facts);
    forecast["draft"] = draft;
    answer(forecast)
}

/// The JSON routes. The query answers with its own status, because "the store
/// held nothing under that id" is not a failure of the query and not a success
/// of the request either.
fn with_store<F>(state: &Mutex<Store>, sessions_dir: &std::path::Path, query: F) -> Response
where
    F: FnOnce(&mut Store) -> Result<(u16, Value)>,
{
    let mut store = match state.lock() {
        Ok(store) => store,
        Err(_) => return json_error(500, "the index lock was poisoned"),
    };
    if let Err(error) = store.ingest_sessions(sessions_dir) {
        return json_error(500, &format!("ingest failed: {error:#}"));
    }
    match query(&mut store) {
        Ok((status, value)) => json(status, &value.to_string()),
        Err(error) => json_error(500, &format!("{error:#}")),
    }
}

/// [`with_store`] for routes that render HTML from what they query.
fn with_store_html<F>(state: &Mutex<Store>, sessions_dir: &std::path::Path, render: F) -> Response
where
    F: FnOnce(&mut Store) -> Result<String>,
{
    let mut store = match state.lock() {
        Ok(store) => store,
        Err(_) => return html_error(500, "the index lock was poisoned"),
    };
    if let Err(error) = store.ingest_sessions(sessions_dir) {
        return html_error(500, &format!("ingest failed: {error:#}"));
    }
    match render(&mut store) {
        Ok(markup) => html(200, &markup),
        Err(error) => html_error(500, &format!("{error:#}")),
    }
}

/// A guide document, rendered from its Markdown source at first request
/// (`crate::guide`). The console's blanket policy (`style-src 'self'`) would
/// strip the guide's one inline `<style>` element, so this response names
/// that exact element by hash instead of loosening the policy to
/// `'unsafe-inline'`; the hash is computed from the rendered bytes, so a
/// re-rendered guide can never serve unstyled. Everything else stays at the
/// console's strictest, with the local appearance script allowed by origin.
fn guide_page(name: &str) -> Response {
    let Some(document) = crate::guide::console_page(name) else {
        return html_error(404, "no such guide page");
    };
    let style_src = match inline_style_hash(document) {
        Some(hash) => format!("'sha256-{hash}'"),
        None => "'none'".to_owned(),
    };
    let mut response = html(200, document);
    response.headers.set(
        "Content-Security-Policy",
        &format!(
            "default-src 'none'; script-src 'self'; style-src {style_src}; font-src 'self'; base-uri 'none'; frame-ancestors 'none'"
        ),
    );
    response
}

/// The CSP source hash of the document's one inline `<style>` element:
/// base64 of the SHA-256 of the element's exact text, per CSP2.
fn inline_style_hash(document: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let start = document.find("<style>")? + "<style>".len();
    let end = start + document[start..].find("</style>")?;
    Some(base64(&Sha256::digest(&document.as_bytes()[start..end])))
}

/// Standard base64. CSP hash sources are base64-encoded by definition, and
/// this is the one place in the workspace that needs the encoding, so it is
/// written here rather than pulling a dependency for eleven lines.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let word = u32::from_be_bytes([
            0,
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ]);
        out.push(ALPHABET[(word >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(word >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(word >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(word & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn asset(body: &str, content_type: &str) -> Response {
    binary_asset(body.as_bytes(), content_type)
}

fn binary_asset(body: &[u8], content_type: &str) -> Response {
    let mut response = Response::new(200, body.to_vec());
    response.headers.set("Content-Type", content_type);
    hardened(response)
}

fn html(status: u16, body: &str) -> Response {
    let mut response = Response::new(status, body.as_bytes().to_vec());
    response
        .headers
        .set("Content-Type", "text/html; charset=utf-8");
    hardened(response)
}

/// [`html`] for the JSON routes. A response is hardened by being served, not
/// by what it renders: `nosniff` is exactly the mitigation for a JSON body
/// carrying bytes a caller chose, and an error answer carries those more often
/// than a page does.
fn json_value(status: u16, value: &Value) -> Response {
    json(status, &value.to_string())
}

fn json(status: u16, body: &str) -> Response {
    hardened(Response::json(status, body))
}

/// A second layer under [`console::html::esc`]. The console renders evidence
/// it did not author — a recorded page title is attacker-influenced — and one
/// missed interpolation would otherwise be immediately exploitable. Every
/// resource this console loads is served by this binary, so the policy costs
/// nothing to state at its strictest.
fn hardened(mut response: Response) -> Response {
    response.headers.set(
        "Content-Security-Policy",
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; font-src 'self'; connect-src 'self'; \
         form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
    );
    response.headers.set("X-Content-Type-Options", "nosniff");
    response.headers.set("X-Frame-Options", "DENY");
    // `same-origin`, not `no-referrer`: no recorded URL leaves with a
    // cross-site request either way, and under `no-referrer` a browser
    // serialises the `Origin` of the console's own form posts as `null`,
    // which the write guard has to refuse.
    response.headers.set("Referrer-Policy", "same-origin");
    // The console mirrors a record that changes under it, and is iterated on
    // live; a browser that served a page or stylesheet from cache would show a
    // stale view of the operator's own evidence. Never cache — always refetch.
    response.headers.set("Cache-Control", "no-store");
    response.headers.set("Vary", "Accept");
    response
}

/// An error a person reads: the status and the reason inside the console's
/// own shell, with a heading, a language and a way back, rather than a bare
/// paragraph or a JSON body.
fn html_error(status: u16, detail: &str) -> Response {
    html(status, &console::app::error_page(status, detail))
}

/// The moment a screen was read, stated on the screen, to the minute in UTC.
fn read_time() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string()
}

fn json_error(status: u16, detail: &str) -> Response {
    json(status, &json!({"error": detail}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    /// The editor's rows decode in order, the blank row is dropped, and a
    /// field with no partner is refused by name rather than dropped.
    #[test]
    fn attribution_rows_decode_in_order_and_an_unpaired_field_is_refused_by_name() {
        let pairs = attribution_pairs(&fields(&[
            ("revision", "abc"),
            ("match", "code/ozone"),
            ("engagement", "ozone"),
            ("match", "code"),
            ("engagement", "personal"),
            ("match", ""),
            ("engagement", ""),
        ]))
        .unwrap();
        assert_eq!(
            pairs,
            fields(&[("code/ozone", "ozone"), ("code", "personal")])
        );
        let trailing = attribution_pairs(&fields(&[
            ("match", "code"),
            ("engagement", "x"),
            ("match", "tail"),
        ]))
        .unwrap_err()
        .to_string();
        assert!(
            trailing.contains("last \"match\" field has no \"engagement\""),
            "{trailing}"
        );
        let doubled = attribution_pairs(&fields(&[
            ("match", "a"),
            ("match", "b"),
            ("engagement", "x"),
        ]))
        .unwrap_err()
        .to_string();
        assert!(
            doubled.contains("followed by another \"match\""),
            "{doubled}"
        );
        let orphan = attribution_pairs(&fields(&[("engagement", "x")]))
            .unwrap_err()
            .to_string();
        assert!(orphan.contains("no \"match\" before it"), "{orphan}");
    }

    /// The offline shell must serve fonts as binary data and permit only local assets.
    #[test]
    fn embedded_brand_and_fonts_are_available_through_the_router() -> Result<()> {
        let home = tempfile::tempdir()?;
        let state = Mutex::new(Store::open(&home.path().join("telemetry.db"))?);
        for (path, content_type, signature) in [
            ("/favicon.svg", "image/svg+xml", b"<svg".as_slice()),
            ("/brand/edge.svg", "image/svg+xml", b"<svg".as_slice()),
            (
                "/fonts/Geist-Variable.woff2",
                "font/woff2",
                b"wOF2".as_slice(),
            ),
            (
                "/fonts/GeistMono-Variable.woff2",
                "font/woff2",
                b"wOF2".as_slice(),
            ),
        ] {
            let response = route(
                &state,
                home.path(),
                &home.path().join("sessions"),
                &Injected {
                    providers: &[],
                    search: None,
                    liveness: None,
                },
                &loopback_guard(),
                &Request::get(path),
            );
            assert_eq!(response.status, 200, "{path}");
            assert_eq!(response.headers.get("Content-Type"), Some(content_type));
            assert!(response.body.starts_with(signature), "{path}");
            let csp = response
                .headers
                .get("Content-Security-Policy")
                .unwrap_or_default();
            assert!(csp.contains("img-src 'self'") && csp.contains("font-src 'self'"));
            assert!(csp.contains("default-src 'none'") && !csp.contains("unsafe-inline"));
        }
        Ok(())
    }

    fn loopback_guard() -> HostGuard {
        HostGuard {
            address: "127.0.0.1:4173".parse().unwrap(),
            allow_remote: false,
        }
    }

    #[test]
    fn a_non_loopback_listen_is_refused_without_the_opt_in() {
        assert!(exposed_addresses("127.0.0.1:0").unwrap().is_empty());
        assert!(exposed_addresses("localhost:0").unwrap().is_empty());
        assert!(!exposed_addresses("0.0.0.0:0").unwrap().is_empty());

        let home = tempfile::tempdir().unwrap();
        let error = serve(ServeOptions {
            listen: "0.0.0.0:0".to_owned(),
            allow_remote: false,
            home: home.path().to_path_buf(),
            providers: Vec::new(),
            search: None,
            liveness: None,
        })
        .unwrap_err();
        let stated = format!("{error:#}");
        assert!(stated.contains("reachable from outside this machine"));
        assert!(stated.contains("--allow-remote"), "the way out is named");
    }

    /// A page on the public internet can point its own hostname at 127.0.0.1
    /// and script a browser into reading this console. Only the `Host` header
    /// separates that from the operator's own tab.
    #[test]
    fn a_foreign_host_header_is_refused_on_a_loopback_bind() {
        let guard = loopback_guard();
        assert!(!guard.permits(Some("evil.example.com")));
        assert!(!guard.permits(Some("evil.example.com:4173")));
        assert!(
            !guard.permits(Some("127.0.0.1:9999")),
            "port is checked too"
        );

        assert!(guard.permits(Some("localhost:4173")));
        assert!(guard.permits(Some("127.0.0.1:4173")));
        assert!(guard.permits(Some("127.0.0.1")));
        assert!(guard.permits(Some("[::1]:4173")));
        assert!(guard.permits(None));

        let opted_in = HostGuard {
            address: "0.0.0.0:4173".parse().unwrap(),
            allow_remote: true,
        };
        assert!(opted_in.permits(Some("evil.example.com")));
    }

    /// A browser posting the console's own form names the console as its
    /// `Origin`; a page elsewhere names itself, or `null` where its origin is
    /// opaque. `Sec-Fetch-Site: same-origin` is the browser vouching for the
    /// request, and it is the only thing that lets a `null` origin through.
    #[test]
    fn a_write_is_taken_from_the_consoles_own_page_and_from_nowhere_else() {
        let guard = loopback_guard();
        assert!(guard.permits_origin(Some("http://127.0.0.1:4173"), None));
        assert!(guard.permits_origin(Some("http://localhost:4173"), None));
        assert!(guard.permits_origin(Some("http://[::1]:4173"), None));
        assert!(guard.permits_origin(None, None), "no page at all");
        assert!(!guard.permits_origin(Some("http://evil.example"), None));
        assert!(!guard.permits_origin(Some("http://127.0.0.1:9999"), None));
        assert!(!guard.permits_origin(Some("https://127.0.0.1:4173"), None));
        assert!(!guard.permits_origin(Some("null"), None));
        assert!(!guard.permits_origin(Some("null"), Some("cross-site")));
        assert!(guard.permits_origin(Some("null"), Some("same-origin")));
        assert!(guard.permits_origin(Some("http://evil.example"), Some("same-origin")));
    }

    /// `--allow-remote` says which address to bind, not which names to answer
    /// to; on a loopback bind the name guard stays on.
    #[test]
    fn allow_remote_does_not_open_a_loopback_bind_to_foreign_names() {
        let opted_in = HostGuard {
            address: "127.0.0.1:4173".parse().unwrap(),
            allow_remote: true,
        };
        assert!(!opted_in.permits(Some("evil.example")));
        assert!(opted_in.permits(Some("127.0.0.1:4173")));
        assert!(opted_in.permits(Some("localhost:4173")));
    }

    /// A `Host` the parser cannot read is not a `Host` this console bound.
    #[test]
    fn a_host_value_that_does_not_parse_is_refused_not_half_checked() {
        let guard = loopback_guard();
        for host in [
            "127.0.0.1:99999",
            "localhost:abc",
            "localhost:4173 evil",
            "[localhost]evil.example",
            "localhost:",
        ] {
            assert!(!guard.permits(Some(host)), "{host} was accepted");
        }
        assert!(guard.permits(Some("localhost:4173")));
        assert!(guard.permits(Some("[::1]:4173")));
        assert!(guard.permits(Some("[::1]")));
    }
}
