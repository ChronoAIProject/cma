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

`CHRONO_USAGE_METRICS_ENABLED` defaults to `false`. Allocated reports and their
workers remain available in this mode for both owner and administrator routes.
They read allocation history without reading measured bindings, rollups, live
samples or infrastructure diagnostics. Measured quantities remain null with
existing unavailable/no-runtime states and `source_health: source_unavailable`;
this source-health value does not fail an otherwise complete allocated report.
Existing retained measurements are not exposed by collection-off reports.
Collection-enabled reports retain the same measured aggregation and source path.
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
reservation after a configuration change. On successful or failed finalization,
the same lease-fenced update reduces the reservation to actual cached row/chart
bytes. Those bytes remain charged until the expired projections have been removed.
This lets the one-minute owner/admin refresh coexist with the ten-minute cache
lifetime without charging every small terminal report its maximum work budget.
Already terminal reports from an older writer keep their conservative reservation
until normal expiry cleanup; no durable schema or reader floor changes.

Reporting keeps its four-connection pool, 250 ms checkout pressure threshold,
two-second per-command server deadline and 60-second report deadline. The
30-second pressure circuit observes individual connection holds above two seconds
and failed checkouts immediately. Successful checkouts above 250 ms require
three consecutive samples with no more than one second between them; a healthy
checkout resets that soft-pressure streak. A successful checkout exceeding the
two-second hard deadline still opens the circuit immediately. This avoids turning
one completed, slightly delayed checkout into a 30-second reporting outage.
A multi-query projection's cumulative wall time is
only diagnostic: several bounded reads must not poison the next report read.
Command monitoring remains disabled; pressure logs contain durations and fixed
categories, never command/reply bodies or customer identifiers.

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
| Report generation | 13,194 ms (60,000 ms deadline) |
| Process RSS before / after generation | 44,672 / 51,232 KiB |
| Full-cohort current-owner reauthorization | 97 ms |
| Chart and owner-search verification | 1,108 ms |
| Serialized row/chart cache | 95,944,788 bytes (256 MiB default limit) |

The source-query explain used an index and examined one document for one
requested physical read-model member. Every page and top-plus-Other sum matched
the complete cohort; allocated ledger states stayed unchanged and no allocation
events were written. This is a one-minute large-inventory bound, not a guarantee
that 20,001 dense 30-day histories fit the same budget. Broader reports can return
an explicit retryable error with a narrower-range suggestion. Reports never
truncate their matched cohort to fit a page or silently treat absent data as zero.

### Scale gate isolation and failure evidence

The Mongo contract coordinator runs the 20,001-Sandbox scale contract alone,
against a fresh bounded replica set, before starting any ordinary contract
partitions. The scale container uses the same pinned Mongo image, two CPU limit,
2 GiB memory limit, 0.25 GiB WiredTiger cache and owned disk volume as the other
fixtures. Its cleanup completes before the ordinary partitions start. Ordinary
contracts continue to run even if the scale assertion fails; the overall gate
preserves every failure. The scale contract is excluded from those partitions,
so it runs exactly once per gate, without retry-to-green.

For a controlled local reproduction, run
`bash .github/scripts/mongo-contract-tests.sh --scale` from the repository with
`CARGO_BUILD_JOBS=2` or less, holding the team's shared heavy-work lock for the
entire fixture lifecycle. Repeat that command as separate recorded experiments,
each with fresh data. Shared-runner success does not establish production capacity.

Failure output includes phase, page number (one-based within pagination), phase
and total duration, circuit-open count, last circuit reason/duration, and remaining
circuit time. Reasons distinguish failed checkout, hard checkout deadline,
sustained slow checkouts, and connection-hold deadline. Only fixed categories and
numbers are captured; names, query bodies, cursor tokens and credentials are not.
Even a circuit that opens and expires before completion fails the scale assertion.
The 60-second generation budget, 512 MiB process RSS budget, two-second ownership
reauthorization budget and complete-cohort/index/pagination assertions remain.

A controlled repeat on 2026-11-07 passed all five fresh-fixture runs with zero
circuit openings, 20,001 Sandboxes and 101 pages each. Generation took
11,412–13,294 ms, full workload 21,801–25,178 ms, and post-generation RSS
46,240–56,672 KiB. The client was macOS/ARM64 with the pinned Mongo fixture
limits above and no concurrent heavy test/build. These results do not identify
the original Linux/x64 CI pressure event or establish production capacity.

## Local validation outcome (2026-11-05)

The implementation includes the `develop` merge at `ecfb557b6` (#1316).
Workspace formatting, Clippy, locked builds, dependency audits, source-size and
structure checks, secret scanning, API/configuration documentation checks,
manifest validation and policy checks passed. The workspace run covered 7,157
tests; three OpenAPI inventory assertions were corrected and all 21 OpenAPI
checks then passed. After the merge, all 2,939 affected-crate tests passed.
All 463 disposable Mongo contracts passed, followed by all nine reporting
contracts on the merged source, including the scale fixture above.

The TypeScript SDK passed 4,109 tests with 96.59% line coverage; the Rust SDK
passed 251 tests and 37 doctests with 93.12% line coverage. SDK formatting,
lint, type/build, package/consumer and dependency checks passed. Frontend
validation accounted for all 3,279 unit cases, ten desktop browser cases and
five browser-guard cases. Temporary-directory and occupied-port fixture
failures passed isolated reruns; browser teardown passed using the repository's
supported Node 24.15.0 harness runtime. The ignored Linux writer-boundary test
executed on native ARM64 Linux and passed one actual test.

Image validation on this Apple Silicon machine had these results:

| Image | Build / execution result | HIGH/CRITICAL vulnerability scan |
| --- | --- | --- |
| Web | Passed for Linux AMD64 and ARM64 | AMD64 passed |
| API | Linux AMD64 passed | Passed |
| Image Manager | Linux AMD64 passed | Passed |
| CMAEG | Rust release compiled; required sandbox check exited 125 at `close_range` | Not reached |
| Runtime | Rust release compiled; recovery test failed at `pidfd_open` | Not reached |
| Desktop runtime | Same recovery test failed at `pidfd_open` | Not reached |

Both runtime variants passed the preceding 42 Node tests. The failing Python
case was `Proc.test_real_linux_socket_peer_and_pidfd_available`, which raised
`OSError: [Errno 38] Function not implemented`. A minimal probe in the same
builder isolated the architecture-dependent syscall behavior:

| Execution environment | `close_range` | `pidfd_open` |
| --- | --- | --- |
| Native ARM64 Linux | Success | Success |
| AMD64 user-mode emulation | `ENOSYS` (38) | `ENOSYS` (38) |

The CMAEG and runtime checks remain unchanged and fail closed. These attempts
did not produce final images for their scans or built-image contracts, so those
gates are not recorded as passing. They require a suitable Linux AMD64 runner;
on 2026-11-05 the lead approved completing these AMD64 image gates in CI because
the local machine is Apple Silicon. The CMAEG, runtime and desktop runtime
image jobs must pass in CI before merge. Every other required local gate was
run, with the results and resolved fixture failures recorded above.
No production configuration or workload was changed during verification.

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

### Administrator filter discovery

`POST /api/v1/admin/usage/metrics/options` requires `read_all_sandboxes` on every
request, including pagination and selected-value resolution. The read-only JSON
body keeps search names out of proxy request URLs. It does not create
or require a report. `kind=owner|sandbox|runtime`, optional literal name `search`
(up to 128 bytes), `owner` (Sandbox choices only), `limit` (1–50), and an opaque
principal/query-bound `cursor` return `{items:[{value,label,disambiguator?}],next}`. `selected`
resolves one value for URL restoration and cannot accompany search or cursor.
Owner names come from the retained public directory. Primary labels stay plain.
Only duplicates on the returned page receive a disambiguator: join date for
owners; owner name, then creation date for Sandboxes. Matching dates gain UTC
time at the least distinguishing precision (minutes, seconds, then available
fractional seconds). Precise owner observation times additionally require
`ReadUserDirectory` on the current request; `ReadAllSandboxes` alone retains
join dates and last-resort references. A signed continuation cannot preserve a
revoked directory capability, and clients cannot request precision in the body.
All dates are normalized to UTC. Final `(label, disambiguator)` pairs are checked
after choosing context, including owner names that resemble generated text.
Colliding rows gain further human detail before a last-resort secondary reference.
Selected labels remain plain. The shared Core `OptionContext` and
`disambiguate_options_with_context` policy also supports owner rankings.
`OptionContext::owner` defaults to dates; an authorized consumer uses
`owner_with_access` with `OwnerContextAccess::from_authorization` from the current
caller. The single-hint helpers remain compatible.
Sandbox names come
from the retained usage inventory; runtime choices include suspended runtimes.
Discovery honors the existing reporting circuit before database work, without
requiring a report. An open circuit returns 429. Response sizes, connection count
and database/request time are bounded; name scans and grouping may still visit
the candidate cohort. Results are private/no-store, and
errors never log search text or display names. The Ops web filter requires this
API extension; report endpoints remain compatible with #1398 and later.
