# Buffered Agent history: local paired results

Issue #1793. Measured source `41f0d1d3792bb82b0f602e3dd6afe65d21125376`, including #1831 head
`8a22a4cbc1fcc461eb7bd0058b5cdbb5cd31bcfa` and develop `4cb65509b` through merge
commits. The earlier measure-first gate and pre-implementation baseline remain in
[history-auth-baseline.md](history-auth-baseline.md). Activation and production
measurement instructions are in [history-auth.md](history-auth.md).

## Method and limits

The committed `resource_access_tests::history_measurement` harness exercised the
actual Agent v2 routes, services and Mongo stores against disposable MongoDB
8.0.13, image digest
`sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c`.
Rust 1.96.1, native aarch64 macOS debug build, concurrency one request within each
fixture, on a shared build machine. A normal build slot was used. There was no
production access, synthetic production traffic or simulated latency in this run.

After warming all four modes, twenty complete traversals rotated mode order.
Each 260-row traversal had three 100-row pages; each 1,000-row traversal had ten.
Both histories included a command-origin turn and steering/message binding.
Active and idle heads were measured separately, twenty requests per mode each.
Authentication was a trusted fixture context; identity and rate-limit middleware
were outside this measurement. Timings include the command observer and exact
raw-response comparison. Reported BSON bytes are returned documents, not wire
traffic or Mongo disk bytes. Command phase groups below are inferred from command
shape, not production spans.

The two measurement tests passed: 68.720 s for 1,000 rows and 14.846 s for 260 rows.
The same run also passed the legacy metadata refusal and both segmented range
contracts. Its separate tail-session assertion failed because a preceding
intentional visitor failure left asynchronous driver cursor cleanup in the
recorder; that assertion is corrected and verified separately. This does not
change the page workload or the measurements below.

Raw run log SHA-256:
`3e0abd1ef12b75b31449fc20434d6ba35ff51da3a36f1104b592b6db569bb2cd`.
The original log was `api/target/history-auth/four-mode.log`; the committed tables
retain the results after temporary build artifacts are removed.

## Commands and authorization snapshots

`S` denotes `CHRONO_ACCESS_SNAPSHOT_SESSION_ENABLED`; `P` denotes
`CHRONO_AGENT_EVENT_PAGE_SINGLE_FENCE_ENABLED`. Counts below are exact per request
and identical at both fixture sizes. Physical checkout counts equalled command
counts. Every repeated page also retained identical command groups and returned
BSON document/byte counts within its mode.

| Request | S off, P off | S on, P off | S off, P on | S on, P on | Snapshots, P off → on |
| --- | ---: | ---: | ---: | ---: | ---: |
| First snapshot page | 30 | 22 | 14 | 11 | 8 → 3 |
| Later snapshot page | 28 | 20 | 12 | 9 | 8 → 3 |
| Active head | 26 | 19 | 13 | 10 | 7 → 3 |
| Idle head | 12 | 8 | 9 | 6 | 4 → 3 |

Holding S off, P reduces page commands by **53.3%** on the first page and **57.1%**
on later pages. Holding S on, P reduces them by **50.0%** and **55.0%**.
All page comparisons exceed the 40% acceptance threshold. The enabled personal
path has exactly three snapshots for snapshot pages, active heads and idle heads.

The original baseline gate remains satisfied: with S/P off, authorization accounts
for 24/30 = 80.0% of first-page commands and 24/28 = 85.7% of later-page commands.

| Derived phase / command group | First page, P off → on | Later page, P off → on | Active head, P off → on | Idle head, P off → on |
| --- | ---: | ---: | ---: | ---: |
| Authorization: agents aggregate | 8 → 3 | 8 → 3 | 7 → 3 | 4 → 3 |
| Authorization: resource_shares find | 8 → 3 | 8 → 3 | 7 → 3 | 4 → 3 |
| Authorization: abortTransaction, S off | 8 → 3 | 8 → 3 | 7 → 3 | 4 → 3 |
| Authorization: abortTransaction, S on | 0 → 0 | 0 → 0 | 0 → 0 | 0 → 0 |
| Log: agents find | 2 → 1 | 2 → 1 | 2 → 1 | 0 → 0 |
| Log: agent_log find | 1 → 1 | 1 → 1 | 1 → 1 | 0 → 0 |
| Log: agent_log getMore | 1 → 1 | 1 → 1 | 1 → 1 | 0 → 0 |
| Binding: agent_commands find | 2 → 2 | 0 → 0 | 1 → 1 | 0 → 0 |

The tail proof saves one redundant Agent query on this unprepared-log workload.
It preserves the existing legacy/segmented proof rather than issuing a second
query. The complete history scan remains: each snapshot page and active head
performs exactly one `PublicWindowPrefix` scan, covering 260 documents / 65,788
BSON bytes or 1,000 documents / 252,268 BSON bytes. Idle heads perform no scan.
This result does not claim #1648 scan removal.

## Latency

Each cell is **p50 / nearest-rank p95 / maximum**, in milliseconds. Sample counts
are per mode. Snapshot results pool that fixture's page positions; active and
idle heads remain separate. These local samples cannot establish production
p95/p99 objectives or isolate production pool contention.

| Rows / request | n per mode | S off, P off | S on, P off | S off, P on | S on, P on |
| --- | ---: | ---: | ---: | ---: | ---: |
| 260 / snapshot | 60 | 49.597 / 73.824 / 103.291 | 43.768 / 70.448 / 78.691 | 30.982 / 52.230 / 108.104 | 27.874 / 39.744 / 47.593 |
| 260 / active head | 20 | 45.397 / 50.097 / 73.706 | 38.694 / 42.801 / 43.044 | 27.950 / 31.306 / 33.653 | 25.263 / 26.791 / 27.015 |
| 260 / idle head | 20 | 15.246 / 17.077 / 17.366 | 12.126 / 12.793 / 13.215 | 11.568 / 13.041 / 13.942 | 9.110 / 9.742 / 9.985 |
| 1,000 / snapshot | 200 | 85.441 / 110.635 / 202.829 | 77.862 / 85.832 / 152.380 | 63.551 / 72.227 / 135.904 | 60.551 / 65.937 / 125.421 |
| 1,000 / active head | 20 | 80.657 / 83.820 / 85.419 | 73.479 / 77.477 / 78.623 | 61.801 / 65.048 / 66.289 | 59.158 / 61.088 / 61.258 |
| 1,000 / idle head | 20 | 16.149 / 18.020 / 19.183 | 12.691 / 14.331 / 14.817 | 12.406 / 14.078 / 15.489 | 9.822 / 11.197 / 11.741 |

With S off, P lowered the snapshot median by 37.5% for 260 rows and 25.6% for
1,000 rows. The 260-row snapshot maximum increased from 103.291 to 108.104 ms
while its median and p95 improved; shared-machine tails are not a fixed latency
guarantee. Production attribution remains pending the same-image, natural-traffic
windows in the measurement protocol, including comparison with #1538's p95
≤500 ms / p99 ≤2 s objectives without claiming those objectives are met.

## Returned documents and bytes

Each cell is **documents / BSON bytes**, per request. The S switch does not change
these values, so the table isolates P. Scan bytes above remain unchanged; reduced
totals here come from fewer authorization/hierarchy documents.

| Rows / request | P off | P on |
| --- | ---: | ---: |
| 260 / first page | 312 / 114,233 | 281 / 86,514 |
| 260 / later page | 310 / 110,868 | 279 / 83,149 |
| 260 / active head | 305 / 107,432 | 280 / 84,892 |
| 260 / idle head | 24 / 20,616 | 18 / 15,462 |
| 1,000 / first page | 1,052 / 300,713 | 1,021 / 272,994 |
| 1,000 / later page | 1,050 / 297,348 | 1,019 / 269,629 |
| 1,000 / active head | 1,045 / 293,912 | 1,020 / 271,372 |
| 1,000 / idle head | 24 / 20,616 | 18 / 15,462 |

## Release handoff

Keep P default-off. The three-snapshot and command-reduction gates, exact raw
response parity across all four modes, and unchanged scan-work assertions passed
on both fixtures. The focused suite passed all 192 tests, including purge and
consistent owner-transfer refusals after binding, revocation and expiry, final
fencing after dependency failure/timeout, frozen cursors, retained soft-delete
parity, byte bounds and memory integrity. Later validation results are recorded
in the final verification handoff; these measurements alone are not a complete
release gate.

Independent authorization/revocation review, dedicated PR CI, staging activation
and rollback rehearsal, and lead-owned production evidence remain release gates.
Do not close #1793's production-verification requirement on local timing alone.
