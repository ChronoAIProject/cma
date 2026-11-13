# #1741 exact-head CI and merge evidence

#1741 merged into `develop` at
`734bc9c2ef5d3589aa25311003c69d9b94bf20a7` on
2026-11-13 at 02:19:19 UTC. Its reviewed and published head was
`7fdfd7cdd130cd1b797b65bf83fa9860d34cbf24`; that head remained unchanged
through review, both CI attempts, and the merge.

The [independent review](https://github.com/ChronoAIProject/cma/blob/eeacaa2d3bd1f7a8bfc63d13757a2bf771127dfb/docs/notes/reviews/2026-11-13/team/pr1741-7fdfd7cd.md)
approved that exact head. Its link was added to the existing PR appendix while
preserving the original description and footer. This post-merge evidence is
committed with subsequent integration work so recording the outcome does not
change the previously reviewed source identity.

## Exact-head remote qualification

[Workflow 43338425537](https://github.com/ChronoAIProject/cma/actions/runs/43338425537)
completed successfully on attempt 2 at the reviewed head. Immediately before
merging, `gh pr checks 1741` reported 25 successful checks, zero pending checks,
and zero failed or canceled checks. The required Backend Mongo contracts and
all three Backend test shards were successful.

| Check | Passed tests | Evidence |
| --- | ---: | --- |
| Backend tests (1/3) | 3,030 | [Original successful job](https://github.com/ChronoAIProject/cma/actions/runs/43338425537/job/131607879588) |
| Backend tests (2/3) | 3,000 | [Original successful job](https://github.com/ChronoAIProject/cma/actions/runs/43338425537/job/131607879522) |
| Backend tests (3/3) | 2,969 | [Original successful job](https://github.com/ChronoAIProject/cma/actions/runs/43338425537/job/131607879568) |
| Backend Mongo contracts, attempt 2 | 1,097 | [Successful rerun](https://github.com/ChronoAIProject/cma/actions/runs/43338425537/job/131629187397) |

The ordinary workspace total is 8,999 passed tests. Mongo totals come from all
32 terminal nextest summaries: four isolated fixtures and 28 ordinary
partitions. Partition exclusions are not failures and are not summed as
additional executed tests. The failed-job rerun retained the three successful
shards; it did not execute those shards again.

Complete downloaded job-log SHA-256 values:

| Log | SHA-256 |
| --- | --- |
| Backend shard 1 | `42bc7936b9d5bc952e8b0dc9a7101c0d8055baae99c03eb2546fcac6407e2ca4` |
| Backend shard 2 | `49604f7a73a5ee5f5fb43ddaea91ebc7e002da170ed91fa5d865f5fc4dcb43e7` |
| Backend shard 3 | `6600521154dc1ef77e7de828b85a5199d4ac24d8ee1de1e623736c48a7edccb2` |
| Mongo attempt 1 | `294c735f1d159d099040d7f1dc3f1dc236ef44196f1a451b3ab69adc74af1ffb` |
| Mongo attempt 2 | `9b545eadc1991663de0bf42c2d230f91725b03a44b540e1b5173a6842630a802` |

## First failure and authorized retry

The [first Mongo job](https://github.com/ChronoAIProject/cma/actions/runs/43338425537/job/131607879711)
executed 1,097 contracts: 1,096 passed and exactly one failed,
`reconcile_pacing_tests::unchanged_ten_thousand_row_log_has_one_scan_with_pacing`.
The assertion at `reconcile_pacing_tests.rs:265` was
`after.0 * 5 <= before.0`.

| Pacing switch | Observation window | Reconciliation scans | Documents | Reply bytes | Scan time |
| --- | ---: | ---: | ---: | ---: | ---: |
| OFF | 4.286 s | 4 | 40,000 | 10,721,692 | 3,952,762 us |
| ON | 4.001 s | 1 | 10,000 | 2,680,423 | 940,738 us |

ON met the single-scan expectation. The OFF baseline completed fewer than the
five scans required by the relative assertion, matching the separately
identified throughput-dependent test defect from #1835. The complete log
contained no other failed test. The Backend aggregate failed only because its
Mongo dependency failed, and all three shards remained green.

The lead explicitly authorized one failed-job rerun only for this signature,
after the entire workflow completed. Those conditions were verified before
executing `gh run rerun 43338425537 -R ChronoAIProject/cma --failed` once.
No source, test expectation, timing bound, or configuration was changed to
obtain the successful rerun. A second failure would have required stopping
and reporting; there was no second failure.

## Merge and retained decisions

After the independent GO and final green checks were verified, the granted
conditional slot was used with:

```text
gh pr merge 1741 -R ChronoAIProject/cma --merge --admin --match-head-commit 7fdfd7cdd130cd1b797b65bf83fa9860d34cbf24
```

The command exited successfully, and a fresh PR read confirmed `MERGED` and
merge commit `734bc9c2ef5d3589aa25311003c69d9b94bf20a7`.

`CHRONO_PLATFORM_COUNTER_SHARDS_ENABLED` remains false in code and deployment
configuration. The earlier measured admission trade-off remains material:
p95 96.564 to 114.219 ms and reply bytes 82,140 to 168,090, despite settlement
p95 improving from 64.965 to 14.338 ms. CI success does not qualify activation.
Exact-key, same-fingerprint Revive receipt recovery still requires current
resource authorization and remains available during STOP; new commitments
remain refused. The exact counter census classification remains part of #1741.

See the committed primary review, fix evidence, counter measurements, and
candidate review for the implementation and local qualification boundaries.
These remote results qualify the exact published head; they do not convert
earlier local test runs into tests of a later source or claim production
release qualification.

VERDICT #1741 @ 7fdfd7cdd130cd1b797b65bf83fa9860d34cbf24: GO
