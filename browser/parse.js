// ChatGPT event-stream parser.
//
// The one place in this extension that reads ChatGPT's private
// `/backend-api/conversation` stream format. It is a port of an earlier
// in-house Rust parser; that parser's test streams and assertions are carried
// in `test/parse.test.js`, so a change of behaviour here fails there.
//
// What the stream supports claiming:
//   - search-result links (`entries` in a search-result group) are sources the
//     model saw as titles and snippets;
//   - citation links (`items` of a content reference, their
//     `supporting_websites`, and a footnote's `sources`) are sources bound to
//     the answer, and every cited source is also in the retrieved set;
//   - nothing grounds: the stream never carries the page text the model read,
//     so no set of grounded sources is produced.
// `utm_*` parameters are stripped, non-http(s) URLs dropped, and each set is
// sorted and deduplicated, so a tagged and a bare link to one page collapse.

"use strict";

// Parse a conversation event stream. Never throws; unreadable input yields
// empty sets.
//
// Returns { conversationId: string|null, retrieved: string[], cited: string[] }.
export function parseSse(rawText) {
  let conversationId = null;
  const search = [];
  const cited = [];

  if (typeof rawText !== "string") {
    return { conversationId: null, retrieved: [], cited: [] };
  }

  for (const line of rawText.split(/\r?\n/)) {
    if (!line.startsWith("data:")) continue;
    const rest = line.slice("data:".length).trim();
    // `[DONE]` and other control lines are not JSON and are skipped.
    let value;
    try {
      value = JSON.parse(rest);
    } catch (_) {
      continue;
    }
    // An ad frame is a paid placement, not a source the model drew on; it is
    // never read, so its advertiser cannot appear as a source.
    if (value && value.type === "ads") continue;
    const id = harvest(value, search, cited);
    if (conversationId === null && id !== null) conversationId = id;
  }

  const citedCanon = canonicalise(cited);
  const retrieved = canonicalise(search.concat(citedCanon));
  return { conversationId, retrieved, cited: citedCanon };
}

// Walk a frame at any depth, collecting source URLs by the structure that
// carries them. Returns the conversation id this subtree carried, if any.
function harvest(value, search, cited) {
  let foundId = null;

  if (Array.isArray(value)) {
    for (const item of value) {
      const id = harvest(item, search, cited);
      if (foundId === null && id !== null) foundId = id;
    }
    return foundId;
  }

  if (value === null || typeof value !== "object") return null;

  if (typeof value.conversation_id === "string") foundId = value.conversation_id;

  if (Array.isArray(value.entries)) pushUrls(value.entries, search);

  if (Array.isArray(value.items)) {
    for (const item of value.items) {
      pushUrl(item, cited);
      if (item && Array.isArray(item.supporting_websites)) {
        pushUrls(item.supporting_websites, cited);
      }
    }
  }

  if (Array.isArray(value.sources)) pushUrls(value.sources, cited);

  for (const key of Object.keys(value)) {
    const id = harvest(value[key], search, cited);
    if (foundId === null && id !== null) foundId = id;
  }

  return foundId;
}

function pushUrls(items, out) {
  for (const item of items) pushUrl(item, out);
}

function pushUrl(item, out) {
  if (item && typeof item.url === "string") out.push(item.url);
}

function canonicalise(urls) {
  const out = [];
  for (const raw of urls) {
    const canonical = canonicalUrl(raw);
    if (canonical !== null) out.push(canonical);
  }
  out.sort();
  return out.filter((url, index) => index === 0 || url !== out[index - 1]);
}

// Drop non-http(s) and strip `utm_*` parameters (ChatGPT appends
// `?utm_source=chatgpt.com`), keeping every other query parameter.
export function canonicalUrl(raw) {
  let url;
  try {
    url = new URL(raw);
  } catch (_) {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  const tracking = [...url.searchParams.keys()].filter((key) => key.startsWith("utm_"));
  for (const key of tracking) url.searchParams.delete(key);
  // An empty query serialises with no `?`, as the Rust `url` crate writes it.
  if ([...url.searchParams.keys()].length === 0) url.search = "";
  return url.toString();
}
