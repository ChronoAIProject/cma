# CMA Operations

This runbook covers deployment-owned operations for the Sandbox/Agent model.
The current runtime contract is **3**, not the contract-2 image named in the
original #832 plan. Product semantics are defined in [CMA Model](CMA_Model.md);
public routes are in [REST](API_REST.md).

## CMAEG

CMAEG (CMA Event Gateway) is a separate stateless service that receives
external event channels and call agent providers. Its management service
listens on `CMAEG_LISTEN`, defaulting to `0.0.0.0:8090`. The deployment image is
`cma-cmaeg`; `infra/cmaeg/` declares one replica in `chrono-bot`, with credentials
injected from Kubernetes Secrets as environment variables.

The gateway loads its outbound provider policy once at startup. `CMAEG_PROVIDERS`
is a JSON array; an empty array is the default and means that no provider calls
are permitted. Each entry has this shape:

```json
[
  {
    "slug": "cma_codex",
    "base_url": "https://provider.example",
    "kind": "responses_http",
    "paths": {
      "agent_card": "/.well-known/agent-card.json",
      "binding": "/api/v1/cmaeg/bindings/{binding_id}",
      "conversation": "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}",
      "responses": "/api/v2/responses"
    },
    "max_inflight": 256
  }
]
```

Provider slugs are lowercase names of up to 32 characters. The registry accepts
HTTPS origins and cleartext HTTP only for loopback or Kubernetes `.svc` hosts;
credentials, queries, fragments, unresolved placeholders, traversal segments
and duplicate slugs are rejected before the service starts. Path variables are
percent-encoded as single segments, and redirects are disabled. Keep provider
credentials in the runtime secret mechanism; the allowlist contains origins
and route templates, never bearer keys.

The gateway startup settings are:

The metrics implementation uses `metrics` 0.24 and
`metrics-exporter-prometheus` 0.18 (MIT / Apache-2.0), with the exporter's optional
HTTP server and push client disabled. CMAEG owns the separate Axum scrape listener.
Registration is process-wide and idempotent. Histogram buckets are 0.005, 0.01,
0.025, 0.05, 0.1, 0.25, 0.5, 1, 2, 5, and 10 seconds. See
[CMAEG observability](CMAEG_Operations.md) for metric definitions, request tracing,
timing headers, and the status of the socket timing and latency harness.

Gateway configuration keys, their types/defaults, and runbooks are maintained in
[CMAEG operations](CMAEG_Operations.md).

Each allowlisted provider gets its own pooled HTTP client with a bounded idle
pool, no redirects and the CMAEG user agent. Cleartext `.svc` clients use HTTP/2
prior knowledge. Changes to any of these keys require a normal deployment and
startup validation; malformed policy or non-positive limits fail closed.

Credentials are injected from native Kubernetes Secrets as environment variables
and read before the listener binds. Invalid key rings, empty tokens, incomplete
service-account credentials, and invalid origins stop startup. The production
gateway obtains and renews its outbound CMA bearer through NyxID client
credentials; startup proves that exchange before opening the listener. The
separate service token authenticates inbound rekey operations. Credential values
never belong in the provider allowlist or ConfigMap. Optional legacy file inputs
remain available outside this deployment.

Creator authentication calls NyxID `GET /api/v1/users/me` with a three-second
deadline. Only token hashes are cached: successes for 60 seconds and failures for
10 seconds, with oldest-entry eviction at 10,000 entries. A cached success can
continue during a short NyxID outage; an uncached unreachable check returns 503.
Invalid bearers return 401. The record client sends the service bearer only to
the configured canonical CMA record URL, with no redirects or retries and a
1.5-second read deadline (five seconds for writes and lists).

The management implementation uses `dashmap` for the bounded authentication
cache and `subtle` for constant-time service-token comparison. It uses the
workspace's existing `time` and `uuid` dependencies for timestamps and resource
IDs. The pure protocol library uses `url` to validate record pointers, including
IPv6 loopback and operator-configured cluster addresses.

### Relay callbacks

`POST /v1/callbacks/{channel_id}` accepts JSON NyxID Telegram relay callbacks.
The gateway verifies single-valued callback token, message ID, timestamp and
platform headers, RS256 signature and pinned issuer/audience, exact body digest,
route membership, and the bound reply token before normalizing any activity.
An optional relay user token can corroborate the owner and key; it grants no
additional authority. JWT headers cannot supply a signing-key endpoint.

JWKS requests use only the resolved channel's retained URL, with no redirects,
a three-second timeout, a 32 KiB response limit and at most sixteen RSA keys.
The cache holds at most 4,096 URLs, removes expired idle entries at capacity,
and refuses new URLs when every slot is live. Failed fetches clear stale keys;
repeated misses and unknown key IDs are throttled for five seconds.

Only human text messages are admitted. Bot/system/unknown senders, edits,
interactive actions, system events, nontext content and attachments return
`202 {"status":"refused","reason":"..."}` with a stable reason. Sender and chat
allowlists and group admission are evaluated after verification. Mention gating
requires the pinned `bot_username` and Telegram mention entities (UTF-16 offsets);
missing identity evidence refuses the group message. Reply gating requires
`reply_to_message.from.is_bot == true`. Raw chat types and verified forum topic
IDs govern partitioning; normalized route metadata cannot supply a topic.

Admitted messages produce a neutral activity, deterministic conversation ID and
sealed event reference expiring no later than the reply token or thirty minutes.
Duplicate callbacks are handed through to provider idempotency. Accepted and
deferred launcher outcomes both return 202 with event and conversation IDs;
launcher refusals return explicit RFC 9457 problems. Ingress timing is defined by [CMAEG protocol §11](CMAEG_Protocol.md#11-s3-latency-and-server-timing).

### Provider turns and shutdown

Confirmed delivery references use a distinct authenticated token kind,
`cmaeg1.mr.<kid>.<sealed-data>`, carrying channel, event, route and platform message
identifiers. They are capped at 4 KiB and cannot be substituted for event refs.
Agent tool callers must present a single NyxID agent-key bearer (`nyxid_ag_` or
`nyx_` plus 16–128 URL-safe key characters); the gateway hashes it and compares
the channel's retained key hash in constant time without a NyxID request.

The production callback route starts a provider turn with the channel's retained
agent credential, deterministic conversation ID, and the same event idempotency
key on every retry. A missing conversation is created once before retrying the
turn. Upstream busy responses are retried within the configured deadline with
jitter; local provider capacity returns 503 `provider_busy` immediately with
`Retry-After: 2`. Header setup and the entire turn have separate deadlines.

The gateway acknowledges when the provider accepts, or at the acceptance budget
with a deferred 202. Deferred turns and turns whose HTTP caller disconnects keep
consuming the stream under the total turn deadline. Named outcomes cover
acceptance, deltas, committed messages, tools, completion, failure and incomplete
responses. Telegram and custom sinks consume these outcomes through the delivery engine. Stream loss and timeout remain explicit
`stream_lost` and `turn_timeout` failures.

Turn metadata carries hashed channel/partition identifiers, event and actor IDs.
Event references longer than the provider's 512-byte metadata limit are omitted
with a content-free warning. The reference still travels in the provider
`event_context` extension and the internal delivery context. No credential or source envelope is
rendered into provider input. Text assembly is bounded to 256 KiB and 256 items;
local tracked turns are bounded to 4,096 and tool-reply dedupe to 50,000 live refs.

On SIGTERM the listener continues answering `/readyz` with 503 `draining` while
tracked consumers finish; new launches return a typed 503 `draining`. HTTP drain
then completes within the same deadline. The deadline cancels remaining local
consumers and releases their provider permits; it cannot cancel work already
admitted remotely. Duplicate relay events may have separate consumers, with
provider idempotency deciding the actual turn. Tool-reply markers contain only
reference hashes and expire with their authenticated event reference.

The runner emits `cmaeg_ingress_to_provider_seconds` and
`cmaeg_provider_accept_seconds` as structured tracing measurements. The current
provider start timestamp marks HTTP dispatch; the observability work adds the
socket-write measurement required by the final latency harness.

### Agent tools and native MCP

Agent tools are available as REST and as stateless MCP streamable HTTP at
`POST /mcp` (protocol `2025-06-18`, JSON responses). The two interfaces call the
same authenticated operations. `initialize`, `ping`, `tools/list` and `tools/call`
are supported; other JSON-RPC methods return `-32601`. Notifications return 202
without performing a tool send. MCP validates the public Origin when present;
GET and DELETE return 405. Tool schemas are generated from the Rust request types,
with all four tools requiring `event_ref`; updates also require `message_ref`.

| REST operation | Frozen operation ID / native MCP tool | Result |
| --- | --- | --- |
| `POST /v1/events/{event_ref}/replies` | `replyToEvent` | Await the delivery receipt, including rendering and confirmed/rejected/unknown outcome. |
| `POST /v1/messages/{message_ref}/update` | `updateMessage` | Await an update receipt; Telegram relay edits are unsupported. |
| `POST /v1/events/{event_ref}/typing` | `sendTyping` | `sent` or `unsupported`; Telegram relay typing is unsupported. |
| `GET /v1/events/{event_ref}/context` | `readEventContext` | Public activity and reply capabilities, excluding credentials and the raw source envelope. |

NyxID exposes these as `cmaeg__replyToEvent`, `cmaeg__updateMessage`,
`cmaeg__sendTyping` and `cmaeg__readEventContext`. Reply/update REST bodies use
`OutboundMessage`; their MCP arguments wrap that same body in `message`.
A 20 KiB text reply is supported; text over 32 KiB returns 413. Tool HTTP bodies
are limited to 128 KiB. Delivery enqueue and receipt together have an eight-second
deadline; expiry returns 504 `internal` with `delivery_timeout`. A timeout does
not confirm whether delivery happened. Confirmed tool replies mark the exact
reference hash to suppress automatic final delivery. Telegram replies use NyxID's
relay reply endpoint; edits and typing return `unsupported`.

The Telegram renderer preserves escape atoms while splitting at 4,000 UTF-16
units, preferring paragraph boundaries. It breaks mentions with a zero-width
space and strips bidi controls. Cards become text, buttons and choices become
numbered text, forms become labelled lines, and attachments become placeholders
with HTTPS links where available. Rendering reports name each block degradation;
index `65535` denotes a whole-message `chunked:<count>` transformation.

Eight bounded workers serialize commands for each channel/event pair. Acknowledgements
do not suppress the final answer. Committed messages and tool replies suppress the
terminal fallback, and tool replies suppress failure notices. A tool send with an
unknown outcome also suppresses automatic output to avoid a possible duplicate.
Per-event state expires 30 seconds after the terminal outcome; hashes of consumed
single-use tokens remain until token expiry. These are local safeguards; gateway
restarts and multiple replicas rely on NyxID's single-use token enforcement and the
provider's durable turn idempotency.
Already-admitted internal turns retain delivery authority through completion even
when their event references expire; new tool calls still enforce reference expiry.
Expired reply tokens are replaced by the channel route key at dispatch time.

The first send uses an unexpired reply token; later parts and sends use the retained
channel route key. Each route has a three-second minimum interval on this replica.
A 429 delays one retry (Retry-After capped at ten seconds) using the channel key;
a second 429 or a malformed/lost response has an unknown outcome and is not retried.
Chunked sends stop on the first unconfirmed part and report partial delivery as
unknown. Their receipt contains the first confirmed platform message reference.
During shutdown the gateway drains provider consumers and then queued delivery
within the same deadline, so a terminal event cannot be discarded merely because
its stream consumer has exited.

Missing/invalid bearers return 401 and a channel key-hash mismatch returns 403.
Malformed/tampered event refs return 400 `event_ref_invalid`; expired refs return
410 `event_ref_expired`. Channel resolution still applies its deletion, restore
and cache rules. MCP tool failures include the same Problem document in JSON-RPC
`error.data`. An MCP update must bind both supplied references to the same channel
and event.

### Custom CloudEvents and sinks

`POST /v1/channels/{channel_id}/events` accepts structured
`application/cloudevents+json` and binary `application/json` with `ce-*` headers,
up to 256 KiB. Without `mode=stream`, it returns 202 after provider acceptance or
the configured acceptance budget, while work continues in the tracked runner.
The channel must have a custom source; malformed events and unsupported query
modes return 400. Custom ingress shares the callback concurrency limit.

Custom-source authentication uses the exact retained HMAC secret bytes and Standard
Webhooks `webhook-id`, `webhook-timestamp`, and `webhook-signature: v1,<base64>`
headers. Timestamps must be within five minutes; duplicate authentication headers
are refused. NyxID source keys are checked locally against the configured SHA-256
hashes. Custom channels admit only `fixed`, `event_subject`, or `per_event`
partitioning. Long URI subjects retain their full activity value and conversation
derivation; sealed references carry a digest when the partition exceeds 512 bytes.
Actor URIs over the provider metadata limit also use a digest in metadata; the
provider's stored activity and public context retain the complete URI.
Templates substitute known placeholders once, cap canonical data at 8 KiB and
total output at 32 KiB with `[truncated]`, and leave unknown placeholders intact.

A configured Sync sink enables `?mode=stream`: 200 SSE carries `cmaeg.text.delta`,
`cmaeg.message`, and terminal `cmaeg.completed` or `cmaeg.failed`, with keepalive
comments every 15 seconds. Disconnecting does not cancel the admitted turn.
Waiters are local to the replica, capped at 4096 with eight queued frames each and
a shared 32 MiB payload budget. Slow consumers receive `sync_backpressure` and
close. A sync tool receipt confirms acceptance into that local stream buffer;
the protocol does not acknowledge consumption by the caller. Tool replies on a
replica without the waiter still use the channel's other configured sinks.

Result-webhook sinks receive a signed CloudEvent on every tool reply and on
Completed, Failed, or Incomplete. Neutral messages remain verbatim in
`data.message`. The types are `cmaeg.response.message`, `cmaeg.response.completed`,
and `cmaeg.response.failed`. The source contains a channel hash prefix, not a
sealed reference. Provider diagnostics and credentials are excluded. A Source
sink on a custom channel delivers nowhere; it does not imply a webhook.

Webhook targets require HTTPS without user information or fragments. Each attempt
resolves DNS, rejects private/special-use addresses, pins the accepted addresses
for TLS connection, and refuses redirects and proxy environment settings. Each
attempt has a five-second total budget. Transient failures retry at 1, 4, and 16
seconds, keeping the same webhook ID and body and refreshing the signed timestamp.
Receivers must deduplicate that ID; retries can repeat a previously accepted send.
These retries are in process and share the delivery drain deadline. HTTP 4xx
other than 429, redirects, and invalid destinations are definitive refusals.

### Provider event-context extension

The 4 KiB sealed reference cannot contain an entire permitted 32 KiB activity.
To make context readable after a gateway restart, a turn's existing Responses
POST also carries `event_context` with schema version 1, `channel_id`,
`conversation_id`, the full normalized `activity`, and the opaque `event_ref`.
The provider must store it atomically with turn admission/idempotency and expose
its reference to the bound agent. This adds no network operation to ingress.
Synthetic management turns omit the extension.

Authenticated context reads use GET at the configured provider conversation path
plus `/events/{event_id}`. The gateway uses the retained agent credential, keeps
the request on the allowlisted origin, refuses redirects, and bounds response
time to three seconds and size to 1 MiB. Returned channel, conversation and event
identities must match the authorized reference. Unsupported or unavailable
provider context returns 503 `event_context_unavailable`; gateway memory is not a
fallback authority. The CMA provider implementation must implement this additive
contract before real context reads can succeed. Local tests cover a complete
32 KiB Unicode message read through a new gateway resolver.

### Channel management

The management API is available at `/v1/channels`; `/openapi.json` describes all
six operations, their authentication schemes, and the RFC 9457 error schema.
Every HTTP response, including health, routing errors and body-limit refusals,
carries `CMAEG-Protocol-Version: 1` and `Cache-Control: no-store`. Problem bodies
use `application/problem+json`; readiness failures retain their diagnostic JSON. The request body limit is 1 MiB; the sealed
channel definition must fit CMA's 65,536-byte record limit.

| Method and path | Authorization | Result |
| --- | --- | --- |
| `POST /v1/channels` | Creator bearer | Validate, bind the provider, encrypt and store the definition, return 201 with the channel ID, record ID, provider result and endpoints. |
| `GET /v1/channels/{channel_id}` | Owning creator | Return the public definition, current record version and endpoints. |
| `PUT /v1/channels/{channel_id}` | Owning creator | Replace channel policy with the required `expected_version`; return 200 or 409 on a stale version. |
| `DELETE /v1/channels/{channel_id}?expected_version=N` | Owning creator | Unbind the provider, delete the matching record and return 204. |
| `POST /v1/channels/{channel_id}/test` | Owning creator | Run one synthetic provider turn under a 30-second total deadline; return acceptance latency, up to 200 Unicode characters and the provider level. |
| `POST /v1/channels/{channel_id}/rekey` | CMA service bearer | Reseal the stored definition under the current master key with a version check and return its `kid`. |

Create requires schema version 1, a name of 1–80 bytes, an Agent Profile snapshot,
a registered provider slug, sources, partition and reply policies, and
credentials. The provider receives the profile, its canonical digest, the agent
key and the gateway tools advertisement. The channel key and HMAC secrets remain
inside the encrypted definition. A profile's author metadata is independent of
the authenticated channel owner. Relay sources require a channel key; custom
HMAC sources must reference a supplied base64 HMAC secret. The gateway derives
each relay JWKS URL from its HTTPS issuer origin.

Relay sources may pin an optional `bot_username` (5–32 ASCII letters, digits or
underscores, without `@`). This is the Telegram bot identity for mention matching;
the callback's agent name does not establish that identity. Scoped admission uses
`{"type":"deny"}`, `{"type":"open"}` or
`{"type":"allowlist","ids":["platform-id"]}` for sender and chat policies.

The returned endpoints identify the NyxID callback and custom-event URL that the
creator registers elsewhere. The callback route is available; the custom-event route is reserved for its
source implementation. Management creation does not register a NyxID route. After deleting a channel,
**the creator must also remove its NyxID route**. The gateway does not list
channels for creators; the creator keeps its channel IDs.

The optional create `Idempotency-Key` header is limited to 256 ASCII bytes.
Successful responses replay for ten minutes **on the same replica only**.
Concurrent equal requests on that replica allocate one record and binding;
reusing the key with another payload returns 409. A different replica or a
process restart has no replay entry. Keep creation requests on one replica when
using this guarantee. At 10,000 live replay entries, new keyed creates return
503 until entries expire.

Read/update/delete/test enforce the sealed definition's owner; another subject
receives 404. Malformed channel IDs return 400, while authenticated tampering or
an unknown key ID returns 404. CMA's record version is authoritative and is
returned both at the top level and in the public definition. Newly stored
records begin at version 1.

Updates preserve the channel ID, record ID, binding ID and creation timestamp.
The provider slug cannot change on an existing channel. Omitted credentials
are retained; explicitly supplied credential fields replace only those fields.
Changing either the profile digest or the agent key rebinds the provider before
the record write. A failed create record write triggers a best-effort unbind;
a failed update record write never unbinds the existing channel.

Provider mutation and CMA record storage do not share a transaction. Mutation
locks serialize changes within a replica, and CMA version checks detect racing
record writes across replicas. A failed write after rebind can leave the provider
with the attempted profile until the caller rereads and retries its intended
update with the latest version. A failed record delete after unbind should be
retried with the current version. Coordinate management writes for a channel
when multiple replicas serve creators.

For master-key rotation, put the new key first while retaining old keys, restart
the gateway through the normal deployment process, then invoke rekey for each
retained channel. Rekey rewrites the blob and advances its record version; the
channel ID remains stable. Retain the old key while any channel ID sealed with
it remains in use, even after its stored blob has been rekeyed.

Local verification is reproducible with
`cargo test -p cmaeg --test management` from `api/`. It starts fake CMA, NyxID
and provider services, launches the actual gateway binary with mounted test
files, exercises create/read/delete/OpenAPI using `curl`, and checks responses
and captured tracing output for the supplied credential fixtures. Other cases
cover provider refusal, record-store rollback, ownership, version conflicts,
credential retention, idempotency, Unicode output, truncated streams and rekey.

### Channel restoration and readiness

Ingress and tools use the shared channel resolver. A fresh entry is an in-memory
lookup with no outbound request. A cold entry opens the sealed pointer, checks
its exact CMA record URL, fetches one encrypted record, validates the definition
and decodes the agent-key hash. Concurrent misses for the same channel share
one restore, including when the initiating caller disconnects. The record
version overrides the version inside the encrypted definition.

Only record-not-found responses are negatively cached. CMA timeouts and server
failures return 503 `channel_restore_unavailable` and can be retried. Invalid
stored definitions return 503 `channel_invalid`; malformed IDs return 400 and
authenticated tampering returns 404 before CMA is contacted. Restore attempts
are rate limited per channel, returning 429 `rate_limited` after the available
burst is consumed. Warm hits do not consume this allowance.

An expired positive entry can be served while one background refresh is running,
for at most one additional TTL. Negative entries never get this grace period.
At capacity, insertion removes expired entries and then evicts the oldest entry
in a bounded sample of 32. There is no external cache or persistent replica
state. Update, delete and rekey invalidate the local cache, and an in-flight
restore cannot republish a definition invalidated by that operation. Other
replicas discover changes at the next TTL refresh; an outage can extend stale
serving to the single additional TTL described above.

`/readyz` reports `ready`, `reasons` and `cma_reachable`. Missing master keys or an
unparsed provider registry make readiness false with HTTP 503. Production
startup validates both before binding the listener. CMA reachability is probed
at `/ready` immediately and every ten seconds and is diagnostic only: a CMA
outage does not remove a locally initialized gateway from service. Unreachable
CMA can still prevent cold channel restoration while cached channels remain
available.

Each restore emits a `cmaeg.restore` span with its record ID and pointer key ID,
and an elapsed-seconds event carrying the reserved metric name
`cmaeg_restore_duration_seconds`. Opening a blob under an older key logs its
rekey need. Histogram export is added by the observability issue. No channel
credentials or decrypted record bodies are included in these events.

`cargo test -p cmaeg --test restore` exercises concurrent restoration, TTLs,
rate limits, capacity, cancellation, invalidation races and the latency bound.
Its HTTP harness submits 100 concurrent misses and checks one restore span and
100 responses. Its `curl` readiness test stops and restarts the local fake CMA
and observes the ten-second background probe through both reachability changes.
The `/resolve` route in that harness exists only in the test server.

## Owner Bots through CMAEG

Set `CHRONO_CMAEG_BASE_URL` to the internal gateway origin and
`CHRONO_CMAEG_PUBLIC_URL` to its public origin. They must be configured together;
the API refuses malformed origins at startup. Bot routes and the durable worker
are enabled only when both are present, using NyxID authentication.

An owner signs in to CMA with renewable NyxID credentials and needs
`cma:bot:manage`. A separate bearer or agent key cannot borrow that owner's
stored login. Before every background effect, CMA refreshes and verifies the
owner's current authority. Expired or revoked authority keeps work pending and
requires the owner to reconnect; it does not cause a fallback to the gateway
service identity.

The owner first registers a personal Telegram bot with NyxID and connects the
profile's services, the managed model service, and the `cmaeg` tools service.
CMA will refuse a bot that already has an active NyxID route, preserving existing
bridge routing. The `cmaeg` catalogue service must expose CMAEG's protocol tools;
its presence is a provisioning requirement, not something CMA creates implicitly.

`POST /api/v1/bots` accepts `{name,nyxid_bot_id,profile:{id,revision},admission,reply}`
and an `Idempotency-Key`. It returns 201 with a durable `creating` resource;
poll its GET route until `active` or `failed`. HTTP disconnects do not cancel
admitted work. The selected Agent revision and the published platform-default
Sandbox and Workspace revisions are pinned at admission. Managed model access is
required. Environment resources, workspace location, setup commands, service
scope, skills, prompt, permissions and thinking effort follow the CMAEG profile
schema; unsupported access is explicitly refused.

Creation mints separate 30-day agent and route keys in the exact
`cma:bot:<bot_id>:<agent|route>:<generation>` namespace. The agent key has only the
resolved profile/model/tool service grants. The route key has no service or node
grants. Plaintext exists only during bounded requests; Bot records retain IDs,
hashes and the earliest expiry. The channel reserves a durable record UUID,
starts with no admitted relay routes, then receives the exact route ID after
the callback and NyxID route are confirmed. Requests use the owner bearer,
explicit record versions, bounded response reads, no redirects and no automatic
mutation retries.

Enable, disable, rotation and deletion return 202 with operation progress.
Disable replaces admission with the protocol's scoped deny policy; enable
restores the original policy. Rotation retains channel and conversation identity,
reconciles provider and route credentials before revoking predecessors, and is
scheduled three days before the earliest key expiry. An interrupted creation
retains its reserved identity and enters cleanup after its claim expires; lost
plaintext is never reconstructed. Cleanup discovers only exactly owned keys and
routes and refuses conflicting use. Existing Sandbox continuity requirements
continue to apply to provider binding teardown.

The `/test` action performs a synthetic provider turn. It reports acceptance
latency and first text and does not send a Telegram message. It does not establish
channel delivery, ingress latency or restart continuity for a real conversation.
Local interoperability tests under `cma::gateway::tests` execute the compiled
gateway binary with HTTP fixtures and CMA's actual profile mapper. Build `cmaeg`
before running these tests in isolation; full CI builds the workspace first.

## Delivery Authority

For the 2026-09-22 gateway deployment, the maintainer additionally authorizes a
non-default `cmaeg-service` NyxID role with exactly `cma:gateway:records` and
`cma:provider:bindings`, a dedicated `cmaeg-prod` service account assigned that
role, and credential storage directly in Kubernetes Secrets. This is a narrow
exception to the earlier milestone's NyxID restriction below.

For this milestone, the maintainer's explicit instructions override the release
step in #832: deliver changes to `develop`, build images and deploy directly to
production. The 2026-09-16 delivery direction prioritizes parallel feature
implementation and consolidated required build checks, followed by production
deployment and broader live verification and fixes. **Never
merge or push `main`, invoke the release workflow or change NyxID configuration
or administration.** A direct
deployment is not a GitHub release and creates no release tag by implication.

The maintainer's 2026-09-14 continuity requirement in bridge issue #850 revokes
the historical refactor reset authority. No database reset, blanket provider
cleanup, forced pause or image-driven replacement of existing live Sandboxes is
permitted. Existing user resources receive read-only verification. Faults and
cleanup use exact run-owned disposable resources with owner and lineage checks.
Changing an image default does not authorize rotation of retained runtimes.
A failed required build check requires a fix and an affected rerun. Live
verification follows the initial control-plane rollout; unproved capabilities
remain disabled while those checks establish their enablement conditions.

The maintainer has authorized production deployment under this delivery plan.
The Release Evidence section below records incomplete verification explicitly.
Do not advertise public readiness or call the milestone complete while required
implementation or verification remains outstanding.
Controlled test enablement for the final journeys is distinct from public
publication and must be withdrawn if verification fails.

## Bridge Continuity Observation

The [owned continuity runners](CMA_Owned_Continuity.md) preserve the prepared
active-turn, native-history and passive browser-reconnect checks as committed,
parameterized tooling. They require successful disposable fixture receipts and
never select ordinary user resources or perform a deployment.

Issue #850 is a prerequisite for enabling milestone 17. Use
`.github/scripts/cma-bridge-continuity-observer.mjs` to capture read-only public
Agent and Kubernetes evidence for exact selected Agents. The observer issues no
Agent commands, runtime exec, lifecycle calls, credential changes or cleanup.
It stores hashes of durable events instead of message/tool bodies, and refuses
gaps, duplicate sequences, incomplete pagination and unavailable observations.

Keep the configuration and receipts outside Git. The saved NyxID access-token
file must be owned by the operator and inaccessible to other users. Configuration
selects the exact verified cluster context, namespace and deployment identity:

```json
{
  "schema": "cma-bridge-continuity-config-v1",
  "origin": "https://your-cma-origin.example",
  "context": "your-verified-kubernetes-context",
  "namespace": "your-cma-namespace",
  "deploymentId": "your-cma-deployment-id",
  "tokenFile": "/absolute/private/nyxid/access_token",
  "agentIds": ["agt_selected_existing_agent"],
  "watch": { "durationMs": 60000, "intervalMs": 2000, "sampleTimeoutMs": 15000 }
}
```

```sh
node .github/scripts/cma-bridge-continuity-observer.mjs capture /private/continuity.json /private/before.json
node .github/scripts/cma-bridge-continuity-observer.mjs capture /private/continuity.json /private/after.json
node .github/scripts/cma-bridge-continuity-observer.mjs compare /private/before.json /private/after.json
node .github/scripts/cma-bridge-continuity-watch.mjs /private/continuity.json /private/watch.json
```

Capture refuses to overwrite receipts. A comparison fails on changed Agent
ancestry, creation identity or profile pin, lost/changed historical event prefix,
loss of live availability, replaced Pod, changed container/image or restart count.
Normal appended history and transitions among Ready, Working and Waiting for
approval are permitted. Review identity and history regressions against the
requirement to resume the exact retained work. A temporary API interruption may
be allowed by the deployment scope; an observer finding does not create a
zero-outage permission gate or authorize repairs or cleanup. Record a failed
command separately and use new receipt paths for its corrected rerun.

The bounded watcher uses the same protected token and exact resource scope. It
polls sequentially, retains intermediate read failures and later recoveries,
compares each complete observation with the last complete observation, and
records public read timings, hashed event prefixes, durable cursors and runtime
identity. Each poll rereads the event prefix so changed historical records are
detectable. The receipt explicitly records intervals between samples,
incomplete samples and scheduled polls skipped by slow observations. Event and
tool bodies and external exception text are excluded from the receipt.

`durationMs` is between 1 second and 1 hour; `intervalMs` is between 250 ms and
60 seconds; `sampleTimeoutMs` is between 250 ms and 60 seconds. Neither interval
nor sample timeout may exceed the duration, and at most 1,200 polls may be
scheduled. A sample timeout cancels the current read before a later poll starts.
The total deadline, SIGINT or SIGTERM aborts active HTTP/body reads and kills and
reaps the active `kubectl` process. No new poll starts after stopping. Cleanup
and writing the receipt can finish after the sampling deadline.

The watcher reserves a new mode-0600 output file before making reads. Existing
files and symlinks are refused. Its `completed` status and zero exit code mean
the bounded observation finished; inspect `assessment`, `findings`, `samples`
and `gaps` for the recorded evidence. Temporary outages remain findings after
recovery. Cancellation or setup failure writes an incomplete receipt and exits
nonzero. These statuses do not decide deployment permission or establish exact
resume by themselves.

Successful sampled observations prove only their declared coverage. They do not
prove availability between samples, native conversation identity, dirty-file
preservation, exactly one runtime input, old-tab drafts, stream reconnect or safe
cleanup. The result names these unproved properties explicitly. Complete them
with the disposable previous-version live Agent upgrade/rollback campaign in
#850/#873, including approval waits, sibling Agents, paused history and Trigger
origin. Under the current delivery plan, broader live acceptance follows the
initial production deployment. Keep accepted work and existing runtime cohorts
intact through both feature disable and rollback; enable individual capabilities
only within their demonstrated runtime and authority contracts.

The shared IO admission ledger adds `agent_commands` and
`agent_command_orders` collections without changing the retained Agent or log
record schemas. Command input, response identity, immutable acceptance facts and
queue order commit in one Mongo transaction, with unique authority/key and
Agent/order indexes. Resource observations are fenced in the same transaction.
There is no TTL on command keys. An unknown commit returns an unavailable
outcome; only a retry with the same key may resolve that reservation. No runtime
input is sent by the reservation adapter.

Command workers claim one per-Agent dispatch lane with a bounded 1–60 second
lease and a monotonic fence. Renewal, release and refusal require the exact live
claim. A refusal of unclaimed queued work instead compares its exact observed
revision and proves the absence of both a binding and a live lease atomically;
this permits revoked/expired work to finish while an Agent is paused or busy.
Normal messages retain their order while controls retain the active turn
observed at admission. In one transaction, the conversation store binds the
claimed command to its exact input, metadata, operation, turn and runtime
generation, and commits the Agent and semantic logs. Runtime input follows only
a confirmed commit. Ambiguous-commit verification includes the exact command
binding, since identical controls can otherwise have identical Agent/log facts.
A bound command cannot be claimed for dispatch again, including after lease
expiry or release. Existing queued messages and Trigger first-message delivery
retain priority. Production admission and credential custody currently accept
matching personal callers; a channel envelope does not establish a personal caller.

Command outcome recovery reads the complete authoritative Agent log in one
snapshot and records the observed cursor and delivery separately from the
immutable dispatch binding. It reconstructs turn completion, interruption and
approval waits, retains unresolved delivery until runtime evidence arrives, and
never clears a newer command's lease. Runtime loss preserves uncertain writes
as typed failures even when retirement logs the turn interruption first. The
durable Agent log provides the recovery source if the response projection write
is interrupted; live fragments cannot establish completion.

Initial conversation opening persists the native identity and its open receipt
before Ready publication appends `ConversationOpened`. History readers accept
that unpublished generation only when the completion log matches the exact
Agent's durable open receipt and no Ready state or turn has been published. This
keeps early Responses admission observable across that startup boundary without
changing retained identities, log order or worker recovery. Published conversation
identity and generation still have to match their semantic log.

The production command scheduler scans nonterminal records in stable ID pages
of 50 through the additive `agent_command_work_scan` index. Successful pages
advance the cursor even if their work remains blocked. Up to eight tasks run
concurrently; slow discovery occupies a separate task. Store IO is bounded to
15 seconds, execution attempts to 45 seconds, and 30-second lane leases renew
every 10 seconds with a five-second renewal bound. Any uncertain renewal stops
the attempt. Settlement rereads the ledger before release or refusal: a binding
always selects reconciliation, and an unavailable read cannot prove an absent
dispatch. Shutdown stops new work, drains for up to 25 seconds, then cancels
remaining attempts; their durable leases/bindings support the next replica.

Unclaimed message preparation obtains fresh authority and checks the current
record again before entering the existing Sandbox revival pipeline. It holds no
short dispatch lease and sends no input. Preparation is bounded to 15 seconds;
an admitted revival remains durable if the attempt is cancelled. The resource
execution adapter checks current owner kind and target state, rereads the renewed
claim after credential IO, and retains the actual actor. It obtains an opaque
resource execution proof before dispatch. The proof binds the exact command,
owner, policy revision, ancestry and pinned profile and has a monotonic lifetime
of at most 60 seconds. Membership-dependent proof expires at the earlier
membership deadline. Reads and retries cannot renew an existing proof.

Both in-memory and Mongo conversation commits verify that proof against their
own current resource snapshot. Mongo retains the real parent write that
serializes policy changes with dispatch, and checks expiry again before commit.
The ordered runtime writer checks held policy, lifetime, target conversation,
runtime generation, operation and turn after queueing and before invoking the
runtime handler. Refusal before handler invocation returns a pre-write failure
and leaves the writer usable. Cancelling a pending send also prevents later
handler invocation without retiring the writer; if its caller disappeared before
recording the result, the durable binding still requires reconciliation. Once
handler invocation begins, uncertain delivery retires the writer generation and
requires reconciliation. Outcome evidence can commit after authority expires or
is revoked.

The production admission and execution boundaries support an independently
authorized human actor on a shared or organization-owned Agent. The actor supplies
their own retained sign-in; runtime and connector credentials remain with the
resource owner. Preparation obtains separate Agent interaction and Sandbox
management decisions, fences the exact pending command and schedules one durable
revival step. Mongo writes a fresh command-row marker alongside the parent and
event to serialize concurrent claim/refusal writers. Ambiguous commit recovery
uses that exact marker and remains possible after the proof expires. Organization
resource creation/credential provisioning and bridge delegation remain incomplete.
Human command admission and replay use the [Responses v2 contract](API_Responses.md).

`command_authorization_audits` is an additive collection with an immutable audit
ID and a `(command_id, _id)` index. New human admission, resource dispatch binding
and command-authorized Sandbox revival append evidence in the same transaction
as the accepted effect. Admission retries and AlreadyScheduled/AlreadySatisfied
revival observations retain prior evidence without appending another entry.
Each entry separates credential custody from the actual resource policy revision,
owner and actor, and records the capabilities supplied by personal ownership,
operator authority, organization roles and exact share revisions/modes. Revival
has separate Agent and Sandbox observations. These are historical observations,
never current permission; revocation does not delete them or replay accepted work.

No raw access/refresh token, input content, private runtime configuration or native
conversation identity is copied into audit evidence. Audit persistence is internal
and does not widen command status or recipient history projections. Audit IO is
part of acceptance: an audit write failure or expiry before commit rolls back the
whole effect. Old serialized command receipts and Agent/Sandbox aggregates retain
their existing formats. Older code can continue reading them during rollback,
while historical unaudited effects remain explicitly unaudited. There is no audit
TTL or cleanup migration in this change. Release upgrade/rollback evidence still
requires the disposable continuity campaign.

New authenticated Trigger launches also retain an immutable initiating-actor
sidecar, distinct from the Trigger identity that performs resource creation.
This evidence is fenced with the exact Launch effect when Agent provenance is
created. Older launches remain valid with an unknown initiating actor. Rollback
acceptance for these new writes is still open: an older whole-document Launch
writer can discard the sidecar, and an older provenance validator can reject a
distinct initiating actor. Before rollout, a compatible rollback writer/reader
and the previous-version disposable continuity campaign must preserve this
evidence; deleting or relabeling it is not a recovery procedure.

Ordinary CMA provider authentication refuses NyxID relay access, callback and
reply tokens, including relay key/scope claims. NyxID userinfo can project the
bot owner's subject and RBAC from a relay access token; that projection does not
grant CMA dashboard or owner-operation authority. The local claims inspection
only denies restricted credentials. Every other credential still goes through
the existing provider verifier. Bridge ingress must validate its own callback
and scoped execution authority before admitting work.

The disposable cluster bootstrap is
`.github/tools/cma-bridge-continuity/bootstrap.py`. Install its pinned Python
requirements, then supply the repository, a fresh private directory outside Git,
the verified cluster context and fixture owner subject. It creates a random
`cma-bridge-e2e-<run>` namespace with a resource quota, a separate Mongo replica
set Pod, provider server/key, namespace-scoped service accounts and roles, and a
credential-free home seed. It reads the deployed component pins and copies only
the existing NyxID application and storage client credentials; it never copies
production database access, encryption keys, provider keys, user homes or history.
Runtime NetworkPolicy blocks other namespaces and metadata endpoints.

The bootstrap journal records exact namespace and resource UIDs before later
mutation. Its guard refuses production namespaces, foreign markers, replacement
resources, unjournaled resources and broader cluster roles. The only fixture
cluster role permits reading the named `gvisor` RuntimeClass. `resume.py` accepts
the same private directory and reconciles exact creation intents; an ambiguous
create needs the same run marker and creation-spec fingerprint. A disappeared
journaled object is not silently recreated. Neither tool exposes blanket cleanup.

The operator needs ordinary workload creation plus permission to create/bind
the declared fixture roles. Bootstrap checks these create permissions with
SelfSubjectAccessReview before creating resources. A later admission denial
leaves the completed journal intact and must not be worked around by borrowing
a more privileged production service account. Resume requires the journaled
create intent as well as the matching resource fingerprint and namespace UID.
Initialize only the journaled Mongo Pod's replica set, wait for
the new home seed, create its snapshot and pin its observed UID before creating
the saved previous-version API manifests. Checkpoint objects for each run-owned
Sandbox use the existing `cma/checkpoints/<sha256(sandbox_id)>/` prefix, recorded
alongside the exact Sandbox identity. Existing-user checkpoint prefixes remain
outside the campaign's cleanup authority. Creating the environment alone proves
none of the live upgrade, rollback or browser acceptance cases.

## Prerequisites

- Verify the exact cluster context, namespace, deployment id, database and bucket
  before any write. The recorded production namespace is `chrono-bot`, deployment
  id `chrono-bot-prod` and database `chrono-bot`; re-read live configuration rather
  than relying solely on this historical record.
- MongoDB must support sessions, transactions, snapshot reads and majority
  acknowledgment. The application identity needs its documented collection,
  index and validator privileges. A standalone local Mongo process is not a
  substitute for transaction/recovery tests.
- The sandbox provider must accept the pinned image, nonroot home and requested
  resources. The service-account/SecretProviderClass relationship and mounted
  provider key must resolve CMA's own authority, never a similarly named class
  in another namespace.
- Supply referenced secrets out of band. No secret value belongs in a manifest,
  commit, command argument, screenshot or evidence transcript. Keep the existing
  authentication encryption key stable unless an explicit migration is designed.
- External checkpoint bytes use the existing chrono-storage bucket
  `chrono-platform-bot`, not a renamed GridFS bucket. Verify the machine identity
  can upload, read and delete exact objects through the platform proxy and that
  DELETE is independently confirmed by GET 404. Metadata remains in MongoDB.
- The provider-facing model endpoint and baseline services must be usable for a
  fresh visitor. A successful admin account test does not establish that fact.
- Build, label, scan and inspect all three immutable image digests. Check the
  runtime contract label and pinned Codex/bridge versions against the final
  image. Historical Agent-home evidence is a revalidation input, not a pass.

The historical cluster context in [Sandbox Provider](Sandbox_Provider.md) is
`gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke`.
Confirm it is still the target before any context-pinned command. Do not change
account/project context globally while other work may be active.

### Image Identity

All three Dockerfiles declare final-image OCI source, revision and version
labels, including direct builds that do not use the release workflow. The
source is exactly `https://github.com/ChronoAIProject/cma`; supply the complete
reviewed source commit as `CHRONO_BUILD_SHA` and the chosen deployment build
name as `CHRONO_BUILD`. Default development values are not deployable identity.

Inspect every built platform's image configuration before publishing or
deploying. Require those exact three labels and the runtime contract label on
the Sandbox image. An inherited base-image source label, a build log, or a
Dockerfile source check does not establish the installed image's identity.
Labels themselves are assertions, not proof of origin: retain the immutable
source snapshot, build inputs, build result and scan evidence bound to each
digest. Rebuild after source changes; do not relabel older executable content
as a newer commit.

The Rust builders use locked, component- and architecture-scoped target caches
to reuse compilation during verification cycles. Each build still runs Cargo
with `--locked` and copies its selected executable out of the cache into the
build layer; final images contain neither compiler nor cache. Cached artifacts
are not acceptance evidence. Build, identity, runtime and vulnerability gates
apply to the resulting immutable image on every delivery.

CI retains each tested native API, web and runtime image for one day after a
successful component job on `develop` pushes and pull requests from this same
repository. Export runs after the vulnerability scan and, for runtime, the
executed contract and its evidence verification. The artifact is named
`ci-native-image-<component>-<actual-source-sha>-<run-id>-<attempt>` and contains a
compressed Docker-save archive plus `metadata.json`. Export saves the inspected
immutable image ID with fast gzip compression; Actions does not recompress it.
It adds no image build, registry credentials, registry write or deployment.

Use artifacts only from the exact successful CI run and attempt whose complete
required checks passed. Verify the repository, source revision, source tree,
component, `linux/amd64` platform, image ID, archive filename, size and SHA-256
against the metadata. For a PR artifact, independently compare its recorded Git
tree to the merged `develop` commit; a matching branch name or PR number is not
equivalence. After `docker load`, inspect that exact image ID again and require
its source/revision/version labels and the runtime contract to match the
metadata. Preserve its actual tested revision and version labels, including CI's
`development`/`dev` version; do not relabel it as a later commit or release.
Source-tree equivalence and the original successful checks provide the delivery
identity for this CI handoff. A registry push still requires separately
authorized delivery and verification of the resulting immutable registry digest.

These archives contain only the tested native platform and do not establish the
multi-platform root digest or release identity described in [Release](Release.md).
They are unencrypted and readable by the repository's Actions-artifact audience;
one-day retention limits duration, not readership. Expired artifacts require a
new successful CI run. SDK packages, release workflows, registry ACLs and tags
are unaffected.

### Lifecycle Scheduling

Bridge ingress, worker occupancy/results and outbound attempts emit bounded
operational log fields through the existing sink. See
[Bridge operational signals](BRIDGE_OPERATIONAL_SIGNALS.md) for dimensions,
latency/age semantics, safe correlation and suggested log-based monitors.

Sandbox lifecycle scheduling permits at most four concurrent sweeps, with
one new sweep offered per second while capacity remains. Durable per-Sandbox
claims and revision fences, not the scheduler's task list, grant authority.
A slow restore therefore does not monopolize all lifecycle work, and a new
pause-cancellation fence can be claimed while its superseded capture unwinds.
Four simultaneous long operations can still consume all slots; this is a
bounded concurrency improvement, not an unbounded latency guarantee.

Workspace, periodic checkpoint maintenance, and checkpoint-object cleanup
sweeps remain serial. Shutdown stops admitting sweeps, permits a 25-second
drain, and cancels remaining child tasks without clearing their durable leases.
The API requests 256 MiB and is limited to 1 GiB to accommodate the existing
bounded multipart buffers across concurrent lifecycle transfers.

## NyxID Prerequisites

CMA uses two independent origins. `CHRONO_NYXID_BASE_URL` is the API/OAuth
origin; `CHRONO_NYXID_PUBLIC_URL` is the browser registration/connect origin and
must be one canonical HTTPS origin. Production configuration names
`https://nyx-api.chrono-ai.fun` and `https://nyx.chrono-ai.fun` respectively.
`CHRONO_SERVER_PUBLIC_URL` remains the CMA browser origin so its host-only cookie
and callback return to the same application.

Read `GET /auth/signup-options`. It returns only `email_signup`,
`social_providers`, `invite_code_required` and `register_available`.
`register_available: false` means no usable registration path is advertised:
the provider may have disabled all methods, or its configuration could not be
obtained safely. Do not advertise account creation from a guessed provider URL.
`INVITE_CODE_REQUIRED` is **NyxID's** operator setting, not a CMA environment key.
If it is enabled, public signup is not established without an authorized invite
path. Under this delivery's authority, report the prerequisite and stop public
enablement rather than modifying NyxID.

The required pre-existing default visitor role has these exact values:

```json
{
  "name": "CMA visitor",
  "slug": "cma-visitor",
  "description": "Can start CMA agents from trigger buttons and use their own sandboxes",
  "permissions": [
    "cma:trigger:launch",
    "cma:sandbox:use",
    "cma:sandbox:delete"
  ],
  "is_default": true
}
```

The role-administration contract is NyxID's `POST /api/v1/admin/roles`, not a
route CMA calls. The JSON above is a verification reference, **not authorization
to create or change the role in this delivery**. Do not add sandbox-create,
profile-management, Trigger-publishing or administrator permissions. A fresh
user must receive the intended defaults from the existing provider configuration.

CMA derives a visitor from `LaunchTriggers` without `CreateSandbox`; it does not
trust a role slug. Hosted OAuth/API-key/device-code connection pages are also
NyxID-owned. CMA creates short-lived links as the visitor and polls their result,
without registering another callback or handling a service secret in its UI.

## Browser Authority And Recovery

A valid CMA browser cookie identifies the account; it does not by itself grant
resource permissions or prove renewable credentials are available. In provider
mode, requests with usable durable access tokens verify live userinfo and,
when advertised, introspection. Stored permission snapshots are not request
authority. Discovery, userinfo and introspection share a 15-second verification
deadline; a durable reread then requires the same owner, login incarnation,
token generation and exact still-usable access token. This is a request-time
check, not an atomic binding of a later browser request or resource mutation.

Permission evidence must contain a complete string array. Introspection must
contain a boolean `active`; an optional `sub` must exactly match userinfo.
Malformed evidence or provider failure fails closed without using old grants.
Missing, expired or blank durable access, or explicit inactive evidence, leaves
browser identity only: no capabilities and no delegated credential. An active
token with `permissions: []` grants no capabilities but remains a verified
delegated credential. Bearer and login verification reject inactive evidence.

On-demand renewal is coordinated in MongoDB, not a process-local mutex. Each
subject has a login `incarnation`, monotonic `revision` and `token_generation`,
and a nullable `refresh_lease`. The lease lasts 45 seconds; another caller waits
at most 20 seconds, polling every 50 milliseconds. A cancelled worker leaves a
bounded lease for takeover. Completion checks the exact incarnation, generation,
holder and unexpired lease, then replaces by revision without upsert. New login
invalidates old claims; stale success, rejection or abandonment cannot overwrite
it or release another worker's claim. Completion preserves the latest profile
and grants; terminal token loss clears stored grants in the same replacement.

Only a refresh response with HTTP `400` and `error: "invalid_grant"` discards
renewable custody. HTTP `401`, `429`, provider outages, malformed responses and
transport errors retain it and release only the caller's claim. Nonrenewable
access remains usable until actual expiry, when tokens and stored grants are
cleared. Browser-cookie expiry and logout remain separate from token custody.

The exact `/auth/login`, `/auth/callback` and `/auth/logout` routes do not resolve
ambient credentials before running. A stale cookie therefore cannot prevent
explicit sign-in or logout. Callback state, expiry, one-time consumption, PKCE
and initiating-browser binding still apply; navigation or callback never starts
a Sandbox or Launch. Other authenticated routes still verify current authority.

Successful explicit login can repair an exact current-schema `auth_subjects`
row only when **all four** fencing fields are absent. It installs new verified
tokens and a fresh incarnation. Existing cookies and background refresh cannot
infer missing generations or repair such a row. Partial or malformed fencing,
unknown document schemas and counter exhaustion fail closed, including during
login. Do not add fields by hand or erase authentication collections as a
recovery shortcut; those cases require a separately reviewed data repair.

New Sandbox admission additionally requires the owner's renewable credential
prerequisite. A typed `409 sandbox_credential_setup_required` requires explicit
same-account sign-in and continuation. Retain an ambiguous command's original
key and body; use a fresh key only after a definitive refusal. A successful
`GET /auth/session` alone does not prove that the next admission will succeed.

## Configuration Inventory

Every quoted `CHRONO_*` key read by `cma/src/config.rs` and its modules is listed
below. `.github/scripts/check-config-keys.sh` checks manifest/reference parity;
`.github/scripts/check-config-docs.mjs` checks this documentation. Secret values
are referenced in `infra/api/secret.yaml`, never stored in ConfigMaps.

| Key | Contract / Default |
|---|---|
| `CHRONO_SERVER_BIND` | Listen address, default `0.0.0.0:8080` |
| `CHRONO_SERVER_PUBLIC_URL` | CMA browser origin; default local `http://localhost:8080`, production explicit |
| `CHRONO_SERVER_CORS_ORIGIN` | Comma-separated exact allowed origins; default local `http://localhost:5173` |
| `CHRONO_AUTH_MODE` | `provider` in production; `trusted` is local fixed-subject development only |
| `CHRONO_BRIDGE_MANAGEMENT_ENABLED` | Fresh Bridge setup/configuration/enable/grant/import/retry authority; false by default; retained reads, exact receipts and safe reductions stay available |
| `CHRONO_BRIDGE_INGRESS_ENABLED` | Fresh signed callback acceptance; false by default; accepted input can drain through independently enabled stages |
| `CHRONO_BRIDGE_CONTROL_ADMISSION_ENABLED` | Fresh signed cancel/steer admission selector; false by default; existing Message/refusal/control receipts recover first. Fresh admission and advertised operations additionally require current source consent/grants and the exact locally published runtime with Conversation plus Interrupt/Steer. Missing local ownership remains retryably unavailable; no remote owner proxy is composed. |
| `CHRONO_BRIDGE_ALLOCATION_ENABLED` | New Channel allocation/provisioning/child/start effects; false by default; preserve current phases and in-flight outcomes |
| `CHRONO_BRIDGE_EXECUTION_ENABLED` | New Channel command admission and held-command release; false by default; already Released commands drain in shared FIFO order |
| `CHRONO_BRIDGE_OUTBOUND_ENABLED` | Fresh explicit reply-tool admission and provider send attempts; false by default; recover retained receipts/outcomes and never auto-retry Unknown |
| `CHRONO_BRIDGE_TELEGRAM_ENABLED` | Telegram fresh-effect gate, independent of other platforms and stages; false by default |
| `CHRONO_BRIDGE_TELEGRAM_NEW_ENABLED` | Telegram-new fresh-effect gate, independent of Telegram and other stages; false by default |
| `CHRONO_BRIDGE_DISCORD_ENABLED` | Discord fresh-effect gate; false by default |
| `CHRONO_BRIDGE_DISCORD_TEXT_ATTACHMENTS_ENABLED` | New single Discord text-file admission; false by default. Requires independent Discord/ingress gates and current authority; retrieval/Agent work also requires normal allocation/execution gates. Deploy supporting readers/workers before enablement. Disabling preserves original receipts and lets accepted work drain under normal gates; before rollback, drain it with a supporting worker or retain explicit suspension. See [attachment retrieval](BRIDGE_ATTACHMENT_RETRIEVAL.md). |
| `CHRONO_BRIDGE_LARK_ENABLED` | Lark fresh-effect gate, independent of Feishu; false by default |
| `CHRONO_BRIDGE_FEISHU_ENABLED` | Feishu fresh-effect gate, independent of Lark; false by default |
| `CHRONO_BRIDGE_SLACK_ENABLED` | Slack boundary gate; false by default; missing human/loop evidence still refuses admission |
| `CHRONO_BRIDGE_WHATSAPP_ENABLED` | WhatsApp boundary gate; false by default; missing human/loop evidence still refuses admission |
| `CHRONO_BRIDGE_X_ENABLED` | X DM boundary gate; false by default; missing human/loop evidence still refuses admission |
| `CHRONO_BRIDGE_WORKSPACE_PER_PARTITION_ENABLED` | Fresh shared-Sandbox Workspace placement selection and allocation; false by default; requires complete runtime support and management/allocation/execution gates. Independent operator rollout choice after canary, not continuity attestation; exact receipts, unchanged placement edits and retained work survive disabling. |
| `CHRONO_BRIDGE_WORKTREE_AGENT_PER_PARTITION_ENABLED` | Fresh shared-Workspace Worktree/Agent placement selection and allocation; false by default; independent of Workspace-per-partition selection with the same runtime and stage requirements. Retained execution, reconciliation and cleanup stay authorized normally. |
| `CHRONO_BRIDGE_V2_UI_TRANSPORT_ENABLED` | Eligibility for the separately integrated v2 browser transport; false by default; does not disable existing public v2 API routes or change an accepted request protocol |
| `CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED` | New authenticated Launch admission selects common first-message commands under retained personal ownership; false by default. Proven pre-write failure is terminal for new common prompts; ambiguous writes retain the same command/response and refuse legacy recovery. Accepted protocol is immutable; disabling the selector retains accepted common commands and legacy launches retain their original delivery/recovery behavior. Keep false until every worker and permitted rollback version recognizes and transactionally excludes common/legacy dispatch; older workers ignore additive sidecars. |
| `CHRONO_TRUSTED_SUBJECT` | Local trusted-mode subject; default `local-developer`, irrelevant in provider mode |
| `CHRONO_NYXID_BASE_URL` | Required API/OAuth HTTP(S) origin in provider mode |
| `CHRONO_CMAEG_BASE_URL` | CMAEG management HTTP(S) origin; production `http://cmaeg`. Set together with its public origin; omitting both disables Bot routes/workers. |
| `CHRONO_CMAEG_PUBLIC_URL` | CMAEG public HTTP(S) origin used to validate returned callbacks; production `https://bot-gateway.chrono-ai.fun`. |
| `CHRONO_NYXID_PUBLIC_URL` | Required exact canonical HTTPS browser origin in provider mode |
| `CHRONO_NYXID_CLIENT_ID` | Provider-issued OAuth client id, not a chosen display name |
| `CHRONO_NYXID_CLIENT_SECRET` | Required OAuth client secret reference |
| `CHRONO_AUTH_ENCRYPTION_KEY` | Secret reference: standard-base64 encoding of exactly 32 random bytes |
| `CHRONO_MONGODB_URI` | Required secret reference, `mongodb://` or `mongodb+srv://` |
| `CHRONO_MONGODB_DATABASE` | Database name; code default `chrono_bot`, production explicitly `chrono-bot` |
| `CHRONO_DEPLOYMENT_ID` | Required stable Kubernetes-label-compatible CMA ownership identity |
| `CHRONO_SANDBOX_PROVIDER` | New allocation backend: `opensandbox` (compatibility default) or `agent-sandbox` |
| `CHRONO_RETAIN_OPENSANDBOX` | Keep legacy provider available for persisted bindings; default true |
| `CHRONO_AGENT_SANDBOX_ENABLED` | Keep direct agent-sandbox access available during coexistence/rollback; selecting it as allocation backend also enables it |
| `CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE` | Dedicated mounted signing key, at least 32 bytes; required when agent-sandbox is enabled; retain across restarts |
| `CHRONO_AGENT_SANDBOX_RUNTIME_CLASS` | Optional infra-provided RuntimeClass name |
| `CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT` | Workload service account; default `cma-runtime`, API token automount disabled |
| `CHRONO_AGENT_SANDBOX_IMAGE_PULL_SECRETS` | Comma-separated namespace-local image pull Secret names, or empty for node/infra registry pull identity |
| `CHRONO_SANDBOX_BASE_URL` | Required canonical HTTP(S) provider origin |
| `CHRONO_SANDBOX_API_KEY_FILE` | Required readable nonempty mounted provider key; never a ConfigMap value |
| `CHRONO_SANDBOX_NAMESPACE` | Required DNS-label namespace for CMA-owned provider resources |
| `CHRONO_SANDBOX_IMAGE` | Required immutable OCI reference ending in `@sha256:<64 lowercase hex digits>` |
| `CHRONO_SANDBOX_HOME_STORAGE_CLASS` | Durable home StorageClass; configure all four home fields together, or omit all for legacy ephemeral operation |
| `CHRONO_SANDBOX_HOME_CAPACITY_GIB` | Home disk capacity, integer 1-1024 GiB; production 64 |
| `CHRONO_SANDBOX_HOME_TEMPLATE` | Ready, credential-free seed VolumeSnapshot in the Sandbox namespace; initialized with `infra/storage/home-seed.yaml` and `home-template.yaml` |
| `CHRONO_SANDBOX_HOME_TEMPLATE_UID` | Exact immutable Kubernetes UID of the seed snapshot; startup rejects a missing, replaced or unready seed |
| `CHRONO_UPSTREAM_BASE_URL` | Required canonical HTTPS model API base without trailing slash |
| `CHRONO_UPSTREAM_WIRE_API` | Exactly `responses` |
| `CHRONO_PLATFORM_BASE_URL` | Required canonical HTTPS platform proxy origin; also the upstream for the runtime's explicitly granted GitHub API proxy |
| `CHRONO_BASELINE_SERVICES` | Default `chrono-llm-public,ornn-api`; unioned into newly admitted Sandbox scope |
| `CHRONO_SANDBOX_CPU` | Seed default Sandbox Profile size; default `1`, accepts positive cores or millicores |
| `CHRONO_SANDBOX_MEMORY` | Seed default size; default `1Gi`, accepts positive whole Mi/Gi |
| `CHRONO_DEFAULT_AGENT_MODEL` | Seed default Agent model; default `gpt-5.6-sol`, nonblank |
| `CHRONO_DEFAULT_AGENT_EFFORT` | Seed default effort; default `high`, one of `minimal,low,medium,high,x_high` |
| `CHRONO_INSTALL_COMMAND_TIMEOUT_SECONDS` | Per-command bound 1-600; default 600 |
| `CHRONO_INSTALL_TOTAL_TIMEOUT_SECONDS` | Total ordered-command bound, at least command bound and at most 1800; default 1800 |
| `CHRONO_CLONE_DEPTH` | Git clone depth 1-1000; default 50 |
| `CHRONO_CHECKPOINT_BUCKET` | Required external chrono-storage bucket; existing production bucket `chrono-platform-bot` |
| `CHRONO_STORAGE_CLIENT_ID` | Required machine identity for checkpoint storage, distinct from browser OAuth client |
| `CHRONO_STORAGE_CLIENT_SECRET` | Required storage machine-secret reference |
| `CHRONO_ACTIVITY_RETENTION_DAYS` | Activity TTL, 1-3650 days; default 90 |
| `CHRONO_SERVER_RATE_ADMISSION_PER_MINUTE` | Pre-auth network budget/burst; default 6000 |
| `CHRONO_SERVER_RATE_HEALTH_PER_MINUTE` | Separate probe budget, minimum 30; default 120 |
| `CHRONO_SERVER_RATE_DEFAULT_PER_MINUTE` | Ordinary request budget/burst; default 600 |
| `CHRONO_SERVER_RATE_CREATE_PER_MINUTE` | Creation request budget/burst; default 10 |
| `CHRONO_SERVER_RATE_INPUT_PER_MINUTE` | Agent input request budget/burst; default 120 |
| `CHRONO_SERVER_TRUSTED_PROXY_CIDRS` | Exact trusted load-balancer peers; empty by default, production explicit |
| `CHRONO_SERVER_TRUSTED_PROXY_HOPS` | Trusted rightmost forwarded hops, 1-16; default 1 |
| `CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS` | Empty or the complete build-pinned set; must not overlap trusted peer ranges |

`RUST_LOG` controls tracing and defaults to `info`; it is not a `CHRONO_*`
configuration field. Log level changes never authorize credential-value output.
Keep model/service/provider configuration out of ad hoc frontend build flags.

Execution policy is pinned before each effect and participates in its operation
fingerprint. A changed timeout or clone-depth environment value must not change
an already admitted replay plan. Existing Sandboxes likewise keep their pinned
profile and service scope. Startup defaults seed missing default profiles; they
are not a live rewrite mechanism for published profiles or existing settings.

## Bridge rollout switches

See [Channel operational signals and alert definitions](CMA_Channel_Observability.md)
for the existing text log format, durable queue/cleanup/authority observations,
and disabled alert policy definitions. Health observations never authorize replay
or replace retained recovery evidence.

The registered feature switches are independent process-start booleans. Missing values
mean false; only the exact strings `true` and `false` are accepted. A stage also
requires its pinned platform switch. Enabling management does not enable ingress
or workers. Disabling ingress lets previously accepted work drain when its other
stages are enabled. Switches supply availability only; current caller, Bridge,
grant, custody, resource and original-source checks still apply.

Execution disable stops new Channel admission and held-command release. Commands
already Released into an Agent's shared queue drain in original FIFO order;
dispatch-bound commands reconcile their existing effect. Do not skip a retained
Channel head to prioritize another caller, and do not pause the common command
or lifecycle workers. Held reservations can temporarily consume shared capacity
until their original deadline or a proven revocation. Immediate clearance needs
an explicit audited cancellation operation, not a flag-induced silent reorder.

Outbound disable preserves exact tool receipts, pending order, original
custody/destination, deadlines and attempts. Recover the original receipt before
refusing a new tool intent. In-flight sends may finish recording; expired Sending
attempts become Unknown through local reconciliation. Reenabling never supplies
retry consent or resets an attempt, deadline, source identity or reply token.
Unknown delivery still requires the explicit recovery command and its duplicate
risk acknowledgement. Local expiry and outcome bookkeeping must stay available.

Retain accepted-input disposition evidence independently of payload TTL. An
input paused past its original admission deadline expires truthfully; it must not
silently disappear or obtain a fresh deadline on reenabling. An already accepted
or uncertain browser submission retains its protocol, exact body and idempotency
key even when v2 UI transport eligibility changes. The existing public v2 API
continues serving SDK callers.

A rollout is globally disabled only after every old replica has stopped starting
new work and drained its in-flight effects. A process environment change is not
an instantaneous global cancellation. No runtime image rotation, live Agent pause,
provider-role expansion, database reset or retained-resource rewrite is required
to change these control-plane switches.

Channel-created active Sandboxes renew their runtime credential through a
separate bounded scheduler. Each step verifies the original retained human
control-plane grant, current selected-key custody, original allocation and
current resource identity. Renewal replaces the key on the same runtime and
revokes the previous key after replacement; it does not create another Agent or
conversation. Renewal audit records retain the actual human actor and hashed
grant identity without credentials. An unknown mint result stays held and must
not cause a second mint request.

This maintenance remains available when new-admission switches are disabled.
The renewal lease prevents concurrent Sandbox lifecycle changes while accepted
Agent commands continue through their shared queue. A retained Ready allocation
can renew after reset only under the original resource identity and the current
`DrainOriginal` policy. `SuspendOriginal`, removed or replaced human consent,
and retired resources block renewal. Ordinary owner-token issuance never
substitutes for missing Channel authority.

Dedicated Channel revival and retirement use their retained original-human,
allocation, resource and runtime authority. Renewal alone does not authorize
restore or teardown. Fresh retirement can remove its exact owned runtime and
PVC through existing namespaced authority, then publish Retired with sealed,
retained history. Missing or ambiguous original receipts hold finalization.
Underlying disk reclamation belongs to the storage provider; CMA requests no
additional Kubernetes or cloud permissions. Image rotation remains held. See
[Channel lifecycle recovery](CMA_Channel_Lifecycle_Recovery.md) for the implemented
paths and their remaining verification boundaries.

## Platform Settings

Settings are durable and revision-checked through `/api/v1/admin/settings`.
The Operations UI calls the same public API. Read first, retain its revision and
submit `expected_revision`; reconcile conflicts without automatic overwrite.

| Section | Shipped defaults |
|---|---|
| `defaults` | `sbp_default`, `wkp_default`, `agp_default`, each revision 1 |
| `inactivity.pause_after_seconds` | 900; valid range 60-86400 |
| `retention.checkpoint_grace_days` | 7 |
| `retention.agent_log_days` | 90 after terminal state, subject to integrity rules below |
| `limits.max_sandboxes_per_user` | 3 |
| `limits.visitor_max_sandboxes` | 1 |
| `limits.visitor_max_launches_per_day` | 5 |
| `triggers.enabled` | false |
| `triggers.orchestration_paused` | false |
| `marketplace.enabled` | true |

Enabling Triggers permits new public use. Orchestration pause freezes admitted
progress rather than manufacturing a failed Launch. Inventory/retirement,
settings, activity and runtime reads each require their operator capability.
Do not make an administrator's ability depend on an unverified frontend role.

Visitor daily charging happens only when idempotency proves a new Launch was
created. Exact replay and later reviewed Start do not consume another unit.
Exhaustion is `resource_quota` with the next UTC reset time. Sandbox counts are
transactional, so concurrent Launches cannot bypass the visitor's one-Sandbox
limit. The request and visitor daily counters are process-local: production
must remain **one API replica**, with Recreate rollout, until a durable atomic
ledger replaces them. Restart resets of process-local counters are a known
operational limitation, not proof of a distributed quota.

## Deployment continuity and historical reset procedure

The migration-era reset and direct-deployment procedure is revoked for milestone
17 by [#850](https://github.com/ChronoAIProject/cma/issues/850). It provides no
current authority to reset a database, rename live collections, remove owner
Sandboxes, delete provider inventory or adopt a replacement runtime for an
existing Agent. Deployment requires the completed continuity evidence and a
compatible rollback floor described at the start of this document.

Health becoming Ready is not proof of conversation continuity, checkpoint
restoration, baseline visitor access or correct embed behavior. The API pod's
termination grace must exceed its bounded HTTP and worker drains; review it
whenever those timeouts change. Preserve accepted operations, immutable creation
receipts, native conversations, checkpoints and unresolved cleanup authority.
An expired client deadline or stopped worker does not prove an earlier provider
effect can no longer finish. No ordinary rollout may delete owner Sandboxes to
conceal a recovery defect.

### Historical collection evidence

The September 7 inventory and isolated migration rehearsal remain historical
evidence only. They do not establish the current production inventory or permit
repeating collection renames, index removal or resets. The local ignored
`archived_legacy_collections_preserve_custody_and_allow_full_new_store_bootstrap`
test exercises initialization beside archived records and the prior event-index
boundary in an owned disposable database.

Retain historical encrypted custody with its original encryption key and
deployment identity, unresolved revocation debts, session tombstones and exact
legacy object-key ownership until separately verified settlement. A rejected
revocation is not successful cleanup. Historical view definitions and index
metadata remain part of restoration evidence; a generic backup does not prove
that the original namespace, indexes and native conversations can be recovered.

Never start an incompatible historical API against the live database to settle
old debts. Its workers could reopen namespaces or run renewal, checkpoint and
reconciliation against resources they cannot safely interpret. Any legacy
cleanup requires a separately reviewed scope and exact retained authority;
normal CMA-owned key revocation does not authorize NyxID role, account, service
configuration or administrative changes.

### Existing-Owner Verification

The maintainer currently supplies one existing NyxID account. Its local CLI
login can authenticate the real CMA authorization-code flow: preserve CMA's
state, initiating-browser cookie and PKCE challenge, send the CLI access token
only to the exact trusted NyxID authorization origin, and let CMA's callback
establish its own renewable custody. Do not inject CLI refresh tokens into
MongoDB or treat a bearer-only `/auth/session` result as browser sign-in proof.
Keep any browser storage state and credential sentinels in regular `0600` files
outside the repository. Never place credentials in process arguments or reports.

`web/e2e/owner-config.ts` is a separate opt-in entry point with exactly one
`OWNER-resource-lifecycle` scenario. It creates one Sandbox, Workspace and Agent
from reviewed owner-published revisions, observes a completed first answer,
checks browser return and ordinary pause/resume, then submits exactly one public
AG-UI continuation. The second completed answer must recall the first-turn
marker and reported Worktree directory. Exact first-turn history and the
Agent/Workspace/Sandbox/Worktree identities must remain unchanged. Another
browser reopen must retain both durable turns without another create, run,
Pause or Resume command. Cleanup touches only resources recorded in its
owner-bound journal. It does not use operator fault controls.
Ambiguous creation retains cleanup ownership instead of deleting possible parents.

AG-UI `runId` is correlation, not idempotency; the harness refuses every repeat
submission. The finite POST stream is inspected in memory through the SDK frame
decoder, with an 8 MiB total bound, the existing frame limits and a 180-second
deadline. Raw SSE is not persisted. Local transport and guard tests do not prove
that the deployed model continues successfully; that requires executing the
remote scenario against the independently verified deployment.

Supply `CMA_OWNER_E2E_TARGET` and `CMA_OWNER_E2E_MUTATION_TARGET` as the same exact
HTTPS origin, `CMA_OWNER_E2E_SUBJECT`, `CMA_OWNER_E2E_FIXTURE_FILE`, and a fresh
`CMA_OWNER_E2E_OUTPUT_DIR`. The private fixture contains the same origin/subject,
browser storage-state and forbidden-values paths, exact Sandbox/Workspace/Agent
profile IDs, revisions and digests, plus CMA/NyxID commit and image-digest
provenance. Set `CMA_E2E_BROWSER_PATH` when using an installed browser. From
`web/`, discovery requires no remote inputs:

```sh
./node_modules/.bin/playwright test --config e2e/owner-config.ts --list
```

Removing `--list` executes the explicitly configured remote run. Its evidence
is labelled `existing_owner`, one account, and `owner_only_partial`; supplied
provenance remains operator-declared until independently verified. This does
not advance the full J/G campaign. Fresh signup, restricted-visitor behavior,
cross-owner isolation and second-owner forks remain unproved without another
account. The lifecycle baseline also does not prove runtime rotation, restart
fault recovery, external storage cleanup,
provider login, real embeds, or public readiness. Existing full-campaign inputs
and gates remain unchanged.

## Checkpoints And Cleanup

The external storage service is reached through the platform machine-credential
flow and `/api/v1/proxy/s/chrono-storage-service`. The existing bucket is retained
under its new checkpoint configuration name. MongoDB owns checkpoint manifests,
writer fences, publication authority and deletion intents; external objects own
the bytes. GridFS is not an acceptable substitute for the required external
persistence boundary.

Archives above 32 MiB use bounded 32 MiB physical objects through that same
authenticated storage route. The Mongo stage's opaque key records the versioned
part layout and total size before any upload; cleanup can therefore enumerate
every part after an interrupted upload, even if no part manifest was published.
A bounded manifest is uploaded only after every part is readable and both the
part digests and whole-archive digest match. Restore verifies the manifest,
each part and the complete archive. Existing single-object checkpoints remain
readable. Do not roll back to an API that cannot read the part layout after a
multipart checkpoint is published. This avoids the production ingress's HTTP
413 limit without changing runtime archive bytes or writer-fence authority.

New upload stages record a fixed 1,200-second (20-minute) client transfer budget
in `cleanup_after`, increased from 600 seconds after a still-progressing
6.26 GB upload exhausted that shorter budget. This is a CMA-owned bound, not an
expiry sent to the external storage service. Existing stages keep their exact
recorded deadlines across upgrades; never extend a live stage by editing its
record. Transfer cancellation and cleanup both use the stored deadline.

The 180-second per-request timeout, four concurrent 32 MiB parts, three bounded
immutable-part attempts, exact readback checks and Retry-After handling remain
unchanged. Storage credentials are refreshed between requests when needed.
Both periodic and explicit lifecycle captures still have a 1,800-second total
bound, including archive creation and publication, so a 20-minute upload budget
leaves less than ten minutes for the remaining work and does not guarantee every
large home can finish. Periodic capture claims renew every minute; the explicit
lifecycle worker retains its configured 31-minute lease. Timeout preserves
writer-recovery and cleanup authority rather than claiming publication.

Publish only after bounded upload, size/digest verification and readable bytes.
Use immutable physical object identities so cleanup of a losing upload cannot
delete a concurrent winner. Retention preserves the two recorded checkpoints
and does not remove an active recovery reference. Record deletion intent in the
same transaction that removes publication authority, then perform bounded HTTP
cleanup after commit. Never keep a Mongo transaction open around external HTTP.

A DELETE followed by GET 404 is necessary but insufficient while an earlier
ambiguous upload can still finish. The client upload deadline is not a remote
write-lifetime guarantee. Retain minimal `checkpoint_archive_tombstones`
indefinitely for uncertain stages, containing only the exact opaque physical
key, schema version and bounded retry state. Confirmed absence permits Sandbox
purge to finish, but never relinquishes cleanup ownership of that key. Recheck
with exponential backoff from one minute to one hour, in bounded pages, so a
write arriving after any client deadline is still reclaimable. Restart must
resume this cleanup; timeout or lost response must not silently erase the only
record of an orphan. No tombstone contains credential fields. Final external
storage and delayed-upload cleanup evidence remains a required deployment gate.

## Rollback And Alerts

Pause orchestration and new public enablement before investigating a systemic
admission/recovery defect. Preserve existing resources and histories. Prefer a
forward fix built from the completed development branch. Reverting the web image
alone is safe only if its public contract still matches the API. Do not roll an
old Session-era binary onto the new database, or an incompatible runtime onto
new manifests. Restoring a pre-reset backup is a separate destructive epoch
transition, never an ordinary rollback that preserves post-reset work.

Alert on sustained readiness/store failures, reconciliation timeouts, unresolved
credential revocations, checkpoint publication/cleanup backlog, rejected
integrity/fence checks, first-message recovery-required state, repeated runtime
absence, capacity failures, abnormal HTTP 429/503 rates and inactive baseline
services. Record object/operation/request ids and failure classes, not secrets.
Do not convert observation timeouts into permanent resource failure.

Inspect operator inventories and durable event heads first. Do not repair a log
by deleting one event, relabeling a checkpoint, editing a lease or changing an
execution fingerprint. A worker may resume only from its recorded authority.

## Restart Drills

Run on disposable owned resources, then repeat the required production proof.
Keep a dirty Worktree, a home-local installed program and two distinct Agents
with identifiable durable conversations. Capture ids, checksums, image digests,
provider generations and event heads before each fault.

| Fault / Boundary | Required result |
|---|---|
| Restart during `pausing` | Resume the recorded checkpoint/fence operation; no lost home data |
| Restart during `reviving` | Restore the same hierarchy and exact Agent conversations |
| Runtime image default changes | Retain existing runtimes, both Agents, dirty Worktree and installed software; new allocations use the new image, and rollback preserves both cohorts |
| Install failure/timeout | Typed failed step; no credential or proxy activated early |
| Clone failure / Worktree conflict | Recorded failure without duplicate Workspace/Worktree |
| Agent start timeout | Named unavailable/failed state, no guessed readiness |
| First-message ambiguity | Recovery-required card and no blind duplicate dispatch |
| Worker crash between steps | Reclaim its durable operation under a new fence |
| Browser closed mid-Launch | Reopen the owned Launch and resume retained progress |
| Popup blocked / same-tab return | Supported handoff path, no credential stored and no implicit Start |
| NyxID unavailable for requirements | Unavailable state, not satisfied services or new work |
| Ambiguous/late checkpoint upload | Retained cleanup intent, no leaked winner deletion, verified eventual absence |

## Agent Log Capacity


CMA validates an agent's complete durable log before accepting a mutation. To
keep that validation within a bounded MongoDB snapshot, normal log writes stop
at either 10,000 entries or 64 MiB of exact encoded BSON, whichever is reached
first. The permanent store error is `agent_log_capacity_exceeded`. Existing log
pages remain readable, and the owner can still stop and delete the agent; start
a new agent for further conversation.

Lifecycle publication has two internal reserves that normal conversation and
title writes cannot consume. A stop or startup-failure transition may use at
most another 64 entries or 512 KiB, and deletion may use a final 64 entries or
512 KiB. These staged ceilings ensure a full stop reserve cannot strand the
required deletion record. Each atomic append is separately limited to 64 log
entries so one transaction cannot materialise an unbounded command batch.

`retention.agent_log_days` is the maximum time logs are retained after their
agent becomes terminal; it does not increase these per-agent capacity limits.
Never delete individual `agent_log` rows to reclaim capacity. A missing row or
an entry above the aggregate high-water mark is treated as durable corruption.

After an ambiguous Mongo commit, CMA reconciles the complete authority snapshot
at the commit session's operation time. The request fails closed as unavailable
if reconciliation has not completed within 30 seconds. Because cancelling an
in-flight Mongo driver future is unsafe, that read-only check finishes in the
background; at most 16 such checks may remain pending per API process.

## Release Evidence

### Browser Harness Prerequisites

The staging harness lives in `web/e2e/`. Run commands from `web/` after `npm ci`:
`npm run e2e:typecheck`, `npm run e2e:mechanics` for local browser mechanics, and
`npm run e2e` for the configured remote target. The default remote run is only
three read-only smoke cases. Listing cases with `npm run e2e -- --list` executes
no journey. No staging fixture or credential is supplied by the repository.

Read-only smoke requires `CMA_E2E_BASE_URL` (one exact HTTPS origin),
`CMA_E2E_PUBLIC_ID` (an existing published Trigger), and `CMA_E2E_OUTPUT_DIR`
(a fresh absolute evidence directory outside the checkout, under an existing
parent). Existing leaves and symlinks are refused. Use
`CMA_E2E_BROWSER_PATH` when selecting an already installed browser. Mutation
journeys additionally require `CMA_E2E_MODE=mutation`,
`CMA_E2E_MUTATION_TARGET` exactly equal to the origin, `CMA_E2E_FIXTURE_OWNER`,
and `CMA_E2E_FIXTURE_FILE`. The manifest must match that origin and owner.
It and every referenced auth state, hook and credential-sentinel file must be
private regular files outside the checkout, with mode `0600`; never commit them
or reference a symlink into the repository.

The manifest contract in `web/e2e/environment.ts` requires separate owner and
second-owner identities, a distinct visitor identity for each exported visitor
case, baseline services, exact immutable profile revisions, all required Trigger
states, OAuth/API-key/device-code connections, controlled fault fixtures, and
four real embed hosts (Markdown, HTML, JavaScript and React). It also records
the declared CMA/NyxID commits and immutable API/web/runtime image references.
The external hooks must actually complete ordinary signup/connection browser
flows, arm/release the specified owned fault, and observe the required runtime
state. Placeholder success hooks are not evidence. Missing prerequisites fail
the campaign; they must not become skipped cases reported as completed.

Preflight reads the supplied identities and capabilities before any mutation.
Visitors need Launch/use-own/delete-own capabilities without direct Sandbox
creation, profile management or Trigger publication; owner fixtures need the
relevant creation/publication capabilities. Provision an explicitly authorized
disposable campaign with sufficient capacity: J1's two live always-new Sandboxes
cannot pass under the default visitor one-Sandbox limit. Do not silently loosen
quotas or substitute an administrator for a fresh visitor. Signup account cleanup
remains an external operator responsibility; the harness never calls NyxID
administrative account/role handlers.

J0 verifies the newly registered account has exactly `launch_triggers`,
`use_own_sandboxes` and `delete_own_sandbox`, returns to the original placement,
and receives a completed first assistant answer without another CMA command.
J5 requires the pinned Workspace fixture to identify a verifiable public Git
repository, a live Ornn catalogue with a published skill release, readable
deployment resource bounds, and a service catalogue containing a declared
baseline service. It verifies the repository before publication, uses the live
skill/resource/service controls, and forks both Agent and Sandbox Profiles as
the second owner while checking provenance and copied content. J6 needs capacity
for three Agents: two for the existing home/rotation proof and one admitted while
Paused through normal creation. That third creation deliberately loses only the
browser response, retains the real admission in the cleanup ledger, retries the
same key/body, then reloads without an explicit revive or another input command.
These additions are campaign requirements, not remote execution evidence.

The owner-bound append-only `cleanup.ndjson` ledger contains only resources
actually created by this campaign, including late child resolutions from its
admitted Launches. Reused resources are not cleanup-owned. Cleanup stops Agents
before deletion, retires published objects, verifies terminal state/absence, and
keeps polling a cancelled Launch even after a cancellation conflict. A started
Launch with unresolved children is not safe to clean merely because it reports
`cancelled` or `failed_terminal`; retain the ledger for reconciliation and do not
delete possible parents while creation authority remains ambiguous.

Traces, HAR, video, default screenshots and Playwright raw DOM error contexts
are disabled. Only guarded canonical CMA-page screenshots and curated reports
may enter evidence; check DOM, browser storage, password fields and configured
credential sentinels before capture. Keep temporary raw runner output outside
the checkout and out of published artifacts. The custom report contains static
case names, status, duration and safe prerequisite failures, not assertion dumps.

The scheduled workflow is GET-only. Mutation execution is manual-only in the
protected `staging-e2e-mutation` environment, on the isolated labelled self-hosted
runner with externally provisioned fixtures and trusted `develop` checkout.
Adding the harness or enabling a job does not authorize a staging campaign.
Preflight creates the evidence leaf exclusively with mode `0700` and marks it
with a fresh run identity only after prerequisite validation succeeds. The
reporter checks that identity before writing; artifact upload requires the
current harness step's validated-setup output and lists only curated filenames.
A failed preflight cannot authorize uploading an old output directory.

### Evidence Classification

Local mechanics checks use intercepted browser HTML and prove harness behavior,
not staging journeys. Shared backend/SDK corpus replay proves deterministic
public contract composition, not a live NyxID, kernel or storage deployment.

On 2026-09-07, the built-UI local profile-authoring checks passed 3/3 at
1440, 390 and 320 pixels. They exercised skill/service unavailability and retry,
preserved draft values and skill pins, delayed/unavailable resource bounds, and
repository stale-result rejection, typed failure, retry and canonical URL
preservation. The separate local startup checks passed 5/5: Workspace and Agent
creation from both Paused and Reviving parents, lost-response same-key/body retry,
accepted reload without another create or manual revival, and durable sign-in
notice replay/clearing while preserving unrelated first-message recovery without
a fake turn or browser write. Startup captures cover both themes at all three
widths, including the unsettled dialog and accepted/replayed states.

Run these local suites from `web/` against an existing production-build loopback
preview (default `http://127.0.0.1:4342`) with Node 26.8.1 and an installed browser:

```sh
npm run build
npx playwright test --config e2e/profile-authoring.config.ts
npx playwright test --config e2e/agent-startup.config.ts
```

Use `CMA_AUTHORING_PREVIEW_URL` or `CMA_STARTUP_PREVIEW_URL` to select a different
loopback preview, and `CMA_E2E_BROWSER_PATH` to select the browser executable.
Each suite writes screenshots and structured local-boundary attachments to its
own fresh OS temporary directory (`cma-profile-authoring-browser-*` or
`cma-agent-startup-browser-*`), outside the checkout. HTTP responses are
intercepted local fixtures, including recorded backend response shapes; the
sign-in destination is an inert example. No provider login, remote creation,
actual repository resolution, runtime revival or remote failure is executed by
these checks. They do not advance any remote gate or journey in the tables below.

The local credential-setup browser checks also passed 3/3 on 2026-09-07 via
`npx playwright test --config e2e/credential-setup.config.ts` from `web/`.
`CMA_CREDENTIAL_PREVIEW_URL` selects the loopback preview, with the same default
and browser-executable option as the suites above. All three surfaces (Sandbox
creation, Trigger landing, and draft continuation) preserve the current tab and
choices after a typed `sandbox_credential_setup_required` refusal. They open the
existing CMA sign-in URL in a separate window, handle a blocked window, reject
a different account on recheck, and require an explicit fresh-key retry after a
same-account check. Each later dispatch rechecks the live session against the
original account, including ambiguous same-key retries; account changes block
the mutation without discarding its key/body. This check does not atomically
bind a later HTTP request to that session. A later lost response keeps that new
command's original key/body. Trigger recovery retains the original repeat
choice, placement and referrer through reload. Pending connection completion
cannot replace the explicit Continue required after Draft recovery. Reload
restores that per-Launch retry requirement, its original account, and any
unresolved Start key/body; it does not admit another command. Eighteen screenshots cover
both themes at 1440, 390 and 320 pixels in a fresh
`cma-credential-setup-browser-*` OS temporary directory.

This suite intercepts sign-in navigation and identity reads locally. A matching
session confirms only account identity, not renewable credential readiness;
the next public admission may refuse again. It executes no NyxID login,
credential minting, remote Sandbox provisioning or remote credential mutation.

The official Node/Python Responses client checks use the real loopback CMA
composition with scripted runtime input and in-memory persistence. Their pinned
client versions and exact verification boundary are recorded in
[API_Responses.md](./API_Responses.md#6-official-sdk-conformance); they are not
production runtime evidence.
TypeScript replay labels each case by its actual boundary: `wire` calls the
typed operation and checks the emitted request against the recorded request;
`preflight-validation` proves local rejection without a network call;
`serializer-protection` proves that typed serialization prevents the malformed
request; `browser-only` checks navigation URL construction or the server-owned
callback composition. Protected malformed requests and OAuth callbacks are not
SDK HTTP round trips. Their recorded problem documents are checked separately.
`raw-wire-only` body-limit probes use bounded compact byte expansion in the
backend corpus. They cannot be emitted by the SDK's structured serializer and
prove only recorded error decoding on the SDK side, not a typed HTTP replay.
Rust replay makes the same distinction between actual typed HTTP, SDK preflight,
unrepresentable wire/type shapes and browser-owned navigation. Deserialization
failure while constructing a typed fixture is not SDK validation. Both kinds of
local refusal independently check the recorded problem decoder without claiming
that the protected malformed request was sent over HTTP.

Backend fault scenarios exercise real public handlers and services with selected
Core ports returning source-backed typed failures. They prove the public error
projection, not that an external outage or an impractically large model boundary
was reproduced. For example, a publish revision-overflow port failure does not
claim construction of billions of retained Trigger revisions. The inventory
separates HTTP problems from protocol failure outcomes; error-only streams do
not satisfy successful-stream coverage. A retrieved failed Response is a valid
resource projection, not an HTTP refusal. Propagated runtime failure classes are
open strings, so their fixtures are representative protocol examples, not an
exhaustive catalog of provider errors. Fixed lifecycle stop reasons remain
separately enumerated in the protocol inventory.

Remote harness reports identify `executionTarget: configured_remote_origin` and
`mocked: false`, but supplied commit/image pins remain
`provenanceSource: operator_declared` and `provenanceVerified: false` until
independent deployment evidence verifies them. Smoke reports supply no pins.
Do not relabel declared provenance as remotely observed identity. The deliberate
browser transport-response-loss case is local fault injection; provider failures,
restart/rotation and runtime observations require their real audited hooks.

No remote campaign has been run by adding this harness. G-A1-G-A5 still require
independent proof against the final pinned kernel/runtime; no browser pass can
establish prompt order, skill hot reload or exact conversation restore alone.

### Required Final Gates

Status as of 2026-09-07: local implementation and deterministic/browser checks
are progressing. **No final production reset, deployment or public-readiness
claim is recorded here.** Historical experiments must remain labelled with their
original image and environment. Update this register with actual observations,
not a prediction that a passing unit test will reproduce in production.

Every evidence entry must include: gate/journey id; timestamp and environment;
CMA commit; API/web/runtime immutable digests; NyxID/provider version or commit;
redacted identity class; prerequisites; exact actions; observed result; report
and screenshot locations; request/resource ids; cleanup; remaining limitations.
Keep raw credentials and real handoff URLs out of reports.

| Gate | Final required evidence | Current final status |
|---|---|---|
| G-A1-G-A5 | Distinct Agent homes, prompt order, actual skill hot reload, shared auth/model access, exact conversation capture/restore | Pending on final contract-3 digest; historical section 8 evidence is not final |
| G-S1 | Install with proxy socket/credential absent; installed home software survives pause/revive | Pending final image/production proof |
| G-S2 | Fresh default visitor with baseline-only profile reaches ready Agent | Pending existing NyxID prerequisite and production proof |
| G-P1 | Restart during pausing/reviving and two-Agent dirty-home rotation | Pending final image/production proof |
| G-T1 | README/camo, HTML, strict-CSP script and React click-through with attribution | Local publisher fixtures/browser checks; real hosts pending |
| G-T2 | OAuth, API-key and device-code connection assist completion | Deterministic adapter/UI checks; real provider journeys pending |
| G-T3 | 50 visitor clicks admit at most 5 new Launches with typed reset | Local quota checks; production burst pending |
| Storage | External streamed bytes, integrity, ambiguity, retention and delayed-upload cleanup | Adapter integration and final provider proof pending |

| Journey | Required observation | Final status |
|---|---|---|
| J0 | Account creation from landing returns to same Trigger and ready Agent, one explicit pre-sign-in click, zero post-sign-in clicks | Pending |
| J1 | Always-new Sandbox path | Pending |
| J2 | Reused Sandbox with new Workspace | Pending |
| J3 | Reuse both; Agent ask/continue/new and resume-existing branches | Pending |
| J4 | Missing service, connection assistant, then explicit continuation | Pending |
| J5 | Publish profile, second-owner fork, Trigger wizard, embed and stats | Pending |
| J6 | Public-profile trust, new Sandbox/Workspace, two Agents, stop/delete, pause/revive | Pending |
| J7 | Retired Trigger and paused orchestration freezes rather than fails | Pending |

Security evidence must additionally prove: no credential in browser storage,
DOM or captured bodies; badge sets no cookie; cross-site form POST refused;
visitor cannot directly create a Sandbox, manage Profiles or publish Triggers;
foreign resource/handoff reads concealed; no leak through errors or telemetry.
Capture landing, trust review, connection assistant, progress and Agent views
in both themes and at desktop/compact widths.

The portable public, publisher and operator browser proofs under `web/src/`
record mocked-public-boundary rendering, routing, accessibility and request
contracts. Their screenshots are valuable local evidence but are not real
NyxID, provider, checkpoint or production journey passes. Backend, SDK, secret,
image, manifest and structure gates all remain mandatory before delivery.

### Explicit Sandbox command scheduling

The API process starts the durable Sandbox command scheduler under its existing
shutdown coordinator. It polls once per second, discovers at most 50 queued
records per page, and runs at most eight commands concurrently. A blocked page
retains its cursor progression so later commands are discoverable. Local duplicate
work is suppressed; competing processes are fenced by the exact queued record and
atomic Sandbox revision/authority checks. Shutdown allows 25 seconds for active
work to drain; remaining tasks are cancelled and durable records remain recoverable.
The original five-minute deadline is never renewed. Transient provider/store
failures retry through discovery; terminal rejection does not schedule runtime IO.
Applied receipts describe durable transitions, whose runtime work is completed
by the existing lifecycle recovery loop. Public reads and same-key retries recover
lost acknowledgments without adding another transition.

### Bot lifecycle records

The `bots` collection stores owner-scoped creation receipts, immutable profile
revisions, reserved channel-record UUIDs, external resource IDs, credential hashes
and worker progress. Credential bytes are not part of this aggregate. Writes and
reads require majority acknowledgement. A unique `(owner, creation_key_hash)`
index prevents concurrent requests with one creation key from admitting two Bots;
changed request content conflicts. Deleted records retain this receipt so a late
retry cannot silently recreate external resources.

The selected Agent Profile revision is pinned with the current published Sandbox
and Workspace defaults. Agent profiles do not reference parent environments in
the CMA model. Later default or draft changes cannot rewrite an admitted Bot.
Every reference must be readable by the owner and must identify a published
revision. An omitted Agent model pins the deployment's configured default model.

Bot commands and worker claims share the aggregate's revision fence. A claim
expires after 120 seconds; successful checkpoints renew it. An abandoned create
is reclaimed as cleanup work because its transient key values cannot be recovered
from the Bot record. Cleanup must resolve any reserved external resource before
completing. Pending mutations block incompatible commands, and expired writers
cannot save progress over a replacement worker. `cma:bot:manage` maps to the
owner-only `ManageOwnBots` capability; it grants no binding or gateway record
management authority.

The core worker checkpoints create, policy changes, rotation and deletion before
external effects. A failed create remains durable cleanup work, including a
120-second settling interval before discovery of uncertain key, channel and route
creations. Cleanup verifies absence before completing; dependency failures retain
retryable work. Rotation reconciles both the retained provider binding and route
before revoking obsolete keys, including when a provider accepted a replacement
before the channel record write failed. Its external execution port requires
bounded calls, complete owner-scoped discovery and idempotent deletion.

The production adapter, worker scheduling, renewal, activity audit, public Bot
routes and both Bot SDKs compose this foundation. Local interoperability tests
run the compiled gateway through creation, restart recovery, policy updates,
rotation and cleanup with fake NyxID and provider dependencies. The adapter uses
the scoped NyxID tools service and accepts CMA's explicit refusal of the optional
native MCP endpoint; refusals of requested profile fields fail creation and
retain durable cleanup. These checks do not establish real Telegram delivery or
authorize retiring the existing bridge.

Direct agent-sandbox runtimes receive `CHRONO_RUNTIME_CONTROL_TOKEN` at bootstrap.
The supervisor consumes it before preparing child environment maps and requires
it on runtime control, checkpoint and WebSocket endpoints. Existing provider
runtimes retain their proxy authentication. This change requires building the
matching runtime image before selecting the direct backend.


## Three-service local implementation and rollout prerequisites

The web now has Event Gateway, Agent Manager and Image Manager service tabs.
`/api/event-gateway/*` and `/api/image-manager/*` strip their prefixes to the
independent services; `/api/agent-manager/*` maps to AM's existing `/api/*`.
Existing Agent URLs, cookies and public REST/Responses endpoints remain valid.
Both the managed Gateway HTTPRoute and nginx fallback implement this routing.

Image Manager uses Cloud Build for an immutable public GitHub commit and pushes
to `asia-northeast1-docker.pkg.dev/chronoai-501608/chronoai-registry`.
Its GKE identity needs Cloud Build submission/read, Logging read, and permission
to act as the dedicated build service account. That build account needs only the
build's required logs/source and repository push permissions. Populate
`CMA_IM_BUILDER_IMAGE` with a tested `gcr.io/cloud-builders/docker@sha256:...`.
Private repository credential delivery and physical registry garbage collection
are not implemented. Retirement is logical and retains images for live Agents.
The IM manifest stays at zero replicas with an unreleased image until a real
image digest, configured identities and migrated database are available.

All services use separate Mongo credentials. Image Manager requires a replica
set because build admission and the retirement fence share a transaction.
Generate exact roles with each binary's `--print-runtime-role`; the image and
gateway migrations run explicitly with their separate migration credentials.
Existing AM stores retain their own startup schema/index initialization within
the AM collection allowlist. Broad existing `readWrite`/admin credentials are
rejected at startup and must be replaced before deployment.

Deploy the adapter-capable AM first with `CHRONO_SANDBOX_PROVIDER=opensandbox` and
`CHRONO_RETAIN_OPENSANDBOX=true`. It records existing generation choices in
`runtime_backend_routes`. After all old workers are replaced and retained
requests have durable routing, enable agent-sandbox for new generations. A route
is written before external creation and never changes when the default changes.
Existing runtime handles continue to use their original provider. A first
cutover containing retained unbound Sandboxes without routing is refused, rather
than replaying uncertain creation against a different provider. Fresh empty AM
databases can enable agent-sandbox immediately. Rollback must retain the backend
for every persisted route; do not disable OpenSandbox while its rows remain.

Before routing live Gateway traffic to its new database, copy the legacy sealed
records with an operator-owned migration identity, preserving record ID, owner,
blob and version exactly. Keep the stable master keys and canonical legacy
pointer origin. Quiesce channel *management mutations* during this authority
transfer, reconcile counts/versions/digests, then switch Gateway DB credentials.
The runtime service must not retain access to the legacy AM collection. This
local change does not perform the production data transfer or any rollout.

agent-sandbox integration targets `agents.x-k8s.io/v1beta1` (upstream v1.0.6).
Infra owns CRDs/controller; AM submits Sandbox objects with explicit CPU/memory,
pinned runtime images and owned existing PVCs. Destructive operations use UID and
resourceVersion preconditions. Controller-created Pods are discovered through
CMA labels plus exact Sandbox controller UID, never by a namespace-wide cleanup.
Lifecycle pause retains the existing writer-fenced checkpoint/teardown flow;
the adapter also implements upstream Suspended mode, but the application does
not bypass its conversation/checkpoint protocol to use that mode.

### Transfer retained Gateway records

Use a one-time read-only credential for the legacy `gateway_channel_records`
collection to run `.github/scripts/export-gateway-records.js` with `mongosh`.
Select the legacy database in the connection URI and set
`CMA_GATEWAY_EXPORT_FILE` to a new private local file. Quiesce channel management
mutations during export, verification and routing cutover; existing Agent turns
must continue. The exporter creates the file with mode 0600 and prints only the
record count. The service processes never receive that legacy credential.

Run `cmaeg --migrate` with the destination migrator credential, then
`cmaeg --import-legacy-records <export-file>` using that same destination identity.
Import preserves the record UUID, owner, sealed bytes and version, verifies each
insert with majority reads, and supports exact replay. It refuses conflicts and
never overwrites destination records. A partial failure is resumed with the same
file after its cause is repaired. Keep the gateway's original master keys and
canonical CMA base URL so existing callback/channel IDs remain valid. Verify
counts and original callback resolution before enabling management writes, then
remove the temporary export and credentials according to the operator's policy.

Image build dispatch records explicitly distinguish a definitive submission
rejection from an unconfirmed outcome. Unknown Cloud Build POSTs are observed by
their original unique tag and never resubmitted automatically. A crash before
POST may therefore require operator reconciliation; elapsed time alone cannot
prove that no build was accepted. Retirement blocks new builds/resolution but
keeps existing immutable registry blobs for retained Agents.
