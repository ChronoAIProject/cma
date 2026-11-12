# Cookie identity and request-budget measurement

Tracks #1795 and the measurement-only phase of #1806.

Cookie authentication currently reads browser custody and subject authority in two sequential Mongo commands before online NyxID verification. The opt-in joined reader replaces those two commands with one indexed aggregate and keeps both existing document validators, post-verification token-custody fence, and session logout recheck. CHRONO_AUTH_BROWSER_SESSION_JOIN_ENABLED defaults to false in code and infra; false retains the original command sequence. Normal-path NyxID verification reuse is not implemented. The owner decision remains pending in #1542, as required by the issue.

The identity diagnostic phase isolates ambient middleware from handler reads. Fixed led/shared counters contain no Cookie, token, or subject labels. Successful ordinary Cookie requests are expected to change from three identity commands to two; /auth/session from four to three, with its settings read outside identity. No reduction in documents examined or background load is claimed merely from the round-trip reduction.

Local verification and measurements are appended after the contract run. The local task-experience route uses a no-I/O authenticated fixture; /auth/session uses the real handler without a platform-settings store. NyxID is a local HTTP contract provider. Those measurements do not establish production latency.

Production measurement plan (lead/operator executes after deployment):

1. Use the same final candidate image and diagnostic-hook version in both windows. Record image digest/source SHA, UTC and SGT window bounds, process/Pod hashes, replica count, pool sizes, post-ready age, and exact feature settings. Background fixes must already be stable. Warm up for two minutes, then collect two matched 15-minute windows with the same fixed polling schedule and workload. Use Cookie credentials only for the designated requests and retain at least 200 completed requests per route per window; extend equally if needed. First window has CHRONO_AUTH_BROWSER_SESSION_JOIN_ENABLED=false, second true. Keep all other behavioral settings, rate limits, and diagnostics controls identical. Repeat false if workload or pool contention changes materially.
2. Turn on CHRONO_MONGO_DIAGNOSTICS_ENABLED only in the approved diagnostic image. Group summaries by pool=general, worker=request, phase=identity, route (matched method/template), collection, command, metric. Include auth_browser_logins and auth_subjects. Report complete command counts, command/checkout distributions, failures, unknown outcomes, overflow/drop counts, and checkout mean from total_us/count. The aggregate is attributed to auth_browser_logins; its joined subject read is server work within that one client command. The session handler settings read remains outside identity. Exclude refresh, login, rejected/missing/expired credentials, and retries from the steady-state budget assertion, but report these cohorts separately rather than discarding them silently.
3. Correlate request hashes with the first identity-provider request to measure request-start-to-NyxID latency. Report per-route request p50/p95 for GET /api/v2/ui/task-experience and GET /auth/session, pre-NyxID p50/p95, online led/shared counts, actual Mongo checkout mean, sample count and error/status distribution. Maintain the existing trace/sampled-event limitation explicitly; summaries alone cannot produce per-request correlations. Do not derive an unbiased percentile from slow-event-biased detail samples. New duration buckets support distribution bounds, including the exact <=25 ms population boundary, rather than invented exact quantiles.
4. Pass criterion for #1795 mechanism: complete successful steady-state Cookie cohorts show identity commands/request 3 to 2 and session 4 to 3, no unexpected getMore or full scan, and every independent request continues online verification (overlapping requests may share it). Existing logout, generation, malformed-record and fallback contracts remain green. Report latency deltas with checkout changes; lower command count alone does not prove that background stalls are fixed or promise the estimated 0.2-second saving. Nonzero drops/overflow, missing correlation or mixed workload makes the affected conclusion unverified and requires a new clean window.
5. Rollback: set CHRONO_AUTH_BROWSER_SESSION_JOIN_ENABLED=false and restart through the authorized release process. Verify the baseline command sequences return. No record format or durable state migration is involved. Keep the same image for comparison. Post the operator results to #1795 and #1543; raise the separate, still-pending reuse decision in #1542 using the measured identity data.
6. For #1806, in the same stable post-background-fix window, report request_rate_limits command and checkout distributions, commands/second, and its fraction of all general-pool commands, with auth_subjects point-read command durations as the comparator. The difference is only an approximation to majority cost; operation shape, queueing, and scheduling also differ. Report bucket bounds and whether at least 95% of observations meet 25 ms. Do not add command and checkout percentiles to invent the percentile of their sum. Require correlated complete observations for the per-request added-latency decision if separate distributions do not establish it. Only the lead's production evidence can authorize the <=25 ms no-implementation versus >25 ms prepaid-lease branch. No rate-limit behavior change is included in the measurement-only work.

## Local shared-machine failure disposition

The full workspace run encountered
`layer4-runtime::ledger_import::tests::staging::imported_terminal_bytes_and_unsigned_authority_survive_with_a_durable_hold`
at `api/crates/layer4-runtime/src/ledger_import/stage_tests.rs:10`. The failure was
`mutation_ledger_migration_capacity`, with
`elapsed_nanoseconds_at_least=450517502041` against the 450-second migration budget;
the test's wall time was 592.717 seconds. The local run log is
`api/target/request-chain/nextest.log` while the verification worktree is retained.

The lead classified this as a local contention failure in an untouched crate:
the shared 18-core machine was running three to four other full suites. No bound,
assertion, test selection, or runtime code was changed to hide it, and the test was
not retried to erase the failure. The baseline develop run
[43260948284](https://github.com/ChronoAIProject/cma/actions/runs/43260948284)
at `287fee1ffb476c239563315e0a1b05c4236378af` completed successfully. This local
failure is retained as evidence rather than reported as a green workspace run.
The integration PR's dedicated-runner CI must pass; a repeat there is a blocker
owned by the runtime ledger team (#1772/#1775).

The completed run reported **8,745 passed, 2 failed, 1,288 skipped** in
1,051.999 seconds. Its second failure was
`layer4-runtime::ledger_import::tests::retirement_archives::real_input_archive_crosses_both_legacy_limits_without_deactivating_sources`
at `api/crates/layer4-runtime/src/ledger_import/retirement_stage_tests.rs:19`, also
`mutation_ledger_migration_capacity`, with
`elapsed_nanoseconds_at_least=538102869166` and test wall time 692.794 seconds.
The same untouched-crate, local-contention disposition applies; dedicated-runner
PR CI remains required for both tests.


## Focused local verification

Rust 1.96.1 / cargo-nextest 0.9.151 on a local arm64 Mac, with the runner's pinned
MongoDB `8.0.13@sha256:cf340b1e5283843c63eb12999922f20c463ae31285f746d30f05dcc21cd1d47c`:
402 focused authentication tests passed; workspace all-target/all-feature clippy
passed with warnings denied; workspace doctests passed; configuration keys,
configuration documentation, source-file limits and changed-content gitleaks passed.
The Mongo runner's 40 infrastructure tests passed. The full contract run is
recorded separately when finished.

LLVM 22.1.2 coverage instrumentation ran all 1,739 `cma` unit tests successfully
and 15 selected real-Mongo auth contracts successfully after correcting one new
fixture: a replacement string is valid for `incarnation`, so the malformed-type
fixture now uses BSON boolean false. The refusal assertion was retained.
The new rollout selector and strict config parser have 100% line coverage; the
joined Mongo reader has 52/58 lines covered (89.66%). Modified auth middleware,
Cookie verification and resolver files have 96.68%, 97.60% and 95.96% respectively.
This is scoped coverage evidence, not a claim of 80% coverage for the entire
workspace, whose other crates were not exercised in this instrumentation run.

Both readers examined two indexed source records, with two primary-key lookups
and zero collection scans. The aggregate uses the `_id_` index on the joined
collection. Every sequential successful Cookie request still initiated both
NyxID userinfo and introspection. The source validators, malformed-record refusals,
logout recheck and token-generation recheck remained active.
