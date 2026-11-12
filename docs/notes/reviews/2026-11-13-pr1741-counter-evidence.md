# PR #1741 counter and census evidence

This records the local accounting comparison for #1496 and the counter reader
extension owned by #1741. It is not an activation approval or the final PR
verdict. The original-head review and correctness corrections are recorded in
[primary review](2026-11-13-pr1741-primary-review.md) and
[correction evidence](2026-11-13-pr1741-fix-evidence.md).

## Measured trade-off and activation decision

The first instrumented comparison measured concurrent admission p95 increasing
from **96.564 ms to 114.219 ms**, while admission reply payload approximately
doubled, from **82,140 to 168,090 bytes** across 30 admissions. Concurrent
settlement p95 fell from **64.965 ms to 14.338 ms**. These results support removing
the shared accounting write from settlement, but do not establish an overall
production improvement. The lead confirmed that
`CHRONO_PLATFORM_COUNTER_SHARDS_ENABLED` stays **default OFF** in code and the
ConfigMap. Activation is a later decision using representative measurements.

The comparison uses internal accounting transactions, not the full provider
request path. Each phase performs 30 operations: two waves spanning the 15
partitions used for new owners. Sequential phases await each operation;
concurrent phases issue one operation per partition in each wave. Conversion
and fixture setup happen before measurement. The host is shared, so elapsed
times include host scheduling and local queue time. No latency pass threshold
or production performance claim is attached to this sample.

The checked-in harness is
`api/crates/cma/src/store/budget/counter_measurement.rs`, exercised by
`distributed_counters_measure_sequential_and_concurrent_accounting`. It uses
MongoDB command and connection-pool events plus the database profiler on the
disposable, digest-pinned MongoDB 8.0.13 fixture described in the correction
evidence. Request/reply bytes are serialized BSON command/reply payload bytes,
not transport-frame or TLS bytes. Commands include transaction protocol work;
connection checkouts equal command counts in this sample. Read/write counts
identify commands that touch root/counter accounting, not returned row counts.
The batched admission query reads the root and all sixteen counters once.

| Layout | Schedule / phase | Commands / checkouts | Request bytes | Reply bytes | Root reads / writes | Counter reads / writes | p50 µs | p95 µs | Elapsed µs |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Original | Sequential reserve | 150 | 71,970 | 82,140 | 30 / 30 | 0 / 0 | 6,344 | 7,551 | 193,659 |
| Original | Sequential settle | 90 | 46,770 | 30,300 | 30 / 30 | 0 / 0 | 4,211 | 5,436 | 131,966 |
| Original | Concurrent reserve | 150 | 71,970 | 82,140 | 30 / 30 | 0 / 0 | 51,468 | 96,564 | 195,200 |
| Original | Concurrent settle | 90 | 46,770 | 30,300 | 30 / 30 | 0 / 0 | 32,591 | 64,965 | 128,540 |
| Distributed | Sequential reserve | 150 | 83,964 | 168,090 | 30 / 0 | 30 / 30 | 7,477 | 8,772 | 233,248 |
| Distributed | Sequential settle | 90 | 40,956 | 24,462 | 0 / 0 | 30 / 30 | 3,947 | 5,130 | 121,739 |
| Distributed | Concurrent reserve | 150 | 83,964 | 168,090 | 30 / 0 | 30 / 30 | 61,057 | 114,219 | 234,285 |
| Distributed | Concurrent settle | 90 | 40,956 | 24,202 | 0 / 0 | 30 / 30 | 10,475 | 14,338 | 25,313 |

The profiler reported **zero collection scans** in every phase. Distributed
settlement performs no root reads or writes. Admission still serializes on the
existing settings fence and local admission queue; its sum reads more bytes.
The comparison does not include conversion cost, collisions between owners in
the same partition, multiple API replicas, provider latency or production load.

## Accounting guarantees under review

The layout has exactly sixteen `platform-counter:<start-seconds>:<partition>`
keys and a strict root `counter_layout: 1` marker. Conversion atomically moves
existing model balances to partition zero. New reservations bind to partitions
1–15 using a stable owner hash and optional `period_shard`; legacy holds retain
their original period and settle against partition zero without backfill.

Admissions take the exact sum under the existing actual settings write fence.
Cloud corrections and late usage that increase committed accounting also take
that fence. Ordinary settlement and release can only preserve or reduce the
total, so they update only their bound counter. Closure persistence validates
the root, policy and all contributing counter revisions with actual writes.
Turning the process switch OFF stops new conversions and restores local
scheduling; compatible readers still account for already converted periods.

The regression tests cover independent accounting without root writes,
competing admissions across store instances, legacy conversion and OFF recovery,
stale STOP after counter release, late usage against admission, concurrent
conversion, aborted conversion, missing counters, and real command/turn/request
bindings. A further review found that converting before a refused first
admission could use an uncommitted layout as evidence for STOP persistence.
Admission is now evaluated before conversion, preserving committed evidence
on refusal. The dedicated regression requires persisted STOP and zero counters.

## Census scope and integration

The lead assigned the narrowly scoped counter reader extension to #1741;
#1754 owns the existing period/observation/STOP classification. The extension
uses #1754's distinction between known key families and record ownership.
Only command/turn/request families may be selected by incidental Agent fields.
Canonical counter keys are known independent accounting and are excluded from
every individual Sandbox archive. The decimal seconds must fit a nonnegative
BSON signed integer and partitions are exactly 0–15. Unknown, malformed,
overflowing, signed, padded and trailing-content keys still refuse capture.

Tests exercise all partitions and timestamp boundaries, incidental ownership
exclusion, refusal without releasing the archive hold, and byte-exact restoration
of reservation `period_start`/`period_shard` fields. The root layout marker does
not change the period's independent ownership. The catalogue and its hash do
not change. Integration with merged #1754 and a complete converted-layout
fixture remain prerequisites to the final exact-head verdict.

## Verification checkpoints

The first counter selection passed 72 of 73 tests. Its measurement test passed
and produced the table above, but that whole run is not reported as green. The
failed real-command fixture lowered only the turn allowance, which legitimately
allows an existing valued hold to run. The corrected fixture also lowers the
owner ceiling below outstanding liability; the zero-provider-send assertion
is unchanged. The next Mongo selection passed 74/74. Its subsequent clippy
failure was a production import used only by tests; the import was moved to
the test module. The inclusive selection then passed **222/222**, including the first-refusal
regression, replay, current authorization, counter/census checks and B1–B7.
Clippy for `cma`, `layer2-core` and `layer1-public`, with all targets, all
features, locked dependencies and warnings denied, passed after that run.
Formatting, whitespace, source-size, deployment-key and configuration-doc
checks also passed. Final integrated gates remain separate checks.

The root-write baseline disables conversion while keeping the new assertion on
the original accounting path. It fails at “routine reservation and settlement
must not write the shared period”: the root revision and balances change.
Restoring conversion passes that assertion and preserves the exact totals.
This is a targeted behavior comparison, not a claim that the baseline ran the
whole original PR binary.

| Supporting local log | SHA-256 |
| --- | --- |
| Root-write baseline (`b6-root-before.log`) | `c1528fdb60c727944f3c217d57dfdf091a12c93e4878d1ead383c018305f5914` |
| First comparison and 72/73 run (`counter-after.log`) | `93c2784483964a34721930d702813a289d4636f94c3e7542c261fdc365021429` |
| Corrected 74/74 run (`counter-after-corrected.log`) | `687518c846e5c05c23edb9e5ac0eaa8650caacda13998bb9202735224667076e` |

The measurements, outcome descriptions and test source are committed evidence;
raw fixture logs are temporary local supporting material. No production data,
identities, credentials or financial figures are included here.

The counter first-refusal and census baseline selection failed **0/2**, both at
the intended behavior boundary. The earlier draft returned the expected budget
refusal but did not persist STOP; the old census refused capture of canonical
counter keys. Production sources were restored automatically before the next
selection. Baseline log SHA-256:
`c5c032edee813dfd02d2b75d9435ca46bee8f9d51f250d3f90270a38b5824d35`.

The 222/222 run repeated the comparison on the shared host. Command counts,
BSON bytes and root/counter reads/writes matched the first sample; every phase
again had zero collection scans. Concurrent reserve p95 was **160.222 ms →
223.984 ms**, and concurrent settlement p95 was **146.055 ms → 58.458 ms**.
These timings demonstrate host variability and repeat the admission cost versus
settlement benefit; they do not replace representative activation measurements.
The inclusive log SHA-256 is
`cb5f469a1626e54e58142e88e606c8892fe4530aca45d1c851d65b53f72561a2`.

Clippy log SHA-256: `1d87d253f11cded46ee6e54ff06c35ce6038d2dc284e8e24de431857a5f69572`.
