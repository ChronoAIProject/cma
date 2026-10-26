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

Routed tool sends use the same actual receipt path. Composition must set
`ToolsState.routed_replies = Some(Arc::new(delivery.tool_replies()))` alongside
routed execution; the default server wiring remains `None` in this foundation.
An immutable encrypted event-reference binding is written before the provider
can start, so a later channel owner or routing change cannot make an admitted
routed tool call fall back to legacy sending. Current caller, channel, source,
consumer and provider authority are still revalidated before each network
attempt. Direct tools and durable delayed reply targets use the hook.

A separate sealed CAS reply guard coordinates tool and automatic sends without
overwriting execution/projection ownership. It keeps only the latest tool and
automatic attempts, their actual bounded receipts, a sticky confirmed-tool bit,
and a terminal closed bit. Enqueueing never sets the confirmed bit. Projection
can query `tool_reply_status`; automatic delivery also atomically reserves this
guard, closing the lookup/send race even when a persisted projection selected
an answer before the tool receipt arrived. A proved tool reply suppresses a
later automatic answer across restart. An unresolved or unknown attempt blocks
new effects, including attempts after an earlier confirmed tool reply.

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

Every terminal frame, including `NoEffect`, must close the shared reply guard
before its cursor advances. Closing races tool admission through CAS: one can
win, never both. Pending tools defer acknowledgement; unknown tools quarantine
it. The already settled frame checkpoint is retained so a retry can finish
closing without sending again. Closed ordinary references refuse new tool
sends. An explicit delayed reply-target capability may continue through its
existing independent durable receipt path after a proved terminal close or
after original automatic-reply authority expires; unresolved sends cannot use
terminal closure to bypass the guard.

Default replies pin a nonsecret destination during initial preparation:
Telegram source/bot/chat/thread, relay source/platform/conversation route, or
the exact result-webhook URL and signing-key-id list. The destination binding
never stores the platform credential or signing key. Restored work retains that address.
Every attempt requires fresh channel configuration to authorize the same
destination and uses current credentials. Edits cannot redirect already
prepared work; a revoked or changed destination refuses new delivery. A settled
receipt can still be reused without sending, even after that configuration
changes. Delivery observations identify the default destination separately from
the origin.

Native Telegram uses the existing chat transport. Relay defaults use the
existing initiated-send contract with a stable target/frame/effect operation
identity and no fabricated inbound-message authority. Webhook defaults use the
existing fenced signing and transport path. Partial or uncertain delivery
retains the existing conservative `Unknown` outcome.

`validate_routed_delivery(channel, plan)` must run for new admissions before
`Candidate::prepare`. It refuses a missing/unauthorized configured default and
refuses Sync by default. An explicitly composed custom-stream lane may instead
call `validate_routed_delivery_with_sync(channel, plan, true)` and construct its
coordinator with `with_custom_stream(custom.clone())`, using the exact same
`CustomDelivery` and process-local waiter registry as custom HTTP ingress. These
are construction seams, not activation: existing constructors, ingress flags and
legacy delivery remain unchanged. Sync with a default destination remains
unsupported before admission in this bounded slice.

The routed custom stream records local evidence separately from every frozen
result-webhook leg. Local `Buffered` proves only that the bounded in-process
queue accepted a frame; it does not prove that the HTTP client read it.
`Unavailable` means that no waiter accepted the attempted frame, including
disconnect and backpressure. An interrupted enqueue reservation becomes `Lost`:
the original frame is never replayed, and any surviving local waiter is closed
with a `sync_delivery_lost` failure control. A restarted process has no old waiter;
there is no durable stream reconnect guarantee. The existing waiter count,
eight-frame queue, one-MiB frame limit and aggregate byte budget remain in force.

Only the primary consumer retains the local waiter. Its automatic text deltas
and committed messages are projected while automatic answers are enabled and
not suppressed by a confirmed tool reply. Terminal `cmaeg.completed` or
`cmaeg.failed` control is independent of that text policy: it is emitted even
with `auto_final=false`, tool suppression, or a terminal no-effect projection.
Suppressed completion controls contain no automatic answer text. Secondary
consumers cannot emit into or close the primary stream.

Terminal result webhooks remain independent sinks, with one bounded record per
frozen URL/signing-key-id pair (at most the channel's existing three-sink limit).
Every new attempt rechecks current authority and uses current credentials.
Local queue failure does not erase a confirmed webhook receipt. Each webhook's
intent is persisted before sending and its actual outcome before advancing;
an abandoned network intent becomes `Unknown` and is never automatically sent
again. Recovery skips settled webhook legs and may continue only legs that were
durably never started. Local stream outcomes never become platform `Confirmed`
or an aggregate platform receipt, and the public platform receipt types are
unchanged.

Local-only routed tool replies remain `Unsupported` before any local enqueue.
For explicitly enabled mixed Sync/webhook configurations, tool replies use only
the existing webhook transport and retain its real receipt and stable operation
key behavior. Their local tool frame is not supported in this slice; a webhook
receipt says nothing about the local reader. Legacy mixed tool delivery is
unchanged. This avoids returning `Unsupported` after emitting an unrecorded
local side effect and encouraging a caller to repeat it.

Tests exercise the real encrypted record HTTP/CAS protocol, delayed actual
engine sends, queue refusal, missing receipts, abandoned intents, authority
revocation, stale leases, sibling isolation, exact CAS read-back, lost cursor
acknowledgement after receipt persistence, expiry, changed frame/effect rejection
and terminal completion fencing. No test sends an external message.
Suppression tests additionally cover restart after actual tool delivery, lost
receipt persistence and lost write confirmation, concurrent tool/automatic
admission, stale writers, terminal no-effect closure, late tool refusal, sticky
confirmation followed by an uncertain send, and routing/owner revocation.
Sync tests additionally cover independent terminal completion, primary isolation,
missing and slow readers, process/worker loss around enqueue, real webhook
transport signing, separate mixed outcomes, settled webhook reuse and uncertain
network-intent fencing. All transports remain local fixtures; no external
message is sent.
