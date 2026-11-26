# cma-scale local verification results

Overall: **NO-GO / incomplete**. No shared-Kubernetes acceptance scenario has
passed. This record distinguishes harness validation from real application
acceptance. Tracking issue: #1957. Historical base integration source:
`d6fc953a6cb43f8d9c90337b4e4ce17ecb291fbc`.

Investigated ownership dependency: `aadd212c2810fb12d443998e3cf75bab2fd670cf`.
Its first combined OFF/ON process test **FAILS at enabled N=2**, returning
HTTP 503 instead of 202. N=1 OFF/ON and N=2 OFF completed first; N=3 was not
reached. This supersedes any suggestion that the historical process pass
qualifies the ownership candidate. Developer-15 owns the investigation.
The reviewed `0dd2db929` diagnostic reproduced this acceptance disagreement on
that same application, retaining `routed_admission_unavailable` and fixture stage
observations through the rejection check. Newer corrective candidates have
not been exercised by this process fixture.

## Current deadline gate and A3 comparison

The owner-adopted 25 essentials control local N1/N2/2→4→2/kill by15 November
01:30 SGT and final production verification by18:00. Older full-matrix counts
below are historical/best-effort, not additional essentials blockers. Actual
candidate image/configuration, usable fixture, coordinated window and executed
essentials remain absent from this source packet. S3 still requires automatic
HPA up/down with min2/max5 and independent lifecycle/capacity evidence.

The comparator now pins the complete A3 table: critical uses qualified measured
noise, UX uses at most10% p95 (5% aim), non-sensitive uses30ms. Confidence upper
bounds must fit the one applicable allowance; absolute owner targets, unknown
classes and all failure/incomplete denominators remain. Activity API<1s and
page completion are separate pending evidence cells; no page ceiling is invented.
This source delta has32 JavaScript tests passing,99.68% lines,97.96% branches
and97.89% functions; the new policy module is100% for all three. ESLint/syntax
and source-size checks pass.
These are helper tests; no numerical application or production comparison ran.
The subsequent M3 sequence correction passes35 JavaScript tests: defaults to
2→4→2, retains explicit2→5→2 support, rejects a mismatched sequence, and fails
on downscale canary errors. Lint/syntax/source-size checks pass.

The separate EG lifecycle component at5fc00eba3c corrects architect EGL-001:
an arbitrarily late timeout could previously qualify complete evidence.
Its mandatory registered return bound retains a late timeout observation but
prevents COMPLETE.20 pure/mocked tests pass; the original counterexample is
retained. Architect01 closed EGL-001 on the corrected source. It supplies no watchdog, live
execution, process death or no-late-effect proof.

The current local per-request bootstrap has no qualified production driver/raw
contract. One production post-window cannot satisfy the passive method's three
baseline/three post-window requirement. A3 supplies no estimator substitution;
that early statistical comparison remains INCONCLUSIVE without further qualified
evidence. No overlapping window or local noise is promoted to production evidence.

## Executed local gates

| Gate | Result | Evidence and limits |
|---|---|---|
| Historical JavaScript harness before A3 | PASS | 28 tests; 99.66% lines, 97.85% branches, 97.84% functions for lib/scenarios; ESLint and syntax checks pass. Current A3 gate is recorded above. |
| N=1-only selection | PASS, implementation only | Three windows, no fault/scale/cross-replica calls; all 43 rows retained; missing/failed/unrendered cases and cleanup tested. Live driver still absent. |
| Disposable Mongo runner | PASS, implementation only | Final 28 Python tests; three focused parser cases cover 23/23 executable lines. Prior c08 coverage: runner 93.78%, custody 87.74%. Includes real local descendants and full runner with mocked Docker; counts are separate. |
| Pinned Rust formatting and touched-crate lint | PASS | Rust 1.96.1 fmt; cmaeg all-target/all-feature Clippy with warnings denied. |
| Original EG executable control | PASS | Same test-binary hash as replica case; signed ingress and complete streamed response; disposable Mongo removal reported; historical descendant exit was not independently observed. |
| Imported delivery oracle | PASS | Three tests on this checkout; Unknown/Partial/no-effect/rejected/missing cannot qualify confirmed terminal delivery. Exact developer-08 source pins are in peer-provenance.json. |
| Historical routed EG ingress, N=1/2/3, ownership off | PASS, scoped | Independent operating-system processes and Mongo clients, same routing selection in all cohorts, one signed event through every ingress, one provider POST, one independently read confirmed terminal receipt/cursor, and one sink observation. |
| Integrated ownership N=1/2/3 OFF/ON | FAIL | Enabled N=2 concurrent ingress 503/202; receipt `evidence/mongo-ownership-aadd212-initial.json`. Original control PASS in `evidence/mongo-control-aadd212.json`. |
| Instrumented original-application diagnostic | FAIL | Tooling 0dd, application aadd212; N2 ON 503/202, public detail retained, uncensored exit 101. N1 OFF/ON and N2 OFF completed. Parent/group/container cleanup confirmed; see `evidence/mongo-diagnostic-0dd.json`. |
| Shared Kubernetes user journeys | BLOCKED | No usable synthetic auth/runtime/provider/browser fixture or implemented environment driver. |

The routed process run took 10.50 seconds, 2026-11-13T16:51:27Z through
16:51:38Z. It used authenticated Mongo8.0.13, 1 CPU / 1 GiB container bounds,
a 1 GiB temporary database filesystem and a 64 MiB oplog. The Mongo image digest,
test-binary hash, output hash and historical cleanup claim are in
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
17:31:44Z, and reported cleanup=true. Its parent-exit check did not independently
confirm descendant exit; no historical leak is asserted. Its first rejection was retained as
`unclassified`; the updated test emits only allowlisted problem detail/code
and after-response durable-plan/target-presence observations. Those observations
cannot prove the rejected request committed its partition queue. Transaction
contention is the implementer's investigation hypothesis, not an established RCA.

## Diagnostic retention correction

The source-frozen `643461b` diagnostic run ended BLOCKED after 124.62 seconds
with cleanup=true in its historical receipt; `evidence/mongo-diagnostic-643461b.json` retains the
original receipt. Its broad exception handler lost the failure phase and partial
observations. The elapsed time does not establish a specific exception or the
application rejection reason. Debugger-02 tracked this as EG-002.

The corrected runner retains fixed failure phase/class, timeout and censoring
flags, child exit/signal, output SHA-256 and allowlisted partial observations
before cleanup. It emits markers during execution into its private capture and
never saves raw stdout. Setup failures and test deadlines remain separate; the
14-test retention gate did not cover a surviving descendant. Exact-source review
of be5a8a46b found F1/EG-005: direct-parent exit incorrectly certified process
cleanup. That runner remains NO-GO; the c08 correction and its later execution
are recorded separately below.
The application N=2 rejection remains unresolved, with developer-15 testing
transaction contention and concurrent retained-item handling.

## Process custody and pod identity corrections

Architect F1 / debugger EG-005 blocks the be5a8a46b Mongo runner. The correction
uses a separate direct-child anchor to reserve the group ID until every other
member is absent, on normal and exceptional outcomes. Parent exit and group
exit are separate observations. TERM/KILL each have a ten-second bound; the
120-second test threshold is unchanged. Unconfirmed cleanup retains the private
fixture directory and Mongo container and cannot report PASS. This custody
model requires the known non-daemonizing fixture executables and an exclusive
single-threaded child reaper; it does not contain arbitrary escaping workloads.

The first local regression gate found that macOS getpgid can return absent for
an unreaped zombie. The corrected reservation relies on exclusive custody of
that unreaped child, whose PID cannot be reused until wait. The initial failing
gate remains in the workspace evidence; the passing rerun is separate. Native
local tests cover a TERM-exited parent with a TERM-ignoring descendant writing
to a file, early parent failure, normal/absent-group cleanup, observation errors
and lost custody. An end-to-end runner regression mocks every Docker operation
and uses real local processes to check removal ordering.

Architect F2 corrects pod deletion to send DeleteOptions.preconditions.uid in
the actual raw API request. The name-reuse regression returns conflict and
preserves the replacement. This is a mocked API test; no cluster deletion or
process-death observation is inferred. Both corrections received exact-source
c08c55341 GO. The later instrumentation received its own scoped verdicts below.
These tooling verdicts do not resolve the original N2 503 or an acceptance row.
Developer-15's published ab546c contention fix is a separate tested dependency;
no new process execution against it is claimed here.

## Reviewed original-candidate diagnostic at c08

The architect and scoped reviewers cleared the c08c55341 tooling correction.
The next already-authorized run retained the original aadd212 application and
643461b compiled fixture, with both executable hashes checked before execution.
It ran 2026-11-13T18:24:08.997431Z–18:26:13.614117Z and ended
**BLOCKED / process_test_deadline**, with the unchanged120s test wait and124.643s
total elapsed including setup/cleanup. This is not a recovery or drain timing.

Completed markers were N1OFF, N1ON, N2OFF, N2ON and N3OFF. No ingress rejection
or assertion marker was retained. The N3OFF PASS marker precedes function-local
teardown; even entry to N3ON is unobserved. The aggregate test is censored and
has zero completed test cases. This run does not explain or close the original
N2ON503 and does not qualify the newer ab546c/23f6a application source.

The reviewed runner retained the output hash, partial markers and fixed timeout
phase. Parent/group/container cleanup are independently true; TERM was needed,
KILL was not. Post-run inventory found zero anchors, application processes,
private fixture directories or labelled scale containers. See
`evidence/mongo-diagnostic-c08.json`, `diagnostic-source-binding-c08.json`,
`diagnostic-outcome-c08.json` and `cleanup-c08.json`. Earlier receipts remain
unchanged. The subsequent source-only instrumentation adds fixed stage start/end markers
and per-replica HTTP header statuses. It preserves cohort-local monotonic times
and marks completion after local teardown. The6f580ea84 instrumentation received
scoped architect GO. A final parser correction discards unterminated records to
avoid interpreting a truncated numeric field.28 Python tests and lint pass; its
three focused parser tests cover23/23 executable lines. Rust/emitter bytes remain
unchanged from the passing pinned fmt/strict cmaeg Clippy and diagnostic unit
gates. See `evidence/marker-cutoff-gates.json` for this additive correction.
Architect and scoped peer source GO at 0dd2db929 preceded the separately bound
original-application invocation below. The 120-second test deadline is unchanged.

## Instrumented original-application diagnostic at 0dd

The owner invocation ran 2026-11-13T18:49:42.409622Z–18:49:52.374197Z under the
standing local-verification assignment and normal resource slot. It used tooling
0dd2db929, the 6f580ea84 compiled fixture, and the original aadd212 application.
Test binary SHA256 starts `e81f4e57`; application SHA256 starts `54cbd773`.
Full bindings are in `evidence/diagnostic-source-binding-0dd.json`.

Result: **FAIL / cross_ingress_not_accepted**, exit 101, no timeout or censoring.
Reported 9.991006 s includes fixture setup/cleanup and is not an application
latency, drain or recovery measurement. N1 OFF, N1 ON and N2 OFF completed their
workload checks and explicit teardown/cohort-end boundaries. N3 was not entered.

| N2 ON observation | Cohort-local monotonic milliseconds |
|---|---:|
| Migration start/end | 2 / 133 |
| Process 1 ready start/end | 133 / 490 |
| Process 2 ready start/end | 490 / 842 |
| Independent oracle open start/end | 844 / 903 |
| Ingress 1 / ingress 2 start | 903 / 904 |
| Process 1 HTTP 503 header | 932 |
| Process 2 HTTP 202 header | 935 |
| Both bodies parsed, in input order after request barrier | 935 |
| After-response oracle start/end | 935 / 939 |

The two calls replay one identical synthetic event. The rejected response has
public detail `routed_admission_unavailable`; one response passes the accepted
body check. The independent after-response read finds a durable plan and zero
existing target-state records. This does not identify which caller created the
plan or prove that the rejected call committed its queue entry. The assertion
ends the test before terminal-delivery waiting and provider/sink reconciliation.
No terminal accepted-event accounting, loss, duplicate or late-delivery finding
follows from this snapshot.

The public detail does not select an internal admission error or transaction
phase. No matching typed per-request application warning was retained. The
reviewed runner discarded raw capture after hashing and allowlist extraction.
Developer-15 owns the application RCA. This run does not recover the original
discarded exception, locate the c08 timeout phase or qualify newer corrective
source. No fault, cluster, HPA, load or external-provider acceptance occurred.

Parent/group/container cleanup are independently true; no TERM/KILL was needed
and no fixture directory was retained. A later inventory found zero owned
anchors, application/test processes, fixture directories or labelled containers.
`evidence/diagnostic-outcome-0dd.json` and `cleanup-0dd.json` retain the result
interpretation and inventory. `diagnostic-publication-0dd.json` binds all files.
The original raw result SHA256 is
`99f2033c552ea9a8613288b7d74d6f19ad4a13d0168f1a48f3bc7676ea72bf8d`.
The published `mongo-diagnostic-0dd.json` compacts observation rows for review;
decoded data is identical and the manifest records exact raw reconstruction.
The published pre-run binding renames its compiled-source metadata key to
`compiled_fixture_source`; the original bytes remain retained and reconstructable.
This avoids a scanner false positive on the Git revision without an exemption.
All historical receipts remain unchanged. Full acceptance remains BLOCKED.

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

Every completed disposable-container run reported container removal. Those
historical receipts used the reviewed flawed parent-only process predicate and
do not independently prove that every descendant had exited before disposal.
No historical leak is asserted. New regression fixture cleanup is recorded
separately. Shared cluster lifecycle and checkout cleanup belong to
the environment manager and lead respectively. Task build outputs remain active
for candidate integration; they are outside the repository and are not delivered.

Retained workspace evidence:
`tech-lead.staff/developer-17/evidence/`.
Full preflight matrix:
`tech-lead.staff/developer-17/evidence/final/preflight/RESULTS.md`.
The PR is a draft until driver/candidate integration, acceptance execution and
architect review of the exact commit are complete. The lead owns merge.
