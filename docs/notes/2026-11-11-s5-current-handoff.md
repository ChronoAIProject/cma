# S5 current source handoff

Source checkpoint: `e9b134d8c` on `tl/cma-log-segments/capacity-recovery`.
Part of #1351 and recovery-consumer #1349. Develop freeze is lifted. No S5 PR,
review verdict, merge relay, executable recovery acceptance or activation exists.

## Latest source checkpoint; execution pending

Normally merged S2 recovery registrations `34c9a3228`, including bounded scoped
page enumeration `9e5c47a72`. The store and its Mongo tests are now
registered in source; the dry-run router is exported and parent pacing is wired
independently with a default-false flag. No new compile pass is claimed yet.

Seven additional core tests cover live completion crashes at three boundaries,
unknown completion with visible/absent writes, exact attempt receipts, reused
holder names with stale fences, input/native changes after handshake, competing
admission snapshots and revoked ownership. The live cases cover idle and active
Agents and preserve original input/receipts without scheduling ordinary work.
Service acknowledgement now requires the entire expected completion for that
lease attempt, including revision, attempts, fence and timestamps.

New scoped settlement helpers (`agent_capacity_recovery/settlement.rs`) propose
claim, retry, supersession and completion inside the caller's transaction. They
require the original S4 admission publication and exact prior result digest,
replace only the exact work carrier, fence parents and invoke the sole S4 Agent
CAS/log publication. Carrier insertion/replacement share the same bounded codec.
Three additional registered Mongo tests cover carrier commit/abort, stale CAS,
immutable request/native/input facts, corrupt projections and unknown fields.
These carrier tests grant no recovery authority and are not end-to-end admission.

The 45-test core candidate and new Mongo source have not executed yet: heavy-0
has remained occupied through the attempted acquisitions. The persistent-lock
runner uses `lockf -k -n`, two jobs, one target and disk/inode guards. It releases
the lock after each gate. No target or Mongo fixture has been allocated so far.

## Previously executed

The earlier registered core recovery domain/state/service checkpoint had 38 passing tests. The
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
Its public root export is now registered by S2; production composition is still pending.

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

The lineage loader uses ascending `_id` metadata pages of 128 documents through
S2's merged `TransactionScope::find_page(...,sort,limit)` (1..=256 rows and 4 MiB
per page, no partial result on failure). New cma source on this branch has not
run a compile or Mongo gate; no acceptance result is claimed.

## Required integration and remaining S5 work

S2's store/test/export/config registrations and bounded scope method are merged.
Remaining shared work is production dry-run and recovery composition plus carrier
migration/schema/backup/grant inventory. The carrier is `agent_capacity_recovery_work`:
`_id` is the stable operation key, `format_version` is int64 1, `agent_id` is the
original Agent ID and `work` is the strict domain record. Decode is bounded to
32 KiB BSON and 16 KiB work JSON. Discovery must index/bound `work.phase` with
`work.ready_at` or `work.lease.expires_at`; no collection-wide worker scan.

S2's strict-lint allocation now owns `sandbox_reconciliation.rs` and
`sandbox_unpublished_recovery.rs`; S5's scoped child signatures remain unchanged.
S4 owns the lifecycle checkpoint root. These allocated roots require their
owners' wiring and executable qualification.

S1 still supplies transaction source certification. S4 preparation for an already
segmented source currently permits only Rollover admission, so the CapacityRecovery
empty-batch successor case remains reported to S4. The legacy storage-recovery
path requires its exact CapacityRecovery admission and staged BootstrapPlan.

S5 still needs the production transaction/outcome adapter around the new scoped
settlement helpers, exact completion receipt reconciliation, admission/status
HTTP composition and restricted native reconnect/registry follow-through. A crash after durable completion but before
registry activation must never enter ordinary PublishConnection, which can replay
managed context/settings or schedule queued first input. The new model tests establish receipt idempotence after this crash but do not
establish production registry activation. The existing adapter channel has no
native-state confirmation; the ordinary reattach path replays settings/context.
The concrete ownership/interface request is in
`docs/notes/2026-11-11-s5-runtime-confirmation-handoff.md`. No shared adapter files
have been edited without allocation.

## Evidence and resource handling

Logs are persisted under the lead's absolute notes directory:
`2026-11-11-1351-preflight-gates.log` (38 tests and Clippy),
`2026-11-11-1351-final-core-recovery.log` (prior 34-test exact acdb0eb72 result),
and `2026-11-11-1351-current-core-gates.log` (prior recovery/projection/Clippy run).
Later S4 bootstrap/compile dependency merges are not a new executable gate pass.
The previously reported light gates passed: workspace fmt, repository size check (5,917 files),
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
