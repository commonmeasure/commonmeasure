// Bing Copilot Search capture, read from the rendered page.
//
// The same approach as `google.js`: read only the answer block in the user's
// own results page, collect the citation links inside it, and send each as
// cited and retrieved. Nothing grounds.
//
// The anchors are Bing's answer container (`#copans_container`, or the
// answer block labelled "Copilot Search"), and otherwise a short visible
// "Copilot" label bounded by the organic results container (`#b_results`).
// If those disappear, this finds nothing rather than guessing.

(() => {
  "use strict";

  const BING_HOSTS = ["bing.com", "bing.net", "microsoftonline.com", "live.com"];

  const SETTLE_MS = 1500;

  let timer = null;
  let sentQuery = null;
  let sent = new Set();

  // The query is compared, never sent.
  function currentQuery() {
    try {
      const url = new URL(location.href);
      return url.searchParams.get("q") || url.searchParams.get("query");
    } catch (_) {
      return null;
    }
  }

  function isBingHost(host) {
    const lower = host.toLowerCase();
    return BING_HOSTS.some((bing) => lower === bing || lower.endsWith("." + bing));
  }

  // Bing's click wrappers carry the destination as `u=a1<base64url>`.
  function decodeBingU(value) {
    if (!value) return null;
    const raw = value.startsWith("a1") ? value.slice(2) : value;
    try {
      const padded = raw
        .replace(/-/g, "+")
        .replace(/_/g, "/")
        .padEnd(Math.ceil(raw.length / 4) * 4, "=");
      const decoded = atob(padded);
      return /^https?:\/\//i.test(decoded) ? decoded : null;
    } catch (_) {
      return null;
    }
  }

  function unwrap(href) {
    try {
      const url = new URL(href, location.href);
      if (isBingHost(url.host)) {
        const direct = url.searchParams.get("url") || url.searchParams.get("r");
        if (direct) return direct;
        const decoded = decodeBingU(url.searchParams.get("u"));
        if (decoded) return decoded;
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
    if (isBingHost(url.host)) return null;
    const tracking = [...url.searchParams.keys()].filter((key) => key.startsWith("utm_"));
    for (const key of tracking) url.searchParams.delete(key);
    if ([...url.searchParams.keys()].length === 0) url.search = "";
    return url.toString();
  }

  function findLabel() {
    const candidates = document.querySelectorAll(
      'h1, h2, h3, [role="heading"], [aria-label], [data-testid]'
    );
    for (const element of candidates) {
      const text = (element.getAttribute("aria-label") || element.textContent || "").trim();
      if (text.length >= 60) continue;
      if (/^((bing\s+)?copilot|ai-powered answer)\b/i.test(text)) return element;
    }
    return null;
  }

  function answerRoot() {
    // The answer module's own container keeps the tooltip, the related-query
    // carousel and the ordinary results outside the capture boundary.
    const explicit = document.querySelector("#copans_container");
    if (explicit) return explicit;

    const answer = document.querySelector('.answer_container[aria-label="Copilot Search"]');
    if (answer) return answer.closest("#copans_container") || answer;

    const label = findLabel();
    if (!label) return null;
    const results = document.querySelector("#b_results");
    let root = label;
    let depth = 0;
    while (root.parentElement && root.parentElement !== document.body) {
      if (results && root.parentElement.contains(results)) break;
      // A Copilot-only page may render no `#b_results`; a bounded climb keeps
      // the block from growing into the whole application shell.
      if (!results && depth >= 8) break;
      root = root.parentElement;
      depth += 1;
    }
    return root;
  }

  function citationAnchors(root) {
    return root.querySelectorAll(
      [
        "a.md_citlink[href]",
        ".cht_cit a[href]",
        ".cht_cit_panel_content a[href]",
        ".b_genserp_citation_hover_md a[href]",
      ].join(", ")
    );
  }

  function extractSources(root) {
    const seen = new Set();
    for (const anchor of citationAnchors(root)) {
      const destination = unwrap(anchor.getAttribute("href"));
      if (!destination) continue;
      const canonical = canonicalUrl(destination);
      if (canonical) seen.add(canonical);
    }
    return [...seen].sort();
  }

  function tryCapture() {
    const root = answerRoot();
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
      .sendMessage({ kind: "bing-copilot-search", retrieved: fresh, cited: fresh })
      .catch(() => {
        // The worker may be restarting; these sources are missed.
      });
  }

  function schedule() {
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(tryCapture, SETTLE_MS);
  }

  new MutationObserver(schedule).observe(document.documentElement, {
    childList: true,
    subtree: true,
  });
  schedule();
})();
