# Sandbox hierarchy batching

The default-off `CHRONO_SANDBOX_BATCHED_READS_ENABLED` flag reduces serial
child queries in Sandbox detail reads. It requires
`CHRONO_SANDBOX_SNAPSHOT_READS_ENABLED=true`; configuration rejects any other
combination with batching enabled. Both flags accept only exact `true`/`false`.
Workspace detail reads retain their transaction path. There is no schema,
writer, HTTP, permission or response-shape change.

## Read contract

The root is read first. One aggregation then streams tagged Workspace,
Worktree and Agent rows through `$unionWith`, with the existing indexed
`sandbox_id` predicate in each branch. The root and aggregation share the
snapshot session and its pinned `atClusterTime`, including cursor continuation.
No concurrent operation uses that ClientSession. Individual records use the
unchanged decoders; per-type ID sorting restores the original `_id` order.
Hierarchy validation and the fresh final revocation fence remain unchanged.

The aggregation has a ten-second server execution limit, shared across its
branches and cursor batches. This is a server execution bound, not an end-to-end
HTTP deadline. No joined array, blocking server sort or result truncation is
introduced. A hierarchy may exceed the BSON per-document limit because records
are returned individually. If an original document is within 128 bytes of the
16 MiB limit, a small sentinel selects the original three child queries at the
same snapshot time. Partial buffered rows are discarded. Driver errors, corrupt
records and cursor errors never select this fallback or
return a partial hierarchy. Existing transient-error retry semantics still apply.

The full decoded hierarchy is retained in memory to validate and construct the
existing response. Streaming removes a server-side joined-document limit; it
does not make response memory constant. Large hierarchies require additional
`getMore` checkouts, and aggregation overhead can outweigh round-trip savings
on a low-latency local database. Measure before activation.

## Verification and measurement

Real-Mongo contracts cover transaction/snapshot/batch result parity, empty roots
and child collections, ordering, corruption, indexed branches, cursor batches,
large individual records, same-snapshot fallback, concurrent move/delete before
the aggregate and before `getMore`, a fresh authorization-policy change, server
failures and cancellation. Query explain checks all three winning index plans
and rejects collection scans.

The reproducible ignored measurements use a pinned Mongo replica set and three
synthetic populations: 4 Workspaces/16 Worktrees/16 Agents; 16/256/256; and
128/4096/4096. The largest is a retained-history stress case: 16 Workspaces
and at most 16 Agents per Workspace remain live; excess Workspaces/Worktrees
are removed and excess Agents stopped, within the existing admission limits.
Each also has 1,025 unrelated Agents in the same collections.
These are production-like volume scenarios, not measured production size
distributions. Twenty warmed authorized reads per mode rotate mode order,
include Mongo-backed colors/names and all five detail timers, and assert exact
response parity. Report nearest-rank p50/p95, max, phase times, CMAP checkouts
and cursor continuations. HTTP credential extraction, serialization, network
transport and production contention are outside this local measurement.

Run the `resource_access_tests::sandbox_snapshot::` ignored tests with
`CHRONO_TEST_MONGODB_URI` and `CHRONO_TEST_MONGODB_FAILPOINT_URI` pointing only at
a disposable failpoint-enabled replica set, one test at a time. The tests
write synthetic fixture data and are unsuitable for production databases.

## Local comparison

Pinned Mongo 8.0.13, two CPU/two GiB disposable replica set, Rust 1.96.1
unoptimized test profile, twenty warmed samples/mode with rotated order.

| Population (Workspaces / Worktrees / Agents) | Transaction p50 / p95 (ms) | Snapshot p50 / p95 (ms) | Batched p50 / p95 (ms) | Checkouts/read: transaction → snapshot → batch |
| --- | ---: | ---: | ---: | ---: |
| 4 / 16 / 16 | 14.678 / 22.670 | 13.344 / 15.266 | 12.708 / 16.042 | 13 → 12 → 10 |
| 16 / 256 / 256 | 83.547 / 86.698 | 81.530 / 86.784 | 75.901 / 86.007 | 15 → 14 → 11 |
| 128 / 4096 / 4096 | 1035.464 / 1519.219 | 992.305 / 1798.156 | 1064.249 / 1337.719 | 16 → 15 → 11 |

Batching saves 2/3/4 connection checkouts per read versus snapshot-only.
The largest retained-history median regresses about 7.3% despite fewer checkouts;
its p95 improves in this run. Small and full-capacity medians improve about
4.8% and 6.9%. Shared-host noise and unoptimized decoding limit these local
results. Keep the flag off until release-profile/load and production phase
evidence support activation. These figures do not explain production's route
median. The hierarchy still dominates locally; a separate linear per-Workspace
count projection experiment would help distinguish projection CPU from database
round trips without touching authorization or the final fence.

## Activation and rollback

Enable the independent detail-timing flag first to obtain phase baselines.
Compare a release-approved snapshot-only canary with batching enabled using
aggregate phase distributions, error rates and database load. The existing
production route p50 alone does not attribute latency to the hierarchy reader.
Keep batching off if the hierarchy phase regresses or the query plan is not
indexed. Disable only batching to restore the snapshot-only reader; disable
batching and snapshot reads together to restore the transaction reader.
Neither rollback requires data conversion. Production changes remain subject
to the release owner and exact-artifact verification.
