# NyxID service connection return

CMA registers `https://cma.nyxid.ai/connection-return` as an exact redirect URI
on its NyxID OAuth client alongside `https://cma.nyxid.ai/auth/callback` for
browser sign-in. Retain both old `https://bot.chrono-ai.fun` callback entries
during the domain migration. Other environments use their configured CMA public origin with
the same paths.

The hosted connect-link request sends the fixed `/connection-return` URI without
an intent query parameter. NyxID validates OAuth client callbacks by exact
registered URI; a new random query value for every attempt cannot match that
registration. NyxID may append its own terminal `status` and `connect_link_id`
parameters when returning the hosted window.

The return page is informational. Its query parameters, navigation and window
closure do not grant service access, accept an approval or start an Agent turn.
CMA does not use an untrusted callback nonce as continuation authority. The
approved access request persists the owner, Agent lineage and exact approved
services. The main window retains the target and handoff intent, verifies the
current owner before recovering a saved attempt, and polls the authenticated
connection check for that target. The background access worker separately
checks the exact approved service set before advancing its durable workflow.
It notifies the original conversation only after a newer credential generation
has been delivered to the correct runtime. Notification admission uses the
request's stable idempotency key.

An existing NyxID binding uses the exact binding's manual repair page. CMA does
not create another hosted connection for an already-bound service. Forced
re-login is supported only for the native Codex subscription flow. Therefore,
an already-satisfied scope test proves approval, credential delivery and
automatic notification but does not exercise a fresh hosted connection callback.

## Registration and verification

Read the current client through NyxID's official developer-app API, then patch
only `redirect_uris`, preserving every existing entry and adding the fixed
return URI. The configured production client is
`c0cd33d2-27ba-4b8d-b38e-562364fadb72`.

```http
GET /api/v1/developer/oauth-clients/{client_id}
PATCH /api/v1/developer/oauth-clients/{client_id}
Content-Type: application/json

{"redirect_uris":["https://bot.chrono-ai.fun/auth/callback","https://bot.chrono-ai.fun/connection-return","https://cma.nyxid.ai/auth/callback","https://cma.nyxid.ai/connection-return"]}
```

The example assumes those are the complete existing and added URI set; use the
freshly read list when applying it. Read back the client and compare the other
metadata. Do not rotate the client secret or change consent scopes for this
configuration repair.

The broker regression uses a provider that rejects callbacks other than the
exact registered static URI, and exercises two distinct handoff intents. The
ConnectionWizard regression proves that a claimed successful callback and
window closure do not release an access request, and that even a ready response
from a different owner is rejected. The access-dialog test waits for durable
request success after service readiness. The local browser callback test
exercises real popup navigation/reuse against isolated identity fixtures; it
does not claim a live NyxID credential connection.

A full live proof requires an owner-authorized relevant service that is
naturally missing: actual Agent MCP proposal, owner approval, authenticated
missing-state observation, the real Connect all window and hosted completion,
the provider-issued return URL, independently verified readiness, credential
delivery and one completed automatic notification in the original conversation.
Do not revoke an existing connection or add an unrelated grant merely to create
that precondition. A hosted-link callback exercised separately on an existing
service is evidence of callback transport only; it is not evidence of the
missing-to-ready gate and must be reported separately.
