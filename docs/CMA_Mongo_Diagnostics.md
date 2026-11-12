# Mongo request attribution

The API emits diagnostic command, physical connection and detail-phase events by
default. This is observability only: it does not enable optimized readers,
admission, reconciliation or recovery behavior. Diagnostic builds retain the
behavioral flags selected by the release owner.

`CHRONO_MONGO_DIAGNOSTICS_ENABLED=false` disables command/attributed connection
monitoring at process startup, while retaining the existing general-pool latency
summaries. `CHRONO_SANDBOX_DETAIL_TIMINGS_ENABLED=false` independently disables
detail-phase events. Both default to on, including the API ConfigMap; changing
either requires the release owner's Pod rollout. Do not include diagnostic
controls when setting *behavioral* feature flags off for the experiment.

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
Request context is task-local and is reinstalled on streaming-body polls.
Checkout context is retained until the asynchronous driver check-in callback.
Spawned schedulers, reconciliation, recovery assessment, metrics and capacity
settlement explicitly establish fixed work labels. `unattributed` is a coverage
gap to measure, not evidence of no background work.

## Sampling and accounting

All command completions and physical connection returns at or above **100 ms**,
and all failures, are logged. Every 100th fast command and checkout is sampled.
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
