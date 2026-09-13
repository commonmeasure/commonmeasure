---
title: Machine-readable declarations a source can make
draft: true
---

# Machine-readable declarations a source can make

What a content source can state, in a form a program can read, about who it
is, what it allows and what it asks for; the standing of each mechanism; and
how the edge reads it. The rules the product applies to these statements are
`DECISIONS.md` §Source declarations; the packages that build the readers are
`ROADMAP.md` §Usage reporting to the sources that require it, §Output
provenance, §Verified fetcher identity and §Source preferences and the named
source. Terms like crossing, mediated
and grounded are defined in [`docs/GLOSSARY.md`](../GLOSSARY.md).

Every claim below names its source and the date it was checked. Standards
drafts move; check the source again before relying on a section number.

## Vocabulary

- **Manifest** — the JSON file a content owner publishes at
  `/.well-known/content-telemetry.json` stating who they are, where telemetry
  about their content should go, and which domains and identifier prefixes
  they claim (Content Telemetry v1.0 §8).
- **Preference** — a machine-readable statement of what an owner allows or
  disallows (training, AI use, search). Not a licence and not enforcement.
- **Licence** — terms under which content may be used, possibly with payment
  and a reporting duty. On the web, RSL is the machine-readable form.
- **Reporting demand** — a requirement that an agent report its use of
  content. Carried by a licence, never by a manifest.
- **Token** — the name a bot gives in its `User-Agent` header and that
  publishers address in `robots.txt`. The product's token is
  `CommonMeasureBot` (`PRODUCT.md` §Naming).

## Where a reporting demand comes from

Not the manifest. Either an RSL licence carrying the Content Telemetry
reporting binding, discovered from the page, or terms the operator holds and
references in `policy.json`. Absent both, no demand exists and the record
says so.

## Content Telemetry manifest

Identity and destination for an owner, agent or platform. Published standard,
v1.0, maintained in the `SPUR-Coalition/telemetry` repository and served at
contenttelemetry.org; the event schemas and the manifest schema are pinned
in `schema/` (`schema/SOURCE.md`), with the repository's manifest fixtures
under `schema/manifest-tests/`.

What the standard says (checked 5 September 2026):

- §8.7, how a consumer treats a manifest: a 404 or network error means the
  participant is unverified and nothing is rejected on that basis; invalid
  JSON, a schema failure, duplicate key ids, or a `domains` entry outside the
  manifest's own host mean reject the manifest; accept any `1.x`; cache by
  `Cache-Control` (hosts are told to use a one-hour age during onboarding and
  one day after).
- §8.5: a manifest has no field for an owner to demand a reporting level.
  §7.3: owner manifests do not tell agents where to send sessions; an agent
  sends to a consumer that routes per owner.
- §7.2: an agent sends a `Content-Telemetry-ID` header (a UUID) on each fetch
  and puts the same UUID on its retrieval event, so the owner can match
  reports to its own logs.
- §6.8: any content event may carry `data.evidence` entries with `scheme`,
  `ref` and `digest`.
- §8.4: manifests carry Ed25519 signing keys.

The reporting binding is a separate profile, published in the
`SPUR-Coalition/telemetry-profile` repository: an RSL `<reporting
type="telemetry" profile="https://contenttelemetry.org/profiles/spur"
endpoint="...">` element whose body is JSON with `conformance_level`
(required) and optional `privacy_level`, `manifest`, `delivery`, `coverage`
and `content_id_scheme`.

How the edge reads it: one GET at the well-known path on the domain root,
after the page bytes are fetched, never delaying the crossing
(`crates/commonmeasure-harness/src/manifest.rs`). A path manifest under a
prefix describes an agent or platform the domain operates, not the owner's
content. On a miss at a subdomain, the apex one label up is tried once,
because an apex manifest may claim subdomains; without a public suffix list
the edge does not climb further.

Sizing: across one operator's record of 27 sessions with crossings, 956
crossings touched a median of 8 distinct hosts per session, one session
touched 194, and there were 0.54 distinct hosts per crossing without a
cross-session cache. With a daily cache, discovery costs one extra
connection per host per day.

Known publishers: no third-party manifest publisher is known. On 5 September
2026, contenttelemetry.org, bbc.co.uk, telegraph.co.uk and apnews.com all
answered 404 at the well-known path.

## RSL 1.0 (Really Simple Licensing)

XML licence for web content; recommendation dated 10 December 2025 at
rslstandard.org. Discovered three ways: a `License:` line in `robots.txt`;
an HTTP header `Link: <url>; rel="license"; type="application/rsl+xml"`; an
HTML `<link>` or `<script type="application/rsl+xml">` in the page. Carries
`permits` (usage values such as `ai-input` and `ai-train`, user types,
geography), payment terms, legal terms and, with the reporting binding
above, the reporting demand.

How the edge reads it: the `Link` header from the page response at no extra
cost, and `robots.txt` once per host.

## Cloudflare Content Signals

A `robots.txt` line: `Content-Signal: search=yes, ai-input=no,
ai-train=no`. Launched 24 September 2025 under CC0 at contentsignals.org.
Three signals only. A preference, with no terms, endpoint or reporting. Read
from `robots.txt`. Cloudflare's managed `robots.txt` can prepend such a line
and a set of `Disallow` rules ahead of a site's own static file, so the
served file can contradict the site's licence.

## IETF AI Preferences (aipref)

Two working-group drafts, checked 5 September 2026:

- **Vocabulary** (`draft-ietf-aipref-vocab`, editor's copy of 25 August
  2026): categories `train-ai` and `search`; a statement gives each category
  allow, disallow or unknown; absent means unknown; §5.1 combines statements
  most-restrictive-wins; §5.2 says contracts override preferences; §3.2 says
  the specification does not decide whether preferences are followed.
- **Attachment** (`draft-ietf-aipref-attach-05`, 19 August 2026): a
  `Content-Usage` HTTP header on the response, and a `Content-Usage` rule in
  `robots.txt` groups with optional path prefixes, applying only to crawlable
  paths, using the `robots.txt` copy current at fetch time, cached up to 24
  hours.

Open pull requests at github.com/ietf-wg-aipref/drafts on the same date:
#250 adds a generative-AI-model definition that excludes classification,
ranking and scoring, narrows training to generative models, adds an AI-use
category ("using an asset as input to a generative AI model, where the asset
is not directly provided by the user", label `ai-use`), and says search
overrides other categories; issue #249 asks whether a URL counts as
"directly provided"; #252 separates acquisition from use (preferences govern
use, not fetching, and should be retained when the two happen in different
places); #253 says transforming an asset, embeddings included, does not erase
its preferences; #240 covers derived forms; #251 covers documenting
conformance.

How the edge reads it: the header from the page response, and the
`robots.txt` rules for its own token group, with the `*` group as the
fallback.

## IAB Tech Lab CoMP (Content Monetization Protocols)

v1.0, finalised 28 April 2026. An API between an AI system and a content
owner or marketplace: the AI system states who it is and what it intends; the
owner returns content packages with scope, a terms reference and a retrieval
method. Its own page says it is not a licensing system, not a marketplace and
not a blocking system. Relevant as a supply adapter, not as a signal on the
open web.

## C2PA

C2PA 2.4 (1 April 2026) defines three ways to attach a signed manifest to
text: Annex A.8, invisible Unicode variation selectors appended after the
text, surviving copy and paste; A.9, an ASCII-armoured block in a comment or
front matter carrying a URL or data URI; A.7, a `<script
type="application/c2pa">` or `<link rel="c2pa-manifest">` in HTML.

Libraries: `c2pa-rs` (the official Rust SDK, Apache-2.0 or MIT, 0.x; builds,
signs and reads manifests; the component pick in
[`open-source-landscape.md`](open-source-landscape.md)); `c2pa-text` (MIT, Rust crate 3.0.0 of 19 June
2026; embeds and extracts manifest bytes in text under all three annexes,
embedding only, taking JUMBF bytes from `c2pa-rs`); `encypher-ai` (AGPL-3.0
Python SDK, not usable in the open core); `c2patxt` (Python, an independent
A.8 implementation, useful as a second verifier).

Assertions: the CAWG training and data mining assertion 1.1 (ratified 16 May
2025, label `cawg.training-mining`) has entries `cawg.ai_inference`,
`cawg.ai_training`, `cawg.ai_generative_training` and `cawg.data_mining`,
each `allowed`, `notAllowed` or `constrained`; `constrained` may carry
`constraint_info`, which the specification says can be a URL to a policy
file, and is treated as `notAllowed` absent more information. The CAWG
metadata assertion binds any XMP or IPTC field, including rights fields.
Older `c2pa.`-prefixed training labels are replaced by these.

How the edge reads it: an embedded manifest from pasted text (A.8) or pasted
HTML (A.7), through the prompt hook, with the C2PA SDK; the preference from
the training-and-data-mining entries, `constrained` read as not allowed;
`constraint_info` recorded as a reference for the mediated path to follow,
because the hook makes no network request. A pasted image is not read: the
host's prompt hook carries text only. How the edge writes a manifest for its
own output is the `output-provenance` processor ([`docs/contracts/processor.md`](../contracts/processor.md)
§Status).

## Web Bot Auth

Signed HTTP requests from bots. Individual IETF drafts by Cloudflare and
Google: `draft-meunier-webbotauth-httpsig-protocol-02` (18 August 2026)
defines the signature using RFC 9421 HTTP message signatures, a
`Signature-Agent` header for key discovery, and a key directory at
`/.well-known/http-message-signatures-directory`;

`Signature-Agent` carries the origin, not the directory's own URL. The
architecture draft's §4.5 says the reference for discovery "is a FQDN. It
SHOULD provide a directory hosted on the well known registered in Section 4
of [DIRECTORY]", and its §4.2.4 example is `Signature-Agent:
"https://signer.example.com"`. A verifier appends the well-known path
itself, so a header carrying the full directory URL sends it to that path
twice and it finds nothing. This runtime sends the origin
(`crates/commonmeasure-harness/src/identity.rs`).

`draft-meunier-webbotauth-registry-03` (26 June 2026) defines a Signature
Agent Card describing identity, purpose, rate expectations and keys.
Cloudflare verifies bots this way (documentation updated 1 July 2026). The
keys are Ed25519, the type the Content Telemetry manifest carries in §8.4,
so one key can serve both.

Why it matters: a `robots.txt` token is free text and can be spoofed.
Publishers can safely allow `CommonMeasureBot` only where the request
signature verifies, and can check compliance by matching the
`Content-Telemetry-ID` on each signed request against the reports they
receive. The package is `ROADMAP.md` §Verified fetcher identity.

## Sources

- Content Telemetry v1.0: <https://contenttelemetry.org>; repositories
  `SPUR-Coalition/telemetry` and `SPUR-Coalition/telemetry-profile` on
  GitHub.
- RSL 1.0: <https://rslstandard.org>.
- Content Signals: <https://contentsignals.org>.
- aipref vocabulary editor's copy:
  <https://ietf-wg-aipref.github.io/drafts/draft-ietf-aipref-vocab.txt>;
  attachment draft:
  <https://www.ietf.org/archive/id/draft-ietf-aipref-attach-05.txt>; pull
  requests: <https://github.com/ietf-wg-aipref/drafts/pulls>.
- CoMP: IAB Tech Lab, Content Monetization Protocols v1.0.
- C2PA 2.4 and CAWG assertions: <https://c2pa.org>, <https://cawg.io>.
- Web Bot Auth drafts: <https://datatracker.ietf.org/doc/draft-meunier-webbotauth-httpsig-protocol/>,
  <https://datatracker.ietf.org/doc/draft-meunier-webbotauth-registry/>.
