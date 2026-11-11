# S5 current source handoff

Source checkpoint: `ac6c085bb` on `tl/cma-log-segments/capacity-recovery`.
Part of #1351 and recovery-consumer #1349. Develop freeze is lifted. No S5 PR,
review verdict, merge relay, executable recovery acceptance or activation exists.

## Implemented and executed

The registered core recovery domain/state/service has 38 passing tests. The
last four exercise the authorized dry-run service: final parent intent, owner
change before release, foreign callers, incomplete/wrong ancestry and restricted
metadata. The service uses the existing ResourceAccessService and current-read
policy/binding check. It never reads a log, admits work or invokes the runtime.
The existing model tests cover capacity-only admission, unknown/known aborts,
restart, lease replacement/expiry, no replay, parent intent and gate closure.
Core all-target Clippy with warnings denied passed after this preflight change.
These tests use the explicit model store/runtime; they are not Mongo acceptance.

The REST child is
`rest/agent_capacity_recovery.rs::agent_capacity_recovery_router`, receiving
`Arc<CapacityRecoveryPreflightService>`. It exposes authenticated GET
`/api/v1/agents/{id}/capacity-recovery/dry-run` with private/no-store responses.
Its public root export/composition is still S2-owned and not registered here.

## Scoped source checkpoints

- Admission/status/authority/work use the exclusive TransactionScope.
- `checkpoint_capture::validate_and_fence_begin_scoped(store,current,next,agents,scope)`
  is the S4 lifecycle boundary. The final parameter is `&mut TransactionScope<'_>`.
- S5's allocated checkpoint root now dispatches v2/unknown envelopes to scoped
  capture and fences, then consumes release before the remaining checkpoint,
  Sandbox, event and archive writes in the same transaction. Legacy-only dispatch
  retains the original validator. A concurrent conversion is refused inside the
  legacy transaction and never falls back to lifetime reads for a v2 envelope.
- `sandbox_reconciliation::transaction::apply_scoped(store,request,scope)` consumes
  its scope after segmented publication, then validates/appends legacy siblings
  and the Sandbox/event outcome in that same transaction. A v2 sibling grants no
  adoption permission to legacy siblings. Commit/abort/unknown handling stays at
  the root. The transaction child is currently private to its existing parent.
- The allocated maintenance root captures v2 activity through the scope, then
  continues existing policy, command/session, child and Sandbox fences in the
  same transaction. Preview remains rollback-only. Legacy-only behavior stays
  on the existing path. Consented Pause is currently test-only and still needs
  its corresponding scoped test wiring; production Revive has no log scan.
- `sandbox_unpublished_recovery::transaction::{load_scoped,apply_scoped}` consume
  the scope before original domain helpers use certified per-kind native maxima
  and legacy sibling checks. Both helpers remain unwired at the external root.

The lineage loader uses ascending `_id` metadata pages of 128 documents, against
the requested S2 constrained `TransactionScope::find_page(...,sort,limit)` method
(maximum requested 256). That method is not present yet. None of these new cma
paths has compiled or run Mongo gates; they are source checkpoints only.

## Required integration and remaining S5 work

S2: store `pub(crate) mod agent_capacity_recovery`, integration-test child
`mod agent_capacity_recovery_tests`, default-false worker config field
`parent_not_ready_pacing_enabled`, the REST export/composition above, bounded
scope page method, and carrier migration/schema/backup/grant inventory.

The precise ownership handoff is in #1351 comment 6794826184; caller signatures
are in comment 6795069287. Correction: the actual runtime-loss root is
`sandbox_reconciliation.rs::recover` (starts the transaction around line 161),
not `sandbox_worker_store.rs:126`, which starts purge in this revision. The former
and `sandbox_unpublished_recovery.rs` are absent from the exact manifest. Assign
those roots explicitly or publish their owner wiring; S5 has not edited them.

S1 still supplies transaction source certification. S4 preparation for an already
segmented source currently permits only Rollover admission, so the CapacityRecovery
empty-batch successor case remains reported to S4. The legacy storage-recovery
path requires its exact CapacityRecovery admission and staged BootstrapPlan.

S5 still needs the production accepted-work Mongo adapter, exact completion
receipt reconciliation, admission/status HTTP composition and restricted native
reconnect/registry follow-through. A crash after durable completion but before
registry activation must never enter ordinary PublishConnection, which can replay
managed context/settings or schedule queued first input. These are not covered
by the current model-only completion test or by successful storage adoption.

## Evidence and resource handling

Logs are persisted under the lead's absolute notes directory:
`2026-11-11-1351-preflight-gates.log` (38 tests and Clippy),
`2026-11-11-1351-final-core-recovery.log` (prior 34-test exact acdb0eb72 result),
and `2026-11-11-1351-current-core-gates.log` (prior recovery/projection/Clippy run).
Later S4 bootstrap/compile dependency merges are not a new executable gate pass.
The latest light gates pass: workspace fmt, repository size check (5,917 files),
all 111 changed Rust files strictly below 500 lines, whitespace, and full reachable
history secrets (3,643 commits; zero findings). The earlier all-local-refs scan
reported findings outside this clean reachable history; unrelated branches were
not modified and no secret content was reported.

The owned target was removed after the gates, freeing 0.98 GB (0.92 GiB), on top
of the earlier 0.71 GB cleanup. No owned target, disposable Mongo container or
volume remains. Shared locks are never cleanup artifacts. Subsequent gates used
`lockf -k` with two jobs and one target. A read-only check found heavy-0 missing
again after cleanup; S5 did not recreate it. A later read found a new inode while
heavy-1 retained its inode. No heavy gate ran across that latest disappearance.
Future gates require the existing shared path, persistent lockf invocation and
the machine disk floor. No unrelated worktree was modified.
