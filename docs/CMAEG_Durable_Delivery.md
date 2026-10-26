# Durable downstream receipts for routed replay

This opt-in foundation does not activate routed ingress or change existing live
delivery. It extends the encrypted execution record described in
`CMAEG_Routed_Execution.md`; no new database collection or external service is
required.

`RoutedDelivery::deliver_frame` receives the committed event identity, original
target ID, current opaque lease, authenticated `RecoveryEvent` and the explicit
effect selected by the caller's projection. `ReplayEffect::NoEffect` records that
this frame has no downstream send. `ReplayEffect::Reply` passes the exact selected
message to the existing bounded delivery engine. This layer does not invent a
new response projection: the caller must durably preserve any projection state
needed to reconstruct later effects before acknowledging a no-send frame.

The operation reloads the original plan/body and checks the target, owner,
request hash, response, current channel/source/consumer/provider authority and
lease. Raw and parsed event representations must agree. Numeric frames are
contiguous, beginning at zero when no cursor exists. A bounded checkpoint in the
same sealed CAS record stores the response ID, frame number, hashes of the exact
frame and effect, terminal status and downstream outcome. It retains only the
latest checkpoint; plaintext messages and raw provider frames are never logged.

For a send, durable `Intent` must be confirmed before enqueueing. The new
`DurableReply` command waits for the actual transport `IntentReceipt`, not queue
acceptance. Its consumer-qualified event key uses the existing fixed eight
workers and bounded queues, pacing, rendering and transport. The replay path uses
the channel credential rather than reusing a single-use ingress reply token.
Legacy `Reply` and `StreamOutcome` paths retain their behavior.

The send window is at most 20 seconds under a freshly renewed 30-second lease.
Every network attempt rechecks current authority, the captured channel version,
lease and original event expiry after pacing. A synchronous commit guard checks
the cancellation flag and deadlines immediately before the first effect.
Cancelling or timing out the owning future disables any still-queued send.
An already transmitted request cannot be revoked; no exactly-once guarantee is
claimed. Native media operations exceeding this window remain conservatively
unknown and require reconciliation.

The actual confirmed or refused receipt is committed before the separate cursor
CAS. A lost write response is accepted only after exact value/version read-back.
If a receipt was persisted but cursor acknowledgement failed, replay reuses the
matching receipt without another send. Changed raw frames or effects at the
same cursor fail closed. Queue refusal proves no send and has a durable rejected
receipt. Rate limiting and unsupported effects are also explicit settled
refusals, not successful delivery.

Lost worker receipts, timeouts and transport uncertainty persist `Unknown` and
leave the cursor unchanged. An `Intent` found after interruption also becomes
`Unknown`; neither that frame nor a later frame may cause another send or cursor
advance. There is no automatic reconciliation or blind retry. Operators need
explicit platform evidence before any future reconciliation feature can settle
these states.

`Journal::acknowledge_delivery` now requires the matching settled checkpoint.
`Journal::complete` requires a settled, acknowledged terminal-frame checkpoint.
Simply reading a provider frame cannot advance or complete execution. Existing
settled receipts can finish acknowledgement after reply authority expires, but
new effects retain the original expiry.

Ephemeral synchronous waiters cannot provide durable completion evidence and
are explicitly unsupported in this lane. Default-destination projection is
also explicitly unsupported until it has its own durable authority binding;
it never falls back to the origin. These limitations do not affect the live
default lane. Activation additionally needs the scheduler and projection state,
tool-reply suppression, per-platform recovery evidence and a shared Sync
finalization design.

Tests exercise the real encrypted record HTTP/CAS protocol, delayed actual
engine sends, queue refusal, missing receipts, abandoned intents, authority
revocation, stale leases, sibling isolation, exact CAS read-back, lost cursor
acknowledgement after receipt persistence, expiry, changed frame/effect rejection
and terminal completion fencing. No test sends an external message.
