# Reference

<!-- nav: 11 Reference -->

`Client::builder(base_url)` returns a `ClientBuilder`. The builder accepts an
optional request timeout and bearer token, and produces a cloneable `Client`.
The timeout covers both response headers and the complete body. Reverse-proxy
path prefixes are retained; resource identifiers and query values are encoded
as data, not interpolated into request URLs.

`Client::read_conformance()` performs `GET /api/v1/conformance` anonymously and
returns `Conformance`, containing `service`, `status`, and the currently exposed
`interfaces`.

| Resource Group | Operations |
| --- | --- |
| `agent_profiles()`, `workspace_profiles()`, `sandbox_profiles()` | `list`, `read`, `create`, `save`, `publish`, `list_revisions`, `read_revision`, `set_visibility`, `retire` |
| `profile_authoring()` | `bounds`, `services`, `search_skills`, read-only `resolve_repository` |
| `marketplace::<C>()` | `list`, `read`, `fork` |
| `sandboxes()` | `list`, `read`, `create`, `delete`, `pause`, `revive`, `events` |
| `workspaces()` | `list`, `read`, `create`, `delete` |
| `worktrees()` | `list`, `read` |
| `agents()` | `list`, `read`, `create`, `input`, `steer`, `interrupt`, `answer_approval`, `stop`, `delete`, `events`, `transcript`, `usage`, `resources`, `models`, `settings`, `update_settings`, `skills`, `install_skill`, `access_requests`, `approve_access_request`, `deny_access_request`, `update_service_access` |
| `auth()` | `read_session`, anonymous `read_signup_options`, navigation-only `login_url`, `logout_url` |
| `triggers()` | `list`, `read`, `create`, `save`, `delete`, `publish`, `retire`, `list_revisions`, `list_placements`, `create_placement`, `retire_placement`, `embed`, `stats` |
| `launches()` | `create_or_resume`, `list`, `read`, `start`, `cancel`, `requirements`, `events`, `stream`, `create_connection`, `read_connection` |
| `agui()` | `read_capabilities`, `read_sandbox`, `run`, `events` |
| `responses_v2()` | `read_capabilities`, `create`, `create_for_agent`, `read`, `read_conversation`, `list_items`, `replay`, `observe_controls`, `submit_control`, `interrupt_with_input`, `read_command`, `observe_queued_cancellation`, `cancel_queued_response` |
| `agent_events_v2()` | `read_transport_eligibility`, `snapshot`, `events` with `consume_next` and `acknowledged_cursor` |
| `responses()` | `create_conversation`, `read_conversation`, `list_items`, `create`, `read`, unsupported `delete`, `cancel` |
| `admin()` | `list_sandboxes`, `read_sandbox`, `sandbox_events`, `retire_sandbox`, `list_profiles`, `retire_profile`, `read_settings`, `replace_settings`, `patch_settings`, `reset_settings`, `pause_triggers`, `resume_triggers`, `read_runtime_inventory`, `list_warm_pools`, `configure_warm_pool`, `resize_warm_pool`, `list_activity` |
| `Client` reads | `read_conformance`, `read_openapi`, `read_defaults`, `read_trigger_preview`, `read_trigger_badge` |
| Explicit refusals | `agents().terminal`, `unsupported().mcp`, `unsupported().a2a` |

`C` is one of `AgentProfileContent`, `WorkspaceProfileContent`, or
`SandboxProfileContent` from `cma_sdk::profiles`. Profile drafts and immutable
revisions keep the corresponding content type. `save` requires the exact
`draft_version`, replaces content, and retains omitted name or description.
`publish` requires the exact draft version. `fork` requires an explicit immutable
revision rather than selecting a moving latest revision implicitly.

Owned profile drafts require profile-management authority. Exact published
revision reads also support accessible public and owner revisions with the
service's resource-use authority; authorization is always decided by CMA.
Marketplace reads validate the requested kind, identity, revision consistency,
and required Sandbox trust metadata.

`PageOptions` holds an opaque `cursor` and optional `limit`. Owner lists and
marketplace results return `ResourcePage<T>`, whose `next` is the exclusive
cursor for the next call. Child Workspace and Worktree lists return
`ResourceList<T>`. `SandboxListOptions` and `AgentListOptions` add typed filters;
`MarketplaceListOptions` adds search text.

`EventOptions` holds an exclusive durable sequence `after` and optional `limit`.
Sandbox events return `Vec<SandboxEvent>`; Agent events return
`ResourcePage<AgentEvent, u64>`. Persist processed sequence numbers before
advancing a polling cursor. These are bounded durable reads, not live feeds.

Creation returns HTTP 201 resources without claiming readiness. Sandbox
lifecycle and Workspace deletion return HTTP 202 resources. Agent input returns
HTTP 202 with `InputAccepted { queued }`; other Agent commands accept an empty
HTTP 202 response. Agent resources report Sandbox-wide counters, with `None`
when a counter is unavailable.

Keyed owner REST mutations require `&IdempotencyKey`. The constructor accepts 1 to 255 visible
ASCII characters without spaces. Invalid identifiers, zero revision numbers,
and zero Sandbox profile resource allocations are refused before HTTP.
Unknown status or event tags, malformed JSON, wrong resource identities, and
unexpected success statuses are protocol errors. Response bodies are limited
to 8 MiB even when the peer omits `Content-Length`.

Trigger and Launch list responses normalize the backend's `next_cursor` into
`ResourcePage::next`. Launch REST events instead expose their exact numeric
`next_cursor`; `LaunchReplayOptions` configures the owner-only AG-UI companion
at `GET /api/v1/agui/launches/{id}/events`. Stream `cursor()` returns the last
complete event, without implying the application has persisted it.

`Launch::first_message_command` is an optional immutable association containing
`agent_id`, `command_id`, and `response_id`. Owner reads/lists include it only for
an attached common first prompt; legacy Launches and reuse with no new prompt
omit it. Use `responses_v2().read` and the events extension with current Agent
history authority to observe that response. The association grants no authority
and never triggers input submission. Refresh it with `launches().read` rather
than replaying cached admission, and retain the same response after ambiguity.

`ReplayOptions` controls AG-UI finite history or follow mode. `RunRequest` admits
one user message or approval answer. `agui().run_with_key(&request, key)` adds an
optional retained key alongside existing unkeyed `run`; Working ordinary input
queues a Message and `runId` remains correlation only. `AgentEvent::cursor` is set only at a new
durable record boundary; live frames never acknowledge one. `SandboxCompanion`
has typed Sandbox, Workspace, Worktree, and first-message lifecycle snapshots.

Responses uses Agent identity as `conversation`; `CreateResponseRequest` accepts
plain text or exactly one user message with plain text or one `input_text` part.
Streaming and durable storage are required. Standard tools, background work,
stateless generation, and conversation deletion are not invented. Response
event names and connection-local sequences are checked before yielding.
`responses().create_with_key(&request, key)` retains an explicit command key;
existing `create` is unkeyed. Missing/null predecessor means the first turn.
Both keyed legacy helpers accept 1–256 bytes without control characters in a
valid HTTP header and never retry automatically. See [Streaming](streaming.md)
for uncertain admission and previous-server recovery rules.

Badge retrieval returns a typed modified SVG or not-modified result, retaining
the required nonempty ETag. A 304 requires an explicit request validator. Markup
is returned as inert text, not parsed or sanitized for DOM insertion. It sends
no configured bearer credential. Auth and
OpenAPI root paths preserve any configured deployment prefix, as do versioned
API routes. OAuth callback is browser-owned and has no SDK exchange method.

`read_embed_script(&EmbedScriptOptions)` retrieves the supplemental anonymous
`/embed/v1.js` browser asset, retaining exact JavaScript and conditional 304
semantics. It does not execute the script and is not a Layer1 API operation.
Chronological transcript `messages` are required typed entries with sequence,
user/assistant role, content, and append timestamp; preserve this order rather
than concatenating the owner/Agent summary fields.

`read_agent_card()` reads typed public provider discovery without credentials.
`unsupported().agent_card()` remains a compatibility alias and returns the same
card; older servers' named refusals remain typed API errors.

Legacy encrypted Gateway storage methods remain for older installations and migration tooling. Current Agent Manager deployments retire channel/connection/interaction record endpoints; Event Gateway owns its own storage. The compatibility client exposes channel records through `client.gateway_records().put/get/list/delete`. These operations require gateway management authority. `blob` is base64 ciphertext; creation uses `expected_version: null`, and replacement supplies the retained version. They use version preconditions without an Idempotency-Key header.

Provider binding management uses `client.provider_bindings().put/get/delete/ensure_conversation/delete_conversation`. Supply the complete profile snapshot and digest for PUT. These gateway-only operations require both management capabilities; use the scoped binding credential for Responses and immutable event-context reads. Binding responses never contain the supplied credential.

## CMA Event Gateway

`cma_sdk::cmaeg::Cmaeg` is an independent client configured with the gateway origin,
without `/v1`. Methods are `create_channel`, `get_channel`, `recover_channel`,
`update_channel`, `delete_channel`, `test_channel`, `test_channel_source`,
`test_channel_consumer`, `update_channel_routing`, `rekey_channel`, `post_event`,
`stream_event`, `reply_to_event`, `update_message`, `send_typing` and
`read_event_context`. Wire types live in `cma_sdk::cmaeg::types`.

Connection setup uses `get_channel_capabilities`, `create_channel_connection`,
`list_channel_connections`, `get_channel_connection`, and
`cancel_channel_connection`. Use the creator-token client and pass the separate
originating channel agent key to creation. `ConnectionOptions { owner: Some(id) }`
selects an organization for one operation; the default selects the creator.
Reserve and retain the request's `record_id` for durable retries. Listing returns
one page; follow `next_after` even when `items` is empty. Channel creation/update
includes optional `connection_ref` and `connection_admission`. Native Telegram
source, credential and webhook endpoint fields are available in the wire types.

Native bot mentions are an explicit source opt-in. Read
`CapabilityView.source_features` and require `native_bot_mentions_v1` before
selecting `BotMessageAdmission::NativeMentions`; the receiving identity and
consumer's guest permissions and loop limits must also be configured. Older
responses deserialize this field as an empty list, and scoped admission without
`bot_messages` still defaults to denial and serializes without that field.
When upgrading Rust code that constructs these public types directly, add
`bot_messages: Default::default()` to existing `AdmissionPolicy::Scoped` values
and `source_features: Vec::new()` to existing `CapabilityView` values. These are
Rust source construction changes; existing JSON requests and responses remain
compatible.

A channel read with several consumers carries the read-only
`definition.routing` (`RoutingPolicy`), with each consumer's public key hash and
never a key; it is `None` on every other channel.
A routed event's `Activity`, and the `PublicActivity` that `read_event_context`
returns, carry `routing` (`ActivityRouting`): the source slot, how the targets
were resolved, the targets with the primary consumer first and, on a hand-off,
its `Causation`. It is `None` on every unrouted event. Keep an activity you
persist as the JSON you received, so fields this SDK does not know survive.
On a routed channel `ContextResponse.consumer` (`ConsumerSummary`) names the
consumer whose agent key made the call, and `provider_binding_id` is that
consumer's binding; it is `None` on every other channel. A hand-off event reports
`capabilities.buttons` and `capabilities.cards` as `"none"`.

```rust
use cma_sdk::cmaeg::{Cmaeg, types::OutboundMessage};

async fn reply(origin: &str, agent_key: &str, event_ref: &str) -> cma_sdk::Result<()> {
    let gateway = Cmaeg::builder(origin)?.bearer_token(agent_key)?.build()?;
    let receipt = gateway.reply_to_event(event_ref, &OutboundMessage {
        whatsapp_template: None,
        native_attachments: None,
        thread_reply: None,
        text: Some("Received".into()), blocks: None, reply_to: None,
    }).await?;
    // Inspect receipt.delivery and receipt.rendering before claiming delivery.
    let _ = receipt;
    Ok(())
}
```

Configure a separate creator-token client for management, and a dedicated CMA
service-token client for `rekey_channel`. Channel requests contain a redacted
`Secret` wrapper for credential strings. Other gateway documents omit `Debug`
because they can contain opaque channel, event or message references. Never log
those references or credentials.

`create_channel(&body, Option<&str>)` accepts a replica-local idempotency key;
reserve `record_id` in the body for durable creation recovery. After an uncertain
response, retain the complete request and reconcile with `recover_channel`.
Updates require the complete policy and `expected_version`; deletes require the
observed `u64` version. Omitted update credentials retain their values. HTTP
problems use `Error::Api`, preserving `code`, status and retry metadata.

`test_channel_source(id, source_index)` checks saved producer configuration only:
`scope` is `configuration_only` and `external_delivery_tested` is false. It sends
no platform request or message. `test_channel_consumer(id, consumer_id)` runs an
isolated turn for that exact consumer with a 125-second timeout and no external
reply. `update_channel_routing(id, &body)` takes `expected_version` and editable
policy; every consumer ID, order, binding and credential must remain unchanged.
It does not provision or remove consumers, and extended policy writes require
deployment support. An exact retry after lost success can confirm the immediately
following revision; other stale revisions conflict. The SDK never retries it.

`OutboundMessage::native_attachments` requests native image, file, audio or video
delivery from an HTTPS URL or bounded base64. Check
`ContextResponse.capabilities.native_media` first: support is deployment-gated.
There may be at most four attachments and 64 KiB of total decoded inline data
within the existing tool-body limit. Ordinary attachment blocks remain links.
Text edits require the advertised edit capability and a retained message
reference; inspect the update receipt. `verify_raw_event: true` on a Discord or
WhatsApp relay source requests signed raw-event correlation and requires the
deployment's extended-source writer support; omission preserves existing trust.
When constructing these Rust types directly, add `native_attachments: None`,
`native_media: None`, or `verify_raw_event: false` to the respective old literals.
These are source-construction changes; existing JSON remains compatible.

Custom events use structured CloudEvents. `EventOptions::default()` uses the
configured source-key bearer; `EventOptions { webhook: Some(WebhookAuth { ... }) }`
signs the exact serialized event and omits that bearer. `sign_webhook` uses the
UTF-8 secret string directly, never base64-decodes it. The SDK adds a direct
reference to the already-pinned `ring` dependency for HMAC-SHA256.

`GatewayStream::next()` returns `Option<Result<GatewayEvent>>` and checks event
and conversation identity. The stream retains typed terminal failures and the
separate `Backpressure` outcome. EOF before a terminal frame returns a transport
error. Dropping the reader ends observation without cancelling the provider turn;
there is no automatic reconnect or Last-Event-ID replay. The configured request
timeout covers the complete stream. Requests do not follow redirects or retry
uncertain mutations. Context reads retain the full persisted activity; typing and
message-update receipts explicitly report unsupported transports.


## Owner Bots

`client.bots()` exposes `create`, `list`, `get`, `available`, `test`, `disable`,
`enable`, `rotate_keys` and `delete`. Use the current renewable CMA sign-in with
`cma:bot:manage`; independent agent keys cannot borrow the owner's stored login.
The owner must already have registered a personal Telegram bot in NyxID and
connected the profile's explicit services and managed model service. CMA attaches
CMAEG tools natively with the Bot's own agent key; no separate tools catalogue
connection is required.

Creation takes a name, `nyxid_bot_id`, a published Agent profile reference, an
admission policy and a reply policy. Optional `sandbox_profile` and
`workspace_profile` references select personally owned published environments;
each omitted reference pins its platform default. Bot responses include the
pinned environment references (older servers may omit them). Custom profiles
preserve installed software, service access, repository and setup commands.
Retain the creation idempotency key across
uncertain outcomes. HTTP 201 returns the durable Bot in `creating`; poll `get`
for `active` or `failed` and inspect `operation.step` for cleanup progress.
Other mutations return 202 with progress. A deleted Bot becomes 404 only after
its provider binding, channel, routes and owned credentials have been removed.
The `test` method runs a synthetic provider turn and sends no Telegram message.

Bot responses contain no credential bytes. Preserve channel IDs and callback
URLs exactly; never decode, reconstruct or log opaque gateway references.

## Bot Father operations

`client.bot_setups().advance` observes the authenticated source event.
`client.bot_setups().act` accepts `AgentSetupAction`, whose
`BotSetupActionRequest` selects `BotSetupAction::View`, `Draft`, `Choose`,
`Customize`, or `Confirm`. Both operations require the configured Father binding
credential and the original opaque event reference. Retain `request_id` and
unchanged request content across an uncertain retry. Every operation except
`View` also supplies `BotSetupExpected` with the latest setup ID and revision.

`BotSetupView::confirmation_digest` identifies the exact reviewed plan. A
`Confirm` operation forwards that digest; CMA still verifies an attested user
confirmation for that plan. These operations cannot establish a user identity,
replace consent, or borrow another user's credentials. The conversational Agent
chooses when setup help is relevant; ordinary chat needs no setup API call.

## Agent-delegated operations

`client.agent_delegations().identity(&request)` resolves a source actor's linked
identity or browser-consent URL. `operate(&request)` uses that actor's consent for
an allowlisted ordinary CMA operation. Requests carry the current `event_ref` or
CMA callback continuation, local path/method, optional body/owner, and a required
`idempotency_key` for mutations. Inspect `AgentDelegationOperationResult.status`
before treating the inner operation as successful. These provider APIs require
the configured Bot Father binding; they never accept or return user credentials.

## Persistent documents

`client.documents()` exposes `list`, `create`, `read`, `update`, `source`,
`revisions`, `shares`, `grant`, `revoke`, `links`, `create_link`, `revoke_link`,
`comments`, `comment`, and `remove_comment`. Types live in `cma_sdk::documents`.
The registry is independent of Agent and Sandbox lifetimes. `DocumentListOptions`
supports literal title `q`, `scope`, `cursor`, and page `limit` (1–50).

All writes take `&IdempotencyKey`; preserve the original key when retrying an
uncertain result. Versioned mutations take `expected_revision`; source reads
may select an immutable revision. Recipient grants are `Read` or `Comment`.
The optional token on read/comment calls goes only in
`X-CMA-Document-Share-Token` and still requires user authentication.
`create_link` accepts a caller-generated random 256-bit lowercase hex token;
returned metadata never includes it. Keep share tokens out of logs and URL
queries. Link access alone never exposes a user's registry inventory.

## Gateway native cards and relay authoring

### Lark and Feishu native cards

Discover the event context before using interactive blocks. When the operator
enables native Lark cards, eligible contexts return `buttons: "native_card"` and
`cards: "native_card"`; the SDK uses the existing button/choice/card/link message
shapes. Button callbacks return the stored option IDs and selected set, never
arbitrary client callback data. Retain the returned message reference for a
replacement after the click. Disabled, threaded and non-default-consumer contexts
retain their advertised fallback capabilities. Native forms and media mixed with
cards are unsupported; inspect rendering and delivery outcomes. An unknown send
or edit may have reached the platform and must not be blindly retried.

### Approved WhatsApp templates

Approved WhatsApp templates use the optional `whatsapp_template` field on
`OutboundMessage`. Consult `readEventContext` for `capabilities.whatsapp_templates`
before sending. `WhatsappTemplate`, `WhatsappTemplateLanguage`,
`WhatsappTemplateComponent` and `WhatsappTemplateParameter` describe the bounded
name/language and positional text parameters. Templates are exclusive of ordinary
text, blocks, native attachments and thread/reply targets. They reuse the event's
authorized recipient, cannot be edited, and do not enable interactive callbacks
or initiated sends. Templates must already be approved in the WhatsApp account;
see the gateway protocol for exact limits. Omit the field (Rust: `None`) to retain
ordinary message behavior. An unknown receipt must not be retried automatically.

### Relay authoring capabilities

`get_channel_capabilities` also returns optional `relay_authoring`: relay platform verification
modes and their `writes_enabled` status, plus independent raw-verification,
dynamic-source, multi-consumer, native-mention and thread-contract writer gates.
The existing `platforms` list remains native connection setup (Telegram only).
Absent metadata means older discovery, not permission to opt into raw
verification. Retain saved settings without silently switching to normalized
trust. These capabilities do not attest upstream transport health or provide
NyxID bot provisioning. Cache per authenticated owner context, not globally;
the server still validates every write.
