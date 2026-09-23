// Popup: whether the binary answers over native messaging, and what happened
// to the last answer sent. The record itself is read in the operator console
// (`commonmeasure serve`), not here.

"use strict";

import { NATIVE_HOST } from "./message.js";

const text = (id, value) => {
  document.getElementById(id).textContent = value;
};

async function render() {
  try {
    const status = await chrome.runtime.sendNativeMessage(NATIVE_HOST, { status: true });
    text("binary", `commonmeasure ${status.version}`);
    // The doctor's line, without its "recording:" label.
    text("recording", String(status.recording).replace(/^recording: /, ""));
  } catch (error) {
    text("binary", `not reachable: ${error.message || error}`);
    text("recording", "run commonmeasure install chrome, then reload the extension");
  }

  const last = await chrome.storage.local
    .get(["lastAt", "lastHost", "lastRecorded", "lastError"])
    .catch(() => ({}));
  if (last.lastAt) {
    const when = new Date(last.lastAt).toLocaleString();
    text(
      "last",
      last.lastError
        ? `${last.lastHost}, ${when}: ${last.lastError}`
        : `${last.lastHost}, ${when}: ${last.lastRecorded} crossing(s) recorded`
    );
  }
}

render();
