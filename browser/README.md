# Browser extension

A Chrome extension that records the sources three browser AI answers show:
ChatGPT on the web, Google AI Overviews and Bing Copilot Search. The
model's own web search on those surfaces crosses no tool Common Measure can
offer, so nothing there can be refused; the extension observes the answer
and the binary records each source as an observed
[crossing](../docs/GLOSSARY.md), retrieved and never grounded.

The extension keeps no record of its own. Each answer's sources go to the
`commonmeasure` binary over Chrome native messaging, and the operator
console (`commonmeasure serve`) renders them with the rest of the record.

## Install

1. Register the binary with Chrome. This writes the native messaging host
   manifest that lets the extension start the binary:

   ```sh
   commonmeasure install chrome
   commonmeasure doctor chrome
   ```

2. Open `chrome://extensions`, turn on Developer mode, choose Load unpacked
   and select this `browser/` directory. The extension's id is
   `hojjbnoeobjkjklcdhhnncmmojmcneig` on every machine, because the public
   key in `browser/manifest.json` fixes it; the manifest `install chrome`
   writes allows that id and no other.

3. Open the extension's popup. It shows the binary's version and whether a
   session log can be written, or Chrome's reason the binary cannot be
   reached.

To share the extension without the repository, build the archive, which
holds only the files Chrome loads:

```sh
sh browser/package.sh
```

The archive is written to `dist/`; the recipient unzips it and loads the
directory in the same way.

## What is recorded

| Surface | Read from | Session |
|---|---|---|
| ChatGPT on the web | the conversation's event stream, copied in the page and parsed by `browser/parse.js`: search-result links and citation links | the conversation id |
| Google AI Overviews | the external links inside the rendered overview block | none; each answer is its own session |
| Bing Copilot Search | the citation links inside the rendered Copilot answer block | none; each answer is its own session |

- Every source is one observed crossing with no content hash: no surface
  exposes the page text the model read.
- A citation is recorded as a retrieved crossing. The session evidence
  format has no field that marks a source as cited.
- The prompt, the answer and the search query are never read by the
  extension or sent to the binary.
- A ChatGPT answer without web search carries no source and sends nothing.
- The Google and Bing readers depend on the page's visible labels and
  containers. If either page changes, the reader finds nothing rather than
  guessing.
- The record is only as trustworthy as the page: a script running in the
  page could add a source its model never saw.

The message format, the mapping onto session evidence and each surface's
verification state are `docs/contracts/host-integration.md` §2 and §6.

## Files

| File | Role |
|---|---|
| `manifest.json` | Manifest v3: the content scripts, the background worker, the popup, the `nativeMessaging` permission and the key that fixes the id |
| `inject.js` | Runs in the ChatGPT page and copies each event-stream response to `content.js` without touching the page's own copy |
| `content.js` | Passes the copied stream to the background worker |
| `google.js` | Reads the AI Overview's sources from the Google results page |
| `bing.js` | Reads the Copilot answer's sources from the Bing results page |
| `parse.js` | The ChatGPT stream parser |
| `message.js` | Builds the message the binary reads |
| `background.js` | Parses ChatGPT streams and sends each surface's answer to the binary |
| `popup.html`, `popup.js` | Whether the binary answers, and the outcome of the last answer sent |
| `package.sh` | Builds the archive |

## Tests

The parser tests carry the assertions of the Rust parser `parse.js` was
ported from, over the same stream, and check that the parser's output is
the message `crates/commonmeasure-cli/tests/browser_e2e.rs` feeds the
binary:

```sh
node --test browser/test/
```
