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

The custody namespace requires one supervisor writer. Its local state lock
serializes each comparison and write; this is not a filesystem protocol for
multiple simultaneous supervisor writers. Every persistent transition verifies
the exact expected retained record, including terminal cleanup, and confirmation
and revocation recheck it after joining streams. Missing or changed custody is
preserved as evidence and refused rather than repaired from memory.

This document does not qualify the ordinary Sandbox/Workspace work lease or its
external effects. Desktop epochs cannot fence provisioning, credential issuance,
installation or Workspace work. Their short-authority recovery contract remains
separate.

## Adapter and public composition

Replicas-enabled public composition selects the existing authorized desktop
service path. Its fresh browser/bearer proof, resource policy and post-open checks
remain in place. Disabled composition uses the existing legacy service path;
the receiver still enforces any retained floor from previous activation.

Codex, hosted backend and runtime-provider routing forward an optional owned
desktop port. Missing ports refuse; there is no fallback to legacy after an owned
request. Before a challenge, the concrete transport captures its retained logical
Sandbox binding, external handle, assignment revision and endpoint. Native paths
also capture and freshly compare the complete Pod/container incarnation.
Kubernetes exec and port-forward modes revalidate the assignment after preparing
the retained stream and before sending runtime credentials, with retirement
cancellation active through the handshake. They keep that capture through
confirmation, renewal and revocation. Owned native Direct is unsupported because
its current connection API cannot check the captured Pod before sending headers;
it refuses before discovery or credential-bearing I/O and never falls back to
legacy desktop. Provider endpoint discovery is
not repeated during that sequence. Native pool assignment is not performed by
desktop negotiation.

The independent control handle uses its original monotonic lifetime and consumes
each retained challenge once. Late acknowledgements cannot revive expired local
authority. HTTP mutation retries and redirects are disabled on the dedicated
provider-proxy control client; Kubernetes control preserves its existing no-replay
transport contract. Ambiguous open or revoke leaves durable claim release to
expiry. Local transport fixtures and a synthetic display do not qualify retained
production images, real Kubernetes/provider behavior or the desktop latency gate.
