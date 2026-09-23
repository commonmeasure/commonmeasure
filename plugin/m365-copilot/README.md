# Microsoft 365 Copilot

This package adds a Common Measure Research declarative agent to Microsoft 365
Copilot. It calls an organisation's hosted edge at `/mcp/m365-copilot` using
OAuth tokens issued by Common Measure Hub. The hosted transport is described
in the [host contract](../../docs/contracts/host-integration.md).

## Status

- **Copilot in the browser, `live-verified`** on 17 September 2026 with
  personal packages 1.0.5 and 1.0.6: Hub consent and token exchange, an
  authenticated `context_status`, an admitted public fetch and a policy
  refusal in one MCP session, and the cleared public-source metadata and
  aggregate refusal count delivered to the Hub.
- **Word on Mac (16.113.914.0), `live-verified`** on 17 September 2026:
  Word's own status call and a paired public fetch and refusal matched the
  edge's source record and the Hub's receipt.
- Direct sign-in from Word does not complete: the Hub email link opens
  outside Word's dialog and Microsoft does not finish the connection. Sign
  in once through Copilot in Chrome, then start a fresh Word chat. Retry
  links can lose the OAuth continuation; start a retry from the agent's
  sign-in button.
- Whether Copilot exposes which acquired sources an answer used is not
  established. Answer citations alone do not establish use, and missing
  observations stay unavailable in the record.
- Word can pass the open document to the agent through its own document
  context. That access does not go through Common Measure's fetch tool and
  produces no Common Measure source record.

## Build and connect

Prerequisites: Python 3, an enrolled and managed hosted edge reachable over
HTTPS, its organisation registered at the Hub, a Hub member account, and a
Microsoft 365 tenant that permits Copilot and custom app upload. The tenant
administrator decides availability and distribution. Installing this
package gives no supplier access.

1. In Microsoft 365 Agents Toolkit, create a declarative agent with an action
   from the hosted edge's MCP URL and select OAuth. Use static OAuth: dynamic
   client registration omits the `resource` parameter the edge requires, and
   the edge correctly refuses it. Register a confidential Hub client with the
   exact Teams callback, keep PKCE enabled, and include the exact MCP
   endpoint as `resource` in the authorisation URL. Keep the resolved
   `OAuthPluginVault.reference_id` and app ID for that app and environment.
   Follow [Microsoft's build procedure][build] and
   [OAuth guide][oauth]; the guide currently requires `AnyApp` for MCP.
2. Build the archive from the repository root. The auth reference is an
   identifier, never a client secret.

   ```sh
   python3 plugin/m365-copilot/package.py \
     --endpoint "$M365_MCP_ENDPOINT" \
     --app-id "$M365_APP_ID" \
     --auth-reference "$M365_AUTH_REFERENCE" \
     --privacy-url "$M365_PRIVACY_URL" \
     --terms-url "$M365_TERMS_URL" \
     --output dist/commonmeasure-m365-1.0.0.zip
   ```

3. Validate the archive with Agents Toolkit, then upload it under the
   tenant's custom-app policy (Microsoft 365 Admin > Agents > All agents >
   Add agent) and restrict it to test users before wider distribution. Check
   the availability the upload actually sets. Sign in at the Hub when Copilot
   asks.

The archive contains `manifest.json`, `declarativeAgent.json`,
`ai-plugin.json` and the two required PNG icons. The builder refuses an
existing output file and a major version of zero, which Microsoft's tenant
validation rejects. It makes no network call, client registration, upload or
deployment.

The agent discovers its tools at run time: `functions: []` and
`run_for_functions: ["*"]`, per [Microsoft's discovery contract][discovery].
The hosted endpoint exposes only `context_fetch`, `context_search` and
`context_status`. For a package with pinned definitions, export a
`tools/list` result from the matching server binary and pass
`--tools-file <export.json>`; the builder requires exactly those three tools
and keeps their schemas. Pinned definitions do not prove run-time discovery.
The agent declares no built-in web search, SharePoint or other knowledge
capability. Its [instructions](instructions.md) guide source handling; they
do not enforce host-wide acquisition or disclosure controls.

## Acceptance in a new tenant

Use a test organisation and non-confidential material. Record redacted
outcomes with the exact edge, Hub and app versions; never keep bearer tokens,
codes or client secrets in evidence.

| Check | Required evidence |
|---|---|
| Authentication | The callback, the PKCE method, and whether Microsoft sends `resource` on authorisation and token exchange. The audience must stay this edge's exact MCP endpoint. Record a refusal if incompatible; do not remove audience or PKCE checks. |
| First call | Copilot's client identity, the `context_status` response, the session ID and an `oauth_subject` linked to the intended Hub member. |
| Admission | Fetch an operator-controlled public page under the managed policy, and match the returned text and its hash to a mediated crossing in the source record. |
| Refusal | Fetch a second public URL the policy denies. Keep the refusal and verify no body was admitted. Ask the agent to retry by another route and record the host's response separately. |
| Search | Exercise a configured provider and an unavailable one; the unavailable one must stay explicit. |
| Separation and expiry | A second member leaves a distinct authenticated session. Removing membership or revoking the grant stops renewal; an access token can stay valid until its one-hour expiry. |
| Output observation | Whether the host exposes which acquired sources an answer used, through a supported interface. |
| Reporting | Hosted-session egress clearance, then an authorised report and its receipt. A session with only refusals produces no batch. |

Copilot's native SharePoint access is not a Common Measure adapter; do not
include internal activity in publisher reporting.

## Local verification

```sh
python3 -m unittest discover -s plugin/m365-copilot -p 'test_*.py'
```

On 16 September 2026 a fixture archive with reserved example URLs and a
dummy vault reference passed validation against Microsoft's app 1.23,
declarative agent 1.8 and plugin 2.4 JSON schemas, and the corrected archive
passed all 59 of Microsoft's package rules. The Python `jsonschema`
validators are instantiated directly, because their schema self-check
rejects an unused ECMAScript `\p{L}` expression in Microsoft's app schema.
This establishes local validity, not acceptance by a tenant.

Microsoft references, checked 16 September 2026:

- [Build an MCP plugin][build].
- [Configure dynamic client registration][dcr].
- [Plugin manifest 2.4][plugin-schema].
- [Declarative agent 1.8 JSON schema][agent-schema].
- [Microsoft 365 app 1.23 JSON schema][app-schema].
- [Declarative agents in Word][word], and
  [document interaction][document].

[build]: https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/build-mcp-plugins
[dcr]: https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/plugin-authentication-dynamic-client-registration
[oauth]: https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/plugin-authentication-oauth
[discovery]: https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/plugin-dynamic-tool-discovery
[plugin-schema]: https://developer.microsoft.com/json-schemas/copilot/plugin/v2.4/schema.json
[agent-schema]: https://developer.microsoft.com/json-schemas/copilot/declarative-agent/v1.8/schema.json
[app-schema]: https://developer.microsoft.com/json-schemas/teams/v1.23/MicrosoftTeams.schema.json
[word]: https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/build-declarative-agents
[document]: https://learn.microsoft.com/en-us/microsoft-365/copilot/extensibility/declarative-agent-document-interaction
