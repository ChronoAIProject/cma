# Local cma-scale verification

Tracking: #1957; acceptance: #1360, #1541, #1535 and #1530. Integrate through
`tl/cma-am-replicas/integration` / #1370. The lead owns merge. Under the speed
directive, architects review integrated production changes; intermediate tooling
needs no additional review gate. Never wait for GitHub CI.

This directory currently contains executable scenario orchestration, evidence
validators, local HTTP/Kubernetes helpers and harness tests. **The environment
driver is a required integration dependency, not a shipped working CMA adapter.**
The driver must bind the actual local application, synthetic users, independent
durable outcome oracle, browser, streams and fault controls. No real environment
scenario has passed merely because the unit/loopback tests pass. Missing inputs
produce BLOCKED rows and exit 2. Do not substitute `test/fixtures.mjs` for a driver.
The separate real local create runner is now in `timing/local-create.mjs`; see
`timing/LOCAL-CREATE.md` for its runtime/auth/Web prerequisites and observer tests.
It supplies plain/desktop timing only and has not run against an application yet.

## Prerequisites

- Local-env-manager-02 hands off the isolated `kind-cma-scale` context and
  `cma-scale` namespace, private kubeconfig location, namespace UID hash, explicit
  loopback application URLs, per-replica ingress, candidate SHA/digests/switches,
  resources and Mongo replica-set identity. Numeric loopback hosts only: no DNS,
  public endpoint, redirects, owner profile or production credential bootstrap.
- Local-verifier-02 owns the matrix and mutation window. No concurrent HPA ramp
  and rollout during qualification. Preserve both the desired and actually alive
  replica counts, including every terminating process. A PDB does not bound
  HPA terminations or protect against node failure.
- The entrypoint pin is an identity check, not a sandbox for arbitrary imported
  JavaScript. Use the injected guarded fetch and bounded
  local helpers. Preflight must bind actual environment evidence, not return
  operator guesses. Every blocking operation must honor cancellation/deadlines.
- Provision test-owned synthetic identities, scoped credentials in environment
  variables, Agent/runtime/browser fixtures, durable receipts, provider receipt
  reconciliation and an independent execution/effect ledger. Never load real
  user data. No Kubernetes Secret reads or pod exec.
- Use the exact A2 catalog in `latency-route-catalog-a2.json`, pinned by SHA-256
  `a47475609633f3f736147dbded6dace5864d174b0cf58283425e639aba3ad951`.
  Its 43 criteria preserve all 37 route rows (34 distinct method/path pairs)
  and six client boundaries, including conditional and alternate paths.
  Plain and desktop create each require <=5s end to end; open/send/interrupt
  <=1s and usage <=2s. A2 withdraws the former create<=1s interpretation.
  Frozen historical observations are context, never replacement samples.

## Local implementation gates

From this directory, with Node >=22:

```sh
npm ci --ignore-scripts --cache /absolute/task-workspace/npm-cache
npm run lint
npm test
npm run coverage
```

Coverage applies to new `lib/` and `scenarios/`, not the unchanged, separately
verified timing helpers. No package has a production install/build side effect.
Keep dependency cache, build target and test temporary directories within the
assigned workspace; delete them after the retained evidence is captured.

The real EG process fixture is in
`api/crates/cmaeg/tests/relay/runner_process.rs`. `with_database` accepts a shared
`Arc<DatabaseFixture>` and `with_providers` injects a provider registry; child
executables each create independent clients and local state. The routed case
reuses the repository HTTP snapshot/SSE fixture and developer-08's exact helper
commit, recorded in `peer-provenance.json`. New tests nest under `runner_binary::replicas`, so existing authenticated
Mongo contract runners select them. Use pinned Rust 1.96.1, CARGO_BUILD_JOBS=4,
task-local CARGO_HOME/CARGO_TARGET_DIR/TMPDIR. Pin both RUSTC and RUSTDOC to
the absolute 1.96.1 toolchain binaries as well as Cargo; the shared wrapper may
otherwise resolve a different compiler from PATH. Set RUSTUP_TOOLCHAIN=1.96.1
for the invocation. These are process-local settings. Required gates:

```sh
cargo fmt --all --check
cargo clippy --locked -p cmaeg --all-targets --all-features -- -D warnings
cargo test --locked -p cmaeg --test relay_ingress runner_binary::replicas -- --ignored --test-threads=1
```

The process test requires an authenticated disposable Mongo replica set via
`CMA_AUTHENTICATED_TEST_MONGODB_URI`; it refuses an ordinary standalone Mongo server. Reuse
`.github/scripts/authenticated-mongo-contract.sh` under the environment
manager's agreed lifecycle, never an existing production database. The process test uses the same routed admission/recovery selection at N=1/2/3,
with separate OFF controls and ON ownership cohorts. Each cohort has a fresh
database; ON invokes `--migrate-replicas`, shares one public deployment origin
and advertises each process's own loopback address. This avoids mode-off reuse
after the protocol floor is activated. It uses
independent real EG executables and a separate limited Mongo oracle. It checks a
single terminal text receipt/cursor against the provider POST ledger and sink
observations. It is **not** proof of native provider exactly-once, the
full ownership contract, pod loss or node loss.

For a compiled test binary, `mongo-local.py` creates and removes its own Mongo
container and runs exactly one selected test. It cannot attach to an existing
cluster. Passing cohort markers and fixed rejection/after-response custody
observations are retained even for incomplete/censored execution; raw response
bodies and stdout are discarded after hashing and allowlist extraction. Inspect
failure_phase/error_class, timed_out, exit_code/termination_signal,
test_parent_cleanup, process_group_cleanup, process_cleanup and container_cleanup
before interpreting a receipt. `owned_process.py` reserves the process-group ID
with a separate direct-child anchor. The anchor is never polled/reaped until all
other group members are observed absent. This includes killed/zombie anchors;
parent exit and pipe EOF do not release group custody. Every normal/exception
exit checks the group, then bounds TERM/KILL cleanup to ten seconds each. Only
after that observation may the fixture directory and Mongo be removed. Failed
confirmation retains both resources and reports BLOCKED/incomplete cleanup.
The group helper assumes these known non-daemonizing executables; it is not a
sandbox for arbitrary children that escape into a different group. It requires
a single-threaded POSIX caller with the default SIGCHLD disposition and exclusive
ownership of waiting for its anchor. Local regressions exercise a parent that
exits while a descendant ignores TERM with file-backed output, including the
complete runner with mocked Docker and real local processes.

The previous `be5a8a46b` runner is NO-GO under architect F1 / debugger EG-005.
The c08c55341 correction received exact-source architect and scoped peer GO.
Its one authorized original-candidate diagnostic remained censored; see RESULTS.
Any changed instrumentation requires a new exact-source verdict before real use. Historical receipts are unchanged and do not establish
independent descendant-exit proof. Use `--test-name` for the original executable control; the default selects
the replica test. The fixed 1 GiB database filesystem matches the existing
repository runner and the oplog is bounded to 64 MiB. A 512 MiB filesystem failed
index creation with Mongo 14031 and was corrected; this was a runner sizing bug.

```sh
python3 mongo-local.py --binary /absolute/task-workspace/build-target/debug/deps/relay_ingress-HASH --out /absolute/task-workspace/evidence/mongo
```

## Censored diagnostic stages

The original six-cohort diagnostic retained five PASS markers but exhausted the
120-second test wait before overall completion. PASS markers historically precede
function-local teardown, so they do not prove the next cohort was entered. The
added `replica_trace.rs` markers expose cohort entry, setup, migration, each
replica ready, ingress/headers/body, independent oracle, each exit, reconciliation,
database cleanup and teardown. The outer wrapper marks cohort completion only
after the inner future returns and its local fixtures have dropped.

`cohort_stage_observations` retain fixed phase/start/end names, fixture process
ordinals and monotonic milliseconds relative to that cohort. Values from different
cohort origins must not be subtracted. An unmatched start remains censored; an
end means only that boundary returned. The emitter always ends a record with a
newline; an unfinished final record is discarded because cutoff may truncate its
last numeric field. `ingress_header_observations` retain status
codes as each response arrives, before waiting for other replicas. A202 header is
not proof of durable admission or delivery. No identities, bodies, credentials or
free-form errors enter these new fields. Earlier receipts remain unchanged.

The final newline-framed parser at 0dd2db929 received scoped architect and peer
GO. One owner-operated invocation exercised these markers on original application
aadd212 and failed N2 ON ingress with 503/202; see RESULTS and the 0dd evidence
packet. Its after-response plan/target snapshot is not terminal delivery or the
failed request's queue-commit proof. The older censored c08 phase remains unknown.
Every future invocation still needs its applicable source, binary, environment
and execution scope; these reviews supply no new allowance or deadline increase.

## Run

The owner-adopted 25 essentials and A3 class table control the deadline gate.
The historical `all` selector is a full-matrix implementation, including old
trial counts and an unconditional external-qualification blocker. It is not the
M3 essentials adjudicator. M3 uses N1/N2/2→4→2/kill; production automatic HPA
min2/max5 remains a separate required S3 observation. No old matrix row supplies
an additional promotion prerequisite beyond its applicable essential.

`scalingPeak` defaults to4 for M3; the example configuration declares it explicitly.
Use5 only for an explicitly selected 2→5→2 run. Collection and comparison require
the selected sequence and retain the actual replica sequence in raw samples and
comparison rows. A 2→5→2 observation cannot satisfy a registered 2→4→2 comparison.
The autoscale scenario stops stress at the selected peak while connected users
and canaries remain active through downscale. The HPA min2/max5 policy is unchanged.

`performance-classes-a3.json` preserves the exact decoded architect A3 inventory:
43 catalog memberships, 80 observed routes and 18 operation families. The
published copy compacts rows; its SHA256 is pinned in `performance-policy.mjs`,
alongside the original artifact hash. Each comparison retains the class, context,
table hash, allowance and original error/incomplete denominator. Critical paths
use preregistered measured noise; UX uses 10% of the fixed baseline p95 (5% aim);
non-sensitive work uses 30ms. Do not add noise to percentage or millisecond caps.
An awaited critical dependency inherits CRITICAL; an unknown class is blocked.
Absolute owner targets and the upper-confidence-bound comparison still apply.

This updates the 43-row comparator, not the missing application driver or the
supplemental Activity collector. Activity API p95 must be strictly below 1s;
page completion remains separately measured with no invented absolute ceiling.
Baseline stability/noise registration remains required; the legacy zero default
noise ceiling is conservative and is not a measured noise allowance. A3 must
not be used to declare a noisy/inadequate sample suitable or widen a critical
allowance after seeing its comparison. The lead's speed directive removes
intermediate tooling-review gates; integrated production changes retain their
correctness/safety review. Actual execution scope and production approvals remain.

Copy `config.example.json` to `config.local.json`, fill the exact reviewed local
handoff and pin the driver. Place no credential values in that file. Then:

```sh
node run.mjs --config config.local.json --out /absolute/task-workspace/evidence/local-run
```

For the lead-directed N=1-first handoff, the environment owner establishes fixed1
and proves one Ready process per service with no terminating predecessor. Run:

```sh
node run.mjs --scenario baseline-n1 --config config.local.json --out /absolute/task-workspace/evidence/n1-capture
```

This selector calls only driver preflight, three `measureCohort('n1')` windows and
cleanup. It does not call fault, scaling or cross-replica operations. Each window
must retain actual replica count, timestamps and all 43 criteria. Incomplete,
failed or unrendered journeys block the capture; raw evidence is retained before
evaluation. A completed capture is not a performance-comparison PASS: N2/scaling
deltas stay unmeasured and overall status remains NO_GO (exit 2). The driver still
needs implementation and review before either command is usable with CMA.

The example uses the handed-off TLS listener for both services. The reviewed
driver must apply the environment's exact service Host routing and local CA
verification; the port alone does not select AM versus EG. Do not disable TLS
verification or replace these placeholders with production endpoints.

Running without `--config` is a safe preflight failure and writes a complete
BLOCKED matrix. It performs no application/cluster request. A successful live
scenario prints `DONE:` immediately; a failure prints `BLOCKED:` with a fixed
error code and stops subsequent gates. Preserve the failure, cleanup proof and
oracle artifact hash, then peer-send the owner through the guarded team command.
AM: developer-13; desktop/shared state: developer-14; EG ownership: developer-15;
EG ingress/delivery: developer-08; workers: developer-02; HPA/capacity: developer-16.
If the grant is absent, report the denial in the current pane for lead relay.

## Scenario contract

| Gate | Required observation |
|---|---|
| Cross-replica | Every ordered create/use route pair across each AM and the balancer; create/open/send/interrupt/live stream/desktop; actual serving replica hashes; live and durable stream continuity; rendered open; desktop frame and Sandbox exclusivity; durable reconciliation. |
| AM sudden loss | Mid-stream and mid-command after a just-renewed lease; actual process death; original accepted command/effect identity; sustained client success within 30s including injection time; terminal and replay continuity. |
| EG sudden loss | Inbound work demonstrably accepted and in flight; durable receipt/provider outcome reconciliation; no unknown counted as success. |
| HPA | Actual metric-triggered desired/Ready5 for both services, stress removal, full downscale stabilization back to2 while canaries continue; no failed/dropped requests or stream gaps. A2 max 5 strengthens the original >=4 gate. |
| 100 users | 100 simultaneous users/streams throughout; 20 admitted turns/minute and10 reads/second retained through up/down. |
| Deterministic | 2/3 independent process/client instances, real Mongo CAS/transactions, partitions, epochs, stale input/result/ACK/terminal, reconnect, auth/hops, desktop, quotas, compatibility, switch-off, zombie/skew; cold/warm caches and separate rollout. |
| Replica sweep | AM and EG2→3→5→2, 100 users, continuously admitted work, real resource/client accounting and >=30% measured Mongo headroom. This manual progression does not prove HPA. |
| Repeated loss | 101 AM trials (separate graceful/process results); EG5 process +5 local node-loss equivalents; every recovery <=30s. Node evidence requires kill plus cordon/rescheduling, old-owner nonservice and replacement recovery. A process kill alone never labels itself node loss. |
| EG load | 20 partitions, hot10 events/s, others1/30s;100k retained/100 active;8-part replies/attachments; worker-panic and saturated-shard; provider-start p95<=2s/max<=10s, no pre-start expiry, no unresolved Unknown/Partial. |
| Performance | Exact frozen catalog; three N=1 windows, preregistered repeatability noise, matched N=2/scaling and candidate baseline identity; absolute limits and confidence upper bound; all attempts remain accounted. |

Contract v1/A1/A2 also requires final-candidate N=1 versus the approved latency-work
baseline, compatible-rollout performance, native/hosted runtime compatibility,
and provider-specific S2 canary evidence plus assigned 24h post-go-live monitoring.
A2 replaces the earlier pre-release 24h/1000-event requirement. Every accepted
canary must be terminal/reconciled; unresolved Unknown/Partial blocks promotion,
and reconciled failures do not become successful deliveries. **Those evidence integrations
remain outstanding and prohibit a track GO even if the implemented local subset
passes.** Local verification never grants production activation.

A2 HPA policy is min2/max 5, up stabilization 0s with Max(+2 pods,+100%)/60s,
down stabilization 300s and -1/120s. Extend the down period to ceil(qualified
p95 handoff+30s) when that p95 exceeds 90s. Missing/censored timing cannot qualify
cadence. Capacity uses hard process lifetime, all terminating processes and all
Mongo clients plus topology sockets, not the p95. No rollout/HPA overlap.
The current local environment's max 4 budget and 500m EG limit still require the
environment/capacity owners' resolution before this max 5 campaign can run.

## Evidence and measurement

`results.json` and `RESULTS.md` retain explicit PASS/FAIL/BLOCKED per gate.
The default artifact contains no response bodies, credentials, raw resource/user
IDs, browser storage, HAR, screenshots or traces. Use SHA-256 for identities and
join oracle artifacts by hashes. Retain source SHA, image digests, switches,
fixture seed, hardware/resource shape and window timestamps in a sanitized
environment manifest; its hashes must match every cohort. Preserve raw timing
samples in separate sanitized artifacts; summaries never replace the samples.
The runner writes every cohort to `samples/` before using it in comparison,
retaining failed attempts and empty missing-route arrays. Existing sample files
cannot be overwritten. Reports retain all 43 criteria even when preflight fails,
with unmeasured values and BLOCKED status.

Reuse provenance is `timing/provenance.json`. The unchanged core module was verified
against prod-verifier-02's `PASS_FOR_LOCAL_SYNTHETIC_REUSE_ONLY` verdict, SHA256
`771748498e063396ab22392c9fd3e361645949f9f5bf74381a23bd5a3d550fbd`.
The local create runner now includes the lifecycle/composer/cleanup helpers.
Its private cleanup journals retain local resource IDs for recovery; sanitized
timing results omit them. The full environment/scale driver remains outstanding.

Percentiles use the reviewed helper's nearest-rank definition. p99 is omitted
below 1000 completed observations. Failed attempts retain elapsed time and prevent
acceptance. Open-loop scheduled-start timing includes generator queueing; generator
overload is a failure. Separate healthy/fault windows and cache/navigation modes.
Do not replace completed-journey latency with HTTP headers or acknowledgement.

Three baseline windows precede N=2. `preregistered-noise/` is written before
candidate measurement. There is no approved numerical noise ceiling: current
default is zero; nonzero baseline variance is BLOCKED. The comparator reports a
95% circular-block-bootstrap interval (block 10, 1000 deterministic draws) for p95
difference; its upper bound must fit the fixed A3 class allowance. The analyst/architect
must review dependence, sample size and the frozen policy before acceptance.
Do not tune block size, tolerance, sample window or exclusions after seeing N=2.
This local per-request method has no qualified production driver/raw-receipt
contract. It cannot replace the analyst's passive repeat-window method, whose
three baseline and three post-window requirement is not met by one post-soak
window. A3 changes neither estimator nor evidence adequacy. Keep that early
production comparison INCONCLUSIVE until its applicable method is qualified.

## Cleanup and review handoff

Each fixture must prove its own cleanup; failed cleanup stops progression. Stress
must stop on errors while canaries survive until measured downscale completes.
`LocalCluster.removePod` submits raw DELETE with `DeleteOptions.preconditions.uid`
and zero grace for the verified test-owned pod. A replaced name causes a server
conflict, with no name-only fallback. The request explicitly does not prove
process death. Architect F2 is source-cleared at c08c55341. That verdict does not authorize
a cluster fault; a scoped environment/window and independent death oracle remain required.
The driver must independently observe actual termination and exact accepted work.
The environment manager owns shared cluster lifecycle. Never remove peer worktrees,
shared build caches or infrastructure. The lead owns this checkout's final cleanup.

Handoff once by plain peer-send: changed behavior, commit, test command/results
and the exact remaining runtime input. Do not generate intermediate hash chains
or tooling-review requests. Report actual scenario outcomes separately from
component tests; missing application observations remain unqualified.
