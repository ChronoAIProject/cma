# Durable routed execution journal

This is a disabled execution foundation, following
`docs/CMAEG_Routed_Admission.md`. It does not connect ordinary ingress to routing,
install a scheduler, configure a provider capability, or activate production
fan-out. A concrete provider adapter and durable platform delivery journal remain
required before activation. Existing ingress and default launch behavior are
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
platform delivery remains separate work.

Tests use the real `RecordClient` HTTP transport against a bounded record server
that enforces create-only writes, CAS, expiry and lost-response faults. They cover
competing claims, stale workers, unknown CAS confirmation, large bodies,
credential rotation, corruption, authority changes, event expiry, unknown-start
recovery, accepted replay, cursor fencing and synchronous waiter ownership.
These tests do not replace a real database concurrency test or evidence for a
concrete provider's idempotency and replay contract.
