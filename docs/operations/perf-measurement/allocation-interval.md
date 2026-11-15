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
200-Sandbox/day bound, two-second database deadline, 2,000-event batch size,
100,000-event cap, and 64 MiB byte cap remain in effect. A failed cursor or corrupt
record returns an error rather than a partially usable day. Cache ownership,
watermark, and exact-range checks are unchanged.

## Index dependency

The measured improvement requires this existing inspection candidate shape:

```javascript
{ sandbox_id: 1, ended_at: 1, started_at: 1, revision: 1 }
```

The catalog entry is `usage-event-interval`, index name
`sandbox_usage_events_interval_candidate`. It is currently inspection-only;
`step: None` does not establish that it exists in any deployment or authorize its
creation. An equivalent usable index can have a different name because the query
has no mandatory hint. Keep the existing revision index for incremental reads.

This change neither builds an index at startup nor creates a migration authority.
Before expecting a production speedup, an authorized operator must inspect the
serving database's `sandbox_usage_events` index definitions and options. The
minimal read is a bounded `listIndexes`; no customer document or credential needs
to be returned. If no suitable index exists, its build requires a separately
reviewed activation plan and explicit approval. Without it, the reader remains
correct but may retain the expensive scan.

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
tests create and drop their own randomly named databases. This command is not a
production diagnostic and must never use a production connection.
