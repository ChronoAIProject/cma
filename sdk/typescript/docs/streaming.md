# Streaming

<!-- nav: 7 Streaming -->

AG-UI uses the Agent id as `threadId`; Responses uses it as `conversation`.
Both submit once and stream the existing Agent conversation. A broken connection
does not prove an admitted mutation failed, so the SDK never retries a POST.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
export async function readHistory(agentId: string, after: number) {
  const frames: unknown[] = [];
  await cma.agui.replay(agentId, {
    after,
    follow: false,
    onEvent: (event) => {
      frames.push(event);
    },
  });
  return frames;
}
```

An event callback is an acknowledgement boundary. Persist the event and its
cursor before resolving the callback. On failure, resume from the last durable
acknowledgement, not the largest event observed in memory. Replay identity and
monotonic ordering are checked. `follow: false` reads one captured history;
following replay bounds reconnects that make no durable progress.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
export async function followLaunch(launchId: string, signal: AbortSignal) {
  await cma.launches.stream(launchId, {
    after: 0,
    signal,
    onEvent: (event) => {
      console.log(event.seq, event.kind);
    },
  });
}
```

Launch progress has its own event sequence, independent of Agent conversation
cursors. Persist those cursors separately. `launches.events` provides bounded
pages when a live stream is unnecessary.

AG-UI and legacy Responses v1 return after their terminal event is acknowledged and cancel
the reader without waiting for server EOF. An Agent failure event is different
from a transport error. `timeoutMs` bounds the initial response handshake;
`signal` can bound the entire stream. Use `readCapabilities` before assuming
support, and preserve the user's existing history during recovery.

The legacy Responses v1 subset accepts one nonempty user text input with `stream: true`
and an existing `conversation`. Unsupported fields and delete/cancel operations
produce named problems. See [Reference](reference.md) for exact request shapes.

Agent replay accepts current organization/shared history access. Shared readers
receive approved events; private sign-in/configuration records become empty
`STATE_DELTA` patches with durable cursors. Acknowledge these cursors without
changing application state. Shared connections close on policy changes or at
the membership deadline (at most 60 seconds). Reconnect with the actual request
credential and the last fully acknowledged cursor; the new request checks
current membership again. Stream closure never submits another Agent input.
Personal-owner replay remains available during organization-provider outages.

## Retained keys for legacy POSTs

Pass optional `idempotencyKey` in `agui.run(request, options)` or
`responses.create(request, options)`. The SDK captures the body, Agent identity
and key before awaiting credentials or request IDs. Store that input and key
before sending; an explicit retry must use the same values. No automatic retry
or key replacement occurs after timeout, abort or lost response.

A key accepts 1–256 bytes without control characters and must be representable
as an HTTP header. Separate unkeyed attempts remain separate commands. AG-UI
`runId` is correlation only, and ordinary input on a Working Agent queues a new
Message. Legacy Responses still requires the exact predecessor; absent/null
means the first turn, including on a retained keyed request.

The upgraded server reserves a common receipt before readiness and sends
`x-cma-command-id` / `x-cma-response-id` when known, including prebinding HTTP
errors. A binding observation timeout is uncertain work, never a synthetic
failed turn. Repeat the exact keyed POST explicitly to recover the same turn,
or use the public command status/history reads. Legacy wire IDs and sequence
rules are unchanged; receipt IDs do not replace them. Old servers ignored
these keys, so do not replay a potentially executed pre-upgrade request on the
assumption that a new key record can prove its history.

## Responses v2

`responsesV2` admits input durably and exposes the retained receipt before any
frames. Persist a stable key before sending. The SDK never retries a POST or
reconnects automatically. `createForAgent` selects the same Agent by URL;
`read`, `readConversation`, `listConversationItems` and `readCapabilities` use
current authorization for recovery and discovery.

```ts
import { createClient, type V2ResponseEvent, type V2ResponseReceipt } from '@chronoai/cma-sdk';

const cma = createClient();
export async function submit(
  agentId: string,
  input: string,
  stableKey: string,
  saveReceipt: (receipt: V2ResponseReceipt) => Promise<void>,
  saveEvent: (event: V2ResponseEvent) => Promise<void>,
) {
  return cma.responsesV2.create(
    { conversation: agentId, input, stream: true },
    {
      idempotencyKey: stableKey,
      onReceipt: saveReceipt,
      onEvent: saveEvent,
    },
  );
}
export async function reconnect(
  responseId: string,
  lastAcknowledged: number,
  saveEvent: (event: V2ResponseEvent) => Promise<void>,
) {
  await cma.responsesV2.replay(responseId, {
    startingAfter: lastAcknowledged,
    onEvent: saveEvent,
  });
}
```

For v2, persist `sequence_number` after processing each standard event. A flat
`error` event has no durable SSE ID: record the observation error without
advancing the saved replay cursor. It is followed by `TransportError`; retrieve
or replay the same response with current authorization. The parser also rejects
premature EOF, changed identities, inconsistent item indexes/text, and events
after the terminal response. Replaying after the terminal cursor returns no
events. The stream waits for EOF to detect invalid trailing frames; use `signal`
to bound observation. `timeoutMs` bounds the handshake.

V2 uses opaque response IDs, a queued lifecycle, conversation on each turn,
and no predecessor. Text is emitted at durable message commit boundaries. Shared
readers receive empty metadata, and membership expiry can end their connection.
Output items are a typed union of `message` and informational `mcp_call` activity.
Narrow on `item.type` before reading message `content`. Activity carries an empty
`arguments` string, null `output`/`error`, and a display title; it never asks the
client to execute a tool. The three `response.mcp_call.*` events retain stable
indexes through interleaved text, overlapping calls and replay. Approval details
use the independently authorized AG-UI companion. Nested v2 HTTP errors become `ApiError` with their exact code,
`extensions.type`, `extensions.param`, request ID and retry delay. Deployment
middleware may still return the shared REST problem envelope, which is also
supported. See the [API contract](../../../docs/API_Responses.md).

Binding-key callers may supply an external conversation ID; the returned
conversation identifies the mapped Agent. The Agent alias and request conversation
may differ when the server resolves both to the same Agent. Known Agent IDs still
receive exact identity validation. An optional `event_context` retains the complete
schema-version-1 event snapshot atomically with admission. Preserve the original
body and idempotency key on retry. Changed context or another key for the same event
conflicts. The runtime receives its `event_ref`, while the complete object is read
with `responsesV2.readEventContext(bindingId, conversationId, eventId)`.
The helper requires binding authentication and validates conversation/event identity.
The snapshot limit is 1 MiB; metadata and runtime input limits remain independent.

## Response-targeted controls

`responsesV2.observeControls(agentId, responseId)` returns the exact response's
source command, bound turn, Agent aggregate revision and runtime support. A queued
response has no target yet. Runtime support describes an observation; the server
checks current control permission and the pending approval again at admission.

`submitControl` requires a retained `idempotencyKey` and a strict body containing
`turn_sequence`, `expected_agent_revision` and one command. Persist the complete
intent before sending. After a lost reply, retry the same Agent, response, body
and key, including the old revision; the SDK never observes a newer turn or
creates a replacement key automatically. Header keys contain at most 256 UTF-8
bytes, no control characters, and must survive Fetch header encoding and whitespace
normalization unchanged. Numbers must be positive JavaScript safe integers; an
observation outside that range is rejected rather than rounded.

```ts
import { createClient, commandIsTerminal, type V2ResponseControlRequest } from '@chronoai/cma-sdk';

const cma = createClient();
export async function requestCancellation(
  agentId: string,
  responseId: string,
  retainedKey: string,
  persist: (intent: {
    agentId: string;
    responseId: string;
    key: string;
    body: V2ResponseControlRequest;
  }) => Promise<void>,
) {
  const observed = await cma.responsesV2.observeControls(agentId, responseId);
  if (observed.target === null || !observed.target_active)
    throw new Error('Observe an active bound response first');
  const body: V2ResponseControlRequest = {
    turn_sequence: observed.target.turn_sequence,
    expected_agent_revision: observed.agent_revision,
    command: { type: 'cancel' },
  };
  await persist({ agentId, responseId, key: retainedKey, body });
  const accepted = await cma.responsesV2.submitControl(agentId, responseId, body, {
    idempotencyKey: retainedKey,
  });
  const status = await cma.responsesV2.readCommand(accepted.control.receipt.command_id);
  return { accepted, status, commandTerminal: commandIsTerminal(status) };
}
```

Other commands are `{ type: 'steer', text }` and
`{ type: 'resolve_approval', approval_id, decision: 'allow' | 'deny' | 'abort' }`.
Steer text retains whitespace and accepts at most 256 KiB of UTF-8. Obtain the
approval identity from the separately authorized AG-UI companion for this turn.

`readCommand` makes one bounded GET without submitting input. Poll explicitly
with your application's deadline, backoff and `signal`; preserve `ApiError.code`
and `retryAfterMs` when observation is unavailable. Aborting a local request
cancels observation only. `queued`, `waiting_action`, delivery `recovery_required`
and `runtime_lost` remain distinct from terminal status. A completed control
means delivered: call `observeControls` again and inspect its source
`response.status` to confirm actual `cancelled`, `completed` or `failed` outcome.
The target may finish normally before a cancel takes effect.

Inspect the ordinary API error's `code`: `response_control_not_admitted` proves
that the original scope/key has no receipt and its submitted revision is permanently
stale in the same Agent incarnation. Discard that pending intent, observe again,
and require a new explicit action before submitting another control.
`response_control_conflict` remains uncertain even on the first client call;
retain the original key and body for recovery. `agent_command_key_conflict` means
the key was reused for changed intent; `capability_unsupported` means the observed
runtime cannot perform the command.
The optional discovery `controls` and `command_status` fields describe this CMA
extension; older discovery documents remain readable. Standard Responses
cancellation and verified platform-sender controls are separate contracts.
Cancellation before turn binding uses the dedicated methods below.

## Interrupt with new input

`interruptWithInput` atomically admits cancellation of an observed active response
and a distinct new user input. Pass the original Agent and response IDs, a
`V2ResponseInterruptionRequest` containing `turn_sequence`, `expected_agent_revision`
and `text`, and one retained idempotency key. Use `observeControls` to obtain the
current binding and revision before submission.

HTTP 202 returns `control` and `response` command views plus the original
`target` and `expected_agent_revision`. The control has no response ID; the new
input has a different response ID from the interrupted response. Its queue
priority places it before ordinary queued messages while preserving their
identities and relative order. Admission does not prove cancellation or new-turn
startup: observe the original response outcome and the new command separately.

After a lost response, retry the exact original IDs, body and key. The SDK never
retries or retargets automatically. Generic conflicts remain uncertain;
`response_interrupt_not_admitted` is the definitive proof that the original pair
was not admitted and its observed target became stale. Keep ordinary Stop on
`submitControl` with a cancel command; it admits no replacement message.

## Queued response cancellation

`observeQueuedCancellation(agentId, responseId)` returns the source command,
its `command_revision` and current `cancellable` observation. Eligible sources
are queued messages or unbound Channel messages waiting for action. CMA checks
fresh human/API operation authority and source state again during cancellation;
this observation does not authorize a verified platform sender.

`cancelQueuedResponse` requires a strict `V2QueuedCancellationRequest` with the
original `source_command_id` and `expected_command_revision`, plus a retained
key in `CommandOptions`. This revision belongs to the source command, not the
Agent. The client sends one POST and never generates keys, repeats mutations,
or replaces the original source selection after a lost reply.

```ts
import { createClient, type V2QueuedCancellationRequest } from '@chronoai/cma-sdk';

const client = createClient();
export async function cancelPendingResponse(
  agentId: string,
  responseId: string,
  key: string,
  persist: (savedIntent: string) => Promise<void>,
  signal: AbortSignal,
) {
  const observed = await client.responsesV2.observeQueuedCancellation(agentId, responseId, {
    signal,
  });
  if (!observed.cancellable) throw new Error('Response is no longer cancellable before dispatch');
  const body: V2QueuedCancellationRequest = {
    source_command_id: observed.response.receipt.command_id,
    expected_command_revision: observed.command_revision,
  };
  await persist(JSON.stringify({ agentId, responseId, key, body }));
  return client.responsesV2.cancelQueuedResponse(agentId, responseId, body, {
    idempotencyKey: key,
    signal,
  });
}
```

HTTP 200 returns a completed `cancellation` receipt with no response ID, turn or
runtime delivery, the immutable `target`, and the source command's current
`response` view. This receipt establishes committed cancellation before dispatch.
On exact retry, the receipt and target remain stable while the current source
view may change. Existing standard Responses event shapes stay unchanged.

After any uncertain reply, retry explicitly with the same route, body and key.
`queued_response_cancellation_conflict` remains uncertain even on the first call.
`queued_response_cancellation_not_admitted` is the dedicated committed proof that
the original intent has no receipt and cannot be admitted at its old source
revision; it allows discarding that pending intent. Observe again and require a
new explicit action before creating another intent. `agent_command_key_conflict`
means that a retained key was reused for changed input. Ordinary `ApiError.code`
preserves these distinctions. Optional discovery `queued_cancellation` identifies
this extension, and older discovery documents remain readable.

## Rich Agent replay and transport eligibility

`client.agentEventsV2` exposes authenticated `readTransportEligibility`, bounded
`snapshot`, and read-only `events`. Eligibility applies to new tabs. A missing
endpoint, unavailable response, unknown schema, or invalid document cannot select
a new v2 tab. Existing v2 tabs retain their transport and exact accepted or
uncertain input when eligibility becomes false; never replay that input through
v1. Eligibility and verified response bindings are observations, not authority.

Snapshots contain the safe `V2AgentEventKind` vocabulary, including complete tool
status/title, messages, approvals, usage, and original turn timestamps. A withheld
row has `kind: null` and still advances chronology. Live text and reasoning arrive
only through `cma.agent.live`; they have no SSE ID or durable cursor. Reconnection
gaps have an unknown frame count. Missed reasoning is unrecoverable and must never
enter durable storage.

Agent cursors are opaque `ag1` coordinates bound to one Agent incarnation.
Numeric `sequence` orders Agent history independently of standard Responses
`sequence_number`. Snapshot continuation uses a separate `ags1` token. Every page
stays within 200 rows and 4 MiB through its original `through_cursor`. Apply every
page before using that frozen watermark to start the tail. Parsers reject gaps,
duplicate rows, changed watermarks/identities, private fields, invalid response
bindings, and unsafe JavaScript numeric coordinates.

```ts
import type { Cma, V2AgentEventItem, V2AgentStreamFrame } from '@chronoai/cma-sdk';

async function recoverAgent(
  client: Cma,
  agentId: string,
  applyDurable: (item: V2AgentEventItem) => Promise<void>,
  showLive: (frame: V2AgentStreamFrame) => Promise<void>,
  saveStateAndCursor: (cursor: string) => Promise<void>,
  signal: AbortSignal,
) {
  let pageCursor: string | undefined;
  let through: string;
  do {
    const page = await client.agentEventsV2.snapshot(agentId, { limit: 100, pageCursor, signal });
    for (const item of page.items) await applyDurable(item);
    through = page.through_cursor;
    pageCursor = page.next_page_cursor ?? undefined;
  } while (pageCursor !== undefined);
  await saveStateAndCursor(through);
  await client.agentEventsV2.events(agentId, {
    afterCursor: through,
    signal,
    onEvent: async (frame) => {
      if (frame.type === 'cma.agent.event') await applyDurable(frame.data);
      else await showLive(frame);
    },
    onCursor: saveStateAndCursor,
  });
}
```

`saveStateAndCursor` must atomically persist normalized durable state and its
coordinate under the account/session and Agent scope. `onCursor` runs only after
`onEvent` succeeds for a complete durable frame. Callback or storage failure
closes observation before the next frame; ephemeral frames never invoke
`onCursor`. Make reducers safe for replay after a crash between application and
the storage commit. A snapshot bootstrap that cannot commit its final state must
restart from the preceding durable checkpoint.

Reconnect explicitly with the last saved `afterCursor`. Optional `lastEventId`
sends the standard `Last-Event-ID` alternative; if both are supplied they must
match. `follow: false` reads through a captured tail head and ends. No read path
automatically reconnects, retries input, falls back, retargets, or submits a
command. A normal close can reflect expired authority; reopen with current
authorization, the retained cursor, and application backoff. Local abort stops
observation only.

Typed `ApiError` codes include `invalid_agent_cursor`, `agent_cursor_beyond_head`,
`agent_incarnation_changed`, `agent_not_found`, `agent_history_unavailable`, and
`agent_read_unavailable`. A `cma.agent.error` frame reaches the consumer and then
rejects observation without acknowledging a cursor. Changed incarnation requires
rebuilding normalized history for the new resource.

### Current Agent event head

`agentEventsV2.head(agentId, { signal })` reads a constant-size, authorized live
baseline independently of historical snapshot pagination. It returns
`through_cursor`, `incarnation` and `active_turn`: either `null` or the exact
durable `turn_started` item, including any verified `response_binding`. Start a
separate live tail after this cursor while historical pages load; buffer and
reconcile later events by durable sequence when merging history. The head is not
an acknowledgement that earlier history has been applied. A turn may finish or
a new turn may start after capture; those events arrive on the tail normally.
The SDK validates Agent identity, incarnation, cursor bounds and the active
turn's exact sequence and response binding before returning the baseline.

### Edit pending input in place

The public queue lists every editable human/API message in its original admission
order, including messages submitted from another tab. The server admits at most
64 pending commands. HTTP pages contain at most four entries and a nullable
`next_after` admission-order cursor, keeping worst-case JSON escaping within the
SDK's unchanged 8 MiB response limit. The SDK reader follows these pages and
returns the full bounded list; it rejects repeated cursors, duplicate commands or
more than 64 accumulated entries. Pages observe current state independently;
concurrent edits to earlier pages appear on the next refresh. Refresh after admission,
edit, cancellation, observed conversation changes, and returning to the page.
The ledger does not emit conversation events before dispatch; clients displaying
an active queue should also refresh periodically to observe other tabs.

An edit preserves the source command, response and queue position. It replaces
the complete text, including packed attachment text, only before runtime dispatch.
Persist the route, exact source revision, text and a new idempotency key before
sending. A lost reply must be recovered with identical bytes and the same key.
`queued_input_not_admitted` proves the original revision is stale; refresh the
queue before creating another intent. The edit receipt returns its immutable
replacement text and resulting revision, alongside the source's current status.
Removal continues to use queued cancellation with a fresh observation.

```typescript
import { createClient } from '@chronoai/cma-sdk';

export async function editFirstQueuedInput(
  client: ReturnType<typeof createClient>,
  agentId: string,
  retainedEditKey: string,
) {
  const queue = await client.responsesV2.listQueuedInputs(agentId);
  const item = queue.data[0];
  if (item?.editable) {
    await client.responsesV2.editQueuedInput(
      agentId,
      item.response.receipt.response_id!,
      {
        source_command_id: item.response.receipt.command_id,
        expected_command_revision: item.command_revision,
        text: 'Updated message, including any attachment text',
      },
      { idempotencyKey: retainedEditKey },
    );
  }
}
```
