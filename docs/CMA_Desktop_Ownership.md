# Desktop receiver authority

The desktop receiver uses a Sandbox/runtime/session/epoch claim independent of
Agent-manager ownership. Its authenticated control messages bind an external
runtime handle asserted by the adapter to the receiver's locally assigned
Sandbox, generation, home identity and actual boot. The external handle is not
independently derived by the receiver. Native pool assignment must have produced
the validated assigned `RuntimeConfig`; preassignment pool metadata is not a
substitute for the logical Sandbox identity. The adapter must retain its exact
assignment, Pod/container incarnation and endpoint across the exchange.

A challenge precedes durable claim acquisition or renewal. It is single-use,
bounded to ten seconds on the receiver's monotonic clock, and remembers whether
it authorizes a new connection or renewal of an existing session. Confirmation
never restarts that clock. A renewal requires the original session still live;
expired, revoked or completed sessions require a larger epoch and a new session.
An optional absent wire boot does not bypass the actual receiver boot binding.

Before acknowledging a new owner, the receiver closes legacy admission, persists
a retired floor, cancels prior streams and joins their tasks. It retains pending
WebSocket upgrades in the same task set. Cancellation of a draining control
request cannot detach these tasks. Only after drainage and a successful active
floor write may the receiver acknowledge the claim and accept one owned socket.
Revocation persists a tombstone and joins the exact session before acknowledging.
Uncertain confirmation or revocation is not permission for early durable release.

Every desktop frame read and send, including a blocked sink, observes expiry and
revocation before polling ready I/O. Control requests have a separate bounded
admission semaphore and do not share the frame queue. These are source contracts;
local synthetic-display tests do not establish real image, provider, process
lifetime, recovery-time or endpoint-latency qualification.

## Retained reader floor and rollback

The fixed runtime state namespace contains `desktop-owner.activated` and
`desktop-owner/floor.json`. The marker lives outside the floor directory. Missing,
unreadable, malformed or changed custody after activation fails closed; a lost
floor directory cannot make the receiver look never activated. The record retains
the last epoch/session, exact runtime and wire boot, actual receiver boot and
assignment identity. No TTL or ordinary cleanup removes either artifact. Home
retirement must retain them until all old managers and receiver processes are
excluded and the whole runtime identity is permanently retired.

Never-activated runtimes retain legacy desktop admission. After activation,
turning replicas off does not remove the receiver floor or reopen legacy I/O.
Rollback requires a receiver binary that continues enforcing both artifacts,
the epoch/session tombstones and the boot binding. Older binaries that ignore
this namespace are not compatible rollback targets. There is no desktop legacy
reset endpoint. Image inventories, old-writer exclusion, activation and any
retirement procedure require their separate integration and release disposition.

This document does not qualify the ordinary Sandbox/Workspace work lease or its
external effects. Desktop epochs cannot fence provisioning, credential issuance,
installation or Workspace work. Their short-authority recovery contract remains
separate.
