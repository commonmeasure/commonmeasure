---
title: What an agent meets at the door
description: One AI agent, four web fetches, read from the record they left. Two pages were admitted, two were refused, and every decision has a reason a person can check.
---

# What an agent meets at the door

An AI agent that fetches a web page today meets nothing at the door. Its
tool requests the page, the page arrives, and the text goes into the
model's working memory. Nothing checks first whether the organisation
running the agent permits that source, nothing reads what the publisher has
said about automated use, and nothing writes down what came in. Afterwards
there is a transcript, if the host program keeps one, and no more.

Common Measure is a component an organisation places between its agents and
everything they read. Before each piece of content moves, it applies the
organisation's rules; whatever the decision, it records where the content
came from, what the source declared about it, and a fingerprint of exactly
what the agent read. This page follows one agent through four fetches under
that component and reads the record each one left. Two pages were admitted
and two were refused, and each decision carries a reason that a person can
verify without trusting the software or reading the agent's conversation.

## The words this page uses

A few terms recur, and each is defined in one line in the
[glossary](../GLOSSARY.md).

- A **crossing** is any moment content from outside enters an agent's
  working context: a page fetched, a search result read.
- A **mediated crossing** is one the agent requested through Common
  Measure's own fetch tool, so the rules could be applied before the
  content moved, and the crossing could be refused. The tool is
  `context_fetch`.
- **Admission** is the yes-or-no decision on whether fetched content may
  enter the model's context, made by deterministic rules.
- The **operator** is the organisation that runs the agent and owns the
  rules. Its rules are the **source policy**, a file on its own machine.
  An **access rule** is one ordered line in it: a host pattern and what
  happens to sources matching it. The first matching rule decides.
- The **evidence log** is the append-only file each session leaves, one
  JSON record per line. Together with the other artefacts a session
  produces it is the **source record**, the thing a person or a regulator
  checks afterwards.
- A **processor** is a pluggable check or transform that runs on content
  at a crossing and writes a record of its own each time it runs.
- The **edge** is the part of Common Measure that runs on the operator's
  machine. A **hub** is a hosted service an edge can enrol with; the edge
  in this account is not enrolled, which matters for one of the fetches.

## The setting

The agent runs in Claude Code, a program that hosts AI coding agents, with
Common Measure registered as the fetch tool. The operator's source policy
binds the working directory to a scope in strict mode, where anything the
rules do not allow is refused. The scope holds 32 ordered access rules.
Rules 1 to 31 allow named publisher hosts, `www.gov.uk` and `*.bbc.co.uk`
among them. Rule 32 has the pattern `*` and the action `refuse`, so any
host the first 31 rules do not name is refused. The smallest policy that
produces the same four outcomes is committed as `demo/policy/door.json`
(four rules, so its last rule is rule 4), and the
[getting started walkthrough](../GETTING-STARTED.md) runs the four fetches
under it.

The agent is asked to fetch four addresses through `context_fetch`. What
follows is quoted from the evidence logs of the two sessions in which those
fetches were made. Each quoted record shows the fields the discussion turns
on. The fields left out (timestamps, the working directory, the list of
blind spots every processor states about itself, and the policy digest) do
not change the reading, and no quoted value has been altered.

One fact about the edge applies to every fetch. It is not enrolled with a
hub, so it holds no key a publisher could verify. Each request went out
under the product's agent name, and the crossing records carry
`"user_agent": "CommonMeasureBot/0.2.0"` with an `unsigned` field stating
that the edge is not enrolled and holds no key a publisher can verify. A
publisher that admitted these requests admitted a name, not an identity.

## 1. gov.uk: admitted, with two hashes

The first address is `https://www.gov.uk/government/organisations`. Three
processors ran before the crossing was recorded. The first turned the page's
markup into readable text:

```json
{"event": "processor_invoked", "payload": {
  "processor": {"name": "html-text-extractor", "version": "1"},
  "stage": "transform",
  "decision": "admit",
  "method": "the body decoded as UTF-8 and its readable text extracted under the rules the configuration digest pins",
  "inputs": [{"reference": "https://www.gov.uk/government/organisations",
              "content_hash": "sha256:f3bd985bf80fc7a23dd998177bb1db27780028b14777888504d3f4b32a9cde3c"}],
  "outputs": [{"reference": "https://www.gov.uk/government/organisations",
               "content_hash": "sha256:1a18717c55ede85cf8fb906bbece35b391789e72adb01c2a161284beec407b1a",
               "tokens": 5264}],
  "detail": {"content_type": "text/html; charset=utf-8", "extracted": true,
             "bytes_received": 508306, "characters_delivered": 40559,
             "lossy_decoding": false, "token_basis": "whitespace-words"}}}
```

The second and third were the personal-data detector and the
prompt-injection screen, which judge the extracted text before the model
sees it. Each recorded `"decision": "admit"` with `"findings": []`.

Then the crossing itself:

```json
{"event": "crossing_mediated", "payload": {
  "mode": "mediated",
  "host": "claude-code",
  "url": "https://www.gov.uk/government/organisations",
  "host_name": "www.gov.uk",
  "content_hash": "sha256:1a18717c55ede85cf8fb906bbece35b391789e72adb01c2a161284beec407b1a",
  "retrieved_hash": "sha256:f3bd985bf80fc7a23dd998177bb1db27780028b14777888504d3f4b32a9cde3c",
  "estimated_tokens": 10140,
  "token_basis": "characters/4",
  "grounded": true,
  "http_status": 200,
  "licence": {"state": "unknown"},
  "declarations": {
    "robots": {"url": "https://www.gov.uk/robots.txt", "status": 200,
               "reading": {"group": "*", "crawlable": true, "statements": [], "licences": []}},
    "statements": [],
    "effective": {"train-ai": "unknown", "ai-input": "unknown", "ai-index": "unknown", "search": "unknown"},
    "governing": "statements"},
  "named_by": "unknown",
  "identity": {"user_agent": "CommonMeasureBot/0.2.0"},
  "policy_scope": "common-measure/demo",
  "principal": "os-user:501",
  "authentication_basis": "os_user"}}
```

Two hashes are on the crossing, and they are the point of it.
`retrieved_hash` is the SHA-256 of the 508,306 bytes the origin served, and
it equals the extractor's input hash. `content_hash` is the SHA-256 of the
40,559 characters of text the agent actually read, and it equals the
extractor's output hash. The two records therefore tie the bytes to the
text through a named processor with a pinned configuration. Anyone holding
the same page bytes can recompute both values and confirm that this text
came from those bytes, without trusting the edge, the console or the agent.

Two token figures appear, and they differ because they measure different
things by different rules. The extractor counted 5,264 whitespace-separated
words of the text it produced; the crossing estimates 10,140 tokens by
dividing characters by four. Each carries its basis in the record. Neither
is a tokeniser's count, and the record does not present either as one.

The licence state is `unknown`, and not `allowed`. The edge read
`https://www.gov.uk/robots.txt`, found that the group for all agents
permits the path, and found no statement about AI use and no licence. An
absent statement is recorded as unknown, never as permission. The edge also
looked for a discovery manifest at `/.well-known/content-telemetry.json`.
The one on `www.gov.uk` answered 404, and the probe of the apex host was
stopped by the operator's own rules: the manifest record's reason reads
`https://gov.uk/.well-known/content-telemetry.json is refused by policy:
access rule 32 (*) refuses host gov.uk.` The operator's rules govern every
request the edge makes, including the ones it makes on its own account.

## 2. people.com: admitted, and the terms it published were invisible

The second address is `https://www.people.com/`, a large consumer news
site. The extractor received 514,876 bytes of markup and delivered 13,392
characters of text, 2,208 words, with the input hash
`sha256:9beeb9302dc6ca243c2bfb0a57d2a5f6185f16aecfb9a4eb3ded803e2bdd4640`
and the output hash
`sha256:41355f9499ab11c3ea1937e44be3897c75f86ea8f24232736486b75a86ea30d4`.
The crossing was admitted:

```json
{"event": "crossing_mediated", "payload": {
  "mode": "mediated",
  "url": "https://people.com/",
  "host_name": "people.com",
  "content_hash": "sha256:41355f9499ab11c3ea1937e44be3897c75f86ea8f24232736486b75a86ea30d4",
  "retrieved_hash": "sha256:9beeb9302dc6ca243c2bfb0a57d2a5f6185f16aecfb9a4eb3ded803e2bdd4640",
  "estimated_tokens": 3348,
  "token_basis": "characters/4",
  "grounded": true,
  "http_status": 200,
  "licence": {"state": "unknown"},
  "declarations": {
    "robots": {"url": "https://www.people.com/robots.txt", "status": 200,
               "reading": {"group": "*", "crawlable": true, "statements": [], "licences": []}},
    "statements": [],
    "effective": {"train-ai": "unknown", "ai-input": "unknown", "ai-index": "unknown", "search": "unknown"},
    "governing": "statements"},
  "named_by": "unknown",
  "identity": {"user_agent": "CommonMeasureBot/0.2.0"}}}
```

The record says the publisher's `robots.txt` places this agent in the group
for all agents, that the group permits the page, and that the file makes no
statement about AI use and names no licence. That reading is correct, and
the file itself shows why it is also incomplete. Its rules give the group
for all agents two disallowed paths, `/embed?` and `/cdn-cgi/`, and nothing
else. Dozens of named agents, among them `GPTBot`, `ClaudeBot` and
`PerplexityBot`, are disallowed from the whole site by name. An agent the
file has never heard of falls into the group for all agents and is let in.

Above those rules the file opens with a notice, written as comment lines:

```text
#People Inc. content is made available for your non-commercial use subject to our
#Terms of Use here: https://www.people.inc/brands-termsofservice.
#Use of any crawler or other tool, device, or process to data mine or scrape the
#content on this website using automated means for any purpose other than
#directing traffic to this website or serving authorized advertisements on this
#website is prohibited without prior written permission from People Inc.
#Prohibited uses include but are not limited to:
#(1) text and data mining activities under Art. 4 of the EU Directive on
#Copyright in the Digital Single Market;
#(2) development or operation of any artificial intelligence, machine learning,
#or large language model (LLM) technology, including by training or fine-tuning
#such technology or using it for retrieval-augmented generation; and
#(3) creating data sets containing People Inc. content or sharing it with others.
#For the avoidance of doubt, the fact that any such tool, device, or process is
#not blocked via this robots.txt file does not constitute a waiver of any of
#People Inc.'s rights under our Terms of Use or relevant law.
```

The notice ends with the address of the publisher's content-licensing
desk. It prohibits exactly what this fetch did, and no agent can act on it,
because the format has no place for it. A comment in `robots.txt` is text
for a person. The machine-readable fields in the same file said the page
was open. The publisher's door admitted an unverified agent, and the
publisher's terms were invisible to it. The next fetch shows what changes
when a publisher states its terms in a form an agent can read.

## 3. The Guardian: refused on a term the licence stated and the edge could not meet

The third address is a Guardian article. The crossing was refused before
any request for the article was made:

```json
{"event": "crossing_refused", "payload": {
  "mode": "mediated",
  "url": "https://www.theguardian.com/politics/2026/sep/09/mr-congeniality-burnham-advises-badenoch-to-ditch-the-point-scoring-over-defence-spending",
  "host_name": "www.theguardian.com",
  "grounded": false,
  "licence": {"state": "unknown"},
  "refusal": "The licence https://theguardian.com/license.xml permits AI input under payment type subscription, and this edge holds no settlement rail, so the payment term is unmet.",
  "declarations": {
    "robots": {"url": "https://www.theguardian.com/robots.txt", "status": 200,
               "reading": {"group": "*", "crawlable": true, "statements": [],
                           "licences": ["https://theguardian.com/license.xml"]}},
    "licences": [{"url": "https://theguardian.com/license.xml", "mechanism": "robots-license",
                  "status": 200, "content": "/",
                  "terms": {
                    "statements": [
                      {"source": "rsl-licence", "category": "train-ai", "preference": "allow", "detail": "https://theguardian.com/license.xml: permits usage train-ai"},
                      {"source": "rsl-licence", "category": "ai-input", "preference": "allow", "detail": "https://theguardian.com/license.xml: permits usage ai-input"},
                      {"source": "rsl-licence", "category": "ai-index", "preference": "disallow", "detail": "https://theguardian.com/license.xml: no licence permits usage ai-index (licence 1)"},
                      {"source": "rsl-licence", "category": "search", "preference": "disallow", "detail": "https://theguardian.com/license.xml: no licence permits usage search (licence 1)"}],
                    "payment": {"kind": "subscription", "custom": "https://licensing.theguardian.com/"}}}],
    "effective": {"train-ai": "allow", "ai-input": "allow", "ai-index": "disallow", "search": "disallow"},
    "governing": "statements"},
  "named_by": "unknown"}}
```

The chain the record shows runs in four steps. The edge read the
publisher's `robots.txt` and found a `License:` line naming
`https://theguardian.com/license.xml`. It fetched that file, which is a
licence in the Really Simple Licensing format, a published standard in
which a content owner states which uses of its content are permitted and on
what payment terms. It read that the licence covering the whole site
permits AI input and AI training under a subscription, and permits neither
indexing nor search. Then it refused, because the edge holds no settlement
rail: no arrangement through which it could present a subscription or make
a payment, so the term the licence set could not be met.

Nothing about the article was fetched. The record carries no
`http_status`, no `retrieved_hash` and no `content_hash`, because there was
nothing to hash. The agent received the refusal as a tool error carrying
the same sentence, and the refusal is in the log with the licence it was
read from.

The refusal is not a verdict on the publisher. This is a licence an agent
can act on, and the agent acted on it. The same licence admits the page the
moment the operator holds the subscription and declares it in the source
policy as **terms**, an agreement the operator holds with a source, which
govern over the source's published statements.

## 4. The Economist: refused by the operator before anything was asked

The fourth address is `https://www.economist.com/`. The record is short:

```json
{"event": "crossing_refused", "payload": {
  "mode": "mediated",
  "host": "claude-code",
  "url": "https://www.economist.com/",
  "host_name": "www.economist.com",
  "grounded": false,
  "licence": {"state": "unknown"},
  "policy_scope": "common-measure/demo",
  "principal": "os-user:501",
  "authentication_basis": "os_user",
  "refusal": "access rule 32 (*) refuses host www.economist.com.",
  "named_by": "unknown"}}
```

This refusal came from the operator's own rules, not from anything the
publisher said. The host matched none of the 31 allow rules and fell to
rule 32, and the refusal names the rule by its position and its pattern so
that the operator can find the line that decided. The record holds no
declarations at all, because the edge never read the Economist's
`robots.txt` and never sent a request. When the operator's rules refuse a
host, nothing reaches that host.

## What the record claims, and what it does not

Every record above carries `"mode": "mediated"`. That is its grade: the
agent asked Common Measure to carry the crossing, so the rules ran before
the content moved. The product also records crossings it merely witnessed
after the fact, and crossings reconstructed from a transcript later, and
it never adds the three kinds together into one number.

The records hold identifiers and hashes, never the content. The hash of
what entered the model's context is the whole claim a session makes about a
source's bytes. Anyone who can obtain the same bytes can check it, and
nobody can recover the content from the record.

`"named_by": "unknown"` on every crossing means these sessions recorded no
prompt, so the record does not say whether a person or the agent chose each
address. Where a host reports prompts, that field records the answer.

The two screens that ran on the admitted pages name the rule each one
matched, and record their own blind spots beside each invocation. A clean
result means no rule matched, not that the page was safe.

Because the edge was not enrolled, no publisher could verify who was
asking. An enrolled edge signs each request with a key a publisher can
check against a published directory, so a site can admit the agent on the
signature rather than on the name.

Reading these records needed no access to the product. A person with the
log files answers every question this page raised: which addresses were
fetched, what each publisher declared, why each refusal happened, and what
text the model read, down to the byte.

## Where to go next

The [getting started walkthrough](../GETTING-STARTED.md) takes a clean
checkout through a session that records crossings of your own, a policy
that refuses one, and the console that shows it back. The
[session evidence contract](../contracts/session-evidence.md) defines every
field quoted above. The [glossary](../GLOSSARY.md) holds the terms.
