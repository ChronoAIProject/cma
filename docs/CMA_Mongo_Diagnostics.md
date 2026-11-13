# Mongo request attribution

Mongo command and attributed connection diagnostics are **off by default** in code
and the checked-in API ConfigMap. Only exact
`CHRONO_MONGO_DIAGNOSTICS_ENABLED=true` installs the metadata observer when the
client starts. Missing, false or other values leave it off and retain the existing
pool telemetry. Detail-phase timers remain independently on by default;
`CHRONO_SANDBOX_DETAIL_TIMINGS_ENABLED=false` disables them.

Tonight's candidate keeps command diagnostics **off**. The metadata-hook fix
requires review and a later image before the release controller enables it for a
diagnostic window. Both controls are read at startup. They do not enable optimized
readers, admission, reconciliation or recovery behavior. Do not include diagnostic
controls when setting behavioral feature flags off.

## Event fields and privacy

`mongo_diagnostic` events have these structured fields:

| Field | Meaning |
| --- | --- |
| `event` | `started`, `succeeded`, `failed`, `completed` (outcome unknown), `checked_out`, `checked_in`, `connection_closed`, or `checkout_failed` |
| `pool` | `general` or `reporting`; never combine reporting-pool demand with general-pool contention; existing Prometheus pool label `application` corresponds to diagnostic `general` |
| `route` | HTTP method plus matched route **template**; `unmatched` is used without a template, never the raw URI |
| `request_hash` | SHA-256 of a process-random salt and the normalized request ID; correlation is confined to a process lifetime |
| `hold_hash` | Hash of a client-random salt and checkout sequence; joins commands to their eventual physical hold without exposing driver/server identities |
| `worker` | Fixed background work name, `request`, or `unattributed`; never a job/resource identity |
| `phase` | Fixed operation phase, including `identity` (ambient credential resolution, before the handler), `auth`, `hierarchy`, `colors`, `names`, `final_fence` |
| `command`, `collection` | Allowlisted Mongo command and schema collection; unknown values become `other` |
| `duration_us` | Command duration, checkout wait or completed physical hold, according to `event` |
| `docs_returned`, `docs_returned_known` | The metadata hook has no batch-length field: zero plus false means unavailable, not a measured zero |
| `session` | Presence of session metadata; the session identifier is discarded |
| `transaction`, `transaction_known` | False plus false: the hook does not expose exact `autocommit:false`; retryable-write transaction numbers are not treated as transactions |
| `outcome_known` | True for a reported error or a successful cursor result; false for other completions, including cancellation |
| `command_duration_scope` | `driver_command_span`; includes driver command processing, not server execution time alone |
| `connection_hold_elapsed_us` | Hold elapsed at command completion, or total hold on check-in/close; **not additive across command events** |
| `sample_every`, `slow`, `deferred` | Fast-event period, duration at least 100 ms, and delayed started-event emission |

A per-client OpenTelemetry provider consumes a fixed set of allowlisted driver
attributes and has no exporter or global-provider registration. Query text is
explicitly disabled (`query_text_max_length=0`), including when an environment
variable requests it. The eager public BSON command handler is never installed.

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
No event writes command/reply bodies, including the failure path. The driver
redacts some sensitive command spans entirely. Completed spans without a positive
result marker use `completed` with `outcome_known=false`; this includes dropped
operations, so cancellation never fabricates a successful command. A driver
cursor-result marker establishes success for find/aggregate. The driver exposes
only the input cursor ID for getMore, so its completion is unknown too. Other
successful commands have no equivalent marker in this API. Pool accounting remains complete
independently of command-span coverage.

`mongo_diagnostic_summary` reports **unsampled** totals every ten seconds
while the client exists, with an activity-triggered fallback. Group by `pool, route, worker, phase, command, collection,
metric`. `metric` is `command`, `checkout`, or `hold`; `count`, `total_us`, `max_us`,
`docs_returned`, `docs_returned_known_count`, `outcomes_unknown`, `failures` and
`window_us` are numeric. The metadata hook has zero known document counts; do not
interpret its document total as a measured zero. Unknown outcomes are counted
separately from observed failures. Sum `total_us / 1e6` for
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
checkout totals and detail-phase p50/p95. Returned-document counts are unavailable
from this observer and need separate evidence. Report
`unattributed`, overflow, command failures and route sample counts explicitly.

The public BSON command handler in MongoDB 3.9.1 copies and decodes complete
replies before invoking a callback. This implementation instead uses the driver's
supported OpenTelemetry metadata hook with query text disabled. The callback
never receives reply documents. It retains only allowlisted names, bounded
tracking keys and task-local labels, with no payload sizing or hashing.

This changes diagnostic coverage: exact returned-document counts and transaction
markers are unavailable, and some completion outcomes are unknown as described
above. Command duration now covers the driver's command span, including response
handling. Compare diagnostic windows only with matching observer implementations;
do not mix the old callback's timings with the new span timings as if identical.
Physical hold timing still comes from the same CMAP checkout/check-in boundary.

Compiling the driver's OpenTelemetry feature also constructs bounded session and
command metadata even with tracing disabled. This is independent of reply size;
the off/on ABBA fixture uses the same build, and does not establish zero change
against an older binary compiled without that driver feature. The control remains
default off and overrides the driver's environment defaults. Server-side query
plans and returned-row counts still require separate evidence.

## Historical public-callback measurements

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
overhead. This historical observer was superseded after the isolated CI failure
below. An optimized historical comparison is recorded below.

The driver's raw command-event interface is private. Its optional OpenTelemetry
interface avoids whole-reply conversion with query text disabled, but does not
expose cursor batch lengths or the exact `autocommit:false` marker. It cannot
preserve every original field without a driver API change. The replacement
therefore marks those fields unknown explicitly, as described above.

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
comparison establishes a production overhead bound. The original rollout
decision accepted this local cost: the measured approximately 0.2 ms on
200-document reads is small, while production full scans are reported at
1.3–1.7 seconds and are targeted by segmentation. That production context is a
rollout decision, not a conversion of the local fixture into a production
measurement. The subsequent exclusive CI failure invalidated that overhead
expectation; tonight's candidate keeps diagnostics off.

The metadata-only replacement is tracked in #1765 and requires its own
review and a later image. Phase timers and existing pool metrics can
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

## CI overhead isolation

The real-Mongo diagnostics contract runs once on its own disposable replica set,
after report scale, owner allocation and audit bounds, before the concurrent ordinary Mongo
partitions. The coordinator waits for fixture cleanup before starting the next
phase. `bash .github/scripts/mongo-contract-tests.sh --diagnostics` selects the
same test directly; CI retains successful output as well as failure output.
The 40 reads per mode, ABBA order, three document sizes, privacy and correlation
checks, equal checkout counts and `on_mean < 2 * off_mean + 5,000 us` ceiling are
unchanged. This is a test-scheduling change, not a runtime optimization or a
relaxation of the guard. There is no retry-until-pass policy.

PR #1779 job 131174448380 failed with unchanged diagnostics code and driver:

| Documents | Off mean (us) | On mean (us) | Off/on p50 (us) | Checkouts per mode |
| --- | ---: | ---: | --- | ---: |
| 25 | 2,030 | 3,181 | 1,974 / 3,327 | 40 |
| 200 | 9,818 | 19,454 | 9,673 / 20,177 | 40 |
| 10,001 | 428,278 | 871,132 | 417,531 / 863,294 | 200 |

The large-read mean exceeded the 861,556 us limit by 9,576 us. Its p50 also
exceeded the analogous relative bound, so replacing the mean with a median
would not explain that sample. The same test passed in develop run 43200000706,
attempt 2, job 131136521321, in 26.798 seconds versus 55.560 seconds here.
Successful-test output was suppressed, so that run's means are unavailable;
the complete develop Mongo job failed for a different contract.
Both runs overlapped two disposable Mongo fixtures. This supports investigating
load sensitivity but does not establish a production regression or prove runner
contention caused the failure. The exclusive run removes this known confounder
and makes the next measurements available for comparison. A failure in that
controlled run must still be investigated; the threshold remains a hard gate.

## Isolated CI failure and metadata replacement

The exclusive fixture failed again in job 131225672805: 10,001-document means
were 211.169 ms off / 440.221 ms on, physical holds 8.952 s / 18.102 s, and both
modes used 200 checkouts. Thus fixture contention does not explain the overhead.
A local no-op public callback still raised the same ABBA read from 202.359 ms to
337.557 ms (holds 3.224 s to 8.660 s). No application queue, hashing or logging
existed in that treatment; the driver event copy/decode path causes the scaling.
In MongoDB 3.9.1 this is the raw reply clone in `src/client/executor.rs:698`,
the pre-handler conversion in `src/client.rs:373`, and owned BSON conversion
in `src/event/command/raw.rs:53`. The application queue contains metadata only.
Neither payload hashing nor payload sizing caused this observed cost.

The replacement keeps the 40-read ABBA workload and the exact
`on_mean < 2 * off_mean + 5,000 us` assertion. The collector's completed spans
carry only metadata and use the existing nonblocking queue. Privacy, error,
cancellation, context separation, pool correlation and explicit unknown fields
have targeted tests. The measurements below use the complete installed observer; a no-op telemetry
probe alone is not release evidence.


## Complete metadata-hook fixture

Rust 1.96.1 native debug, MongoDB 3.9.1/BSON 3.1.0, disposable MongoDB 8.0.13
replica set (2 CPUs, 2 GiB, 0.25 GiB WiredTiger cache). Each mode reads 40 times
in the same ABBA blocks, with nested 1 KiB documents, batch size 2,048 and pool
size 30. The measured source head after merging develop is `137703e39`. The observer
includes task-local attribution, CMAP tracking, hashes,
summaries and the nonblocking production text formatter. The API, remote
transport and production log-sink I/O are outside this fixture.

| Documents | Off/on mean ms | Mean change | Off/on p50 ms | Off/on physical hold total s | Checkouts each |
| --- | --- | --- | --- | --- | --- |
| 25 | 1.294 / 1.584 | +22.4% | 1.066 / 1.242 | 0.050556 / 0.062151 | 40 |
| 200 | 4.921 / 4.342 | -11.8% | 3.993 / 3.985 | 0.198674 / 0.174826 | 40 |
| 10,001 | 167.825 / 164.746 | -1.8% | 165.155 / 161.951 | 6.855596 / 6.711591 | 200 |

The unchanged overhead ceiling passed at every size; results and checkout counts
matched, privacy checks passed, and both sampled physical holds joined to their
originating command completion, request hash and phase (2/2). The separate
real-driver insert/update/find, transaction commit and failure fixture also
passed, as did 25 targeted diagnostics/pool/read-telemetry unit tests.

An earlier timing trial measured off/on means of 2.126/3.264, 4.907/4.680 and
188.123/182.270 ms at the same sizes. Its overhead assertions passed, but its
later correlation assertion still selected only `succeeded`; it failed on the
explicitly unknown getMore outcomes. The final assertion above includes
`completed` only with `outcome_known=false` and still requires the same hash and
phase joins. Neither the ABBA workload nor its latency ceiling changed.

The qualifying run before merging develop also passed all checks, with off/on
means 1.776/1.298, 5.025/6.120 and 172.505/170.826 ms respectively.

Small-read means varied in direction between trials; negative differences are
not speedup claims, and this is not a production overhead bound. All three large-read
trials remove the previously measured doubling. On successful reads, allocation
work is limited to command/session metadata in the driver and fixed fields/Arc
labels in the observer; no heap-allocation profiler was run. Driver error handling
can still construct error strings internally; the observer discards them. Server-side stalls and the
application's original regression still require the planned production window.

## Cookie identity attribution

The fixed `identity` phase covers ambient request credential resolution only,
including the Cookie login read, subject read, verification custody recheck and
`GET /auth/session` login recheck. The handler resumes its original phase, so
its settings read and any later reads of the same collections are excluded.
`cma_cookie_identity_resolutions_total{source="led"|"shared"}` counts callers
that led an online verification or shared an overlapping verification. The
matching debug event `cookie_identity_resolution` carries only that fixed source
and count. `reused` is reserved for a separately approved reuse policy; this
implementation never emits it. Tokens, Cookie values and subjects are not labels.

With the browser-session join disabled, successful ordinary Cookie requests
issue three identity `find` commands; session inspection issues four. With
`CHRONO_AUTH_BROWSER_SESSION_JOIN_ENABLED=true`, these become one `aggregate`
on `auth_browser_logins` and one or two `find` commands respectively. Mongo
attributes the aggregate to its outer collection; the joined subject read does
not appear as a separate client command. Server explain evidence is needed to
account for both indexed document reads.


## Unsampled duration distributions

Every `mongo_diagnostic_summary` row includes `duration_bucket_bounds_us` and
`duration_bucket_counts` for its existing `pool, route, worker, phase, command,
collection, metric` group. Bounds are fixed inclusive upper limits, in microseconds:
`[1000, 5000, 10000, 25000, 50000, 100000, 250000, 500000, 1000000, 5000000]`.
The eleven counts are **non-cumulative**: the first includes zero through 1 ms,
the second is greater than 1 ms through 5 ms, and the last is greater than 5 s.
Counts sum to the row's `count` unless the saturating counters have exhausted
`u64`. Durations above a boundary by even one nanosecond enter the next bucket.
Arrays use the tracing debug-array representation in both text and JSON fields.

These counts include every recorded observation, including failures and unknown
command outcomes; report the existing failure/unknown counts beside them. Sum
matching buckets across complete windows and select the bucket containing the
nearest-rank p50/p95 to report a **range**, not an exact percentile. For the
#1806 25 ms threshold, the first four buckets divided by the total show the exact
fraction at or below that boundary. This does not establish a percentile for
command-plus-checkout latency: their sum requires correlated measurements and
cannot be obtained by adding percentiles. Command rates and general-pool command
shares continue to use unsampled `count` and the observation window duration.

The histogram adds eleven `u64` counters (88 bytes) per existing bounded group;
it adds no label dimensions, retained samples, payload inspection or independent
configuration. It is present only when the existing default-off Mongo diagnostic
observer is installed. The existing queue/drop/overflow caveats still apply:
nonzero dropped or overflow rows make collection-specific conclusions incomplete.
The histogram measures client-side command/checkout/hold durations. It does not
identify majority acknowledgement as the sole cause of a write/read difference,
and it does not enable Default-tier prepaid leases or change rate-limit behavior.

### Agent file preview phases

`CHRONO_AGENT_FILE_READ_TIMINGS_ENABLED` is a strict boolean, default `false`.
When enabled, authorized file-content/download reads emit `agent_file_read_phase`
with fixed `phase`/`outcome` labels, registered route, opaque request hash and
`elapsed_us`. No file path, content, runtime address or credential is recorded.

The serial outer phases are `auth`, `runtime_status`, `runtime_read`,
`receipt_validation`, and `final_fence`. Runtime status includes
`home_before_status`/`home_after_status` verification; both runtime status and
file transfer include nested `status_endpoint`/`status_control` and
`read_endpoint`/`read_control` transport timings respectively. Native assignment,
policy and Pod validation after status is `status_post_validation`.
Nested phases overlap their parent: correlate by request and do not add all
phase durations or percentiles. Setup, path validation and other unmeasured
work can leave gaps; missing phase records do not establish zero work. Errors and cancelled futures retain their
outcome. Timings are elapsed waits, not CPU attribution. Native assignment
validation remains inside runtime status, outside its control exchange.

The browser bounds an optional file preview (headers and body) to four seconds
and offers an explicit retry. Navigation remains usable during the read.
Downloads and writes retain their existing limits; no file or authority cache
is introduced. Production attribution requires enabling the timing switch
through the normal owner/release process and observing the resulting phases.
