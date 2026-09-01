# TypeScript Operations

<!-- nav: 60 Operations -->

All client methods accept an optional final `OperationOptions` with `signal` and
`timeoutMs`. Unary success bodies are consumed or cancelled before the method
returns, so a prompt, steer, or approval does not leave an SSE response pinned.

## Input and turn control

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
const sessionId = 'sesn_example';

await cma.sendInput(sessionId, { text: 'Run the tests', newline: true });
await cma.steer(sessionId, 'Run only the integration tests first');
await cma.interrupt(sessionId);
```

Text is valid on conversation and terminal sessions when granted. Raw bytes are
terminal-shaped input:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
await cma.sendInput('sesn_example', { bytes: new Uint8Array([0x03]) });
```

Check the session's granted capabilities before `sendInput`, `steer`, `interrupt`,
or terminal operations. Input is deliberately refused while a session rotates.

## Approvals

An `approval_requested` durable event carries the request identifier. Resolve it
exactly once with `allow`, `deny`, or `abort`:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
await cma.resolveApproval('sesn_example', 'approval_example', 'deny');
```

`deny` refuses that action and lets the turn continue. `abort` refuses it and asks
the runtime to abandon the turn. An `approval_resolved` event lets another reader
fold the pending card away.

## Terminal REST operations

`resizeTerminal`, `screenSnapshot`, and `fetchTranscript` are ordinary REST
operations. They are not a live PTY attachment:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
await cma.resizeTerminal('sesn_example', { cols: 120, rows: 32 });
const screen = await cma.screenSnapshot('sesn_example');
const transcript = await cma.fetchTranscript('sesn_example');

console.log(screen.lines.length, transcript.byteLength);
```

The bidirectional WebSocket protocol is documented separately in
[API_PTY.md](../../../docs/API_PTY.md) and is not exposed by this package version.

## Administrative chat hosts

Callers with administrative session-read capability can walk every bounded
shared-host page. Retirement additionally requires cross-owner session control:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
const hosts = await cma.listChatHosts();
const first = hosts[0];
if (first !== undefined) {
  const retired = await cma.retireChatHost(first.id);
  console.log(retired.status);
}
```

`listChatHostsPage` exposes one manual page and an opaque continuation.
Continuations are bound to `/api/v1/admin/chat/hosts`; absolute, cross-origin,
and different-collection targets are refused before a credential is sent.
Repeating `retireChatHost` after retirement remains a `200` success.

## Profiles, pricing, resources, and usage

The TypeScript client also exposes environment-profile CRUD and catalogues,
`quoteSession`, `readSpend`, `readResources`, `readAllowance`, `listKernels`, and
`getUsage`. These are typed REST projections. The protocol document remains the
source for fields and refusal status:

- [environment profiles](../../../docs/API_REST.md)
- [pricing and usage](../../../docs/API_REST.md#9-pricing-and-usage)
- [capability declaration](../../../docs/API_REST.md#10-capability-declaration)
