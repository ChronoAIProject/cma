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
Provider tools and approval details use the independently authorized AG-UI
companion. Nested v2 HTTP errors become `ApiError` with their exact code,
`extensions.type`, `extensions.param`, request ID and retry delay. Deployment
middleware may still return the shared REST problem envelope, which is also
supported. See the [API contract](../../../docs/API_Responses.md).

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
cancellation, queued-response cancellation and verified platform-sender controls
are separate contracts.

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
