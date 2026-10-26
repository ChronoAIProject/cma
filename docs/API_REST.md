# REST Interface

This is the public HTTP contract for CMA's Sandbox model. The served
`GET /openapi.json` is OpenAPI 3.1 and supplies exact JSON schemas, parameters,
security declarations and response media types. The operation inventory below
is checked against that document, including its AG-UI and Responses projections.
For the focused Agent lifecycle and Responses v2 workflow, use
`GET /agent-manager/openapi.json` and [Agent Manager API](API_Agent_Manager.md).
It preserves existing operations and removes unrelated paths and unreachable schemas.
[CMA Model](CMA_Model.md) defines lifecycle and profile semantics;
[CMA Trigger](CMA_Trigger.md) defines publishing, consent and visitor reuse.
[CMAEG protocol](CMAEG_Protocol.md) defines channel ingress, provider bindings
and explicit outbound delivery through the separate Event Gateway.

There are no `/sessions`, `/chats`, environment-profile, contribution,
sponsorship or terminal-attachment APIs in this release. Resource management
uses Sandbox, Workspace, Worktree and Agent ids. A resource's readable name is
not an id or a concurrency fence.

## Inline Agent creation

`POST /api/v1/agents` accepts a name, an optional Image Manager reference,
optional GitHub repository, and inline Agent settings. The browser's Agent
Manager proxy exposes this at `/api/agent-manager/v1/agents`. `Idempotency-Key`
is required. A `202` receipt contains an **operation** ID; read
`GET /api/v1/agent-lifecycle/{id}` until `status` is `succeeded` and then use its
`agent_id` to open the Agent. A phase event marked `ready` does not complete the
operation. Workspace/runtime readiness is reported only after Agent preparation.

To add an Agent to an existing owned, ready Workspace, pass `workspace_id` to
the same lifecycle endpoint. This path retains the Workspace and parent Sandbox;
it rejects `image` and `repository` because those belong to the existing
environment. Omitted or empty service selections inherit the parent's connected
key scope, and requested services outside that scope are refused. The caller
needs the existing Workspace's creation authority but does not need
`CreateSandbox`. Admission and every resumed worker validate the current
Workspace/Sandbox ownership, readiness and connection prerequisites before
creating the inline Agent. The durable operation key also binds `workspace_id`;
replaying it for another Workspace or for a new Sandbox conflicts.

The permanent operation ledger binds a key to one owner's submitted request.
Retries keep that exact body and key, including after a lost HTTP response.
The originally resolved image digest and captured settings remain pinned even
if the image catalogue advances. A different request under that key is refused.
New operations also capture the configured CPU/memory allocation. Legacy
operations retain their original minimum reservation and immutable limits.

`waiting_for_connection` carries `connection.services` and `connection.target`
for the connection wizard. After verifying the same owner's connections, repeat
the original POST with the same body and key to resume that operation. Saved
child identities are retained. This lifecycle ledger handles replay directly;
the generic 24-hour HTTP response cache does not intercept resumption. The
browser retains the pending request and receipt in owner-scoped session storage.

Workers discover eligible operations every 10 seconds, process at most 16 per
replica, and renew a 90-second claim every 20 seconds. Every status/event/child
write checks the current claim token and expiry. Stage events are replay-safe.
The Agent Manager's restricted database role includes `agent_lifecycle_operations`.

Each operation also supplies the durable admission identity for its Sandbox and
Agent. Concurrent workers and retries after an admission response is lost recover
the same children, including when the owner's quota has since been exhausted.
The original authenticated actor remains attached to Agent creation. Legacy
records without a captured actor retain unknown attribution.

Temporary dependency failures and readiness timeouts return the operation to
`queued`, with durable retry delays of 5, 10, 20, 40 and then 60 seconds. Periodic
discovery observes this delay. Child identities and completed stages survive
each retry. Terminal resource conflicts remain `failed`.

Image Manager authentication failures return `401 not_authenticated`; an image
permission refusal returns `403 permission_denied`. These are distinct from a
missing NyxID service connection and do not provide a connection-wizard target.

## Authentication And HTTP

Use `Authorization: Bearer <credential>` for backend calls, or the same-origin
CMA browser cookie. Never embed credentials in query strings, HTML or badges.
Cookie requests are subject to origin defenses; cross-origin browser clients
must use an explicitly configured CORS origin. Allowed request headers include
`Content-Type`, `Authorization`, `Last-Event-ID`, `Idempotency-Key` and
`X-Request-Id`; exposed response headers include `Link`, `Retry-After` and
`X-Request-Id`. CORS is not authorization.

An unavailable identity provider, including a rate-limited verification request,
returns a sanitized `503` rather than reporting an invalid login. An invalid,
expired or revoked credential remains an authentication refusal. Overlapping
verification of the same bearer can share one provider request sequence; a later
request verifies it again. A temporary verification error does not authorize a
request or change the idempotency key of an uncertain command.

Creating a new Sandbox also requires renewable owner delegation already held by
CMA. A request bearer does not establish renewable Sandbox authority.
Complete the existing `/auth/login` OAuth
flow with the same account to establish delegation. Bearer SDK calls remain
supported after this setup; the browser session need not accompany them.

Agent input and deferred controls independently retain the exact ordinary NyxID
user access token supplied by an API caller, encrypted until its verified expiry.
This command custody does not replace or refresh the account's saved OAuth login.
NyxID introspection must confirm an active access token, matching subject, current
permissions and explicit expiry; restricted machine, delegated, relay, scoped and
sender-constrained tokens are refused. Existing browser requests continue using
their original sign-in custody. See [Responses](API_Responses.md) for admission
and recovery behavior.

`POST /api/v1/sandboxes`, public Launch creation, and Draft Launch start return
`409 sandbox_credential_setup_required` when a new Sandbox is needed but that
delegation is missing or no longer renewable. No Sandbox is admitted, and a
refused Draft start leaves the Launch unchanged. Reused Sandboxes and Agents do
not acquire this prerequisite merely for reuse. Identity-store or provider
outages remain sanitized `503` failures, not a request to repeat sign-in.
Default-context `POST /api/v1/conversations` inherits this prerequisite when it
needs a new Sandbox. That protocol-compatible operation is unkeyed: retry only
a definitive setup refusal after sign-in, not an ambiguous creation failure.

Use the SDK's `auth.loginUrl` or `auth().login_url` with a canonical CMA return
path, preserve the pending command without any credential, and verify that the
returned identity is the same subject. A successful browser session alone does
not prove the provider supplied renewable delegation: the next admission still
checks it. For keyed operations, after a definitive setup-required refusal and
successful sign-in, explicitly retry with a **new** idempotency key. The previous
key continues to replay its exact refusal; transport-ambiguous retries must
retain the original key and body. Neither OAuth callback nor return navigation
starts work.

Send `Content-Type: application/json` when a route has a JSON body. An empty
object and an absent body are not interchangeable: Agent stop requires `{}`
(or its documented reason field), whereas interrupt, pause and revive have no
request schema. Request bodies are capped at 2 MiB before per-field validation.
Unknown request fields are refused on closed DTOs. Integers, enums and profile
references are validated, not coerced from arbitrary strings.

Owner routes return the same `404` for missing and foreign resources. An
administrator uses the capability-gated `/admin` operations rather than
impersonating another owner. Public preview and badge access does not authorize
Launch creation. Visit [API Overview](API_Overview.md#authentication-and-visitors)
for browser identity and visitor capabilities.

The composed identity/discovery/static routes also include:

| Operation | Meaning |
|---|---|
| `GET /openapi.json` | Anonymous machine-readable contract |
| `GET /auth/login` | Begin NyxID OAuth; return destination is validated |
| `GET /auth/callback` | Complete identity exchange; never starts Agent work |
| `GET /auth/session` | Browser identity, capabilities and visitor flag; 401 when signed out |
| `POST /auth/logout` | Revoke browser login and expire cookie; redirect to `/` |
| `GET /embed/v1.js` | Fixed versioned-by-integrity embed script; no credential access |
| `POST /api/v1/mcp` | Typed 501 `interface_not_available` |
| `GET /.well-known/agent-card.json` | Public provider card for binding Responses; A2A JSON-RPC remains unavailable |
| `/api/v1/a2a` | Typed 501 `interface_not_available` |

`GET /auth/signup-options` appears in the checked inventory. Its four-field
response is exactly `{ email_signup, social_providers, invite_code_required,
register_available }`. CMA reports registration availability but never creates
NyxID users or roles. Anonymous preview/badge GETs and OAuth callbacks are inert.

## Legacy Event Gateway storage protocol

The three `/api/v1/gateway/*-records` namespaces below are retired from Agent
Manager deployment and its OpenAPI. Event Gateway now owns its sealed storage in
`eg_channel_records`, with an explicit legacy import command. These protocol
descriptions and the SDK compatibility methods remain for older installations
and migration tests; they are not endpoints on the current Agent Manager. Their
eleven operations are classified `legacy_gateway` in the shared contract corpus.

The `/api/v1/gateway/channel-records` routes store opaque, base64-encoded CMAEG
channel-definition blobs for callers with the NyxID permission
`cma:gateway:records`. CMA does not decrypt or interpret the blob. Record ids
are canonical UUID v4 values, blobs contain 1 to 65,536 decoded bytes, and all
responses carry `Cache-Control: no-store`.

`PUT /api/v1/gateway/channel-records/{record_id}` accepts
`{ owner, blob, expected_version }` and returns the new version and update time.
An omitted expected version creates a record; a supplied version performs a
compare-and-set update. A stale version returns `409 conflict`. `GET` returns
the full base64 blob and returns `404 gateway_record_not_found` when absent.
`DELETE` accepts the optional `expected_version` query parameter and returns
`204` on deletion, `404 gateway_record_not_found` when absent, or `409 conflict`
for a stale version.

`GET /api/v1/gateway/channel-records?owner=&after=&limit=` lists stable
record-id order and returns `{ items, next_after }`. The default limit is 100
and valid limits are 1 through 200. Listing and all mutations require the same
capability; an authenticated caller without it receives `403 forbidden`.

The `/api/v1/gateway/connection-records` namespace provides the same list, read,
compare-and-set write and delete contract in an independent collection. CMAEG
uses it for encrypted platform connection setup, credential claims and durable
completion notifications. The same `cma:gateway:records` permission applies;
record IDs, versions and deletions are isolated from channel records. CMA treats
these blobs as opaque and does not own platform credentials or setup logic.

## Event Gateway Interaction Records

`PUT`, `GET`, and `DELETE /api/v1/gateway/interaction-records/{record_id}` use a
separate namespace for encrypted, expiring interaction state. They require
`cma:gateway:records`, accept the same canonical UUID v4 record identifiers and
1–65,536 decoded-byte blob limit as channel records, and return
`Cache-Control: no-store`. The calling gateway owns encryption and interaction
semantics; CMA stores opaque bytes with durable version and expiry checks.

`PUT` accepts `{ owner, blob, expected_version, expires_at }`, where `blob` is
base64 and `expires_at` is a Unix timestamp in seconds strictly after now and at
most seven days in the future. An omitted/null `expected_version` creates
version 1. Updates require the current version and exactly the same owner and
expiry; neither ownership nor retention can change through an update. The
response is `{ record_id, version, updated_at }`. Conflicting creation, stale
versions, or changed ownership/expiry return `409 conflict`.

`GET` returns `{ record_id, owner, blob, version, created_at, updated_at }`.
`DELETE` requires the `expected_version` query parameter and returns `204` after
deletion. Missing or expired records return an empty `404` for both operations;
expiry is enforced before MongoDB's asynchronous TTL cleanup. These records are
not included in the channel-record listing. Invalid identifiers, owners, base64
or expiry return `400 invalid_gateway_record`; an invalid decoded blob size
returns `413 gateway_record_too_large`. Insufficient authority returns
`403 forbidden`, and unavailable storage returns `503 store_failed`.

## Agent-delegated CMA operations

These provider APIs require the configured Bot Father binding credential and a
current event reference or CMA-issued callback continuation. User identity is
resolved from the original admitted event, its retained agent/worktree and the
current CMA consent grant. Request bodies cannot supply user credentials or a
grant ID. A supplied organization owner selects the requested context; current
membership and each operation's existing permissions determine access.

`POST /api/v1/agent-delegations/identity` accepts
`{"event_ref":"<current event or CMA continuation>"}` and returns the linked
identity or CMA browser sign-in URL. The browser uses fresh PKCE authentication
and explicit consent. Consent activation and the agent callback are durable and
recover after API restarts. Ordinary questions do not require consent.

`POST /api/v1/agent-delegations/operations` accepts:

```json
{
  "event_ref": "<current event or CMA continuation>",
  "owner": { "kind": "organization", "subject": "<organization ID>" },
  "method": "GET",
  "path": "/api/v1/sandboxes?organization=<organization ID>"
}
```

Omit `owner` for Personal authority. `body` carries a mutation's ordinary API
body and every mutation requires `idempotency_key`. The allowed operations cover
organization discovery, ordinary sandbox/workspace/agent/profile reads and changes, Marketplace reads
and forks, Responses execution and controls. Existing resource handlers enforce
permissions and durable admission; deferred commands retain the original user
and delegation. Administrative, authentication, gateway-record and arbitrary
proxy destinations are not accepted. Bot creation uses the reviewed setup flow
below. An organization owner cannot be used with legacy creation routes that
implicitly create Personal resources: CMA refuses those calls before dispatch.
Organization Bot creation remains available through the typed setup workflow.
Paths must be canonical local API paths; no URLs or caller headers are forwarded.

A completed operation returns `{"status":200,"body":<normal API result>}`; the
inner status can also be the ordinary API's problem response. Responses
admission returns an inner `202` with `response_id` and `command_id`, allowing
normal polling without exposing the internal stream. Outer refusal means the
binding, source, consent or requested operation could not be authorized.
Responses never return the user's renewable credential. Request bodies are
limited to 256 KiB and operation results to 1 MiB. Authority and source mapping
are rechecked after dispatch; mutations must reuse their original idempotency
key after an uncertain response.

## Bot Father Setup Workflow

Bot Father is an ordinary CMA agent with a specialized skill. CMAEG forwards all
messages, commands, login returns and interaction events through its normal
provider path; it contains no Bot Father dispatch, command parser or setup UI.
The agent interprets the user's intent and calls CMA's public workflow API.

`POST /api/v1/bot-setups/actions` applies an agent-selected operation to the
verified sender's durable plan. It requires the configured Bot Father binding
credential and the **current** sealed `event_ref` or a CMA callback continuation
reference (at most 4096 bytes). CMA asks
the gateway's generic context endpoint for the original Activity retained with
the provider turn, verifies the binding/private-chat sender, and derives the
actor identity from that event. The request cannot supply or rewrite Activity.

```json
{
  "event_ref": "<current event reference>",
  "request": {
    "request_id": "<unique request UUID>",
    "expected": { "setup_id": "<returned setup ID>", "revision": 2 },
    "action": { "type": "choose", "choice_id": "personal" }
  }
}
```

The typed actions are:

| Action | Additional fields | Behavior |
| --- | --- | --- |
| `view` | none; `expected` is optional | Open/resume the sender's plan, process a verified login return, and return current choices/progress. |
| `draft` | `draft: {name?, instructions?}` | Record conversationally supplied name/instructions at Login, Owner or Name. |
| `choose` | `choice_id` from the current view | Select an owner, defaults/customization, service, channel Bot, admission rule or navigation action. |
| `customize` | `customization: {agent?, workspace?, sandbox?, availability?}` | Submit the complete profile for the current Customize step. The agent prepares JSON from the user's preferences. |
| `confirm` | `plan_digest` from the review | Admit the reviewed plan only with separate signed user consent. |

Every mutation requires the returned setup ID and revision. Several distinct
operations can use the same human event, allowing the agent to apply a user's
combined preferences without additional messages. Repeat an uncertain request
with the same ID, event, revision and payload. A changed payload under a recorded
mutation ID or a stale revision returns `409 setup_changed`; a matching replay
returns the retained view without repeating the operation. An unchanged `view`
is a repeatable read and does not reserve a mutation receipt.

The sender follows the CMA-owned sign-in link, completes NyxID login and explicitly
authorizes the displayed Telegram actor in the browser. CMA then automatically
notifies the initiating agent through a durable callback; no chat proof or extra
message is required. The callback continuation resolves the original signed
human input, so login itself cannot confirm resource creation. CMA retains the
renewable credential and verifies Personal or organization ownership, profiles,
owner-specific services, connections and channel Bots. No user token is exposed
to the agent.

The response includes `setup_id`, `revision`, `step`, `interaction_id`,
`identity`, `owner`, `name`, `configuration`, `choices`, `services`,
`connection_url`, `external_bot_id`, `admission`, `confirmation_digest`, `result`
and `notice`. The agent explains those choices naturally and uses the generic
gateway reply/interaction tools when suitable. Users do not submit slash
commands or configuration JSON.

Before creation, show the complete effective plan and request explicit
confirmation. `confirm` requires the exact plan digest and revision plus a new
verified event created after the review. The original event must contain clear
creation consent (for example `Create Bot` or `确认创建`) or the matching generic
interaction action. An agent action alone, a greeting, an old event or the event
that prepared the review cannot authorize provisioning. CMA records the plan
before using the existing durable Bot worker to create resources. Report
completion only when `result` confirms the Bot is active.

`POST /api/v1/bot-setups/advance` remains a compatibility interface accepting
`{event_ref, activity?, draft?, customization?}`. Supplied Activity must pass the
gateway's generic digest attestation; text is retained verbatim. New agent
integrations use `actions`. Legacy encrypted setup contexts remain readable until
their event expires; ordinary model turns retain their context through the normal
provider contract. Plans and action receipts survive replica restarts.

Responses use `Cache-Control: no-store`. Named workflow failures are
`400 invalid_setup`, `403 setup_authority_required`, `404 setup_not_found`,
`409 setup_changed`, and `503 setup_unavailable`. Authentication failures are
rejected before the handler. The gateway's independent event/reply integration
protocol is hosted at https://bot.chrono-ai.fun/event-gateway/docs .

## Provider Bindings

The CMAEG service principal manages bindings under `/api/v1/cmaeg/bindings`.
Every operation requires both `cma:gateway:records` and `cma:provider:bindings`.
The binding records the authenticated manager separately from the resource owner
in the profile. A different manager cannot read or replace that binding.
Responses use `Cache-Control: no-store` and never contain supplied key material.

`PUT /api/v1/cmaeg/bindings/{binding_id}` accepts
`{schema_version:1,profile,profile_digest,agent_key,tools:{native_mcp_url,nyxid_service_slug}}`.
The digest is `sha256:` followed by the canonical profile digest. The response
contains `binding_id`, `owner`, `profile_digest`, `sandbox_id`, `workspace_id`,
`level`, `honoured`, `refused`, `state`, `generation`, and timestamps. The current
turn-contract level is `l2` (text and informational activity items). Native tools
require the exact configured CMAEG endpoint and a runtime with proven proxy
support; unsupported endpoints are explicitly refused. Resource
provisioning is asynchronous: `GET` reports `provisioning`, `ready`, `failed`
(with a reason), or `retired`. Repeating the same digest and key returns the
retained reservation. A supplied key hash can belong to only one binding;
retired records retain that reservation. Reusing the key on another binding
returns a conflict. A changed profile waits for prior work to settle, retires
old conversation Agents, and keeps compatible Sandbox and Workspace snapshots.

The optional `target` field selects `{"type":"profile"}` (the default) or
`{"type":"existing_agent","agent_id":"agt_existing"}`. Existing-agent binding
checks the actual personal/organization resource owner, retains all existing
Agent configuration, and returns `level:"l1"` with automatic final replies.
The required profile envelope still supplies the owner and digest; its model,
environment and instruction settings are not applied. Every transport
conversation resolves to that same Agent through the canonical provider
conversation `existing_agent`. The response includes the non-default target.
Changing the target or profile of this association is rejected. Key rotation
changes only binding authentication, without changing the existing Sandbox
credential. Deleting an existing-agent binding or conversation only removes
the association and never retires the borrowed Agent or parent resources.

`PUT /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}` accepts
an empty body or `{}` and returns `{conversation_id,agent_id,state}` with 201
for a new reservation or 200 for a replay. Conversation ids contain 8–128 ASCII
letters, digits, underscores or hyphens. Each generation retains exactly one
Agent and Worktree per conversation id. Resource admission revives a paused
Sandbox through the existing lifecycle transaction. A binding that is not ready
returns `409 binding_not_ready`; malformed ids return `400 invalid_binding`.
Once reserved, a conversation still provisioning its Agent returns
`429 conversation_not_ready` and `Retry-After: 1` on Responses requests. Retry
the same input and idempotency key; a preparation refusal admits no turn.
Missing, foreign, failed or retired conversations remain 404.

A profile binding whose owned Sandbox or Workspace is missing or being removed
returns `409 binding_resources_unavailable` on binding GET/PUT and conversation
PUT, even if its retained record still says `ready`. Reconnect the channel to an
available Agent. These checks do not recreate resources, rotate credentials or
change conversation history. Paused and recovering resources retain their
existing lifecycle behavior, and borrowed Agent bindings keep their ownership
rules. Authorized DELETE remains available for stale bindings. Storage failures
remain `503 binding_unavailable`; they are not classified as deleted resources.
The gateway presents this specific refusal as an actionable 409 instead of a
generic provider 502.

CMA's optional profile extension `environment.session_affinity:"source_user"`
changes provider selection without changing gateway partitioning. CMA verifies
the original human Activity with the gateway, then maps binding generation,
platform and sender to one durable Agent/Worktree. Different chats from the same
sender reuse that Agent while retaining their original event reply addresses.
A transport conversation PUT returns `200 {conversation_id,state:"ready"}` without
`agent_id`; provisioning waits for authenticated source identity on the first
Responses call. Preparation returns `429 conversation_not_ready` with
`Retry-After: 2`. Transport conversation DELETE acknowledges without retiring any
user Agent; binding retirement owns their cleanup. The companion
`environment.isolated_agents:true` enables private filesystem/process/network
namespaces and constrained platform access for each new Agent. These are opaque
CMA provider profile extensions, not gateway user-routing policies.

`GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}`
returns the retained full event context to the authenticated binding credential.
For Father setup events it reads the encrypted workflow context described above;
for normal provider turns it reads the existing durable model-turn ledger. A
workflow context read rechecks the current binding before and after storage
access. Gateway reference verification and the `expires_at` deadline remain
part of the gateway's provider-integration protocol; these operations add no
native MCP tool names.

Both DELETE operations return 204 after durable retirement admission. Conversation
cleanup stops the Agent and removes its Worktree through the existing workers;
binding cleanup also retires its Sandbox. Once retired, a binding or conversation
id cannot be resurrected in the same generation. Supplied keys use encrypted
custody and are never minted, renewed, or revoked by the managed-key controller.

Binding PUT with the same profile digest and a changed supplied key rotates the
credential in place. The response reports `provisioning` until the replacement is
delivered (or retained for a fully paused Sandbox's revival); callers poll GET
until `ready` before revoking the previous externally managed key. Replays retain
the same credential generation. Existing Sandbox, Workspace and conversation
identifiers are preserved. A busy Sandbox lifecycle returns a conflict that can
be retried after the operation settles.

## Errors, Retry And Concurrency

Errors use `application/problem+json` with standard `type`, `title`, `status`,
`detail` and a stable `code`. Additional structured context may name a resource,
limit, current revision, required digest, cursor head or reset time. Inspect
`code` and HTTP status, not prose. Preserve `X-Request-Id` for support without
logging credentials or rejected secret-bearing input.

| Status | Typical handling |
|---|---|
| 400 | Correct malformed body, query, cursor or idempotency header |
| 401 | Reauthenticate; do not retry an unauthenticated mutation blindly |
| 403 | Capability refusal; changing the URL or resource id cannot grant access |
| 404 | Missing or concealed resource; do not infer another owner's existence |
| 409 | State, draft-version, expected-revision, trust or idempotency conflict; read current state |
| 422 | Unsupported operation or semantic validation refusal; correct input |
| 429 | Count/rate limit; respect the reset information and `Retry-After` when supplied |
| 503 | Dependency or durable-store unavailable; retry only within the operation's safety contract |

Every inventory row marked `key` requires `Idempotency-Key`. Generate a fresh
opaque key per logical mutation, retain its exact body, and reuse that pair when
the result is ambiguous. Reusing a key with changed content conflicts. An exact
replay returns the original status/body, not a fresh snapshot. In particular,
replaying a Draft Launch's creation key still returns its original 201 even
after the Launch has progressed; read the Launch to obtain current state.

`idempotency_in_flight` and `idempotency_recovery_required` leave the command
unsettled. Do not replace its key merely because a response was lost. Read-only
GET retries and SSE reconnects do not authorize a new input or create POST.
AG-UI run and Responses creation do not use this REST key contract: submit
once and recover retained output through reads.

Profiles use `draft_version` for draft save/publication. Settings use
`expected_revision`. These are server-issued optimistic concurrency values,
not timestamps or request ids. A conflict requires a new read and an explicit
reconciliation of the user's intended changes; never overwrite newer state
using an automatically substituted revision.

## Profile Authoring

The four `/api/v1/profile-authoring` operations require authentication and
`ManageOwnProfiles`. They are read-only lookups, including the repository
resolver's POST: no idempotency key, resource admission, profile edit, clone,
or Sandbox creation occurs. All successful responses are HTTP 200. Optional
output fields are omitted rather than returned as `null`.
Service and skill lookups use the current request's delegated authority when
present, including for bearer callers without a previous browser sign-in.
That authority is transient, never persisted by the lookup, and never included
in a public response or repository verification request.

`GET /api/v1/profile-authoring/bounds` returns
`{ min_cpu_millis, max_cpu_millis, min_memory_mib, max_memory_mib }` from the
same fixed deployment policy used to validate Sandbox Profiles. It does not
depend on catalogue or repository availability.

`GET /api/v1/profile-authoring/services` returns an array of owner-visible
`{ id, catalog_id?, name, description? }` choices. `id` is the selectable scope
identifier; `catalog_id`, when reported, is the source's canonical service
identity for opening its detail page and must not replace the profile's scope
identifier. An empty array is a successful empty
catalogue. Source failures return `503 profile_services_unavailable`, not an
empty catalogue or credentials.
Catalogue descriptions retain the complete source text up to 6144 UTF-8 bytes;
this authoring limit does not change the separate profile draft limits.

`GET /api/v1/profile-authoring/skills?q=&limit=&cursor=` returns
`{ items: [{ id, name, description?, versions: [string] }], next? }`. Omitted
`q` browses with an empty query; queries allow at most 200 UTF-8 bytes and no
control characters. The limit defaults to 20 and accepts 1-50. A supplied
cursor must be nonempty, at most 2048 bytes, and free of control characters.
Versions are up to 100 latest selectable, nondeprecated releases, ordered
newest-first by numeric `major.minor` and retained as strings. Existing profile
pins are not changed by catalogue filtering. A filtered page may contain no
items while retaining an advancing `next`; continue until `next` is absent.
Invalid queries and source-rejected cursors return
`400 invalid_profile_authoring_query`; unavailable or malformed source data
returns `503 profile_skills_unavailable`.

`POST /api/v1/profile-authoring/repositories/resolve` takes `{ url, reference? }`
for a public GitHub repository and an optional branch or tag. The URL must
contain exactly two nonempty ASCII owner/repository components,
each at most 100 bytes after removing the repository's optional `.git` suffix.
A single trailing slash is accepted. The response is
`{ url, reference?, default_branch, commit_sha }`. The URL is canonical and may
normalize owner/repository case and remove `.git` or trailing slashes.
`reference` is retained exactly when requested and omitted otherwise; the
observed default branch is never substituted into it. `default_branch` and the
lowercase 40-hex `commit_sha` are verified server-side observations. No caller
credential is forwarded to GitHub and redirects are not followed.

Invalid URLs or references return `422 invalid_repository` or
`invalid_repository_reference`; repository absence/inaccessibility returns
`404 repository_not_found`, and a missing branch/tag returns
`404 repository_reference_not_found`. Failed verification returns
`503 repository_unavailable`. Malformed JSON, incompatible body fields,
oversized bodies, and missing/wrong content types use `invalid_json` with
400, 422, 413, and 415 respectively. All four routes return
`401 not_authenticated` or `403 forbidden` before exposing validation or source
results to unauthorized callers.

## Profiles And Marketplace

The three profile APIs share lifecycle shape but have different content schemas.
Create uses `{ name, description?, content }`; read returns identity, owner,
visibility, draft content, `draft_version` and publication information. Update
uses `{ draft_version, content, name?, description? }`. Publication posts
`{ draft_version }` to `/revisions` and returns an immutable published revision.
Visibility uses `{ visibility: "private" | "public" }`. Making a profile private
or retiring it hides discovery without invalidating existing instance snapshots
or exact published references. A retired profile cannot publish new revisions.

A profile reference is `{ "id": "agp_example", "revision": 1 }`, with a
positive published revision of the correct kind. Agent, Workspace and Sandbox
references are not interchangeable. Array order is preserved and the canonical
content digest includes that order. The exact reserved defaults `agp_default`
and `wkp_default` are accepted as Agent and Workspace references respectively;
other reserved-looking strings and cross-kind IDs are not interchangeable.

Newly selected references must resolve to an owned or public, nonretired profile
revision. An exact pin inherited from the server-recorded fork source or the
parent's own prior published revisions remains usable when the child becomes
private or retired. Changing its ID or revision is a new selection and requires
current access. Draft JSON alone never grants reference authority, and inherited
pins do not grant direct access to a private child's profile routes. Publication
rechecks these rules atomically with reading the draft and inserting its revision.

```json
{
  "name": "Repository reviewer",
  "description": "Read and review repository changes",
  "content": {
    "model": null,
    "thinking_effort": "medium",
    "permission_level": "guarded",
    "skills": [],
    "system_prompt": "Review the repository and report actionable findings."
  }
}
```

Workspace content has `location`, `name`, `setup_commands` and
`agent_profiles`. `location` is `{ type: "git_repository", url, reference? }`
or `{ type: "directory", name }`. Repositories must be supported public GitHub
URLs. The effective directory is under `/home/cma`; main checkouts are never
Agent working directories.

Sandbox content has `cpu_millis`, `memory_mib`, `nyxid_services`,
`agent_provider`, `model_access`, `max_workspaces`, `max_agents_per_workspace`,
`workspace_profiles`, `software_install_commands` and `environment_variables`.
Only `agent_provider: "codex"` is supported. Model access is
`chronoai_managed` or `own_credentials`. Environment variables are public
configuration, not secret storage. [CMA Model](CMA_Model.md#profile-contract)
lists complete field bounds, reserved names and trust rules.

Marketplace `kind` is `agent`, `workspace` or `sandbox`, not the plural profile
route component. Listing accepts search `q` and a cursor. Detail exposes usable
published content and transitive trust disclosure. Fork posts `{}` for the
current usable revision or `{ revision: n }` for an explicit revision, returning
a new caller-owned profile with provenance. There is no shared mutable fork.

## Instance Creation And Lifecycle

`GET /api/v1/defaults` returns three exact published references named
`sandbox_profile`, `workspace_profile` and `agent_profile`. Do not guess default
ids, reconstruct references from display names or silently use a later revision.

Create a Sandbox with `{ sandbox_profile: { id, revision }, name? }`.
Create a Workspace under it with `{ workspace_profile: { id, revision } }`.
Both requests accept `trust_confirmed` and `commands_digest` when the selected
foreign executable content requires explicit review. Use the server-provided
digest; trust for one command set does not authorize another.

Create an Agent under a ready Workspace with:

```json
{
  "agent_profile": { "id": "agp_example", "revision": 1 },
  "title": "Repository review",
  "first_message": "Inspect the repository and summarize the changes."
}
```

The response contains the Agent and its parent context. Worktree and Agent
admission reserves one dedicated branch/directory atomically; there is no
independent Worktree-create route. Creation acceptance is not readiness or
first-message completion. Follow the Agent, Sandbox events and recorded
first-message state. Counts are enforced transactionally at admission.

Owned Agent detail optionally includes `first_message.purpose` (`setup` or
`task`) and `first_message.turn_sequence` once dispatch is fenced. Missing
purpose means legacy unknown; an Agent's creation origin does not establish its
message purpose, including when a Trigger reuses an untouched lifecycle Agent.

Owned Agent detail, lists and hierarchy expose optional `runtime_metadata` with
`working_directory`, `description`, `description_source` (`provider_title` or
`prompt_preview`), and `description_is_setup_preview`. Missing source is unknown.
The setup flag is true only with exact provider-input evidence linked to the
recorded automatic setup message. Ignore a preview as a display title only when
its source is `prompt_preview` and this flag is true; then use the configured
Agent title. Explicit native titles remain authoritative, including text equal
to a setup preview. This is title provenance, not a semantic title generator.
`runtime_metadata_changed` replay events carry the same atomic metadata snapshot.
Shared conversation reads retain their existing metadata redaction.


Sandbox pause/revive and removal return recorded lifecycle state. Stopping an
Agent preserves readable history; deleting it records cleanup of its Worktree
and home. Workspace removal and Sandbox retirement are durable operations,
not a direct recursive filesystem API. The [CMA Model](CMA_Model.md#lifecycle-states)
contains the actual transition whitelist.

## Durable Sandbox commands

`POST /api/v1/sandboxes/{id}/commands` accepts an explicit `pause`, `revive`, or
`retire` command and the positive `expected_revision` from a current authorized
Sandbox detail. Supply `Idempotency-Key`. A shared detail now includes `revision`;
older servers may omit it, in which case clients cannot submit this command.

```json
{"command":"pause","expected_revision":7}
```

Every successful submission or replay returns HTTP 202, `Location` pointing to
`GET /api/v1/sandbox-commands/{id}`, and `Cache-Control: private, no-store`.
The receipt contains `command_id`, `sandbox_id`, `command`, `expected_revision`,
`admitted_at`, the original five-minute `queue_deadline`, `revision`, and `state`.
It exposes no owner configuration, credential custody reference or internal audit.
A queued receipt has revision 1 and `state: {"status":"queued"}`. Terminal receipts
have revision 2. Applied state includes `at`, `sandbox_revision`, and `disposition`
(`scheduled`, `already_scheduled`, or `already_satisfied`). Applied means the
durable transition was committed; refresh Sandbox status for runtime progress.
Rejected state includes `at` and one of `authority_revoked`, `queue_expired`,
`target_changed`, `not_eligible`, `connections_required`, or
`credential_setup_required`; the command scheduled no effect.

Pause/revive require current `manage_sandbox`; retirement requires owner/admin
`manage_sharing` and personal owners also need their existing deletion capability.
A share recipient cannot retire the owner's Sandbox. The scheduler rechecks the
actual admitting actor and current resource authority before committing a
transition. Revival checks the resource owner's required connections.

An uncertain response must be recovered with the **same key and request body**,
including its original expected revision. One queued intent per Sandbox is allowed.
HTTP 409 `sandbox_command_busy` and HTTP 503 `sandbox_command_unavailable` include
`Retry-After: 1`; neither indicates completed runtime work. A different request
under an existing key returns `idempotency_key_reused`. A stale new intent returns
`sandbox_command_conflict`. Receipt reads require the original requesting actor,
current target visibility and the same resource incarnation, and return HTTP 200.

The shared Sandbox page offers these controls from current effective access and
rechecks it on each submission. Retirement requires an inline confirmation.
Unconfirmed intents retain their key/body through refresh and shared navigation
within the same mounted viewer console; sign-out or a different viewer starts a
new scope. This recovery state is not persisted across a full browser reload. Queued receipts are polled, and terminal results refresh
actual Sandbox status. Access loss cancels receipt reads and removes the shared
view. Personal Sandbox controls and personal conversation drafts retain their
existing behavior.

## Agent Input And Reads

REST input and steer accept `{ text }`. Input returns `202` with `{ queued }`;
queueing is an admission fact, not proof of runtime delivery. Approval answers
use `{ decision: "allow" | "deny" | "abort" }`. Stop accepts `{}` or
`{ reason: "owner" }`; a caller cannot impersonate an internal failure reason.
Interrupt, approval, stop and deletion return empty 202 bodies where indicated.
Do not parse those bodies as JSON or infer completion from acceptance alone.

Production input, steer, interrupt and approval routes admit work through the
common Agent command ledger using the actual authenticated human/API actor and
current resource permissions. Organization admins may interact where effective
access permits it; view grants and member/viewer roles remain read-only. Input's
new durable acceptance returns `{ "queued": true }`, including receipt recovery
after execution finishes. Controls bind the active turn observed atomically at
admission and cannot drift to a later turn; an idle Agent refuses controls.

These v1 routes preserve their original operation-scoped `Idempotency-Key` and
exact request-byte fingerprint. An existing completed v1 receipt replays its
original status, headers and body. New common commands use a distinct derived
operation key. If a process loses its outer v1 response after common admission,
the retry looks up that exact receipt under fresh current authority without
reserving or dispatching again. A pending/expired older operation with no common
receipt remains fenced with its named idempotency error. Changing its key is not
a recovery operation. Admission uncertainty is never stored as a final failed
receipt that would hide accepted work. This migration does not change v1
Responses/AG-UI parsing. Response-targeted controls use the separate v2 contract
documented in [Responses](API_Responses.md#cma-response-controls). Rich Agent
history and new-tab transport selection use [Agent event replay v2](API_Agent_Events_V2.md).

The personal-owner Agent detail includes its Sandbox, Workspace, Worktree, profile, status
and first-message information, not an aggregate revision field. Transcript returns `{ agent_id,
through_seq, turns }`; usage has its own `through_seq` and cumulative input,
output and context readings. Resource readings are Sandbox-scoped observations
projected alongside Agent identity, not per-Agent billing.

Detail, transcript, event and usage reads accept current organization or share
authority. Other authorized readers receive an explicit shared detail document:
`access: "shared"`, `id`, `status`, optional `title`/`model`, `created_at`,
`updated_at`, and optional `ready_at`/`stopped_at`/`deleted_at`. Parent hierarchy,
paths, profiles, credentials and native conversation identifiers are omitted.
Sandbox metrics require independent Sandbox access. The TypeScript
`agents.readAccessible` and Rust `agents().read_accessible` methods parse both
personal and shared details while existing `read` methods retain their full type.

Shared event pages replace out-of-scope records with
`kind: { type: "access_redacted" }` at the unchanged sequence/timestamp. Count
these records and advance cursors normally. Shared transcripts and events omit
correlation metadata, sign-in secrets and private runtime diagnostics. The
stored log is unchanged and complete-log validation still applies. Buffered
results are refused when access changes or expires during IO. Organization
outages return 503 `organization_authority_unavailable`; inaccessible Agents
retain 404 `agent_not_found`. Shared hierarchy, live and interaction
interfaces remain under development.

`GET /api/v1/organizations` returns `{ items: [{ id, role, display_name? }] }`
ordered by canonical organization ID. Roles are `admin`, `member` or `viewer`.
These choices support scope selection; each resource request obtains new evidence.

`GET /api/v1/agents?organization=<canonical-id>` lists Agents owned by that
organization or covered by its current Agent/whole-Sandbox shares. Omit
`organization` to retain the existing personal inventory without organization
lookups. `status`, `cursor` and `limit` apply after organization filtering and
overlapping grants are deduplicated. Only authorized rows determine `next`;
there is no unfiltered resource count. Each subsequent page rechecks membership
and grants. An inaccessible Sandbox or Workspace filter is refused, including
for a narrow-shared Agent, because a filter result could otherwise reveal its
parent. A buffered page is refused after a policy change, even if only its
hidden lookahead row lost access. Other authorized readers receive the same
safe Agent-only fields as shared detail; personal owners retain their full list
summary. The SDK methods are TypeScript `agents.listAccessible` and Rust
`agents().list_accessible` with `AccessibleAgentListOptions`.

`GET /api/v1/agents/{id}/provenance` returns immutable creation facts:
`agent_id`, `created_at`, and `creation_origin` (`manual`, `trigger`,
`channel_bridge`, or `legacy_unknown`). The owner and a current manager of the
owning organization also receive `details`: optional creating actor, original
Trigger/launch references, channel references, and Workspace profile index.
Shared recipients and independent operators receive only the category and
timestamp. Policy and the original membership deadline are checked after IO.

New manual creation records the authenticated actor separately from ownership.
New authenticated Trigger launches capture the original verified initiating actor
in an immutable Launch sidecar. Trigger workers retain their exact Trigger actor
and launch ID as the creating identity, while Agent provenance separately records
that initiating actor. Launch replay and Agent creation recheck the original
sidecar; request JSON cannot select either actor. Older launches without this
evidence retain an unknown initiator. Legacy inference
requires the matching initial creation log and canonical creation key and never
invents a creator from the owner. Workspace profile origins are retained with
unknown creation category when no supported category can be proved. Retries,
profile changes, sharing and runtime recovery preserve creation facts. This read
does not write historical records. A separate online worker starts 30 seconds
after startup, scans at most 25 Agent IDs per page and spaces individual captures
by 250 milliseconds. Each store call has a 15-second deadline. It repeats complete
sweeps after five minutes, retries refused rows on a later sweep and stops with
service shutdown. New facts are captured transactionally; existing provenance is
never rewritten. Missing or inconsistent ancestry refuses capture, and missing
creation evidence produces `legacy_unknown` without inventing a creator. Mongo
uses metadata lineage guards while preserving serialized resource records,
revisions, conversations, runtime attachments and history. Staged channel
allocation records the actual channel-created Agent category and original source.
Explicit channel binding and accepted-command history is available through the
separate channel-usage routes below.
The web provides authorized organization hierarchy navigation and owner share management.

`GET /api/v1/agents/provenance` lists safe creation summaries in stable Agent-ID
order. Omit `organization` and `shared` for personal ownership; explicit organization scope
requires current membership and selects organization-owned or shared Agents.
Rows contain only `agent_id`, optional `title`, `status`, `created_at`,
`updated_at`, and `creation_origin`. Private source/creator details and hierarchy
IDs are omitted even for managers. The inventory includes retained Agents in
all lifecycle states, including deleted Agents, unless `status` is supplied.
Additional filters are `sandbox`, `workspace`, and `creation_origin`; parent
filters require independent visibility of that parent. A narrow Agent grant
cannot reveal the result of a parent filter.

`shared=true` selects **Shared with me**, the union of explicit Agent and whole-Sandbox
grants to current organizations, excluding personally owned resources before paging.
Overlapping organizations and grants produce one row per Agent. Organization ownership
alone does not add a resource to this aggregate; use Organization scope for those
resources. `shared=true` and `organization` are mutually exclusive. Each request
rediscovers organizations and obtains fresh evidence for at most 128 choices, with
at most eight provider lookups concurrently. A confirmed nonmembership removes a
stale choice; an outage refuses the aggregate rather than returning an incomplete
successful page. Original membership deadlines remain binding through final checks.

`limit` defaults to 50 and accepts 1–200; `cursor` resumes after a consumed Agent
ID. Without a category filter the server reads at most `limit + 1` candidates.
With `creation_origin` it examines at most 201 candidates per request, so a
successful page can be partial or empty and still have `next`. **Continue until
`next` is absent, even when `items` is empty.** This cursor may identify a consumed
authorized Agent excluded by the category filter. All candidates, filtered rows
and lookahead are authorized and rechecked before release; policy changes or
expired authority refuse the whole page. Each request has a 15-second deadline.
Listing reads conservative legacy evidence without mutating retained records.
The existing `/agents` response and cursor behavior remain compatible.

The web Agent header and details panel use this immutable category for creation
labels. A later published Trigger shortcut is displayed separately and cannot
relabel a manual or Workspace Profile Agent. Missing or refused provenance stays
unavailable; it is never inferred from a shortcut. The Sandbox tree shows a
category per Agent, derives parent summaries from all loaded child facts, and
offers a creation-origin filter over Agents in that tree. It reads at most four
optional provenance projections concurrently and retains only categories for
tree rows. Facts refresh once per minute and on foreground, and are discarded
on changed viewer/client context, disabled views, or read refusal. These tree
filters apply to loaded tree rows. The dedicated REST inventory also covers
retained archived Agents.

The web inventory is available at `/agents` and
`/organizations/{organization}/agents`, and through **Agents** in the account
menu. It offers named Personal/Organization scopes, immutable creation and status
filters, previous/next paging and **Export this page**. Empty filtered pages retain
the next-page action. Exports perform a fresh authorized page read and include
only the safe creation-summary fields. Changing scope cancels an outstanding
export; downloaded files remain outside later revocation.

Agent deep links classify the public accessible response before choosing a
controller. Shared/organization Agent views read only the Agent's authorized
history and effective capabilities. They do not fetch parent resources, profile
drafts or runtime readings. Viewers see history and pending approvals without
interaction buttons; current interaction/approval capabilities enable the
composer, response interruption and approval answers. These use the public SDK
and recheck effective access before the server independently authorizes each
command. An access refusal clears the shared view and cancels its stream without
logging out the user's unrelated personal work. Personal controllers and drafts
survive a foreground classification refresh; an actual refusal removes them.

Scoped inventory/history values refresh every 30 seconds, expire no later than
55 seconds from the read start (or the earlier server authority deadline), and
clear on foreground, viewer/client/scope change or refusal. A 15-second timeout
also clears an unverified result. Approved Agent artifacts use the separately authorized routes below.

The sidebar offers Personal and named Organization navigation. Organization
Sandboxes use the SDK's authorized paginated inventory, with fresh previous/next
reads. Expanding a Sandbox loads authorized Workspace summaries; expanding a
Workspace loads its safe Agent/Worktree projection. Narrow Agent grants remain
discoverable through **All authorized Agents**, without requesting their hidden
parents. Changing sidebar scope preserves the active personal Agent controller
and its unsent draft. Organization selection survives sidebar collapse.

Canonical `/sandboxes/{id}` links classify current accessible metadata before
mounting the personal Sandbox page or a safe shared view. `/workspaces/{id}`
provides authorized Workspace names, counts and Agent/Worktree summaries. Shared
hierarchy views read no profiles, provider connections, runtime observations,
paths or secret contents. Each buffered detail checks original policy and
authority expiry after IO, validates parent and Agent/Worktree assignment, and
clears on refusal or scope/client change. Child access refusal invalidates its
parent's displayed branch. Personal Sandbox controls remain mounted through a
classification refresh; refusal discards remembered personal classification.

Effective access includes a safe `owner` summary: a personal owner reports only
`is_current_person`, while an organization reports `organization_id` for matching
an authorized organization choice. Personal IDs and parent identities remain
private. Inventory, tree and shared detail views display this ownership with the
current resource access role derived from the returned capabilities. An organization
admin holding a view-only share is shown as **View only**. Organization names come
only from current authorized choices; unavailable names never fall back to raw IDs.
Visible-row policy reads are limited to four concurrent requests and expire with
the earliest returned authority deadline. These labels do not authorize actions.

Owners can open **Sharing** from personal Agent Details or a Sandbox page when
current effective access includes `manage_sharing`. Agent sharing starts with
the narrow Agent scope; selecting **Whole Sandbox** is explicit and requires
acknowledging current and future children before creating a grant. Named current
organization choices, view/org-role exposure previews, direct grant mode changes
and revocation use the public SDK. The panel explains inherited access and never
automatically broadens a refused interactive Agent grant. Current stores require
a compatible whole-Sandbox grant while independent isolation remains unproved.
Recipients cannot reshare; ownership, origin and conversations are unchanged.

Grant lists are checked against their resource identity, initial policy and
authority deadline after buffering. Each change rechecks effective permission,
uses an idempotency key and validates the receipt's requested effect. Edits and
revocation send the displayed revision; a conflict reloads current grants and
requires another explicit action. Unconfirmed attempts retain their logical
command key within the current resource view. Scope/client changes and access
loss cancel pending reads/preflights; late replies cannot send a new command.
Organization-choice failure leaves independently authorized revocation available.
Backend share routes enforce the final owner, membership and isolation checks.

`GET /api/v1/agents/{id}/source` returns authorized provisioning origin and shortcuts.
Personal owners retain the existing source document:
`manual`, `workspace_profile`, or `trigger`. Trigger-created Agents also include
the published Trigger title, public id, revision and quick messages when the
source remains resolvable. Quick messages follow the latest published revision;
draft edits stay private and retired Triggers return no shortcut actions.
Reading source never sends a message or replays the first prompt. Each optional
Trigger content `quick_messages` entry has a required unique name (120 UTF-8
bytes), description (2,000 bytes) and text (32,768 bytes), with at most eight
entries. Missing or empty arrays preserve legacy content digests.

Other authorized readers receive `{ access: "shared", agent_id, origin: { type } }`
with the same three creation categories. Private Trigger/Launch IDs, profile
indexes and published shortcuts are withheld. Shared reads do not consult the
owner's Launch or Trigger dependencies. Success and dependency errors are fenced
against current Agent policy and evidence lifetime after IO. TypeScript
`agents.sourceAccessible` and Rust `agents().source_accessible` return explicit
personal/shared unions. Existing `source` methods retain their personal types
and reject a shared projection.

`GET /api/v1/agents/{id}/settings` returns the owner's current `model`,
`reasoning_effort`, `permission_mode`, and `pending`. Historical profiles that
leave model selection to the runtime return an empty model until explicitly set.
`PATCH` on the same path requires an `Idempotency-Key` and all three settings;
reasoning effort is `minimal|low|medium|high|x_high`, and permission mode is
`guarded|autonomous`. Updates require an idle Ready Agent and preserve its
immutable profile and conversation identity. Desired settings are persisted
before runtime dispatch and reapplied after reconnect; `pending: true` means
runtime confirmation is outstanding and further input remains fenced. Retry
network failures or ambiguous 503 replies with the original body and key.

`GET /api/v1/agents/{id}/models` returns `{ models: [{ id, label, available }] }`
from the Chrono LLM catalog using the owned Sandbox's existing scoped credential.
It includes eligible text models and never expands service access. `available`
requires the exact current runtime to advertise catalog-model support and the
Agent and Sandbox to be in supported active states. Retained runtimes without
that capability keep their existing settings and return disabled choices.
Catalog failures return a sanitized `503`; responses use `Cache-Control: no-store`.
See [Managed models](CMA_Managed_Models.md) for runtime selection validation.

`GET /api/v1/agents/{id}/skills` returns `{ items, pending, can_install,
unavailable_reason? }`. Each item includes its exact `id`, `version`, `source`,
and `status` (`pending`, `installed`, or `failed`), with optional `name` and
`error`. `POST` to the same path requires an `Idempotency-Key` and
`{ "id": "<Ornn skill ID>", "version": "<exact version>" }`. It admits an
additive installation only for an owned, idle Ready Agent on a supported
runtime and returns `202 AgentSkillsView`. Poll the read route for runtime
confirmation. Replaying the same request and key does not install twice;
failed installations remain recorded and cannot be retried as a new install.
Available choices come from the owner's existing profile-authoring skills
catalog. See [Agent skills](CMA_Agent_Skills_Runtime.md) for installation and
recovery semantics.

`POST /api/v1/agents/{id}/attachments?name=<filename>` uploads raw file bytes
into the owned Agent's persistent environment. It requires an `Idempotency-Key`;
`Content-Type` supplies the file's media type and defaults to
`application/octet-stream`. Send one file per request, at most 20 MiB, without
`Content-Encoding`. The API validates the actual received length and sanitizes
the name to a single non-hidden filename. The response is
`201 AgentAttachmentView` with `id`, `agent_id`, `name`, `path`, `size_bytes`,
`media_type` and `sha256`. Uploading does not submit a conversation message;
the client may reference the returned path in a later explicit message.

Uploads require current owner authority or an accepted, active typed share link
with `view_write` permission, an active Sandbox and a retained runtime that
advertises persistent attachment support. Legacy sharing and delegated
conversation rights alone do not authorize file writes. Agent ancestry, runtime and
home identity are rechecked across the upload. Invalid metadata or incomplete
bytes return `invalid_attachment` (400); oversized files return
`attachment_too_large` (413). Unavailable environments, unsupported retained
runtimes and changed upload identity return a typed 409. Upload capacity returns
`attachment_capacity` (429), a body that does not complete within 45 seconds
returns `attachment_timeout` (408), and transient runtime failures return
`attachment_unavailable` (503). Retry uncertain uploads with the original key,
metadata and bytes. Responses use `Cache-Control: private, no-store` and
`X-Content-Type-Options: nosniff`.

`GET /api/v1/agents/{id}/access-requests` returns `{ items }` for the Agent owner.
Each item retains its exact request and Agent IDs, `services`, `reason`, durable
`status`, and optional safe `error`. Owner approval uses
`POST /api/v1/agents/{id}/access-requests/{request_id}/approve` with
`{ "services": ["service-id"] }`; denial uses the sibling `/deny` with `{}`.
Both commands require an `Idempotency-Key` and return the current request with
HTTP 200. Service selections contain 1–32 identifiers, each 1–128 ASCII letters,
digits, hyphens or underscores. Bodies are limited to 8 KiB.

Approval requires a verified caller context matching the authenticated owner
and an outbound request credential; missing credentials are refused before
any approval effect. The Core service owns durable decision replay and current
owner checks. Reuse the exact decision body and key after an uncertain reply.
Both `connecting` and `applying` responses include `connection.target` with
`{ "type": "agent_access_request", "id": "request-id" }` for the owner's
connection flow. Connections can be rechecked and restored throughout application
without changing the approved services or admitting another approval.
`applying` awaits runtime verification, and only `succeeded` confirms applied
access. Poll the list after a connection return. `pending`,
`denied` and `failed` preserve their durable meanings. All responses are private
and non-cacheable. Typed refusals use `agent_access_invalid` (400),
`agent_access_forbidden` (403), `agent_access_not_found` (404),
`agent_access_conflict` (409), and `agent_access_unavailable` (503).

`PUT /api/v1/agents/{id}/service-access` accepts
`{ "services": ["chrono-llm-public", "ornn-api"], "expected_services": ["chrono-llm-public", "ornn-api", "api-github"] }`
with an `Idempotency-Key` and returns HTTP 202 `AgentAccessRequestView`.
The owner Save is consent to replace the complete service set shared by all
Agents in that Sandbox. Optional services may be removed; the current platform
baseline is mandatory. Both sets are bounded to 1–32 distinct service identifiers.
`expected_services` must equal the currently effective set. A stale set or reuse
of the same key with a different selection returns 409. Exact retries recover the
original durable request even after its scope change has applied. A new owner
Save starts `connecting`, without a separate pending approval.

Before final Save or Approve, the browser connection flow can check
`{ "type": "agent_service_access", "id": "agent-id", "services": ["selected-service-id"] }`.
This owner-bound target resolves the selected scope and any model-access
prerequisites without granting access or recording consent. Complete required
connections before submitting final consent; the worker revalidates connections
and retained owner authority before changing scope.

New scope updates rotate the managed key in the existing live runtime. They do
not pause or restart the Sandbox, replace its Pod, or replace Agent conversations.
A paused Sandbox waits for an explicit owner resume. The scope revision,
runtime identity and credential generation floor are durable; a concurrent
lifecycle or later scope change cannot retarget an old update. Runtime delivery
and old-key revocation must both complete before the callback. A busy Agent
receives durable steering in its original active turn; a quiet Agent receives a
normal message in its original conversation. `succeeded` requires confirmed
callback delivery, independent of the time needed to finish the model response.
Historical already-paused workflows retain their original recovery path.

`GET /api/v1/agents/{id}/events` is a **JSON page**, not SSE. It accepts `after`
and `limit` (1-200, default 200), returns `items` and optional `next`, and omits
internal dispatch keys and runtime approval locators. Read all pages, require
advancing cursors and combine transcript fallback without duplicating messages.

Message feedback is private, durable metadata for the Agent owner.
`GET /api/v1/agents/{id}/message-feedback` pages non-null votes using `after`
and `limit` (1–200, default 200). `PUT
/api/v1/agents/{id}/messages/{message_id}/feedback` sets or clears a vote with
`{"vote":"up"}`, `{"vote":"down"}`, or `{"vote":null}`. The message ID must be
`event:<sequence>` for a committed owner or assistant message. This idempotent
metadata update requires no `Idempotency-Key` and never starts or interrupts
Agent work. See [Agent Manager feedback](API_Agent_Manager.md#message-feedback)
for ownership, pagination, persistence and concurrent-update behavior.

Each transcript turn includes ordered `messages` with the durable `sequence`,
`role` (`user` or `assistant`), `content` and `appended_at`. Steering remains a
distinct owner message inside the active turn, even when assistant messages
precede it. The `owner_message` summary holds only the initial input and
`agent_messages` only assistant text; use `messages` to reconstruct chronology.
Merge transcript and event replay by durable message sequence, not by turn id.
Sandbox events are separately scoped JSON history. Launch event pages have
`launch_id`, `events` and optional `next_cursor`; their cursors must not be used for Agent history.
For live Agent and Launch streams see [AG-UI](API_AGUI.md).

## Triggers, Launches And Connections

[CMA Trigger](CMA_Trigger.md) gives complete publisher content and fixture-derived
request examples, source placement rules, the a/b/c reuse mapping, consent,
requirements and all five snippet forms. Do not infer the Launch body from a
preview or reuse an authoring DTO for visitor admission.

The anonymous preview is a safe projection, not the publisher-owned draft.
Create Launch is authenticated and takes a stable idempotency key. It can
return a reviewed Draft (201) or admitted work (202) when `start_if_ready` and
current requirements allow it. A retained Draft continues through
`POST /api/v1/launches/{id}/start`, which rechecks requirements and explicit
trust/repeat consent. Cancellation is separate. There is no public worker retry.

The requirements snapshot includes `services`. Each entry keeps its service id
and state and may additionally expose current `display_name`,
`connection_mode: "hosted" | "manual" | "unknown"` and `setup_url`. Missing
metadata remains unknown; a slug is not evidence that SSH/manual setup is
needed. Historical Launch snapshots are not rewritten by current metadata.

Connection assist creates a short-lived owner-bound handoff for one service
and reads its status. It never puts a service credential in the response.
Only an active service recheck can establish connection; popup close, OAuth
return and a persisted handoff id cannot. [Services](Services.md) documents
hosted/manual/unsupported/unavailable outcomes and recovery.

The embed descriptor is owner-only and requires `placement_id`. It provides
server-derived URLs, snippets, SRI and CSP directives. Public badges are
anonymous SVG with closed label/theme query values, optional placement
attribution, ETag and conditional 304 handling. Never supply arbitrary SVG,
HTML, style, redirect origins or credential-bearing query parameters.

## Administration

Administration exposes settings, Sandbox/profile/runtime inventories, activity,
retirement and Trigger pause/resume under specific operator capabilities.
Owner views do not gain these operations from a route name or role slug.
Settings replacement uses `{ expected_revision, settings }`; patch uses
`expected_revision` plus complete changed sections, not arbitrary nested
JSON Patch operations. Reset uses `{ expected_revision }`. Read the resulting
revision after every successful change. Runtime inventory can fail unavailable
without proving that a runtime is absent.

Native warm-pool administration requires `ConfigurePlatform`:

- `GET /api/v1/admin/warm-pools` returns durable targets, current provider
  observations and the deployment's default image/CPU/memory. An unavailable
  provider retains its target records with `available: false`; unobserved
  readiness is `null`.
- `POST /api/v1/admin/warm-pools` requires `Idempotency-Key` and accepts
  `{ cpu_millis, memory_mib, target_replicas }`. The server supplies its pinned
  system runtime image, initialized home seed and Pod policy. The `202` response
  acknowledges a durable target, not ready capacity. Retry an uncertain create
  with its exact original body and key.
- `PUT /api/v1/admin/warm-pools/{id}` accepts
  `{ expected_revision, target_replicas }` and returns `202`. Set the target to
  zero to stop spare-capacity replenishment. Read current state after an
  uncertain write or conflict before submitting a reviewed update.

The reconciler saves the original pool/template UIDs before expanding capacity.
Subsequent updates preserve those identities and their Pod policy; a missing or
replaced resource causes reconciliation to report unavailable and retry, without
creating replacement capacity. Target replicas are bounded to 0–1000. Active
claimed Sandboxes continue their own retained-home lifecycle independently of
spare-capacity changes.

See [Operations](CMA_Operations.md) for settings meaning, deployment prerequisites,
alerts, recovery, data-reset boundaries and production evidence. There is no
arbitrary shell, database deletion or NyxID administration endpoint here.

## Operation Inventory

Resource share management uses `kind=agent|sandbox`. The owner can create a
`view` or `org_roles` grant for a canonical current organization choice, list
active direct grants, read a revoked grant, change its mode, or revoke it.
Create bodies contain `target_org_id` and `mode`; update bodies contain `mode`
and `expected_revision`; revoke requires the `expected_revision` query.
Every mutation requires an explicit `Idempotency-Key`. Equivalent retries return
the original receipt even after later revocation; changed input with that key
returns 409. Owner/actor/evidence fields in request bodies are rejected.

Effective-access kinds additionally include `workspace` and `worktree`. Its
capabilities are a current policy projection, with a bounded revalidation time;
they are not credentials for later commands. Shared Agent REST history and
AG-UI replay/follow enforce current policy. File, attachment and interaction
boundaries are still being integrated. See
[Resource authorization](CMA_Resource_Authorization.md) for the current limits.

The following rows are derived from the actual OpenAPI export and checked for
complete method/path coverage. Schema names refer to
`/openapi.json#/components/schemas/<name>`. `key` means the required
`Idempotency-Key` header. Path variables are required; listed query fields are
optional unless marked `*`. Success schemas describe the projected response,
not a guarantee that asynchronous work has finished. The shared error contract
above applies even when an operation annotates only its principal responses.

<!-- openapi-operations:start -->

### Event Worker Profiles

These owner-scoped resources compose three published CMA profile revisions into
one full, immutable gateway configuration. All operations require
`manage_own_profiles`. Creation uses `Idempotency-Key`; updates and retirement
compare `expected_revision`. Repeating an identical successful update recovers
that revision; conflicting edits return 409. Retirement preserves revisions and
existing Bot snapshots. The `profile` field contains the complete
`EventWorkerProfile`; `source` records the three CMA references and authoring text.

| Operation | Behavior |
|---|---|
| `GET /api/v1/event-worker-profiles` | List active owned profiles using `after` and `limit` (1–200). |
| `POST /api/v1/event-worker-profiles` | Create and pin the complete configuration; return 201. |
| `GET /api/v1/event-worker-profiles/{id}` | Read the current owned revision, including retirement. |
| `PUT /api/v1/event-worker-profiles/{id}` | Save a new revision with `expected_revision` and `source`. |
| `DELETE /api/v1/event-worker-profiles/{id}` | Retire with query `expected_revision`, preserving history. |
| `GET /api/v1/event-worker-profiles/{id}/revisions/{revision}` | Read an immutable owned historical revision. |


### Bots

Creation optionally accepts `sandbox_profile:{id,revision}` and
`workspace_profile:{id,revision}` alongside the required Agent `profile`.
Explicit environment profiles must be personally owned and published; an omitted
reference pins the corresponding platform default. Bot responses expose all three
pinned references. Alternatively, supply `event_worker_profile:{id,revision}`
and the three matching source references to pin a saved full worker configuration.
The Bot retains that snapshot even if the worker is later edited or retired.
Reusing a creation key with different environment references
returns 409, including changing an omitted default into an explicit reference.

| Operation | Behavior |
|---|---|
| `POST /api/v1/bots` | Create a durable owner Bot with an Idempotency-Key; 201 returns creation progress. |
| `GET /api/v1/bots` | List owned Bots with exclusive `after` and `limit` (1–200). |
| `GET /api/v1/bots/connections` | List the signed-in owner’s ready Gateway connections as `{connection_ref,label}`. |
| `GET /api/v1/bots/nyxid-bots` | Compatibility inventory projection; new clients use `/connections`. |
| `GET /api/v1/bots/{bot_id}` | Read owned Bot state, operation progress, channel endpoints and last synthetic test. |
| `POST /api/v1/bots/{bot_id}/test` | Run a synthetic provider turn; does not send an external chat message. |
| `POST /api/v1/bots/{bot_id}/disable` | 202 admits durable denial of all source events. |
| `POST /api/v1/bots/{bot_id}/enable` | 202 admits restoration of the original source admission policy. |
| `POST /api/v1/bots/{bot_id}/rotate-keys` | 202 admits rotation of the consumer Agent credential (and the route credential for legacy consumers), with recovery and cleanup. |
| `DELETE /api/v1/bots/{bot_id}` | 202 admits deletion; GET returns 404 after provider, channel, routes and keys are removed. |

### Resource access and share management

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/organizations` | - | - | 200 OrganizationChoicesView; 401/403/503 ProblemDocument |
| `GET /api/v1/resource-access/{kind}/{id}` | - | - | 200 EffectiveAccessView |
| `GET /api/v1/resource-shares/{kind}/{id}` | - | - | 200 ActiveShareList |
| `POST /api/v1/resource-shares/{kind}/{id}` | CreateShareBody; key | - | 200 ShareReceiptView |
| `GET /api/v1/resource-shares/{kind}/{id}/{share_id}` | - | - | 200 ResourceShareView |
| `PATCH /api/v1/resource-shares/{kind}/{id}/{share_id}` | UpdateShareBody; key | - | 200 ShareReceiptView |
| `DELETE /api/v1/resource-shares/{kind}/{id}/{share_id}` | -; key | expected_revision* | 200 ShareReceiptView |
| `PUT /api/v1/cmaeg/bindings/{binding_id}` | PutBindingRequest | - | 200 BindingView; 400/401/403/409/503 ProblemDocument |
| `GET /api/v1/cmaeg/bindings/{binding_id}` | - | - | 200 BindingView; 401/403/404/409/503 ProblemDocument |
| `DELETE /api/v1/cmaeg/bindings/{binding_id}` | - | - | 204 empty; 401/403/409/503 ProblemDocument |
| `PUT /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}` | optional empty object | - | 200/201 BindingConversationView; 400/401/403/404/409/503 ProblemDocument |
| `DELETE /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}` | - | - | 204 empty; 400/401/403/404/409/503 ProblemDocument |
| `GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}` | - | - | 200 EventContext; 401/403/404/503 Responses error |
| `GET /api/v1/gateway/channel-records` | - | owner, after, limit | 200 GatewayRecordPageView |
| `PUT /api/v1/gateway/channel-records/{record_id}` | PutGatewayRecordRequest | - | 200 PutGatewayRecordView |
| `GET /api/v1/gateway/channel-records/{record_id}` | - | - | 200 GatewayRecordView; 404 ProblemDocument |
| `DELETE /api/v1/gateway/channel-records/{record_id}` | - | expected_version | 204 empty; 404/409 ProblemDocument |
| `GET /api/v1/gateway/connection-records` | - | owner, after, limit | 200 GatewayRecordPageView |
| `PUT /api/v1/gateway/connection-records/{record_id}` | PutGatewayRecordRequest | - | 200 PutGatewayRecordView |
| `GET /api/v1/gateway/connection-records/{record_id}` | - | - | 200 GatewayRecordView; 404 ProblemDocument |
| `DELETE /api/v1/gateway/connection-records/{record_id}` | - | expected_version | 204 empty; 404/409 ProblemDocument |
| `PUT /api/v1/gateway/interaction-records/{record_id}` | PutInteraction | - | 200 PutGatewayRecordView; 400/403/409/413/503 ProblemDocument |
| `GET /api/v1/gateway/interaction-records/{record_id}` | - | - | 200 GatewayRecordView; 404 empty; 400/403/503 ProblemDocument |
| `DELETE /api/v1/gateway/interaction-records/{record_id}` | - | expected_version | 204 empty; 404 empty; 400/403/409/503 ProblemDocument |
| `POST /api/v1/agent-delegations/identity` | AgentDelegationIdentityRequest | - | 200 identity or login URL; 400/401/403/404/409/503 problem |
| `POST /api/v1/agent-delegations/operations` | AgentDelegationOperationRequest | - | 200 result envelope; 400/401/403/404/409/503 problem |
| `POST /api/v1/bot-setups/actions` | AgentSetupAction | - | 200 SetupView; 400/403/404/409/503 ProblemDocument |
| `POST /api/v1/bot-setups/advance` | AdvanceSetup (compatibility) | - | 200 SetupView; 400/403/404/409/503 ProblemDocument |

### Administration

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/admin/activity` | - | actor, action, limit, cursor | 200 ActivityPageView |
| `GET /api/v1/admin/profiles` | - | kind, visibility, owner, cursor, limit | 200 ProfilePageView |
| `POST /api/v1/admin/profiles/{kind}/{id}/retire` | - | - | 200 ProfileSummaryView |
| `GET /api/v1/admin/runtime-inventory` | - | - | 200 RuntimeInventoryView |
| `GET /api/v1/admin/sandboxes` | - | owner, status, runtime_version, cursor, limit | 200 SandboxPageView |
| `GET /api/v1/admin/sandboxes/{id}` | - | - | 200 SandboxDetailView |
| `GET /api/v1/admin/sandboxes/{id}/events` | - | after, limit | 200 array<object> |
| `POST /api/v1/admin/sandboxes/{id}/retire` | - | - | 200 object |
| `POST /api/v1/admin/sandboxes/{id}/runtime-update` | UpdateSandboxRuntimeRequest | - | 202 SandboxView |
| `GET /api/v1/admin/settings` | - | - | 200 SettingsView |
| `PUT /api/v1/admin/settings` | ReplaceRequest | - | 200 SettingsView |
| `PATCH /api/v1/admin/settings` | PatchRequest | - | 200 SettingsView |
| `POST /api/v1/admin/settings/reset` | ResetRequest | - | 200 SettingsView |
| `POST /api/v1/admin/triggers/pause` | - | - | 200 SettingsView |
| `POST /api/v1/admin/triggers/resume` | - | - | 200 SettingsView |
| `GET /api/v1/admin/warm-pools` | - | - | 200 WarmPoolList |
| `POST /api/v1/admin/warm-pools` | ConfigureWarmPoolBody; key | - | 202 WarmPoolView |
| `PUT /api/v1/admin/warm-pools/{id}` | ResizeWarmPoolBody | - | 202 WarmPoolView |

Warm pool views include `config_sha256`, the immutable image/resources/home/Pod
policy digest. Inventory includes `default_config_sha256` for the current default
configuration (`null` when planning is unavailable). Select the deployment default
by exact digest equality: old and new policies can share image and CPU/memory.
A new policy requires a new creation key; replay and resize retain the original
policy. New native pools use `fsGroupChangePolicy: OnRootMismatch`; historical
pools and retained homes keep their admitted templates.

### Agent profiles

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agent-profiles` | - | cursor, limit | 200 AgentProfilePageView |
| `POST /api/v1/agent-profiles` | CreateAgentProfileRequest; key | - | 201 AgentProfileView |
| `GET /api/v1/agent-profiles/{id}` | - | - | 200 AgentProfileView |
| `PUT /api/v1/agent-profiles/{id}` | UpdateAgentProfileRequest; key | - | 200 AgentProfileView |
| `POST /api/v1/agent-profiles/{id}/retire` | -; key | - | 200 AgentProfileView |
| `GET /api/v1/agent-profiles/{id}/revisions` | - | cursor, limit | 200 AgentProfileRevisionPageView |
| `POST /api/v1/agent-profiles/{id}/revisions` | PublishProfileRequest; key | - | 201 AgentProfileRevisionView |
| `GET /api/v1/agent-profiles/{id}/revisions/{revision}` | - | - | 200 AgentProfileRevisionView |
| `PUT /api/v1/agent-profiles/{id}/visibility` | SetProfileVisibilityRequest; key | - | 200 AgentProfileView |

### Agents

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `POST /api/v1/agents` | AgentLifecycleCreateRequest; key | - | 202 AgentLifecycleCreateReceipt |
| `GET /api/v1/agent-lifecycle/{id}` | - | - | 200 AgentLifecycleOperationView |
| `GET /api/v1/agents` | - | organization, sandbox, workspace, status, cursor, limit | 200 AccessibleAgentPageView |
| `GET /api/v1/agents/{id}` | - | - | 200 AccessibleAgentView |
| `DELETE /api/v1/agents/{id}` | -; key | - | 202 empty |
| `GET /api/v1/agents/{id}/events` | - | after, limit | 200 AgentEventPageView |
| `GET /api/v1/agents/{id}/message-feedback` | - | after, limit | 200 MessageFeedbackPageView |
| `PUT /api/v1/agents/{id}/messages/{message_id}/feedback` | SetMessageFeedbackRequest | - | 200 MessageFeedbackView |
| `GET /api/v1/agents/provenance` | `shared`, `organization`, `sandbox`, `workspace`, `status`, `creation_origin`, `cursor`, `limit` | - | 200 AgentProvenanceInventoryPageView |
| `GET /api/v1/agents/{id}/provenance` | - | - | 200 AgentProvenanceView |
| `PUT /api/v1/agents/{id}/service-access` | ReplaceAgentServiceAccess; key | - | 202 AgentAccessRequestView |
| `POST /api/v1/agents/{id}/attachments` | Raw binary bytes; key | name* | 201 AgentAttachmentView |
| `GET /api/v1/agents/{id}/access-requests` | - | - | 200 AgentAccessRequestList |
| `POST /api/v1/agents/{id}/access-requests/{request_id}/approve` | ApproveAgentAccessRequest; key | - | 200 AgentAccessRequestView |
| `POST /api/v1/agents/{id}/access-requests/{request_id}/deny` | DenyAgentAccessRequest; key | - | 200 AgentAccessRequestView |
| `GET /api/v1/agents/{id}/settings` | - | - | 200 AgentSettingsView |
| `PATCH /api/v1/agents/{id}/settings` | UpdateAgentSettingsRequest; key | - | 200 AgentSettingsView |
| `GET /api/v1/agents/{id}/models` | - | - | 200 AgentModelsView |
| `GET /api/v1/agents/{id}/skills` | - | - | 200 AgentSkillsView |
| `POST /api/v1/agents/{id}/skills` | InstallAgentSkillRequest; key | - | 202 AgentSkillsView |
| `GET /api/v1/agents/{id}/source` | - | - | 200 AccessibleAgentSourceView |
| `POST /api/v1/agents/{id}/stop` | StopAgentRequest; key | - | 202 empty |
| `GET /api/v1/agents/{id}/terminal` | - | - | 404 ProblemDocument; 501 ProblemDocument |
| `GET /api/v1/agents/{id}/transcript` | - | - | 200 AgentTranscriptView |
| `POST /api/v1/workspaces/{id}/agents` | CreateAgentRequest; key | - | 201 AgentView |

### Agent home files

Paths are relative to the runtime's persistent `/home/` directory. Reading
requires current file-view authority; creating, editing and deleting require
owner authority or an accepted, active typed `view_write` sharing link. Each
operation rechecks access, and protected credential paths remain unavailable.
Reads, downloads and writes are bounded to 32 MiB. A write's `expected_sha256`
protects an edit against intervening changes; omit it when creating a new file.
Directory creation uses `directory: true`. File operations do not require an
`Idempotency-Key`.

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agents/{id}/files/tree` | - | path | 200 FileListView |
| `GET /api/v1/agents/{id}/files/content` | - | path*, max_bytes | 200 FileReadView |
| `GET /api/v1/agents/{id}/files/download` | - | path* | 200 application/octet-stream |
| `POST /api/v1/agents/{id}/files` | WriteFileRequest | - | 200 FileMutationView |
| `DELETE /api/v1/agents/{id}/files` | - | path*, recursive | 200 FileMutationView |

### Agent sharing links

Only the Agent owner manages links and searches the recipient directory.
Redemption requires an authenticated recipient, the link token, and membership
in the configured audience. Public links still require authentication. Link
creation accepts a 32-byte cryptographically random token encoded as 64
lowercase hexadecimal characters; only its SHA-256 digest is retained. Keep the
token in the browser URL fragment and send it in the redemption JSON body.
Revocation is independent for each link and uses `expected_revision`. Links
grant Agent access without granting Sandbox inventory or lifecycle rights.
See [Agent sharing](Agent_Sharing.md) for audience and permission semantics.

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agents/{id}/share-links` | - | - | 200 ShareLinkList |
| `POST /api/v1/agents/{id}/share-links` | CreateLinkBody; key | - | 200 ShareLinkView |
| `DELETE /api/v1/agents/{id}/share-links/{share_id}` | -; key | expected_revision* | 200 ShareLinkView |
| `GET /api/v1/agents/{id}/share-recipients` | - | q* | 200 RecipientList |
| `POST /api/v1/agent-share-links/{share_id}/redeem` | RedeemLinkBody | - | 200 ShareLinkView |

### Agent operations

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `POST /api/v1/agents/{id}/approvals/{request_id}` | ApprovalDecisionRequest; key | - | 202 empty |
| `POST /api/v1/agents/{id}/input` | AgentTextRequest; key | - | 202 InputAcceptedView |
| `POST /api/v1/agents/{id}/interrupt` | -; key | - | 202 empty |
| `POST /api/v1/agents/{id}/steer` | AgentTextRequest; key | - | 202 empty |

### Agent usage

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agents/{id}/resources` | - | - | 200 AgentResourcesView |
| `GET /api/v1/agents/{id}/usage` | - | - | 200 AgentUsageView |

### Ag-ui

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agui/agents/{id}/sandbox` | - | - | 200 JSON |
| `GET /api/v1/agui/capabilities` | - | - | 200 JSON |
| `GET /api/v1/agui/events` | - | thread_id*, starting_after, after, follow | 200 SSE |
| `POST /api/v1/agui/run` | RunRequest | - | 200 SSE |

### Trigger launches

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agui/launches/{id}/events` | - | starting_after, after, limit, follow | 200 SSE |
| `GET /api/v1/launches` | - | trigger, state, cursor, limit | 200 LaunchPageView |
| `GET /api/v1/launches/{id}` | - | - | 200 LaunchView |
| `POST /api/v1/launches/{id}/cancel` | -; key | - | 200 LaunchView |
| `GET /api/v1/launches/{id}/events` | - | after, limit | 200 LaunchEventPageView |
| `GET /api/v1/launches/{id}/requirements` | - | - | 200 RequirementsSnapshotView |
| `POST /api/v1/launches/{id}/start` | StartLaunchRequest; key | - | 202 LaunchView |
| `POST /api/v1/triggers/public/{public_id}/launches` | CreateLaunchRequest; key | - | 201 LaunchView; 202 LaunchView |
| `POST /api/v1/triggers/public/{public_id}/entry-intent` | - | - | 200 TriggerEntryIntentView |

### Discovery

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/conformance` | - | - | 200 ConformanceDocument |
| `GET /auth/signup-options` | - | - | 200 SignupOptions |

### Responses

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `POST /api/v1/conversations` | CreateConversationRequest | - | 200 Conversation |
| `GET /api/v1/conversations/{id}` | - | - | 200 Conversation |
| `GET /api/v1/conversations/{id}/items` | - | after, limit | 200 ConversationItemList |
| `GET /api/v2/ui/transport` | - | - | 200 UiTransportView |
| `GET /api/v2/agents/{agent_id}/events/head` | - | - | 200 EventHead |
| `GET /api/v2/agents/{agent_id}/events/snapshot` | - | limit, page_cursor | 200 EventPage |
| `GET /api/v2/agents/{agent_id}/events` | - | after_cursor, follow / Last-Event-ID | 200 Agent event SSE |
| `GET /api/v2` | - | - | 200 JSON capabilities |
| `POST /api/v2/responses` | CreateV2ResponseRequest | - | 200 SSE |
| `POST /api/v2/agents/{agent_id}/responses` | CreateV2ResponseRequest | - | 200 SSE |
| `GET /api/v2/responses/{response_id}` | - | - | 200 V2ResponseObject |
| `GET /api/v2/responses/{response_id}/events` | - | starting_after | 200 SSE |
| `GET /api/v2/conversations/{conversation_id}` | - | - | 200 Conversation |
| `GET /api/v2/conversations/{conversation_id}/items` | - | after, before, limit, order | 200 JSON items |
| `GET /api/v2/agents/{agent_id}/responses/{response_id}/controls` | - | - | 200 V2ResponseControls |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/controls` | V2ResponseControlRequest / required Idempotency-Key | - | 202 V2ResponseControl |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/interrupt` | V2InterruptRequest / required Idempotency-Key | - | 202 V2Interruption; atomic cancellation and distinct replacement response |
| `GET /api/v2/agents/{agent_id}/queued-inputs` | - | after, limit (1–4; default 4) | 200 V2QueuedInputPage |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-input` | V2QueuedInputRequest / required Idempotency-Key | - | 200 V2QueuedInput |
| `GET /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation` | - | - | 200 V2QueuedCancellationObservation |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation` | V2QueuedCancellationRequest / required Idempotency-Key | - | 200 V2QueuedCancellation |
| `GET /api/v2/commands/{command_id}` | - | - | 200 V2CommandStatus |
| `POST /api/v1/responses` | CreateResponseRequest | - | 200 SSE |
| `GET /api/v1/responses/{response_id}` | - | - | 200 ResponseObject |
| `DELETE /api/v1/responses/{response_id}` | - | - | 422 ProblemDocument |
| `POST /api/v1/responses/{response_id}/cancel` | - | - | 422 ProblemDocument |

### Profiles

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/defaults` | - | - | 200 DefaultProfilesView |

### Profile authoring

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/profile-authoring/bounds` | - | - | 200 ProfileCapacityBoundsView |
| `GET /api/v1/profile-authoring/services` | - | - | 200 array<AuthoringServiceView> |
| `GET /api/v1/profile-authoring/skills` | - | q, limit, cursor | 200 AuthoringSkillPage |
| `POST /api/v1/profile-authoring/repositories/resolve` | ResolveRepositoryRequest | - | 200 RepositoryResolutionView |

### Profile marketplace

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/marketplace/{kind}` | - | q, cursor, limit | 200 MarketplacePageView |
| `GET /api/v1/marketplace/{kind}/{id}` | - | - | 200 MarketplaceEntryView |
| `POST /api/v1/marketplace/{kind}/{id}/fork` | ForkProfileRequest; key | - | 201 ForkedProfileView |

### Sandbox profiles

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/sandbox-profiles` | - | cursor, limit | 200 SandboxProfilePageView |
| `POST /api/v1/sandbox-profiles` | CreateSandboxProfileRequest; key | - | 201 SandboxProfileView |
| `GET /api/v1/sandbox-profiles/{id}` | - | - | 200 SandboxProfileView |
| `PUT /api/v1/sandbox-profiles/{id}` | UpdateSandboxProfileRequest; key | - | 200 SandboxProfileView |
| `POST /api/v1/sandbox-profiles/{id}/retire` | -; key | - | 200 SandboxProfileView |
| `GET /api/v1/sandbox-profiles/{id}/revisions` | - | cursor, limit | 200 SandboxProfileRevisionPageView |
| `POST /api/v1/sandbox-profiles/{id}/revisions` | PublishProfileRequest; key | - | 201 SandboxProfileRevisionView |
| `GET /api/v1/sandbox-profiles/{id}/revisions/{revision}` | - | - | 200 SandboxProfileRevisionView |
| `PUT /api/v1/sandbox-profiles/{id}/visibility` | SetProfileVisibilityRequest; key | - | 200 SandboxProfileView |

### Agent channel usage

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agents/{id}/channel-usage` | - | - | 200 AgentChannelUsageView |
| `GET /api/v1/agents/{id}/channel-usage/history` | - | cursor, limit | 200 AgentChannelUsageHistoryView |

Channel usage is separate from immutable creation origin. An authorized summary
contains `agent_id` and `channel_usage` with `active`, `has_history`, nullable
`first_used_at` and nullable `last_used_at`. Only current bridge managers receive
`bindings` or can request paginated history. Binding attachment records history
without inventing an accepted external command. History retains actor, time,
reason and before/after binding generations, including reset, rebind and detach.
Current policy, original authority deadline and retained Agent identity are
rechecked after buffering. Public callers cannot write channel facts through
these routes, and channel use grants no additional CMA resource rights.

The creation inventory also returns the safe `channel_usage` summary on each row.
`channel_active=true|false` and `channel_history=true|false` combine with creation
origin, status and resource scope before pagination. For example, a manual Agent
with `channel_active=false&channel_history=true` retains its manual creation
origin after detachment. Empty filtered pages can have a continuation cursor.
Private binding, actor and external conversation references are absent from
inventory rows and exports.

### Approved Agent artifacts

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/agents/{id}/artifacts` | - | cursor, limit (1–50) | 200 AgentArtifactPageView |
| `POST /api/v1/agents/{id}/artifacts` | PublishAgentArtifactRequest; key | - | 201 AgentArtifactView, including idempotent replay |
| `GET /api/v1/agents/{id}/artifacts/{artifact_id}` | - | - | 200 AgentArtifactView |
| `GET /api/v1/agents/{id}/artifacts/{artifact_id}/download` | - | - | 200 application/octet-stream attachment |

Publication requires current `manage_sharing` authority and an explicit selected
`{name, media_type, content_base64}` body. No caller owner, actor, filesystem path
or remote URL is accepted. It records immutable bytes and publication audit
under exact retained Agent ancestry. Content must be 1 byte through 1 MiB of
canonical standard base64. Display names contain 1–160 UTF-8 bytes, trimmed,
without controls, slash/backslash or `.`/`..`. Supported types are `text/plain`,
`text/markdown`, `application/json`, `application/pdf`, `application/octet-stream`,
`image/png`, `image/jpeg` and `image/webp`. There are at most 256 artifacts per
Agent. Invalid content returns `invalid_artifact_request` (400); full capacity
returns `artifact_capacity` (429). Keep the same key/body when the result is
uncertain; a changed request under the same key returns `idempotency_key_reused`.

Metadata exposes exactly `id`, `agent_id`, `name`, `media_type`, `size_bytes`,
`sha256` and `created_at`. `view_artifacts` grants access to this explicit registry,
including for narrow Agent readers. It does not add `view_files` or reveal parent
paths/archives. Reads recheck current policy and the original authority after
buffering; revoked/expired access prevents later downloads. Download filenames
in HTTP headers are generated from the content hash, with `nosniff` and
`private, no-store`. SDK downloads return bytes; the web verifies size/hash and
current permission before creating its local file. Previously downloaded copies
cannot be recalled. Sandbox state is persisted by the attached Agent Sandbox
volume; CMA does not expose a Sandbox snapshot archive endpoint.

### Sandboxes

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/sandboxes` | - | organization, shared, status, cursor, limit | 200 AccessibleSandboxPageView |
| `POST /api/v1/sandboxes` | CreateSandboxRequest; key | - | 201 SandboxView |
| `GET /api/v1/sandboxes/{id}` | - | - | 200 AccessibleSandboxDetailView |
| `DELETE /api/v1/sandboxes/{id}` | -; key | - | 202 SandboxView |
| `GET /api/v1/sandboxes/{id}/events` | - | after, limit | 200 array<SandboxEventView> |
| `POST /api/v1/sandboxes/{id}/pause` | -; key | - | 202 SandboxView |
| `POST /api/v1/sandboxes/{id}/commands` | SandboxCommandRequest; key | - | 202 SandboxCommandReceiptView |
| `GET /api/v1/sandbox-commands/{id}` | - | - | 200 SandboxCommandReceiptView |
| `POST /api/v1/sandboxes/{id}/revive` | -; key | - | 202 SandboxView |
| `POST /api/v1/sandboxes/{id}/runtime-update` | UpdateSandboxRuntimeRequest; key | - | 202 SandboxView |
| `POST /api/v1/sandboxes/{id}/service-access` | GrantSandboxServiceAccessRequest; key | - | 200 SandboxView; new grants require fully Paused, existing grants replay without change |

`runtime-update` accepts `{ "target_version": "<exact image reference>" }`
with `Idempotency-Key` from the Sandbox owner. Use the owner detail's
`runtime_update.latest_version`, or its retained `target_version` when retrying a
failed update. HTTP 202 records the request; follow `runtime_update.state` on
Sandbox reads until completion. Busy Agents defer the update until idle, and
eligible managed Sandboxes retain their original home and conversations. Stale
or conflicting targets return 409; replay an uncertain request with its original
body and key.

Operators with `ControlAnySandbox` can queue the same update across owners using
`POST /api/v1/admin/sandboxes/{id}/runtime-update` with that request body. This
operator endpoint does not require an idempotency key: retrying the same pending
target leaves the request unchanged. It validates the configured latest target
or an already pinned retry target and audits the actual operator, Sandbox owner,
and target. Queueing does not interrupt current work; the existing idle checks
and transaction fencing retain the original PVC and conversation identities.
Operator Sandbox detail exposes `runtime_update` with the pinned target, current
state, and completion or failure metadata. The owner endpoint still hides
other owners' Sandboxes, including from operators.

Sandbox inventory is personal by default. `organization=<canonical ID>` selects
current organization ownership and whole-Sandbox grants. `shared=true` selects the
deduplicated union of incoming whole-Sandbox grants across current organizations,
excluding the caller's own personal resources before pagination. The two selectors
are mutually exclusive. Narrow Agent grants never reveal parent rows or counts.
Every aggregate page requires complete current membership discovery and checks
policy, evidence lifetime and hidden lookahead after count reads. Dependency
failure refuses the result without partial rows or a stale continuation. Both
SDKs expose this scope through their accessible Sandbox list method.

### Workspaces

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/sandboxes/{id}/workspaces` | - | - | 200 AccessibleWorkspaceListView |
| `POST /api/v1/sandboxes/{id}/workspaces/inline` | CreateInlineWorkspaceRequest; key | - | 201 WorkspaceView |
| `POST /api/v1/sandboxes/{id}/workspaces` | CreateWorkspaceRequest; key | - | 201 WorkspaceView |
| `GET /api/v1/workspaces/{id}` | - | - | 200 AccessibleWorkspaceDetailView |
| `DELETE /api/v1/workspaces/{id}` | -; key | - | 202 WorkspaceView |

### Service connections

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `POST /api/v1/service-connections` | CreateServiceConnectionRequest; key | - | 201 CreatedServiceConnectionView |
| `GET /api/v1/service-connections/{handoff_id}` | - | - | 200 ServiceConnectionStatusView |

### Triggers

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/triggers` | - | cursor, limit | 200 TriggerPageView |
| `POST /api/v1/triggers` | CreateTriggerRequest; key | - | 201 TriggerView |
| `GET /api/v1/triggers/public/{public_id}` | - | - | 200 TriggerPreviewView |
| `GET /api/v1/triggers/public/{public_id}/badge.svg` | - | label, theme, placement | 200 SVG; 304 empty |
| `GET /api/v1/triggers/public/{public_id}/open/{placement_id}` | - | - | 303 public Trigger page |
| `GET /api/v1/triggers/{id}` | - | - | 200 TriggerView |
| `PUT /api/v1/triggers/{id}` | UpdateTriggerRequest; key | - | 200 TriggerView |
| `DELETE /api/v1/triggers/{id}` | -; key | - | 204 empty |
| `POST /api/v1/triggers/{id}/publish` | PublishTriggerRequest; key | - | 201 PublishedTriggerRevisionView |
| `POST /api/v1/triggers/{id}/retire` | -; key | - | 200 TriggerView |
| `GET /api/v1/triggers/{id}/revisions` | - | - | 200 array<TriggerRevisionView> |
| `GET /api/v1/triggers/{id}/stats` | - | - | 200 TriggerStatisticsView |

Trigger deletion removes unpublished drafts and archives previously published
Triggers. Archived records disappear from owner reads/listing and refuse new
Launches; already-admitted Launches retain their frozen revisions and resources.
The exact successful deletion command remains replayable with its original key.

### Trigger placements

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/triggers/{id}/embed` | - | placement_id* | 200 EmbedDescriptor |
| `GET /api/v1/triggers/{id}/placements` | - | include_retired | 200 array<PlacementView> |
| `POST /api/v1/triggers/{id}/placements` | CreatePlacementRequest; key | - | 201 PlacementView |
| `POST /api/v1/triggers/{id}/placements/{placement_id}/retire` | -; key | - | 200 PlacementView |

### Workspace profiles

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/workspace-profiles` | - | cursor, limit | 200 WorkspaceProfilePageView |
| `POST /api/v1/workspace-profiles` | CreateWorkspaceProfileRequest; key | - | 201 WorkspaceProfileView |
| `GET /api/v1/workspace-profiles/{id}` | - | - | 200 WorkspaceProfileView |
| `PUT /api/v1/workspace-profiles/{id}` | UpdateWorkspaceProfileRequest; key | - | 200 WorkspaceProfileView |
| `POST /api/v1/workspace-profiles/{id}/retire` | -; key | - | 200 WorkspaceProfileView |
| `GET /api/v1/workspace-profiles/{id}/revisions` | - | cursor, limit | 200 WorkspaceProfileRevisionPageView |
| `POST /api/v1/workspace-profiles/{id}/revisions` | PublishProfileRequest; key | - | 201 WorkspaceProfileRevisionView |
| `GET /api/v1/workspace-profiles/{id}/revisions/{revision}` | - | - | 200 WorkspaceProfileRevisionView |
| `PUT /api/v1/workspace-profiles/{id}/visibility` | SetProfileVisibilityRequest; key | - | 200 WorkspaceProfileView |

### Worktrees

| Operation | Body / Key | Query | Response |
|---|---|---|---|
| `GET /api/v1/workspaces/{id}/worktrees` | - | - | 200 AccessibleWorktreeListView |
| `GET /api/v1/worktrees/{id}` | - | - | 200 AccessibleWorktreeView |



### Personal document Artifacts

Persistent documents are independent of their originating Agent, Workspace and Sandbox. `/api/v1/documents` is an authenticated per-user registry backed by majority-durable Mongo metadata in `cma_document_artifacts`; immutable Markdown sources live in the configured S3-backed object service. Agent/Sandbox deletion does not delete documents. List/search reads bounded metadata projections and never reads source objects or history/receipt payloads.

All writes require `Idempotency-Key`. Every metadata change increments `revision`; only title/source edits advance `source_revision` and add an immutable source revision. Owner title/source edits, grant/link changes and comment removal carry `expected_revision`. Stale revisions return 409; retries with the original key replay without repeating effects; different bodies under the same key return 409. Create replay returns current document metadata. Comments use a policy-fenced CAS with bounded retry and may anchor a specific source revision. Owner can remove any comment; a currently authorized commenter can remove their own.

Owner may grant `read` or `comment` to explicit authenticated subjects, or create authenticated token links. The browser generates a CSPRNG 256-bit token (64 lowercase hex), retains it only in the URL fragment and submits it as `X-CMA-Document-Share-Token`; the backend retains only SHA-256. Authentication is required in addition to the token. Links do not make documents searchable. Revocation is checked again after bounded source download, before bytes are returned. Document responses use private,no-store; nosniff; no-referrer. Ordinary `origin_agent_id` is an informational label and grants no runtime publishing authority.

Runtime MCP `publish_document` and `update_document` derive owner/Agent from confirmed runtime capability; no arbitrary owner field is accepted. Runtime update can change only verified runtime-authored documents for the current Agent/owner. The private runtime author field is separate from caller-supplied origin labels. Full runtime/conversation identity is checked again immediately before metadata commit after object upload, and replay requires current capability. Documents remain owner-accessible even if the originating runtime is removed.

Limits: UTF8 Markdown 1 MiB, title 240 bytes, search 160 bytes, comments 8 KiB; page 1–50 (default 25); source revisions 256; direct grants 128; links 32; comments 256; retained receipts 1024; serialized metadata 12 MiB. Sources are content-addressed by server-derived owner/document/digest keys; callers never select storage locations. A failed metadata CAS may leave an unreachable immutable source object; no document metadata or source is lost. There is no document deletion endpoint in this increment.

Errors:400 invalid_document_request/missing_idempotency_key/invalid_idempotency_key/invalid_json;401 not_authenticated;404 document_not_found conceals unavailable authority;409 document_revision_conflict/idempotency_key_reused;413 invalid_json size;415 json_content_type_required;422 invalid_json shape;429 document_capacity;503 document_unavailable.

| Operation | Behavior |
|---|---|
| `GET /api/v1/documents` | List/search owned and explicitly shared documents. |
| `POST /api/v1/documents` | Create owned Markdown document. |
| `GET /api/v1/documents/{id}` | Read effective document metadata. |
| `PATCH /api/v1/documents/{id}` | Owner source/title compare-and-swap. |
| `GET /api/v1/documents/{id}/source` | Read current or specific source revision. |
| `GET /api/v1/documents/{id}/revisions` | Page immutable source revision metadata. |
| `GET /api/v1/documents/{id}/shares` | Owner direct recipient inventory. |
| `PUT /api/v1/documents/{id}/shares/{subject}` | Grant/update read or comment access. |
| `DELETE /api/v1/documents/{id}/shares/{subject}` | Revoke direct recipient access. |
| `GET /api/v1/documents/{id}/share-links` | Owner link metadata, never secrets. |
| `POST /api/v1/documents/{id}/share-links` | Register authenticated token link. |
| `DELETE /api/v1/documents/{id}/share-links/{link_id}` | Revoke authenticated token link. |
| `GET /api/v1/documents/{id}/comments` | Page document comments. |
| `POST /api/v1/documents/{id}/comments` | Add comment under current grant. |
| `DELETE /api/v1/documents/{id}/comments/{comment_id}` | Owner or author comment removal. |

SDK surfaces: TypeScript `client.documents`; Rust `client.documents()`. Both expose list/create/read/update/source/revisions/shares/grant/revoke/links/createLink(create_link)/revokeLink(revoke_link)/comments/comment/removeComment(remove_comment). Read/comment options accept an explicit share token only in the header. Full and focused AgentManager OpenAPI documents include all 15 operations.



| `GET /api/v1/agents/{id}/preferences` | - | - | 200 AgentPreferencesView |
| `PATCH /api/v1/agents/{id}/preferences` | UpdateAgentPreferencesRequest; key | - | 200 AgentPreferencesView |

<!-- openapi-operations:end -->

## Contract Verification

From the repository root, export the actual Rust OpenAPI document and pass it
to the route-coverage checker (set `CARGO_TARGET_DIR` to the shared target when
working in this milestone's coordinated worktree):

```sh
cargo run --manifest-path api/Cargo.toml -p layer1-public --example openapi --locked --quiet | node .github/scripts/check-api-docs.mjs
node --test .github/scripts/check-api-docs.test.mjs
```

The Layer 1 test suite independently checks that every exported method/path is
present in this document, so route additions cannot silently pass the backend
gates while leaving this inventory stale. The OpenAPI JSON remains a generated
artifact, not a checked-in substitute for the server's current contract.

### Shared hierarchy reads

The Sandbox detail, Sandbox Workspace list, Workspace detail, Workspace Worktree
list and Worktree detail routes require current visibility of the requested
hierarchy. A narrow Agent share exposes none of these parents or their counts.
Whole-Sandbox grants, organization ownership and explicit global authority can
supply visibility under the common resource policy.

Personal-owner responses preserve the existing complete JSON. Other readers get
an explicit `access: "shared"` projection containing authorized IDs, names,
statuses, timestamps and child counts/summaries. These projections omit owner
identity, profile configuration, repository paths/URLs/branches/commits, runtime
bindings, lifecycle internals, credentials and failure diagnostics. Every child
and assigned Agent must agree with the authorized ancestry. Current policy and
unexpired request-bound membership are checked after the hierarchy read; changed
policy returns `resource_access_changed`, and unavailable/expired membership
returns `organization_authority_unavailable` without partial data.

`GET /api/v1/sandboxes` remains a personal collection. Explicit organization
ownership is excluded before pagination, including lookahead. An explicit `organization` filter selects organization-owned
and whole-Sandbox-shared resources under current membership. Organization
ownership admission remains to implement. Snapshot download remains
personal-only: hierarchy metadata visibility does not grant whole-home export.

`GET /api/v1/sandboxes?organization=<canonical ID>` returns an
`AccessibleSandboxPageView`. Each item is the compatible personal summary for
its personal owner or a safe shared summary with `access: "shared"`, Sandbox
ID/name/status/timestamps and authorized child counts. Child names/IDs are
available through separately authorized hierarchy detail/list reads. A narrow
Agent grant never supplies a Sandbox inventory candidate. Current organization
membership is required even for an empty page; failures do not become a false
successful empty scope. Ownership and whole-Sandbox grants are filtered and
deduplicated before pagination. The hidden lookahead and held evidence are
checked after count IO, before releasing either rows or `next`.
