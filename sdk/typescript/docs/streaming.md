# TypeScript Streaming

<!-- nav: 50 Streaming -->

`streamEvents` reads CMA's durable REST event stream. Each frame has a positive,
per-session `seq`; that sequence is the resume cursor. Store the last event only
after the application has committed its effect, then pass it as `startingAfter`
when opening another stream.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
let cursor = 0;

for await (const frame of cma.streamEvents('sesn_example', { startingAfter: cursor })) {
  await Promise.resolve(frame.event);
  cursor = frame.seq;

  if (frame.event.type === 'unknown') {
    console.log('future event', frame.event.eventType, frame.event.raw);
  }
}
```

Every reconnect sends the last validated sequence as both REST's canonical
`starting_after` query and the standard `Last-Event-ID` header. One overlapping
frame is deduplicated. A frame that moves behind the cursor, names another session,
disagrees with its SSE id, or malforms a known event is a `ProtocolError`; the SDK
does not skip it silently.

The stream is one logical operation for request correlation. Its
`requestIdProvider` runs once before the first connection, and the SDK reuses that
exact `X-Request-Id` on every reconnect. A caller can choose the value directly:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
for await (const event of cma.streamEvents('sesn_example', {
  requestId: 'support-stream-488',
})) {
  console.log(event.seq);
}
```

AG-UI live connections and their history recovery use the same rule: all HTTP
requests belonging to one connection operation send the one resolved caller
request id. The configured `responseObserver` sees every connection response
before the SDK inspects its status, decides whether to reconnect, or reads its
body. Observer failures become `ResponseObserverError`; a missing or malformed
response `X-Request-Id` is reported to the observer as `undefined`.

## Reconnect policy

- Transport failures, `429`, and server failures are retried with bounded
  exponential delay and jitter.
- An SSE `retry` value takes precedence after a connected stream closes.
- Repeated connections that deliver no event are bounded by `maxEmptyConnections`
  (four by default).
- Authentication, authorization, not-found, gone, validation, and protocol
  failures terminate the iterator.
- Aborting the supplied signal closes the response body and throws
  `RequestAborted`. Breaking a `for await` loop invokes iterator cleanup; use an
  abort signal when cancellation must be immediate from another task.

```ts
import { createClient, RequestAborted } from '@chronoai/cma-sdk';

const cma = createClient();
const controller = new AbortController();

try {
  for await (const event of cma.streamEvents('sesn_example', {
    signal: controller.signal,
    maxEmptyConnections: 6,
  })) {
    if (event.event.type === 'turn_completed') controller.abort();
  }
} catch (error) {
  if (!(error instanceof RequestAborted)) throw error;
}
```

## Event vocabulary and forward compatibility

Known durable event types are exported as `KNOWN_SESSION_EVENT_TYPES` and parsed
into typed payloads. A discriminator introduced by a newer server becomes
`{ type: 'unknown', eventType, raw }`, preserving the complete object and allowing
the cursor to advance. A malformed payload under a known discriminator is not a
future event and fails as a protocol error.

AG-UI is a separate, run-shaped projection for conversation frontends. Use
`AguiConnection` and `AguiOperations` when building that surface; use
`streamEvents` when consuming the durable session log. See the
[AG-UI contract](../../../docs/API_AGUI.md) and
[REST replay contract](../../../docs/API_REST.md#5-events).

`createAguiTransport` is a fetch-SSE transport, not a native `EventSource`
wrapper. That distinction is required for a cross-origin browser: `fetch` can
send the configured `Authorization: Bearer` header and expose the response body
reader, while `EventSource` can do neither. Native `EventSource` therefore works
only with CMA's same-origin session cookie. Configure the deployment's origin
allowlist as described in [Authentication](authentication.md#browser-sessions).

`parseEventStream` handles a complete captured AG-UI SSE body. It returns
normalized `AguiEvent` values, groups durable prefixes with their numbered final,
and assigns occurrence identities to unknown siblings. Pass that result directly
to `fold`; do not map `toEvent` over it again. `toEvent` converts one raw frame and
cannot discover a prefix's occurrence identity without the rest of its projection.
An incomplete captured body that stops before the numbered final is a protocol
error.

AG-UI also has one cursorless interface boundary: an unnumbered, non-live
`RUN_STARTED` opens a continuation response that did not create a new durable
turn. The SDK delivers it immediately and in order, but does not mark it
ephemeral, deduplicate it, or advance the reconnect cursor. Every other durable
singleton or final still requires a positive SSE id.

## Why the SDK owns the patches

Earlier clients copied `STATE_DELTA` paths and wire fields into the frontend. That
drift made steering appear absent (`/conf/steer` instead of `/conf/steering`) and
read conversation gaps with fields the service never wrote instead of the real
`frames` count. The paths, gap shape, parser, and projection now live in this
package and are pinned to server fixtures. Consumers should fold exported events,
not reproduce JSON Pointer paths or usage/gap wire shapes.
