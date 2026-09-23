// The ChatGPT stream parser, held to the assertions of the Rust parser it was
// ported from, over the same compact stream. Run with `node --test browser/test/`.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { canonicalUrl, parseSse } from "../parse.js";
import { HOSTS, answerMessage } from "../message.js";

const fixture = (name) => readFileSync(new URL(`./fixtures/${name}`, import.meta.url), "utf8");

// A stream in the shape ChatGPT sends: the search call, a stub content
// reference, a filled one with a supporting site, a footnote and two
// search-result groups.
const SSE = fixture("chatgpt-search.sse");

test("cited and searched sources are kept apart and nothing grounds", () => {
  const parsed = parseSse(SSE);
  assert.equal(parsed.conversationId, "conv-abc");
  assert.deepEqual(
    Object.keys(parsed).sort(),
    ["cited", "conversationId", "retrieved"],
    "no grounded set is ever produced"
  );
  // Reuters appears tagged in a reference and bare in the footnote, and is
  // one source; the Guardian is a supporting site. Axios and Al Jazeera are
  // search results only.
  assert.deepEqual(parsed.cited, [
    "https://www.reuters.com/world/deal/",
    "https://www.theguardian.com/world/live/",
  ]);
  assert.deepEqual(parsed.retrieved, [
    "https://www.aljazeera.com/news/2026/6/8/iran/",
    "https://www.axios.com/2026/06/07/iran/",
    "https://www.reuters.com/world/deal/",
    "https://www.theguardian.com/world/live/",
  ]);
});

test("empty or unreadable input yields nothing", () => {
  for (const input of ["", "not an event stream\ndata: [DONE]", undefined, 42]) {
    assert.deepEqual(parseSse(input), { conversationId: null, retrieved: [], cited: [] });
  }
});

test("an ad frame's advertiser never becomes a source", () => {
  const ad = JSON.stringify({
    type: "ads",
    content: {
      advertiser_brand: { name: "Example Outfitters", url: "https://www.outfitters.example/" },
      ad_unit_header: { title: "Sponsored" },
      ad_cards: [
        {
          title: "Sticker pack",
          target: { value: "https://www.outfitters.example/p/1?oppref=x&utm_source=chatgpt.com" },
          sources: [{ url: "https://www.outfitters.example/landing" }],
        },
      ],
    },
  });
  const parsed = parseSse(`${SSE}\ndata: ${ad}\n`);
  assert.ok(parsed.retrieved.includes("https://www.axios.com/2026/06/07/iran/"));
  assert.ok(
    [...parsed.retrieved, ...parsed.cited].every((url) => !url.includes("outfitters.example")),
    "the advertiser must not appear as a source"
  );
});

test("utm parameters are stripped and every other parameter kept", () => {
  assert.equal(canonicalUrl("https://x.example/p?utm_source=chatgpt.com&id=7"), "https://x.example/p?id=7");
  assert.equal(canonicalUrl("https://x.example/p?utm_source=chatgpt.com"), "https://x.example/p");
  assert.equal(canonicalUrl("mailto:a@b.example"), null);
});

// The message file is also the payload `crates/commonmeasure-cli/tests/browser_e2e.rs`
// feeds the binary, so the extension's output and the binary's input are one
// file and cannot drift apart unnoticed.
test("the stream becomes the message the binary reads", () => {
  const { conversationId, retrieved, cited } = parseSse(SSE);
  assert.deepEqual(
    answerMessage(HOSTS.chatgpt, conversationId, { retrieved, cited }),
    JSON.parse(fixture("chatgpt-search.message.json"))
  );
});

test("a surface that gives no session sends none", () => {
  const message = answerMessage(HOSTS.google, null, { retrieved: ["https://a.example/"], cited: [] });
  assert.equal(message.host, "google-ai-overview");
  assert.equal("session" in message, false);
});
