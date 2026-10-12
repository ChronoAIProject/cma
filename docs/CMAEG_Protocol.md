# CMA Event Gateway protocol 1.0

## 1. Scope and terms

This document is the normative contract for CMAEG clients, event sources and
agent providers. MUST, MUST NOT and SHOULD have their usual RFC 2119 meanings.
A **channel** binds an immutable profile snapshot, provider binding, source
policies, partition rule and reply policy. A **source** submits authenticated
events. An **activity** is the source-neutral event given to the agent. A
**partition** selects a stable provider conversation. A **turn** is one admitted
provider response. A **sink** receives results. A **binding** is the provider's
prepared environment for the channel. CMA stores encrypted channel records;
CMAEG keeps only disposable caches and active work in memory.

## 2. Versioning and compatibility

The HTTP protocol base is `/v1`; versioned responses carry
`CMAEG-Protocol-Version: 1`. This header identifies the major wire version, while
OpenAPI `info.version` identifies the additive revision (`1.0`). A client MUST
use the documented base and MUST NOT derive it from a package or image version.
Health endpoints and native `/mcp` have their own unversioned transport paths.

Changes within `/v1` MUST be additive. Adding an optional request field, response
field, operation or enum variant is allowed. Removing a field, changing its type,
renaming an operationId, making an optional request field required, adding a
required request field or removing an enum variant is forbidden. Clients MUST
ignore unfamiliar response fields and handle unfamiliar enum values without
assuming they mean success. Clients MUST NOT send new request fields to an older
server unless that server's supported revision is known.

Deprecation MUST be announced in this document with its migration and successor
version. A deprecated operation MUST continue to satisfy its retained contract,
return `Deprecation: @<Unix-seconds>` and, when retirement is scheduled,
`Sunset: <HTTP-date>`. Retirement of an incompatible contract requires `/v2`;
setting these headers alone does not authorize a breaking `/v1` change. No v1
operation is currently deprecated and these headers are therefore absent.

The reviewed OpenAPI baseline is `api/crates/cmaeg/openapi/v1.json`. It has one
server, `/v1`, and paths relative to that server. The compatibility test resolves
references, arrays and variants and checks operations, fields, requiredness,
enums, parameters, media types, responses and security. Constraint changes are
conservatively flagged for review. The freshness test also checks the CLI export.
From the repository root, regenerate, review the diff, commit:

```sh
cargo run --manifest-path api/Cargo.toml -p cmaeg --locked -- --print-openapi > api/crates/cmaeg/openapi/v1.json
cargo nextest run --manifest-path api/Cargo.toml -p cmaeg --locked -E 'test(openapi_compat)'
node .github/scripts/check-api-docs.mjs api/crates/cmaeg/openapi/v1.json docs/CMAEG_Protocol.md
git diff -- api/crates/cmaeg/openapi/v1.json docs/CMAEG_Protocol.md
```

A non-additive baseline diff MUST NOT be approved under `/v1`. Updating the file
does not waive compatibility review. Human interoperability review is separate
from automated checks; no second-engineer review is claimed by this document.

## 3. Conventions, limits and problems

Bodies use UTF-8 JSON and `snake_case` property names. Tagged unions use `type`
unless the schema explicitly uses `status` or `outcome`. Request objects reject
unknown fields. Open extension objects are exceptions: CloudEvents extension
attributes and `data`, profile `environment`, relay
`raw_platform_data`, interaction `values`, and provider Responses metadata.
Clients MUST ignore unknown response properties. Timestamps emitted by CMAEG
are RFC 3339 UTC; Unix seconds are used only in signed tokens/webhook headers.
Identifiers are case-sensitive opaque strings. Optional values can be absent;
clients SHOULD omit absent request values rather than invent empty strings.

| Limit | Value |
| --- | --- |
| JSON management body | 2 MiB framework limit; schema limits below also apply |
| REST tools / MCP body | 128 KiB; MCP read deadline 5 s |
| Relay body / read deadline | 1 MiB default, configurable up to 16 MiB / 5 s |
| Custom CloudEvent body / read deadline | 256 KiB / 5 s |
| Ingress concurrency | 256 per replica by default, shared by sources |
| Sources per channel / sinks | 1–4 sources (one of each variant) / at most 3 sinks |
| Encrypted channel record | 64 KiB including sealing overhead |
| CloudEvent ID and type / source and subject / time | 512 / 4096 / 128 bytes |
| CloudEvent extension name | 1–20 lowercase ASCII letters/digits, non-reserved |
| Profile system prompt / environment JSON | 32 KiB / 64 KiB |
| Profile skills / MCP servers | at most 32 / at most 8 |
| Outbound text, text block, card body, choice prompt, attachment caption | 32 KiB each |
| Message blocks / card fields / buttons / choices / form fields | 16 / 16 / 8 / 20 / 12 |
| Block and option IDs / option labels / card title and field label | 512 / 64 / 200 bytes |
| Attachment reference / MCP URL | 4096 bytes |
| Telegram rendered chunk | 4000 UTF-16 code units, including escaping |
| Channel ID / event reference / message reference | 512 / 4096 / 4096 bytes |
| Relay JWT / JWT header / JWKS body / keys | 16 KiB / 1024 bytes / 32 KiB / 16 |
| Provider metadata | 16 keys, 64 bytes per key, 512 bytes per value |
| Provider event context response | 1 MiB, 3 s deadline |
| Custom template data / rendered prompt | 8 KiB canonical JSON / 32 KiB |
| Live custom streams | 4096 waiters, 8 queued frames each, 1 MiB/frame, 32 MiB shared |
| Creator idempotency cache | 10,000 entries; 10-minute lifetime; replica-local |
| Relay routes | At most 64 unique IDs, each 1–512 bytes |
| Profile name / description / service scope | 80 / 2000 bytes / 64 services |
| Channel name / bot username / template | 80 bytes / 5–32 alphanumeric or underscore, no @ / 8 KiB |
| Sender/chat/key-hash allowlists | nonempty unique lists, at most 512 entries, each at most 512 bytes |

Limits are byte lengths unless another unit is stated. Deployment-specific
bounds and deadlines are listed in [operations](CMAEG_Operations.md).

Errors are RFC 9457 `application/problem+json`, for example:

```json
{"type":"about:blank","title":"invalid request","status":400,"code":"invalid_request","detail":"invalid mode"}
```

`title`, numeric `status` and stable `code` are present; `type` identifies the
problem kind, while optional `detail` and `instance` are diagnostic. Clients MUST
branch on `code` and HTTP status, never on diagnostic prose. The full code set is:

| Code | Meaning / usual HTTP status |
| --- | --- |
| `invalid_request` | Invalid JSON, field, mode or bounds; 400, body timeout 408 |
| `not_authenticated` | Missing or invalid creator/service/agent authentication; 401 |
| `forbidden` | Valid caller lacks the referenced channel/event authority; 403 |
| `channel_not_found` | Record absent or deliberately hidden from another owner; 404 |
| `channel_restore_unavailable` | Cold restore cannot complete; 503 |
| `channel_invalid` | Restored record is corrupt or fails validation; 503 |
| `provider_unavailable` | Provider/authentication backend unavailable; usually 503 |
| `provider_rejected` | Provider rejected setup or a turn; 502 |
| `provider_busy` | Provider/local turn capacity exhausted; 503 |
| `draining` | Instance is refusing new work during shutdown; 503 |
| `event_ref_invalid` | Invalid sealed reference; 400 |
| `event_ref_expired` | Event or message authority expired; 410 |
| `callback_invalid` | Callback signature, claims, headers or binding invalid; 401 |
| `admission_refused` | Admission refused outside the relay refusal envelope |
| `rate_limited` | Ingress/restore capacity exceeded; 429 |
| `conflict` | Version or idempotency conflict; 409 |
| `payload_too_large` | Body exceeds ingress limit; 413 |
| `unsupported_media_type` | Unsupported ingress content type; 415 |
| `internal` | Internal processing failure; 500 |

## 4. Opaque references

`channel_id` is stable for the channel's life, including updates and record
re-encryption. Store the complete returned value; do not rebuild it from
`record_id`. `event_ref` carries expiring event authority and reply routing; relay
references expire by the earlier of 30 minutes and reply-token expiry. Custom
references last at most 30 minutes. `message_ref` identifies a confirmed delivery
and carries channel/event authority and expiry. It is returned only when the
transport can identify the delivered message. Clients MUST NOT decode, alter,
log or substitute these three kinds of reference. A reference alone is not
sufficient authorization: tools also require the channel's agent key.

## 5. Management API

All six operations return `Cache-Control: no-store`. Except rekey, they use
`Authorization: Bearer <creator NyxID token>` and enforce ownership. The owner
subject in `profile.metadata.owner` MUST equal the authenticated creator.
`schema_version` for both channel requests and profiles is `1`.

Create: `POST /v1/channels` returns 201 `ChannelResponse`. The following complete
request uses a custom source; angle-bracket values are runtime substitutions,
not literal credentials. The HMAC secret MUST be a base64 string; retain the
string itself for signing as described in §6.

```json
{
  "schema_version": 1,
  "name": "Build assistant",
  "profile": {
    "schema_version": 1,
    "metadata": {"name":"Build assistant","description":"Build results","owner":{"subject":"<creator-subject>","kind":"human"},"provider":"cma_codex"},
    "model": {"access":"managed","service":"<model-service>","api_type":"responses","model":"<supported-model>","thinking_effort":"medium"},
    "permission": {"action_mode":"guarded","service_scope":[]},
    "context": {"system_prompt":"Summarize the event and reply.","skills":[],"mcp_servers":[]}
  },
  "provider": {"slug":"cma_codex"},
  "sources": [{"type":"custom","auth":{"type":"hmac","key_id":"builds"}}],
  "partition": {"version":1,"key":"event_subject"},
  "reply": {"auto_final":true,"ack_after_ms":2000,"sinks":[{"type":"sync"}]},
  "credentials": {"agent_key":"<bot-agent-key>","hmac_secrets":[{"key_id":"builds","secret":"<base64-string>"}]}
}
```

Every top-level field above is required. `credentials.agent_key` is required;
`channel_key` is optional for custom sources and required for relay sources.
`hmac_secrets` defaults to `[]`; entries are `{key_id,secret}` with unique IDs.
Agent/channel credentials contain 1–16384 printable non-space ASCII bytes;
HMAC IDs contain 1–512 bytes and secrets 1–4096 bytes. Public replies never echo
these credentials. A provider slug MUST be in the operator's allowlist and match
`profile.metadata.provider`; callers cannot supply a provider URL.

Profile fields not illustrated above: optional `environment` is provider-specific
JSON; metadata `description` defaults to empty. Identity `kind` is `human`,
`service_account` or `agent_key`. Model `access` is `managed` or `custom`;
`api_type` is `responses` or `chat_completions`; `thinking_effort` is `minimal`,
`low`, `medium`, `high` or `very_high`. All model fields are required. Permission
`action_mode` is `guarded` or `autonomous`; `service_scope` defaults to `[]`.
Context fields default to empty. A skill is `{source:"ornn",id,version?}`; an MCP
server is `{url,auth}` with HTTPS URL and auth `{type:"none"}` or
`{type:"nyxid_service",slug}`. Provider refusals MUST explicitly identify
unsupported fields; accepting a profile does not silently promise every field.

An optional `Idempotency-Key` header of 1–256 ASCII bytes replays an identical
create for the same creator on the same replica for ten minutes. Different
content under the same key returns 409. It is not durable or cross-replica.

A creator may additionally reserve a canonical UUID in the optional `record_id`
create field. CMAEG stores an encrypted pending definition at that identity before
contacting the provider. The definition retains `creation` with `channel_id`, a
request `fingerprint`, and `ready`. Pending definitions cannot process events or
accept policy updates. A successful bind marks the reservation ready. Repeating
the same body and record identity on another replica or after restart returns the
same channel and binding identities; changed content returns 409. A ready replay
reports empty honoured/refused lists, like GET. Record version is opaque and may
start above one because reservation and binding completion are separate writes.

`GET /v1/channel-records/{record_id}` lets the same owner recover the retained
channel, including its exact callback URL, after a lost response. Other owners
receive 404. Failed or uncertain provider calls retain their pending reservation
for explicit retry or deletion; the caller must reconcile that identity before
allocating another. This is management recovery, not durable event delivery.
Deletion releases the reservation; callers must never reuse a retired record UUID
or race cleanup against an outstanding create attempt.


A channel response has this shape (ellipsis denotes the previously supplied
profile/policy objects, not a literal JSON value):

```json
{"channel_id":"<opaque>","record_id":"<record>","version":1,"provider":{"slug":"cma_codex","binding_id":"<binding>","level":"l1","honoured":[],"refused":[]},"endpoints":{"nyxid_callback_url":"https://gateway.example/v1/callbacks/<opaque>","custom_event_url":"https://gateway.example/v1/channels/<opaque>/events"},"definition":{"schema_version":1,"name":"Build assistant","record_id":"<record>","owner":{"subject":"<creator-subject>","kind":"human"},"profile":{},"profile_digest":"sha256:<digest>","provider":{"slug":"cma_codex","binding_id":"<binding>","level":"l1"},"sources":[],"partition":{"version":1,"key":"event_subject"},"reply":{"auto_final":true,"ack_after_ms":2000,"sinks":[{"type":"sync"}]},"secrets":{"agent_key_hash":"sha256:<digest>"},"created_at":"2026-09-22T00:00:00Z","version":1}}
```

`definition.profile`, `sources`, `partition` and `reply` echo validated policy;
`profile` above is abbreviated. Relay definitions additionally include the
derived `jwks_url`. `profile_digest` is SHA-256 of compact JSON with recursively
sorted keys. `provider.honoured` lists field paths; `refused` contains
`{field,reason}`. GET responses have empty honoured/refused lists; they are not a
new binding negotiation. `definition.secrets` contains only `agent_key_hash`.

| Operation | Request | Success | Errors in addition to common management errors |
| --- | --- | --- | --- |
| `GET /v1/channels?after=<record UUID>` | Owner-scoped cursor; no owner override | 200 `{items,next_cursor}`; at most 50 channel responses | 403 missing read permission |
| `GET /v1/permissions` | No body | 200 subject and service role catalog | 401 invalid service identity |
| `POST /v1/channels` | Complete create body; optional idempotency header and reserved record UUID | 201 channel response | 409 reused key with different content; 503 cache capacity |
| `GET /v1/channel-records/{record_id}` | No body | 200 recovered channel response | 404 absent, unreserved, or other owner |
| `GET /v1/channels/{channel_id}` | No body | 200 channel response | 404 absent or other owner |
| `PUT /v1/channels/{channel_id}` | Complete create policy plus required `expected_version`; optional credential patch | 200 channel response with incremented version | 409 stale version |
| `DELETE /v1/channels/{channel_id}?expected_version=N` | Required unsigned version query; no body | 204, empty body | 400 missing/unknown/invalid query; 409 stale version |
| `POST /v1/channels/{channel_id}/test` | No body | 200 `{"accepted_after_ms":42,"first_text":"...","level":"l1"}` | Provider refusal, timeout or no usable result |
| `POST /v1/channels/{channel_id}/rekey` | CMA service bearer; no body | 200 `{"kid":1}` | 401 service authentication failure |

PUT replaces policy, not a merge patch. Omitted/null `credentials` retains all
credentials. A supplied patch has optional `agent_key`, `channel_key` and
`hmac_secrets`; omitted/null fields retain prior values, while a supplied HMAC
list replaces the list. Referenced secrets must still exist. Update cannot change the provider slug and preserves
channel identity and enforces the optimistic record version. Every successful PUT
reconciles the provider binding, including when credentials are retained. This
repairs a previous update that changed the provider but failed to save the record;
a GET alone does not establish the provider's credential state.
Delete unbinds the provider before removing the record; an unbind failure
preserves the record and is returned to the caller. Rekey re-encrypts the record under the
current master key and retains `channel_id`; operators must retain keys needed
to open existing channel pointers.

Common management errors: 400 `invalid_request` for invalid IDs, schemas, policy,
credentials or unknown providers; 401 `not_authenticated`; 404
`channel_not_found`; 409 `conflict`; 502 `provider_rejected`; 503
`provider_busy`/`provider_unavailable` for provider, record or authentication
backend failures; 500 `internal`. Body extraction failures are reported as
400. Authentication precedes mutation; no operation grants access based on a
caller-supplied owner alone.

## 6. Sources

### NyxID relay

Configure `{type:"nyxid_relay",issuer,key_id,route_ids,platform:"telegram",
admission,bot_username?}`. The issuer MUST be HTTPS; the gateway derives
`<issuer>/.well-known/jwks.json`. Requests MUST NOT supply `jwks_url`.
Admission is `{type:"all"}`, `{type:"allowlist",sender_ids:[...]}` or
`{type:"scoped",senders,chats,groups}`. `senders`/`chats` are `{type:"deny"}`,
`{type:"open"}` or `{type:"allowlist",ids:[...]}`. Groups are `deny`, `all`,
`mention`, `reply_to_bot` or `mention_or_reply_to_bot`.

Send `POST /v1/callbacks/{channel_id}` with `Content-Type: application/json` and
exactly one each of `x-nyxid-callback-token`, `x-nyxid-message-id`,
`x-nyxid-platform`, `x-nyxid-timestamp`; optional `x-nyxid-user-token` must also
be single-valued. Empty/comma-joined values are rejected. The timestamp is RFC
3339 and must agree with the callback token's `iat` within 60 seconds.

Body fields: `message_id`, `correlation_id`, `platform`, `reply_token`,
`agent:{api_key_id,name}`, `conversation:{id,platform_id,type}`,
`sender:{platform_id,display_name?}`, `content:{type,text?,attachments?}`,
`timestamp`. Optional fields are `thread_id`, `reply_to_message_id`,
`reply_to_platform_message_id`, and `raw_platform_data`. Attachment entries have
required `content_type,url` and optional `image_key,file_key,platform_message_id,
filename,mime_type,size_bytes`. Attachment receipt is currently refused.

All three token kinds use RS256, base64url without padding, and a header containing
only `alg:"RS256",typ:"JWT",kid`. Keys come only from the channel's pinned JWKS.
Standard claims are `iss,aud,iat,exp,jti,token_type` and optional `nbf`, with
nonnegative Unix times, `exp > iat`, and 60-second clock tolerance. Callback JWT:
`aud:"channel-relay/callback"`, `token_type:"relay_callback"`, lifetime ≤300s;
additional claims `api_key_id,message_id,platform,body_sha256` bind the exact
raw body using lowercase SHA-256 hex. `correlation_id == jti`; header/body/key
and platform identities must agree; `conversation.id` must be in `route_ids`.
An empty `route_ids` list is a pending route binding and admits no callbacks.
Creators may create the channel, set the route key callback URL, create the
identity-provider route, then update the channel with its exact route ID.
This does not broaden sender or chat admission policies.
Reply JWT: `aud:"channel-relay/reply"`, `token_type:"relay_reply"`, lifetime
≤1800s; `api_key_id,conversation_id,inbound_message_id,platform` bind the same
callback. Its expiry must still be in the future without expiry tolerance.

Optional user JWT: `aud` equals issuer, `token_type:"access"`, lifetime ≤300s,
`sub` equals channel owner, `relay:true`, `relay_api_key_id` matches, and required
`relay_allowed_service_ids`, `relay_allowed_node_ids`, `relay_allow_all_services`,
`relay_allow_all_nodes` corroborate relay identity only. Lists have at most 256
bounded identifiers. `act`/`cnf` must be absent/null; `delegated`, `sa` and
`assistant_forward` must be absent/false. It grants no additional invocation rights.

Authenticated supported messages return 202
`{"status":"accepted","event_id":"...","conversation_id":"..."}` after
provider acceptance or the acceptance budget. Policy/unsupported messages return
202 `{"status":"refused","reason":"..."}` and launch no turn. Reasons are:
`bot_sender`, `system_sender`, `unknown_sender`, `message_edit`,
`interactive_action_unsupported`, `system_event`, `content_unsupported`,
`attachment_unsupported`, `sender_denied`, `chat_denied`, `group_denied`,
`group_not_addressed`, `addressing_evidence_unavailable`, `thread_evidence_unavailable`.
Signature/binding failure is 401 `callback_invalid`, not a policy refusal.

| Telegram capability | Contract |
| --- | --- |
| Input | Human text only; edits, callbacks/actions, system and attachment input refused |
| Group addressing | Pinned bot username and UTF-16 mention entities; bot reply evidence from raw Telegram payload |
| Topics | Verified forum topic evidence required for thread partitioning |
| Output | Escaped text; rich blocks degrade as §8 specifies |
| Edit / typing / interactive callbacks | Unsupported; never reported as performed |
| Attachments | Safe link text only; no file transfer |

### Custom CloudEvents

Configure `{type:"custom",auth,schema?,prompt_template?}`. Auth is
`{type:"hmac",key_id}` referencing a retained HMAC secret, or
`{type:"nyxid_key",key_hashes:["sha256:<lowercase-hex>"]}`. The latter accepts a
single `Authorization: Bearer <agent-key>`, locally hashes it and compares in
constant time. Custom ingress keys contain 1–4096 printable non-space ASCII bytes. No remote
NyxID request occurs on this path. Tools additionally require the NyxID agent-key
syntax: `nyxid_ag_` or `nyx_` followed by 16–128 URL-safe characters.

POST to `/v1/channels/{channel_id}/events`. Structured mode uses
`application/cloudevents+json` with required `specversion:"1.0",id,source,type`,
optional `subject,time,datacontenttype,dataschema,data`, and CloudEvents extensions.
`source` is an absolute URI. Binary mode uses `application/json` for the data
body and single-valued `ce-specversion`, `ce-id`, `ce-source`, `ce-type` headers,
with optional `ce-subject`, `ce-time`, `ce-dataschema` and extension headers.
Only JSON data is supported. The `schema` field MUST be omitted or null in v1; non-null values are rejected.

HMAC uses the Standard Webhooks header/envelope format with CMAEG's explicit key
convention: the configured base64 secret string's UTF-8 bytes are the HMAC key;
DO NOT decode it or strip a prefix. Sign exactly
`webhook-id + "." + webhook-timestamp + "." + raw_body` using HMAC-SHA256.
Send `webhook-signature: v1,<standard-base64-digest>`. Timestamp is decimal Unix
seconds within ±300s. Each of the three headers occurs exactly once; the
signature header may contain up to 16 space-separated signatures. Webhook ID is
1–512 bytes; timestamp ≤20 and signature header ≤2048 bytes. Authenticate before
normalizing the body. This executable Python example reads runtime values and
prints a signed request's headers and exact body:

```python
import base64, hashlib, hmac, json, os, time, uuid
body = json.dumps({"specversion":"1.0","id":str(uuid.uuid4()),
    "source":"https://build.example","type":"build.completed",
    "subject":"project-a","data":{"result":"passed"}}, separators=(",", ":")).encode()
webhook_id, timestamp = str(uuid.uuid4()), str(int(time.time()))
signed = webhook_id.encode() + b"." + timestamp.encode() + b"." + body
signature = base64.b64encode(hmac.new(os.environ["BUILD_WEBHOOK_SECRET"].encode(), signed, hashlib.sha256).digest()).decode()
headers = {"Content-Type":"application/cloudevents+json", "webhook-id":webhook_id,
    "webhook-timestamp":timestamp, "webhook-signature":"v1," + signature}
# POST body unchanged with headers to the returned custom_event_url.
print(json.dumps({"headers":headers,"body":body.decode()}))
```

Templates replace `{{type}}`, `{{source}}`, `{{subject}}`, `{{id}}`, `{{time}}`
and `{{data}}` once; unknown placeholders remain literal; substituted values are never
reinterpreted as template syntax. Without a template the gateway supplies a
bounded textual event summary. Ordinary success is the same 202 accepted body
as relay ingress. `?mode=stream` requires `{type:"sync"}` in `reply.sinks` and
returns 200 `text/event-stream`. Frames have JSON data:

| SSE event | Data |
| --- | --- |
| `cmaeg.text.delta` | `{event_id,conversation_id,text}` |
| `cmaeg.message` | `{event_id,conversation_id,text,message,error}` |
| `cmaeg.completed` | Same shape; nullable `text`, `message:null`, `error:null`; terminal |
| `cmaeg.failed` | Same shape with a problem in `error`; terminal |

Keepalive comments are not events. A slow consumer receives terminal
`cmaeg.failed` with `{"error":{"code":"internal","detail":"sync_backpressure"}}`.
There is no durable replay or Last-Event-ID contract. Disconnect removes the
waiter without cancelling the admitted turn.

A `{type:"result_webhook",url,hmac_key_id}` sink receives structured CloudEvents
of type `cmaeg.response.message`, `cmaeg.response.completed` or
`cmaeg.response.failed`, with `specversion:"1.0"`, generated `id`, UTC `time`,
`subject` equal to inbound event ID, `datacontenttype:"application/json"`, source
`<gateway-origin>/channels/<opaque-channel-digest>`, and the five-field data shape
above. The gateway signs exact bytes with the same HMAC convention. Recipients
MUST deduplicate by webhook ID; four attempts use stable IDs/body with fresh
signature timestamps and 1/4/16-second backoffs, five seconds per attempt.
Destinations MUST be HTTPS and resolve only to public addresses; each attempt
revalidates and pins DNS. Redirects and proxy environment routing are disabled.
A custom `{type:"source"}` sink does not imply a result webhook.

Both ingress types can also return 400/408 `invalid_request`, 404
`channel_not_found`, 413 `payload_too_large`, 415 `unsupported_media_type`, 429
`rate_limited`, 503 restore/provider/draining problems, or 502
`provider_rejected`. Ingress overload includes `Retry-After: 1`; local provider
capacity includes `Retry-After: 2`. Custom authentication failure is 401.

## 7. Activity and partitioning

A full activity is `{schema_version:1,event_id,kind,actor,conversation,occurred_at,source}`.
Actor is `{id,kind,display_name?}`, kind `human|bot|system|unknown`. Conversation
is `{id,kind,thread_id?}`, kind `private|group|channel|thread|custom`. Source is
`{type:"nyxid_relay",platform:"telegram",route_id}` or `{type:"custom"}`.
Activity kind is one of:

| Type | Fields |
| --- | --- |
| `message` | `text,mentions_bot,reply_to?,attachments` (default `[]`) |
| `interaction` | `block_id,action_id,values` (default `{}`) |
| `command` | `name,args` |
| `event` | `ce`, the complete CloudEvent |

Attachment fields are `reference,kind,mime_type?,filename?,size_bytes?`, with
kind `image|file|audio|video|other`. Custom actor and subject URIs are retained in
full in provider context; compact references/metadata may use digests.

Partition is `{version:1,key}`. Relay keys are `conversation_and_sender`,
`conversation` and `thread`; thread requires verified topic evidence. Custom keys
are `fixed`, `event_subject` and `per_event`. `event_subject` uses the subject when present and otherwise the source URI. The resulting conversation ID is **deterministic and stable for
the channel; opaque to clients**. A partition change affects future mapping;
clients MUST NOT manufacture IDs by copying an implementation's hash algorithm.

## 8. Tools API and MCP

All tools require the single channel agent-key bearer, compared locally in
constant time, and an unexpired reference. REST tools return JSON 200 receipts
or problems. Native MCP at `/mcp` uses JSON-RPC 2.0 and protocol `2025-06-18`,
with `initialize`, `notifications/initialized`, `ping`, `tools/list`, `tools/call`.
Send `Content-Type: application/json`; an optional `MCP-Protocol-Version` must be
`2025-06-18`, and an optional `Origin` must equal the configured public origin.
Notifications receive empty 202; calls use string/integer/null IDs (strings ≤256
bytes). Tool refusals use JSON-RPC error `-32000` with the problem in `error.data`;
parse/invalid request/unknown method/invalid params use `-32700/-32600/-32601/-32602`.
Tool call results contain text JSON, matching `structuredContent`, and `isError`.
A transport success alone does not mean platform delivery succeeded.

| Name | REST | MCP arguments |
| --- | --- | --- |
| `replyToEvent` | POST `/v1/events/{event_ref}/replies`, body message | `{event_ref,message}` |
| `updateMessage` | POST `/v1/messages/{message_ref}/update`, body message | `{event_ref,message_ref,message}`; both references must name the same event |
| `sendTyping` | POST `/v1/events/{event_ref}/typing`, no body | `{event_ref}` |
| `readEventContext` | GET `/v1/events/{event_ref}/context` | `{event_ref}` |

NyxID service proxy tool names are `<configured-slug>__replyToEvent`,
`<configured-slug>__updateMessage`, `<configured-slug>__sendTyping`, and
`<configured-slug>__readEventContext`; native names omit the slug. The default
slug is `cmaeg`. A message is `{text?,blocks:[],reply_to?}`; text or at least one
block is required. `reply_to` is an opaque message reference. Blocks:

| Block type | Complete fields after `type` | Telegram rendering | Custom rendering |
| --- | --- | --- | --- |
| `text` | `text` | Escaped text | Exact JSON |
| `card` | `title,body?,fields:[],image?`; fields `{label,value}` | Title/body/labelled fields; safe image link | Exact JSON |
| `buttons` | `id,options` | Numbered text, no callback controls | Exact JSON |
| `choice` | `id,prompt,options,multi` | Prompt and numbered options; no interactive selection | Exact JSON |
| `form` | `id,fields` | Labelled textual form | Exact JSON |
| `attachment` | `reference,kind,caption?` | Safe URL as link text; unsafe references dropped | Exact JSON/reference |
| `link` | `url,label?` | Escaped safe link | Exact JSON |

Options are `{id,label,style?}` with style `default|primary|danger`, default
`default`; IDs must be unique within an option list. Forms use
`{id,label,kind,required}`; kind is `{type:"text"}`, `{type:"number"}`,
`{type:"date"}` or `{type:"choice",options}`. Block IDs must be unique within
the message. Rendering never interprets arbitrary agent text as Telegram markup.
Chunking preserves UTF-16 boundaries and complete escape atoms.

A reply/update receipt is `{message_ref?,rendering,delivery}`. Rendering is
`{outcome:"rendered|degraded|unsupported",blocks:[{index,outcome}]}`, where each
block outcome is `{type:"exact"}`, `{type:"degraded",how}` or
`{type:"dropped",why}`. Index `65535` describes whole-message chunking. Delivery
is tagged by `outcome`: `confirmed` with optional `platform_message_id`,
`rejected` with `reason`, `rate_limited` with `retry_after` seconds, `unknown`
with `reason`, or `unsupported`. Unknown means the platform may have delivered;
clients MUST NOT blindly resend. Typing returns `{outcome:"sent|unsupported"}`.
Current Telegram and custom transports return unsupported for editing/typing.

Context is `{event_id,conversation_id,activity:{kind,actor,conversation,occurred_at},
capabilities:{edit,typing,buttons,cards,attachments,max_text_chars}}`. Telegram
capabilities are `false,false,"numbered_text","text","link_text",4000`; custom
are `false,false,"json","json","reference",32768`. Context retrieval requires
provider persistence described below; if unavailable it returns 503
`provider_unavailable` with `detail:"event_context_unavailable"`.
Tool errors additionally include 400 invalid request/reference, 401 missing/invalid
key, 403 mismatched authority, 410 expired reference and restore errors from §3.

## 9. Provider contract

Provider URLs are operator-allowlisted templates, never source-controlled URLs.
External provider calls use the retained agent bearer. For the `cma_codex`
provider, binding and conversation management use the configured CMA service
bearer only when both the origin exactly matches `CMAEG_CMA_BASE_URL` and the
path is a CMA binding or conversation management path. Turns and event context
reads continue to use the retained agent bearer. Redirects are disabled; another
allowlisted provider never receives the CMA service bearer.
`PUT <binding-path/{binding_id}>` receives
`{schema_version:1,profile,profile_digest,agent_key,tools:{native_mcp_url,nyxid_service_slug}}`.
The provider MUST authenticate the agent identity, bind ownership, idempotently
prepare the profile under that binding ID, and return
`{binding_id,level,honoured:[field-paths],refused:[{field,reason}]}`. It MUST name
unsupported requirements. `DELETE` on the same path releases the binding;
absence may return 404, which the gateway treats as already unbound.

`PUT <conversation-path/{binding_id}/{conversation_id}>` with `{}` ensures the
caller-selected conversation exists under that binding. It MUST be idempotent,
retain its ID and enforce binding ownership. DELETE removes it. A Responses
request referencing an absent conversation returns 404 `conversation_not_found`;
the gateway ensures it once and retries with the same idempotency key.

`POST <responses-path>` receives `{conversation,input,stream:true,metadata,event_context?}`
and `Idempotency-Key`. The provider MUST durably admit at most one turn for the
same key, including concurrent and retried requests, and return an SSE stream.
Metadata is a bounded string map; `input` contains user message items. Acceptance
is `response.created` or `response.in_progress` with a nonempty response ID, e.g.
`{"type":"response.created","response":{"id":"resp_..."}}`. HTTP success
without that event does not prove admission to the gateway.

Supported SSE types: `response.created`, `response.in_progress`,
`response.output_item.added`, `response.output_text.delta`,
`response.output_text.done`, `response.output_item.done`,
`response.mcp_call.in_progress`, `response.mcp_call.completed`,
`response.mcp_call.failed`, `response.completed`, `response.failed`,
`response.incomplete`. Output items use `output_index` and `item`; message items
have `id,type:"message",content:[{type:"output_text",text}]`. Text frames have
`item_id` plus `delta` or `text`. Completed frames carry `response`; failed frames
carry `error:{code,message}` directly or inside `response`; incomplete frames
carry `incomplete_details:{reason}`. Unknown event types are ignored for forward
compatibility. A stream ending without a terminal outcome is `stream_lost`.
Providers MUST commit message items before emitting `output_item.done`.

The optional `event_context` extension is
`{schema_version:1,channel_id,conversation_id,activity,event_ref}`. The provider
MUST persist it atomically with turn admission and idempotency, expose `event_ref`
to the agent, and serve it through authenticated
`GET <conversation-path>/events/{event_id}` with the same complete object. A retry
MUST NOT overwrite an already admitted event with another context. Channel,
conversation and activity event IDs must match; the gateway validates all three.
Synthetic management tests omit this extension. Unsupported/unavailable reads
must fail explicitly; the gateway does not invent a context from the sealed ref.

429 indicates retryable upstream capacity; 409 with `stale_response`,
`session_busy` or `conversation_in_progress` is also retryable. Other 5xx responses
are unavailable;
`Retry-After` is decimal seconds. Other provider refusals retain a typed failure.
Requests may retry busy admission within the configured budget with the same
idempotency key. No stream failure may silently create a new logical turn.
The gateway's total turn timeout bounds consumption; disconnecting an ingress
caller does not revoke already admitted work.

The public Agent Card is fetched from the configured card path. `capabilities`
may be an object or string array. Classification is `l1` basic Responses,
`l2` `activity_items`, `l3` `client_tools`, `l4` `cancel`, `l5` `rich_input`;
the highest advertised recognized capability selects the level. A level is an
advertisement, not proof of unimplemented behavior. Providers MUST only advertise
capabilities verified against their implementation. The binding response records
its own level and field refusals for that channel.

## 10. Delivery semantics

An acknowledgement may be sent after `ack_after_ms` for a long turn; zero disables
it. It is progress only and never suppresses the final answer. After an ack,
inactivity may produce heartbeats (60s default), at most five. Committed message
items are delivered when `auto_final` is true; completion text is a fallback only
when no committed/tool answer exists. A confirmed or unknown tool reply suppresses
later automatic answer delivery. A failure notice is attempted once unless a tool
has already replied. Typing/acknowledgement alone does not mark an answer delivered.

Telegram sends are paced at least three seconds apart per route. The first send
may consume an unexpired single-use reply token; subsequent sends use the channel
key. A 429 can be retried once with the channel key after its bounded wait.
Unknown sends are never blindly retried. Chunked messages retain their rendering
report; partial delivery is not full confirmation.

Provider idempotency deduplicates inbound events. Local delivery state prevents
duplicates within a running replica; it is bounded and not durable. Crashes can
lose an accepted turn's unobserved output, acknowledgements, queued deliveries or
custom stream frames. A crash between external delivery and recording its receipt
can leave an unknown outcome. CMAEG promises neither durable delivery nor exactly
once external effects. It does not cancel the provider merely because an ingress
connection closes. Shutdown refuses new turns, then drains turns and queued sends
under one configured deadline; deadline exhaustion is an accepted loss boundary.
External tool calls enforce reference expiry even though already-admitted internal
turns can finish delivery after that expiry.

## 11. S3 latency and Server-Timing

Ingress latency is the last inbound body byte through the first successful TCP
write of the provider Responses request. It excludes provider processing. Warm
p99 MUST be ≤1000ms; cold p99 ≤2000ms, with restore wait p99 ≤800ms. The local
harness additionally checks warm p50 <50ms. Tool delivery p99 MUST be ≤500ms for
an available route; waiting behind another send includes its three-second pacing
wait and is reported honestly rather than subtracted.

Ingress `Server-Timing` fields are `restore`, `verify`, `launch`, `total`, in
milliseconds. `launch` starts at body completion and overlaps restore/verification;
DO NOT sum them. `total` includes body reading and acceptance waiting. With no
observed socket write the header says `launch;dur=0.000;desc="not_written"`;
this is not a successful zero-latency sample. An acceptance timeout may return
before a later observed write; metrics can record that write but the sent header
cannot change. Tool REST/MCP responses have `deliver;dur=...` through receipt or
refusal. Every public response propagates or generates `x-request-id`.
Measured fixture evidence and executable instructions are in the operations doc.

## 12. Operation inventory

Generated from the reviewed OpenAPI snapshot; effective paths include its server.

<!-- openapi-operations:start -->
| Operation | operationId |
| --- | --- |
| `POST /v1/callbacks/{channel_id}` | `receiveRelayCallback` |
| `POST /v1/channels` | `createChannel` |
| `GET /v1/channels` | `listChannels` |
| `GET /v1/permissions` | `getPermissions` |
| `GET /v1/channel-records/{record_id}` | `recoverChannel` |
| `DELETE /v1/channels/{channel_id}` | `deleteChannel` |
| `GET /v1/channels/{channel_id}` | `getChannel` |
| `PUT /v1/channels/{channel_id}` | `updateChannel` |
| `POST /v1/channels/{channel_id}/events` | `receiveCustomEvent` |
| `POST /v1/channels/{channel_id}/rekey` | `rekeyChannel` |
| `POST /v1/channels/{channel_id}/test` | `testChannel` |
| `GET /v1/events/{event_ref}/context` | `readEventContext` |
| `POST /v1/events/{event_ref}/replies` | `replyToEvent` |
| `POST /v1/events/{event_ref}/typing` | `sendTyping` |
| `POST /v1/messages/{message_ref}/update` | `updateMessage` |
<!-- openapi-operations:end -->


## Independent service identity and persistence

Management authenticates the `cma-event-gateway` NyxID resource and applies the
`cma-eg:*` permissions before owner checks. The browser uses a separate encrypted
server-side session via `/auth/login`, `/auth/callback`, `/auth/session` and
`POST /auth/logout`; cookies do not authorize the other CMA services. Existing
agent tool and relay authentication retain their protocol-specific contracts.

Sealed records live in `eg_channel_records`, using the gateway's collection-scoped
credential. Agent Manager never reads this collection. Existing channel pointers
retain their canonical record URL and master keys, while record resolution is
local to the gateway. Transfer legacy records before routing existing channels
to this service, preserving record IDs, owners, versions and encrypted blobs.
