# Lifecycle reopen contention: issues #1361 and #1362

FINDING: #1361 root cause - Lifecycle reopen stops polling the opening future
while recording startup output. Tool confirmation can hold the same per-Agent
write queue that the recorder needs, so neither future progresses until the
outer lifecycle deadline drops them.

FINDING: #1362 root cause - The accepted Agent stop waits behind that same local
write queue. Retirement correctly waits for the still-live Agent. Once reopen
is dropped, stop publication and retirement can proceed. Both issues share one
root cause, independent of the maintenance sweep regression.

## Production evidence and its limits

Read-only VictoriaLogs queries used the Kubernetes API proxy in namespace
`monitoring`. The issue identifiers were used only as query selectors; this
report contains no owner identity, resource identifiers, request bodies or
conversation content. No production state was changed, no Cloud Logging or
database access was attempted, and no synthetic Agent activity was generated.

The reported verification windows precede the later API deployment. The logs
come from the API ReplicaSet created at 2026-11-05 00:19:58 UTC. A read-only
ReplicaSet GET confirms source `1597d0978ff9d1e4ca39073d3fa3b4a81bcd9a87`
for the API image. AgentWorker and SandboxWorker are composed in that same
API binary. These are control-plane deployment provenance checks, not a fresh
inspection of the already-removed runtime Pod.
The relevant reopen, tool-confirmation and writer-queue code is unchanged
between that production revision and the investigation's develop baseline.

All times below are 5 November 2026 UTC. Times are log emission times after the
named operation; they are not claimed as exact durable event timestamps.

| Time | Correlated observation |
| --- | --- |
| 03:27:24.328 | Fresh Agent `OpenConversation` attempt completed. |
| 03:27:25.011 | `PublishConnection` completed; first-message dispatch followed. |
| 03:28:31.280 | Sandbox `pause.PublishPaused` attempt completed. |
| 03:29:49.833 | Revive runtime provisioning attempt completed. |
| 03:29:50.812 | Credential delivery completed. |
| 03:29:51.555 | Home restore completed; reopen followed. |
| 03:29:54.543 | Native conversation handshake completed in 1,918 ms; session load took 928 ms. |
| 03:35:31.954, inferred | Stop `FenceInteraction` execution began, inferred by subtracting execution duration from its eventual completion log. |
| 03:36:57.566–04:00:28.679 | Retirement's `StopAgents` attempts continued, approximately every 30 seconds. |
| 04:00:51.020 | `revive.ConversationReopen` returned with `published=false` after 1,859,432 ms. |
| 04:00:51.027 | Stop published `CloseProcess`: queue wait 1,519,057 ms, total publication 1,519,073 ms, one transaction attempt, `commit_confirmed=true`. |
| 04:00:52.969 | `CloseProcess` attempt completed. |
| 04:00:53.979 | `PublishStopped` attempt completed. |
| 04:00:58.451 | `StopAgents` advanced; credential revocation followed at 04:00:58.655. |
| 04:01:29.208 | Last runtime teardown attempt completed; `MarkRetired` followed. |
| 04:01:38.660 | `MarkRetired` attempt completed; no further retirement retries through 06:00 UTC. |

The selected Sandbox had 49 `StopAgents` attempts including the final advancing
attempt, one `MarkRetired` attempt and two `TeardownRuntime` attempts. A
`published=true` work-attempt log alone can also mean a retry was persisted.
Advancement to the following step, successful Agent stop publication and absence
of further retirement retries corroborate cleanup completion. A fresh owner API
terminal snapshot was not read; downstream allocation release is not observable
from these worker logs.

The logs directly establish a post-handshake stall and a local writer-queue wait,
not a slow stop transaction or missing desktop. They do not expose the mutex
owner or individual poll events. Source inspection and the deterministic
contention regression establish the lock cycle that explains both durations and
their simultaneous release. There is no evidence of log-capacity exhaustion in
this case, nor a reason to attribute it to an 18-minute Agent lease.

## Lock cycle and maintenance independence

At the pinned production revision:

1. `sandbox/worker/lifecycle/conversations.rs` calls `reconnect_agent` through
   `reopen_output::drain_while` and retains the lifecycle guard.
2. `layer3-adapter/src/hosted/adapter_agent.rs` completes the native handshake,
   then confirms the access-tool binding before returning the channel.
3. `store/agent_access_tools.rs::run` acquires `agent_writes.lock` before its
   transaction and retains it across awaited database operations.
4. The output drain polls the opener and the output receiver. Once an update
   arrives, it directly awaits `record_reopen_update`, whose commit also needs
   `agent_writes.lock`. The opener is no longer polled, so it cannot finish its
   transaction or release the lock. Its nested confirmation timeout is also
   unpolled.
5. `store/agent_worker_transaction/publication_transaction.rs::commit` queues
   `FenceInteraction`'s publication behind this lock. `live_agent_stopper.rs`
   returns its existing retryable waiting condition until the Agent is stopped.
6. `SandboxWorkerConfig::with_execution` raises the lifecycle lease to
   `DEFAULT_TOTAL_TIMEOUT_SECONDS + 60`, i.e. 1,860 seconds. The observed
   1,859.432-second attempt ends at this rounded lease deadline. Cancellation
   releases the opener's writer lock; stop publication then uses one short
   transaction and cleanup advances.

Maintenance and admitted lifecycle work use independent workers. The production
maintenance failure was already active, but this revive had provisioned and
adopted its runtime and completed its handshake. Stop and retirement subsequently
advanced before the 04:32 UTC deployment of `934f68607`. Thus neither operation
was waiting for #1363's home inventory fix. The analogous ordinary Agent
connection drain already drives confirmation concurrently; lifecycle reopen had
not received that scheduling correction.

A separate 03:20–04:05 UTC maintenance query returned 30 background attempts,
all `reason=deadline`, all `completed=0`. The last was at 04:03:51.346, after the
reported resource's cleanup advanced, with `pending_ms=13,335,514`. This directly
corroborates cleanup progress while maintenance remained stalled.

## Correction and verification

The lifecycle drain continues polling the opener while the current receipt is
being durably recorded and acknowledged. It retains an opener result until that
receipt completes, never polls a completed future again, and preserves an
original opening refusal if output custody also fails. It keeps the same
lifecycle guard, finite lease deadline, durable commit-before-ACK ordering and
existing retry/terminal failure mapping. It does not repeat native open or change
operation identities, durable schemas, desktop configuration or maintenance.

- Baseline with the new contention regressions: three tests failed by timeout;
  the lease-expiry test passed. No fault was injected into production.
- With the correction: all nine reopen-drain tests pass. Coverage includes
  concurrent opener success/refusal, release of the writer needed by accepted
  stop publication, retry after unavailable storage without repeating the
  opener or ACK, receipt refusal despite opener success, ACK failure preserving
  the original refusal, and finite lease cancellation without acknowledging
  uncommitted output.
- The existing real-Mongo reopen regression is added to the CI fixture
  selection and its required-prefix inventory. It checks parent fencing,
  duplicate receipts and publication preservation. All seven fixture-runner
  regressions pass.
- Source-size validation passes for 5,157 source files after merging develop.
  All changed Rust files are below 500 lines. Repository structure, configuration/API inventories,
  native recovery and retained-transport contracts pass. Dependency audit and
  the 3,047-commit baseline secret scan pass; the implementation diff is clean.
- Rust 1.96.1, `CARGO_BUILD_JOBS=2`, CI debug/stack settings: workspace format
  and all-target/all-feature Clippy with warnings denied pass. The locked CMA
  build passes. The full normal CMA suite passes: 1,189 tests, with 651 opt-in
  tests excluded by that runner.
- Release/provider executable workflow contracts and their 108/63 rejection
  mutations pass after the Mongo selection change. Staging inventory and the
  related runner/provenance tests pass.

The initial disposable Mongo run passed all 472 contracts across 12 bounded
fixtures, including the reopen fencing/receipt-preservation contract. All eight
isolated Skopeo transport checks passed and their fixtures were cleaned up.

Before publication, develop `926c2f076` was merged as `405296b8c`, retaining
both the new funding Mongo selector and the reopen selector. The merged branch
passes the structure and executable workflow contracts, all seven Mongo runner
regressions, and the dependency audit. Workspace all-target/all-feature Clippy
passes with warnings denied. All 3,131 normal CMA, core and adapter tests pass
(653 opt-in tests excluded), including all nine reopen-drain tests. The locked
CMA build passes. The merged Mongo inventory contains 474 contracts across 12
fixtures. Its full run passed 473 and encountered one `MaxTimeMSExpired` at the
existing 750 ms binding-key lookup limit in
`store::bindings::key_classification::tests::rotation_between_unique_barrier_and_history_keeps_all_retired_hashes`.
That code and test are unchanged by this PR and the merge; the test also passed
in the initial full run. Rerunning the exact unmodified `count:10/12` partition
on a fresh disposable fixture passed all 39 contracts, including that test,
and completed fixture cleanup. No timeout, test assertion or CI gate was changed
to obtain this result. The reopen Mongo contract passed in the merged full run.
All 474 selected contracts therefore have passing results; the initial merged
run was not wholly green and this isolated retry is part of the validation
record. The source history and final diff secret scans are clean.

## Deployment and repair guidance

One PR references both issues. The lead owns merge and deployment. The reported
case already advanced through stopped/retired cleanup on the old binary; no
one-time repair is indicated. Do not revive or reuse a resource whose retirement
was already accepted, and do not resubmit its stop/retirement requests.

Deploying #1363 restores the stalled maintenance sweep; it does not fix this
lock cycle. Other affected in-flight operations can release at their existing
lifecycle deadline or when their old API process drains, but an old worker can
hit the same cycle again on retry. A new API process uses the corrected drain
for naturally reclaimed durable work. No data migration or forced lease edit is
needed for this correction. Independently failed or superseded lifecycle records
must be diagnosed from their actual state; deployment is not a blanket promise
to revive every failed Agent.

For read-only production verification, correlate the issue's identifiers only
inside log selectors. Require `ConversationReopen` to advance to queue delivery
and active publication while preserving conversation identity. When stop is
already accepted, require `FenceInteraction` → `CloseProcess` → `PublishStopped`
and parent `StopAgents` → credential revocation → teardown → `MarkRetired`.
Inspect `Agent work publication timing.queue_ms` separately from transaction
attempts and elapsed time. Confirm the owner-visible terminal state using the
lead's authorized read access before downstream reconciliation; do not release
NyxID allocation by hand as a substitute for CMA finality. Production acceptance
of a new pause/revive/marker flow requires separately authorized fresh test
resources and is not claimed by these read-only observations.
