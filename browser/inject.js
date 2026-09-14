// Page-world capture on ChatGPT.
//
// Runs in the page's own JavaScript context (`world: "MAIN"`) at
// document_start, so it wraps `fetch` before the application makes its first
// request. The assistant's answer streams back as server-sent events; this
// reads a clone of each event-stream response to the end and posts the text
// to the isolated content script. The page's own copy is untouched, so reading
// here cannot stall or alter the rendered answer. It never parses the stream
// (`parse.js` does, in the background worker) and it never reads the request,
// so the prompt stays in the page.
//
// Anything else running in the page can post to the same channel. What this
// records is what the page delivered, and a script in the page could forge it.

(() => {
  "use strict";

  const CHANNEL = "commonmeasure";

  const isStream = (response) => {
    try {
      return (response.headers.get("content-type") || "").includes("text/event-stream");
    } catch (_) {
      return false;
    }
  };

  // Capture is best-effort throughout: a read error misses this turn and is
  // never thrown back into the page.
  async function forward(response) {
    try {
      if (!response.body) return;
      const reader = response.clone().body.getReader();
      const decoder = new TextDecoder();
      let text = "";
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        text += decoder.decode(value, { stream: true });
      }
      text += decoder.decode();
      if (text) {
        window.postMessage({ channel: CHANNEL, kind: "chatgpt-sse", body: text }, window.origin);
      }
    } catch (_) {
      // Missed turn.
    }
  }

  const originalFetch = window.fetch;
  window.fetch = function (...args) {
    const promise = originalFetch.apply(this, args);
    promise
      .then((response) => {
        if (response && isStream(response)) forward(response);
      })
      .catch(() => {});
    return promise;
  };
})();
