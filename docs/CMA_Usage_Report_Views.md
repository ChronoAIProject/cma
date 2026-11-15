# Retained Sandbox usage views

The latest-view API returns a currently authorized committed report while a replacement runs in the existing reporting workers. It does not reuse identity or ownership authority across requests. Developer-11 owns server/OpenAPI/SDK delivery; the web integration is tracked separately by developer-07.

## Public contract

`GET /api/v1/admin/usage/metrics/view` and the owner route `/api/v1/usage/metrics/view` accept `window=24h|7d|30d|custom` (default `24h`), `from` and `to` only for custom ranges, optional `step_seconds`, strict JSON `filters` encoded as one query parameter, and optional `refresh=true`. TypeScript uses `SandboxMetrics.latest`; Rust uses `UsageMetrics::latest`.

A 200 response contains `schema_version: 1`, `window`, `requested_range`, the existing `report` descriptor and summary, Unix-second `generated_at`, `stale`, and `refresh: {state, report_id, retry_after_seconds}`. Refresh state is `idle`, `pending`, or `deferred`. The report's actual `query`, `effective_range`, `as_of` and `expires_at` remain unchanged. An old snapshot is never labelled with the newly requested time range. Sorting and paging remain presentation requests against the returned exact report ID.

Render a ready summary immediately, including when stale or refresh is deferred. Load the first page and any selected charts from that exact report ID; keep their existing current authority checks. A refresh does not blank or disable the retained report. The web must keep at most one view/status request in flight per page, cancel obsolete work on scope/identity changes, and honor Retry-After. An SDK instance alone is not a principal or sign-in-epoch cache key.

202 means that no safe retained snapshot exists and a job has actually been admitted. It includes the pending report identity, null `generated_at`, false `stale`, pending refresh state, and Retry-After 2. It never fabricates an empty or partially authorized summary. Finite retention permits formerly computed combinations to expire or be invalidated by authority changes; it cannot promise perpetual coverage for arbitrary idle filters.

400 rejects malformed input; 401/403 preserve authentication/capability refusal; typed 404 `not_found` is a selected-resource/authority failure; 429 is admission/read capacity exhaustion; 503 is unavailable dependency or integrity evidence. A disabled retained-view writer returns 501 `report_view_disabled`. Only that exact 501 or an untyped router 404 with no problem extensions on an older replica permits compatibility fallback. No fallback follows typed resource denial, malformed success or generic 503. All responses are private, no-store.

Descriptor/status GET and HEAD requests use the independent bounded reporting read lane from #1948: two process slots, no queue, 1 MiB buffered response limit, five seconds including body collection, Retry-After 2. Page, series and owner projections retain their existing contracts and isolated reporting Mongo pool. Valid grouped charts can exceed 1 MiB.

## Storage and supported writers

A stable key includes principal/admin scope, canonical membership filters, rolling window or exact custom range, resolution and aggregation/schema version. Refresh creates a new UUID namespace with `retained_view_v1`, separate `view_pending`/`view_running` states, a fixed retention deadline before any child writes, and existing job-generation/lease fencing. Predecessor workers cannot claim those new states. No new index is introduced.

Publication atomically marks the new namespace Ready and replaces its latest pointer under the active job, key revision and worker generation. Failed or capacity-limited replacements preserve the prior pointer and snapshot. Old and replacement bytes remain charged to existing tenant/global budgets. Completion records exact row/chart counts; current live ownership checks and namespace/expiry checks reject partial cleanup. Every request reauthorizes, including explicit report IDs and later pages/groups. The supported-writer boundary excludes arbitrary administrator mutation of private cache collections, but malformed/conflicting evidence encountered by readers is refused. Direct mutations of authoritative source ownership remain in scope of live validation.

After replacement, the detached old namespace retains a 30-second read grace. Cleanup conditionally retires the scanned generation and state only while its current deadline has expired on Mongo's clock, before deleting children. A delayed scan cannot retire a reclaimed ordinary job or a renewed deadline. Chunk and completion writes also check lease and logical expiry on Mongo's clock, with the byte-reservation expression combined with the lease expression. Reservations are released only after child deletion is acknowledged. Expiry and retirement are checked again before returning child results; responses are buffered. Retained parent jobs have no active TTL while reserved; only acknowledged child deletion releases the reservation and starts a 60-second parent tombstone TTL. Automatic TTL deletion is eventual storage cleanup and is not budget-release evidence.

The retention class is bounded to one day, default freshness five minutes, eight keys per principal and 64 total, with at most 64 retained-job metadata records per principal and 512 total (including tombstones). Existing report endpoints keep their ten-minute class. The warmer defaults to 24 hours. On a compatible image, each warm selection may explicitly choose `window:"24h"`, `window:"7d"`, or `window:"30d"`; the subject/admin/Sandbox cohort in `VIEW_SCOPES` remains range-independent. An older image rejects the new field. Each range uses a separate retained key and reservation, so enabling additional ranges needs measured generation cost and separate rollout approval. Scope principals must be the exact authenticated subject; `operator` in tests is not a production alias.

## Activation packet — HOLD pending exact candidate gates and owner approval

Each activation below is a separate owner decision conveyed through the lead. Merge does not activate it. Bind the packet to the tested source SHA, approved image digest, complete reader/writer deployment receipt and current effective non-secret configuration before execution. No production operation has been run as part of this implementation.

1. **Retained-view writer.** `CHRONO_USAGE_REPORT_VIEWS_ENABLED` defaults false. Enable only after every serving/worker replica supports retained states and reads. Use the required `CHRONO_USAGE_REPORT_VIEW_SCOPES` cohort: start with the owner's own test Sandbox IDs containing the test Agents, selected under explicit approved principal/admin scope. Unlisted callers or selections receive `report_view_disabled`. This creates retained report namespaces and latest-pointer records in existing report collections; it creates no new indexes. Expand to the owner's default Everyone view only after separate approval and a successful canary. No old worker may remain active at writer activation.
2. **Background warmer.** `CHRONO_USAGE_REPORT_WARMER_ENABLED` defaults false and requires retained views plus an explicit bounded operator scheduling scope. Start with the same owner's test Sandbox scope. Each cycle uses a fresh holder token and the existing shared worker/admission budgets; it stores no provider credentials and confers no end-user capability. This is operator-authorized materialization, not impersonation of a currently authorized user. Consumers always prove live rights. Expanding the scheduling scope to unfiltered Everyone is a separate owner-approved step.

For each canary record full client-send through summary, first page/labels and final render; collect p50/p95/p99, N, maxima, all failures/cancellations, fresh/stale/pending/deferred counts, generation/finalization stages and source reads, report queue/reserved bytes, reporting pool occupancy, and each process read-lane rejection. Compare Agent transcript/message/create latency and errors in the same image/configuration window. A small canary proves operability and authority, not large-report speedup.

Abort immediately on foreign data, stale ownership attribution, partial or malformed success, reader-floor mismatch, budget/fence violations or loss of the last-good snapshot during a failed refresh. Abort the performance expansion if full rendered reporting remains above two seconds or transcript/Agent error and latency regressions appear. A cold first-ever filter remains separately reported, never relabelled as a warm success.

Rollback closes the warmer first and then retained admission using their separate switches. Preserve the compatible reader/writer binary until admitted work drains or is reconciled and retained namespaces expire/are safely cleaned. Keep existing report IDs and data through their approved lifetimes; do not point an older reader/writer at active retained jobs. Use the lead's reviewed image rollback route only after the compatibility floor is satisfied. Deletion and reservation-release failures remain visible and charged; do not force-release bytes or edit pointers manually.

## Incremental allocation source buckets

`CHRONO_USAGE_ALLOCATION_ROLLUPS_ENABLED` is default off. On a compatible image,
it enables maintenance of disposable `sandbox_usage_allocation_buckets_v1`
source projections and their use during report generation. The authoritative
allocation ledger, public report schema, authorization, report worker leases,
and reporting pool remain in use. The separate live and backfill lanes publish
under Mongo server-time leases; bucket data and their resume checkpoint share
the publication transaction.

Every daily total and hourly projection is calculated directly with the existing
raw allocation projector. A report reuses a cached interval only when its
source provenance and complete selected range match. At most two partial dates
use prepared inputs that preserve the original selected-day fold, including
event order, gaps and projected tails. Current-day reuse requires the captured
source head and query coverage; complete historical coverage can outlive a
source revision. Hourly values are never subtracted or regrouped into a
different floating-point calculation. A rolling 30-day request can cross
31 UTC dates: the bound is interior daily totals and at most two boundary
computations, not a claim of at most 30 physical Mongo documents. Missing coverage,
changed source evidence or incomplete backfill never silently becomes zero.
When only the source head advances, a bounded revision query reads appended
events in the same report snapshot and replays them with the prepared prefix.
It never combines independently rounded totals. Missing prefix coverage or
unsupported overlap falls back to the original raw query.
Supported ledger rebaselining can change complete lifecycle coverage to
incomplete at a newer monotonic source revision. That transition discards the
prepared coverage for reading and uses the raw calculation until maintenance
rebuilds it under the existing publication fence. Owner/identity changes,
same-revision drift and backwards source metadata still refuse.
Malformed cache evidence is refused. Current report ownership and membership
are still verified independently of these source projections.

Initial source backfill is distinct from cold immutable-report generation.
Activation measurements must record backfill progress and duration, raw/source-cache
coverage, report generation/finalization, and committed first-page/owner-label
latency for 24h, 7d and 30d. A new report ID alone does not establish a cold
source cache. Fixed worker diagnostics include range_seconds, step_seconds and
allocation_rollups alongside the report digest and outcome; they contain no
caller subject. The new source path does not relax the report read-lane timeout
or byte cap. The 7d web preset should omit explicit resolution and use the
server's stable 600-second default; a 300-second step may exceed 2016 buckets
on a minute-aligned rolling range and remains correctly rejected.

Deploy compatible readers/maintenance workers with the new switch false first.
Approve activation only after the new collection/index readiness, load budget,
backfill and exactness evidence are reviewed. Preserve existing view and warm
cohorts, the metrics collection flag and unrelated settings. Disable the new
switch to stop source-bucket scheduling and use; retain compatible workers
until in-flight fenced work has completed or expired. Buckets are disposable
with a 32-day TTL, and disabling the switch does not rewrite or delete raw
ledger entries, report pointers, report children or reservations. Stop the
retained warmer first if rolling back its independently approved writer scope.

The new namespace has an explicit optional Agent Manager grant and archive
census entry. Existing credentials can start with the switch off; activation
requires its full confined grant. Keep a compatible release for archive tooling
after granting the namespace or producing new census manifests: old isolated
credentials and old archive verifiers do not understand the expanded census.
Disabling the switch is a supported rollback of cache use and maintenance,
not permission to roll back the binary below that compatibility floor.

Observe an admitted report by its immutable report ID. A failed retained job
is retired immediately and normally returns `410 report_expired`; terminal
410/404 must not trigger automatic new admission. Repeated latest-view requests
can otherwise admit replacements after failure. Bound transient retries and
observation time in the UI. `Retry-After` is a poll delay, not a completion ETA.
Ready metadata may attach only to a matching report ID. A differently ranged
preview is never completion of the requested range.
