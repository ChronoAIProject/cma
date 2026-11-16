# Allocation interval query ordering

Issue #2140 covers repeated raw allocation-event scans during on-demand reports.
Each cache miss reads at most 200 Sandboxes for one UTC-day range. Sorting that
query in Mongo by `(sandbox_id, revision)` can select the revision index and scan
the selected Sandboxes' retained history before filtering the requested interval.

The full interval reader now lets Mongo choose an overlap access path without a
server sort. After the entire bounded cursor succeeds and every record passes
validation, it orders each Sandbox's vector by its integer revision. Projection
receives the same order as before. Incremental `allocation_events_after` queries
keep the existing revision-bound seek and database sort.

The snapshot session, strict overlap predicates, schema/query-field validation,
200-Sandbox/day bound, 2,000-event batch size,
100,000-event cap, and 64 MiB byte cap remain in effect. A failed cursor or corrupt
record returns an error rather than a partially usable day. Cache ownership,
watermark, and exact-range checks are unchanged.

The reader consumes raw BSON and charges the received document length against
the byte cap. It decodes each event once while preserving strict nested-record
validation, exact typed envelope comparisons, and refusal of duplicate authority
fields. This removes repeated document reconstruction and serialization.

## Completely cold multi-day ranges

With allocation buckets disabled, a multi-day report can prepare its daily
legacy projections from a single full-range read per small group. A same-snapshot
existence read first checks the selected page's at-most-31 UTC dates. Any cache
record, including a partial or invalid one, selects the existing daily path.

For an entirely empty cache, the new internal reader accepts at most eight
Sandboxes and thirty days. It keeps the exact overlap predicate, snapshot,
decoder and integer revision order. Its 100,000-event and 64 MiB limits apply to
the entire group's interval, across every cursor batch. Each original daily
slice filters exact overlaps before calling the unchanged projector. Crossing
events participate in every day they overlap; an out-of-day foreign owner cannot
change a different day's completeness. Daily merge order is unchanged.

Preparation retains at most 31 dates by 200 identities, 100,000 expectations and
64 MiB of serialized cache updates. Raw histories are released after each
sequential eight-member group. A raw/output cap discards the whole prepared map
and selects the original daily path once for that inventory page. Database,
decoder, circuit and deadline errors remain errors. Cache writes start only
after every group has validated and passed the output caps, use the original
owner/range/watermark/TTL format, and contain at most 200 updates per operation.
Each group read, cache precheck and cache-write operation retains the ten-second
wall guard; all preparation remains inside the sixty-second report deadline.
There is no parallel work on the snapshot or new persistent cache schema.

Aggregate application phase clocks report cache precheck, raw cursor/validation,
projection and cache writes without customer identifiers or a BSON reply
callback. This successor is unmeasured; the previous optimized candidate
completed six cases but met two seconds only for personal/admin 24h. Cold-range
reuse still transfers every required event and does not establish a two-second
full-report result.

## Index dependency

The existing awaited Sandbox store startup path now ensures this additive index:

```javascript
{ sandbox_id: 1, ended_at: 1, started_at: 1, revision: 1 }
```

The catalog entry is `usage-event-interval`; new startup creation names it
`sandbox_usage_events_interval`. A complete compatible index under another name
is accepted because the reader uses no mandatory hint. Startup performs bounded
inventory and postcondition readback and refuses incompatible or incomplete
indexes. Existing revision indexes remain available for incremental reads.
See [the migration record](../../CMA_Usage_Interval_Index.md) for startup limits,
compatibility, and rollback behavior. Production index presence remains unknown
until an authorized metadata readback is available.

## Allocation deadlines and circuit protection

Allocation cache reads, raw interval reads, and cache updates use a five-second
Mongo operation limit. A ten-second wall timeout covers the complete allocation
call, including cursor consumption, validation, projection, and cache writes.
Standalone raw and delta readers have the same finite wall guard. A timeout
returns an unavailable result and never returns a partially fetched day.

The reporting pool's connection-hold threshold is six seconds, allowing a
bounded allocation command to finish without opening the circuit at the old
two-second boundary. This hold policy applies to the whole reporting pool.
Connection establishment, server selection, checkout-stall protection, and
unrelated operations retain two seconds. The complete report generation budget
remains sixty seconds; the report lease remains seventy seconds. These are
safety ceilings, not performance targets or evidence that a report meets two
seconds. The native matrix must calibrate completion and timing on retained
history before release.

## Saved local comparison

The synthetic Mongo 8.0.13 fixture contained 360,003 events across 250 Sandboxes
and 60 days, selecting 200 Sandboxes per page. Five warm observations include
complete cursor consumption and local ordering for the candidate. They exclude
native decoding/projection, report generation, and browser time.

| Query | Baseline p50 / p95 ms | Interval index + local order p50 / p95 ms |
|---|---:|---:|
| Recent day, one Sandbox | 2.016 / 2.539 | 0.966 / 1.402 |
| Recent day, 200 Sandboxes | 175.084 / 207.151 | 83.354 / 96.697 |
| Older day, 200 Sandboxes | 170.665 / 188.262 | 144.587 / 176.886 |

The recent page returned 4,800 events: documents examined fell from 288,003 to
4,800, with 5,000 candidate keys examined. The older-day candidate still examined
144,201 keys. One paired 30-day sweep fell from 5,622.444 to 3,350.014 ms; all 30
ordered event-ID hashes matched. This is a query subtotal, not report latency.
The baseline pinned the revision index seen in the initial unhinted reproduction;
the candidate used no hint.

Controls showed that removing the sort without the interval index still scanned
288,003 documents. Adding the index while retaining the sort could still choose
the old revision index for older ranges. The unchanged unhinted delta query read
10 keys/documents for 10 events. Neither a missing production index nor a specific
production timeout plan is established by this local evidence.

Evidence remains private. Final local comparison SHA256:
`a41a750d778ab27f19ca97cd346ba89dfdb1918b71684138c3136ad66d4efd00`.
Machine-readable analyst report SHA256:
`3d1ff701d29bf4e48d3fc17c49a9c263c5225ba2fee9c03f7d47509f1ab03334`.

## Native regressions

The `allocation_cold_range` filter covers exact daily parity, crossing intervals,
owner isolation, prepared-output caps, cold/cache selection, late corruption and
snapshot/circuit authority. Its native contracts use the same two explicit
database overrides as the interval contracts and remain in ordinary Mongo CI.

The `allocation_interval` test filter covers revision ordering, exact projected
allocations, large integer revisions, incomplete histories, corrupted fields,
and byte/event bounds. Its two disposable-Mongo tests also cover multiple cursor
batches, strict overlap boundaries, snapshot consistency, operation without the
interval index, and the unchanged incremental path.

Run only against a disposable replica set:

```sh
cargo test --locked -p cma --bin cma allocation_interval -- --include-ignored --test-threads=1
```

`CHRONO_TEST_MONGODB_URI` must point to that disposable replica set. The integration
tests accept explicit `CHRONO_TEST_ALLOCATION_DB_01` and
`CHRONO_TEST_ALLOCATION_DB_02` disposable database names for the scoped local
lane. Ordinary CI uses unique disposable databases when these overrides are
absent. Scoped local invocations run serially and create/drop only their assigned
databases. The `interval_index` filter covers
startup inventory, creation, idempotence, and refusal of drift; the
`usage_reports::budget::tests` filter covers existing pressure behavior.

The ignored `native_usage_hotfix_full_report_six_case_matrix` fixture is built
unchanged into two exact source revisions. It records six N=1 cold allocation
cache cases, personal/admin over 24h/7d/30d, on 421 Sandboxes and 3,758,688
retained events. It runs actual store startup and authenticated in-process
routes through generation/finalization and page reads, samples native RSS, and
retains failed cases. It excludes external authentication, network, polling and
browser time. No percentiles or production recovery follow from this fixture.
These commands must never use a production connection.
