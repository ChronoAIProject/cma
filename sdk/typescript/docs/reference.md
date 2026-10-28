# Reference

<!-- nav: 11 Reference -->

`createClient(config)` returns a `Cma` with the following public resources:

| Resource                                                | Methods                                                                                                                                                                                                                                                                                                                       |
| ------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `agentProfiles`, `workspaceProfiles`, `sandboxProfiles` | `list`, `read`, `create`, `save`, `publish`, `listRevisions`, `readRevision`, `setVisibility`, `retire`                                                                                                                                                                                                                       |
| `profileAuthoring`                                      | `bounds`, `services`, `searchSkills`, read-only `resolveRepository`                                                                                                                                                                                                                                                           |
| `marketplace.agent`, `.workspace`, `.sandbox`           | `list`, `read`, `fork`                                                                                                                                                                                                                                                                                                        |
| `sandboxes`                                             | `list`, `read`, `create`, `pause`, `revive`, `updateRuntime`, `delete`, `events`                                                                                                                                                                                                                                              |
| `workspaces`                                            | `list(sandboxId)`, `read`, `create(sandboxId, body, options)`, `delete`                                                                                                                                                                                                                                                       |
| `worktrees`                                             | `list(workspaceId)`, `read`                                                                                                                                                                                                                                                                                                   |
| `agents`                                                | `list`, `read`, `create(workspaceId, body, options)`, `input`, `steer`, `interrupt`, `answerApproval`, `stop`, `delete`, `events`, `transcript`, `usage`, `resources`, `models`, `settings`, `updateSettings`, `skills`, `installSkill`, `accessRequests`, `approveAccessRequest`, `denyAccessRequest`, `updateServiceAccess` |
| `triggerAuthoring`                                      | `list`, `read`, `create`, `save`, `delete`, `publish`, `retire`, `listRevisions`, `listPlacements`, `createPlacement`, `retirePlacement`, `embed`, `stats`                                                                                                                                                                    |
| `launches`                                              | `preview`, `createOrResume`, `read`, `list`, `start`, `cancel`, `requirements`, `events`, `stream`, `createConnection`, `readConnection`                                                                                                                                                                                      |
| `agui`                                                  | `run`, `replay`, `readCapabilities`, `readSandboxSnapshot`, `streamLaunchEvents`                                                                                                                                                                                                                                              |
| `responsesV2`                                           | `readCapabilities`, `create`, `createForAgent`, `read`, `readEventContext`, `readConversation`, `listConversationItems`, `replay`, `observeControls`, `submitControl`, `interruptWithInput`, `readCommand`, `observeQueuedCancellation`, `cancelQueuedResponse`, `listQueuedInputs`, `editQueuedInput`                        |
| `agentEventsV2`                                         | `readTransportEligibility`, `snapshot`, `events` with acknowledgement after consumer success                                                                                                                                                                                                                                  |
| `responses`                                             | `createConversation`, `readConversation`, `listConversationItems`, `create`, `read`, `delete`, `cancel`                                                                                                                                                                                                                       |
| `auth`                                                  | `readSession`, `readSignupOptions`, `loginUrl`, `logoutUrl`                                                                                                                                                                                                                                                                   |
| `admin`                                                 | `listSandboxes`, `readSandbox`, `sandboxEvents`, `retireSandbox`, `listProfiles`, `retireProfile`, `readSettings`, `replaceSettings`, `patchSettings`, `resetSettings`, `pauseTriggers`, `resumeTriggers`, `readRuntimeInventory`, `listWarmPools`, `configureWarmPool`, `resizeWarmPool`, `listActivity`                     |

`readConformance(options?)` performs `GET /api/v1/conformance` anonymously.
`readOpenApi(options?)` reads the generated `/openapi.json` anonymously, preserving
its schemas, operation annotations and extensions.

`agents.terminal(id)` preserves the named `capability_unsupported` refusal, or
the owner-scoped not-found response. `unsupported.mcp()` and `unsupported.a2a()` probe the explicit
`interface_not_available` responses. `readAgentCard()` reads the typed public
provider card anonymously; `unsupported.agentCard()` remains a compatibility
alias and now returns that card. Older servers' named refusals remain `ApiError`.
These helpers reject with `ApiError`; they do not provide a terminal or implement
either protocol. An unexpected successful response is a `ProtocolError`.

`launches.list({ trigger, state, cursor, limit })` filters the visitor's Launches
by Trigger id and the closed `LaunchState` enum. Its `next` cursor is opaque.

`triggers.badge(publicId, { label, theme, placement, ifNoneMatch })` reads an
anonymous SVG or a conditional `304` cache result with its entity tag.
`triggers.embedScript()` reads the public script as text without executing it.
Both suppress bearer credentials and cookies and reject redirects.

`triggers` is an alias of `triggerAuthoring`. The `admin` namespace requires
explicit operator capabilities. Settings replace, patch and reset carry
`expected_revision`; a patch supplies at least one complete section and leaves
omitted sections unchanged. Null sections and empty patches are refused. Neither
settings commands nor Trigger pause/resume invent an idempotency key or retry.

`readDefaults(options?)` performs the authenticated `GET /api/v1/defaults` and
returns only `sandbox_profile`, `workspace_profile`, and `agent_profile`, each
an immutable `{ id, revision }` reference. It requires `use_own_sandboxes`.
Read those exact revisions with the matching profile client's `readRevision`;
never infer configured defaults from Marketplace listings or replace a pinned
revision with the latest. Immutable revision reads admit the owner's profiles
and non-retired public profiles; draft access remains owner-management-only.

```ts
import { type Conformance, readConformance, resolveConfig } from '@chronoai/cma-sdk';

const config = resolveConfig({ baseUrl: 'https://cma.example.com' });
const deployment: Conformance = await readConformance(config, {
  requestId: 'deployment-check-1',
});
```

`Conformance` contains `service`, `status`, and an array of currently exposed
`interfaces`. Generic transport helpers remain available for diagnostics and
SDK development. Resource methods validate required fields, closed enums,
nonnegative safe integers and exact success statuses. Unknown resource-response
fields are omitted; an unknown discriminator in a closed resource-event union is
a `ProtocolError`. AG-UI preserves extension fields as described below.

Sandbox and Workspace creation return base views with HTTP 201. Use `read` for
expanded hierarchy detail. Their lifecycle commands return base views with
HTTP 202; acceptance does not mean the lifecycle has completed. Agent operations
likewise return HTTP 202, with only `input` carrying `{ queued: boolean }`.

Profile `save` and `publish` require the last observed `draft_version` for
compare-and-swap. `marketplace.*.read` returns a complete immutable revision and
resolved nested revisions; Sandbox entries also carry trust confirmation and
the commands digest. `fork` requires an explicit revision, keeping a retry
pinned to the revision the owner actually inspected.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
const draft = await cma.agentProfiles.read('agp_default');
console.log(draft.draft.thinking_effort, draft.draft_version);

const accepted = await cma.agents.create(
  'workspace-id',
  {
    agent_profile: { id: 'agp_default', revision: 1 },
    first_message: 'Review the current changes.',
  },
  { idempotencyKey: 'create-agent-command-1' },
);
console.log(accepted.id, accepted.first_message?.state);
```

Examples use illustrative identifiers and command keys. A real caller supplies
an existing resource identifier and a fresh durable key for each new logical
command. It retains that key and the exact body until the outcome is settled.

Agent `resources` keeps unavailable numeric readings as `null`; missing
`context_window` in usage is distinct from a reported zero. Event and transcript
DTOs are discriminated unions. Durable histories do not manufacture live
reasoning or replace the public conversation stream.

## Trigger Authoring

`triggerAuthoring` manages the publisher's Draft and immutable publications. Every
target is an exact profile `{ id, revision }`, and every target profile must be
public at publication. Saving a Draft never changes the published Trigger. The
server validates reuse combinations, limits and publication eligibility; keep
typed field refusals beside the edited field.

```ts
import { createClient, type TriggerContent } from '@chronoai/cma-sdk';

const cma = createClient();
export async function saveEditedTrigger(
  id: string,
  content: TriggerContent,
  observedVersion: number,
  commandKey: string,
) {
  return cma.triggerAuthoring.save(
    id,
    { content, draft_version: observedVersion },
    { idempotencyKey: commandKey },
  );
}

export async function publishReviewedTrigger(id: string, savedVersion: number, commandKey: string) {
  return cma.triggerAuthoring.publish(
    id,
    { draft_version: savedVersion },
    { idempotencyKey: commandKey },
  );
}
```

Save returns a `TriggerDocument`; Publish returns a `PublishedTriggerRevision`
with the publication's `trigger_id`, `public_id`, revision, digest and content.
Use the Save result's `draft_version` for the separate Publish command. Retain
that version and the Publish key if its outcome is unknown.

Placements are stable identities, separate from their HTTPS `source_url` and
human `label`. `listPlacements(id, { includeRetired: true })` includes retired
records. `retirePlacement(id, placementId, options)` retires a placement without
rewriting its historical attribution.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
export async function addReviewedPlacement(triggerId: string, commandKey: string) {
  const placement = await cma.triggerAuthoring.createPlacement(
    triggerId,
    { source_url: 'https://example.com/project', label: 'Project README' },
    { idempotencyKey: commandKey },
  );
  const embed = await cma.triggerAuthoring.embed(triggerId, placement.id);
  return { placement, embed };
}
```

The embed response owns every Markdown, HTML, JavaScript, React and URL snippet,
plus `javascript_sri` and the `content_security_policy.img_src` / `script_src`
directives. Display and copy these exact values; do not reconstruct them from
the public id. `stats` reports totals and placement attribution. A failed stats
read is unavailable data, never an observed zero.

### React Button

The descriptor's React snippet uses the optional peer-dependent React entry.
It renders only an anchor and image, makes no API call, and is safe during SSR:

```tsx
import { CmaTriggerButton } from '@chronoai/cma-sdk/react';

export const publishedButton = (
  <CmaTriggerButton
    triggerId="pub_example"
    placementId="plc_readme"
    label="open_in_cma"
    theme="auto"
    baseUrl="https://cma.example"
  />
);
```

`triggerId` is the public id, not the publisher's private Trigger id. Optional
`placementId` controls attribution; without it the link uses `/t/{publicId}`.
`label` is `open_in_cma`, `chat_with_this`, `run_with_cma`, or `try_it`; `theme`
is `auto`, `light`, or `dark`. Their defaults are `open_in_cma` and `auto`.
`baseUrl` must be an absolute HTTP(S) origin with no credentials, path, query or
fragment; its default is `https://bot.chrono-ai.fun`. Prefer the exact supplied
descriptor props for non-production deployments. Package checks compile and
render the backend-generated snippet against the packed SDK.

## Visitor Launches

A public GET does not imply consent. Call admission only from a visitor's explicit
gesture or its already-saved bounded intent after sign-in:

```ts
import { createClient, repeatChoiceFrom, type CreateLaunchRequest } from '@chronoai/cma-sdk';

const cma = createClient();
export async function admitVisitorIntent(
  publicId: string,
  body: CreateLaunchRequest,
  commandKey: string,
) {
  try {
    return {
      record: await cma.launches.createOrResume(publicId, body, { idempotencyKey: commandKey }),
    };
  } catch (error) {
    const existing = repeatChoiceFrom(error);
    if (existing !== undefined) return { existing };
    throw error;
  }
}
```

The visitor command uses `start_if_ready: true`, optional `placement_id` and
`referrer_origin`, and an optional explicit `choice: 'resume' | 'new'`. The SDK
accepts HTTP 201 and 202 and returns a `Launch` for both. Inspect `state` and
`review.needs`: Draft opens only its requested review, while accepted work opens
progress. Repeated admission with the same key returns the recorded response;
it is not a way to refresh or advance an existing Draft.

Owner Launch reads and lists expose optional `first_message_command` with
`agent_id`, `command_id`, and `response_id` for an attached common first prompt.
Legacy Launches and reuse with no new prompt omit it. Use the response ID with
`responsesV2.read` or the events extension under current Agent history
authority; reading the association never submits input or grants authority.
Refresh with `launches.read` after an old cached admission response. Retain the
same response when delivery is ambiguous and never infer resend permission from
an absent association.

After explicit trust review or satisfied account setup, continue that same Draft
with `start(id, { trust_confirmed_digest, acknowledgements }, options)`. Show the
recorded commands and environment values verbatim, and submit the exact digest
the visitor approved. Start returns HTTP 202 and refreshes authoritative account
requirements. `requirements(id)` is a read with optional `display_name`,
`connection_mode` and `setup_url`; it does not itself advance the stored Launch.

For hosted setup, open a blank popup synchronously from the gesture, then call
`createConnection({ service_id, launch_id, intent_id }, options)`. Validate its
HTTPS `hosted_url` before navigating the popup. Read `readConnection(handoff_id)`
with bounded polling, verify the Launch and service identities, and continue via
Start only when its state is `satisfied`. `expired`, `cancelled` and `failed` are
separate outcomes. A manual or unknown `connection_mode` does not authorize
inventing a hosted handoff; use only the supplied manual setup metadata.

JSON progress pages come from `events(id, { after, limit })`, returning
`{ launch_id, events, next_cursor }`. The SSE companion uses the same durable
sequence but delivers individual `LaunchEvent` values. Follow with a bounded
signal, retaining an acknowledged cursor and last successful record:

```ts
import { createClient, ProtocolError, type Launch } from '@chronoai/cma-sdk';

const cma = createClient();
export async function followPreparation(
  id: string,
  signal: AbortSignal,
  showRecordedProgress: (record: Launch) => Promise<void>,
) {
  let record = await cma.launches.read(id, { signal });
  await showRecordedProgress(record);
  await cma.launches.stream(id, {
    after: record.event_head,
    signal,
    onEvent: async (event) => {
      const next = await cma.launches.read(id, { signal });
      if (next.event_head < event.seq)
        throw new ProtocolError('Progress snapshot is behind its event');
      await showRecordedProgress(next);
      record = next;
    },
  });
  return record;
}
```

`stream` makes one connection. Supply bounded reconnect and polling fallback in
the host application; do not acknowledge an event until its snapshot or durable
projection has actually been applied. A `ready` record's Agent resolution gives
the visitor's Agent id. Navigate there without submitting the first message again.
`cancel(id, options)` is an explicit command; `failure.retryable` is not a public
retry operation. Retryable worker work resumes under server control. Starting
over requires a new explicit intent and new key.

## Agent Conversations

`agui.run` posts once to `/api/v1/agui/run`, and `agui.replay` reads
`/api/v1/agui/events`. In both, `threadId` is an existing Agent id, not a new Chat
identity. A run request supplies user messages or resolved approval interrupts.
`AgentRunOptions.idempotencyKey` is optional; retain it with the same input for
explicit recovery. Ordinary Working input queues a Message. `runId` remains
correlation only, and the request/key are captured before credentials resolve.

```ts
import { createClient, type AgentConversationEvent } from '@chronoai/cma-sdk';

const cma = createClient();
export async function sendExplicitMessage(
  agentId: string,
  text: string,
  signal: AbortSignal,
  apply: (event: AgentConversationEvent) => Promise<void>,
) {
  await cma.agui.run(
    { threadId: agentId, messages: [{ role: 'user', content: text }] },
    { signal, onEvent: apply },
  );
}

export async function recoverConversation(
  agentId: string,
  acknowledgedCursor: number,
  signal: AbortSignal,
  apply: (event: AgentConversationEvent) => Promise<void>,
) {
  await cma.agui.replay(agentId, {
    after: acknowledgedCursor,
    follow: true,
    maxReconnects: 4,
    signal,
    onEvent: apply,
  });
}
```

Frames preserve protocol `type`, `threadId`, `runId` and extension fields. The
SDK validates frame identity, SSE framing and cursor boundaries, but does not
replace an application's event-specific reducer: extension values are `unknown`.
One durable event may project several frames; only its final frame carries
`cursor`. The SDK buffers that projection and advances replay only after all
callbacks resolve. Store a cursor only after applying the corresponding frame.
Live frames carry `cma:live: true` and no durable cursor; they are supplemental
content and never replace recorded history.

`replay(agentId, { follow: false, onEvent })` reads a finite captured history.
Following replay reconnects with bounded
backoff, defaults to four consecutive no-progress reconnects, and accepts a limit
from zero through twenty. A successful durable acknowledgement resets that
count. `run` never reconnects its POST. `timeoutMs` bounds admission and response
headers, not an active AG-UI response body; use `signal` to bound the entire
operation. Replay transport failure does not change the durable run outcome.

`readCapabilities()` returns the deployment's typed AG-UI feature declaration.
`readSandboxSnapshot(agentId)` returns the same Agent's Sandbox, Workspace,
Worktree and first-message state and rejects a mismatched thread identity.
`streamLaunchEvents` uses the visitor Launch companion: optional `after`,
`limit` (1-200) and `follow: false` capture one bounded page. The default follows
until disconnect or caller cancellation; reconnect is caller-owned.

## Responses

`responses` projects the same owned Agent conversation. `createConversation`
accepts optional `metadata` identifiers `agent_profile`, `workspace`, and
`sandbox`; it creates an Agent context, not a separate Chat aggregate.
`readConversation` and `listConversationItems` read its durable projection.
Items are newest-first, with `after`, `limit` (1-100), nullable `first_id` and
`last_id`, and `has_more`. Provider tool activity uses `cma_chat.tool_activity`,
not an invented standard function call.

```ts
import { createClient, type ResponseEvent } from '@chronoai/cma-sdk';

const cma = createClient();
export async function sendResponse(
  agentId: string,
  text: string,
  signal: AbortSignal,
  apply: (event: ResponseEvent) => Promise<void>,
) {
  return cma.responses.create(
    { conversation: agentId, input: text, stream: true },
    { signal, onEvent: apply },
  );
}
```

`create` submits once and returns the terminal `ResponseObject` after each
callback has acknowledged its frame. `failed` and `incomplete` are authoritative
Agent outcomes; an interrupted transport throws without inventing either one.
The connection-local sequence starts at zero; it is not a durable replay cursor.
`timeoutMs` bounds only admission and response headers (605 seconds by default);
use `signal` to bound the complete operation. Never resubmit a disconnected POST
automatically. Recover with durable `read(responseId)` or the Agent history.

The subset requires `conversation`, exactly one nonempty user text message, and
`stream: true`. `ResponseStreamOptions.idempotencyKey` optionally retains one
invocation for explicit recovery; existing calls stay valid. An absent/null
`previous_response_id` requires the first turn; otherwise it enforces a linear prior response;
`model` is accepted but cannot override the pinned Agent Profile. `store: false`,
background execution, tools and instruction overrides are unsupported. The
explicit `delete` and `cancel` helpers preserve the server's
`responses_field_unsupported` problem as `ApiError`; neither operation is
advertised as a supported Agent action.

Legacy encrypted Gateway storage methods remain for older installations and migration tooling. Current Agent Manager deployments retire channel/connection/interaction record endpoints; Event Gateway owns its own storage. The compatibility client exposes channel records through `client.gatewayRecords.put/get/list/delete`. These operations require gateway management authority. `blob` is base64 ciphertext; creation uses `expected_version: null`, and replacement supplies the retained version. They use version preconditions without an Idempotency-Key header.

Provider binding management uses `client.providerBindings.put/get/delete/ensureConversation/deleteConversation`. Supply the complete profile snapshot and digest for PUT. These gateway-only operations require both management capabilities; use the scoped binding credential for Responses and immutable event-context reads. Binding responses never contain the supplied credential.

## CMA Event Gateway

`Cmaeg` is an independent client configured with the gateway origin, without a
`/v1` suffix. Its operations are `createChannel`, `getChannel`, `recoverChannel`,
`updateChannel`, `deleteChannel`, `testChannel`, `testChannelSource`,
`testChannelConsumer`, `updateChannelRouting`, `rekeyChannel`, `postEvent`,
`streamEvent`, `replyToEvent`, `updateMessage`, `sendTyping` and `readEventContext`.
Gateway wire types and `signWebhook` are exported under `cmaeg`.

Native Discord thread context is opt-in. When ordinary context advertises
`capabilities.thread_history: true`, call
`readEventContext(eventRef, { includeThreadHistory: true })` to read at most 20
text messages strictly before that event in the same thread. The optional
`thread_history` includes author/timestamps and attachment counts, without
download links or message action references. There is no caller-supplied channel,
pagination or history backfill; ordinary calls make no Discord history request.
The gateway gate and native source's `thread_history: true` must both be enabled.

Connection setup uses `getChannelCapabilities`, `createChannelConnection`,
`listChannelConnections`, `getChannelConnection`, and `cancelChannelConnection`.
Use the creator-token client and supply `{agentKey, owner?}` to creation: the
separate agent key must authorize the originating `event_ref`. Reserve the
request's `record_id` before creation and retain it for retries. Owner selection
is per operation; omit `owner` for personal resources. Listing accepts `after`
and returns one page; follow `next_after` even when `items` is empty. Channel
creation/update accepts `connection_ref` and `connection_admission`. Native
Telegram source, credential and webhook endpoint fields are typed as well.
`listRelayResourceConnections('lark' | 'feishu')` reads optional existing personal
and eligible shared organization bot API connections for the current credential subject. It returns `items` with
`user_service_id`, `catalog_service_id`, `name`, `platform` and `source` (`personal`
or `organization`); there is no owner override or credential creation. An empty list does not invalidate saved/manual
bindings. Keep receiving app/bot identity explicit and grant the channel key
access separately. Older gateways may return 404; unavailable inventory is an
error rather than an empty successful list.
A channel read with several consumers carries the read-only
`definition.routing` (`cmaeg.RoutingPolicy`), with each consumer's public key
hash and never a key; it is absent on every other channel. A routed event's
`cmaeg.Activity`, and the `cmaeg.PublicActivity` that
`readEventContext` returns, carry `routing` (`cmaeg.ActivityRouting`): the source
slot, how the targets were resolved, the targets with the primary consumer first
and, on a hand-off, its `cmaeg.Causation`. It is absent on every unrouted event.
Keep an activity you persist as the JSON you received, so fields this SDK does
not know survive. On a routed channel `cmaeg.ContextResponse` carries `consumer`
(`cmaeg.ConsumerSummary`), the consumer whose agent key made the call, and its
`provider_binding_id` is that consumer's binding; `consumer` is absent on every
other channel. A hand-off event reports `capabilities.buttons` and
`capabilities.cards` as `"none"`.

```ts
import { Cmaeg, type cmaeg } from '@chronoai/cma-sdk';

export async function reply(
  gatewayOrigin: string,
  agentKey: string,
  eventRef: string,
  text: string,
) {
  const gateway = new Cmaeg({ baseUrl: gatewayOrigin, credential: () => agentKey });
  const message: cmaeg.OutboundMessage = { text };
  return gateway.replyToEvent(eventRef, message);
}
```

Use a separate creator-token client for management. `createChannel` accepts an
optional `{idempotencyKey}`; reserve `record_id` in its body for recovery across
replicas. Keep the complete request after uncertainty and reconcile with
`recoverChannel(recordId)`. `updateChannel` replaces the complete policy and
requires `expected_version`; `deleteChannel(id, expectedVersion)` sends that
version in the query. Omitted/null update credentials retain their current values.
A stale mutation throws `ApiError` with `code === 'conflict'`.

`testChannelSource(id, sourceIndex)` checks the saved producer configuration;
it reports `scope: 'configuration_only'` and `external_delivery_tested: false`.
It does not contact the platform or send a message. `testChannelConsumer(id,
consumerId)` runs an isolated provider turn for that exact consumer, with a
125-second timeout and no external reply. `updateChannelRouting(id, body)` takes
`expected_version` and editable routing policy while preserving every consumer
ID, order, binding and credential. Adding or removing consumers is not supported
by this operation. Extended policy writes require deployment support. After a
lost success, an exact retry can confirm the immediately following revision;
other stale revisions return a conflict. The SDK never retries mutations itself.

`getChannelActivity(id, {owner?})` returns a typed `cmaeg.ActivityPage` of recent
events, consumer processing states and observed transport deliveries. The page
advertises its retention window, item limit and `has_more`; it is a bounded view,
not a complete audit log. `occurred_at` is normalized event time: the source
timestamp when supplied, or gateway normalization time for custom CloudEvents
without `time`. The original CloudEvent's `time` stays absent in that case;
`occurred_at` alone does not establish a source timestamp. `received_at` is gateway
receipt time. A consumer's completed processing state does not itself prove
delivery: inspect each delivery's `outcome`, `recipients`, `observed_at` and
optional confirmed receipt timestamp.

`streamChannelActivity(id, {owner?, cursor?, lastEventId?, signal?})` is an async
iterator of `cmaeg.ChannelActivityEvent`. Both `activity` and `reset` events carry
a complete `ActivityPage` in `data`; replace the prior snapshot atomically. Each
connection starts with a snapshot. `reset` indicates expired retention or missed
notifications, and also replaces the entire snapshot. The SDK validates that the
explicit SSE `id` equals the snapshot cursor before yielding it. Retain only the
cursor of the snapshot your application successfully applied, then pass it to a
new connection as `cursor` or `lastEventId` (`Last-Event-ID` takes precedence).
The SDK never reconnects automatically. Clean EOF ends the connection; truncated
frames fail. `timeoutMs` bounds the handshake, while `signal` cancels observation
of the active stream. The gateway also closes each connection after five minutes.
Owner selection applies to only that request and never changes client credentials.

`OutboundMessage.native_attachments` explicitly requests native image, file,
audio or video delivery using an HTTPS URL or bounded base64. Read event
`capabilities.native_media` before using it; deployment support must be enabled.
The limit is four attachments and 64 KiB of total decoded inline data within the
existing tool-body limit. Ordinary attachment blocks remain links. Exact relay
text editing also depends on deployment support and a retained message reference;
inspect the event's edit capability and the update receipt rather than assuming
every platform can edit every message. For Discord and WhatsApp sources,
`verify_raw_event: true` opts into signed raw-event correlation and requires the
deployment's extended-source writer support; omission preserves existing trust.

An enabled native Discord upload can return an opaque `message_ref` for delete
and own-reaction effects when the complete upload fits one confirmed Discord
message. This reference does not authorize text or attachment updates. Query its
message-effect capabilities and inspect effect receipts. Several native messages
or uncertain delivery produce no upload reference; several files in one message
can share one reference. The SDK types and method signatures are unchanged.

Native Telegram can likewise return an effects-only reference for one confirmed
individual upload when its separate media-effects gate is enabled. A caption on
that upload is allowed, but top-level text plus an upload or several attachments
produce several native messages and no aggregate media reference. Caption/media
editing and album effects remain unsupported.

`postEvent` and `streamEvent` accept a structured `cmaeg.CloudEvent` plus optional
`{webhook:{secret,id,timestamp}}`. The timestamp is decimal Unix seconds; the
configured base64 secret string is used as UTF-8 without decoding it. The client
snapshots the event before signing and sends those exact JSON bytes. HMAC requests
omit creator credentials and cookies. `cmaeg.signWebhook(secret,id,timestamp,body)`
returns the three Standard Webhooks headers for caller-owned exact body bytes.

`streamEvent` is an async iterator of `GatewayStreamEvent`. It preserves delta,
message, completed, failed and terminal `sync_backpressure` outcomes, validates
stable event/conversation identity, and treats EOF before completion as uncertain.
`timeoutMs` bounds the complete stream, and `signal` cancels observation. Neither
cancellation nor early iterator return cancels the admitted provider turn. Streams
have no Last-Event-ID replay and are never automatically retried or reconnected.

Replies and updates return delivery/rendering receipts; inspect `delivery.outcome`
and `rendering` before claiming delivery or native UI support. Typing can return
`unsupported`. Event context contains the full persisted activity. Channel, event
and message references remain opaque and must not be logged. No redirect or
automatic mutation retry is performed. Unknown response properties are ignored;
CloudEvents extension attributes are retained.

## Owner Bots

`client.bots` exposes `create`, `list`, `get`, `connections`, `available`, `test`, `disable`,
`enable`, `rotateKeys` and `delete`. Use the current renewable CMA sign-in with
`cma:bot:manage`; independent agent keys cannot borrow the owner's stored login.
Use `connections()` to list ready Event Gateway connections as
`{connection_ref,label}`. Complete channel setup in Event Gateway and connect
the selected Agent configuration’s services and managed model service. CMA attaches
CMAEG tools natively with the Bot's own agent key; no separate tools catalogue
connection is required.

Creation takes a name, `connection_ref`, a published Agent profile reference, an
admission policy and a reply policy. The legacy `nyxid_bot_id` remains accepted
for existing integrations; provide exactly one connection identifier. A Gateway
connection keeps platform credentials and webhook management within the Gateway. Optional `sandbox_profile` and
`workspace_profile` references select personally owned published environments;
each omitted reference pins its platform default. Bot responses include the
pinned environment references (older servers may omit them). Custom profiles
preserve installed software, service access, repository and setup commands.
Retain the creation idempotency key across
uncertain outcomes. HTTP 201 returns the durable Bot in `creating`; poll `get`
for `active` or `failed` and inspect `operation.step` for cleanup progress.
Other mutations return 202 with progress. A deleted Bot becomes 404 only after
its provider binding, channel, routes and owned credentials have been removed.
The `test` method runs a synthetic provider turn and sends no external message.

Bot responses contain no credential bytes. Preserve channel IDs and callback
URLs exactly; never decode, reconstruct or log opaque gateway references.

## Bot Father operations

`client.botSetups.advance` reads an attested source event and returns the current
setup view. `client.botSetups.act` lets the conversational Agent select a typed
`view`, `draft`, `choose`, `customize`, or `confirm` operation. Both require the
configured Bot Father binding credential and the original opaque `event_ref`.
Operations carry `request.request_id` for replay protection; every operation
except `view` also needs `request.expected` with the latest `setup_id` and
`revision`. Preserve the same request ID and content when retrying an uncertain
result. A changed revision requires reading the current view and making a new
decision.

The view's `confirmation_digest` identifies the exact reviewed plan. Send that
digest in a `confirm` action only after the user has confirmed that plan. CMA
independently checks the attested user confirmation; an Agent-selected action
cannot replace consent, establish a user identity, or borrow a different user's
credentials. Ordinary chat does not require calling either setup operation.

## Agent-delegated operations

`client.agentDelegations.identity({ event_ref })` resolves a signed-in source
actor or returns CMA's browser-consent URL. `client.agentDelegations.operate()`
accepts the same event capability, a canonical local `path`, `method`, optional
`body` and optional resource `owner`. Mutations require `idempotency_key`.
These methods require the configured Bot Father binding credential; they do not
accept a user token or claimed grant ID. The operation result is `{ status, body }`;
inspect the inner status before treating a mutation as successful. A CMA callback
continuation is accepted only by CMA APIs, never gateway reply tools.

## Persistent documents

`client.documents` exposes `list`, `create`, `read`, `update`, `source`,
`revisions`, `shares`, `grant`, `revoke`, `links`, `createLink`, `revokeLink`,
`comments`, `comment`, and `removeComment`. Documents remain available after
an originating Agent or Sandbox is removed. Listing supports `q`, `scope`
(`owned`, `shared`, `all`), `cursor`, and `limit` (1–50).

Every mutation requires `idempotencyKey`; retain it when retrying an uncertain
result. Owner edits and grant changes carry `expected_revision`. Source reads
accept an optional immutable source revision. Authorized recipients can read,
or comment when their permission is `comment`. Optional `shareToken` is sent
only in `X-CMA-Document-Share-Token`, alongside the user's authentication; it is
never appended to a URL. The caller generates a random 256-bit token for
`createLink` and keeps it in the application URL fragment. CMA returns link
metadata without the token. Revocation is effective on subsequent source reads,
including downloads that were in progress when authority was revoked.

## Gateway native cards and relay authoring

### Lark and Feishu native cards

Discover the event context before using interactive blocks. When the operator
enables native Lark cards, eligible contexts return `buttons: "native_card"` and
`cards: "native_card"`; the SDK uses the existing button/choice/card/link message
shapes. Button callbacks return the stored option IDs and selected set, never
arbitrary client callback data. Retain the returned message reference for a
replacement after the click. Named consumers require the operator's separate
consumer-interaction gate and preserve their original binding and conversation.
Disabled or threaded contexts retain their advertised fallback capabilities. Native forms and media mixed with
cards are unsupported; inspect rendering and delivery outcomes. An unknown send
or edit may have reached the platform and must not be blindly retried.

### Separate native Lark / Feishu source

`NativeLarkSource` requests `type: "native_lark"`, `separate_app: true`, an
`identity` (`NativeLarkIdentity`) and an admission policy. The identity binds
`region: "lark" | "feishu"`, `app_id`, `tenant_key` and `bot_open_id`.
`NativeLarkCredentials` supplies the atomic `app_secret`, `encrypt_key` and
`verification_token` object through `credentials.native_lark`; public source
objects never contain those secrets. Discover the optional
`native_lark_writes_enabled` and `native_lark_intake_enabled` authoring capability
flags before setup. Both default off and require the operator's durable-routing
prerequisites. Creation requires a stable canonical UUID v4 `record_id` and
returns `endpoints.native_lark_webhook_url`.

Use a separately provisioned app and its webhook; replacing an existing NyxID
app's webhook would redirect its traffic. This source supports native text
replies, retained recall notifications with unknown actors, and human reaction
add/remove deltas. `SourceKind` carries the same regional app/tenant/bot identity.
Targets retain the original conversation and consumer authority; these incoming
notifications confer no outgoing delete/edit/reaction authority. Incoming edit
propagation remains unsupported; original-conversation history has separate consent.
Native received personal/group cards and locations become bounded message text
without changing the authenticated sender, conversation, admission or reply target.
Shared identities do not grant directory, group or attachment access. Live platform
acceptance remains separate from local contract tests. See the repository's
native Lark setup document and preserved incoming-event audit for provisioning
and limits.

Native interactive replies have separate optional management discovery flags,
`native_lark_controls_enabled` and `native_lark_forms_enabled`, both default off.
The retained event context advertises `buttons` and `cards` as `native_card`,
`max_text_chars: 4000`, and the optional `forms` boolean when native controls are
available. Configure `card.action.trigger` in the separate app's **Callback
configuration**, using the encrypted native webhook URL; this is separate from
message event subscriptions. Do not subscribe to the legacy callback version.

Use the existing outbound blocks and form fields. Controls retain the original
human actor and exact private/group conversation, including an existing group
thread when its original native authority is retained. Both default and named
consumers retain their exact original owner. A confirmed single interactive
message returns a stable `message_ref` for PATCH replacement, recall and one
reaction. Replacements preserve the original 24-hour lifetime; changing app
credentials, source, routing, consumer key or provider revokes stale authority.
Unknown sends or updates never produce replacement authority and must not be
blindly retried. Gate-off stops new issuance while current retained references
and durable callback recovery remain readable. Media mixed with cards remains
unsupported; inspect rendering, delivery and message capability results.

### Native WhatsApp image, PDF, audio, sticker and video replies

Discover `capabilities.native_media` before using `native_attachments`. Native
WhatsApp has an independent default-off gate and supports JPEG/PNG `image`,
PDF `file` and bounded `audio` attachments. Audio supports AAC-LC ADTS (`audio/aac`,
`.aac`), AMR narrowband (`audio/amr`, `.amr`), MPEG Layer III (`audio/mpeg`, `.mp3`),
single-track self-contained AAC-LC M4A (`audio/mp4`, `.m4a`) and mono Opus Ogg
(`audio/ogg`, `.ogg`). Audio captions are refused; text is a separate message,
and voice-note presentation is not exposed. The additive optional `mime_types`, `max_total_bytes`,
`max_image_bytes` and `max_caption_chars` fields report that exact subset: 8 MiB
aggregate bytes, 5,000,000 bytes per image and 1024 caption characters; shared
metadata still limits captions to 1024 UTF-8 bytes. Existing limits remain four
attachments and 65536 decoded inline bytes. Public HTTPS downloads use bounded
address validation. Matching MIME type and filename extension are required when
provided; images must have 8-bit RGB/RGBA headers. Audio undergoes bounded
container/codec checks without a runtime decoder; those checks do not certify
complete payload decodability. Other document formats remain unavailable. The original event fixes the recipient
and quoted message; clients cannot provide a Meta media ID. Confirmed replies
record exact target receipts, while uncertain/partial sends must not be retried
automatically. Actual Meta/client acceptance remains separate from local tests.

The additive `sticker` kind requests static or animated `image/webp` presentation;
`.webp` is the matching extension. `native_media.sticker` (`NativeStickerCapabilities`)
reports a 512×512 canvas, 100000 static/500000 animated bytes, at most 128 frames,
10000 ms per cycle and at least 8 ms per frame. Captions are refused; accompanying
text stays separate. Files undergo bounded container/frame checks without pixel
decoding. The sticker message carries only the upload result ID. Unsupported
native transports and all NyxID/initiated paths refuse this kind before any send;
legacy capabilities omit the optional `sticker` object.

The `video` kind accepts `video/mp4` (`.mp4`) or `video/3gpp` (`.3gp`), with
captions. Optional `native_media.video` (`NativeVideoCapabilities`) declares
`video_codec: "h264"`, `video_profile: "baseline"`, `audio_codec: "aac_lc"`,
`max_audio_tracks: 1`, `max_width: 1920`, `max_height: 1080`, `max_frame_rate: 60`,
`max_duration_ms: 600000`, `max_samples: 65536` across tracks,
`progressive_only: true`, `max_slices_per_frame: 1`, and `fast_start_required: true`.
The container must be self-contained, unencrypted and nonfragmented, with movie
metadata before media data. A silent video is allowed. The deliberately bounded
profile requires one SPS/PPS and I/P slice headers; it refuses B-frames, unsupported
coding modes and overlapping/invalid sample ranges. These structural checks do not
fully decode media or establish Meta/client acceptance. The message carries the
upload ID and caption only. See the repository's native WhatsApp outbound-media
document for the complete accepted syntax. Legacy discovery omits the optional `video` profile object.

### Approved WhatsApp templates

Approved WhatsApp templates use the optional `whatsapp_template` field on
`OutboundMessage`. Consult `readEventContext` for `capabilities.whatsapp_templates`
before sending. Native private event replies and NyxID relay replies have separate
operator gates; native templates require current native source custody and use
the original sealed recipient/context. Template media IDs and HTTPS links are
independent of native upload capability. `WhatsappTemplate`, `WhatsappTemplateLanguage`,
`WhatsappTemplateComponent` and `WhatsappTemplateParameter` describe the template
name/language and bounded positional parameters. Header parameters can be text
or typed image/video/document references; body parameters can be text, currency
or a date/time fallback label. URL buttons accept a text suffix only. Templates
are exclusive of ordinary text, blocks, native attachments and thread/reply targets.
Event replies retain the event's authorized recipient; owner-initiated sends use
their separately admitted saved route. Templates cannot be edited and grant no
interactive callback authority. The template capability alone does not enable
initiated sends. Templates must already be approved in the WhatsApp account;
see the gateway protocol for exact limits. Omit the field (Rust: `None`) to retain
ordinary message behavior. An unknown receipt must not be retried automatically.

### Relay authoring capabilities

`getChannelCapabilities` also returns optional `relay_authoring`: relay platform verification
modes and their `writes_enabled` status, plus independent raw-verification,
dynamic-source, multi-consumer, native-mention and thread-contract writer gates.
The existing `platforms` list remains native connection setup (Telegram only).
Absent metadata means older discovery, not permission to opt into raw
verification. Retain saved settings without silently switching to normalized
trust. These capabilities do not attest upstream transport health or provide
NyxID bot provisioning. Cache per authenticated owner context, not globally;
the server still validates every write.

### WhatsApp reply buttons and lists

Eligible raw-verified private contexts advertise `buttons: "native_interactive"`
after the gateway operator enables WhatsApp interactions. Use existing
`OutboundMessage.blocks`: one `buttons` block (1–3 options, labels at most 20
characters) or one `choice` with `multi: false` (1–10 options, labels at most 24
characters). Optional text is limited to 1,024 characters including the choice
prompt. Multiple controls, multi-select, cards/forms/links, native media and
threaded replies are refused. Check the returned rendering report for plain
style/preselection degradation. There are no new SDK types or request fields.

Only the exact original sender can consume a confirmed message's controls.
Callbacks return the existing interaction activity with stored action IDs and
selected values. They do not trust selection titles. Repeated delivery keeps
one event/idempotency identity; a new choice requires a new message because
WhatsApp `updateMessage` is unsupported. Unknown receipts are not permission
to resend. See `docs/CMAEG_Protocol.md` for durable binding and rollout limits.

The existing public option-label bound of 64 UTF-8 bytes also applies.

### Owner-initiated messages

`prepareOutboundMessage(channelId, { source_id, route_id, message },
{ idempotencyKey })` creates a durable preparation without sending. The route
must already be configured in that owner's channel. `message` is the shared
`OutboundMessage`, including supported native media and approved WhatsApp templates.
`commitOutboundMessage(channelId, deliveryId, message)` repeats the exact message
and commits one send. `getOutboundMessage` reads the receipt;
`cancelOutboundMessage` cancels only before commitment. Preparation lasts 24 hours;
idempotency records remain for seven days. The deployment authoring flag must be on.

`sendOutboundMessage` is the convenience prepare-then-commit helper. It performs
no automatic mutation retry. A failed commit throws
`cmaeg.OutboundCommitUncertainError` with `deliveryId` and `isRetryable: false`;
use `getOutboundMessage` to inspect the record. A replayed committed/unknown or
cancelled preparation never causes another send. Never replace the key after an
unknown outcome. The original message stays with the caller; CMA stores its hash,
not the content. Thread/reply-to references cannot create unproven destination
authority, and interactive blocks use the explicit text rendering report.

### Named-consumer interactive callbacks

Named Telegram, Lark/Feishu and WhatsApp controls use the same SDK message shapes.
Discover capabilities: issuance needs the platform gate and the separate named
consumer gate. A callback returns to exactly the consumer conversation that
issued the card, including across gateway restart or consumer rename. Current
ACLs and consumer readiness still apply. Credential rotation is supported;
consumer removal or provider rebinding invalidates the old card's authority.
Disabling issuance preserves already-issued callbacks. There is no callback
fan-out or default fallback. Hand-off controls remain unavailable because CMA
has not yet persisted the original human callback audience through hand-offs.

### Typed WhatsApp template parameters

Template header media uses exactly one non-null `id` (1–32 Meta digits) or
`link` (public HTTPS, up to 8192 bytes). Only `document` accepts `filename`.
`currency` carries `fallback_value`, a three-letter uppercase `code` and
`amount_1000` (nonnegative integer, at most 9,007,199,254,740,991).
`date_time` carries `fallback_value` only. The gateway validates placement,
lengths and destination authority; Meta validates approval and media ownership.
The SDK exposes these exact typed fields; it does not infer callback recipients.

### Native Telegram message effects

Native Telegram message effects use `readMessageCapabilities`, `deleteMessage`, and `setMessageReaction`. Pass the existing confirmed
CMA `message_ref`; raw chat or message IDs are not accepted. Delete and reaction
require a stable caller-chosen idempotency key. Reactions accept zero entries to
clear the bot reaction or one emoji/custom-emoji entry. Legacy references and
relay transports report unsupported. An unknown receipt remains unknown and is
never retried automatically. Capability support remains subject to Telegram's
message-age, administrator-rights and reaction rules.

### Native WhatsApp existing-group messaging

Native WhatsApp source and request types accept optional `group_ids` (up to 16
unique opaque Meta group IDs). Absent or empty values retain private behavior.
New opt-ins require explicit `native_whatsapp_groups_writes_enabled` discovery;
retained groups can be removed when authoring is disabled. The separate groups
issuance flag controls new group intake and sends. Eligibility is the exact
phone's OBA approval and current existing-group membership, verified read-only.

Group event actors retain the original participant while conversation IDs retain
the group. Replies and delayed targets use sealed original consumer custody.
Group native media capabilities omit stickers and WebP. Group approved templates
advertise optional `whatsapp_template_components: ["header", "body"]` alongside
`whatsapp_templates: true`, and reject button components. Existing media/template
gates and format limits still apply. No group management, message edit/delete,
reaction or controls authority is added. Connected Meta v25/OBA acceptance is
still outstanding; source capability presence does not prove delivery.

### Confirmed Telegram and Discord media edits

Use `updateMessage(ref, { native_media_update: patch }, { idempotencyKey })` with an explicit
`native_media_update`. Read context `native_media_edits` for new-upload eligibility
and `readMessageCapabilities` for the exact reference's `media_edit` authority.
Historical effects-only references remain ineligible. This increment supports one
attachment in one confirmed platform message, with same-kind replacement only.

Omitting `replacement`, `caption`, or `text` preserves its current value. Empty
caption/text clears it; explicit null refuses. At least one patch member is
required, and ordinary message fields cannot accompany the patch. Telegram
`caption` is visible; Discord `caption` is the attachment description/alt text and
`text` is visible message content. Telegram does not accept `text` here.

Persist the confirmed receipt's next `message_ref`: it carries the next revision
and the original expiry. Retry only the identical body and original reference
with the same key; an unknown outcome retains its mutation fence and never
causes an automatic provider retry. Neither provider offers conditional revision
writes, so external applications sharing bot credentials remain outside CMA's
fence. New issuance stays gated until compatible readers are deployed and release
checks pass. Connected platform tests remain the owner acceptance step.

### Native Discord thread lifecycle

Native Discord thread lifecycle requires the gateway gate and source
`thread_lifecycle: true`. Check `context.capabilities.thread_lifecycle` for the
original event's scope. `client.listEventThreads(eventRef, 'active' | 'archived')`
returns at most 25 scoped threads and an explicit `truncated` flag.
`client.createEventThread(eventRef, key, { name: 'Review', auto_archive_duration: 1440 })`
creates a public thread from the exact original message.
`client.manageThread(key, { target: { type: 'thread', thread_ref }, action: 'archive' })`
also accepts `reopen`, `join`, or `leave`; `target: { type: 'event', event_ref }`
addresses only the thread containing that event. References preserve original
consumer custody and expiry. Unknown outcomes must not be retried under a new
key. Private/forum creation and private sibling enumeration are unsupported.

Rename an active exact thread with
`client.manageThread(key, { target: { type: 'thread', thread_ref }, action: 'rename', name: 'Incident triage' })`.
The name must contain 1–100 printable characters and must be omitted for every
other action. Rename never reopens an archived thread or changes its lock;
Discord enforces the bot's permission. An unknown rename result keeps the same
persistent fence as every other thread mutation.

Native WhatsApp Forms use `prepareFormReply(eventRef, message, stablePreparationKey)`,
then `readFormPreparation(eventRef)`. Poll Pending after `retry_after_ms`. Ready has
not sent a customer message. Commit the exact message with
`replyToEvent(eventRef, message, { idempotencyKey: stableCommitKey, formPreparationRef })`.
The returned `FormPreparationReceipt` explicitly distinguishes asset uncertainty
(`customer_message:not_sent`) from Delivery (`attempted`, with the existing receipt).
Keep keys/content on retry; never resend Unknown under another key. Both preparation
and status have a two-second total gateway deadline; explicit commit remains eight
seconds. Source `forms_enabled` is separate owner consent; operator discovery uses
`native_whatsapp_forms_enabled` and `native_whatsapp_flow_writes_enabled`.
See `docs/CMAEG_Native_WhatsApp_Forms.md` for the supported subset and retained gaps.

### Native Telegram forum topic lifecycle

The source's independent `topic_lifecycle: true` consent and the gateway's
separate default-off topic gate enable exact original non-General forum-topic
close/reopen. Check `context.capabilities.topic_lifecycle` for
`manage_original_topic`, `actions` and `expires_at`. Both default and named
consumers retain only their original admitted authority. Enabling consent
transparently enrolls a previously unrouted channel in default-only durable
routing when authoring prerequisites are available; it does not enable incoming
edit/reaction retention.

Use the existing method:

```ts
import { Cmaeg } from '@chronoai/cma-sdk';

export function closeOriginalTopic(client: Cmaeg, eventRef: string) {
  return client.manageThread('stable-close', {
    target: { type: 'event', event_ref: eventRef },
    action: 'close',
  });
}
```

Use `reopen` / `ThreadAction::Reopen` for reopening. Telegram accepts only the
original Event target and issues no `thread_ref`. There is no list, create,
private-chat or General-topic operation. Discord rejects the additive Close
action before platform IO. A confirmed receipt can omit `thread`.

Keep a stable idempotency key. Same-key replay returns the saved outcome;
conflicting action reuse fails. An unknown write keeps a permanent bot/chat/topic
fence across process restart, different keys and event expiry. Never retry an
unknown result with a new key. Discovery describes scope; current Telegram bot
administrator topic permissions are checked at execution. Source support and
local tests do not establish connected-account native acceptance.

### Native WhatsApp incoming message changes

Native WhatsApp incoming changes use optional source `message_events` consent and
optional discovery `native_whatsapp_message_events_enabled`. Both default off.
Meta supplies private edit/revoke callbacks only for Business App coexistence
accounts. Events use existing `cma.message.updated.v1` (`data.patch.text`) and
`cma.message.deleted.v1`, bound to the retained incoming message and its original
consumer/conversation. Caption changes preserve original attachments; outgoing
edit/delete and history import are outside this feature. See
[the incoming-change contract](../../../docs/CMAEG_Native_WhatsApp_Message_Events.md).

`readEventHistory(eventRef, cursor?)` returns one bounded history page and never
fetches a continuation automatically. Native Lark/Feishu mixed posts may preserve
their textual content with `omitted_content: ['media']` and empty `attachments`;
this marker conveys no media count, type, keys or download authority. Whole-row
omission counts exclude retained mixed posts, while `text_truncated` independently
reports clipped or sanitized text. Older SDK decoders reject `media`, so the gateway's separate
`CMAEG_NATIVE_LARK_MIXED_HISTORY_ENABLED` flag stays off until all consumers and
rollback readers support this value. See [native history](../../../docs/CMAEG_Native_Lark_History.md).

### Native media with controls

Check the optional `capabilities.native_media.controls` object before combining
`native_attachments` with buttons or choices. It declares `max_attachments`,
`buttons`, `choices`, `caption_max_chars` and `update`; absence grants no combined
send capability. The gated Telegram and Discord combinations each use one upload and one
native message/card. Its `update: false` excludes media-card edits. Existing
attachment bounds and event/consumer authority still apply.
