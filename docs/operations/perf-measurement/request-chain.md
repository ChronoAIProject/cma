# Cookie identity and request-budget measurement

Tracks #1795 and the measurement-only phase of #1806.

Cookie authentication currently reads browser custody and subject authority in two sequential Mongo commands before online NyxID verification. The opt-in joined reader replaces those two commands with one indexed aggregate and keeps both existing document validators, post-verification token-custody fence, and session logout recheck. CHRONO_AUTH_BROWSER_SESSION_JOIN_ENABLED defaults to false in code and infra; false retains the original command sequence. Normal-path NyxID verification reuse is not implemented. The owner decision remains pending in #1542, as required by the issue.

The identity diagnostic phase isolates ambient middleware from handler reads. Fixed led/shared counters contain no Cookie, token, or subject labels. Successful ordinary Cookie requests are expected to change from three identity commands to two; /auth/session from four to three, with its settings read outside identity. No reduction in documents examined or background load is claimed merely from the round-trip reduction.

Local verification and measurements appear below. The local task-experience route uses a no-I/O authenticated fixture; /auth/session uses the real handler without a platform-settings store. NyxID is a local HTTP contract provider. Those measurements do not establish production latency.

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
The Mongo runner's 40 infrastructure tests passed. Before the develop merge,
the full real-Mongo contract runner passed all **1,019 tests**: four isolated
checks followed by 26 ordinary partitions (one with 40 tests and 25 with 39).
The runner exited successfully after its owned fixture cleanup. This run
covered the #1795 implementation at `04e099e547b0bfb62ba661e8998246e00fe92b1d`;
the additional duration-bucket tests and diagnostics overhead contract passed
separately as described below.

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

## Local before/after measurements

The ordinary, non-coverage run used 50 sequential requests for every
route/switch/wait combination (400 requests total). The test is
`store::auth::browser_session::tests::requests::cookie_request_chain_commands_and_local_wait_measurements`.
The 25 ms case injects a deterministic sleep at each observed auth-store
boundary to simulate waiting for a connection. This is not actual driver pool
contention, and it is excluded from the command and actual-checkout timings.
Both paths used the same local fixture and NyxID HTTP contract server. The
benchmark mounts authentication middleware only; it excludes rate-limit and
activity-recording middleware, so it measures the identity chain rather than
the full production request stack.

| Route | Injected wait per boundary | Commands/request, off → on | Request p50 ms, off → on | Request p95 ms, off → on | Before NyxID p50 ms, off → on | Before NyxID p95 ms, off → on |
| --- | ---: | --- | --- | --- | --- | --- |
| GET /api/v2/ui/task-experience | 0 ms | 3 → 2 | 2.510 → 2.248 | 3.510 → 2.909 | 1.500 → 1.093 | 2.376 → 1.827 |
| GET /auth/session | 0 ms | 4 → 3 | 3.254 → 2.818 | 4.024 → 3.528 | 1.453 → 1.046 | 2.191 → 1.716 |
| GET /api/v2/ui/task-experience | 25 ms | 3 → 2 | 86.158 → 58.163 | 88.469 → 60.260 | 57.306 → 29.447 | 59.676 → 30.479 |
| GET /auth/session | 25 ms | 4 → 3 | 115.713 → 87.228 | 119.116 → 90.078 | 57.836 → 29.676 | 59.981 → 30.657 |

| Route | Injected wait | Mongo round trips per 50 requests, off → on | Reply BSON bytes per 50 requests, off → on | Total command duration µs, off → on | Actual checkout mean µs, off → on |
| --- | ---: | --- | --- | --- | --- |
| GET /api/v2/ui/task-experience | 0 ms | 150 → 100 | 94,550 → 82,950 | 73,478 → 58,757 | 34.067 → 35.130 |
| GET /auth/session | 0 ms | 200 → 150 | 114,300 → 102,700 | 91,891 → 76,664 | 35.620 → 35.800 |
| GET /api/v2/ui/task-experience | 25 ms | 150 → 100 | 94,550 → 82,950 | 130,035 → 95,150 | 64.200 → 59.930 |
| GET /auth/session | 25 ms | 200 → 150 | 114,300 → 102,700 | 180,534 → 147,389 | 62.235 → 64.460 |

Reply sizes are decoded BSON reply sizes measured by the test observer, not
compressed wire traffic or server disk bytes. The index explain reported
baseline documents/keys examined 2/2 and joined outer plus subject documents/keys
examined 1/1 + 1/1, both with zero collection scans. No getMore was issued.
The sample is small and runs on a shared machine; the deterministic command
reduction is established locally, while production latency remains unverified.

## Request-budget instrumentation

The #1806 measurement change adds eleven non-cumulative duration buckets to each
existing bounded diagnostic group, with inclusive bounds of 1, 5, 10, 25, 50,
100, 250, 500, 1,000 and 5,000 ms, followed by an overflow-duration bucket. It
adds 88 bytes per group and no label dimensions. Diagnostics remain gated by
the existing default-off `CHRONO_MONGO_DIAGNOSTICS_ENABLED` switch. This work
adds no lease switch or rate-limit behavior change; #1806 stays open pending
the production measurement and decision in step 6 above.

Focused boundary, overflow, saturation, drain and privacy tests passed. The
summary module has 212/214 covered lines (99.07%) in the scoped coverage run.
The real-Mongo diagnostics privacy/overhead contract passed unchanged. Its
observed off/on mean command times were 1,444/1,483 µs for 25 returned documents,
4,179/5,647 µs for 200, and 155,035/158,284 µs for 10,001; the corresponding
p50 values were 1,430/1,449, 4,152/4,474 and 154,022/156,423 µs.
These local results validate the contract, not a production overhead guarantee.

## Integration handoff

The work lives on `tl/cma-perf-read/request-chain`. Preserve the #1795 commits
`6bc505f50`, `f6fd885ca` and `04e099e54`. Merge commit `76a81c163` incorporates
develop `ee0c9d7e2`, including #1783 (`c39c0f797`) and #1773 readiness
instrumentation. Its only manual conflict resolution keeps the union of both
branches' Mongo contract selections. The #1806 bucket change is isolated in
`4a953da21` and touches only the summary module and diagnostics documentation.

After the merge, 559 focused tests passed across auth, configuration, Mongo
diagnostics, readiness and rate-limit modules, including the Control tier.
Workspace all-target/all-feature clippy passed with warnings denied. All 26
selected real-Mongo contracts passed: 16 auth, eight Control/Input policy,
one shared request-budget and one diagnostics contract. Their disposable
fixture cleanup succeeded. The unchanged Mongo runner infrastructure suite
passed all 40 tests after the selection conflict was resolved.
The final `cargo fmt --all --check`, configuration/infra agreement, operations
documentation, file-size and changed-content secret-scanning checks passed.

The merged-head Mongo run repeated the same 50-request measurement cases and
retained the exact command sequences and reply BSON totals above. Its latency
and actual checkout values were:

| Route | Injected wait | Request p50 ms, off → on | Request p95 ms, off → on | Actual checkout mean µs, off → on |
| --- | ---: | --- | --- | --- |
| GET /api/v2/ui/task-experience | 0 ms | 3.922 → 3.018 | 4.305 → 3.448 | 51.147 → 52.160 |
| GET /auth/session | 0 ms | 4.973 → 4.071 | 5.565 → 4.616 | 53.115 → 54.833 |
| GET /api/v2/ui/task-experience | 25 ms | 86.684 → 58.551 | 91.268 → 61.198 | 72.707 → 68.860 |
| GET /auth/session | 25 ms | 115.881 → 87.467 | 119.322 → 91.077 | 74.635 → 122.587 |

These repeated results are retained separately from the full-run numbers;
neither run establishes production latency or resolves #1806's production gate.

The integration owner should use `Closes #1795` and `Part of #1806` in the PR
handoff. #1806 must remain open for the production evidence and decision.
Independent review and dedicated-runner PR CI remain integration gates; the
local verification here does not substitute for them. The lead/operator owns
production measurement and activation; no production access was used.

Original local logs were under `api/target/request-chain/`. Their result
excerpts and measurements are retained in this committed document before
worktree build-artifact cleanup. SHA-256 values identify the completed logs:

| Log | SHA-256 |
| --- | --- |
| mongo.log | `2c307239bb79e3f0a293bcf9f282edb419cb5900510ffc9d4c6f7494d266433d` |
| nextest.log | `b770733368eb8ae7daab02b233875665ae88e8d13cb54095e31154e06d260bed` |
| doctest.log | `92a0a764327f6ebef6ee7b8586d5b8161ff3c8d3fb95c8da8798d215e14c8b60` |
| bucket-mongo.log | `4664fcf01943789e6d8689797f8cd6eed7aec074e4198c512f5e7c2344dfc3e4` |
| merged-focused.log | `17e01cdaae12cce8533ddc06f6d67ca4a899cddf8f365825fb349e63ced229ae` |
| merged-clippy.log | `90ed45f540cc99b4a971a424bec675fee994c1b9cb2806fb55838d80842a86f3` |
| merged-mongo.log | `fd22d3434676567666c6adc0c81af58cfa3863242070d07acaa4b9896aec8474` |
