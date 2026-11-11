# Retained live-fragment budget

Part of #1472. `CHRONO_AGENT_LIVE_BUFFER_BUDGET_ENABLED` is a strict boolean,
default `false`. Enable it through a separately qualified staged release.
No persistent schema or data migration is involved.

## Behavior and accounting

When enabled, all `DurableAgentUpdateRecorder` instances in one CMA process share
one **32 MiB** reservation pool for retained live fragments. Each Agent still has
its existing 256-fragment broadcast ring and each text fragment is at most
16 KiB, split on UTF-8 boundaries. No subscriber means no retained fragment.

The producer creates a temporary fragment, measures its actual string capacities,
and obtains a non-waiting byte permit before putting it in shared storage. The
charge includes the fragment structure, its permit, two Arc reference counts,
and the capacities of its Agent ID, message ID and delta strings. Subscriber
copies and deferred v2 source frames hold an Arc to that same immutable value;
they retain the reservation until the final copy is dropped. Another recorder,
Agent or subscriber cannot obtain a separate budget by cloning a service.

If capacity is unavailable, publication inserts an ordered gap marker instead
of retaining the text. The subscription exposes this as the existing typed
`Lagged(1)` result. AG-UI emits its conversation-gap delta and Agent v2 emits
`cma.agent.live_gap`; Responses v1 follows its existing durable catch-up policy.
This is a stream-level signal after HTTP headers, not a new HTTP refusal.
Accepted fragments precede their gap marker; later recovered fragments follow
it. Normal broadcast eviction can coalesce multiple skipped items into its
existing lag count. Gap counts count missed stream items, not bytes.

The live lane contains only provisional message/thought text. Runtime receipts,
durable commits and terminal events do not travel through it. Capacity pressure
never waits for a viewer and never invents a runtime ACK. Message reconciliation
continues from the canonical durable transcript. Provisional thoughts retain
the existing live-only/loss-on-lag contract; this does not make them durable.

Enabled v2 delivery keeps source fragments immutable and does not coalesce them
into a new unreserved allocation. This can increase the number of live frames;
text, ordering, runtime/turn identity, committed-message suppression and final
authority checks keep their existing contracts. Disabled delivery preserves
owned-string cloning and the previous v2 coalescing behavior.

## Limits of this slice

This is a retained-source storage bound, **not** the complete 128 MiB ingress
qualification or a total RSS cap. It excludes allocator bookkeeping/rounding,
broadcast slots, lane/subscriber bookkeeping, temporary producer and decoder
allocations, runtime startup buffers, message assembly, and copies made while
serializing public events or projecting complete replay pages. Those allocations
require separate budgets and measured decoder/RSS evidence. Public stream
admission (#1683) and hosted update backpressure (#1688) are independent flags;
none of these three slices alone certifies M-CONC-02.

The pool is per process, not a distributed fleet reservation. A slow observer
can retain reservations and cause other live lanes to report gaps; tenant
fairness and observer/runtime capacity mapping remain separate acceptance work.
There is no live permit reset, timeout release or hot increase. Reservations
remain owned by their storage until actual drop.

## Validation and release

Local contracts cover shared lanes, concurrent refusal without overshoot,
actual retained string capacities, immutable subscriber clones, deferred-clone
lifetime, ordered gaps/recovery, UTF-8 splitting, eviction, no-observer behavior,
cancelled receive, unsubscribe, final-drop recovery and disabled compatibility.
Recorder coverage verifies enabled delivery alongside durable message/terminal
recording. Existing public stream tests retain authority, generation, turn,
commit/catch-up and lag ordering assertions.

Before activation, pin the source/artifact and effective configuration, then
measure retained source usage plus the explicitly excluded allocations under a
reviewed isolated workload. Bind the limit to the shared ingress/RSS envelope
and verify stream frame-rate/latency after coalescing is disabled. If unknown,
keep the flag off; this document does not approve production saturation.

A production control, only in a lead-coordinated security-harness slot, may use
at most two observers of existing authorized fixtures, ten API requests and a
two-minute window, with zero Sandbox launches and zero model turns. Do not fill
the shared pool to force refusal. Abort on any unauthorized data, failed owner
control, unexpected 429/503, candidate drift or health regression. Close both
bodies, release tracked sessions and verify teardown. Capacity exhaustion is
proved by isolated local contracts until a separately reviewed harness phase
and its bounded execution profile are relayed.

Rollback disables the flag in a staged replacement process. Existing in-process
reservations are never reset while subscribers hold data. Preserve durable
history and the retained-fixture ledger during both activation and rollback.
