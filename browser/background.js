// Background worker: turns each captured answer into one message and hands it
// to the binary over native messaging (`commonmeasure native-host`, registered
// by `commonmeasure install chrome`). The binary records; the extension keeps
// nothing but the outcome of the last delivery, for the popup.
//
// Chrome starts the binary for each message and stops it after the reply, so
// no listener runs between answers and nothing is reachable from a web page.

"use strict";

import { parseSse } from "./parse.js";
import { HOSTS, NATIVE_HOST, answerMessage } from "./message.js";

async function deliver(message) {
  let outcome;
  try {
    const reply = await chrome.runtime.sendNativeMessage(NATIVE_HOST, message);
    outcome = {
      lastHost: message.host,
      lastRecorded: Number.isInteger(reply && reply.recorded) ? reply.recorded : null,
      lastError: reply && typeof reply.error === "string" ? reply.error : null,
    };
  } catch (error) {
    // Chrome's own words: most often "Specified native messaging host not
    // found.", which `commonmeasure install chrome` fixes.
    outcome = { lastHost: message.host, lastRecorded: null, lastError: String(error.message || error) };
  }
  await chrome.storage.local.set({ ...outcome, lastAt: new Date().toISOString() }).catch(() => {});
}

chrome.runtime.onMessage.addListener((message) => {
  if (!message) return false;
  if (message.kind === "chatgpt-sse" && typeof message.body === "string") {
    const { conversationId, retrieved, cited } = parseSse(message.body);
    // A stream with no source in it (most turns without web search) sends
    // nothing, so a conversation leaves a session only when it crossed.
    if (retrieved.length > 0) {
      deliver(answerMessage(HOSTS.chatgpt, conversationId, { retrieved, cited }));
    }
  } else if (message.kind === "google-ai-overview" && Array.isArray(message.retrieved)) {
    deliver(answerMessage(HOSTS.google, null, message));
  } else if (message.kind === "bing-copilot-search" && Array.isArray(message.retrieved)) {
    deliver(answerMessage(HOSTS.bing, null, message));
  }
  return false;
});
