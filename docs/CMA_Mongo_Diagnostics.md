# Mongo request attribution

Mongo command and attributed connection diagnostics are **off by default** in code
and the checked-in API ConfigMap. Only exact
`CHRONO_MONGO_DIAGNOSTICS_ENABLED=true` installs the command observer when the
client starts. Missing, false or other values leave it off and retain the existing
pool telemetry. Detail-phase timers remain independently on by default;
`CHRONO_SANDBOX_DETAIL_TIMINGS_ENABLED=false` disables them.

The release controller explicitly enables command diagnostics in the planned
fix-candidate deployment and keeps them on through tonight's observation. Set the
environment together with that deployment; no additional diagnostic-only rollout
is required in the critical chain. Both controls are read at startup. They do not
enable optimized readers, admission, reconciliation or recovery behavior. Do not
include diagnostic controls when setting behavioral feature flags off.

## Event fields and privacy

`mongo_diagnostic` events have these structured fields:

| Field | Meaning |
| --- | --- |
| `event` | `started`, `succeeded`, `failed`, `checked_out`, `checked_in`, `connection_closed`, or `checkout_failed` |
| `pool` | `general` or `reporting`; never combine reporting-pool demand with general-pool contention; existing Prometheus pool label `application` corresponds to diagnostic `general` |
| `route` | HTTP method plus matched route **template**; `unmatched` is used without a template, never the raw URI |
| `request_hash` | SHA-256 of a process-random salt and the normalized request ID; correlation is confined to a process lifetime |
| `hold_hash` | Hash of a client-random salt and checkout sequence; joins commands to their eventual physical hold without exposing driver/server identities |
| `worker` | Fixed background work name, `request`, or `unattributed`; never a job/resource identity |
| `phase` | Fixed operation phase, including `auth`, `hierarchy`, `colors`, `names`, `final_fence` |
| `command`, `collection` | Allowlisted Mongo command and schema collection; unknown values become `other` |
| `duration_us` | Command duration, checkout wait or completed physical hold, according to `event` |
| `docs_returned`, `docs_returned_known` | Cursor batch length when available; zero plus false means unavailable, not a measured zero |
| `session`, `transaction` | Presence of an `lsid` and `autocommit:false`, respectively; no session/transaction identifiers |
| `connection_hold_elapsed_us` | Hold elapsed at command completion, or total hold on check-in/close; **not additive across command events** |
| `sample_every`, `slow`, `deferred` | Fast-event period, duration at least 100 ms, and delayed started-event emission |

Neither request headers nor BSON commands/replies, filters, database/server
names, resource identities, error text or inherited request spans are emitted.
The production full-text formatter respects each diagnostic event's explicit
root scope. If configuring JSON output, disable both current-span and span-list
inclusion: tracing-subscriber 0.3.23's JSON formatter otherwise falls back to the
active span even for a root event. The tests cover the production formatter and
JSON field capture with both inherited-span settings disabled.
Request context is task-local and is reinstalled on streaming-body polls.
Checkout context is retained until the asynchronous driver check-in callback.
Spawned schedulers, reconciliation, recovery assessment, metrics and capacity
settlement explicitly establish fixed work labels. `unattributed` is a coverage
gap to measure, not evidence of no background work.

## Sampling and accounting

All command completions and physical connection returns at or above **100 ms**,
and all failures, are selected for logging; queue overflow can drop any selected
record, including slow records and summaries. Every 100th fast command and checkout is sampled.
An unsampled command's started metadata is emitted at completion if it is slow
or failed (`deferred=true`). Deferred starts are not live in-flight observations.
No event writes command/reply bodies, including the failure path.

`mongo_diagnostic_summary` reports **unsampled** totals every ten seconds
while the client exists, with an activity-triggered fallback. Group by `pool, route, worker, phase, command, collection,
metric`. `metric` is `command`, `checkout`, or `hold`; `count`, `total_us`, `max_us`,
`docs_returned`, `failures` and `window_us` are numeric. Sum `total_us / 1e6` for
`metric=hold` to rank physical hold-seconds. Do not add command time, checkout
wait, phase time or scan time to physical hold time: these measurements overlap.
Use summaries for totals and sampled/slow details for request correlation. Join
command events to completed physical holds by `hold_hash` (plus Pod hash);
`none` means no matching tracked checkout.

A physical hold spanning multiple commands is labelled command/collection
`multiple` and retains the checkout context. It is not falsely assigned entirely
to the last command or duplicated into every phase. Command events retain their
actual dispatch phase. Server execution/queue time and query plans still need
server-side evidence; client command time includes transport and driver work.

Tracking is bounded to 4096 simultaneous holds and 4096 pending commands per
client. Summary cardinality is limited to 1024 groups plus overflow buckets;
`untracked` reports tracking overflow. Any overflow makes detailed attribution
incomplete and must be reported with the comparison. In-flight holds carry across
summary windows; completed-hold occupancy is not instantaneous utilization.
A periodic `mongo_diagnostic_pool` heartbeat also reports `tracked_commands`,
`tracked_holds`, `oldest_tracked_hold_us` and overflow, even if no command
completes. Its task keeps only a weak reference and exits after client disposal.

## Detail phases

Sandbox and Workspace authorization, hierarchy and final validation retain the
existing operation order and error/cancellation behavior. Sandbox optional color
and owner-name enrichment also carries its own phase. Workspace detail has no
color/name enrichment, so those phases are absent rather than fabricated zeroes.
The `sandbox_read_phase` target remains compatible with the existing collector;
`route` distinguishes Sandbox and Workspace. Its `elapsed_us` is phase wall time.

## Diagnostic production comparison

The first production diagnostic window MUST use develop plus this PR with every
behavioral feature flag off, including
`CHRONO_AGENT_RESPONSE_BOUNDED_READS_ENABLED=false`,
`CHRONO_SANDBOX_SNAPSHOT_READS_ENABLED=false` and
`CHRONO_SANDBOX_BATCHED_READS_ENABLED=false`. Keep fleet admission at 0 and the
explicit admission cohort empty. Keep the two diagnostic controls **on**.
Generation 182 had response projection enabled; generation 180 had all three
reader keys enabled. Neither is an all-flags-off candidate observation.

If the first window is clean, the release owner runs a second short window on
the SAME diagnostic image with only response projection enabled. Hold workload,
post-ready age and route sampling as comparable as practical; retain the exact
configuration for both windows. Use the labelled command, hold and phase events
to confirm or refute the response-projection treatment as the source of the
slowdown. A clean first window alone does not identify a command-level cause. Restored
images have also shown intermittent reporting holds and readiness deadlines;
compare collection/command totals across the same time buckets to distinguish
broad client-side stalls from query-specific work. Broad stalls still need
server/transport/scheduling evidence before assigning a server root cause.

The release owner runs the diagnostic build and retains image SHA, Pod hash,
Ready/old-gone timestamps, pool configuration, diagnostic controls and behavioral
flags. Compare matched post-ready ages and representative route samples. Keep
request IDs and resource identities out of evidence; retain only hashes and
aggregate values. Compare general-pool hold-seconds by route/phase/command,
checkout totals, returned-document counts and detail-phase p50/p95. Report
`unattributed`, overflow, command failures and route sample counts explicitly.

The driver version currently decodes complete BSON commands/replies before
calling a command observer, even though this observer reads metadata only.
Fast-event sampling reduces logging cost **but not that decoding cost**. The
local disposable-Mongo gate compares monitoring off/on with identical 25-, 200-
and 10,001-document reads in ABBA order; retain relative and absolute overhead,
command/check-out parity and hold measurements. Compare diagnostic builds with
the same observer setting. An uninstrumented old image versus an instrumented
new image cannot isolate the application regression from observer overhead.

If observer overhead materially changes the diagnostic window, the release owner
can disable command monitoring while retaining phase timings and the original
pool summaries. This limitation does not establish a production root cause.

## Local validation and accepted diagnostic overhead

At source checkpoint `78629bc4c`, strict clippy passed for CMA, layer2-core and
layer1-public across all targets/features. The targeted selection passed 132
unit tests and two disposable-Mongo tests, including privacy, transaction/error
paths, unchanged checkout counts and command-to-check-in hash correlation.

The native debug ABBA fixture measured 40 reads per mode at each size, with nested
1 KiB payload documents and monitoring as the only treatment:

| Documents | Off mean | On mean | Added time | Relative increase | Checkouts per mode |
| --- | --- | --- | --- | --- | --- |
| 25 | 1.079 ms | 1.263 ms | 0.184 ms | 17.1% | 40 |
| 200 | 4.351 ms | 6.441 ms | 2.090 ms | 48.0% | 40 |
| 10,001 | 173.546 ms | 301.588 ms | 128.042 ms | 73.8% | 200 |

These are local debug observations, not a production overhead estimate. The
callback-only test measured approximately 1.025 microseconds per event, which
excludes the driver's BSON conversion. The real-Mongo assertion is a catastrophic
regression ceiling (less than twice baseline plus 5 ms), **not** proof of low
overhead. The measured decoding cost remains a limitation, accepted for the
planned diagnostic night. An optimized comparison is recorded below.

The driver's raw command-event interface is private. Its optional OpenTelemetry
interface avoids whole-reply conversion with query text disabled, but does not
expose cursor batch lengths or the exact `autocommit:false` marker. It cannot
replace this observer while preserving every requested field without further
work or a driver API change.

An optimized standalone harness then included the actual observer source and
production text formatter writing to a sink, with the same driver, a disposable
replica set and 80 reads per mode/size. It includes task-local context, command
monitoring, CMAP tracking, hashing, summaries and sampled formatting. It excludes
the full API, remote transport and log-sink I/O. One fixture collection is
allowlisted in the harness; no production data is used.

| Documents | Off mean | On mean | Added mean | Off/on p50 | Mean change |
| --- | --- | --- | --- | --- | --- |
| 25 | 0.576 ms | 0.510 ms | -0.066 ms | 0.436 / 0.466 ms | -11.5% |
| 200 | 1.704 ms | 1.906 ms | 0.202 ms | 1.470 / 1.675 ms | +11.9% |
| 10,001 | 76.191 ms | 92.642 ms | 16.451 ms | 69.939 / 85.386 ms | +21.6% |

The negative small-read mean is measurement noise, not a speedup claim; its p50
increased. A separate minimal callback measured 8.615 ms added for 10,001
documents, confirming a driver-level cost without the observer. Neither
comparison establishes a production overhead bound. The lead accepted this cost
for tonight's diagnostic deployment: the measured approximately 0.2 ms on
200-document reads is small, while production full scans are reported at
1.3–1.7 seconds and are targeted by segmentation. That production context is a
rollout decision, not a conversion of the local fixture into a production
measurement. Full command coverage is needed to diagnose from real logs.

A cheaper observer is tracked in #1765, not a prerequisite for this diagnostic
window. Preserve the required metadata and compare optimized fixture results
before changing the implementation. Phase timers and existing pool metrics can
remain enabled with command monitoring off; that mode loses collection/command
attribution.

## Nonblocking output and loss accounting

Driver callbacks enqueue fixed metadata records with `try_send` into one bounded
4,096-record queue. A dedicated OS thread performs tracing formatting and sink
I/O. Detail events, attributed summaries, heartbeats, existing pool summaries and
reporting-circuit messages all use this path. The callback retains only bounded
metadata accounting and fixed-size hash work; it never waits for output capacity
or formats a log line. No BSON payload is retained in the queue. Circuit decisions
and existing metrics remain synchronous so logging pressure cannot defer them.

A full or disconnected queue drops the record and increments an atomic loss
counter. Every ten seconds the writer reports `mongo_diagnostic_output` with
`dropped_total` (cumulative for this process, across both pools) and
`queue_capacity`. If the sink stalls, that report waits on the writer thread;
the counter remains available to that thread when output resumes. A permanently
failed writer cannot report through the failed sink. Missing heartbeats are a
coverage failure, not evidence of zero work.

Each emitted record includes `queue_delay_us`. Log timestamps reflect emission;
backlog can shift records into later wall-clock buckets. Nonzero drops mean even
unsampled aggregates may be absent, and queued records are lost on abrupt process
exit. Report loss and queue delay with comparisons. This is best-effort diagnostic
output, never an audit trail. Command monitoring remains default off; if this fix
misses the candidate cutoff, keep it off with detail timers on as directed by the
release plan.
