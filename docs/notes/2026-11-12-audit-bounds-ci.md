# Near-capacity audit CI timeout

Part of #1350. PR #1779 remains unmerged pending its required checks and review.

Job 131174448380 failed at `audit_bounds.rs:92`: the outer 30-second
`tokio::time::timeout` returned `Elapsed(())`. It did not fail a source-row,
query, reply-size, write-transaction bound, or domain-result assertion. No
first-case aggregate was printed, so the 10,000-row case had not completed.
CI resource records show two Mongo fixtures and overlapping test processes
when that timeout expired. The same test passed on develop `affdcb04f` in
19.730 seconds. The evidence supports a load-sensitive wall-clock timeout.

The audit entry point calls claim, collect, and promote directly. Its fixture
inserts the Agent, source rows, segment metadata, and v2 root directly; neither
calls ordinary adoption preflight. All ten audit implementation and fixture
files have identical Git blob IDs at `affdcb04f` and `5667cf706`. The #1779
process switch is not read on this path.

The unchanged test was reproduced at exact head
`5667cf706881a79e579bc4bdcd58d43c9355b271` using the canonical Mongo 8.0.13
image, 0.25 GiB cache, 2 GiB memory, and two CPUs, under the release lock with
`lockf -k`. It passed both cases in 21.098 seconds total:

| Case | Source rows | Queries | Write transactions | Maximum rows/write transaction | Maximum write bytes | Audit time |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 10,000 entries | 10,000 | 184 | 392 | 129 | 149,819 | 11.285 s |
| 63 MiB messages | 67 | 14 | 5 | 129 | 124,511 | 2.084 s |

The harness now runs this contract in an exclusive `--audit-bounds` phase,
after report scale and owner allocation and before ordinary partitions.
The coordinator waits for its owned container and volume cleanup before
starting another fixture. Ordinary selection excludes exactly this test;
complete selection still includes it exactly once. The 30-second timeout,
10,000-entry and 63 MiB fixtures, query and BSON limits, write bounds, and
production Rust code are unchanged.

Harness regression coverage includes source and archive modes, exact and
nonempty selection, missing/duplicate/non-ignored audit discovery, audit
failure propagation, uncertain cleanup stopping subsequent work, and SIGINT
and SIGTERM cleanup during the audit phase. The selection-proof tool also
accounts for the newly isolated test when comparing older harnesses.

Local evidence is retained at
`notes/2026-11-12-pr1779-audit-bounds/`, including the unmodified-head
reproduction, source-identity proof, harness checks, repeated canonical Mongo
runs, and actual nextest selection proof. The diagnostics overhead failure was initially handled separately in #1782
and is integrated below.

Validation of the harness delta: 35/35 harness tests passed. Three consecutive
canonical `--audit-bounds` runs passed both cases (23.282, 17.349, and 18.396
seconds total), with unchanged query/write counts and all original assertions.
Together with the unmodified-head run, that is four successful real-Mongo
invocations covering eight near-capacity cases. The 6,451-file size gate and
shell/Python syntax checks passed. No Rust source changed after the prior
whole-CMA strict Clippy, 14 real-Mongo tests, and 20 unit tests were qualified.

At `9089393f1`, the actual nextest selection proof passed: the complete 1,005-test Mongo
corpus is unchanged, with 3 singleton exclusive phases and 1,002 ordinary
tests, no omissions or overlaps. Previously ordinary selection contained
1,003 tests; the sole removal is the audit now selected by its exclusive phase.

The subsequent diagnostics isolation commit from #1782 is integrated as a
separate commit. The combined coordinator now runs four exclusive phases
(report scale, owner allocation, audit bounds, diagnostics) before 1,001
ordinary tests. Comparing its exact predicates to the saved actual nextest
inventory proves that all 1,005 tests remain selected exactly once. The audit
30-second timeout and diagnostics `on_mean < 2 * off_mean + 5,000 us` guard
are both unchanged. The production and test Rust sources are unchanged.

Combined validation passed: 40/40 runner regressions, ShellCheck with sourced
files, Bash/Python syntax, the 6,451-file size check, and unchanged guarded-test
blob identities. Every changed file is below 500 lines. CI must still qualify
the unchanged diagnostics ceiling and the complete Mongo corpus on this head.
