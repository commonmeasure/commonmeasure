// Isolated-world relay on ChatGPT.
//
// The page-world script (`inject.js`) cannot reach the extension APIs, and the
// background worker cannot see the page's `fetch`. This script bridges the
// two: it accepts the captured stream posted by `inject.js` on this page and
// passes it to the background worker, which parses it and hands the result to
// the binary.

(() => {
  "use strict";

  const CHANNEL = "commonmeasure";

  window.addEventListener("message", (event) => {
    // Only this page's own window, not an embedded frame or another origin.
    if (event.source !== window) return;
    const data = event.data;
    if (!data || data.channel !== CHANNEL || data.kind !== "chatgpt-sse") return;
    if (typeof data.body !== "string" || data.body.length === 0) return;
    chrome.runtime.sendMessage({ kind: "chatgpt-sse", body: data.body }).catch(() => {
      // The worker may be restarting; this turn is missed.
    });
  });
})();
