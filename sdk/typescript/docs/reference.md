# Reference

<!-- nav: 11 Reference -->

`createClient(config)` returns a `Cma` with the following public resources:

| Resource                                                | Methods                                                                                                                                                                                                                                           |
| ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `agentProfiles`, `workspaceProfiles`, `sandboxProfiles` | `list`, `read`, `create`, `save`, `publish`, `listRevisions`, `readRevision`, `setVisibility`, `retire`                                                                                                                                           |
| `profileAuthoring`                                      | `bounds`, `services`, `searchSkills`, read-only `resolveRepository`                                                                                                                                                                               |
| `marketplace.agent`, `.workspace`, `.sandbox`           | `list`, `read`, `fork`                                                                                                                                                                                                                            |
| `sandboxes`                                             | `list`, `read`, `create`, `pause`, `revive`, `delete`, `events`                                                                                                                                                                                   |
| `workspaces`                                            | `list(sandboxId)`, `read`, `create(sandboxId, body, options)`, `delete`                                                                                                                                                                           |
| `worktrees`                                             | `list(workspaceId)`, `read`                                                                                                                                                                                                                       |
| `agents`                                                | `list`, `read`, `create(workspaceId, body, options)`, `input`, `steer`, `interrupt`, `answerApproval`, `stop`, `delete`, `events`, `transcript`, `usage`, `resources`                                                                             |
| `triggerAuthoring`                                      | `list`, `read`, `create`, `save`, `delete`, `publish`, `retire`, `listRevisions`, `listPlacements`, `createPlacement`, `retirePlacement`, `embed`, `stats`                                                                                        |
| `launches`                                              | `preview`, `createOrResume`, `read`, `list`, `start`, `cancel`, `requirements`, `events`, `stream`, `createConnection`, `readConnection`                                                                                                          |
| `agui`                                                  | `run`, `replay`, `readCapabilities`, `readSandboxSnapshot`, `streamLaunchEvents`                                                                                                                                                                  |
| `responsesV2`                                           | `readCapabilities`, `create`, `createForAgent`, `read`, `readEventContext`, `readConversation`, `listConversationItems`, `replay`, `observeControls`, `submitControl`, `readCommand`, `observeQueuedCancellation`, `cancelQueuedResponse`         |
| `agentEventsV2`                                         | `readTransportEligibility`, `snapshot`, `events` with acknowledgement after consumer success                                                                                                                                                      |
| `responses`                                             | `createConversation`, `readConversation`, `listConversationItems`, `create`, `read`, `delete`, `cancel`                                                                                                                                           |
| `auth`                                                  | `readSession`, `readSignupOptions`, `loginUrl`, `logoutUrl`                                                                                                                                                                                       |
| `admin`                                                 | `listSandboxes`, `readSandbox`, `sandboxEvents`, `retireSandbox`, `listProfiles`, `retireProfile`, `readSettings`, `replaceSettings`, `patchSettings`, `resetSettings`, `pauseTriggers`, `resumeTriggers`, `readRuntimeInventory`, `listActivity` |

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

Encrypted gateway records are available through `client.gatewayRecords.put/get/list/delete`. These operations require gateway management authority. `blob` is base64 ciphertext; creation uses `expected_version: null`, and replacement supplies the retained version. They use version preconditions without an Idempotency-Key header.

Provider binding management uses `client.providerBindings.put/get/delete/ensureConversation/deleteConversation`. Supply the complete profile snapshot and digest for PUT. These gateway-only operations require both management capabilities; use the scoped binding credential for Responses and immutable event-context reads. Binding responses never contain the supplied credential.

## CMA Event Gateway

`Cmaeg` is an independent client configured with the gateway origin, without a
`/v1` suffix. Its operations are `createChannel`, `getChannel`, `recoverChannel`,
`updateChannel`, `deleteChannel`, `testChannel`, `rekeyChannel`, `postEvent`,
`streamEvent`, `replyToEvent`, `updateMessage`, `sendTyping` and `readEventContext`.
Gateway wire types and `signWebhook` are exported under `cmaeg`.

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

`client.bots` exposes `create`, `list`, `get`, `available`, `test`, `disable`,
`enable`, `rotateKeys` and `delete`. Use the current renewable CMA sign-in with
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
