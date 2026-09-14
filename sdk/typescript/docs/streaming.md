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
