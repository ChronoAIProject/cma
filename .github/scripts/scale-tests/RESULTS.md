# cma-scale local verification results

Overall: **NO-GO / incomplete**. No shared-Kubernetes acceptance scenario has
passed. This record distinguishes harness validation from real application
acceptance. Tracking issue: #1957. Base integration source:
`d6fc953a6cb43f8d9c90337b4e4ce17ecb291fbc`.

Current ownership dependency: `aadd212c2810fb12d443998e3cf75bab2fd670cf`.
Its first combined OFF/ON process test **FAILS at enabled N=2**, returning
HTTP 503 instead of 202. N=1 OFF/ON and N=2 OFF completed first; N=3 was not
reached. This supersedes any suggestion that the historical process pass
qualifies the ownership candidate. Developer-15 owns the investigation.

## Executed local gates

| Gate | Result | Evidence and limits |
|---|---|---|
| New JavaScript harness | PASS | 26 tests; 99.65% lines, 97.84% branches, 98.54% functions for lib/scenarios; ESLint and syntax checks pass. |
| N=1-only selection | PASS, implementation only | Three windows, no fault/scale/cross-replica calls; all 43 rows retained; missing/failed/unrendered cases and cleanup tested. Live driver still absent. |
| Disposable Mongo runner | PASS | 14 current unit tests; Python trace reports 94% executable-line coverage. Cleanup, wrong ownership, timeout, endpoint and evidence-overwrite failure paths included. |
| Pinned Rust formatting and touched-crate lint | PASS | Rust 1.96.1 fmt; cmaeg all-target/all-feature Clippy with warnings denied. |
| Original EG executable control | PASS | Same test-binary hash as replica case; signed ingress and complete streamed response; disposable Mongo cleanup confirmed. |
| Imported delivery oracle | PASS | Three tests on this checkout; Unknown/Partial/no-effect/rejected/missing cannot qualify confirmed terminal delivery. Exact developer-08 source pins are in peer-provenance.json. |
| Historical routed EG ingress, N=1/2/3, ownership off | PASS, scoped | Independent operating-system processes and Mongo clients, same routing selection in all cohorts, one signed event through every ingress, one provider POST, one independently read confirmed terminal receipt/cursor, and one sink observation. |
| Integrated ownership N=1/2/3 OFF/ON | FAIL | Enabled N=2 concurrent ingress 503/202; receipt `evidence/mongo-ownership-aadd212-initial.json`. Original control PASS in `evidence/mongo-control-aadd212.json`. |
| Shared Kubernetes user journeys | BLOCKED | No usable synthetic auth/runtime/provider/browser fixture or implemented environment driver. |

The routed process run took 10.50 seconds, 2026-11-13T16:51:27Z through
16:51:38Z. It used authenticated Mongo8.0.13, 1 CPU / 1 GiB container bounds,
a 1 GiB temporary database filesystem and a 64 MiB oplog. The Mongo image digest,
test-binary hash, output hash and successful cleanup are in
`evidence/mongo-routed.json`. No real provider, ownership-mode transition,
load, crash, node loss or HPA was exercised by that test.

## Refreshed integration checkpoint

After merging integration `3415875c609443de0b2ca917b4550e059ff25740`, the same
N=1/2/3 routed-ingress case passed again in 10.22 seconds with a newly compiled
binary. See `evidence/mongo-routed-3415875.json` and
`evidence/refresh-3415875.json`. Formatting and strict cmaeg Clippy passed.
The compiler-path mismatch and two cancelled queued checks are retained;
no current control/oracle rerun is inferred from the earlier results.
The aadd212 ownership run took 95.28 seconds, 2026-11-13T17:30:08Z through
17:31:44Z, and confirmed cleanup. Its first rejection was retained as
`unclassified`; the updated test emits only allowlisted problem detail/code
and after-response durable-plan/target-presence observations. Those observations
cannot prove the rejected request committed its partition queue. Transaction
contention is the implementer's investigation hypothesis, not an established RCA.

## Diagnostic retention correction

The source-frozen `643461b` diagnostic run ended BLOCKED after 124.62 seconds
with cleanup confirmed; `evidence/mongo-diagnostic-643461b.json` retains the
original receipt. Its broad exception handler lost the failure phase and partial
observations. The elapsed time does not establish a specific exception or the
application rejection reason. Debugger-02 tracked this as EG-002.

The corrected runner retains fixed failure phase/class, timeout and censoring
flags, child exit/signal, output SHA-256 and allowlisted partial observations
before cleanup. It emits markers during execution into its private capture and
never saves raw stdout. Setup failures and test deadlines remain separate; an
unconfirmed child exit cannot claim complete cleanup. All 14 runner tests pass
with 94% executable-line coverage. This correction has not been exercised in a
new real run; exact-source tooling review is requested before the next run.
The application N=2 rejection remains unresolved, with developer-15 testing
transaction contention and concurrent retained-item handling.

## Required acceptance scenarios

| Scenario | Status | Remaining evidence |
|---|---|---|
| Cross-replica create/open/send/interrupt/stream/desktop | BLOCKED | Bind real browser, runtime and independent AM ownership/effect oracle to the reviewed driver; execute every replica/balancer pair. |
| AM mid-stream and mid-command loss | BLOCKED | Actual owner death just after renew, original accepted command reconciliation and sustained recovery <=30s; 101 trials. |
| EG inbound sudden loss | BLOCKED | New ownership candidate and migrations; five process and five node-loss-equivalent trials; authoritative provider reconciliation. |
| Autoscale up/down | BLOCKED | Reviewed A2 policy, resolved local max5 capacity and mutation window; actual useful HPA growth to5 and return to2 under canaries. |
| #1535 100 connected users | BLOCKED | 100 streams plus20 admitted turns/minute and10 reads/second through the ramp, zero failed requests. |
| #1530 partition/worker failures | BLOCKED | 20 partitions with hot10/s, retained100k/active100, multipart/attachment receipt list, panic/saturation and provider-start bounds. |
| N=1 versus N=2/scaling performance | BLOCKED | Same-final-candidate baseline repeats, approved stability/noise method, frozen43-criterion measurements and confidence bounds; no route p95 or delta is invented. |
| Track qualification | BLOCKED | Approved-release comparison, compatible rollout/runtime evidence, provider S2 canary and24h monitoring handoff. |

A2 resolves create to **5 seconds end to end**, separately for plain and desktop.
Tab/send/interrupt remain1 second; usage2 seconds. The exact catalog preserves
37 route rows, including repeated/conditional memberships, plus six client
boundaries. The generated preflight table has all43 criteria across N1/N2/scaling
with unmeasured values; see `evidence/PREFLIGHT.md`. Historical production
response-header observations do not fill these cells.

## Retained failures and cleanup

The original512 MiB disposable database bound failed Mongo index creation with
code14031 (disk space). Increasing it to the repository's1 GiB bound resolved
that fixture error. The legacy ingress reference passed N1 then returned409 at
N2 where202 was expected. The first routed attempt returned503 because its
legacy provider fixture had no recovery endpoint. These earlier results are
retained as diagnostics; the passing routed test uses the existing full HTTP
snapshot/SSE fixture and does not reinterpret those failures as success.

The final original executable control passed in 6.33 seconds, 2026-11-13T16:57:27Z
through 16:57:34Z; see `evidence/mongo-control.json`.

Every completed disposable-container run confirmed cleanup. Temporary test
files were removed. Shared cluster lifecycle and checkout cleanup belong to
the environment manager and lead respectively. Task build outputs remain active
for candidate integration; they are outside the repository and are not delivered.

Retained workspace evidence:
`/Users/shining/code/work/tech-lead.staff/developer-17/evidence/`.
Full preflight matrix:
`/Users/shining/code/work/tech-lead.staff/developer-17/evidence/final/preflight/RESULTS.md`.
The PR is a draft until driver/candidate integration, acceptance execution and
architect review of the exact commit are complete. The lead owns merge.
