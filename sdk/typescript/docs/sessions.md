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
resolves only after every visible page has been validated. One `timeoutMs` covers
the complete walk, including every response body. The compatibility walk refuses
more than 100 pages or 20,000 retained rows by default; callers can choose smaller
ceilings with `maxPages` and `maxItems`.

Use the bounded API when a UI or worker should decide when to fetch more. Treat
`next` as opaque and pass it back unchanged:

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
let page = await cma.listSessionPage({ status: 'running', limit: 50 });
console.log(page.items.map((session) => session.id));

if (page.next !== undefined) {
  page = await cma.listSessionPage({ next: page.next });
  console.log(page.items.map((session) => session.id));
}
```

Environment profiles expose the same manual shape through
`listEnvironmentProfilesPage({ limit, cursor, next })`; profiles do not accept a
session-status filter. Page limits are integers from 1 through 200. `cursor` and
`next` are alternatives: a server-issued `next` already carries the original
filters and cannot be combined with new ones.

`preferEnvironmentProfile(id)` returns the complete post-write profile
collection, newest first. Exactly the selected profile has `preferred: true`;
passing `null` clears the preference. The service preserves this released
`/v1` response while assembling it through bounded pages internally.

Creation can return while the lifecycle is still `provisioning`. Use the durable
event stream for progress and `readSession` for the latest full projection.

## Lifecycle and recovery

| Lifecycle      | Meaning for a caller                                            |
| -------------- | --------------------------------------------------------------- |
| `provisioning` | CMA is creating and attaching the runtime                       |
| `ready`        | The runtime is usable and no turn is active                     |
| `running`      | Work is in flight                                               |
| `rotating`     | CMA carries the session; the server returns a retryable refusal |
| `idle`         | The runtime was released; the session can be revived            |
| `terminated`   | No further work can occur under this identifier                 |
| `unknown`      | A newer server reported a lifecycle this SDK does not yet name  |

When lifecycle is `unknown`, `reportedStatus` preserves the server's raw word. It
is neither treated as failure nor made interactive. `recoveryState` is separate
operational health; `stopReason: 'requires_action'` means the runtime is waiting
for an approval or tool result and is not terminal.

During `rotating`, the server returns the retryable `409 session_rotating` error
for new input. Disable input controls for experience and keep reading events until
the replacement is usable; the client guard is not the correctness boundary.

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
