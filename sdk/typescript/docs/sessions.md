# TypeScript Sessions

<!-- nav: 40 Sessions -->

A session is a durable record plus the runtime CMA is trying to make match that
record. The session identifier, event history, agent settings, pricing, and granted
capabilities survive runtime rotation. A JavaScript object returned earlier does
not update in place; call `readSession` when a current answer matters.

## Create, list, and read

An environment profile selects the agent configuration, reach, skills, and optional
runtime size. Quote before creation when the deployment has a price list, and pass
the sealed rate-card identifier back with the create request.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
const quote = await cma.quoteSession('profile_backend');
const created = await cma.createSession({
  environmentProfile: 'profile_backend',
  quotedRateCard: quote.priced ? quote.rateCard : undefined,
  cols: 120,
  rows: 32,
});
const listed = await cma.listSessions();
const current = await cma.readSession(created.id);

console.log(listed.length, current.lifecycle);
```

`listSessions` follows the server's opaque `Link: rel="next"` continuations and
resolves only after every visible page has been validated.

Creation can return while the lifecycle is still `provisioning`. Use the durable
event stream for progress and `readSession` for the latest full projection.

## Lifecycle and recovery

| Lifecycle      | Meaning for a caller                                              |
| -------------- | ----------------------------------------------------------------- |
| `provisioning` | CMA is creating and attaching the runtime                         |
| `ready`        | The runtime is usable and no turn is active                       |
| `running`      | Work is in flight                                                 |
| `rotating`     | CMA is carrying the session to another runtime; do not send input |
| `idle`         | The runtime was released; the session can be revived              |
| `terminated`   | No further work can occur under this identifier                   |
| `unknown`      | A newer server reported a lifecycle this SDK does not yet name    |

When lifecycle is `unknown`, `reportedStatus` preserves the server's raw word. It
is neither treated as failure nor made interactive. `recoveryState` is separate
operational health; `stopReason: 'requires_action'` means the runtime is waiting
for an approval or tool result and is not terminal.

## Capabilities

Capabilities are fixed when the session is created and change only through an
explicit plane change. Check them before offering an operation:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
const session = await cma.readSession('sesn_example');

if (session.capabilities.steer) {
  await cma.steer(session.id, 'Focus on the failing integration test');
}
```

Do not infer capabilities from kernel names or lifecycle state. A typed
`CapabilityRefused` is the boundary backstop when another actor changed the plane
between the read and the operation.

## Management operations

- `stopSession` releases the runtime but keeps the record and history.
- `reviveSession` obtains a runtime for an idle session under the same identifier.
- `changePlane` rotates between conversation and terminal planes.
- `terminateSession` permanently ends work but leaves the record readable.
- `deleteSession` removes the session and history permanently.

During `rotating`, keep reading events and wait for the session to become usable.
Never send input to the outgoing runtime. The full model is defined in the
[API overview](../../../docs/API_Overview.md#2-session-model).
