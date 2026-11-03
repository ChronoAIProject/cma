# Agent Manager API

The code-derived contract at **`GET /agent-manager/openapi.json`** covers the
programmatic Agent lifecycle and interaction workflow. Its server is the current
CMA origin; requests use the existing `/api/v1` and `/api/v2` paths. For example,
production discovery is `https://bot.chrono-ai.fun/agent-manager/openapi.json`.
This is a focused view of the existing API, with the same operation IDs, request
schemas, responses and authorization. The full compatibility contract remains
at `/openapi.json`; existing clients and routes remain supported.

Use one **Agent ID** throughout. Creating an Agent returns a lifecycle operation
ID, and the operation yields `agent_id`. Responses v2 uses that Agent ID as its
conversation ID. Response IDs identify messages/turns; command IDs identify
admitted messages or controls. Neither creates a separate Agent or conversation
control model.

## Basic flow

Authenticate backend requests with `Authorization: Bearer <NyxID access token>`:
use an ordinary NyxID **user access token** obtained through NyxID OAuth for the
account that owns or is authorized to use the Agent. This is not the runtime's
scoped Agent key, an ID token, an Event Gateway binding key or a machine token.
CMA verifies the token's subject, current permissions and expiry; requests use
that user's authority. No browser cookie is required for bearer API calls.
The same-origin CMA browser session cookie is also accepted. Creating a new
Sandbox requires renewable owner delegation established through the existing
CMA OAuth login. A bearer alone does not establish this delegation. See
[REST authentication](API_REST.md#authentication-and-http) for setup and retries.

1. Create an Agent, retaining the request body and one idempotency key:

   ```http
   POST /api/v1/agents
   Authorization: Bearer <access-token>
   Content-Type: application/json
   Idempotency-Key: create-my-agent-001

   {"name":"My Agent"}
   ```

   A `202` reply is `{"id":"<lifecycle-operation-id>","status":"..."}`.
   Add `workspace_id` to use an existing ready Workspace, or `sandbox_id` to
   create an empty Workspace inside an existing Sandbox, through this same
   endpoint. Omit both for a new Sandbox. Read the request schema for supported
   repository and inline settings options.

2. Read `GET /api/v1/agent-lifecycle/<lifecycle-operation-id>`. Keep the returned
   `agent_id`; inspect `status`, `phase`, `events`, and any connection requirement.
   A readiness phase event alone is not operation completion. A lost response
   is recovered with the original POST body and key, not another creation.

3. Send a message to that Agent:

   ```http
   POST /api/v2/agents/<agent-id>/responses
   Authorization: Bearer <access-token>
   Content-Type: application/json
   Idempotency-Key: message-001

   {"input":"Inspect the workspace and summarize it.","stream":true}
   ```

   Read the SSE stream. The successful handshake supplies `X-CMA-Response-ID`
   and `X-CMA-Command-ID`. Persist these IDs. An explicit `conversation` is
   optional on this route; when supplied, it must match the Agent in the URL.
   `stream:true` is required. To recover after a disconnect, use
   `GET /api/v2/responses/<response-id>` and
   `GET /api/v2/responses/<response-id>/events`, retaining the replay cursor.
   Disconnecting the stream does not cancel the admitted message.

4. Read conversation history at
   `GET /api/v2/conversations/<agent-id>/items`. The Agent transcript endpoint
   provides the CMA transcript view. For live Agent chronology, use the v2
   Agent event snapshot and resumable event stream; retain its cursor.

## Included operations

The focused contract contains 37 paths and 43 operations. It retains only
transitively referenced components, used tags and required security schemes.
The generated document is authoritative for exact bodies and error responses.

| Purpose | Existing API surface |
|---|---|
| Create, list, read, delete Agents | `/api/v1/agents`, `/api/v1/agents/{id}` |
| Observe creation | `/api/v1/agent-lifecycle/{id}` |
| Read environment and control Sandbox lifecycle | Sandbox list/detail/events, pause, revive, delete and runtime-update; Workspace reads |
| Send and observe messages | Agent-bound Responses v2 creation, response retrieval/replay, conversation reads/items |
| Observe live Agent state | V2 Agent event snapshot and SSE; Agent transcript and usage |
| Control an active turn | V2 response controls, atomic interrupt-with-input and command status |
| Edit or remove one queued input | V2 queued-input list/edit and queued-cancellation observation/mutation |
| Transfer files | Agent attachment upload and published artifact list/read/download |
| Configure model and settings | Agent model catalogue and settings read/update |
| Revise service access | Agent service-access replacement and access-request list/approve/deny |

Profile authoring, marketplace, administration, Gateway bindings, AG-UI and older
interaction projections remain in the full document. They are excluded from
this focused contract. Initial creation can reuse an environment through the
inline lifecycle request; separate profile-based creation paths are unnecessary
for this flow.

## Controls, queues and retries

Before cancelling, steering or resolving an execution approval, GET the selected
response's `/controls`. Submit its observed `turn_sequence` and
`agent_revision` as `expected_agent_revision`, with a retained idempotency key:

```json
{
  "turn_sequence": 42,
  "expected_agent_revision": 17,
  "command": { "type": "cancel" }
}
```

This stops without adding another message. The `/interrupt` operation atomically
admits cancellation and a distinct replacement message using the same observed
turn/revision and a `text` field. Ordinary sends while an Agent is busy enter its
existing durable queue. Queue edits and cancellations target one response and
source command with its observed command revision; other queued messages retain
their identity and order. Follow command status and the target response's durable
outcome: delivery of a control does not by itself prove that execution stopped.

After an uncertain reply, retry the same key and exact body. Do not change the
revision, target or text under that key. Generic conflicts remain uncertain;
only a documented definitive non-admission outcome permits a fresh intent.
Queue capacity/refusal, errors, cursor recovery and SSE event semantics are
specified in [Responses v2](API_Responses.md#responses-v2).
REST operations use `application/problem+json`; Responses v2 may return its
`error` envelope, while deployment middleware can return the REST problem form.
Clients must handle both. Post-header observation failures require retrieval or
reconnection and do not establish a failed execution.

## Files, settings and access

Upload raw file bytes through
`POST /api/v1/agents/{id}/attachments?name=<filename>`, with a stable
`Idempotency-Key` and the appropriate content type. The receipt supplies the
runtime `path`; refer to that path in the message text. The current Responses v2
subset accepts text and `input_text` parts, not OpenAI file/image content parts.
Each uploaded file is limited to 20 MiB. Published output artifacts have separate
list/read/download routes; uploads do not automatically publish private files.

Answer deliverables use the final recorded answer's `event:<sequence>` identity,
its turn sequence and an explicit resource reference. Read
`GET /api/v1/agents/{id}/answers/{answer_id}/deliverables` only for visible final
answers containing a `cma-deliverable:v1` marker. The list contains up to 32
deduplicated items and reads authorized metadata without downloading their bodies.
`agent_artifact`, `document` plus source revision, and `live_file` plus exact
home-relative path/hash are distinct identities. This API never publishes files.

Read one item at `.../deliverables/{delivery_id}`. Set `verify=true` for an explicit
single-item retry that checks actual bytes. A live file initially reports
`unchecked` (Verify on download); it retains its answer-time size and digest.
Download at `.../{delivery_id}/download`, verify size and SHA-256, and re-read that
item's current authorization before saving. A changed live file returns 409;
a revised document keeps the referenced immutable source revision. Conversation
visibility does not grant file or document access. Document token links use the
existing `X-CMA-Document-Share-Token` header in addition to authentication.

The verification endpoint `POST /api/v1/agents/{id}/deliverable-references` accepts
an explicit resource selector and returns `{reference, marker}`. The runtime
descriptor tool and document publication receipts supply the same marker. Only
an unchanged standalone marker in a successful turn's final nonempty Assistant
message establishes association. File names, code spans and directories do not.
The answer projection reads at most 128 records after the requested message and
512 records in its turn, within 4 MiB/15 seconds; it fails closed beyond these
bounds instead of rebuilding the full conversation.

Read the Agent's model catalogue before updating model, reasoning effort or
permission mode. Use the settings endpoint and observe `pending` until runtime
confirmation. For service access, list access requests and approve or deny the
specific request, or replace the Agent's service scope. Required services remain
mandatory. Follow the access request to its terminal state and complete any
returned connection requirement; acceptance does not mean the new key is already
applied. Execution approval controls and service-access requests are distinct.

Sandbox pause, revive and deletion return accepted lifecycle state; observe the
Sandbox detail/events for completion. Managed runtime updates submit the exact
`target_version` advertised by the Sandbox's update view and retain the same
Sandbox/home/conversation lineage. See [REST runtime updates](API_REST.md) for
eligibility, queued work, original-target retries and failures.

## Message feedback

Feedback is private metadata belonging to the Agent owner. It never starts,
interrupts or queues a turn, changes the transcript, or contacts the Sandbox.
The caller needs current `UseOwnSandboxes` authority and ownership of the Agent
and its complete Sandbox/Workspace/Worktree lineage. Shared readers cannot vote
or read the owner's votes. Foreign and missing Agents return the same 404.

`GET /api/v1/agents/{id}/message-feedback?after=0&limit=200` returns
`{"items":[{"message_id":"event:42","vote":"up"}],"next":null}`. Only non-null
votes are listed in ascending message sequence. `limit` is 1–200; pass a non-null
`next` as the next `after` until it is null. The page reads through the captured
Agent history high-water mark. Votes on messages committed later appear on a
subsequent refresh.

`PUT /api/v1/agents/{id}/messages/event:42/feedback` with `{"vote":"up"}`,
`{"vote":"down"}`, or `{"vote":null}` sets, switches or clears the owner's vote.
The response echoes `message_id` and the saved `vote`. The `vote` field is
required, including when clearing. This absolute metadata update is idempotent
and does not require an `Idempotency-Key`; concurrent requests use database write
order. Clients should serialize changes to a particular message. Retrying an
uncertain old request after a newer selection can replace that selection.

Use `event:<sequence>` from `GET /api/v1/agents/{id}/transcript` message records
or durable Agent event sequences. New writes require a nonempty Assistant answer
with the greatest message sequence in a successfully completed turn. Message
array order and equal content do not establish identity. Active, interrupted,
failed and output-gap turns, intermediate replies, owner messages, lifecycle,
tool, draft and optimistic IDs are rejected with 404. Regenerated answers retain
their separate message/turn identities. Reads retain all historical saved votes,
including votes on formerly eligible messages; tightening write eligibility does
not delete those records. The up/down/null request and response shapes are unchanged.
Clients keep the last confirmed choice while a save is pending, serialize writes,
and verify or retry an uncertain original value before allowing a different choice.
Votes
persist across refreshes and API restarts in Agent Manager's independent
`agent_message_feedback` collection. Its schema and unique owner/Agent/sequence
index initialize additively at startup. Isolated database deployments must add
this collection to the generated Agent Manager role before upgrading; explicit
shared-schema deployments require no new database credential.

### Direct owner service access

Direct NyxID owner access tokens can approve requests and replace service scope
without a browser login. CMA verifies the owner, current management permissions,
finite token expiry and access to each selected service and its backing nodes.
Runtime/Agent keys, relay tokens, service accounts and delegated actors cannot
approve. The existing personal-resource ownership policy still applies.

CMA stores the exact API token in encrypted credential custody and records only
its authority reference with the decision. Deferred execution, credential
issuance, renewal and revocation recover that same authority after restart and
revalidate it; they never substitute a browser login. API tokens are not refreshed
by CMA. Expired or revoked authority stops further credential operations with
`owner_service_authority_required`; authorize further work with a current owner
token. On the original Agent, submit a new idempotency key with both `services`
and `expected_services` equal to the current scope. Once the prior request has
settled or failed, this records fresh consent for that exact scope, including
when credential rotation or revocation is still pending. Pending rotation
continues at its existing scope generation; settled access performs an ordinary
rotation with a fresh backing-node plan. Both preserve the prior audit, runtime
and conversation. A changed scope remains a separate update after rotation settles. Replaying an old key returns its original
result and never substitutes new authority.

HTTP 403 `agent_access_owner_authority_required` requests a direct owner access
token. HTTP 409 `agent_access_service_connection_required` means the owner must
connect the service or authorize its backing resources through NyxID, then retry.
The existing connection workflow remains available to browser owners. Missing
scope-plan support or invalid provider evidence returns a temporary refusal;
CMA never silently issues a service-only key.
