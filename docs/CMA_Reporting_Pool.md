# Usage reporting Mongo pool

The reporting client serves usage report generation, cached pages, options and
background collection independently of the general Agent/lifecycle Mongo client.
When reporting pressure opens its circuit, requests can return HTTP 429
(`reporting_budget_exceeded`, “Usage reporting is busy”). Enlarging the general
client alone does not enlarge the reporting pool.

## Configuration

| Setting | Default | Accepted values | Effect |
| --- | --- | --- | --- |
| `CHRONO_USAGE_REPORTING_MONGO_MAX_POOL_SIZE` | `4` | Decimal integer `1..=16` | Maximum application connections **per Mongo server, per API process**, for the reporting client only. |

Unset uses 4. Empty, signed, whitespace-padded, nonnumeric, overflowing, zero and
out-of-range values fail startup, including when metrics collection is disabled:
allocated reports and cached reads still use this client. Leading zeroes are
accepted as decimal. Startup logs only the selected numeric `max_pool_size` in
`reporting MongoDB pool configured`; no URI is logged by this setting.

This explicit setting overrides `maxPoolSize` in the shared Mongo URI for the
reporting client. The client still forces minPoolSize=0, maxConnecting=1, and
two-second connection/server-selection timeouts. Changes take effect on process
restart. The checked-in ConfigMap retains 4; merging this feature does not raise
production concurrency. The maximum of 16 is an operational guardrail, not a
claim that the database can sustain 16 reporting connections.

Circuit semantics stay fixed: successful checkouts **over 250 ms** form a slow
streak; three consecutive slow checkouts with adjacent gaps of at most one second
open the circuit for 30 seconds. A fast checkout or longer gap resets the streak.
Checkout failure, checkout over two seconds, or observed connection hold over
two seconds still opens it immediately. Each qualifying callback logs an opening
or extension; counts are not necessarily distinct closed-to-open transitions.
Report-worker, queue, source-call and storage budgets are unchanged. The pressure
observer still bounds connection-hold tracking at 32 entries across servers;
the pool setting is not a new occupancy/hold-distribution metric.

## Evidence and initial recommendation

Recommend **8 as the first measured trial**, retaining 4 as the default and
rollback value. Four extra slots give modest burst headroom while preserving the
isolated pool and two report-worker ceiling. Do not jump to 16 or 30, and do not
relax the circuit to hide contention.

The November 10 deploy-4 evidence supports testing a change, but cannot calculate
an optimal size or establish reporting-pool saturation:

| Observation | Result |
| --- | --- |
| Old-image reference hour | 0 reporting circuit records |
| Preceding 24 hours | 7 `sustained_slow_checkout` records, 343–846 ms; none `failed=true` |
| Candidate, 46-minute partial observation | 2 such records at 449/451 ms, less than 1 ms apart; approved rollback trigger fired |
| General-client mean checkout wait, baseline → candidate | 185.615 → 171.205 ms |
| API CPU mean, baseline → candidate | 3.457 → 3.474 cores |

The reporting implementation was unchanged between those images. Near-simultaneous
callbacks may extend one pressure episode; they do not justify deduplicating the
approved rollback trigger. General-client latency summaries exclude this separate
reporting observer. No reporting checkout-rate/hold-time distribution, occupancy
series or contemporaneous server capacity limit was captured, so Little's-law
sizing and an assertion of server headroom would be unsupported. Shared database
load and runtime scheduling remain plausible causes. More sockets can worsen
server pressure; this recommendation is conditional on fresh server evidence.

The aggregate source evidence is linked from #1636 (Part of #1543). Private
release artifacts remain with the release owner; no resource identities or raw
connection strings belong in issue comments.

## Server-side connection budget

Both the general client (`store/mongo.rs`) and the **separate** runtime-routing
client (`composition/adapter.rs`) consume `CHRONO_MONGODB_URI`. If the general
increase is URI `maxPoolSize=30`, runtime routing also gets 30. Do not count it as
a clone or assume it remains at the driver's default 10.

| API configuration | General | Reporting | Runtime routing | Application cap/server/process |
| --- | ---: | ---: | ---: | ---: |
| URI maxPoolSize=30, reporting default | 30 | 4 | 30 | 64 |
| URI maxPoolSize=30, proposed trial | 30 | 8 | 30 | 68 |
| URI maxPoolSize=30, allowed reporting maximum | 30 | 16 | 30 | 76 |

For the ordinary replica-set topology, budget up to two monitor/RTT sockets per
client per server: another **6 per server per API process**. The 8-slot trial
therefore budgets **74/server/process**, or **148/server for two overlapping API
processes**. A 4→8 rollout with both URI pools already at 30 is 70+74=144/server;
148 conservatively covers two processes both at 8. Include terminating processes
until their sockets actually close; two is an overlap example, not a universal
cap on live clients.

For three discovered servers, a conservative all-server envelope is 222 API
sockets (444 with two processes). With primary-only application traffic and no
topology transition, the corresponding cluster envelope is 68 application plus
18 monitor sockets = 86 (172 with two processes). Actual connection demand is
lazy and can be lower. Server limits are per node; a cluster sum cannot prove
primary headroom. Add CMAEG, image-manager, other services, operators/exporters
and a failover/reconnect reserve to each node's budget. The saved evidence does
not contain the Atlas limit or other-client peak, so it does not prove 148 fits.
Before changing the value, the release executor records per-node connections,
limit/headroom, server CPU, cache/IO and replication lag through approved
observability. If the actual URI/topology differs, recompute before rollout.

## Fenced rollout and verification

Only the release executor changes production. Use a fully green develop batch
artifact containing this feature. Establish the new image at reporting 4 and
the independently approved general-pool value first. Hold image, general URI,
workers, resources, replicas and the Agent-log cohort fixed during the 4→8
comparison, or explicitly record that effects cannot be attributed separately.

1. Record a fresh 30-minute reporting/HTTP/CPU baseline plus the reference hour
   and preceding 24-hour circuit records. Capture the numeric pool settings and
   per-server budget above without exporting the URI. Confirm report generation
   and cached/options requests are healthy enough to compare.
2. Capture the Deployment UID, resourceVersion, generation, image, API container
   index/name, env and strategy. Apply a JSON Patch that tests these captured
   fences and changes only the explicit reporting env value to `8`. If absent,
   use a tested env-array update; do not overwrite an unexamined envFrom source
   or a duplicate env entry. Read back the value and verify Ready, old-process
   termination, and the numeric startup log for each serving process.
3. Keep the existing 15-minute health watch and full 60-minute observation after
   rollout. Compare reporting 429 count/rate, circuit records by reason/failed,
   report generation/finalization failures and completion duration, reporting
   route p50/p95 (with sample counts), general checkout wait/hold summaries, API
   CPU/throttling and per-node Mongo connections/CPU/cache/IO/replication. Also
   watch send-message latency and readiness for shared-resource regressions.
   Report missing reporting-specific distributions explicitly. A zero-event
   interval with no reporting activity is not evidence of an improvement.
4. Preserve the existing release rollback gates: any failed checkout circuit
   event, report-generation/finalization failure, readiness/crash-loop regression,
   or circuit rate above reference/sustained back-to-back. With a zero reference
   hour, one isolated failed=false record is report-only; two in an hour trigger
   rollback. Do not deduplicate callbacks to avoid that trigger. Roll back for
   database connection-budget exhaustion or a sustained server latency/error,
   CPU/IO/replication regression against the approved release baseline as well.
5. Rollback is a fresh, fenced env-only change back to `4` on the same qualified
   image; verify all 8-slot processes are gone and reporting recovers. Preserve
   report data, general URI and other rollout settings. A larger value is not an
   automatic fallback. The release owner posts aggregate evidence on #1636 and
   closes it only after production verification.

Target the Wed November 11, 2026 evening train; the final fallback train is Fri
November 13 morning, with deployment and verification complete by 15:00 SGT.
Code must merge at least three hours before its train. If the experiment does
not improve reporting or headroom is unproven, retain 4 and report the unresolved
pressure; configurability alone is not evidence that the owner's error is fixed.
