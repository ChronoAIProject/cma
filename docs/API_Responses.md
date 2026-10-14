# Responses Interface

**Priority 2 - for chat clients and OpenAI-compatible SDKs.**

The canonical contract for new clients is `/api/v2`. It accepts an existing
Agent as `conversation` on every turn and admits input to a durable command
ledger before opening SSE. Existing `/api/v1` clients keep their original
conversation creation, predecessor and turn-derived response identity rules.
Legacy response POSTs also enter common durable command admission. The v1 contract below continues
to apply to those routes; changing a client's base URL requires adopting the
v2 rules in this section.

## Responses v2

An optional request `model` must match the Agent's current conversation settings;
it does not override those settings. Once a command starts a turn, its response
retains the model selected at that durable dispatch boundary. Later settings
changes do not relabel earlier responses. Before dispatch, the retrieved response
and `response.created` retain the immutable creation model as a hint, preserving
their existing equivalence and replay cursors. That opening hint does not
establish which model executed the turn. `response.in_progress`
and the terminal response carry the selected model. `deployment-default` means
the recorded turn delegated model selection to
the runtime. Legacy turns without a retained model use the immutable creation
model only when no settings edit preceded them; otherwise they report `unknown`.

All v2 endpoints belong to the control plane. No Responses server runs inside
an Agent Sandbox. Creation requires a current authenticated credential in CMA's
durable custody. Browser requests retain their existing sign-in incarnation.
For external API requests, CMA independently encrypts the exact NyxID access
token presented by the caller until its provider-verified expiry. The token must
be an ordinary, unrestricted user access token with active introspection, a
matching subject, current permissions and an explicit expiry. Unregistered agent keys,
service-account, delegated, relay, resource-scoped and sender-constrained tokens
cannot use this path. API custody neither replaces a browser login nor borrows
its refresh token; callers obtain a new API token when theirs expires. Current history
authorization is sufficient for reads, even when the command's original execution
credential has been revoked. Personal owners and currently authorized human
administrators can admit commands to shared or organization-owned Agents using
their own retained credentials. View grants and member/viewer roles permit
history without interaction. Before deferred execution the worker checks current
membership, resource policy and the exact admitted credential custody again. Sandbox
revival additionally requires independent `manage_sandbox` authority and uses
the resource owner's connections. Organization resource creation and credential
provisioning remain separate capabilities under development.

A supplied key registered to a live provider binding uses local hash lookup and
only `UseBindingConversations`; no NyxID request or owner login is needed. It can
read and interact with that binding's mapped Agents. It cannot list the owner's
other resources, manage Sandboxes, read files, change sharing or answer approvals.
For these callers, `conversation` may name the external conversation ID or its
mapped `agt_` ID. The Agent URL alias and body may use different IDs when both
resolve to the same mapped Agent. Missing and foreign mappings return
`404 conversation_not_found`; a provisioning mapping returns
`429 conversation_not_ready` with `Retry-After: 1`. A concurrent reservation
snapshot change returns `429 agent_command_admission_conflict` with the same
retry hint. Retry the exact request and idempotency key; changed input still
returns `409 agent_command_key_conflict`, and non-admissible Agent states remain
409. Conversation GET routes accept the same aliases.
Responses retain the mapped Agent ID as their conversation identity.

Binding command receipts retain the binding identity and environment generation,
so rotating its supplied key preserves idempotency and already admitted work.
Admission, controls, cancellation, dispatch and automatic parent revival fence
current binding and conversation authority in the same store transaction. The
worker obtains fresh authority before execution. Retirement or reconfiguration
refuses old-generation work. Held streams close when their binding proof changes;
reconnect with the current key. A queued binding command may revive only its
mapped parent without granting public Sandbox-management authority.

Binding requests may include `event_context` with
`{schema_version:1,channel_id,conversation_id,activity,event_ref}`. The full object
(up to 1 MiB when serialized) is retained atomically with admission and is part of
the command fingerprint. `conversation_id` must be the binding's external
conversation ID, including when the request uses the mapped Agent alias. The
activity must have schema version 1 and a nonempty event ID. The channel ID is
independent of the binding ID. A different context under the same idempotency key,
or a second key for the same binding/conversation/event, returns a conflict.
Neither can overwrite the original object or enqueue another turn.
Command admission retries transient database contention with backoff for up to
five seconds between completed attempts, bounded by the original authorization
deadline. An active transaction or commit is allowed to finish; the retry budget
does not cancel it or renew authorization.

The Agent receives the ordinary input followed by `event_ref: <reference>`.
The full activity is retrieved through authenticated
`GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}`.
This read requires a current key for the exact binding and mapped conversation,
revalidates authority before releasing the object, and returns `Cache-Control:
no-store`. Missing context returns `404 event_context_not_found`; unavailable
storage fails explicitly. Owner credentials do not grant access to this binding
route. The existing 256 KiB runtime input limit includes the reference; the large
context does not consume metadata capacity or become prompt text.

| Method and route | Behavior |
|---|---|
| `GET /api/v2` | Discover the supported subset, replay extension and companion stream |
| `POST /api/v2/responses` | Admit text input for `conversation=AgentId`; require `stream=true` |
| `POST /api/v2/agents/{agent_id}/responses` | Same admission, receipt and events; optional `conversation` must match the URL |
| `GET /api/v2/responses/{response_id}` | Retrieve a queued, active or terminal durable response as JSON |
| `GET /api/v2/responses/{response_id}/events` | CMA extension: replay and follow deterministic durable Responses events |
| `GET /api/v2/conversations/{conversation_id}` | Read the existing Agent as a conversation, with empty metadata |
| `GET /api/v2/conversations/{conversation_id}/items` | Read standard user/assistant message items under current history authority |
| `GET /api/v2/agents/{agent_id}/responses/{response_id}/controls` | CMA extension: observe the bound response turn, Agent revision and runtime control support |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/controls` | CMA extension: durably request cancellation, steering or approval resolution for that exact turn |
| `GET /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation` | CMA extension: observe the exact unbound source command revision and cancellation eligibility under current operation authority |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation` | CMA extension: atomically cancel that unbound source and retain a completed cancellation receipt |
| `GET /api/v2/commands/{command_id}` | CMA extension: poll a safe durable command receipt and delivery state |

Creating conversations, stateless Agent auto-creation, response deletion and
the standard OpenAI cancellation endpoint are outside v2. Select an existing Agent using CMA's
resource APIs. A conversation ID can be a string or `{ "id": "agt_..." }`.
The model pinned in the Agent Profile remains authoritative.

| Request field | V2 policy |
|---|---|
| `conversation` | Required on the canonical route; an existing authorized Agent |
| `input` | Required nonempty text, or one user message with string content or one or more `input_text` parts; adjacent text parts canonicalize |
| `stream` | Literal `true` required; absent/false returns `stream_required` |
| `model` | Optional nonempty string; mismatch returns `model_mismatch` |
| `metadata` | Optional object/null; 16 string pairs, 64 characters per key, 512 per value; visible to the personal owner and the scoped binding |
| `store` | Omitted or literal `true`; data is retained under Agent history policy |
| `previous_response_id` | Omitted/null only; predecessor continuation is unsupported; combining a nonnull predecessor with conversation returns `conversation_previous_response_conflict` |
| `instructions`, `reasoning`, `tools`, `tool_choice` | Refused by name; execution configuration belongs to the pinned profile |
| `background`, `include`, `stream_options`, sampling fields, file/media parts, all other fields | Unsupported; named validation refusal |

Use the `Idempotency-Key` header for retry recovery. It allows 1–256 bytes without
control characters. The scope includes the actual custody authority, Agent and
command kind. Equivalent retries return the original command/response identity;
changed input or metadata returns `409 agent_command_key_conflict`. If the header
is absent, the server creates a fresh key. A client that loses that first reply
must have supplied its own stable key to recover by retry.

A `200 text/event-stream` handshake identifies a durably admitted response through
`X-CMA-Response-ID` and `X-CMA-Command-ID`. The response ID is opaque and retained
from admission onward; it is not derived from a not-yet-assigned turn. An Agent
can be queued, busy, paused or reviving. Server-owned execution continues after a
stream disconnect. The queue holds at most 64 waiting commands, with a five-minute
admission deadline. Queue expiry or lost execution authority produces a durable
failed response. A lost reply does not revoke already-admitted work.

A temporary identity-provider failure keeps an unclaimed command queued within
that original deadline. Waiting and revival checks are paced, and a later attempt
must verify current authority before dispatch. This does not extend credential
expiry or the queue deadline. Revocation, lost permissions and other definitive
authority refusals still prevent execution. Request-time verification outages
return `503`; clients must preserve the original key and body when admission is
uncertain.

| Event | V2 behavior |
|---|---|
| `response.created` | Exactly once in a full replay; stable ID, creation time and initial queued status |
| `response.in_progress` | Once execution is bound to a durable turn |
| `response.output_item.added`, `response.output_item.done` | Standard assistant message and informational `mcp_call` items with stable indexes and IDs |
| `response.mcp_call.in_progress`, `response.mcp_call.completed`, `response.mcp_call.failed` | Durable provider activity transitions; no client tool execution |
| `response.content_part.added`, `response.content_part.done` | Standard `output_text` parts |
| `response.output_text.delta`, `response.output_text.done` | Text emitted at durable message commit boundaries; done contains the complete text |
| `response.completed`, `response.failed`, `response.incomplete` | Terminal projection from durable command/turn state |
| `error` | Observation failure after headers; does not change the command's durable outcome |

This version streams committed messages, not transient provider token fragments.
Provider tool activity uses standard `mcp_call` items with `server_label: "cma"`,
a display `name` bounded to 200 Unicode characters, empty `arguments`, and null
`output` and `error` bodies. These items describe provider activity; they do not
request client tool execution. Their status is `in_progress`, `completed` or
`failed`. Tool starts and terminal updates follow immutable log order, so tools
may overlap and text may appear before a tool completes without rewriting the
replay prefix. Message IDs remain identical to conversation item IDs. Reasoning
fragments, approvals and steer details use the separately authorized AG-UI
companion discovered by `GET /api/v2`. `GET` and terminal SSE output agree on
response IDs, item IDs, metadata, text and usage. `previous_response_id` in v2
response objects is null because the primary mode uses conversation identity.

The replay extension accepts `starting_after=<sequence_number>` or `Last-Event-ID`.
Both, when supplied, must agree. Sequence numbers and SSE `id` values start at zero
and are deterministic for durable events, independent of aliases or reconnects.
Unknown/future cursors are refused. Connection `error` events have no SSE `id`, so
an observation failure cannot skip future durable output on reconnect. Standard
response retrieval accepts no stream/include/replay query parameters.

Streams retain opaque, nonrenewing authorization, with no request credential.
Every buffered frame passes a current policy check before release. Membership
expiry or revocation ends observation; reconnect obtains fresh current authority.
Shared response metadata is empty, private events are omitted, and private runtime
failure text is sanitized. Conversation items accept `limit=1..100`, `order=asc|desc`
(default desc), and one of `after` or `before`, scoped to that conversation.

Before SSE, v2 route errors have JSON `{ "error": { "type", "code", "message", "param" } }`.
Malformed JSON returns 400; missing authentication 401; unsupported fields 422;
current visible interaction refusal 403; missing/inaccessible IDs the same 404;
key/resource conflicts 409; full queues 429; unavailable authority/storage 503.
A durable `conversation_gap` records missing runtime output. History and event
reads retain the surrounding recorded messages and the gap evidence. An affected
response remains `in_progress` while its native turn is active; after its recorded
terminal outcome, Responses v1 and v2 report `incomplete` with
`incomplete_details: { "reason": "conversation_output_gap", "frames": N }`.
Later completion cannot certify missing output as complete. Earlier and later
independent turns retain their own outcomes. The transcript represents an affected
completed turn with its existing failed-outcome shape and failure class
`conversation_output_gap`; this projection does not rewrite the stored native
completion or invent missing messages.

429 and 503 route responses include `Retry-After: 1`. Deployment-wide middleware
authentication/dependency failures and limits retain the shared REST problem
envelope; both CMA SDKs accept both error shapes. After headers, a connection error instructs
the client to retrieve/reconnect; only durable execution failures emit `response.failed`.

### CMA response controls

Observe `/api/v2/agents/{agent_id}/responses/{response_id}/controls` before forming
a new control intent. The response contains the safe source command view,
`agent_revision`, optional `target` (`response_id` and `turn_sequence`),
`target_active`, and `runtime_support` booleans for `cancel`, `steer` and
`resolve_approval`. A queued response has `target: null`; these controls require
a turn already bound by durable dispatch. Runtime support describes an observation,
not the caller's permission or a guarantee that an approval remains pending.

POST to the same path with exactly one retained `Idempotency-Key` and strict JSON:

```json
{
  "turn_sequence": 42,
  "expected_agent_revision": 17,
  "command": { "type": "cancel" }
}
```

Use the observed Agent aggregate revision, not the nested Agent Profile revision.
Other commands are `{ "type": "steer", "text": "Check this first" }` and
`{ "type": "resolve_approval", "approval_id": "approval_...", "decision": "allow" }`;
approval decisions are `allow`, `deny` or `abort`. Only the current pending approval
on the selected turn can be resolved. Unknown/duplicate body fields, caller-supplied
authority fields and unsupported command shapes are refused. Cancel/steer require
current `operate_agent`, approvals require `resolve_approval`, and all mutations
require the caller's current credential custody. History access permits observation
and command polling without granting control authority.

A `202` body has object `cma.response.control`, the safe `control` command view,
the immutable `target`, and the originally submitted `expected_agent_revision`.
The control receipt's own `response_id` is null. Poll its `command_id` through
`GET /api/v2/commands/{command_id}`. Queued, waiting, delivery-uncertain, failed
and completed command states remain distinct. **Completed control means delivered;
it does not establish that the target response was cancelled.** Observe the target
response's own durable outcome for cancellation confirmation. Neither disconnect
nor aborting a local stream reader is a cancellation request.

After an uncertain admission, repeat the original key and immutable JSON body,
including the old revision and turn. Exact recovery precedes active-target/runtime
support checks and returns the original receipt even after a newer turn starts,
subject to fresh authorization. Changing target, revision or command content with
that key returns `409 agent_command_key_conflict`.

`409 response_control_not_admitted` proves that no receipt exists for the original
scope/key and that its submitted Agent revision is permanently stale in the same
Agent incarnation. The server verifies the original bound source and commits fresh
Agent/parent authority fences before returning this outcome. A concurrent earlier
admission must either remain recoverable or conflict with those fences. This code
allows the client to discard that pending intent, observe again and ask for a new
explicit action. Equal/future revisions, missing or invalid source provenance,
authority races and unknown commits cannot establish that proof.

`409 response_control_conflict` remains uncertain even on the first client call;
preserve the original key and body for recovery. An observed unsupported runtime
operation returns `422 capability_unsupported`. Capability loss during a later
race remains a conflict or existing durable execution failure. Never silently
issue a fresh key against a newer turn.

These are authenticated human/API controls, also exposed by both CMA SDKs.
Verified platform-sender controls and web transport rollout have separate
acceptance gates. Existing v1 controls and standard Responses events retain
their established contracts.

```python
from openai import OpenAI
client = OpenAI(base_url="https://cma.example/api/v2", api_key="CURRENT_CMA_CREDENTIAL")
for prompt, key in [("First question", "turn-one"), ("Follow up", "turn-two")]:
    for event in client.responses.create(
        conversation="agt_EXISTING", input=prompt, stream=True,
        extra_headers={"Idempotency-Key": key},
    ):
        if event.type == "response.output_text.delta":
            print(event.delta, end="")
```

This subset follows the official OpenAI
[create contract](https://developers.openai.com/api/reference/resources/responses/methods/create)
and [streaming event shapes](https://developers.openai.com/api/reference/resources/responses/streaming-events).
Pinned official-client fixtures for v2 are
[`openai_python_v2.py`](../api/crates/cma/tests/official_responses/openai_python_v2.py)
and [`openai_node_v2.mjs`](../api/crates/cma/tests/official_responses/openai_node_v2.mjs).
The pinned `openai-node` 7.10.0 (Node 25.5.0) and `openai-python` 3.8.0
(Python 3.13) fixtures completed two conversation-only turns through a disposable
CMA HTTP server, the real command scheduler and personal execution adapter, and a
fixture runtime that commits output. Each exercised nine raw/helper events, four
conversation items, alias retries, retrieval, replay and named refusals. Python's
aggregate helper requires a model argument; the fixture uses the first response's
pinned model. The unchanged v1 fixtures also passed on that server.

Intermittent Python `503 response_unavailable` was reproduced while a new Agent
had durably opened its native conversation but had not yet published Ready.
History now recognizes that intermediate state only with the exact open receipt
and matching completion log. It creates no synthetic conversation event or turn;
missing or mismatched published history remains corrupt. A deterministic actual
handler test freezes that startup phase, and all four official/CMA clients passed
two turns and recovery concurrently after the fix, without retries. An earlier
eight-Agent probe hit Agent-creation capacity (429). These are local fixture
results; production rollout and previous-version continuity remain release gates.

CMA SDKs expose this contract separately as TypeScript `responsesV2` and Rust
`responses_v2()`. Their stream receipt callbacks/accessors expose durable IDs
before frames; replay is explicit and never resubmits input. The v1 SDK methods
retain their previous behavior. Both CMA SDKs also completed two turns with receipt,
alias, retrieval, mid-message replay and item checks concurrently with the pinned
official clients on the disposable worker fixture. Reproduce with
[`responses-v2-fixture.mjs`](../sdk/typescript/scripts/responses-v2-fixture.mjs) and
[`responses_v2_fixture.rs`](../sdk/rust/tests/responses_v2_fixture.rs), setting
`CMA_SDK_FIXTURE_ROOT` to the loopback server root; Rust requires `--ignored`.
See the SDK streaming guides for recovery examples.

### CMA queued response cancellation

Use `/api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation` for
a Message response with no durable runtime dispatch or assigned turn. Both GET
and POST require current human or nondelegated API `operate_agent` authority;
history access alone does not permit eligibility observation. An unbound Channel
response held in `waiting_action` is also eligible under that human/API authority.
This endpoint does not authorize an external platform sender to cancel work.

GET returns object `cma.response.queued_cancellation.observation`, the existing
safe `response` command-status view, `command_revision`, and `cancellable`.
The revision identifies the source command, independently of the Agent aggregate.
POST requires exactly one retained `Idempotency-Key` and this strict body:

```json
{
  "source_command_id": "cmd_SOURCE",
  "expected_command_revision": 1
}
```

A `200` response has object `cma.response.queued_cancellation`, a `cancellation`
command-status view, a current source `response` view, and immutable `target`
containing `response_id`, `source_command_id`, and `expected_command_revision`.
The transaction has already settled the source as `cancelled` and the cancellation
receipt as `completed`. The cancellation receipt has no response ID, turn,
delivery, observed cursor, or failure. It can be polled using the existing command
status endpoint. Cancellation works while the Agent is paused or has no runtime
and does not consume a waiting queue slot or create an Agent turn or log event.

A worker claim alone does not prevent cancellation: the exact source lease is
invalidated atomically. Any durable dispatch binding, including uncertain dispatch
or a pre-write failure, prevents this operation from applying. For an unbound
cancelled response, standard Responses replay emits `response.created` followed
by `response.incomplete` with reason `interrupted`; it emits no fabricated
`response.in_progress`, output item, or turn event.

After a lost or uncertain reply, retry the original key and exact body. An existing
receipt recovers before current source eligibility is checked. The cancellation
receipt and target remain immutable; the accompanying source status is a separate
current view. Reusing the key with a changed target or revision returns
`409 agent_command_key_conflict`. The key shares the existing Interrupt scope;
an incompatible older control receipt also returns that conflict.

`409 queued_response_cancellation_not_admitted` is definitive: the server committed
that the original scope/key has no receipt and the source command revision is now
strictly newer. Its source and resource writes exclude an older cancellation from
committing afterward. Only this code permits discarding that old intent before a
fresh observation and explicit new action. `409 queued_response_cancellation_conflict`
is uncertain; retain the original body and key. Unknown database commits and
unavailable authority return an unavailable response, never a refusal proof.
Success and definitive refusal are exposed only after a final current authority
check. Unknown fields, invalid revisions, and query parameters return
`422 invalid_queued_cancellation_request`; malformed JSON returns `400 invalid_json`.
Missing or invalid keys return `422 invalid_idempotency_key`.

## Retained Responses v1 contract

CMA serves a stateful, streaming subset of the OpenAI Responses API at
`/api/v1`. It is a Layer 1 projection of the same durable Agents used by
the other interfaces: creating a conversation creates an Agent context, and
creating a response sends one turn through the shared Layer 2 operation path.
The conversation identifier is the Agent id. There is no separate Chat model
or `surface` selector.

The subset is deliberately closed. A field or operation outside it is refused
with a named `422 application/problem+json` response rather than accepted and
ignored silently.

See [API_Overview.md](./API_Overview.md) for shared lifecycle semantics and
[API_REST.md](./API_REST.md) for authentication, request limits, errors, and
deployment-wide rate limiting.

---

## 1. Supported resources

All routes accept either a CMA browser cookie or a bearer credential under the
REST authentication contract.

| Method and route | Supported behavior |
|---|---|
| `POST /api/v1/conversations` | Create a Worktree-backed Agent using defaults or owned context overrides |
| `GET /api/v1/conversations/{id}` | Read one caller-owned Agent as a conversation |
| `GET /api/v1/conversations/{id}/items` | Read newest-first durable history |
| `POST /api/v1/responses` | Admit one turn and stream it as SSE |
| `GET /api/v1/responses/{response_id}` | Reconstruct one durable response |
| `DELETE /api/v1/responses/{response_id}` | Explicit authenticated `422` refusal |
| `POST /api/v1/responses/{response_id}/cancel` | Explicit authenticated `422` refusal |

Unknown identifiers, conversations owned by another subject,
and response identifiers that do not name a retained turn
all return the same `404`. This concealment prevents resource existence from
crossing an ownership boundary.

Conversation identifiers are CMA Agent identifiers. Conversation metadata
includes `agent_profile`, `workspace` and `sandbox`, plus server-owned `title`
when one exists. Response identifiers are stable projections of the Agent and
the durable turn sequence:

```text
resp_<agent-id>_<turn-sequence>
```

User message item identifiers use their durable event sequence. Assistant
message identifiers use the response turn sequence. Conversation items are
returned newest first with an OpenAI list envelope. `limit` accepts `1` through
`100`; `after` must be an item identifier from the same conversation.

The standard conversation and item behavior used as the compatibility point is
documented in the official OpenAI references for
[creating conversations](https://developers.openai.com/api/reference/resources/conversations/methods/create.md),
[retrieving conversations](https://developers.openai.com/api/reference/resources/conversations/methods/retrieve.md),
and [listing conversation items](https://developers.openai.com/api/reference/resources/conversations/subresources/items/methods/list.md).

---

## 2. Response field policy

`POST /api/v1/responses` accepts a JSON object. The final user content must be
either a non-empty `input` string or one array item containing a single user
message with text content.

| Field | CMA v1 policy |
|---|---|
| `conversation` | Required and honored; must name a caller-owned Agent |
| `input` | Required and honored; exactly one user text message |
| `stream` | Required as literal `true` |
| `metadata` | Honored; at most 16 string pairs, with bounded keys and values |
| `previous_response_id` | Honored as the linear-thread predecessor fence |
| `model` | Optional string, accepted but ignored; response retains the Agent conversation model selected when its turn started |
| `store` | Omitted or `true`; `false` is refused because chat history is durable |
| `instructions` | Refused by name |
| `reasoning` | Refused by name |
| `tools` and `tool_choice` | Refused by name; caller-defined tools are unavailable |
| `background` | Refused by name |
| `stream_options`, `starting_after` and `include` | Refused by name; stream resume is unavailable |
| Any unknown field | Refused by its supplied field name |

Conversation creation accepts `{}` or metadata containing only owned
`agent_profile`, `workspace` and `sandbox` identifiers. Each supplied value is
nonempty, at most 128 bytes and contains no control characters. Unknown keys,
including caller-supplied `title`, are refused. Omitted context is resolved
through platform defaults. Specifying both Workspace and Sandbox requires
matching owned lineage; it does not move the Workspace.

If default-context selection needs a new Sandbox, its owner must first establish
renewable authority through CMA's public `/auth/login` flow with the same account.
Missing setup returns `409 sandbox_credential_setup_required` before Sandbox
admission. An existing admitted context does not require this setup merely for
reuse. Conversation creation is unkeyed: the caller may retry that definitive
refusal after sign-in, but must not automatically replay an ambiguous creation
failure. An `Idempotency-Key` does not make this endpoint replay-safe.

```json
{
  "metadata": {
    "agent_profile": "agp_example",
    "workspace": "wks_example",
    "sandbox": "sbx_example"
  }
}
```

The `metadata` on a response POST is separate: it accepts at most 16 string
pairs, with keys at most 64 characters and values at most 512 characters.

Threads are linear. An absent/null `previous_response_id` requires no previous
turn; it never means append to whichever turn is latest. A later invocation
must name the latest `resp_<agent-id>_<turn>` identifier for that conversation.
Malformed or cross-conversation predecessors are refused statically. A command
that reaches dispatch with a stale predecessor receives the committed
`409 previous_response_id_conflict` refusal before any runtime send. Ordinary
input may wait behind active work, so this refusal can arrive after admission.
A keyed retry recovers its original receipt before rechecking a new dynamic
predecessor condition. To change the predecessor or input after a definitive
refusal, make a new explicit invocation with a new key.

One optional `Idempotency-Key` accepts 1–256 bytes without control characters.
Keep it with the input, metadata and predecessor before sending. The supplied
key uses the common stable authority/Agent/command-kind namespace directly.
Equivalent canonical intent recovers the same receipt across interfaces;
Append and `AfterTurn(None)` are different semantics and conflict under the
same key. The optional ignored `model` does not change intent. Without a key,
separate HTTP attempts reserve separate commands and cannot safely deduplicate
an uncertain earlier attempt. Conversation creation remains unkeyed.

Stateless response creation, non-streaming creation, background execution,
stream resumption, caller tools, response deletion, and response cancellation
are not part of this subset. The authenticated DELETE and cancel routes exist
so those absences are discoverable and typed. The request and retrieval shapes
track the official OpenAI references for
[creating responses](https://developers.openai.com/api/reference/resources/responses/methods/create.md)
and [retrieving responses](https://developers.openai.com/api/reference/resources/responses/methods/retrieve.md).

---

## 3. Stream contract

Successful response creation returns `200 text/event-stream`. Every data frame
has a `type` and a zero-based `sequence_number`; sequence numbers are contiguous
for that connection. The SSE `event` field repeats `type`.

| Event | Meaning |
|---|---|
| `response.created` | The turn was admitted; the projected response is initially in progress |
| `response.in_progress` | Initial lifecycle projection is open |
| `response.reasoning_summary_part.added` | A live reasoning summary part began |
| `response.reasoning_summary_text.delta` | Live-only reasoning summary text |
| `response.reasoning_summary_text.done` | The live reasoning text ended |
| `response.reasoning_summary_part.done` | The live reasoning summary part ended |
| `response.output_item.added` | The assistant message began |
| `response.content_part.added` | Its output-text part began |
| `response.output_text.delta` | Assistant text delta |
| `response.output_text.done` | Durable assistant text is complete |
| `response.content_part.done` | The completed output-text part |
| `response.output_item.done` | The completed assistant message |
| `response.cma_chat.tool_activity` | Namespaced CMA tool activity extension |
| `response.completed` | Durable turn completion with usage |
| `response.failed` | Terminal failure with `response.error` |
| `response.incomplete` | Terminal incomplete response with details |

`response.cma_chat.tool_activity` carries an `item` whose type is
`cma_chat.tool_activity`. It includes the tool call id, kind, target and status;
`duration_ms` is currently null. Tool updates without a retained kind use
`"unknown"`. Argument and result bodies are not part of this projection.
The same extension item can appear in durable conversation history.

Reasoning has a narrower lifetime. Live `ThoughtDelta` frames project into the
reasoning-summary events above, but reasoning text is never written into
conversation items and is never reconstructed by response retrieval. A client
that was disconnected cannot recover missed reasoning. Assistant text, tool
activity retained by policy, status, metadata, and usage are reconstructed
from the durable event log.

The standard event vocabulary used here follows the official
[Responses streaming events reference](https://developers.openai.com/api/reference/resources/responses/streaming-events.md)
and [streaming guide](https://developers.openai.com/api/docs/guides/streaming-responses.md).
The two `cma_chat.*` types are CMA extensions, not OpenAI event or item types.

---

## 4. Disconnects and recovery

The response turn is owned by CMA after admission. Dropping the HTTP response
body, navigating away, aborting a client-side reader, or losing the connection
does not cancel the core turn. There is no response-stream resume endpoint in
v1. Reconnect by reading the response resource when its identifier is known and
list conversation items to recover durable output. If a terminal response is
not yet available, retry the read under the ordinary REST retry policy.

CMA first reserves the common durable command, then the worker handles queue
order, checkpoint waits, revival and execution. An ordinary Message waits behind
an active turn. `200 text/event-stream` opens only when this exact command has a
durable turn binding. A keyed retry projects that same legacy response again,
with connection-local sequence numbers restarting at zero.

The response headers carry `x-cma-command-id` and `x-cma-response-id`, also on
prebinding errors when admission is known. These identify the common command
and response; they never replace legacy `response.id`, output IDs or
`previous_response_id`. Observe the command through
`GET /api/v2/commands/{command_id}` or repeat an uncertain keyed POST with the
same input and key under current authority. The common response can also be
observed through v2 without changing its legacy turn projection.

Prebinding observation is bounded by ten minutes; the command queue has its
existing five-minute deadline. `504 agent_command_observation_timeout` reports
that no binding was observed within the HTTP budget and leaves work intact.
A committed queue expiry returns `agent_command_queue_timeout`; other committed
unbound failures use explicit authority, Agent, target, runtime or execution
problems. A cancelled queued command returns `409 agent_command_cancelled`.
These errors do not fabricate a `response.failed` event or a `resp_<agent-id>_0`
resource. After a stream opens, loss of authorized history observation closes
the connection without inventing a durable runtime failure.

New keyed admission cannot establish whether a request already executed on an
older server that ignored its key. Do not infer an old receipt from matching
text or nearby history. During cutover, route mutations to upgraded writers and
drain old direct-admission tasks; a rollback must keep the common worker and
ledger available for accepted commands or stop new mutations while they settle.
Retained reads still reconstruct old turns that have no common receipt.

Response retrieval never reads a response database. It identifies the durable
turn, bounds it at the next `TurnStarted`, and folds the retained event log into
output, status, metadata, configured model, and usage. An incomplete historical
turn followed by a newer turn is treated as corrupt history and is not exposed
as a still-live response.

The stateful design corresponds to the official OpenAI
[conversation state guide](https://developers.openai.com/api/docs/guides/conversation-state.md),
with CMA's stricter required-conversation and linear-predecessor rules declared
above.

---

## 5. Declaration

`GET /api/v1/conformance` includes:

```text
https://spec.chrono-ai.fun/cma/1.0/conf/responses-chat
```

The Responses interface projects Agent-context creation, input, retained output
and usage. It does not grant additional capabilities to an Agent, change its
profile, create a terminal or add a Trigger mutation route.

---

## 6. Official SDK conformance

Local compatibility was verified on 2026-09-07 with `openai-node` 7.10.0 on
Node 26.8.1 and `openai-python` 3.8.0 on Python 3.13.11. Both clients passed
against CMA's loopback-only `official_responses_sdk_server` test composition.
The checked-in runners are
[`openai_node.mjs`](../api/crates/cma/tests/official_responses/openai_node.mjs)
and [`openai_python.py`](../api/crates/cma/tests/official_responses/openai_python.py).

Each client created and retrieved an Agent conversation in its default owned
context, read 18 raw streaming frames, consumed 16 aggregate-helper frames,
and paginated three durable conversation items. Assertions covered contiguous
event sequences, live reasoning, CMA tool-activity extensions, reconstructed
assistant output and metadata, and the typed linear-predecessor conflict.
Reasoning was absent from reconstructed history and completed output.

The fixture uses real CMA public handlers, Core context/operation services and
durable update recording with an in-memory store and a scripted runtime port.
It does not contact OpenAI or NyxID, invoke a real agent kernel, or prove
production provisioning or storage. Its owned Sandbox and Workspace already
exist; fresh-Sandbox/default-context admission and context override validation
are covered separately by backend tests and the shared public contract corpus.

Run the ignored server test explicitly, never as part of an unfiltered ignored
test run. It prints `CMA_SDK_BASE_URL`; both runners require that exact loopback
URL. The Node runner also requires `CMA_NODE_SDK_ENTRY` pointing at the installed
SDK's `index.mjs`; the Python runner uses its environment's installed `openai`
package. No real API key is needed. The fixture accepts only its static local
test credential and exposes an authenticated `POST /__sdk/shutdown` for orderly
shutdown, with a ten-minute lifetime bound as a fallback.

Agent chronology, bounded snapshot/tail replay, ephemeral text and Thinking, and
authenticated new-tab transport eligibility are documented in
[Agent event replay v2](API_Agent_Events_V2.md). Agent cursors are independent of
Responses sequence numbers; retained response bindings identify control targets.
