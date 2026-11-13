# Readiness deadline evidence

`CHRONO_MONGO_DIAGNOSTICS_ENABLED=true` enables readiness evidence together with
Mongo command diagnostics. Only exact `true` enables it; the existing code and
ConfigMap default remains false. This setting is read at process startup. No
additional flag, production activation, pool, probe or timeout change is made.

The readiness check still uses the general application Mongo client, a two-second
deadline, a one-second delay after completion and a five-second cache-age limit.
GET `/ready` still serves cached state with the existing response shape. The
diagnostic path is disabled when the control is absent, false or malformed.
When enabled, the existing readiness state-change message also uses the bounded
writer after publishing cached state, so that logging cannot hold publication.
Its fields/message are preserved, with source time and queue delay added. Gate
collectors must treat output loss or stale output as missing evidence.

## Records and interpretation

Each process generates an opaque random `process_hash`. Join it with the
collector's Pod/image/generation binding. `check_sequence`, `operation_sequence`
and `event_sequence` are process-local ordinals, not driver, user or resource
identifiers. A readiness operation has a distinct check sequence; Mongo commands
have distinct operation sequences. Join commands and eventual physical returns
with the existing salted `hold_hash`. A zero check sequence means no readiness
attempt owns that event; a zero operation sequence means no command was assigned.

| Target | Evidence |
| --- | --- |
| `readiness_diagnostic` | Every check start and completion, including repeated failures and cancellation; last observed phase, elapsed time, two-second budget/deadline, wake lag, phase age and deadline overshoot. Every non-ready response includes its cached check sequence, reason, cache age and check duration. |
| `readiness_diagnostic_ring` | Command span start/known success/known failure/unknown completion, checkout start/success/failure, physical return/close and pool lifecycle metadata. Includes original event time, route template, fixed worker/phase, pool, command/collection and hold correlation. |
| `readiness_diagnostic_snapshot` | Frozen preceding 30 seconds at every deadline, outstanding tracked commands/holds (including older holds), source interval, row counts and loss totals. |
| `readiness_diagnostic_output` | Writer heartbeat, cumulative output loss and fixed queue capacities. |

Response reason `workers` means the cached dependency check is healthy but a
required scheduling capability is unavailable. Its check sequence still joins
the cached Mongo check; it does not attribute the worker refusal to Mongo. When
the dependency also fails, its specific reason takes precedence. Worker recovery
restores readiness only while the dependency remains healthy and fresh.

Events retain source `observed_unix_us` anchored to process startup wall time and
`process_monotonic_us`, independently of emission time. `queue_delay_us` measures
writer delay. Use source time for joins; wall-clock corrections after startup do
not alter the monotonic timeline. Every ring row retains its original sequence;
deduplicate overlapping snapshots by process hash plus event sequence. Snapshot
active rows are references to outstanding work, not additional completed holds.

The phases are `pre_checkout_selection_unknown`, `checkout`, `checked_out`,
`command`, `command_completed`, `checkout_failed` and `command_failed`. The
operation scope is retained until asynchronous connection return. Cancellation
records the last confirmed phase. The metadata hook emits `command_completed`
with `outcome_known=false` when a span ends without a positive driver result
marker, including cancelled commands and successful pings. It removes that
command from outstanding tracking but does not advance the attempt phase. Known
success/failure advances the phase; the readiness attempt result separately
records the actual ping result. Never count an unknown completion as success.
`transaction_known=false` means transaction membership is unknown, not false.
Pre-checkout time is not asserted to be server-selection time. Command elapsed
time includes driver/transport work and does not identify server execution time.

An independent one-second timer records scheduled-versus-actual wake delay and
stops with the health monitor. It identifies runtime delay, not the task causing
it. The readiness loop separately records its own wake delay and deadline
overshoot. Diagnostics do not change the public response or extend its deadline.

## Bounds, loss and accounting

Capture uses a per-process 8,192-event ring with a 30-second age horizon, plus at
most 4,096 outstanding holds and 4,096 commands. It uses `try_lock`; competing
callbacks never wait for capture or snapshot locks. Under high traffic, capacity
can shorten the retained history; capacity eviction and tracking/capture loss are
explicit. An incomplete snapshot is not a complete 30-second trace.

Output follows the bounded `try_send` and dedicated-writer pattern used by Mongo
diagnostics. There are 512 readiness records, 512 post-deadline metadata events and a
separate two-snapshot queue. Metadata cannot consume the readiness-record lane.
At most one further snapshot is being written; each snapshot has the same fixed
ring/active caps. No driver or request task formats or writes these records.
Each deadline freezes its own view. For 30 seconds after the latest deadline,
new metadata is also offered once to the normal output queue; overlapping
post-deadline windows do not multiply each event. Output overflow drops evidence,
never application work. The snapshot lane remains bounded when stdout is stalled.

`cma_readiness_diagnostic_loss_total{reason}` exposes initialization, busy capture,
capacity, normal-output and snapshot-output loss independently of the log sink.
`cma_readiness_diagnostic_queue_delay_seconds` measures emitted-record delay;
absence of fresh output is unknown, not zero delay or zero loss. Readiness timer
lag has a gauge and a duration histogram; wake lag and deadline overshoot have
histograms. `cma_readiness_diagnostic_responses_total{reason}` counts all responses
without sampling. No per-check/request/resource labels enter metrics.

Metadata contains no BSON, filter, command reply, raw error, server address,
database, owner/Agent identity or inherited span. Registered route templates and
fixed workers are retained; request correlation remains a process-salted hash.
The metadata-only OpenTelemetry hook inherited from #1813 avoids public command
callbacks and their eager reply BSON decoding. Query-text collection is disabled;
no payload enters the observer. Command timing uses `driver_command_span`, while
physical hold timing keeps the CMAP boundary. Returned-document counts and exact
transaction state are unavailable. Driver feature metadata construction when the
control is off remains the accepted residual tracked separately in #1815.

Rank physical holds once, by route/worker and pool. Do not add command time,
checkout wait, phase time and hold time: they overlap. Report loss, active-tracking
overflow and unowned work alongside findings. Checkout-start counts alone are not
queue depth because a cancelled checkout may have no end event.

## Deployment and remaining limits

This is for a follow-up image. The release controller must qualify the exact
image and live diagnostic setting, collect a fresh 0% comparator and use the
same diagnostics at 10%. All admission readiness, correctness, circuit, rollback,
ownership and image-floor gates remain unchanged. A trigger still rolls back
immediately; evidence collection never postpones the kill switch.

Exact server selection and server-versus-network attribution remain unavailable.
No raw driver DEBUG logging is enabled. Cohort-level command exposure/per-Agent
benefit instrumentation is separate. The bounded recorder may lose data under
pressure; its loss must invalidate completeness claims. A restart resets all
ordinals and evidence, so never combine process lifetimes without their binding.
