---
title: Recognise and verify CommonMeasureBot
domain: network
audience: integrator
section: integrate
---

# Recognise and verify CommonMeasureBot

`CommonMeasureBot` is the name every Common Measure edge presents when it
fetches a page for an agent. It fetches what an agent asks for and does not
crawl. The name is shared; each enrolled edge signs its requests with its
own Ed25519 key under Web Bot Auth (HTTP Message Signatures, RFC 9421).
Trust a request by its signature, never by its `User-Agent`. The formats
are in [`docs/contracts/bot-identity.md`](../contracts/bot-identity.md).

## 1. Address it in robots.txt

The product token is `CommonMeasureBot`. The edge obeys `User-agent`,
`Allow` and `Disallow`, in every policy mode, and honours `Crawl-delay`.
Where no group names it, the `*` group applies.

```text
User-agent: CommonMeasureBot
Disallow: /private/
```

Served from a loopback site, that group refused a fetch of
`/private/staff` before the page was requested; the site logged only the
`robots.txt` request. The agent was told:

```text
refused before the crossing: http://127.0.0.1:8405/robots.txt disallows CommonMeasureBot at http://127.0.0.1:8405/private/staff: the `User-agent: CommonMeasureBot` group, which names CommonMeasureBot; rule `Disallow: /private/`, a literal path prefix. A `Disallow` binds in every policy mode: refused before the request.
```

A `robots.txt` that answers `429` or `5xx`, or cannot be reached, is
treated as RFC 9309 §2.3.1 says: the last copy the edge holds governs, and
with none held the edge refuses the page as a complete disallow.

## 2. What a signed request carries

| Header | Value |
|---|---|
| `User-Agent` | `CommonMeasureBot/<version> (+<bot page>)` |
| `Signature-Agent` | the origin of the hub the edge enrolled with, as a quoted string |
| `Signature-Input` | `sig1=("@authority" "signature-agent");created=…;expires=…;keyid="<key id>";alg="ed25519";nonce="…";tag="web-bot-auth"` |
| `Signature` | `sig1=:<base64>:` |

Where the request carries `Content-Telemetry-ID`, that header is covered
too. The page fetch, each redirect hop and the `robots.txt`, licence and
Content Telemetry manifest probes are signed; a signature lasts 300
seconds. For edges enrolled with Common Measure Hub, `Signature-Agent` is
`"https://hub.commonmeasure.ai"`, whose key directory, agent card and bot
page are at:

- `https://hub.commonmeasure.ai/.well-known/http-message-signatures-directory`
- `https://hub.commonmeasure.ai/.well-known/signature-agent-card.json`
- `https://hub.commonmeasure.ai/bot`

An edge that is not enrolled, or has learnt its key was revoked, sends
`User-Agent: CommonMeasureBot/<version>` and no signature. Such a request
cannot be told from anyone else using the name.

## 3. Verify a signature

1. Check that `Signature-Agent` names an origin you trust.
2. Fetch the key directory from that origin and find the key whose `kid`
   is the `keyid` in `Signature-Input`. No key means unknown or revoked.
3. Rebuild the signature base: one line per covered component, then the
   parameters. `@authority` is the host the request was sent to, lower
   case, with the port only where it is not the default.
4. Verify the Ed25519 signature over the base with the key's `x`.

This script does those steps for one request, using OpenSSL 3 for the
signature check. Save the request's headers as JSON (`Host`, which is
`:authority` in HTTP/2, `Signature-Agent`, `Signature-Input`, `Signature`, and any other header the
signature covers) and pass the origin you trust:

```python
#!/usr/bin/env python3
"""Verify one CommonMeasureBot request.
Usage: verify-bot.py <headers.json> <trusted origin>
headers.json holds the request's Host, Signature-Agent, Signature-Input and
Signature headers, and any other header the signature covers. The trusted
origin is the key directory's origin you accept. Needs OpenSSL 3."""
import base64, json, re, subprocess, sys, tempfile, time, urllib.request

h = {k.lower(): v for k, v in json.load(open(sys.argv[1])).items()}
_, params = h["signature-input"].split("=", 1)
components = re.findall(r'"([^"]+)"', params[: params.index(")") + 1])
keyid = re.search(r'keyid="([^"]+)"', params).group(1)
created = int(re.search(r"created=(\d+)", params).group(1))
expires = int(re.search(r"expires=(\d+)", params).group(1))
assert 'tag="web-bot-auth"' in params and 'alg="ed25519"' in params, "not a Web Bot Auth signature"
assert created - 60 <= time.time() <= expires, "outside the signature's validity window"

origin = h["signature-agent"].strip('"')
assert origin == sys.argv[2], f"signed for {origin}, not the origin you trust"
url = origin + "/.well-known/http-message-signatures-directory"
keys = json.load(urllib.request.urlopen(url))["keys"]
key = next((k for k in keys if k["kid"] == keyid), None)
assert key, f"key {keyid} is not listed at {url}: unknown or revoked"

values = {"@authority": h["host"].lower()}
lines = [f'"{c}": {values[c] if c in values else h[c]}' for c in components]
lines.append(f'"@signature-params": {params}')
base = "\n".join(lines).encode()

sig = base64.b64decode(h["signature"].split("=", 1)[1].strip(":"))
x = base64.urlsafe_b64decode(key["x"] + "==")
der = bytes.fromhex("302a300506032b6570032100") + x  # Ed25519 SubjectPublicKeyInfo
with tempfile.TemporaryDirectory() as d:
    for name, data in (("pub.der", der), ("base", base), ("sig", sig)):
        open(f"{d}/{name}", "wb").write(data)
    ok = subprocess.run(["openssl", "pkeyutl", "-verify", "-pubin", "-keyform", "DER",
                         "-inkey", f"{d}/pub.der", "-rawin", "-in", f"{d}/base",
                         "-sigfile", f"{d}/sig"], capture_output=True).returncode == 0
print(("verified" if ok else "SIGNATURE DOES NOT VERIFY") + f": key {keyid} at {origin}")
sys.exit(0 if ok else 1)
```

```sh
python3 verify-bot.py headers.json https://hub.commonmeasure.ai
```

The script was run against requests a signing edge made to a loopback site,
with the key directory served from a second loopback origin, not against a
live edge enrolled with the hub. A request as sent verified; the same
request with another `Host` did not; with the key removed from the
directory the script reported it unlisted; and against the wrong trusted
origin it refused before fetching anything:

```text
verified: key 759-Mmz_OXqWF8SmrNfiTnjwLCn9bw38Si84ne8rBsU at http://localhost:8499
SIGNATURE DOES NOT VERIFY: key 759-Mmz_OXqWF8SmrNfiTnjwLCn9bw38Si84ne8rBsU at http://localhost:8499
AssertionError: key 759-Mmz_OXqWF8SmrNfiTnjwLCn9bw38Si84ne8rBsU is not listed at http://localhost:8499/.well-known/http-message-signatures-directory: unknown or revoked
AssertionError: signed for http://localhost:8499, not the origin you trust
```

The directory is cacheable for an hour (`Cache-Control: public,
max-age=3600`); fetch it once per hour rather than per request. The
directory response also carries each listed key's own signature over the
directory's authority (`binding0`, `binding1`, …), which a verifier may
check as well (contract §The directory proof).

## 4. What a key id is

A key id names one enrolled edge. It is the RFC 7638 thumbprint of the
edge's public key: SHA-256 over `{"crv":"Ed25519","kty":"OKP","x":"<x>"}`
with no spaces, base64url without padding. You can recompute it from the
directory:

```sh
python3 -c 'import base64,hashlib,sys; print(base64.urlsafe_b64encode(hashlib.sha256(("{\"crv\":\"Ed25519\",\"kty\":\"OKP\",\"x\":\"%s\"}" % sys.argv[1]).encode()).digest()).rstrip(b"=").decode())' 11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo
```

```text
kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k
```

That is the RFC 8037 example key; the key listed in Common Measure Hub's
directory on 23 September 2026 matched its `kid` the same way. The key id
is also the `agent_id` of the usage reports that edge's activity produces
([Receive usage reports about your content](reports.md)).

## 5. Revocation

A key leaves the directory when an organisation owner revokes it at the
hub, when the edge runs `commonmeasure disconnect`, when the member it was
enrolled for leaves the organisation, or when the organisation closes. A
key also drops out when the edge's directory proof lapses. A verifier that
cached the directory can accept a revoked key for at most its cache age,
one hour. The edge learns of an owner's revocation at its next relay run
and then stops signing; until then its requests fail verification once
directory copies expire.

## 6. Raise a complaint about a key

Write to the contact in the hub's agent card (`contacts`), which the bot
page also gives. For Common Measure Hub it is `mailto:hello@commonmeasure.ai`
(read from the card on 23 September 2026). Quote the key id and the
requests concerned: time, URL, `User-Agent` and `Signature-Input`. To stop
one edge at once, refuse requests signed with its key id at your server; a
`robots.txt` group applies to every edge.
