# Ordinary routed admission storage

This internal implementation is not connected to ingress or provider execution.
It complements the pure ordinary-routing planner. It does not change a public
API, enable fanout, or authorize delivery from a restored plan.

The gateway stores routed admissions in its existing encrypted interaction
records. No new collection, infrastructure, or NyxID change is required. Each
record has a fixed expiry within seven days and a maximum ciphertext size of
65,536 bytes. The existing encryption authenticates the record identity and key
version. Records are written with create-only semantics; their version stays 1.

## Commit boundary

The root ID is a domain-separated hash of the owner, channel, channel record and
event identities, encoded as a canonical UUID. It deliberately excludes the
logical request and plan hashes. Consequently, two requests reusing one event
identity compete for one root; a changed logical body is refused while that
root is retained.

The payload preserves the complete normalized Activity, selected source,
provider bindings, target references, stable target IDs, and exact verified
source bytes. Logical request hashing canonicalizes JSON source bodies and
includes the normalized Activity except routing and fallback ingress time.
Producer-supplied time remains in the source/CloudEvent. Random sealing,
reference renewal and later routing edits do not define a new logical request.
The planner's diagnostic trace is intentionally not sealed, following its core
contract.

The payload is limited to 1 MiB, including up to 256 KiB of source bytes and
Activity/reference overhead. It is split into at most 43 chunks of 24 KiB.
Each chunk has an authenticated encrypted envelope containing its root,
logical request hash, complete payload hash, index, count and fixed expiry.
The manifest also records the ordered chunk hashes and exact payload length.
This detects missing, reordered, substituted and corrupt chunks before the
plan can be returned.

All chunks are written first. One create-only root manifest commits admission
last. A competing same-body request returns the original winning snapshot,
including its original routing. A changed-body request receives a conflict.
Lost write responses are resolved by authenticated reads. An uncertain write
never produces an accepted result. Conflicting records that have become
unreadable are never deleted or recreated.

The replay lookup runs before consulting current routing policy. An admitted
duplicate therefore restores its original snapshot even when the owner has
since removed routing or a consumer. This lookup does not authorize any new
effects under removed permissions; the future dispatcher must recheck them.

Before the root commits, uploaded chunks are only an incomplete candidate.
The inspect operation distinguishes absent, partial and committed storage.
Partial candidates require the original source body to retry; a restart cannot
reconstruct bytes that were never durably stored. Competing incomplete
candidates expire through the existing TTL cleanup.

## Recovery and authority

Recovery pages scan at most 32 interaction heads and preserve the underlying
UUID cursor. Chunks and other interaction kinds are skipped; an empty plan
page is not the end of the scan. Corrupt committed records are reported by ID
without exposing their contents. The list is not a consistent snapshot:
concurrent inserts can sort behind the cursor, so a future scheduler must
revisit the collection rather than treat one pass as complete.

Storage retention never extends an event reference's original expiry. Reads
can return a retained historical plan after that authority expires, but an
expired original candidate cannot create a new admission, including after TTL
deletion. Idempotency protection is bounded by retention; the store cannot
prove prior existence after TTL removes all records. Callers must not recreate
an expired event with freshly issued authority as an automatic retry.

Execution remains disabled until durable per-target scheduling, lease fencing,
consumer-scoped delivery/finalization and fresh owner/source/consumer authority
checks are implemented. Synchronous fanout must also enforce its primary reply
contract. Neither a stored binding nor an admitted root replaces those checks.

## Replica-mode compatibility admission

Installing the replica admission queue also makes the shared verified-ingress
fallback durable. Existing selected ordinary routing still uses its normal
resolver. When that path would use the legacy launcher, the gateway instead
retains one original launch under `replica_compatibility_payload_v1`: the exact
Activity, sealed event payload, consumer, provider binding, conversation,
partition key, idempotency key and verified source bytes. It does not add a
routing policy to an unrouted channel or fan out an already selected callback.
The original event expiry and the existing seven-day retention remain separate.

This covers the shared Custom, NyxID callback and native Telegram launch path,
including authenticated interactive callbacks after their existing interaction
store has selected and sealed the owning consumer. Existing dedicated native
Discord, Lark, WhatsApp and GitHub adapters retain their admission checks. GitHub
cannot enter the generic compatibility writer: its root still requires atomic
channel and integration guards. No additional agent-published producer has been
identified; routing/lineage types alone are not evidence of an available producer.

Compatibility admission verifies the original event attestation before reading
or writing a plan. A duplicate restores the winning snapshot before inspecting
current routing or credentials. Changed consumer, conversation, partition,
lineage or idempotency key cannot reinterpret a retained compatibility plan.
The dispatcher checks current owner, source, consumer, provider and credentials
before effects; only the explicit compatibility discriminator allows a current
channel without a routing policy. An admission root alone is insufficient for
success: every target must have acknowledged durable queue custody. Recovery
repairs a retained root after an interrupted enqueue.

New work requires configured provider replay support, supported delivery, an
accepting scheduler and enabled admissions. Missing verified bytes, storage or
queue failure never falls back to a local launch in replica mode. Without the
injected replica queue, the old selector and fallback behavior remain in force.
The new payload discriminator requires a compatible reader for rollback; it
must be included in the replica protocol-floor deployment cohort.

Connection completion uses an explicit injection:
`Arc<RoutedIngress>::replica_completion_launcher(Arc<dyn ActivityObserver>)`
returns `Arc<dyn TurnLauncher>` for `ConnectionService::start_completion_worker`.
It accepts only the internal `gateway.connection.ready` system-event shape,
verifies its sealed event, and persists canonical Activity bytes because no
external request body exists. It retains the original conversation and key.
The completion outbox treats `LaunchOutcome::Admitted` as durable gateway custody;
this does not claim provider acceptance or delivery. Replica-mode service
composition must install this launcher instead of the live turn runner.

These changes do not establish cross-process Sync delivery, public console
consistency, independent process/node recovery or loaded latency. Those service
composition and integrated acceptance gates remain separate.
