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
