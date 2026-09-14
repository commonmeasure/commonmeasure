// The message the extension sends the binary over native messaging, one per
// answer. Its shape is stated in `docs/contracts/host-integration.md` §2 and
// read by `commonmeasure native-host`.

"use strict";

// The native messaging host name `commonmeasure install chrome` registers.
export const NATIVE_HOST = "ai.commonmeasure.browser";

// The `host` value each surface is recorded under.
export const HOSTS = {
  chatgpt: "chatgpt-web",
  google: "google-ai-overview",
  bing: "bing-copilot-search",
};

// One answer's sources for one surface. `session` is omitted when the surface
// gave none, so the binary records the absence rather than an invented value.
export function answerMessage(host, session, { retrieved, cited }) {
  const message = {
    host,
    retrieved: Array.isArray(retrieved) ? retrieved : [],
    cited: Array.isArray(cited) ? cited : [],
  };
  if (typeof session === "string" && session.length > 0) message.session = session;
  return message;
}
