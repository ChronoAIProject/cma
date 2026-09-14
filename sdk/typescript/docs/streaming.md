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

AG-UI and Responses return after their terminal event is acknowledged and cancel
the reader without waiting for server EOF. An Agent failure event is different
from a transport error. `timeoutMs` bounds the initial response handshake;
`signal` can bound the entire stream. Use `readCapabilities` before assuming
support, and preserve the user's existing history during recovery.

The Responses subset accepts one nonempty user text input with `stream: true`
and an existing `conversation`. Unsupported fields and delete/cancel operations
produce named problems. See [Reference](reference.md) for exact request shapes.
