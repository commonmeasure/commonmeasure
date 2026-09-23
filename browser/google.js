// Google AI Overview capture, read from the rendered page.
//
// Google exposes no stable stream for the overview: it is rendered into the
// results page from internal requests whose format changes often, so the
// rendered page is the only contract to read. Two anchors change far less
// often than Google's generated class names: the visible "AI Overview" label
// and the organic results container (`#rso`). The overview block is the
// highest ancestor of the label that does not also contain `#rso`; the
// external links inside it are the overview's sources. Nothing below it is
// read, because the organic results are the user's own search, not what the
// overview drew on.
//
// Each source is a link shown with the answer, so it is sent as cited and as
// retrieved. Nothing grounds: the page does not carry the text the model read.
// If Google renames the label or `#rso`, this finds nothing rather than
// guessing; check it against a live overview after a Google redesign.

(() => {
  "use strict";

  // Google's own hosts, which are search furniture and never an external
  // source. Kept narrow: YouTube, Blogspot and the like are real sources.
  const GOOGLE_HOSTS = [
    "google.com",
    "google.co.uk",
    "gstatic.com",
    "googleusercontent.com",
    "googleapis.com",
    "schema.org",
  ];

  // The overview streams in and "Show more" adds sources, so extraction waits
  // for the page to stop changing rather than running on every mutation.
  const SETTLE_MS = 1500;

  let timer = null;
  // The query the sent sources belong to, and those sources, so a re-render
  // sends only sources not already sent for the same query.
  let sentQuery = null;
  let sent = new Set();

  // The query is compared, never sent.
  function currentQuery() {
    try {
      return new URL(location.href).searchParams.get("q");
    } catch (_) {
      return null;
    }
  }

  function isGoogleHost(host) {
    const lower = host.toLowerCase();
    return GOOGLE_HOSTS.some((google) => lower === google || lower.endsWith("." + google));
  }

  // Resolve a Google redirect (`/url?q=DEST` or `/url?url=DEST`).
  function unwrap(href) {
    try {
      const url = new URL(href, location.href);
      if (isGoogleHost(url.host) && url.pathname === "/url") {
        const destination = url.searchParams.get("q") || url.searchParams.get("url");
        if (destination) return destination;
      }
      return url.toString();
    } catch (_) {
      return null;
    }
  }

  function canonicalUrl(raw) {
    let url;
    try {
      url = new URL(raw);
    } catch (_) {
      return null;
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    if (isGoogleHost(url.host)) return null;
    const tracking = [...url.searchParams.keys()].filter((key) => key.startsWith("utm_"));
    for (const key of tracking) url.searchParams.delete(key);
    if ([...url.searchParams.keys()].length === 0) url.search = "";
    return url.toString();
  }

  // A heading or labelled node whose own short text begins "AI Overview", not
  // a paragraph that merely mentions the phrase.
  function findLabel() {
    const candidates = document.querySelectorAll('h1, h2, h3, [role="heading"], [aria-label]');
    for (const element of candidates) {
      const text = (element.getAttribute("aria-label") || element.textContent || "").trim();
      if (/^ai overview\b/i.test(text) && text.length < 40) return element;
    }
    return null;
  }

  function overviewRoot() {
    const label = findLabel();
    if (!label) return null;
    const results = document.querySelector("#rso");
    let root = label;
    while (
      root.parentElement &&
      root.parentElement !== document.body &&
      !(results && root.parentElement.contains(results))
    ) {
      root = root.parentElement;
    }
    return root;
  }

  function extractSources(root) {
    const seen = new Set();
    for (const anchor of root.querySelectorAll("a[href]")) {
      const destination = unwrap(anchor.getAttribute("href"));
      if (!destination) continue;
      const canonical = canonicalUrl(destination);
      if (canonical) seen.add(canonical);
    }
    return [...seen].sort();
  }

  function tryCapture() {
    const root = overviewRoot();
    if (!root) return;
    const query = currentQuery();
    if (query !== sentQuery) {
      sentQuery = query;
      sent = new Set();
    }
    const fresh = extractSources(root).filter((url) => !sent.has(url));
    if (fresh.length === 0) return;
    for (const url of fresh) sent.add(url);
    chrome.runtime
      .sendMessage({ kind: "google-ai-overview", retrieved: fresh, cited: fresh })
      .catch(() => {
        // The worker may be restarting; these sources are missed.
      });
  }

  function schedule() {
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(tryCapture, SETTLE_MS);
  }

  // The overview arrives after first paint, and Google replaces results in
  // place for a new query, so the document is watched rather than read once.
  new MutationObserver(schedule).observe(document.documentElement, {
    childList: true,
    subtree: true,
  });
  schedule();
})();
