# Measured Sandbox resource reports

Issue #1332 adds observational CPU, working-set memory and persistent-home
filesystem usage. The allocation ledger remains the source of allocated
resource-time. These measurements do not change funding, prices, settlement,
refunds or invoice arithmetic. `billing` explicitly reports `basis: allocated`,
`status: not_integrated`, and `charges: null`.

A Sandbox is the measurement boundary. Selecting an Agent resolves its owned
Sandbox and sets `shared_sandbox_usage: true`. Multiple Agents and multiple home
mount paths do not multiply resource usage.

## Sources and measured evidence

The source is existing VictoriaMetrics tenant 0 in `monitoring`. Runtime
metrics use `job="kubelet", metrics_path="/metrics/resource", container="runtime"`:

- `container_cpu_usage_seconds_total` supplies cumulative actual CPU seconds.
- `container_memory_working_set_bytes` supplies working set, not RSS or limits.
- `kubelet_volume_stats_used_bytes` and `kubelet_volume_stats_capacity_bytes`,
  with `job="kubelet", metrics_path="/metrics"`, supply filesystem usage and
  filesystem capacity. Requested provisioned storage remains separate.

Each request uses backend-generated, validated exact physical names, cluster
and namespace. The API accepts neither PromQL nor source selectors. cAdvisor,
pause containers, duplicate jobs and ambiguous HA replicas are never added to
these values. Unsupported cohorts remain unavailable rather than falling back
silently to a different definition.

A read-only check at **2026-11-04 17:37:22 UTC** sampled an existing cold-created
CMA Pod in `chrono-pod`, cluster label `chronoai-prod-asia-southeast1-gke`.
The four raw exports each contained one series and ten samples in five minutes.
The raw samples matched fixed instant-query values when compared at the source
timestamps:

| Resource | Actual source time (Unix seconds) | Value |
| --- | --- | --- |
| CPU counter | 1793813837.977 | 1363.588001 CPU seconds |
| Memory working set | 1793813837.977 | 31,449,088 bytes |
| Home filesystem used | 1793813822.568 | 1,163,264 bytes |
| Home filesystem capacity | 1793813822.568 | 67,049,664,512 bytes |

The final raw two-minute CPU window gave approximately 0.003737125 cores.
An adjacent initial instant query returned an older RAM value before a later
read matched the exported sample. This is evidence for preserving timestamps
and the finalization delay, not a claim that adjacent queries are atomic.
Sanitized raw source fixtures are committed under
`api/crates/layer3-adapter/src/sandbox_metrics/fixtures/`. These checks establish
real source values. They do not establish the Agent Manager service account's
reachability, uninterrupted historical coverage or historical owner attribution.

## Identity and retention

The observer proves the persisted native-home receipt against Claim, native
Sandbox and PVC UIDs, configured deployment/namespace and current runtime
assignment. Pod controller ownership, Pod UID, runtime container ID, restart
count and generation identify a compute incarnation. A failed compute proof
can leave an independently verified retained home valid.

Compute and home intervals are retained independently. Unknown historical
assignment starts at the first verified observation; today's annotations do not
backfill yesterday's owner. A closed interval ends at its last verified
observation. Missing or replaced physical identity never creates an owner
measurement. Closed bindings remain for at least 90 days; active bindings have
no TTL. Existing allocation/billing records are read-only inputs.

Raw measurements remain in VictoriaMetrics with its existing 30-day retention.
Mongo stores versioned per-minute aggregates in hourly chunks within each
Sandbox/day: integrals, coverage and envelopes, not raw 30-second samples.
Measured chunks expire 30 days after the chunk end; history outside source retention remains unavailable. A separate replayable daily allocation
projection stores the ledger watermark and compact expectation intervals.
Finalized, complete allocation days can be reused after later checkpoints;
projected tails must be recomputed. Neither read model is invoice evidence.

Observation, recent rollups, retained-history replay and admin diagnostics have
independent shared leases. Native proofs run with concurrency eight; each cycle
retains its paging position. Replay resumes hour partitions after a restart,
starting no earlier than retained proof, and catches up without redownloading
closed history for each report. Source failures retain the partition for retry.
A delayed proof closes attribution at its last verified instant; recovery never
fills an unknown identity gap.

The admin infrastructure footer contains current, separately timestamped
CPU/RAM/PVC values for `unassigned_pool` and `unattributed`. It proves the Pod's
native controller UID, deployment and PVC UID before opening a short-lived
observation interval, and excludes current AM-mapped physical identities.
`state` and `unproven_count` expose missing proof, source gaps and staleness.
It is independent of the report's historical customer period and never grants
owner access or contributes to customer/billed totals. No physical-name regex
selects the cohort; cold and resumed native Pods use the same proof.

## Time and aggregation

Ranges are half-open UTC Unix seconds, `[from,to)`, at most 30 days. A report
exposes the requested range and `effective_range`, which ends at
`min(requested_to, measurement_cutoff)`. The cutoff is 120 seconds before the
snapshot. An effective range with no finalized duration has no finalized
samples. Live latest readings carry their own source timestamps.

CPU display rates use the supported counter history within two minutes. Period
CPU core-hours use valid actual counter deltas divided by 3600. A valid interval
has ordered samples in one incarnation, no reset and no gap longer than 90
seconds. A reset starts a new baseline. Partial boundary intervals are clipped
only between supported samples; no activity is invented across a reset or
assignment boundary.

RAM and disk GiB-hours integrate observed gauge bytes over time, divided by
`2^30 * 3600`. Each hold ends at the next observation or 90 seconds, whichever
comes first. Stale intervals never extend an integral through missing time.
These are observational estimates, independent of allocated resource-hours.
Filesystem capacity follows its own scrape times and 90-second freshness
bound. Capacity changes and expiry split the percentage numerator/denominator
intervals without discarding independently valid used-byte observations.

Charts use epoch-aligned minute buckets through 24 hours, five-minute buckets
through seven days, and hourly buckets through 30 days. Coarser steps must be
allowed day-dividing multiples. Every boundary bucket counts toward the 2,016
point limit: an unaligned exact seven-day request defaults to ten minutes.
Too-fine requests return 400 with the allowed minimum.

Within each Sandbox, means are weighted by covered elapsed time. Fleet and
owner means sum those Sandbox means. Missing time is not a zero sample. Coarser
min/max values retain observed envelopes; a fleet envelope sums component
bounds and is not a claim that their peaks occurred simultaneously. Percentage
numerators and denominators use exactly the same covered usage/capacity subset.
Unknown or zero capacity is excluded from that ratio, and values above 100%
remain visible. Total allocated capacity and covered allocated capacity are
both returned. These capacity blocks describe allocations at snapshot time;
covered capacity includes resources with a fresh latest measurement. Historical
coverage and ratio denominators are returned separately.

Every canonical chart bucket is present for a nonempty selection. Missing
buckets have null measurements, an explicit reason, expected duration, unknown
mapping duration and excluded count. Top groups plus Other cover the entire
selected cohort. An empty selection has zero counts and empty charts. A source
failure must never masquerade as an empty selection or healthy zero use.

Authoritative no-runtime state makes CPU/RAM `not_running`. An unmounted home
can retain `last_observed` bytes/time while `latest_bytes` is null and state is
`unmounted` or `stale`. Reporting never mounts a paused home. Disk remains
allocated while paused; measured disk billing would require a separate approved
policy for unmounted intervals.

## Public API and authority

The route prefix is `/api/v1/usage/metrics` for owners and
`/api/v1/admin/usage/metrics` for administrators:

| Request | Result |
| --- | --- |
| `POST /reports` | Typed range/filter input; 200 ready or 202 pending with polling URL and Retry-After |
| `GET /reports/{report}` | Frozen query, cutoff, expiry, source health and full-cohort summary |
| `GET /reports/{report}/sandboxes` | Globally sorted keyset page; default 50, maximum 200 |
| `GET /reports/{report}/series` | CPU, memory or disk; total, Sandbox or admin-only owner grouping |
| `GET /reports/{report}/sandboxes/{sandbox}` | Authorized cached member detail |
| Admin-only `GET /reports/{report}/owners` | Literal owner-subject prefix search and keyset pages over the full report cohort |

Owner routes require `UseOwnSandboxes` or `LaunchTriggers`, force the
caller principal, and reject owner filters. Administrator routes require
`ReadAllSandboxes` on every request. Shared Agent access does not grant usage
access. Cross-owner resources and reports return 404; missing capability
returns 403. Every cached response rechecks current ownership. An Agent must
still resolve to the report's frozen Sandbox. Snapshot IDs and cursors grant
no authority by themselves.

Cursors authenticate the principal, admin/owner scope, report, filter hash,
sort/order and last key. Changing one of these dimensions returns 400.
Owner-search cursors additionally bind their search prefix and have a distinct
cursor domain. Missing values sort last in either direction; Sandbox ID breaks
ties. Expired reports return 410 and require refresh. Budget pressure returns
429 with Retry-After; unavailable computation returns 503. Every response uses
`Cache-Control: private, no-store` and the existing public problem envelope.

The ten allowlisted table sorts are name, owner, status, last activity, mean
CPU, mean memory, latest disk, and allocated CPU/memory/disk resource-hours.
Owner sorting/grouping/search are administrator-only. Display names are
optional and never discovered for owner reports. Filters describe metadata at
snapshot time, not lifecycle state at every historical bucket.

Large integer bytes/seconds are decimal strings above JavaScript's safe integer
range. The TypeScript SDK preserves those strings; Rust uses `Quantity` for
observational quantities and lossless deserialization for integer counters.
Presentation conversion is explicit. No monetary estimate is produced.

## Deployment prerequisites and rollout

`CHRONO_USAGE_METRICS_ENABLED` defaults to `false`.
`CHRONO_USAGE_METRICS_CLUSTER` must match the observed monitoring label, not a
node or zone name. `CHRONO_USAGE_METRICS_URL`, when supplied, is the approved
internal tenant-0 base ending in `/select/0/prometheus/`. Redirects, credentials
in URLs, fragments, query strings and alternate tenant paths are refused.
Without that setting the adapter supports only the fixed Kubernetes API-proxy
GET path for the named vmselect service in `monitoring`.

Before enabling, the lead must verify the Agent Manager service identity can
reach that source. Human kubectl permissions are insufficient evidence. Do not
grant broad `services/proxy` permissions. If a service-specific API proxy grant
cannot be demonstrated to constrain the exact service, use an authenticated
internal read gateway and network policy that admits only Agent Manager.
Monitoring infrastructure is managed outside this repository; applying this
PR does not mutate the monitoring stack or grant broad proxy access.

The runtime-cluster identity needs namespace-confined get/list/watch of Pods,
PVCs, native Sandboxes and SandboxClaims in `chrono-pod`; diagnostic pool proof
also needs read access to SandboxWarmPools. An operator-managed Role must use
only these resources and read verbs. It needs no Secret reads, Pod exec, mount,
create, update, patch or delete permissions for reporting. Bind it to the
actual remote identity selected by the existing native connection config.
Apply runtime-cluster permissions in that cluster, not the control-plane
cluster. Existing provisioning permissions remain independently governed.

New AM-owned collections are `sandbox_metric_bindings`,
`sandbox_metric_rollups`, `sandbox_usage_rollups`, `sandbox_usage_reports`,
`sandbox_usage_report_rows`, `sandbox_usage_report_charts` and
`sandbox_usage_report_control`. Collection/schema initialization is additive;
readers explicitly reject unsupported document versions. Register the same
collection grants before starting an isolated deployment. Initialize and
verify indexes before enabling background work. No migration rewrites
`sandbox_usage`, `sandbox_usage_events` or funding records.

Each report persists its admitted byte reservation. Admission counts those
reservations before adding new work, including when quotas are not exact
multiples of a report budget. A worker cannot grow a cache beyond its original
reservation after a configuration change. Reservations are released only after
the expired row and chart projections have been removed.

Roll out the reader and collections with collection disabled, prove source and
identity permissions, then enable collection for the supported deployment.
Observe mapping lag, coverage, VM latency/errors, reporting checkout latency,
queue and cache bytes, alongside existing Agent Manager latency. Disable the
collection flag to stop new observation while retaining existing bindings,
read models and allocated history. Do not roll back to incompatible readers,
recreate runtime Pods, replace homes or change the deployed runtime floor to
roll back reporting.

## Local scale evidence

The disposable Mongo 8 replica-set fixture contains **20,001 Sandboxes** with
one finalized minute per resource. It exercises the actual report generator,
all server pages, global measured sorting, full-cohort owner top 10 plus Other,
owner search, ownership reauthorization and an indexed source query. On this
shared Apple Silicon development machine it measured:

| Measurement | Result |
| --- | --- |
| Report generation | 11,414 ms (60,000 ms deadline) |
| Process RSS before / after generation | 43,456 / 56,480 KiB |
| Full-cohort current-owner reauthorization | 171 ms |
| Chart and owner-search verification | 1,036 ms |
| Serialized row/chart cache | 95,944,788 bytes (256 MiB default limit) |

The source-query explain used an index and examined one document for one
requested physical read-model member. Every page and top-plus-Other sum matched
the complete cohort; allocated ledger states stayed unchanged and no allocation
events were written. This is a one-minute large-inventory bound, not a guarantee
that 20,001 dense 30-day histories fit the same budget. Broader reports can return
an explicit retryable error with a narrower-range suggestion. Reports never
truncate their matched cohort to fit a page or silently treat absent data as zero.

## Production verification

After deployment, the lead uses existing workloads and read-only Kubernetes
API GETs plus authorized CMA API requests:

1. Verify the actual AM identity's source reachability and namespace-limited
   observation permissions before enabling collection.
2. Compare active cold and assigned warm Sandboxes' raw CPU/RAM/PVC samples,
   timestamps, denominators and report values. Check that unassigned/unresolved
   infrastructure appears only in the administrator diagnostic footer.
3. Confirm paused-home last-observed/stale behavior without mounting it, and
   verify that new identities begin at proven intervals rather than fake
   historical backfill.
4. Check owner/admin separation, Agent selection, cached-page revocation,
   global sorting, owner search, top-plus-Other totals and report expiry.
5. Compare allocated period quantities with the existing ledger API and confirm
   that billing arithmetic and funding configuration did not change.
6. Check AM checkout/conversation latency and reporting budgets before widening
   collection. Record source/identity gaps explicitly. No Agent test message,
   direct production database connection, Pod exec or runtime restart is needed.
