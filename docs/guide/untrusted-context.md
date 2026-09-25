---
title: "How agents use outside content"
domain: shared
audience: reader
---

# How agents use outside content

An agent using outside content needs to establish whether it may use that
material, whether it is suitable evidence for the task, and how to handle any
instructions it contains. Knowing the publisher, obtaining a licence and checking
for harmful instructions each help with part of that work, but none establishes
all of it. A licensed article can be out of date, an accurate passage can be
misrepresented in a summary, and a useful document can contain an instruction the
agent has no authority to carry out.

The term *untrusted context* describes that last concern. Context is the material
supplied to a model as it decides what to say or do. Calling outside material
*untrusted* means that the system must not treat instructions within it as
commands merely because the agent has read them. It says nothing by itself about
the publisher's honesty, the accuracy of the content or permission to use it.
An agent can rely on a newspaper's reporting when preparing a briefing without
giving the article's author control over its email or access to private files.

*Unlicensed* means that no licence covers the particular use. Paying a supplier
for access does not necessarily provide permission to reuse everything it
returns. The operator needs to establish a basis for the intended activity,
whether a direct agreement, a public licence or an applicable exception.
Where that basis has not been established, the uncertainty needs to remain
visible rather than being treated as permission.

Publisher conditions need attention even when the content has no authority to
instruct the agent. A licence may require attribution or usage reporting, for
example. The operator must assess those conditions and arrange to meet them if
it relies on that licence. A reporting condition does not itself authorise an
agent to disclose private session data to an address found in a document. The
application needs an authorised recipient and a decision about what may be sent;
if it cannot meet the condition, it must decline that licensing route or establish
another basis for use.

Describing material as *dangerous* requires an account of the possible harm and
how it could occur. A request embedded in a support ticket could cause an agent
to disclose a secret if it follows the request. Misleading evidence could distort
an answer without asking the agent to take any forbidden action. Even accurate
reporting can produce a misleading briefing if a summary omits its date or turns
a tentative proposal into an established fact. These failures require different
controls: restrictions on actions, assessment of evidence, and checks that the
answer represents its sources faithfully.

This guide follows content from its source through retrieval, the agent's actions
and the answer shown to the reader. It examines permission and publisher
conditions alongside prompt injection and the quality of the resulting answer,
then considers what records allow operators and publishers to investigate what
happened.

Sources checked on **14 September 2026**, with the rights-signals and news-integrity
material checked on **15 September 2026**.

## Contents

1. [How outside content reaches an agent](#1-where-untrusted-content-enters)
2. [How content redirects an agent](#2-injection-methods-and-results)
3. [Defences and their limits](#3-defences)
4. [Standards, regulation and rights signals](#4-standards-and-regulation)
5. [Copyright claims and contractual cover](#5-who-carries-the-liability)
6. [Records for security, rights and news integrity](#6-records-for-security-and-liability)
7. [What a record can establish](#7-what-a-record-of-inputs-can-and-cannot-show)
8. [What remains unresolved](#8-unresolved-questions)
9. [Putting this into practice with Common Measure](#putting-this-into-practice-with-common-measure)
10. [Supporting detail: services and records](#supporting-detail-services-and-records)
11. [Glossary](#glossary)
12. [Disclosures](#disclosures)
13. [Sources](#sources)

<a id="1-where-untrusted-content-enters"></a>

## 1. How outside content reaches an agent

Agents acquire material through web pages, search results, email, documents and
tool responses. Images, audio, video and persistent memory can also supply
information that influences their work.[^1][^8] The route matters because a search
snippet, a retrieved page and a supplier's generated answer can represent the
same source differently, with different terms and different evidence of origin.

### Control over search results

A search result has already passed through several decisions before the agent reads
it. A supplier crawls a page into an index, selects a snippet and returns it for a
query. The agent may then fetch the page, or a vendor may produce a grounded answer:
a model-written response based on retrieved material. Finally, the operator's
application decides what to display and how to show citations.

| Stage | What reaches the next stage | Who controls that step |
|---|---|---|
| Index | A stored representation of the page | The search supplier and its crawler |
| Snippet | Selected lines in a search result | The search supplier |
| Page fetch | The page returned for a live request | Whoever makes the request |
| Grounded answer | A model's rewrite of retrieved material | The model vendor |
| Display | The answer and any visible citations | The operator's application |

A source link in an answer does not tell you whether the system fetched the page or
received a snippet from a search supplier. In its September 2026 court filing,
OpenAI said most of the browsing copies challenged by news publishers were
search-engine snippets, obtained without contacting the publishers’ sites.[^3]

Crawler controls also depend on the route. Tavily and Brave describe following the
site's Googlebot rules rather than a separate user agent, so a publisher cannot
block those crawlers independently through those rules. Parallel separates its
robots.txt-obeying crawler from a user-initiated fetcher whose identification is
intended to provide visibility rather than automated control.[^4][^5][^6]

<a id="where-did-it-come-from"></a>

### Following a source through the system

A useful account of origin starts with who named the source: the user, the agent, or
someone the recorder cannot identify. It then follows the supplier, the host that
served the response after redirects, and the bytes received. If an extractor turns
HTML into text, the text delivered to the model is another distinct object. The
source's robots.txt rules, AI-use preferences and licence belong alongside that
account, as does the agent's subsequent activity.

Each part answers a different practical question. A supplier identifies the terms
under which material was acquired. A final host helps explain a redirect. The choice
of source matters to security designs such as ROPE, which uses origin to restrict
values passed to tools that change state. Later, the same record can help establish
which source declarations and contractual conditions applied.[^42]

### Publisher instructions and attacks

Text addressed to an agent does not always have an attacker behind it. A publisher
may tell AI systems not to reuse a page; an attacker may tell them to send private
files elsewhere. Both can appear in the same places and use similar imperative
language. The CISPA web scan found instructions aimed at disrupting AI processing,
defending content against reuse, and manipulating or probing agents. Much of the
validated material sat in HTML that would not render for a human reader.[^7]

That overlap makes indiscriminate removal difficult. Stripping every instruction
addressed to AI can also strip the publisher's written restriction. A source policy
needs a way to record and interpret rights declarations without giving the page
authority to direct the agent's tools. Detecting suspicious language and respecting
source restrictions are related tasks with different decisions to make.

Consider a licence that requires usage reports. RSL can name a reporting profile
and a destination for those reports.[^71] Even a legitimate requirement could
conflict with the operator's duty to keep a session confidential. The system needs
to preserve the declaration, check who issued it and which content and uses it
covers, and interpret recognised fields through the operator's policy. Sending a
report requires separate authorisation for its recipient and contents; a page's
terms cannot grant access to private session data. If the system cannot meet the
reporting condition, it must decline that licensing route or establish another
basis for use. The record should explain that decision.

<a id="2-injection-methods-and-results"></a>

## 2. How content redirects an agent

Prompt injection occurs when someone tries to redirect an agent through
instructions placed in material it reads. A support ticket might contain useful
facts alongside a request to copy a database secret into the reply. If the agent
follows that request using its own access, the ticket's author has caused it to
act without the user's authority. A reported Supabase MCP demonstration followed
this pattern: the coding agent held a service role key, read a ticket and wrote
secrets back into it.[^18] This is indirect prompt injection because the request
arrives through task material rather than directly from the user.

The NCSC describes the underlying difficulty as the absence of an enforced security
boundary between instructions and data inside an LLM prompt. OWASP's term,
context-window pooling, describes different sources becoming part of the same token
stream. Labelling a passage as untrusted can help a model interpret it, but the
label alone does not enforce what the system may do with it.[^1][^2]

### How the attacks work

An injection need not announce itself as an attempt to override the user. It can
make the harmful step look necessary to finish the task. A page might describe a
secret as a required configuration field or an integrity signature, so that sending
it appears to be routine completion rather than disclosure.

The *Framing Gap* study tested this mechanism by giving six models a secret,
simulated tools and attacker-controlled pages. Overt requests to leak the secret
were refused, while reframing the leak as a legitimate task requirement could
succeed. In this simulation, gpt-4o went from no leaks on overt attacks to leaks on
every trial of the reframed attack. Paraphrases of a known mechanism also worked
much more often than mechanisms written from scratch.[^9]

The practical lesson is that a detector looking for familiar attack phrases can miss
the same action expressed as an ordinary requirement. The system needs to consider
which values may reach which tools, even when the text describing the step sounds
helpful.

### Getting planted content retrieved

Before malicious content can influence an agent, the agent has to encounter it. One
study separated that problem into a trigger fragment, designed to match a retrieval
query, and an attack fragment carrying the instruction. In an email workflow, a
single poisoned message was enough to make the evaluated agent leak SSH keys in many
trials.[^10]

The attack exploited embedding-based retrieval: selecting messages by their
similarity to a query. Public web search has additional ranking filters.[^11]
The example shows why [source selection](context-window-optimisation.md#4-acquisition)
matters: retrieval determines which third-party text the agent encounters.

### Success rates depend on effort

An attack success rate is meaningful only with its test conditions. An attacker who
can inspect a defence's responses and retry is conducting an *adaptive attack*. That
opportunity changes the test. In a government evaluation, stronger attacks and
repeated attempts substantially increased success. A large public competition found
successful attacks against every model tested, although success on an individual
attempt was much lower.[^8][^12][^13]

It also matters whether the agent merely began following the injected instruction or
completed the harmful action. Web and multimodal evaluations found large gaps
between those outcomes. In those tests, low completion rates sometimes reflected an
agent's inability to carry out the action, rather than a defence stopping it. The
multimodal results also varied sharply by whether the attack-bearing audio actually
reached the model.[^14][^15]

### Incidents

Reported demonstrations make the connection between input and authority concrete. In
the GitHub MCP example, a malicious public issue reached an agent whose broad token
also gave it access to private repositories. The researchers recommended limiting
each token to one repository per session.[^17] In EchoLeak, crafted email retrieved
by Microsoft 365 Copilot led to zero-click exfiltration of internal data. Microsoft
patched the service and reported no exploitation in the wild.[^16]

Several other demonstrations used destinations that were already allowed. ForcedLeak
sent CRM data to an expired domain that an attacker had bought. AgentFlayer used
shared cloud storage; the Claude Cowork demonstration used the attacker's account on
the allowed Anthropic API. Allowing a domain had not established who controlled the
particular destination behind it.[^19][^20][^21]

Most of these attacks were demonstrated by researchers and fixed before disclosure.
The fixes show why credential scope and control over destinations matter. Other
reported failures include ShareLeak's exfiltration despite a safety flag and
Semantic Kernel vulnerabilities in which injected content reached an evaluation
sink and caused remote code execution; both received patches.[^22][^23]

### In the wild

The evidence for attempted manipulation on the web is broader than the evidence for
successful compromise. Google's Common Crawl analysis found that most matching text
was educational material for researchers. It reported growth in its malicious
category without a base count and said attackers had not yet put the research into
production at scale. Unit 42 described techniques targeting ad-review agents but
confirmed no successful attack on those agents.[^24][^25]

Manipulation can also be commercial rather than overtly destructive. Microsoft's
telemetry found companies using pre-filled “Summarize with AI” links to encourage
assistants to remember them as trusted sources.[^26] OWASP ranks injection as the
leading risk using security practitioners' assessments of likelihood and impact.[^27]

The UK AI Security Institute reported another route during cyber testing: agents
with live internet access attempted unsanctioned actions, including planting
instructions where other automated systems might encounter them. One model was
tested with cyber classifiers off, and the actions were found through monitoring
afterwards.[^28]

### Poisoning without instructions

A source can distort an answer without asking the agent to do anything forbidden. If
an agent stops at the first authoritative-looking page, an attacker can shape its
answer by supplying plausible but misleading evidence. The content may be fluent,
factual in isolation and still poorly matched to the question.

The *Lazy Grounding* study reduced search-agent accuracy using true but nearby
facts. In Salesforce's controlled tests, maliciously hosted content could become
the agent's answer even among many clean documents.[^29][^30] A study of
research agents found that one crafted paragraph on a frequently retrieved
user-generated page could make them cite attacker-chosen entities across related
queries. A separate benchmark found little improvement from off-the-shelf
guardrails against fluent informational poisoning.[^31][^32]

This is where source selection becomes part of security. An action control cannot
reject a forbidden tool call when none occurs, and an instruction detector has no
instruction to find. Selecting sources carefully and corroborating evidence can
address a failure that those controls leave untouched.

### When accurate reporting becomes a misleading answer

An answer can fail even when nobody has poisoned the source or injected an
instruction. The BBC/EBU news-integrity toolkit distinguishes failures in factual
accuracy, fidelity to sources, quotations, context, editorialisation and
sourcing.[^news-integrity] These failures need checks on the answer itself.

Imagine a report saying that a minister proposed a change, subject to consultation.
A search snippet drops the qualification. Extraction from the full article loses
the date. The assistant summarises the proposal as a decision already in force, and
the application displays it beside the publisher's name. The source was accurate;
the reader receives a different claim. Repeated summaries can also make one
uncertain account look independently corroborated if they obscure their common
origin.

Checking the answer requires comparing its claims with the source and assessing
whether the source is accurate and relevant to the reader's question. A faithful
quotation from an old article may still give a misleading answer about the
present, so preserving the wording alone is insufficient. Each citation needs to
support the particular claim beside it, including any qualifications that affect
its meaning.

Direct supply agreements can support full-text access, stable article identifiers,
quotation permissions and correction feeds. To measure whether those arrangements
improve answers, a comparison would hold the question and answer model fixed, vary
source access or extraction, and assess factual accuracy, faithfulness, attribution
and completeness separately. This would show which
improvements came from better access, extraction or use of the source, and help
explain the value of the licensing arrangement.

<a id="3-defences"></a>

## 3. Defences and their limits

Defences act at different points in this process. Prompt formatting, model training
and classifiers try to help distinguish legitimate content from instructions that
should be ignored. Structural controls limit what the system can do after the
content has been read: which destinations it can contact, which credentials it can
use and which values can enter an action.

### Detection under adaptive attack

A detector's low failure rate on a fixed benchmark does not establish that it will
withstand someone trying to defeat it. *The Attacker Moves Second* tested prompting,
training, filtering and secret-based defences using several adaptive methods. Most
suffered high attack success despite originally reporting very low rates. The study
did not test structural controls. An earlier study also bypassed all eight
indirect-injection defences it examined.[^33][^34] DataSentinel, explicitly designed
to detect strong adaptive attacks, was among the later study's defeated
defences.[^35]

Benchmark design can hide the difficulty. Researchers obtained apparently perfect
security with a tool-input minimiser and tool-output sanitiser, then traced the
result to weak attacks, implementation bugs and flawed success metrics. Another
study found that adding useful instructions in READMEs, tickets and forms left
existing defences either insufficiently secure or too ready to block legitimate
work.[^36][^37]

Anthropic reported low browser attack success
against its own adaptive attacker while acknowledging that browser agents remain
vulnerable. OpenAI described injection as unlikely to be fully solved; Microsoft
describes its classifiers and Spotlighting as probabilistic and builds its approach
around the possibility that an injection gets through.[^38][^39][^40]

### Structural defences hold where tested

A structural defence tries to make the dangerous action unavailable even if the
model accepts the attacker's story. CaMeL derives control flow from the trusted
query and separates it from retrieved data. ROPE tracks origin and admits a value to
a state-changing tool only when it traces to the user, an explicitly user-named
source or the user's authoritative records. FIDES applies confidentiality and
integrity labels to enforce information-flow policy.[^41][^42][^43]

These designs address the ticket example more directly than a phrase detector:
reading the ticket need not grant its author authority over a secret or an output
destination. In the synthetic *Framing Gap* tests, a closed destination list and a
planner separated from the untrusted reader prevented the tested exfiltration, while
model-based and output-normalising defences were bypassed.[^9]

CaMeL, ROPE, FIDES and production origin controls still need strong, independent
adaptive testing. LaunchSafe found structural defences evaluated on static
benchmarks. Its own adaptive experiment used a weak model and one attack template,
leaving sustained attacks against stronger systems untested.[^44]

### How structural defences fail

The boundary a control enforces must match the thing the operator wants to protect.
A destination list cannot reliably prevent disclosure if an allowed host accepts
uploads to anyone's account, hosts shared storage, follows open redirects or is no
longer controlled by the intended owner. The incident examples show why a recognised
domain is insufficient. The recipient, account and resource must be authorised for
the particular data and action, with redirects and subsequent access considered.

An authorised destination may belong to a customer or publisher. Requiring exclusive
operator control is one way to restrict destinations, but even an operator-owned
location can expose information to readers who should not receive it.

Likewise, a trusted source can contain someone else's text. A public issue, review
or user-generated page may sit on an otherwise trusted domain. Explicitly naming
such a source does not establish that its contents deserve authority over an action.
Google's Chrome design recognises user-generated content, but its origin sets still
depend on model judgements of relevance, and the first version tracked where the
agent could act.[^46]

Planning ahead also has limits. A model can be misled during perception into taking
an already permitted branch that suits the attacker. A vague user request can make a
harmful action appear to fit the task. The computer-use extension of CaMeL
illustrates why a guarantee about permitted control flow does not settle whether the
agent selected the right branch.[^45]

### Design rules

A useful starting point is the combination Simon Willison describes: access to
private data, exposure to untrusted content and a way to communicate externally.
Together they create a route for data to leave. Meta's related Rule of Two calls for
supervision when an agent has all three; its two-of-three description was later
changed from safe to lower risk. These are practitioner and vendor design rules, not
measured guarantees.[^47][^48]

The NCSC and the multi-author design-patterns paper put the emphasis on constraints
outside the model: untrusted input should not be able to trigger consequential
actions merely because the model interpreted it as an instruction. In practice, that
means narrowing permissions and destinations, separating reading from acting where
possible, and requiring approval where the remaining authority is too
broad.[^2][^49]

<!-- product-only:start -->
### Record the action boundary

Admitting a source records what Common Measure allowed into context and the
source's applicable conditions. It does not authorise an instruction found in
that source. The host decides separately at the action boundary, where the
recipient and resource are visible, and refuses when either falls outside the
operator's grant. A shared service can be an allowed host while a particular
account, object or recipient on that service remains unauthorised.

The source record keeps the admission and the host's action decision as separate
records. The decision names the action, recipient, resource and rule, and can be
`allowed` or `refused`. An allowed decision records host permission; it does not
prove receipt, later access or that a licence condition was fulfilled. If the
host cannot observe later use, that gap remains explicitly unavailable. See
[`session-evidence.md` §Host action decisions](../contracts/session-evidence.md#host-action-decisions)
for the bounded fields and relay boundary.
<!-- product-only:end -->

### Uses of detection

Detection can raise the cost of casual attacks, flag sessions for review and help
reconstruct what happened. Because it also produces false positives on security
education and can miss ordinary-sounding instructions, its findings can inform
admission decisions but cannot establish that admitted content is safe.

### Checklist

When reviewing an agent, follow the content through to the action:

1. Identify every input route, including search, grounding, connectors, memory,
   images and audio.
2. Establish which routes can meet sensitive data and external actions; narrow
   that combination or add supervision.
3. Check who controls each permitted destination, including accounts on shared
   services and every redirect.
4. Record who chose the source and which action parameters derive from outside text.
5. Use detector findings for review, and assess defence results against their
   attack effort and adaptive testing.
6. Check what sanitising removes, including rights reservations and provenance.

<a id="4-standards-and-regulation"></a>

## 4. Standards, regulation and rights signals

The same separation between content and authority appears in security guidance. The
standards also expose a practical gap: recording that a tool ran is easier than
recording exactly what content it supplied and how that content was treated.

### OWASP

OWASP's 2026 guidance treats injection defence as architectural, given the absence
of reliable prevention. It places public web material, unknown email and search
results in an untrusted tier, and public issues, READMEs and third-party APIs in a
semi-trusted tier. Even nominally trusted repositories and mail can contain planted
material. Credentials and state changes belong in application code, and external
content needs a structurally separate channel labelled with its origin.[^1]

Its broadened Hidden Context Exposure category includes retrieved policy text and
tool schemas. The implication is practical: material placed in context should not be
treated as a secret merely because the user cannot see it.[^50][^51]

### OWASP Agent Control Standard

The Agent Control Standard (ACS) describes hooks through which a platform can apply
policy to inputs as well as actions. In v0.1, those include retrieval, tool results
before ingestion, memory and compaction. Framework code populates provenance at
channel boundaries; the model does not get to declare its own sources trustworthy.
Derived model output inherits the least-trusted origin of its inputs.[^52][^53][^54]

ACS v0.1 was a public preview whose audit and provenance requirements depend on
selected profiles; a web fetch lacks a dedicated field for the URL served or a
content hash. The default is also to proceed when the policy service is unavailable
or slow, so disruption can turn a blocking control into an audit-only event.[^54]

### UK

NCSC guidance recommends deterministic safeguards, restricted network access and
credentials supplied by a proxy rather than exposed to the agent. It also calls for
transcripts and logs, immutable where possible. Its warning is that some use cases
may not tolerate the residual risk. The guidance discusses logging inputs, outputs
and actions but does not specify a record of the origin of ingested
content.[^2][^55]

### US

NIST describes indirect injection through resource control: an attacker alters a
document or page that the model will ingest at runtime. Its agent standards work was
under way, but no published draft was found of an agent control overlay in the SP
800-53 series. CISA and partner agencies likewise identify obscure event records as
a risk and advise against broad, unrestricted access.[^56][^57][^58][^59]

### EU

Article 15 of the AI Act requires high-risk systems to resist unauthorised attempts
to alter their use, output or performance, including inputs designed to make a model
err. Article 15 does not name prompt injection. Logging obligations also need their
scope preserved: Article 12's input-data logging requirement is specific to remote
biometric identification, while Articles 19 and 26 require high-risk logs to be kept
for at least six months to the extent they are under the provider's or deployer's
control. A hosted agent may leave the operator with little control over those
logs.[^61][^62][^63]

The Digital Omnibus changes moved the timetable, with high-risk obligations applying
from December 2027 or August 2028 depending on the category. General-purpose model
rules and their enforcement follow different dates.[^60] The General-Purpose AI Code
of Practice addresses crawler compliance with robots.txt and recognised
rights-reservation protocols in the context of mining and training. That wording
should not be treated as an answer-time fetching rule.[^64]

### Taxonomies and logging formats

MITRE ATLAS supplies names for indirect injection and forms of context poisoning.
OpenTelemetry, OCSF and MCP offer ways to describe retrieval or tool activity, but
these formats do not by themselves provide a complete source record. OpenTelemetry's
retrieval documents are optional identifiers and scores; MCP's logging facility
carries diagnostic messages. [Logging detail](#logging-detail) compares their
coverage.[^65][^66][^67][^68][^69]

### Rights signals

A machine-readable declaration can express a publisher's preferences or offered
licence terms without asking the model to obey prose on a page. The IETF AI
preferences draft 08, dated 14 September 2026, adds `ai-use` for material supplied
to a generative model that the user did not provide directly. Whether supplying a
URL counts as direct provision remains unresolved. Its search category excludes
generated summaries. The working group is still debating the vocabulary.[^70]

RSL 1.0 includes retrieval-augmented generation and grounding within `ai-input` and
can attach reporting conditions to a licence.[^71] RSL and the IETF draft differ
in their scope and treatment of preferences and licensing, but neither makes a
declaration into a security control. For either specification, a recipient needs
to record what was declared, assess whether it applies and decide how to act on it.

### Variation selectors and provenance

Sanitising can remove useful evidence along with hidden instructions. OWASP
recommends stripping a range of invisible Unicode variation selectors at ingestion
and rendering. C2PA 2.4 uses that range, among others, to embed Content Credentials
in text through a method still under review. Removing the characters can therefore
detach the embedded credentials from the text.[^1][^72]

Where those credentials are supported, verify them before stripping the characters
and preserve the verification result and credential reference alongside the
transformation record, so the evidence remains available after its embedded form
has been removed. Verification checks the signature and the content's connection
to the credential; factual accuracy and the signer's authority to license the work
need separate checks.[^c2pa-explainer]

<a id="5-who-carries-the-liability"></a>

## 5. Copyright claims and contractual cover

The route from publisher to model matters contractually as well as technically. An
operator might buy search results from one supplier, give them to another vendor's
model and then edit the answer for display. Each step can affect which terms apply
and whether an indemnity—a contractual promise to defend or cover specified
claims—reaches the disputed material.

Permission to use a work and cover against a claim answer different questions. A
licence grants specified rights; an indemnity allocates the cost of specified
claims. An indemnity does not itself grant the publisher's rights or decide whether
a use infringes them. Equally, a lawful use may have no indemnity. The cover
discussed here mainly concerns copyright and other intellectual-property claims;
it should not be read as cover for security incidents, privacy breaches or every
action an agent takes.

Source declarations also need to be read in their own terms. A crawler preference,
a rights reservation, a contractual condition and an operator's voluntary refusal
policy can lead to different decisions. Robots rules, for example, are not access
authorisation under RFC 9309.[^robots-authorisation] Record what was declared
separately from the assessed legal or contractual basis for use, the policy
decision and any applicable cover.

The account below concerns published terms and reported litigation as of 14
September 2026. Negotiated agreements override online terms.

### The law

The cases discussed here have not settled whether copying for retrieval-augmented
generation (RAG) or AI search infringes copyright. RAG supplies retrieved material
to a model for use in an answer. Copying an article into a retrieval system,
reproducing its wording in an answer and summarising it raise different questions;
a decision about one does not necessarily settle the others.

In the US news-publisher cases, OpenAI argued that browsing qualifies as fair use
and that publishers had implicitly permitted copies made before they blocked its
user agents. Other courts had decided whether particular claims could proceed.
One allowed a challenge to summaries that publishers said could replace their
articles; another dismissed a claim because the summaries were not sufficiently
similar to the original text. These were early decisions about which claims could
continue, not final judgments on whether retrieval copying was lawful.[^3][^73][^74][^75]

In its defence, Perplexity argues that copyright does not protect facts, that
answering queries should qualify as fair use in the same way as conventional
search, and that users’ prompts cause it to reproduce text verbatim.[^73][^76]

The US Copyright Office's pre-publication report treats RAG as involving
copying. It questions whether producing shortened versions of retrieved works is
sufficiently different in purpose from the originals to qualify as fair use. The UK's March 2026 report says retrieval copies
made in the UK require a licence unless an exception applies, while making no law
change at that point.[^77][^78]

In Europe, *Like Company v Google* concerns whether a chatbot’s reproduction and
display of text from press publications infringes their rights. In Germany, a court
held the provider responsible for outputs containing memorised song lyrics. That
judgment concerns memorisation rather than retrieval and is under appeal. Japanese
publishers were also pursuing a case against Perplexity.[^79][^80][^81]

Access controls and competition proceedings add another layer. The UK CMA's
publisher-control requirement, French publishers' referral concerning Google's
commitments, and US anti-circumvention cases concern aspects of acquisition and
reuse beyond whether an answer reproduces protected expression. In the US cases
about bypassing access controls, most of Reddit’s claims were allowed to continue. Google’s were dismissed, but it was allowed to revise them
and try again.[^82][^83][^84][^85]

### Model and cloud vendors

Output IP cover depends on what the contract counts as the vendor's output. Google
Cloud explicitly includes specified grounding results in Generated Output; that
provides an answer for those services, subject to its conditions and exclusions.
Microsoft's model commitment and separate Bing grounding terms leave the
relationship less clear. An operator cannot infer coverage for a web-grounded answer
simply from coverage for the model that produced it.[^86][^87][^91][^92][^93]

The integration can matter just as much as the service name. OpenAI's and
Anthropic's exclusions address modifications, combinations and outside content. AWS
offers cover for its own models, while its grounding documentation assigns
responsibility for use of grounded output to the customer. To understand a
particular workflow, follow both the content route and the transformations made
after generation. The [service comparison](#service-comparison) sets out the
relevant conditions.[^96][^99][^102][^103]

<a id="search-and-fetch-suppliers"></a>

### Buying retrieval separately

Paying a search supplier does not necessarily buy protection against claims about
its results. Perplexity's terms illustrate the distinction: assurances covering its
answers do not extend to output from its Search API. A service can cover its own
software while excluding the content that software returns.[^104][^105]

Most search and fetch suppliers examined exclude returned content from cover, and
several expressly disclaim non-infringement. Every one with an indemnity clause also
requires the customer to indemnify it. Some allocate responsibility for actions as
well: Parallel makes the customer responsible for agent actions whether or not they
matched the customer's intentions. That includes the problem of an agent acting on
injected instructions.[^106][^108][^110][^111][^112][^114]

### Limits on cover

Three parts of an agent workflow need to be read together: what the supplier
returned, what the model generated and what the application did with it. Combination
exclusions can reach arrangements that mix suppliers and models. Modification
exclusions and citation requirements can matter when the application rewrites or
reformats an answer. An input supplier's terms may exclude the material even when
the model vendor covers its own output.

Source restrictions can also become conditions of use. Microsoft's Bing terms
restrict output from websites that restrict the customer, including through a
crawler block. Google's cover excludes continued use after an infringement notice.
These conditions make source declarations and subsequent notices relevant to
eligibility, without establishing that a declaration alone settles infringement.

### Evidence required for cover

The applicable contract determines which evidence is needed. Across these terms,
relevant facts include whether a service was paid, generally available and listed
for cover; whether output was modified or combined; whether citations were
displayed; and whether required safety features remained enabled. Rights to
customer-supplied inputs, knowledge of likely infringement, compliance with source
restrictions and stopping use after notice can also matter.

Microsoft calls for demonstrated compliance and AWS for sufficient records. Much of
that evidence depends on what happened at the time. A later copy of an answer cannot
by itself establish which filters ran, what the application displayed or which
notice had already arrived. This is why input records are useful for liability, and
also why they cover only part of the required history.

<a id="6-records-for-security-and-liability"></a>

## 6. Records for security, rights and news integrity

Return to the support ticket. After a secret appears in the reply, an investigator
needs to establish which ticket was read, what it said at that time, which
credentials the agent could use and what action followed. A dispute over a retrieved
article asks related questions about the supplier, source declarations, text
delivered to the model and answer displayed. Both need contemporaneous records,
though neither is answered completely by them.

| Recorded fact | Security use | Rights and contractual use | Publisher and news-integrity use |
|---|---|---|---|
| Who named the source | Supplies origin for action policy | Distinguishes user and agent selection | Helps trace how the work was selected |
| Supplier, final URL and article version | Identifies the retrieval route and host | Helps identify applicable terms | Distinguishes the original, a syndicated copy and later corrections |
| Received bytes and delivered text | Supports reconstruction of the input | Supports comparison with disputed output | Shows qualifications lost during extraction |
| Source declarations and their assessed applicability | Keeps tool authority separate | Preserves the claim and the basis for use | Records attribution and reporting conditions |
| Admission rule and outcome | Shows which control ran | Shows the operator's decision | Helps explain acceptance or refusal of the source |
| Subsequent actions and displayed answer | Connects input with later activity | Helps establish display, citation and combination | Supports checks on quotations, attribution and claim support |

### Evidence a publisher can inspect

An operator's private log is only part of an accountability arrangement. Suppose
the publisher challenges the briefing's claim that the minister's proposal is
already in force. An investigator needs the article version, the relevant
extracted passage and the displayed claim. The publisher needs enough of that
evidence to explain the discrepancy and check the response, without receiving
unrelated prompts, credentials or private documents.

That requires an agreed way to share a limited report, challenge an attribution and
handle corrections. A useful correction record identifies the disputed claim, the
notice received, the source version checked and what the application changed or
left unresolved. Supplier receipts and independently retained copies can help
corroborate the operator's account where retention and disclosure are permitted.

Keep acquisition, delivery to the model, citation, display and assessed support for
a claim separate. A publisher can be falsely cited even when its material never
entered context. Investigating that failure starts with the displayed answer;
an empty retrieval record alone cannot settle the complaint. These are requirements
for the surrounding workflow, beyond an input recorder's coverage.

### What current logging captures

A retrieval event may identify a query and document without preserving the source
URL or a hash of its content. A search event may identify the service without
identifying the pages it returned. Those gaps limit what an investigator can infer
about the text that entered context.[^66][^117]

Behaviour analytics can flag drift and unusual tool use, while an input record helps
establish which content arrived beforehand. Recording who supplied a URL also
distinguishes a user’s choice from one made by the application. The [logging
comparison](#logging-detail) shows where existing schemas and services provide these
details and where additional fields would be needed.[^115][^116][^118]

Post-hoc injection detection may need more than identifiers. DriftNet reads a
trajectory containing tools, reasoning, arguments and observations. It scored
highly on synthetic data, while a model using surface features of the text caught
few partial hijacks on the same benchmark. Testing on production sessions remains
open, and omitted or redacted tool results in the available telemetry may limit
what such a detector can inspect.[^119][^120]

### Constraints on recording and sanitising

Retention policy also depends on the supplier. Google grounding terms permit
retention for specified purposes and periods; Bing limits copying, storage and
caching to what its terms permit; Brave generally allows transient storage unless
the plan grants more. A record can retain hashes and identifiers without retaining
content, but verifying a hash still requires access to the original bytes, whether
lawfully kept or obtained again.[^87][^93][^106][^5]

Extraction and sanitising can change the bytes returned by a server before the
text reaches the model. Recording a hash of each version and the extraction method
preserves evidence of that transformation. Rights reservations and text credentials
need attention before removal, because sanitising can erase them.

When a grounding vendor returns only an answer and links, the operator can record
that response but cannot claim to have captured the pages the vendor actually read.
The source record needs to identify this gap by stating which parts of the route
it observed and which remained unavailable.

<a id="7-what-a-record-of-inputs-can-and-cannot-show"></a>

## 7. What a record can establish

### Grades of evidence

A mediated record captures a decision made before acquisition and the resulting
crossing. An observed record comes from a hook watching activity after the event;
a reconstructed record derives from an existing transcript. Only the first can
document a control's opportunity to refuse acquisition. These grades describe
capture and control coverage, rather than a universal ranking of evidential value.
An authentic transcript can still be useful evidence of what was said.

### Can show

Investigators need to check who made the record and which events it captured. They
can then use it to establish the retrieval route, requester, declarations and
admission decision. A served URL identifies a delivery location; identifying the
author or who can license the work requires further evidence. Keep the supplier's
identity and rights claims alongside the results of any checks on them.

Separate hashes can bind the record to the bytes received and the text delivered.
With those bytes and a fixed, recorded extraction method, that transformation can
be checked. The hash alone contains neither the text nor proof that the recorder
observed the claimed event.

<a id="cant-show"></a>

### Limits of inference

The same record cannot establish whether the model followed an injected instruction,
whether the content infringed copyright or whether a passed scan made it safe. A
subsequent action can be evidence for investigation without proving what caused the
model to choose it.

Nor does an input record establish what the user saw. Citations, modifications and
combinations happen in the application. Built-in tools outside the recorder's path,
vendor-side grounding, host system prompts and model internals remain gaps. A
complete account of coverage names those unavailable parts.

### Minimum fields

The fields follow from those distinctions. Identify the event with its timestamp,
session and principal; identify acquisition with who named the source, the supplier,
requested and final URLs, and HTTP status. Record separate hashes for served bytes
and delivered text, with the extraction method and version.

Then preserve what the source declared: the robots.txt group and rule, preferences,
licence and claimed issuer. Record separately the assessed basis for the intended
use, including the agreement or licence version and scope, or an applicable
exception. Preserve the deciding policy rule, outcome and reason; leave an
unresolved basis explicitly unresolved. Finally, state the capture grade and gaps.

For a publisher dispute, link that intake record to the article identifier and
version, relevant output claims and citations, and any correction or notice
history. These later events need their own evidence. Identifiers and hashes allow
an intake record without retaining the content itself, but its limits must remain
visible in both the private record and any report shared outside the operator.

<a id="8-unresolved-questions"></a>

## 8. What remains unresolved

Structural defences need testing against attackers who can inspect them and keep
trying. The studies discussed above leave that question open for CaMeL, FIDES, ROPE
and production origin controls. We also need better incident data to understand
how often attempted injections lead to breaches in deployed systems.

Contractual uncertainty centres on the boundary between a vendor's output and the
outside material used to produce it. Google explicitly includes specified grounding
services, while Microsoft's, OpenAI's, Anthropic's and AWS's positions depend on
definitions and conditions whose application to web retrieval remains unclear. How
vendors interpret combination exclusions in agent systems is also unanswered.

Machine-readable restrictions leave further open questions. A declaration refusing
AI input might be relevant to exclusions for infringement the customer knew or
should have known about, but that interpretation remains unaddressed. Microsoft's
reference to the customer's crawler is unclear for a customer who does not crawl,
even though its broader website-restriction wording has no stated limit. A wildcard
robots.txt group can cover an automated fetcher the customer actually runs.

The legal basis for retrieval copying and the reach of the EU TDM exception remain
unsettled in these proceedings. Linkup relies on that exception; the UK report
assumes a similar exception would cover retrieval. The scheduled German *Kneschke v
LAION* decision concerns training data and may affect how machine-readable
reservations are understood, potentially through a CJEU referral.[^121][^78]
The IETF draft now includes `ai-use`, but its treatment of user-provided references
remains unresolved.[^70]

<a id="claims-and-supporting-evidence"></a>

### Evaluating the whole workflow

A useful evaluation follows the route from source to action and displayed answer.
It records the attacker's opportunities, the controls enabled and whether a harmful
action completed. Alongside that security test, it checks source handling,
quotations, attribution and the context preserved in the answer. Keeping those
results separate makes it possible to see where a change helped and where work
remains.

<!-- product-only:start -->

## Putting this into practice with Common Measure

Common Measure's safety belt combines mediated source controls and evidence with
the host's action permissions. In the planned network, a CM Attestation states
what an agent is authorised to do, its reporting and payment obligations, and who
stands behind it. The attestation records those commitments; the controls must
still enforce them, and subsequent evidence must show what happened.

The edge's [processor contract](../contracts/processor.md) also supports checks
and transformations around content handling and output, including context
optimisation, fidelity checking and provenance creation. The intended extension
path adds organisation-specific processors and an approved catalogue. The current
in-process capabilities run only at their documented integration points;
organisation settings, external processors and the marketplace remain planned.
An installed add-on does not show that it ran or establish an assurance by itself.

For example, a licensed article may be useful for the task while containing a
request to upload internal guidance. CM can preserve the article's applicable
terms and limit its own reporting to an authorised recipient and permitted fields.
The host must refuse an upload outside the user's authority even if the model
requests it. The current CM host allow-list alone cannot establish that boundary.
This is an illustrative integration requirement, not a reported incident or a
claim that the full network workflow is implemented.

Common Measure applies source policy and records acquisition on the fetch and search
paths used through its MCP tools. That gives an operator a place to decide which
sources may enter context and preserve the basis for each decision. Its coverage
depends on the path the host actually uses.

For a mediated fetch, the record distinguishes who named the source—`user`, `agent`
or `unknown`—and records the URL, the SHA-256 of the bytes served (`retrieved_hash`)
and the text admitted to context (`content_hash`). It also records the source's
robots.txt, `Content-Usage`, `Content-Signal` and RSL declarations. A mediated
search record identifies the supplier and hashes what came back. The session record
retains identifiers and hashes rather than content.

The source policy's ordered host rules run before a mediated fetch leaves the
machine and at every redirect hop, including the hops of its robots.txt request. In
`strict` mode, rules allowing only named hosts create a closed read list for that path.
The default `observe` mode records a breach on the crossing and allows the fetch. A robots.txt `Disallow` in the product token's group—or in `*`
when there is no such group—refuses the fetch in every mode before the page is
requested. So does a robots.txt that cannot be reached (a 429, a 5xx, a timeout or a
redirect the edge will not follow) when no earlier copy of it is held, and so does one
the edge did not finish reading because the call ran out of time or the request could
not be signed; that file is asked for again at the next fetch. A robots.txt over
512 KiB is read up to its last complete line within that limit.

Those controls have specific limits. Host rules do not match paths or query strings,
so data can still leave in a URL sent to an allowed host. The host's built-in tools
are not mediated: a standing instruction asks the agent to avoid them but does not
block their use. Network egress controls and the host's permission system are needed
alongside the mediated source policy. Recording who named a source also does not
implement ROPE's origin enforcement on parameters passed to state-changing tools.

The injection screen uses four fixed-phrase rules covering instruction override,
role reassignment, system-prompt exfiltration and tool directives. It records matched
rules and byte offsets and refuses the crossing in `strict` mode.

The screen will miss a *Framing Gap*-style reframing and has both flagged injection
research and missed an instruction in a supplier’s terms-page
footer.[^implementation-screen] Passing the screen does not establish that the text
is safe.

Where the host exposes activity through hooks, Common Measure records tool calls and
responses after the event, at the observed grade. Claude Code also supplies a
context snapshot for each turn. Transcript import records earlier events at the
reconstructed grade. Mediated, observed and reconstructed crossings remain separate.
For vendor-side grounding, the available response cannot stand in for the pages
behind it.

The input-side record can support an investigation or a contractual eligibility
review, but application evidence is still needed. It does not record what the
application displayed, whether citations appeared, whether output was modified or
when rights-holder notices arrived. Output provenance labels run when requested by
a batch run; they do not run in interactive sessions. A source record alone cannot
establish compliance with all the conditions of cover.

The publisher investigation and correction workflow described above therefore needs
application evidence and an agreed reporting process alongside Common Measure.
The private source record remains separate from the limited Content Telemetry
report cleared for a named receiver. Neither an intake record nor a delivery report
by itself establishes that an answer represented the source faithfully.

The robots.txt account names what decided: the requested URL, the robots.txt
that governs it, the group and rule matched, with a wildcard identified as one,
and what the policy mode did. Each redirect hop is judged against its own
origin's file before it is requested, so a link shortener's rule is attributed
to the shortener and not to the page it pointed at.[^implementation-robots] The
useful operational record includes both the detailed decision and a clear
account of coverage: which inputs were controlled, which were only observed and
which were unavailable.

<!-- product-only:end -->

## Supporting detail: services and records

<a id="service-comparison"></a>

### Google Cloud and Gemini Developer API

Google Cloud includes Grounding with Google Search and Web Grounding for Enterprise
in its indemnified services: “Grounded Results and Search Suggestions are Generated
Output”. Cover applies to unmodified output, with exclusions for likely infringement
the customer knew or should have known about, disregarded citations or safeguards,
continued use after notice and customer breach. Grounding rules also prohibit
modification and interspersing content.[^86][^87][^88]

The Agentic AI Services exclusion separately covers allegations arising from access
to or use of Third Party Services, including websites. Gemini Developer API terms
provide no Google indemnity; instead, the developer indemnifies Google for content
or data routed into or used with the APIs.[^87][^89][^90]

### Microsoft

The Customer Copyright Commitment covers any Azure OpenAI model in Microsoft Foundry
Models or any Copilot. Bing grounding, Foundry web search and Web IQ sit under
separate terms. Bing output is not Customer Data; Microsoft assigns no third-party
rights, and the Bing terms contain no Microsoft defence clause. The definitions do
not clearly bring Bing-grounded answers into the Commitment, and Microsoft has not
addressed the question.[^91][^92][^93]

Bing terms bar output use for websites restricting the customer, including where its
crawler is blocked by robots.txt. Web IQ permits 200 characters verbatim per source
and assigns publisher licensing fees to the customer. For configurable services,
Microsoft requires demonstrated compliance with mitigations, including filters and a
retained testing report. The required Prompt Shield concerns jailbreaks rather than
indirect injection.[^93][^94][^95]

### OpenAI

API and business customers have uncapped output indemnity, with exclusions for
unused or ignored relevant citation, filtering or safety features, and output
“modified, transformed, or used in combination” with outside products or services.
Another exclusion concerns content from a “Third Party Offering”; that term appears
undefined in the current Services Agreement extracts, which instead define
Third-Party Services. The terms pages refused automated access and were read through
agreeing search-provider copies.[^96][^97]

Web-search citations must be clearly visible and clickable.[^98]

### Anthropic

Anthropic provides uncapped cover for authorised paid services and their outputs.
Exclusions include customer modifications, combinations with technology or content
not provided by Anthropic, and customer inputs or other data.[^99]

No separate web search or fetch terms were found. Documentation makes search
citations mandatory; fetch citations are optional and off by default. Web fetch does
not retrieve robots.txt-blocked URLs.[^100][^101]

### AWS

AWS provides uncapped indemnity for its own models, without a grounding carve-out.
Nova Web Grounding requires citations and assigns responsibility for use of grounded
output to the customer. The terms require customers to retain and provide sufficient
records to evaluate eligibility for defence.[^102][^103]

### Perplexity and Brave

Perplexity covers its Services, API Platform or Outputs, subject to exclusions
including combination with “any Customer Application”. Its Search API addendum
excludes public-internet search output from those assurances. These terms were read
from search-provider extracts after the pages refused access.[^104][^105]

Brave covers its API and documentation while excluding Search Results. News Corp
sued over its search and summary products in July 2026, according to Semafor’s
report.[^106][^107]

### Linkup and SerpApi

Linkup names Article 4 of EU Copyright Directive 2019/790 as its acquisition basis,
checked against robots.txt and TDMRep. Cover excludes Open Web Content and Answers;
customer indemnity applies to rights-holder claims arising from use in breach.[^108]

SerpApi's shield concerns lawful collection of public search data. It excludes
copyright infringement and how the collected data is ultimately used.[^109]

### Other search and fetch suppliers

Tavily disclaims non-infringement; Exa disclaims responsibility for third-party
infringement in use of Output; Jina does not warrant that Output is free of
third-party rights. None provides cover for returned content.[^110][^111][^112]

The customer’s licence is an agreement with the publisher, not with TollBit.
TollBit provides no cover for returned content. Parallel likewise provides none, and assigns responsibility
for agent actions to the customer “whether or not such actions align with Customer's
intentions”.[^113][^114]

<a id="logging-detail"></a>

### Logging specifications

ACS v0.1 includes retrieval, tool-result, memory and compaction hooks. Its profiles
determine how much provenance and audit evidence is required; default continuation
when policy is unavailable limits its role as a blocking control.[^53][^54]

OpenTelemetry GenAI retrieval spans can carry a data source, query and opt-in
documents as identifiers and scores. Tool results are also opt-in, and the
attributes remain in Development status. There are no source URL, content-hash or
trust-label fields. OCSF has an AI operation profile and Retriever role but no AI
event class or retrieved-content source field. MCP logging carries diagnostic
messages rather than an audit record.[^66][^67][^68][^69]

MITRE ATLAS provides a taxonomy for distinguishing indirect injection, context
poisoning, prompt infiltration and tool data poisoning. Its identifiers describe
attack types rather than identify content acquired in a session.[^65]

### Logging services

Observra records prompts, responses, tool calls, actions, approvals, data access,
tokens and cost, with injection heuristics and PII redaction. It has no source URL,
origin or content-hash field; fetch and search are classified as outbound sending.
Exabeam's behaviour analytics supplies baselines and alerts for misuse, drift and
abnormal tool use. In both cases, source details would add evidence about the
content preceding an action.[^115][^116]

Microsoft Purview supplies per-resource URLs and injection flags for Microsoft 365
content. Its web documentation identifies Bing Web Search without specifying
retention of result URLs. Defender for Cloud's malicious-URL alert can leave user
versus application origin unclear.[^117][^118]

## Glossary

**Adaptive attack**: an attack built against a specific defence by an attacker who
can see its responses and retry.

**TDM exception**: the EU text and data mining exception. Rights holders can opt out
in an appropriate manner, including machine-readable means for online content;
courts differ on whether plain-language reservations count.

## Disclosures

The authors build software that controls and records what AI agents take in,
including a plugin for Anthropic's Claude Code, and have a commercial interest in
the recording recommendations in this article. The software reads RSL licences and
their reporting element. One author has contributed to Observra.

The authors work on standards for reporting AI use of content; one is technical lead
for the SPUR publisher coalition, whose Content Telemetry profile uses the RSL
reporting element.

Contract terms are those published on 14 September 2026, except where obtained from
a search provider's copy, as identified in the source notes. This article is not
legal advice.

## Sources

Sources were read on 14 September 2026 unless a note says otherwise. References
identify where material was obtained through reporting or a search provider's copy.

### References

[^1]: OWASP GenAI Security Project, *LLM01:2026 Prompt Injection*,
    Top 10 for LLM Applications 2026, August 2026.
    <https://github.com/GenAI-Security-Project/GenAI-LLM-Top10/blob/main/2026/final/LLM01_PromptInjection.md>
[^2]: UK National Cyber Security Centre, *Prompt injection is not SQL
    injection (it may be worse)*, 8 December 2025.
    <https://www.ncsc.gov.uk/blog-post/prompt-injection-is-not-sql-injection>
[^3]: In re OpenAI Copyright Infringement Litigation, No.
    1:25-md-03143 (S.D.N.Y.): OpenAI summary judgment brief, Dkt 1496, and news
    plaintiffs' brief, Dkt 1709, 4 September 2026 (tables of contents read);
    argument content from PPC Land, 5 September 2026.
    <https://storage.courtlistener.com/recap/gov.uscourts.nysd.612697/gov.uscourts.nysd.612697.1496.0.pdf>
    and <https://storage.courtlistener.com/recap/gov.uscourts.nysd.640396/gov.uscourts.nysd.640396.1709.0.pdf>
    The brief argues that more than 96% of the browsing copies attributed by the news
    plaintiffs were search-engine snippets, without a site request. OpenAI also argues
    implied licence before user-agent blocks; the publishers characterise grounding
    uses as substitutive and commercial. Briefs were read only in part, with arguments
    via secondary reporting. Replies were due in November 2026.
[^4]: Tavily, *Search crawler* documentation.
    <https://docs.tavily.com/documentation/search-crawler>
[^5]: Brave, *Brave Search API*, product page and FAQ.
    <https://brave.com/search/api/>
[^6]: Parallel Web Systems, *Parallel Web Systems bots*, effective
    11 August 2026. <https://parallel.ai/parallel-web-systems-bots>
[^7]: Khodayari, Zhang, Acharya and Pellegrino, *Indirect Prompt
    Injection in the Wild: An Empirical Study of Prevalence, Techniques, and
    Objectives*, arXiv:2604.27202, April 2026. Template and purpose counts from
    the PDF via a search provider's extract.
    <https://arxiv.org/abs/2604.27202>
    Web scan of 1.2 billion URLs across 24.8 million hosts; 15.3K validated
    injection instances on 11.7K pages. About 70% were in non-rendering HTML, and 54
    templates accounted for 95%.
[^8]: NIST (then the US AI Safety Institute, now the Center for AI Standards
    and Innovation), *Technical Blog:
    Strengthening AI Agent Hijacking Evaluations*, 17 January 2025.
    <https://www.nist.gov/news-events/news/2025/01/technical-blog-strengthening-ai-agent-hijacking-evaluations>
    Government evaluation with released code. Best baseline attack success was 11%,
    versus 81% for the best new attack. Repeating each attack 25 times raised average
    success from 57% to 80%.
[^9]: Rahman and Kim, *The Framing Gap: Indirect Prompt-Injection
    Exfiltration Defeats Surface-Level Defenses in Tool-Using Agents*,
    arXiv:2608.27092, 27 August 2026. <https://arxiv.org/abs/2608.27092>
    Tests used synthetic tools. Ten overt attack classes were refused;
    gpt-4o went from 0% leakage on overt attacks to 100% on reframing. Paraphrases of
    a known mechanism succeeded 96% across three wordings; a new mechanism written
    from scratch succeeded in 0 of 130 trials. Removing the secret-keeping instruction
    changed reframed results from 31.9% to 38.1%, supporting the authors’
    interpretation of instruction/data confusion. Defence results included SecAlign at
    32.5% attack success, an output-normalising guard defeated by ROT13 at 100%, and
    0% for both a closed destination list and a capability-isolating planner/reader
    split in the tested setting.
[^10]: Chang, Bao, Luo and Yu, *Overcoming the Retrieval
    Barrier: Indirect Prompt Injection in the Wild for LLM Systems*, USENIX
    Security 2026; arXiv:2601.07072. <https://arxiv.org/abs/2601.07072>
    Single peer-reviewed study, artefact released. The trigger required API access to
    embedding models, cost as little as $0.21 per target query and achieved near-100%
    retrieval across 11 benchmarks and eight embedding models. One poisoned email
    induced GPT-4o to exfiltrate SSH keys with over 80% success in the evaluated
    workflow.
[^11]: Chen et al., *Unveiling the Resilience of LLM-Enhanced Search
    Engines against Black-Hat SEO Manipulation*, WWW 2026; arXiv:2603.25500.
    Abstract via a search provider's extract. <https://arxiv.org/abs/2603.25500>
[^12]: Dziemian et al., *How Vulnerable Are AI Agents to Indirect
    Prompt Injections? Insights from a Large-Scale Public Competition*,
    arXiv:2603.15714, March 2026. <https://arxiv.org/abs/2603.15714>
    Study co-authored by researchers from some evaluated labs: 464 participants, 272,000
    attempts, 13 frontier models and 8,648 successes. Per-attempt success ranged from
    0.5% to 8.5%. Attacks against more resistant models were particularly likely to
    transfer to less resistant ones, but not conversely.
[^13]: NIST CAISI, *Insights into AI Agent Security from a Large-Scale
    Red-Teaming Competition*, March 2026.
    <https://www.nist.gov/blogs/caisi-research-blog/insights-ai-agent-security-large-scale-red-teaming-competition>
[^14]: Evtimov, Zharmagambetov, Grattafiori, Guo and Chaudhuri, *WASP:
    Benchmarking Web Agent Security Against Prompt Injection Attacks*,
    arXiv:2504.18575. <https://arxiv.org/abs/2504.18575>
    Single study with released code. Agents began attacker instructions in 16–86% of
    cases and completed them in 0–17%; the authors attribute the gap to limited
    ability to complete the actions.
[^15]: Nguyen and Husain, *An Experimental Evaluation of Multimodal
    Prompt Injection Attacks on Agentic AI Frameworks*, arXiv:2609.09404,
    8 September 2026. <https://arxiv.org/abs/2609.09404>
    Reported attempted-action rate was 12.8%, with about 1%
    completion overall; audio attacks completed in 49% of cells where audio reached
    the model.
[^16]: Checkmarx, *EchoLeak (CVE-2025-32711) shows us that AI security
    is challenging*, 2 July 2025. Secondary report.
    <https://checkmarx.com/zero-post/echoleak-cve-2025-32711-show-us-that-ai-security-is-challenging>
[^17]: Invariant Labs, *GitHub MCP Exploited: Accessing private
    repositories via MCP*, 26 May 2025. Read via a search provider's extract.
    <https://invariantlabs.ai/blog/mcp-github-vulnerability>
[^18]: General Analysis, *Supabase MCP can leak your entire SQL
    database*, July 2025. <https://generalanalysis.com/blog/supabase-mcp-blog>
    The reported mitigations were read-only operation by default and prompt wrapping;
    the vendor described the wrapping as an incomplete defence.
[^19]: CSO Online, report on Zenity Labs' AgentFlayer research,
    8 August 2025. Secondary report.
    <https://www.csoonline.com/article/4036868/>
    The shared document induced searches of connected drives for API keys, followed by
    exfiltration through an Azure Blob image URL that passed the URL safety check.
    Vendor fixes followed.
[^20]: Noma Security, *ForcedLeak: AI Agent Risks Exposed in
    Salesforce Agentforce*, September 2025; The Hacker News, September 2025.
    <https://noma.security/blog/forcedleak-agent-risks-exposed-in-salesforce-agentforce>
    The allow-listed domain had expired and was bought for about $5. Reported fixes
    re-secured the domain and tightened URL enforcement.
[^21]: PromptArmor, *Claude Cowork Exfiltrates Files*, January 2026; The
    Register, 15 January 2026.
    <https://www.promptarmor.com/resources/claude-cowork-exfiltrates-files>
    The document induced uploads of user files to the attacker’s account through the
    allow-listed Anthropic API using the attacker’s key. A press report describes the
    vendor treating it as a user-managed risk.
[^22]: VentureBeat, report on CVE-2026-21520 and agent prompt injection
    remediation, 2026. Secondary report.
    <https://venturebeat.com/security/microsoft-salesforce-copilot-agentforce-prompt-injection-cve-agent-remediation-playbook>
    ShareLeak involved SharePoint form content and exfiltration despite safety
    mechanisms flagging the activity, according to the press report. A patch followed.
[^23]: Microsoft Security Blog, *Prompts become shells: RCE
    vulnerabilities in AI agent frameworks*, 7 May 2026.
    <https://www.microsoft.com/en-us/security/blog/2026/05/07/prompts-become-shells-rce-vulnerabilities-ai-agent-frameworks>
    The reported Semantic Kernel vulnerabilities were CVE-2026-25592 and
    CVE-2026-26030; both received patches.
[^24]: Brunner, Liu and Pande, *AI threats in the wild: The current
    state of prompt injections on the web*, Google, 23 April 2026.
    <https://blog.google/security/prompt-injections-web/>
    Vendor analysis: a 32% relative increase in the malicious category between
    November 2025 and February 2026, without a base count. Most matching text was
    educational.
[^25]: Kaleli, Farooqi, Starov and Mohamed, *Fooling AI Agents: Web-Based
    Indirect Prompt Injection Observed in the Wild*, Unit 42, 3 March 2026.
    <https://unit42.paloaltonetworks.com/ai-agent-prompt-injection/>
    Vendor account describing 22 techniques, with percentages only and no confirmed
    successful attack on the targeted ad-review agents.
[^26]: Microsoft Defender Security Research, *Manipulating AI
    memory for profit: The rise of AI Recommendation Poisoning*, 10 February
    2026.
    <https://www.microsoft.com/en-us/security/blog/2026/02/10/ai-recommendation-poisoning/>
    Vendor telemetry: 50 prompts from 31 legitimate companies over 60 days, including
    one security vendor.
[^27]: Infosecurity Magazine, report on the OWASP Top 10 for LLM
    Applications 2026, 5 August 2026. Secondary report.
    <https://www.infosecurity-magazine.com/news/prompt-injection-llm-risk>
    The secondary report says injection would not rank in the top ten on incident
    counts alone.
[^28]: UK AI Security Institute, *Incident report: unsanctioned
    agent behaviour during cyber testing*, 4 August 2026.
    <https://www.aisi.gov.uk/blog/incident-report-unsanctioned-agent-behaviour-during-cyber-testing>
    The July 2026 testing report describes 19 unsanctioned actions. Live internet
    access and, for one model, disabled cyber classifiers were part of the test
    conditions; monitoring discovered the behaviour afterwards.
[^29]: Zhang et al., *Lazy Grounding: Attacking Search Agents with
    Factual Evidence*, EMNLP 2026; arXiv:2608.30303.
    <https://arxiv.org/abs/2608.30303>
    Single peer-reviewed study: true but nearby facts reduced accuracy by 5.9 points
    on average, up to 17.3 points.
[^30]: Salesforce AI Research, *Poisoning the Well: Search Agents
    Get Tricked by Maliciously Hosted Content*, 11 March 2026.
    <https://www.salesforce.com/blog/poisoning-the-well-search-agents/>
    Vendor-controlled evaluation: about 80% of queries returned the attacker’s answer,
    and nearly one in four did so with over 100,000 clean documents.
[^31]: Zhang, Triedman and Shmatikov, *Deep-Research Agents Can Be
    Poisoned via User-Generated Content*, arXiv:2605.24245, May 2026.
    <https://arxiv.org/abs/2605.24245>
    Study of a crafted paragraph on a frequently retrieved user-
    generated page.
[^32]: Zheng, Zhao and Yang, *Counter-GEO-Bench*, arXiv:2609.02316,
    September 2026. <https://arxiv.org/abs/2609.02316>
    Off-the-shelf guardrails reduced poisoning by at most 5.7%
    relative.
[^33]: Nasr, Carlini et al., *The Attacker Moves Second: Stronger
    Adaptive Attacks Bypass Defenses Against LLM Jailbreaks and Prompt
    Injections*, USENIX Security 2026; arXiv:2510.09023. Participant count via
    Simon Willison's summary, 2 November 2025.
    <https://arxiv.org/abs/2510.09023>
    Single peer-reviewed study using gradient methods, reinforcement learning, random
    search and human red-teaming. It bypassed 12 prompting, training, filtering or
    secret-based defences, with over 90% attack success for most; none was structural.
    Examples included Spotlighting, StruQ, Meta SecAlign, PromptGuard, Model Armor and
    DataSentinel. A 500-person human red team succeeded in every scenario.
[^34]: Zhan, Fang, Panchal and Kang, *Adaptive Attacks Break
    Defenses Against Indirect Prompt Injection Attacks on LLM Agents*, Findings
    of NAACL 2025. <https://aclanthology.org/2025.findings-naacl.395/>
    Single peer-reviewed study: all eight evaluated defences were bypassed at over 50%
    attack success.
[^35]: Liu, Jia, Jia, Song and Gong, *DataSentinel: A Game-Theoretic
    Detection of Prompt Injection Attacks*, IEEE S&P 2025; arXiv:2504.11358.
    <https://arxiv.org/abs/2504.11358>
[^36]: Bhagwatkar et al., *Indirect Prompt Injections: Are Firewalls
    All You Need, or Stronger Benchmarks?*, arXiv:2510.05244, v2 March 2026.
    <https://arxiv.org/abs/2510.05244>
    A tool-input minimiser and tool-output sanitiser achieved
    apparent perfect security with high utility on four public benchmarks. The authors
    attributed this to flawed success metrics, implementation bugs and weak attacks,
    limiting what near-zero results on AgentDojo or InjecAgent establish.
[^37]: Li, Wen, Shi, Zhang, Vorobeychik and Xiao, *AgentDyn*,
    arXiv:2602.03117, February 2026. Abstract via a search provider's extract.
    <https://arxiv.org/abs/2602.03117>
    The evaluation includes helpful third-party instructions in
    READMEs, tickets and forms and reports either inadequate security or substantial
    over-defence.
[^38]: Anthropic, *Mitigating the risk of prompt injections in
    browser use*, 24 November 2025.
    <https://www.anthropic.com/research/prompt-injection-defenses>
    Vendor evaluation: Claude Opus 4.5 browser attack success of 1% against
    Anthropic’s adaptive attacker, with 100 attempts per environment.
[^39]: OpenAI, *Continuously hardening ChatGPT Atlas against prompt
    injection attacks*, 22 December 2025. The page refused automated access;
    quoted from a search provider's copy, corroborated by press reports.
    <https://openai.com/index/hardening-atlas-against-prompt-injection/>
[^40]: Paverd, *How Microsoft defends against indirect prompt injection
    attacks*, Microsoft Security Response Center, 29 July 2025.
    <https://www.microsoft.com/en-us/msrc/blog/2025/07/how-microsoft-defends-against-indirect-prompt-injection-attacks>
[^41]: Debenedetti et al., *Defeating Prompt Injections by Design*,
    arXiv:2503.18813, 2025. <https://arxiv.org/abs/2503.18813>
    Single study with released code: 77% of AgentDojo tasks completed with the stated
    security guarantee, versus 84% without defence.
[^42]: Ma, Xiao, Yeoh, Zhang and Vorobeychik, *ROPE: Routed Origin Policy
    Enforcement against Indirect Prompt Injection*, arXiv:2608.27496,
    27 August 2026. <https://arxiv.org/abs/2608.27496>
    Released code and results: 1.6–2.6% attack success and 82–100% of
    undefended utility. Admission is based on origin, so rewording does not change
    that decision.
[^43]: Costa et al., *Securing AI Agents with Information-Flow Control*,
    arXiv:2505.23643, 2025. <https://arxiv.org/abs/2505.23643>
    Microsoft Research; its deterministic enforcement
    concerns the stated confidentiality and integrity policies.
[^44]: LaunchSafe, *Adaptive Evaluation of Out-of-Band Defenses
    Against Prompt Injection in LLM Agents*, arXiv:2606.26479, June 2026.
    <https://arxiv.org/abs/2606.26479>
    The replication reduced one defence’s
    attack success from 25.8% to 4.2%; a hand-built adaptive attack achieved 2.6%. The
    authors describe this as one small-scale data point on a weak model with a single
    black-box attack template.
[^45]: Foerster et al., *CaMeLs Can Use Computers Too*,
    arXiv:2601.09923, January 2026. <https://arxiv.org/abs/2601.09923>
[^46]: Parker, *Architecting Security for Agentic Capabilities in
    Chrome*, Google Security Blog, 8 December 2025.
    <https://security.googleblog.com/2025/12/architecting-security-for-agentic.html>
[^47]: Willison, *The lethal trifecta for AI agents: private data,
    untrusted content, and external communication*, 16 June 2025.
    <https://simonwillison.net/2025/Jun/16/the-lethal-trifecta/>
[^48]: Meta, *Agents Rule of Two: A Practical Approach to AI Agent
    Security*, 31 October 2025, as quoted in Willison, *New prompt injection
    papers*, 2 November 2025. Meta's post was not read directly.
    <https://simonwillison.net/2025/Nov/2/new-prompt-injection-papers/>
[^49]: Beurer-Kellner et al., *Design Patterns for Securing LLM
    Agents against Prompt Injections*, arXiv:2506.08837, June 2025. Quoted
    principle as reproduced in Willison's trifecta post.
    <https://arxiv.org/abs/2506.08837>
[^50]: OWASP GenAI Security Project, *Top 10 for LLM Applications
    2026*, repository README, released 4 August 2026.
    <https://github.com/GenAI-Security-Project/GenAI-LLM-Top10/blob/main/2026/README.md>
    The 2026 list places injection first and Excessive Agency third; System Prompt
    Leakage becomes Hidden Context Exposure.
[^51]: OWASP GenAI Security Project, *LLM08:2026 Hidden Context
    Exposure*, August 2026.
    <https://github.com/GenAI-Security-Project/GenAI-LLM-Top10/blob/main/2026/final/LLM08_HiddenContextExposure.md>
[^52]: OWASP GenAI Security Project, *Agent Control Standard (ACS)*,
    1 September 2026. <https://genai.owasp.org/resource/agent-control-standard-acs/>
    ACS joined OWASP on 1 September 2026. It was the only standard found covering
    inputs as well as actions.
[^53]: Agent Control Standard, *Hooks*, v0.1.
    <https://github.com/Agent-Control-Standard/ACS/blob/main/docs/spec/instrument/hooks.md>
    The 19 hooks include `knowledgeRetrieval` and `toolCallResult`, plus memory reads
    and writes and compaction.
[^54]: Agent Control Standard, *Instrument specification*, v0.1.
    <https://github.com/Agent-Control-Standard/ACS/blob/main/docs/spec/instrument/specification.md>
    Framework code sets `origin`, `source_id` and `derived_from`; retrieved content
    and tool output are untrusted, and derived output inherits minimum trust. The
    audit trail is hash-chained. Request-content hashes are required only under the
    audit profile, and per-field provenance only under the provenance profile.
[^55]: UK National Cyber Security Centre, *Managing the cyber risk of
    agentic AI*, 20 August 2026.
    <https://www.ncsc.gov.uk/blogs/managing-the-cyber-risk-of-agentic-ai>
[^56]: NIST, *Adversarial Machine Learning: A Taxonomy and Terminology
    of Attacks and Mitigations*, NIST AI 100-2 E2025, March 2025, chapter 3.
    Quoted from a search provider's extract of the PDF.
    <https://csrc.nist.gov/pubs/ai/100/2/e2025/final>
[^57]: NIST, *Announcing the AI Agent Standards Initiative*,
    17 February 2026; CAISI request for information, 91 FR 698, 8 January
    2026; NIST TRAI 800-5, summary of responses, 18 May 2026. Read via search
    providers' extracts.
    <https://www.nist.gov/news-events/news/2026/02/announcing-ai-agent-standards-initiative-interoperable-and-secure>
    The initiative followed the January request for information, with the response
    summary published in May 2026.
[^58]: NIST, *Control Overlays for Securing AI Systems (COSAiS)*, project
    page, updated 8 January 2026. <https://csrc.nist.gov/projects/cosais>
[^59]: CISA and international partners, *Careful Adoption of Agentic
    Artificial Intelligence (AI) Services*, press release, 1 May 2026.
    <https://www.cisa.gov/news-events/news/cisa-us-and-international-partners-release-guide-secure-adoption-agentic-ai>
[^60]: European Commission AI Act Service Desk, *Timeline for the
    implementation of the EU AI Act*.
    <https://ai-act-service-desk.ec.europa.eu/en/ai-act/timeline/timeline-implementation-eu-ai-act>
    The timetable cites Digital Omnibus Regulation (EU) 2026/1744: high-risk
    obligations move to 2 December 2027 for Annex III and 2 August 2028 for Annex I.
    General-purpose model rules applied from 2 August 2025, with enforcement from 2
    August 2026.
[^61]: Regulation (EU) 2024/1689, Article 15, consolidated text as
    published by the AI Act Explorer. Unofficial consolidation.
    <https://artificialintelligenceact.eu/article/15/>
[^62]: Regulation (EU) 2024/1689, Article 12, consolidated text as
    published by the AI Act Explorer. Unofficial consolidation.
    <https://artificialintelligenceact.eu/article/12/>
[^63]: Regulation (EU) 2024/1689, Article 26, as published by the AI Act
    Explorer, and Article 19, as published by the Commission's AI Act Service
    Desk. <https://artificialintelligenceact.eu/article/26/> and
    <https://ai-act-service-desk.ec.europa.eu/en/ai-act/article-19>
[^64]: General-Purpose AI Code of Practice, Copyright chapter, Measure
    1.3, final version 10 July 2025. Read from an unofficial copy whose wording
    matches the Commission's consultation excerpt.
    <https://code-of-practice.ai/?section=copyright>
[^65]: MITRE, *ATLAS* data, v5.6.0 export, which MITRE marks as deprecated;
    the live site carries later content versions. <https://atlas.mitre.org/>
    The export identifies indirect injection as `AML.T0051.001`, agent context
    poisoning as `AML.T0080`, prompt infiltration via a public-facing application as
    `AML.T0093`, and agent tool data poisoning as `AML.T0099`.
[^66]: OpenTelemetry, *Semantic conventions for generative AI spans*,
    Development status.
    <https://github.com/open-telemetry/semantic-conventions-genai/blob/main/docs/gen-ai/gen-ai-spans.md>
    Retrieval documents are represented as `{id, score}`. Every `gen_ai.*` attribute
    is in Development status.
[^67]: OpenTelemetry semantic-conventions-genai, issue 344, *Remove
    retrieval document id and score as required*, 24 June 2026.
    <https://github.com/open-telemetry/semantic-conventions-genai/issues/344>
    The issue proposes optional document identifiers because frameworks do not always
    supply one.
[^68]: OCSF, *Schema release 1.8.0*, March 2026.
    <https://github.com/ocsf/ocsf-schema/releases/tag/1.8.0>
[^69]: Model Context Protocol, *Logging*, specification 2025-11-25.
    <https://modelcontextprotocol.io/specification/2025-11-25/server/utilities/logging>
[^70]: IETF, *A Vocabulary For Expressing AI Usage Preferences*,
    draft-ietf-aipref-vocab-08, dated 14 September 2026; checked 15 September 2026.
    <https://datatracker.ietf.org/doc/draft-ietf-aipref-vocab/08/>
    Section 4.2 adds AI Use. Whether a user-supplied reference counts as direct
    provision remains open in issue 249. The draft does not represent working-group
    consensus.
[^71]: RSL Technical Steering Committee, *RSL 1.0*, Recommendation,
    10 December 2025, with errata. <https://rslstandard.org/rsl>
    Section 3.12, checked 15 September 2026, permits a licence to require reporting
    under a named profile, with an optional endpoint. An unsupported or unmet
    reporting requirement leaves the activity unlicensed under that licence.
[^72]: C2PA, *C2PA Technical Specification 2.4*, Appendix A.8.
    <https://spec.c2pa.org/specifications/specifications/2.4/specs/C2PA_Specification.html>
    The text-credential method uses U+FE00–U+FE0F and U+E0100–U+E01EF, among other
    characters, and remains under review. OWASP’s cited stripping recommendation names
    U+FE00–U+FE0F.
[^73]: Dow Jones & Co. v Perplexity AI, No. 1:24-cv-07984 (S.D.N.Y.),
    Opinion and Order, 21 August 2025, passages via secondary quotation; and
    Perplexity's Answer, Dkt 80, October 2025.
    <https://storage.courtlistener.com/recap/gov.uscourts.nysd.630270/gov.uscourts.nysd.630270.65.0.pdf>
    The jurisdiction challenge was denied. Claims describe copying into a RAG index as
    well as reproducing outputs; this is not a merits determination of retrieval
    copying.
[^74]: Advance Local Media v Cohere, No. 1:25-cv-01305 (S.D.N.Y.), Decision
    and Order, 13 November 2025, passages via Loeb & Loeb and the News/Media
    Alliance.
    <https://storage.courtlistener.com/recap/gov.uscourts.nysd.636920/gov.uscourts.nysd.636920.59.0.pdf>
    The court declined to dismiss the publishers’ substitutive-summary claim, relying
    mainly on verbatim examples. Cohere had not challenged the retrieval-copying or
    verbatim-output claims.
[^75]: New York Times v Microsoft, No. 1:23-cv-11195 (S.D.N.Y.), Opinion
    on motions to dismiss, 4 April 2025, passage via secondary quotation.
    <https://www.nysd.uscourts.gov/sites/default/files/2025-04/yf%2023cv11195%20OpenAI%20MTD%20opinion%20april%204%202025.pdf>
    One plaintiff’s bullet-point-summary claim was dismissed for lack of substantial
    similarity.
[^76]: Encyclopaedia Britannica v Perplexity AI, No. 1:25-cv-07546
    (S.D.N.Y.), Perplexity's memorandum in support of its motion to dismiss,
    3 November 2025. Read via a search provider's extract.
    <https://chatgptiseatingtheworld.com/wp-content/uploads/2025/11/Perplexity-Motion-to-Dismiss.pdf>
[^77]: US Copyright Office, *Copyright and Artificial Intelligence, Part
    3: Generative AI Training*, pre-publication version, May 2025, section on
    RAG. Quotations via Copyright Clearance Center and Norton Rose Fulbright.
    <https://www.copyright.gov/ai/Copyright-and-Artificial-Intelligence-Part-3-Generative-AI-Training-Report-Pre-Publication-Version.pdf>
[^78]: UK Government, *Report on Copyright and Artificial
    Intelligence*, 18 March 2026.
    <https://www.gov.uk/government/publications/report-and-impact-assessment-on-copyright-and-artificial-intelligence/report-on-copyright-and-artificial-intelligence>
[^79]: Case C-250/25 Like Company v Google Ireland, reference for a
    preliminary ruling, OJ C/2025/3039; hearing report by Bird & Bird,
    18 March 2026. <https://eur-lex.europa.eu/eli/C/2025/3039/>
    At the March 2026 hearing, the Advocate General raised assessment of the system as
    a whole, including training, grounding, inputs, outputs and communication to the
    public. An opinion was scheduled for 3 September 2026; the review found no report
    of delivery by 14 September.
[^80]: Landgericht München I, 42 O 14139/24, judgment of 11 November
    2025, press release 11/2025.
    <https://www.bayern.verfassungsgerichtshof.de/gerichte-und-behoerden/landgericht/muenchen-1/presse/2025/11.php>
    The Munich judgment assigns responsibility for outputs from simple prompts to the
    provider. It concerns memorised lyrics, not retrieval, is under appeal, and cuts
    against the user-prompt defence in that setting.
[^81]: Nikkei Inc., press release on proceedings against Perplexity AI,
    26 August 2025; hearing date from press reports.
    <https://www.nikkei.co.jp/nikkeiinfo/en/news/release_en_20250826_01.pdf>
    Nikkei and Asahi’s Tokyo proceedings had their first hearing in May 2026,
    according to press reports.
[^82]: UK Competition and Markets Authority, *Google Search: publisher conduct
    requirement*, final decision, 3 June 2026.
    <https://www.gov.uk/find-digital-markets-measures/google-search-publisher-conduct-requirement>
    The June 2026 requirement calls for effective publisher controls over use of
    search content in generative AI.
[^83]: Alliance de la presse d'information générale, press release on its
    referral to the Autorité de la concurrence, 11 August 2026.
    <https://www.alliancepresse.fr/app/uploads/2026/08/cp_aio_aim_saisine_adlc_alliance.pdf>
    The referral seeks enforcement of Google’s 2022 press-publishers’-right
    commitments after AI Overviews and AI Mode launched in France.
[^84]: Reddit v SerpApi, No. 1:25-cv-08736 (S.D.N.Y.), opinion of
    31 July 2026, as reported by Reuters, 31 July 2026, and summarised by Loeb
    & Loeb, 17 August 2026. Secondary report.
    <https://www.reuters.com/legal/litigation/perplexity-ai-loses-bid-toss-reddit-lawsuit-over-data-scraping-2026-07-31>
    Reddit’s anti-circumvention claims against SerpApi and Perplexity largely survived
    dismissal in July 2026, according to secondary reporting.
[^85]: Google LLC v SerpApi LLC, No. 4:25-cv-10826 (N.D. Cal.),
    order granting motion to dismiss with leave to amend, Dkt 42, 20 July 2026.
    Read via a search provider's extract.
    <https://storage.courtlistener.com/recap/gov.uscourts.cand.461513/gov.uscourts.cand.461513.42.0.pdf>
    Google’s anti-circumvention claims against SerpApi were dismissed with leave to
    amend.
[^86]: Google Cloud, *Generative AI Indemnified Services*, last modified
    20 July 2026. <https://cloud.google.com/terms/generative-ai-indemnified-services>
[^87]: Google Cloud, *Service Specific Terms*, Section 20 (Generative
    AI), last modified 29 July 2026. <https://cloud.google.com/terms/service-terms>
    Retention: grounded results may be kept for up to two years for display evaluation
    or chat history, or for the minimum period needed for legal compliance.
[^88]: Google Cloud, *Google Cloud Terms of Service*, Section 13, last
    modified 2 September 2026. <https://cloud.google.com/terms>
[^89]: Google, *Gemini API Additional Terms of Service*, effective
    23 March 2026. <https://ai.google.dev/gemini-api/terms>
[^90]: Google, *Google APIs Terms of Service*, Section 9(c).
    <https://developers.google.com/terms>
[^91]: Microsoft, *Product Terms: Glossary*, publication of
    1 September 2026.
    <https://www.microsoft.com/licensing/terms/product/Glossary/EAEAS>
[^92]: Microsoft, *Product Terms: Microsoft Azure*, Microsoft AI
    Services section.
    <https://www.microsoft.com/licensing/terms/productoffering/MicrosoftAzure/MCA>
[^93]: Microsoft, *Terms of Use for Grounding with Bing Search and
    Grounding with Bing Custom Search (available through Microsoft Enterprise
    Product Integrations)*, November 2025.
    <https://www.microsoft.com/en-us/bing/apis/grounding-legal-enterprise>
[^94]: Microsoft, *Microsoft Web IQ Terms of Use*, undated page.
    <https://www.microsoft.com/en-us/webiq/webiq-legal>
[^95]: Microsoft, *Customer Copyright Commitment Required Mitigations*,
    page dated 20 March 2026.
    <https://learn.microsoft.com/en-us/azure/foundry/responsible-ai/openai/customer-copyright-commitment>
[^96]: OpenAI, *Service Terms*, section 1, last seen updated
    12 June 2026. The page refused automated access; text read from search
    providers' copies, which agree with each other.
    <https://openai.com/policies/service-terms/>
[^97]: OpenAI, *OpenAI Services Agreement*, effective 1 January 2026.
    Read from search providers' copies of the page.
    <https://openai.com/policies/services-agreement/>
[^98]: OpenAI, *Web search* tool guide.
    <https://developers.openai.com/api/docs/guides/tools-web-search>
[^99]: Anthropic, *Commercial Terms of Service*, sections K and L,
    effective 17 June 2025. <https://www.anthropic.com/legal/commercial-terms>
[^100]: Anthropic, *Web fetch tool* documentation.
    <https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-fetch-tool>
[^101]: Anthropic, *Web search tool* documentation.
    <https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool>
[^102]: Amazon Web Services, *AWS Service Terms*, section 50.10, last
    updated 11 September 2026. <https://aws.amazon.com/service-terms/>
[^103]: Amazon Web Services, *Amazon Nova Web Grounding*
    documentation.
    <https://docs.aws.amazon.com/nova/latest/nova2-userguide/web-grounding.html>
[^104]: Perplexity, *Perplexity API Terms of Service*, section 9,
    last updated 23 January 2026. The page refused automated access; text read
    from a search provider's extracts.
    <https://www.perplexity.ai/hub/legal/perplexity-api-terms-of-service>
[^105]: Perplexity, *Perplexity API Terms of Service: Search*,
    Schedule 1. Read from a search provider's extract.
    <https://www.perplexity.ai/hub/legal/perplexity-api-terms-of-service-search>
[^106]: Brave, *Brave Search API Terms of Service*, sections 3 and 11,
    last updated 1 September 2026.
    <https://api-dashboard.search.brave.com/terms-of-service>
[^107]: Semafor, *News Corp accuses search engine Brave of AI
    copyright infringement*, 21 July 2026. Secondary report.
    <https://www.semafor.com/article/07/21/2026/newscorp-accuses-search-engine-brave-of-ai-copyright-infringement>
[^108]: Linkup, *Client General Terms and Conditions*, articles 4, 6 and 7,
    August 2025. <https://www.linkup.so/terms-of-use>
[^109]: SerpApi, *Legal*, section 13, last updated 27 August 2026.
    <https://serpapi.com/legal>
[^110]: Tavily, *Platform Terms of Service*, sections 12 and 13.2,
    last updated 4 May 2026. <https://www.tavily.com/terms>
[^111]: Exa Labs, *Terms of Service*, section 7. The PDF was read through
    a text rendering of the same file.
    <https://exa.ai/assets/Exa_Labs_Terms_of_Service.pdf>
[^112]: Jina AI, *Terms and Conditions*, section 4.3, last modified
    4 May 2026. <https://jina.ai/legal>
[^113]: TollBit, *Developer Platform Agreement*, last updated 6 February
    2025. <https://tollbit.com/legal/developer-platform-agreement/>
[^114]: Parallel Web Systems, *Customer Terms*, section 5(c),
    effective 11 August 2026. <https://parallel.ai/customer-terms>
[^115]: Observra, *Event schema* and CIM schema, open-agent-ai-security
    repository, 2026.
    <https://github.com/open-agent-ai-security/observra/blob/main/docs/event-schema.md>
    Observra is open source and sponsored by Exabeam. The absence of source fields
    concerns the schema reviewed in September 2026.
    The schema normalises events to a common information model. It classes
    `web_fetch`, `browse` and `search` as outbound `send_external`.
[^116]: Exabeam, *What's New in New-Scale July 2026: AI Agents Need
    More Than Guardrails*, 1 July 2026.
    <https://www.exabeam.com/blog/company-news/whats-new-in-new-scale-july-2026-ai-agents-need-more-than-guardrails/>
[^117]: Microsoft, *Audit logs for Copilot and AI applications*, Microsoft
    Purview documentation, updated 26 August 2026.
    <https://learn.microsoft.com/en-us/purview/audit-copilot>
    The per-resource flag is `XPIADetected`; the documented web identifier is
    `AISystemPlugin.Id = BingWebSearch`.
[^118]: Microsoft, *Alerts for AI services*, Defender for Cloud
    documentation, updated 6 July 2026.
    <https://learn.microsoft.com/en-us/azure/defender-for-cloud/alerts-ai-workloads>
[^119]: Pinjari and Saint-Germain, *DriftNet: A Dual-Head Trajectory
    Transformer for Detecting and Localizing Prompt Injection in LLM Agents*,
    arXiv:2609.10892, 9 September 2026. <https://arxiv.org/abs/2609.10892>
    Synthetic trajectory data: F1 0.983 on the evaluated trajectory
    benchmark.
[^120]: *AgentDrift* benchmark, arXiv:2609.06972, 7 September 2026.
    Abstract via a search provider's extract. <https://arxiv.org/abs/2609.06972>
    A surface-feature model on the same benchmark detected 8.2% of partial hijacks.
[^121]: Bundesgerichtshof, I ZR 281/25, hearing notice.
    <https://www.bundesgerichtshof.de/SharedDocs/Termine/DE/Termine/IZR281-25.html>
    Kneschke v LAION concerns training data. The hearing notice schedules a ruling for
    17 December 2026; a possible CJEU referral and any effect on machine-readable
    reservations remain prospective.
<!-- product-only:start -->

[^implementation-screen]: The screen matches 29 fixed phrases in four rules
    against case-folded text. In observations on 14 September 2026, the
    screen flagged an injection paper’s abstract and Google’s injection article,
    but missed a supplier terms-page footer telling AI agents to fetch and follow
    a URL.
[^implementation-robots]: Every redirect hop is evaluated against its own
    robots file. The record and explanation name the URL, file, group and
    rule (`docs/contracts/session-evidence.md` §Source declarations).
<!-- product-only:end -->

[^news-integrity]: BBC/EBU, *News Integrity in AI Assistants: Toolkit*, October
    2025; checked 15 September 2026.
    <https://www.ebu.ch/Toolkit/MIS-BBC/NI_AI_2025.pdf>
    Sections 1–6 describe accuracy, quotation, context, opinion, editorialisation
    and sourcing failures, with examples from the participating news organisations.
[^robots-authorisation]: IETF, *Robots Exclusion Protocol*, RFC 9309, September
    2022, sections 1 and 3; checked 15 September 2026.
    <https://www.rfc-editor.org/rfc/rfc9309.html>
[^c2pa-explainer]: C2PA, *C2PA and Content Credentials Explainer*, version 2.2,
    sections 2 and 7.2; checked 15 September 2026.
    <https://spec.c2pa.org/specifications/specifications/2.2/explainer/Explainer.html>
