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
total deadline. Organization keys that receive 403 use `/oauth/userinfo` to
authenticate their subject and must additionally prove that exact subject through
`GET /api/v1/orgs/{subject}/authorization`; a personal subject cannot use this
fallback. Older profiles without `user_type` also require the organization lookup
to distinguish personal and organization ownership. Only token hashes are cached:
successes for 60 seconds and failures for
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

CMA attaches the native MCP server as `cmaeg`; Codex exposes the four names
under its `mcp__cmaeg__` prefix. This path requires no NyxID tools catalogue
registration. Reply/update REST bodies use `OutboundMessage`; their MCP
arguments wrap that same body in `message`.
A 20 KiB text reply is supported; text over 32 KiB returns 413. Tool HTTP bodies
are limited to 128 KiB. Delivery enqueue and receipt together have an eight-second
deadline; expiry returns 504 `internal` with `delivery_timeout`. A timeout does
not confirm whether delivery happened. Confirmed tool replies mark the exact
reference hash to suppress automatic final delivery. Telegram replies use NyxID's
relay reply endpoint; edits and typing return `unsupported`.

With the default `CMAEG_NYXID_COMPLETE_REPLIES=false`, relay delivery retains
the legacy behavior below. After verifying the NyxID dependency, operators can
select one complete ordinary relay request; see the [complete-reply contract](CMAEG_Protocol.md#10-delivery-semantics)
and [coordinated rollout](CMAEG_Operations.md#coordinated-complete-reply-rollout).
Native Telegram chunking and interactive/edit one-part constraints remain in
effect.

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

In default legacy relay mode, the first send uses an unexpired reply token; later
parts and sends use the retained channel route key. Each route has a
three-second minimum interval on this replica.
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
| `POST /v1/channels/{channel_id}/test` | Owning creator | Run one synthetic provider turn with up to 90 seconds for initialization and explicit Busy refusals, then 30 seconds for acceptance/completion; return acceptance latency, up to 200 Unicode characters and the provider level. Clients should allow 125 seconds. Only explicit Busy refusals retry, using the same input and idempotency key; transport or stream failures are not retried. |
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

The owner completes a channel connection in Event Gateway, then selects its
opaque `connection_ref` from `GET /api/v1/bots/connections`. The Gateway owns
platform credentials and webhook activation; CMA owns the consumer's Agent
configuration and scoped Agent key. Connect the configuration's explicit services
and managed model service before creating the consumer. CMA attaches CMAEG tools
through the binding's native endpoint using that consumer's own Agent key.

`POST /api/v1/bots` accepts `{name,connection_ref,profile:{id,revision},admission,reply}`
and an `Idempotency-Key`. It returns 201 with a durable `creating` resource;
poll its GET route until `active` or `failed`. HTTP disconnects do not cancel
admitted work. Optional `sandbox_profile:{id,revision}` and
`workspace_profile:{id,revision}` select personally owned published environments;
each omitted reference uses its published platform default. All three profile
revisions are pinned at admission and returned by the Bot read projection.
Explicit environment references must belong to the owner, including when public;
foreign public profiles require a personal fork before selection. This preserves
custom resources, repository, installation and setup without changing global defaults. Managed model access is
required. Environment resources, workspace location, setup commands, service
scope, skills, prompt, permissions and thinking effort follow the CMAEG profile
schema; unsupported access is explicitly refused.

Connection consumers mint only their scoped Agent key. Legacy requests may
still supply `nyxid_bot_id` instead of `connection_ref`; those integrations mint
separate 30-day agent and route keys in the exact
`cma:bot:<bot_id>:<agent|route>:<generation>` namespace. The agent key has only the
resolved profile and model service grants. The route key has no service or node
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
The consumer browser journey exercises real Gateway routes and restricted Mongo
credentials in process, with local identity, provider and platform HTTP fixtures.

The dashboard’s **Event Gateway consumers** account-menu entry requires the
`manage_own_bots` browser capability. `/bots/new` selects a ready Gateway
connection, an exact published
Agent Profile revision, optional personally owned Sandbox and Workspace revisions,
explicit message admission and reply settings, then reviews the request. Detail pages show durable setup checkpoints, provider level,
access expiry and the last synthetic test. Connection consumers link to Event
Gateway; legacy integrations also show their callback URL. They expose explicit
enable/disable, key rotation and confirmed deletion. Credential values are never
returned to the dashboard. Deletion is complete only after the accepted operation
finishes and the public resource disappears; existing conversations are retained.

Before sending a creation request, the dashboard retains its exact configuration
and idempotency key in browser session storage, scoped to the origin and signed-in
owner. After an uncertain response or reload, **Recover creation** resubmits that
same request only on an explicit click. Other lifecycle commands are never
automatically repeated after uncertainty. Visible pages poll every five seconds;
stale observations disable actions, temporary read failures retain known data,
and access denial clears protected records.

For local browser verification, install the web dependencies and Playwright
browser, build `web/` and `cmaeg`, then run from `api/`:
`cargo test -p cma dashboard_creates_tests_and_deletes_through_real_cma_and_compiled_gateway -- --ignored --nocapture`.
The browser drives real CMA Bot routes, its durable worker and compiled CMAEG
through creation, synthetic test, disable/enable, rotation and confirmed cleanup.
Identity, profile catalogue and provider execution use loopback fixtures and the
Bot store is in memory. This test proves local integration, not production login,
Telegram delivery, database durability or restart continuity.

### Runtime tool credential transport

The runtime has an optional fixed-endpoint HTTP proxy for provider tool
connections. `CHRONO_TOOL_PROXY_URL` is unset by default. When supplied at runtime
creation it must be one canonical HTTPS URL without credentials, query or
fragment; literal loopback HTTP is accepted for local verification. The proxy
binds only `127.0.0.1:9093` and accepts `POST /`, with a 128 KiB request limit,
a five-second request-body deadline, a ten-second upstream deadline and a
4 MiB response limit. The response bound accommodates full context in both the
structured and escaped text representations of a tool result. Caller URLs
cannot select another destination. Redirects,
encoded responses and responses exposing the injected credential are refused.

Each request uses the runtime's current in-memory Sandbox credential, so a
credential delivery changes subsequent calls without reopening the conversation.
No credential file or additional shared secret is used. Uncertain upstream
results are not retried. This transport alone does not attach tools to an Agent:
binding provenance, retained endpoint configuration and confirmed conversation
setup are checked separately before a provider Agent can become ready.

A provider binding can retain one tool endpoint as part of its immutable supplied
credential authority. Exact replays and key rotation preserve that endpoint;
changing or removing it under the same binding identity is refused. Binding
Sandbox creation projects the retained endpoint into the runtime environment.
Records created before this field existed deserialize with no endpoint, and
ordinary Sandboxes omit the new field and environment variable, preserving their
existing deployment fingerprints and runtime image pins. The public binding
handler accepts native tools only at the exact `/mcp` endpoint derived from the
configured CMAEG public origin. Deployments without gateway configuration retain
the explicit refusal. A different advertised destination is refused before
credential custody; enabling support never changes the destination on an old
binding.

A read-only `tool_proxy_status` supervisor operation proves the exact Sandbox,
provision generation, configured destination and current credential generation.
It succeeds only while the runtime's own listener is serving, and includes a
process boot identity. It also works without a persistent home. Older runtimes
cannot supply this proof and are refused before a tool-enabled Agent opens.

Provider Agent attachment independently checks the durable binding conversation,
creating manager, Agent creation identity, pinned profiles, complete lineage,
current runtime and work authority. `provider_tool_opens` stores successful ACP
opens under an exact creation/runtime/request digest using majority reads and
writes. Confirmation rechecks and fences both lineage and binding authority.
No key bytes enter that collection or the ACP descriptor. Fresh sessions require
HTTP tool capability; loading supplies the local `cmaeg` server again. Rejoining
an initialized process requires its confirmed conversation and matching boot
identity. A requested resume ID alone is insufficient. Credential rotation keeps
the local descriptor and successful-open identity while subsequent calls use the
new key. This source is independent of legacy bridge records.

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

## Retired bridge compatibility

CMAEG owns event ingress, channel configuration and delivery. CMA no longer
composes the legacy Bridge control plane, callback routes, worker pipeline,
credential minting or renewal, reply tools, or Bridge SDK clients. Use owner Bots
and the CMAEG provider binding contract described above.

Persisted provenance, usage and history markers remain readable. Narrow Mongo
compatibility checks retain retirement seals and late-upload references and
prevent ordinary lifecycle operations from acquiring old channel authority.
A historical shared-parent reservation or child-removal journal requires an
explicit data migration before reuse or restore; the new worker cannot execute
an old Bridge cleanup command. Retained records are not automatically purged.

Before replacing an API that ran the legacy pipeline, verify there is no accepted
or uncertain legacy work left to drain. Preserve live runtime identifiers,
conversation state, image pins and all retained history during the rollout.
The old Bridge continuity harness and switches have been retired with that
pipeline. The normal lifecycle, checkpoint, Mongo, provider and runtime contracts
remain required.

## Prerequisites

- Verify the exact cluster context, namespace, deployment id, database and storage class
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
- The home PVC must belong to the exact recorded Sandbox allocation and retain
  its original Kubernetes UID. Verify suspend/resume against that PVC and prove
  CRD and Pod removal before releasing it. CMA no longer requires a checkpoint
  bucket, storage machine identity or archive upload/download/delete access.
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

API CI explicitly restores and saves its two Cargo cache mounts across hosted
runners, extracting them before the required BuildKit prune. See
[`API_Cargo_Cache.md`](API_Cargo_Cache.md) for cache identity, branch isolation,
bounds and the manual fresh-builder validation. Ordinary GHA layer export alone
does not persist these mutable mounts; the first compatible run remains cold.

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

Sandbox lifecycle scheduling permits at most four concurrent sweeps, with
one new sweep offered per second while capacity remains. Durable per-Sandbox
claims and revision fences, not the scheduler's task list, grant authority.
A slow restore therefore does not monopolize all lifecycle work, and a new
pause-cancellation fence can be claimed while its superseded capture unwinds.
Four simultaneous long operations can still consume all slots; this is a
bounded concurrency improvement, not an unbounded latency guarantee.

Workspace sweeps remain serial. Periodic checkpoint maintenance and checkpoint
object cleanup workers are retired. Shutdown stops admitting sweeps, permits a
25-second drain, and cancels remaining child tasks without clearing their durable
leases. The API requests 256 MiB and is limited to 1 GiB; this configuration no
longer reserves buffers for CMA archive transfers.

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
| `CHRONO_V2_UI_TRANSPORT_ENABLED` | Eligibility for the integrated v2 browser transport; false code default, true in the production ConfigMap for new Agent tabs (including durable Starting input). Existing tabs retain their selected transport. Deploy with the matching API/Web release, ConfigMap compare-and-set, and API rollout; does not disable existing public v2 API routes or change an accepted request protocol |
| `CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED` | New authenticated Launch admission selects common first-message commands under retained personal ownership; false by default. Proven pre-write failure is terminal for new common prompts; ambiguous writes retain the same command/response and refuse legacy recovery. Accepted protocol is immutable; disabling the selector retains accepted common commands and legacy launches retain their original delivery/recovery behavior. Keep false until every worker and permitted rollback version recognizes and transactionally excludes common/legacy dispatch; older workers ignore additive sidecars. |
| `CHRONO_TRUSTED_SUBJECT` | Local trusted-mode subject; default `local-developer`, irrelevant in provider mode |
| `CHRONO_IMAGE_MANAGER_BASE_URL` | Image Manager service base URL used for delegated image resolution; defaults to the CMA public origin with `/api/image-manager` when unset |
| `CHRONO_NYXID_BASE_URL` | Required API/OAuth HTTP(S) origin in provider mode |
| `CHRONO_CMAEG_BASE_URL` | CMAEG management HTTP(S) origin; production `http://cma-event-gateway`. Set together with its public origin; omitting both disables Bot routes/workers. |
| `CHRONO_CMAEG_PUBLIC_URL` | Retained CMAEG public HTTP(S) origin accepted for exact legacy callbacks; production `https://bot-gateway.chrono-ai.fun`. |
| `CHRONO_CMAEG_PUBLIC_API_URL` | Canonical CMAEG public API base, defaulting to `CHRONO_CMAEG_PUBLIC_URL`; production `https://bot.chrono-ai.fun/event-gateway`. HTTPS with clean optional path segments, no credentials/query/fragment/trailing slash (loopback HTTP allowed for tests). Returned callback and event endpoints must match one exact canonical or legacy base for the same channel. |
| `CHRONO_NYXID_PUBLIC_URL` | Required exact canonical HTTPS browser origin in provider mode |
| `CHRONO_NYXID_CLIENT_ID` | Provider-issued OAuth client id, not a chosen display name |
| `CHRONO_NYXID_CLIENT_SECRET` | Required OAuth client secret reference |
| `CHRONO_AUTH_ENCRYPTION_KEY` | Secret reference: standard-base64 encoding of exactly 32 random bytes |
| `CHRONO_MONGODB_URI` | Required secret reference, `mongodb://` or `mongodb+srv://` |
| `CHRONO_MONGODB_DATABASE` | Database name; code default `chrono_bot`, production explicitly `chrono-bot` |
| `CHRONO_MONGODB_AUTHORITY_MODE` | `isolated` by default verifies exact collection grants; production `shared-schema` allows the existing database-scoped credential while AM opens only its own collection catalog. Neither mode changes validators, indexes or resource authorization. |
| `CHRONO_DEPLOYMENT_ID` | Required stable Kubernetes-label-compatible CMA ownership identity |
| `CHRONO_SANDBOX_BASE_URL` | Required canonical HTTP(S) provider origin |
| `CHRONO_SANDBOX_API_KEY_FILE` | Required readable nonempty mounted provider key; never a ConfigMap value |
| `CHRONO_SANDBOX_NAMESPACE` | Required DNS-label namespace for CMA-owned provider resources |
| `CHRONO_SANDBOX_IMAGE` | Required immutable OCI reference ending in `@sha256:<64 lowercase hex digits>` |
| `CHRONO_SANDBOX_PROVIDER` | Runtime provider selector: `opensandbox` (default) or `agent-sandbox`; selects the direct agent-sandbox backend for new allocations |
| `CHRONO_RETAIN_OPENSANDBOX` | Whether to retain the legacy OpenSandbox endpoint configuration; `true` by default, and required while existing OpenSandbox runtimes remain addressable |
| `CHRONO_AGENT_SANDBOX_ENABLED` | Optional boolean override for the direct agent-sandbox adapter; defaults to the provider selection |
| `CHRONO_NATIVE_PREPARED_TARGET` | Deployment-owned count of credential-free native Sandbox/Workspace/idle Agent hierarchies: `0` (default, disabled), `1`, or `2`. Requires native allocation and `native_vct_full_home_v2`. A matching ready native pool must exist first. A separate durable collection bounds all preparing, ready and retiring reservations to two across replicas. Explicit default Agent Manager creation atomically takes one matching reservation; other requests use normal creation. Set to `0` to retire unclaimed prepared capacity while preserving owned Sandboxes. |
| `CHRONO_NATIVE_WARM_POOL_TARGET` | Number of unclaimed native agent-sandbox members kept ready for SandboxClaim admission (`0`–`1000`). Defaults to `1` when `CHRONO_SANDBOX_PROVIDER=agent-sandbox`, and `0` for legacy OpenSandbox. Requires a native VCT home. Agent Manager persists this deployment-owned target keyed by the exact runtime image, resources and Pod policy before serving, then reconciles `SandboxTemplate`, `SandboxWarmPool` and `SandboxClaim` resources. Existing targets and claimed Sandboxes retain their original policy. |
| `CHRONO_AGENT_SANDBOX_RUNTIME_TRANSPORT` | `direct` (default) connects to the owned Pod IP on TCP 9090. `kubernetes-portforward` uses authenticated Kubernetes `pods/portforward` streams to port 9090. `kubernetes-exec` uses `pods/exec` and the runtime image's `/usr/bin/python3` to relay stdio to fixed `127.0.0.1:9090`. Both carry control, history and conversation WebSockets with the same pinned CA and renewable credentials as CRD operations. The selected subresource needs `get`/`create` authorization. No local listening port, local subprocess, router or `pods/proxy` permission is used. |
| `CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE` | Required readable secret file when the direct agent-sandbox adapter is enabled; never store the control key in a ConfigMap |
| `CHRONO_AGENT_SANDBOX_RUNTIME_CLASS` | Optional Kubernetes RuntimeClass passed to agent-sandbox Pods |
| `CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT` | Kubernetes ServiceAccount for agent-sandbox Pods; defaults to `cma-runtime` |
| `CHRONO_AGENT_SANDBOX_IMAGE_PULL_SECRETS` | Optional comma-separated Kubernetes image-pull Secret names for agent-sandbox Pods |
| `CHRONO_AGENT_SANDBOX_TEMPLATE_NETWORK_POLICY` | Optional operator-owned JSON object with explicit `ingress` and `egress` arrays using Kubernetes NetworkPolicy rules. A configured object emits Template `networkPolicyManagement: Managed`; the controller owns selectors and the NetworkPolicy object. Maximum 64 KiB and 64 rules per direction; only numeric ports are accepted. An unset or blank setting keeps historical controller defaults and identities. A custom policy replaces both controller defaults; an empty direction array grants no traffic from this policy, while allowances from other selecting policies still apply. Configure and prove both directions before native rollout. |
| `CHRONO_AGENT_SANDBOX_NAMESPACE` | Native CRD/Pod/PVC namespace, independent of legacy `CHRONO_SANDBOX_NAMESPACE`; empty or absent inherits the legacy namespace |
| `CHRONO_AGENT_SANDBOX_API_SERVER` | Explicit remote Kubernetes API, one canonical HTTPS origin without trailing slash. Production Deployment resolves this from the infra-owned `chrono-pod-configmap` ConfigMap `CHRONO_POD_API_SERVER` key. Configure with a CA and the selected authentication mode; empty connection fields in default token-file mode preserve inferred local configuration. |
| `CHRONO_AGENT_SANDBOX_AUTH_MODE` | `google-metadata` in production uses the `chrono-bot-ksa` Workload Identity annotation for short-lived tokens; remote API and CA are required and token-file input must be empty. Default `token-file` requires API, CA and token file together when any connection field is set. Metadata credentials are refreshed in the application without gcloud or kubeconfig plugins. |
| `CHRONO_AGENT_SANDBOX_CA_FILE` | Absolute path to a mounted remote Kubernetes PEM CA bundle; TLS verification is mandatory and kube-rs reloads the bundle for new connections |
| `CHRONO_AGENT_SANDBOX_TOKEN_FILE` | Absolute path to a mounted remote bearer token; kube-rs refreshes file contents approximately every minute. Rotate the token before expiry. The explicit remote client does not read global kubeconfig, execute gcloud/plugins, or change legacy credentials. |
| `CHRONO_AGENT_SANDBOX_HOME_MODE` | `native_vct_full_home_v2` (manifest default) creates controller-owned PVCs persisting all of `/home`, including `/home/.codex`, with the public user seed at `/home/cma`; snapshot fields must stay empty. Explicit `native_vct_empty_v1` preserves the original `/home/cma` mount. Existing allocations retain their persisted layout after the default changes. `legacy_snapshot` preserves the historical four-field snapshot policy. With mode absent, a native storage class/capacity pair selects v2; all four native fields select legacy snapshot; no native fields inherits the legacy policy. Deploy v2 with its new runtime image pin and CMA binary together. |
| `CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS` | Native home StorageClass in the actual runtime cluster. Empty VCT mode needs this and capacity; explicit empty-VCT mode may inherit missing class/capacity from the legacy policy. Set the remote class explicitly before enabling cross-cluster native allocation. |
| `CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB` | Native home capacity, canonical integer 1-1024 GiB |
| `CHRONO_AGENT_SANDBOX_HOME_TEMPLATE` | Legacy snapshot mode only: ready, initialized seed VolumeSnapshot in the native namespace and cluster. Must be empty for both native VCT modes. |
| `CHRONO_AGENT_SANDBOX_HOME_TEMPLATE_UID` | Legacy snapshot mode only: exact native seed UID. Historical retained allocations keep their recorded seed identity; empty-VCT homes require no snapshot. |
| `CHRONO_SANDBOX_HOME_STORAGE_CLASS` | Durable home StorageClass; configure all four home fields together, or omit all for legacy ephemeral operation |
| `CHRONO_SANDBOX_HOME_CAPACITY_GIB` | Home disk capacity, integer 1-1024 GiB; production 64 |
| `CHRONO_SANDBOX_HOME_TEMPLATE` | Ready, credential-free seed VolumeSnapshot in the Sandbox namespace; initialized with `infra/storage/home-seed.yaml` and `home-template.yaml` |
| `CHRONO_SANDBOX_HOME_TEMPLATE_UID` | Exact immutable Kubernetes UID of the seed snapshot; startup rejects a missing, replaced or unready seed |
| `CHRONO_UPSTREAM_BASE_URL` | Required canonical HTTPS model API base without trailing slash |
| `CHRONO_UPSTREAM_WIRE_API` | Exactly `responses` |
| `CHRONO_PLATFORM_BASE_URL` | Required canonical HTTPS platform proxy origin; also the upstream for the runtime's explicitly granted GitHub API proxy |
| `CHRONO_DOCUMENT_BUCKET` | Existing S3-backed bucket used for durable Markdown artifacts. Empty disables document source storage and runtime publication tools; writes fail explicitly instead of storing ephemeral documents. Production uses `chrono-platform-bot`. |
| `CHRONO_STORAGE_CLIENT_ID` | Deployment-owned machine identity for the existing `chrono-storage-service` proxy. Required when the document bucket is configured. Never passed to runtime Agents or browsers. |
| `CHRONO_STORAGE_CLIENT_SECRET` | Secret reference for the storage machine identity; required with a configured document bucket. Issued tokens are cached in memory and renewed before expiry or once after a 401. |
| `CHRONO_BASELINE_SERVICES` | Default `chrono-llm-public,ornn-api`; unioned into newly admitted Sandbox scope |
| `CHRONO_SANDBOX_CPU` | Seed default Sandbox Profile size; default `1`, accepts positive cores or millicores |
| `CHRONO_SANDBOX_MEMORY` | Seed default size; default `1Gi`, accepts positive whole Mi/Gi |
| `CHRONO_AGENT_MANAGER_SANDBOX_CPU` | New Agent Manager Sandbox CPU and native default warm/prepared capacity; default `2`, accepts positive cores or millicores. Independent of retained system Profiles. |
| `CHRONO_AGENT_MANAGER_SANDBOX_MEMORY` | New Agent Manager Sandbox memory and native default warm/prepared capacity; default `8Gi`, accepts positive whole Mi/Gi. Existing Sandbox reservations are unchanged. |
| `CHRONO_DEFAULT_AGENT_MODEL` | Seed default Agent model; default `gpt-5.6-sol`, nonblank |
| `CHRONO_DEFAULT_AGENT_EFFORT` | Seed default effort; default `high`, one of `minimal,low,medium,high,x_high` |
| `CHRONO_INSTALL_COMMAND_TIMEOUT_SECONDS` | Per-command bound 1-600; default 600 |
| `CHRONO_INSTALL_TOTAL_TIMEOUT_SECONDS` | Total ordered-command bound, at least command bound and at most 1800; default 1800 |
| `CHRONO_CLONE_DEPTH` | Git clone depth 1-1000; default 50 |
| `CHRONO_ACTIVITY_RETENTION_DAYS` | Activity TTL, 1-3650 days; default 90 |
| `CHRONO_SERVER_RATE_ADMISSION_PER_MINUTE` | Pre-auth network budget/burst; default 6000 |
| `CHRONO_SERVER_RATE_HEALTH_PER_MINUTE` | Separate probe budget, minimum 30; default 120 |
| `CHRONO_SERVER_RATE_DEFAULT_PER_MINUTE` | Ordinary request budget/burst; default 600 |
| `CHRONO_SERVER_RATE_CREATE_PER_MINUTE` | Creation request budget/burst; default 10 |
| `CHRONO_SERVER_RATE_INPUT_PER_MINUTE` | Agent input request budget/burst; default 120 |
| `CHRONO_SERVER_TRUSTED_PROXY_CIDRS` | Exact trusted load-balancer peers; empty by default, production explicit |
| `CHRONO_SERVER_TRUSTED_PROXY_HOPS` | Trusted rightmost forwarded hops, 1-16; default 1 |
| `CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS` | Empty or the complete build-pinned set; must not overlap trusted peer ranges |

New Agent Manager Sandbox operations capture their CPU, memory and Always on
inactivity policy at admission. Their private Sandbox configuration starts with
`always_keep_alive=true`; manual pause remains available. Recovered operations
keep their recorded capacity and policy, and operations admitted before the
Always on default retain their original inactivity behavior. Creating Agents or
Workspaces inside an existing Sandbox never changes that Sandbox's reservation
or availability. The deployment-owned system Profile defaults remain independent
so a new inline default cannot rewrite old Profiles or fail their startup checks.

New Agent admissions, including Agents added to existing Sandboxes, capture the
versioned GitHub workflow and memory-management guidance together. Prepared
capacity uses the same instructions under Agent profile revision 3. Only unclaimed
capacity with older instructions is replenished; admitted operations, owned homes
and active Agents retain their exact prompt, profile, conversation and runtime.
Older empty prompts and GitHub-only revision 2 remain valid for replay.

The memory guidance asks the Agent to check effective limits and shared usage
before expensive tasks, preserve headroom, limit concurrency, stream or batch
large inputs, and check periodically only during such active work. It may stop
its own unnecessary jobs or remove its own disposable caches; it must preserve
other Agents, the runtime and user files. A workload that cannot fit requires a
smaller method or an explained capacity constraint. This is Agent guidance, not a
background monitor or a guarantee against OOM. It adds no monitoring service and
does not alter the Sandbox's existing memory reservation.

CMA no longer reads checkpoint bucket or storage machine credentials. Production
startup installs a retired archive store that refuses upload, download and delete;
old environment variables cannot reactivate storage access. Historical manifest
metadata remains readable and is not evidence of an available restore path.
Existing archive-only paused Sandboxes require an explicit data migration outside
CMA before revival. Retained OpenSandbox runtimes can still release a preexisting
checkpoint writer hold; they cannot create new holds or transfer archives.

`RUST_LOG` controls tracing and defaults to `info`; it is not a `CHRONO_*`
configuration field. Log level changes never authorize credential-value output.
Keep model/service/provider configuration out of ad hoc frontend build flags.

Execution policy is pinned before each effect and participates in its operation
fingerprint. A changed timeout or clone-depth environment value must not change
an already admitted replay plan. Existing Sandboxes likewise keep their pinned
profile and service scope. Startup defaults seed missing default profiles; they
are not a live rewrite mechanism for published profiles or existing settings.

## Platform Settings

Settings are durable and revision-checked through `/api/v1/admin/settings`.
The Operations UI calls the same public API. Read first, retain its revision and
submit `expected_revision`; reconcile conflicts without automatic overwrite.

| Section | Shipped defaults |
|---|---|
| `defaults` | `sbp_default`, `wkp_default`, `agp_default`, each revision 1 |
| `inactivity.pause_after_seconds` | 900; valid range 60-86400 |
| `retention.agent_log_days` | 90 after terminal state, subject to integrity rules below |
| `limits.max_sandboxes_per_user` | 10 |
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

## Persistent Home And Retirement

Native runtime transport is selected independently of allocation. Keep
`CHRONO_AGENT_SANDBOX_RUNTIME_TRANSPORT=direct` where CMA can reach the runtime
Pod IP. With `kubernetes-portforward`, CMA opens a Kubernetes API stream to the
owned Pod's fixed port 9090 for each HTTP request or conversation.
`kubernetes-exec` instead opens the `runtime` container's stdio with fixed
`/usr/bin/python3 -I -u -c` arguments, running a checked-in relay to
`127.0.0.1:9090`. This supports RuntimeClasses whose CRI port-forward cannot
reach the guest's network namespace, provided their exec stream works. Python
must already exist in the admitted runtime image; no runtime image or package
installation is performed by this mode. The relay connects once, confirms
readiness before receiving a request, and bounds each directional buffer to
64 KiB. Its arguments contain only static code, never tokens or request bytes.
The existing
runtime token remains an inner HTTP/WebSocket header; the Kubernetes bearer
credential authenticates only the outer API connection. Neither credential is
placed in request URLs, and transport errors exclude upstream response bodies.
Neither stream mode falls back to another transport automatically.

Each stream validates the exact Sandbox UID, Pod UID, owner reference, address
and readiness before and after opening, before sending runtime credentials.
Opening has a 10-second deadline. Per native client, at most 64 conversations
and 32 HTTP requests can be active, with separate shared budgets so long-lived
conversations cannot consume control capacity. Exhaustion returns AtCapacity
without queuing. Control exchanges retain the one-hour deadline; history reads
have a 120-second deadline and existing response-size bounds. Completion,
cancellation, refusal and stream drop close the local streaming task. The exec
relay exits when its stdin or stdout closes, including under backpressure;
a successful exec exit preserves buffered response bytes for the caller.
A lost response
does not trigger transport-level replay or discovery of a replacement Pod.

Enable a stream mode only after a canary using the deployment's actual remote
identity proves control, history, long-lived WebSocket communication and cleanup
on the configured RuntimeClass. The API server and kubelet carry this traffic;
validate their capacity and connection limits for the intended concurrency.
The checked-in default remains `direct`. Changing this setting does not create
a router, alter NetworkPolicy or grant RBAC; the runtime namespace must already
authorize `get`/`create` on the selected `pods/portforward` or `pods/exec`
subresource for CMA. Exec also runs one small Python process per active stream;
include that cost and disconnect cleanup in the runtime capacity canary.

For native agent-sandbox runtimes, pause retains the original Sandbox CRD and
home PVC, patches `operatingMode: Suspended` and waits for all Pods owned by its
UID to disappear. CMA stores that retained binding even though the Sandbox has
no live runtime. Reconciliation must preserve it through control-plane restarts.

Resume advances the reserved runtime generation on that same CRD and PVC with
`operatingMode: Running`. It retains the admitted image and configuration, and
requires a new process boot identity before publishing a live binding. A change
to the deployment's default image does not authorize a different resumed image.
Lost responses are retried against the original UID and exact old or next spec;
resume never creates a replacement CRD to hide an uncertain PATCH.

Deletion can preempt an uncertain pause or resume. Retained runtime deletion
requires no live runtime HTTP connection and proves both CRD and owned Pod
absence before releasing the home. Paused Agent stop records settle against
that durable parent authority and a fresh provider inventory, retaining their
conversation history without fabricating a runtime close receipt. Native
retirement does not create checkpoint writer-release work: no such hold was
acquired by the retained-home lifecycle.

CMA no longer captures, uploads, downloads or deletes checkpoint archives, and
production storage effects return Unsupported without storage HTTP. Historical
manifest, object-key and cleanup records remain readable for migration and
audit. Their presence does not prove those external objects were removed.
Archive-only paused Sandboxes require an explicit migration outside CMA before
revival. Retained OpenSandbox can release an existing writer hold; it cannot
create a new checkpoint or restore an archive. Do not treat the current API as
a tool for draining historical storage cleanup records.

The 2026-10-06 storage investigation measured 4,292,528 requests in seven days
from the historical `chrono-bot-storage` client. Its retry population included
1,023 records and 52,893 physical keys. Those observations describe the old
archive implementation; they establish neither current storage activity nor
historical deletion completion. Any external migration must retain its own
verified deletion evidence.

Local CRD HTTP and lifecycle worker fixtures cover the new persistence behavior.
Production acceptance still requires the installed controller version, final
runtime image, real CRD/Pod/PVC operations and actual Agent conversation recovery.

## Rollback And Alerts

Pause orchestration and new public enablement before investigating a systemic
admission/recovery defect. Preserve existing resources and histories. Prefer a
forward fix built from the completed development branch. Reverting the web image
alone is safe only if its public contract still matches the API. Do not roll an
old Session-era binary onto the new database, or an incompatible runtime onto
new manifests. Restoring a pre-reset backup is a separate destructive epoch
transition, never an ordinary rollback that preserves post-reset work.

Alert on sustained readiness/store failures, reconciliation timeouts, unresolved
credential revocations, retained runtime/PVC cleanup backlog, rejected
integrity/fence checks, first-message recovery-required state, repeated runtime
absence, capacity failures, abnormal HTTP 429/503 rates and inactive baseline
services. Record object/operation/request ids and failure classes, not secrets.
Do not convert observation timeouts into permanent resource failure.

Inspect operator inventories and durable event heads first. Do not repair a log
by deleting one event, relabeling a checkpoint, editing a lease or changing an
execution fingerprint. A worker may resume only from its recorded authority.

An idle conversation can replay previous turn activity while reopening. CMA
acknowledges those runtime output positions without duplicating history or
opening a new turn when the adapter marks them as lacking prompt evidence.
Conversation metadata still updates, and a recovered active turn still consumes
its retained output. Repeated `no turn is active` recorder failures together with
`queued_message_delivery` indicate this replay path is preventing revival from
settling; preserve the conversation and deploy the corrected recorder.

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
rotation and cleanup with fake NyxID and provider dependencies. The adapter
refuses unsupported native tool or profile settings, retaining durable cleanup.
Bot credentials include the pinned profile's explicit scopes and model service;
CMA no longer adds an implicit NyxID tools catalogue service. These checks do
not establish real Telegram delivery or authorize retiring the existing bridge.


## Event Worker Profile catalogue and catalogue cleanup

The owner Profile library combines My profiles, Marketplace and Event workers.
Saved workers compose owned published CMA revisions and keep their full immutable
configuration in `event_worker_profiles` and `event_worker_profile_revisions`.
Bots retain the exact selected worker snapshot. Revisions survive retirement;
existing Bots are never repointed by an edit. See [public APIs](CMA_Public_APIs.md)
and [gateway architecture](CMAEG_Architecture.md).

Provider binding plans now produce deterministic pinned instance configurations
directly. They do not create imported drafts in the Profile catalogue. Historical
binding profile identifiers and digests are preserved; existing immutable
revisions and instance snapshots remain valid. When all three platform defaults
are explicitly set to custom profiles, boot no longer recreates the system
Profile catalogue entries.

Before an explicitly authorized catalogue deletion, point platform defaults at
retained published revisions and audit Trigger dependencies. Keep immutable
published revisions and instance snapshots required by existing resources.
For each retained Sandbox whose catalogue entry will be removed, persist its
current `always_keep_alive` policy in `sandbox_retained_availability`. The row is
fenced by Sandbox ID, owner, creation timestamp and pinned Profile ID, with the
current `document_schema_version`. The original catalogue policy takes precedence
while present. After removal, inactivity scheduling reads and fences the retained
policy in the same transaction as the Sandbox evidence. Malformed identity
refuses inactivity admission; it cannot authorize a pause. Deploy this reader
before removing the catalogue entry. Do not mutate or pause the Sandbox itself.

Delete only an inventoried, owner- and draft-version-fenced set of catalogue rows
in a transaction, with a mutation receipt and exact retained IDs. Verify restart
does not reseed deleted system profiles and existing runtime Pod UIDs remain
unchanged. A database-wide reset is outside this procedure.

### Ordinary NyxID agent API credentials

`CHRONO_NYXID_SERVICE_ID` is an optional catalog UUID string (unset by default).
When configured, CMA accepts ordinary `nyx_` and `nyxid_ag_` keys whose live
NyxID introspection proves access to this exact catalog service. Register the
existing confidential CMA OAuth client in that catalog's `introspection_client_ids`
before enabling this setting. Use the existing client ID/secret; no additional
shared token is required. Direct requests and NyxID proxy requests use the same
verification. Unconfigured deployments reject ordinary agent keys.

CMA validates `active`, `sub`, `token_type=api_key`, the exact `aud`, `proxy`
scope, complete permissions and any expiry. Every new verification checks live
NyxID grants; overlapping requests only share their in-flight verification. An
identity alone never grants access. Inactive evidence returns 401; provider
outages return 503. Browser cookies cannot override an explicitly supplied
agent key. Existing binding keys retain their restricted conversation authority.
Current/retired binding hashes and hashes retained by future rotations cannot
fall back to ordinary owner authentication. Historical hashes rotated before
this version still rely on NyxID revocation and service scope.

Deferred commands retain the exact encrypted agent key separately from browser
logins and verify it again before execution. Expiring keys retain their exact
provider expiry; verified nonexpiring keys store an explicit null expiry. The
encrypted binding for existing finite-expiry records is unchanged. Older CMA
versions cannot consume newly admitted nonexpiring-key commands, so deployments
with such commands require this version or a compatible successor for rollback.
Bot creation still requires the actual owner's renewable human login; accepting
an agent key does not authorize borrowing that owner's stored login.

### NyxID proxy access to the CMA API

`CHRONO_NYXID_PROXY_ENABLED` defaults to `false` in application code. The
production manifest enables the existing NyxID service identity protocol:
`X-NyxID-Identity-Token`, RS256, with a maximum 60-second assertion lifetime.
Configure `CHRONO_NYXID_PROXY_ISSUER` to the exact discovery issuer and
`CHRONO_NYXID_PROXY_JWKS_URL` to its trusted HTTPS signing-key endpoint. The
production issuer is `https://nyx-api.chrono-ai.fun`, not the login frontend.
CMA requires the exact `CHRONO_NYXID_SERVICE_ID` and an audience equal to
`CHRONO_SERVER_PUBLIC_URL` without its trailing slash. It never follows a
signing-key URL supplied by a token. Invalid or duplicate identity headers
cannot fall back to a browser cookie or unrelated bearer token.

In NyxID, set only the CMA catalog connection's `identity_propagation_mode` to
`jwt` and `identity_jwt_audience` to `https://bot.chrono-ai.fun`, and verify that
existing user connections inherited both values. Retain access-token forwarding
for existing CLI consumers. This uses existing NyxID behavior; it requires no
NyxID code change or deployment. The separate request-bound-read feature below
remains disabled.

NyxID verifies the Agent Key, service scope and applicable action approval before
forwarding. CMA verifies the signed identity and maps its permissions through
the same ownership and resource policies as normal API requests. This includes
lifecycle mutations and message/queue/interrupt commands, not just reads. The
existing browser-only consent gates for key-scope approval and Bot provisioning
remain the same as for independent CLI credentials. Managed runtime provisioning
still requires the owner's existing renewable CMA credential setup.

Accepted asynchronous commands retain a separate `nyxid-proxy:` admission and
its permission ceiling in the encrypted `auth_nyxid_proxy_grants` collection.
They use only the same subject's exact credential incarnation as execution
custody; they are never converted into personal/browser authorization. Workers
recheck current owner permissions and custody, intersected with that ceiling.
Token refresh within the same incarnation is allowed. Loss of token custody or
a new credential incarnation invalidates earlier admissions. Browser-cookie
logout alone does not revoke stored renewable credentials. Ending a NyxBot turn
or expiry of its 60-second assertion does not cancel already accepted work.
NyxID enforces key revocation on new requests; its identity assertion has no
original-key revocation handle for cancelling already accepted CMA commands.

Deploy CMA before enabling the connection setting. Roll back connection settings
before disabling proxy authentication. Retain a compatible CMA worker while
proxy commands are queued; older workers cannot execute `nyxid-proxy:` authority.
The collection is additive and contains no raw access tokens or refresh tokens.
It currently retains admissions for reads as well as writes; do not apply an
expiry policy that could remove authority for pending commands.

### NyxID request-bound reads

`CHRONO_NYXID_RESOURCE_REQUEST_ENABLED` defaults to `false`. Enable it only after
the corresponding NyxID proof issuer and authenticated introspection endpoint
are deployed and verified. It admits a finite set of read-only routes through
the dedicated `X-NyxID-Resource-Request` header; it does not enable lifecycle,
interaction, streaming, Bot creation or deferred commands. Ordinary bearer,
agent-key and browser authentication remain unchanged without that header.

Each proof binds the exact request and expires within 60 seconds. CMA verifies
the proof with its confidential NyxID client and applies current ownership and
RBAC checks. An invalid explicit proof fails closed without falling back to
another credential. The legacy `X-NyxID-Identity-Token` header cannot authenticate
a request. Keep the NyxID issuer disabled until all CMA readers are compatible;
disable issuance first when rolling this feature back. See
`CMA_Resource_Authorization.md` for the admitted routes and protocol limits.

The opt-in test
`auth::resource_request::tests::paired::real_nyxid_keys_and_egress_authorize_only_current_owner_reads`
joins CMA's real authentication and public Agent handlers to the NyxID companion
test `services::resource_request_proof::tests::joined::serve_joined_cma_acceptance_fixture`.
It uses real issued keys, confidential introspection and MCP/REST egress rather
than replacing the provider response. CMA resources remain in memory, with no
runtime and an operation store that refuses any attempted durable work.

Start the NyxID fixture against a fresh, dedicated loopback Mongo replica set;
its guard requires an explicit `NYXID_TEST_DATABASE_URL` on a non-default port,
`directConnection=true`, and no pre-existing application databases. Give
`NYXID_CMA_JOINED_MANIFEST` a new filename inside a private directory (0700).
The fixture publishes that manifest atomically with mode 0600. In the CMA
checkout, point `CMA_NYXID_PAIR_MANIFEST` at the same file and run:

```sh
cd api
cargo test --locked -p cma --bin cma \
  auth::resource_request::tests::paired::real_nyxid_keys_and_egress_authorize_only_current_owner_reads \
  -- --ignored --exact
```

The joined test checks owner listings and stored transcript reads, cross-owner
denial, exact request binding, mutation refusal, live role/service/key revocation
and the active-turn Stop fence. Successful completion shuts down the fixture;
failed runs must also stop their dedicated Mongo process and remove its private
data and credential manifest. The NyxID fixture expires after ten minutes as a
fallback. Record both source revisions and test output. Local acceptance proves
the two implementations interoperate; production topology, live credentials,
safe draining and real NyxBot acceptance remain separate release requirements.

This rollout must preserve every existing user's running Sandbox and Agent,
including the operator's own sessions. Use only isolated fixtures or dedicated
acceptance resources for test turns. Do not restart, pause, migrate, replace or
delete existing runtimes, Claims or home volumes to validate authentication.
Before replacing a control service, verify that its HTTP streams and background
turns drain or resume without cancelling, duplicating or losing accepted work.
An unchanged runtime Pod UID and a Ready replacement API are necessary observations,
but do not establish active-turn continuity. Keep proof issuance disabled if
the provider's deployment topology or draining behavior cannot be verified.

## Organization-owned always-live event workers

A gateway creator authenticated as an organization can prepare an Event Worker
Profile with `metadata.owner.kind: organization`. The gateway verifies the NyxID
subject type, using the authenticated organization authorization endpoint when
older user profiles omit that type. CMA accepts the owner assertion only through
the independently authenticated provider-binding manager. New binding sandbox
admission atomically writes explicit organization ownership evidence; failed
admission cannot leave an ownership sidecar behind. Existing personal bindings
retain their original interpretation. Owner kind cannot change through reconfigure.

The CMA provider accepts `environment.always_keep_alive: true`. The immutable
binding plan retains this policy and automatic inactivity scheduling consults
that exact binding and sandbox lineage. This prevents inactivity pause; it does
not block an explicitly requested manual lifecycle operation. Existing retained
availability policies continue to apply, including catalog-cleanup survivors.

### Bot Father sender-authorized setup

The expanded agent-owned requirements, channel target modes, delegated login
continuation and current implementation gaps are recorded in
[CMA Bot Father](CMA_Bot_Father.md). The setup API below is an intermediate
capability and does not by itself implement that complete design.

CMA Bot Father is an ordinary agent in the ChronoAI organization with the
[CMA Bot Father skill](skills/cma-bot-father/SKILL.md). Its runtime uses
`chrono-llm-public`, `ornn-api`, `cmaeg` and `cma`. The `cma` grant lets the skill
call the existing CMA service through the runtime's credential-injecting NyxID
proxy. It does not grant management authority over other users' resources.
CMA recognizes the binding key and limits it to binding conversations and the
sender-authorized setup and delegated-operation APIs. Adding this transport reach to an externally
managed key does not require replacing its existing sandbox or changing the
immutable environment snapshot. Credential service access must still be verified
through the exact organization connection and NyxID key scope plan.

CMAEG owns generic event, context and reply IO. It has no Bot Father configuration,
slash-command interception, business workflow or setup renderer. Every Activity,
including a login return or interaction result, reaches the normal provider.
The skill interprets natural conversation and calls
`POST /api/v1/bot-setups/actions`; the public contract is documented in
[API REST](API_REST.md#bot-father-setup-workflow). Ordinary questions remain normal
agent work, including during an unfinished setup.

Enable the CMA workflow by setting `CHRONO_BOT_FATHER_BINDING_ID` and
`CHRONO_BOT_FATHER_TELEGRAM_USERNAME` in CMA's environment. The binding must be a
valid `bnd_` identifier and the username must be the exact Telegram username
without `@`. An incomplete or malformed pair fails startup; absent values disable
this CMA integration. These are not gateway settings or credentials.

The agent supplies the current event reference and a typed action separately.
CMA obtains the original retained Activity through the generic gateway context
API, authenticates the configured binding, and derives the private-chat sender
from the attested event. An action cannot replace that identity or rewrite the
message. A sender without a linked identity receives a short-lived sign-in link.
CMA retains the renewable NyxID credential and activates delegation only after
fresh browser-bound PKCE login plus explicit consent for the displayed Telegram
actor. The activation and pending callback are committed together. CMA resumes
the exact initiating Agent through its leased durable outbox, recovers the same
command after restart, and delivers the result through the gateway's 24-hour
reply target. A callback never invents fresh human creation consent. The agent
never receives an access token, refresh token, password or Telegram Bot token.

The skill asks whether resources belong to Personal or an authorized organization,
then offers defaults or customized Agent, Workspace and Sandbox settings.
It prepares configuration from the user's preferences and executes typed CMA
operations. Users do not copy commands or JSON. Organization choices use the exact
organization's service inventory and connections; personal connections cannot
satisfy them. Missing connections use the returned owner-scoped sign-in links.

The user selects an owned Telegram Bot or follows the owner-scoped registration
link. The skill displays the effective owner, Bot, services, admission rules,
profiles and availability for final review. Creation requires a separate signed
user message such as `Create Bot` or `确认创建`, or a matching generic interaction,
received after that review. CMA checks the exact digest/revision and records the
plan before its existing durable Bot worker admits provisioning. An agent's own
request cannot stand in for the user's final confirmation.

Typed operations have durable request IDs and revision checks, so one natural
message can drive several authorized choices and retries do not duplicate work.
Plans, login grants and receipts survive restarts. Bot Father enables
`environment.session_affinity: "source_user"` and `environment.isolated_agents:
true` in its CMA Event Worker Profile. The provider attests the source user,
selects a durable per-user Agent/Worktree in the retained Sandbox, and isolates
the process filesystem, network and platform routes. These profile flags do not
change the projected Sandbox/Workspace or the stored service key. Workspace
capacity remains bounded; exhausted users are never assigned a sibling's Agent.
The agent reports completion
only when CMA returns an active result with the resulting Bot URL.
Legacy `advance` requests and encrypted pre-provider event snapshots remain
compatible; new model turns use the ordinary provider context store.

When moving an existing Telegram bot to Father, first disable its old CMA Bot
connection with `POST /api/v1/bots/{bot_id}/disable` and wait for completion.
The old provider binding, sandbox, conversations and credential renewal remain
owned by the original user. Disabled Bot renewal updates the provider credential
without updating an external route. It preserves the old route key while a
complete owner-scoped inventory proves that key is still bound to the exact
route; after retirement, obsolete keys can be revoked. Enabling a Bot reconciles
the route to the current key before opening gateway admission. Keep the old
connection disabled after migration: its retained runtime continues credential
renewal independently. Do not enable a connection whose route was retired;
that operation remains denied and retries under the existing worker semantics,
which blocks scheduled rotation until the pending operation is resolved.

### Opaque gateway connection record storage

The service-authenticated `/api/v1/gateway/connection-records` namespace uses the
same bounded opaque blob, majority persistence and optimistic concurrency
contract as channel records, in the separate `gateway_connection_records`
collection. CMA does not deserialize platform setup state or platform
credentials. The gateway alone encrypts and interprets these records. The
existing `cma:gateway:records` capability covers the namespace; no new shared
credential, Secret Manager dependency or user credential store is introduced.
