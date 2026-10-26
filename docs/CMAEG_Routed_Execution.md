# Durable routed execution journal

This is a disabled execution foundation, following
`docs/CMAEG_Routed_Admission.md`. It does not connect ordinary ingress to routing,
install a scheduler, configure a provider capability, or activate production
fan-out. The concrete HTTP execution adapter and production preparation
builder are implemented; a scheduler and a durable platform delivery journal
remain required before activation. Existing ingress and default launch behavior are
unchanged.

`Dispatcher::dispatch` accepts an original event identity and one target ID. It
loads the committed admission and finds that ID in its immutable target list.
Retries never evaluate routing again. The execution record uses that target ID,
the original owner/root/request hash, the unchanged event expiry, and the same
fixed retention boundary. All writes use the existing interaction-record API.

Each execution claim has a random claim identity, worker name, increasing epoch,
30-second lease, and CAS version. Even concurrent requests sharing a worker name
cannot both claim the same target. A lost write response succeeds only after
authenticated read-back of the exact expected next version and value. Expired
workers cannot renew or change a later worker's record. These leases fence local
record changes; they cannot revoke a network request already sent. This is not
an exactly-once delivery claim.

Preparation renders the provider input and seals its event reference once. The
exact fields, metadata, reference, and delivery policy are frozen in encrypted
24 KiB chunks, with a bounded 1 MiB total representation. Chunk identities bind
the target, admission, request, full-body hash and index. The target CAS record
references the complete body only after all immutable chunks are confirmed.
Missing, modified or reordered chunks fail closed. Retried starts reuse the same
body and idempotency key. Provider credentials are fetched from current authority
and injected separately; credential rotation cannot rewrite the frozen body.
`FrozenTurn::provider_body` exposes the canonical provider request representation.

The fresh authority port bypasses the channel cache. Before starting, the
dispatcher checks the channel owner and record, the exact admitted source policy,
the original consumer's presence and readiness, credential availability, and the
original provider binding. Source policy edits are conservatively refused rather
than reinterpreted. A durable `Starting` CAS precedes the external call; authority,
lease ownership and event expiry are checked again immediately before that call.
Revocation can still race a network request, so the provider must independently
validate its current credential. No atomic cross-service authorization is claimed.

The execution port must await its start directly and must not spawn a detached
start task or regenerate input, attachment URLs, nonces or event references.
Providers lacking proved response replay are refused before starting. Recovery
capabilities default to false and are configured only after evidence against the
provider's actual contract. A provider slug alone is not evidence.

`Starting` and `Unknown` mean the request may already have been accepted. They
permit an exact-body retry only with proved provider idempotency. Otherwise they
remain unknown without another POST. A proved nonacceptance permits a retry; a
terminal provider refusal is retained. Network errors and 5xx responses are not
proof of nonacceptance. An accepted response retains its exact response ID and
durable cursor and is never posted again. Recovery returns a read/replay directive
with an ownership lease and the original frozen body.

The initial cursor is absent, so replay includes durable event zero. A cursor of
zero is stored only after event zero has a confirmed durable downstream receipt.
Adapters must not translate an absent cursor into `starting_after=0`.

The delivery cursor advances only after a caller has confirmed durable downstream
receipt of that numeric provider event. Merely reading an SSE frame is not a
receipt. Response identity mismatches and backwards cursors are refused. The
completion method requires all provider events and platform effects to have
settled durably. Unknown platform sends need their own receipt/reconciliation
logic and are never automatically resent by this journal. Provider replay may
read retained history after the original event expiry; that does not renew the
expired authority to send replies.

Only the primary target's frozen reference may retain a legacy synchronous
waiter. Secondary targets receive `ReplyAddress::None`. This prevents sibling
consumers from racing to close one synchronous response. Durable asynchronous
platform delivery uses the fenced receipts described in
`docs/CMAEG_Durable_Delivery.md`; stream projection and worker composition remain
separate integration work.

Tests use the real `RecordClient` HTTP transport against a bounded record server
that enforces create-only writes, CAS, expiry and lost-response faults. They cover
competing claims, stale workers, unknown CAS confirmation, large bodies,
credential rotation, corruption, authority changes, event expiry, unknown-start
recovery, accepted replay, cursor fencing and synchronous waiter ownership.
These tests do not replace a real database concurrency test or evidence for a
concrete provider's idempotency and replay contract.

## Concrete HTTP adapter and remaining activation work

`HttpExecutionPort` composes the journal with the opt-in Responses v2 provider
transport. Its `dispatcher()` factory shares the exact plan store, execution
journal and fresh authority resolver used by `reattach()`. `FrozenTurnBuilder`
is an injected preparation boundary; it must render and resolve input without
starting a provider turn. It is called only for a pending target, before the
journal commits the frozen body. `ProductionFrozenTurnBuilder` implements this
boundary with the same preparation/rendering helpers used by `TurnRunner`.
No ingress installation is included in this slice.

Submission sends `FrozenTurn::provider_body()` bytes directly through the
existing bounded provider transport, with the current credential supplied
separately. It does not call the legacy `start_turn`, spawn a background start,
retry automatically or reconstruct a stored request. Provider concurrency is
held through acceptance. The acceptance wait is limited to the configured
request timeout or 20 seconds, whichever is smaller, and at most 1 MiB of wire
input. A successful acceptance requires one exact response identity header and
an SSE `response.created` event at numeric ID and sequence zero, with queued
status and the frozen metadata. Invalid or absent proof remains `Unknown`.

A local provider-slot refusal proves no request was sent. CMA v2's explicit
`400 invalid_json` occurs before command admission and is a terminal refusal.
Other HTTP results, including 409, 429 and 503, are conservatively unknown:
CMA's submit handler can durably admit a command before stream authorization or
snapshot opening fails. This adapter therefore keeps exact-body idempotency
capability false. It will not repeat an uncertain POST.

Reattachment reloads the committed plan, verifies the supplied opaque lease
belongs to that precise target, and authenticates the stored body. It obtains
the response ID and optional cursor only from the accepted journal record.
It revalidates owner, source, consumer, provider and lease immediately before
the snapshot GET, then checks them again with a freshly resolved credential
before the replay GET. The snapshot must match the accepted response and frozen
metadata. Its native CMA Agent ID is pinned for replay and checked against an
optional already-known native Agent ID; it is not compared to the gateway's
conversation alias. There is no POST in this recovery path.

The adapter returns observations and never advances a cursor simply because it
read an event. The later delivery worker must renew and guard its lease, enforce
current delivery authority and original reply expiry, record platform effects
and durable receipts, and acknowledge events only after those effects are
settled. Snapshot/replay failures must remain observation failures and must not
turn an accepted execution into a command failure or trigger a fresh start.

The new local HTTP tests cover exact frozen bytes, credential rotation, a
restart with GET-only recovery, absent versus zero cursor, uncertain responses,
cancelled acceptance, malformed acceptance, revoked authority between GETs,
lease expiry during snapshot, and foreign target/native Agent substitution.
They complement the record/CAS tests and provider recovery protocol fixtures.
Before activation, the scheduler, durable receipt delivery worker and complete
replay/projection behavior need combined integration evidence. Rollout
must select only explicitly enabled channels; existing channels retain their
current execution path while the new path is verified.

## Bounded admission recovery scheduling

The scheduler is implemented as an uninstalled execution component. New durable
admissions can offer identity-only wake-ups through `ScheduleHandle`; a full
queue does not retract an admission. The recovery scanner independently visits
committed manifests through the existing interaction-record API. It scans all
owners as a service, authenticates each manifest against its stored owner, and
never exposes another owner's payload through an owner-scoped endpoint.

The default queue holds 512 target identities and at most eight executions run
concurrently. Recovery visits at most 32 record heads per page, waits 250 ms
between pages and five seconds between complete sweeps or failed reads. Empty
pages can contain unrelated record kinds and do not end a paginated sweep. A
failed read retains its cursor; completed sweeps restart at the beginning so
inserts behind an earlier cursor are eventually discovered. Damaged admissions
are reported and skipped without repair writes. Each full sweep still costs
reads for unrelated interaction records and reconstruction of admitted payloads;
these bounds do not establish a five-second restart-recovery guarantee.

Running targets are deduplicated locally; a bounded cooldown cache limits repeat
hints after completion. Local deduplication is an optimization only: every
executor must still use the durable lease and current authority before effects.
Executor panics release the local slot for later journal-based recovery; a failed
scanner is reported as an unhealthy scheduler instead of silently abandoning
recovery. Shutdown stops new starts, cancels only the read-only scanner, and
drains running tasks for the configured budget (600 seconds by default). A
forced abort is counted and leaves the durable effect journal authoritative.
The new scheduler does not alter production ingress or start workers until its
bootstrap and concrete replay/delivery executor are integrated and enabled.

## Production preparation and composition seam

Construct `ProductionFrozenTurnBuilder::new(providers, keys)` and configure it
with the same `with_media(media, prepare_config)` and
`with_media_tool_providers(tool_providers)` values as the live runner. Inject it
into `HttpExecutionPort`; the builder is not itself a launcher. It verifies the
exact stored target, current owner/source/consumer/provider and explicit replay
support before external preparation. The target's conversation, binding,
idempotency key and consumer are never recomputed from current routing. A new
sealed reference retains the admitted reference's payload, expiry and lineage,
including the secondary-target synchronous-waiter restriction. Partition
metadata comes from the admitted partition even if current channel partition
settings have changed.

The only provider preparation effect is one directly awaited idempotent
`ensure_conversation` PUT. CMA's current API returns an existing binding
conversation without replacing its Agent and uses a durable CAS create for a
missing conversation. Source-user affinity is acknowledged without allocating
an Agent. Preparation never probes with or submits a Responses POST and never
creates a detached provider task. Failure/cancellation therefore cannot start a
turn. An uncertain PUT may have created its durable conversation; a subsequent
pending preparation safely uses the same get-or-create identity.

Input rendering and media preparation are shared with the existing runner.
Plain text, addressed messages, reply context, custom templates and bounded
attachment blocks keep the same semantics. Eager transcripts/excerpts use the
existing media service; pending results, failures and background cache warm-up
retain that service's existing bounds and behavior. Provider tool instructions
are advertised only to the same eligible profile bindings as before. The
rendered nonce, attachment text, metadata and full EventRef are then frozen by
the journal. Accepted/prepared retries use that stored body and never repeat
rendering or conversation preparation. Credentials remain separate.

Preparation is bounded by 20 seconds and the original event/retention expiry;
media work also obeys its configured preparation budget. After the frozen body
is authenticated, the dispatcher renews its still-owned lease before committing
`Starting`, leaving the full lease interval for bounded POST acceptance. A
failed renewal prevents the POST. Current authority and event expiry are checked
again at the start boundary.

The implemented bounded scheduler offers committed target identities to its
`TargetExecutor`. The remaining executor composition must call
`HttpExecutionPort::dispatcher` and hand accepted leases to a fenced
replay/projection worker. The worker must persist its projection state and use
`RoutedDelivery` to settle durable downstream receipts before journal
acknowledgement. Existing `TurnRunner` launch,
sequencing, acceptance, retries and delivery are not rerouted by the builder;
its extraction only shares preparation and rendering functions.
