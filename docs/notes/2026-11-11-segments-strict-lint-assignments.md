# S4 strict-lint ownership and current checkpoint

Earlier assignments and accepted source evidence are preserved in
[the S4 assignment history](2026-11-12-s4-composition-assignment-history.md).

## Lead allocation 03:25

The lead transfers S5's accepted-work service/worker composition to S4 so S5 can
finish its correctness P1/P2. S5 is informed by the lead. Record this boundary
before production edits; consume S5 checkpoint `69eda007a` (or a later published
checkpoint) by normal merge first. Work stays on
`tl/cma-log-segments/writer-publication`; S2 opens the combined PR. Part of #1351.

Exact S4 composition paths under `api/crates/cma/src/`:

- `composition/capacity_recovery.rs`: construct and mount the real recovery service.
- `composition.rs` and `sandbox/composition.rs`: only recovery service/worker
  construction and scheduler registration; retain other owners' policies.
- `store/agent_capacity_recovery/mod.rs`: service/worker module and API registration.
- `store/agent_capacity_recovery/service.rs`, `accepted_service.rs`,
  `worker_store.rs`, `transactions.rs` and `worker.rs`: production accepted-work
  orchestration, fresh authorization, exact transaction-result propagation and
  worker calls. Preserve the existing native proof/acknowledgement contract.
- Focused tests in `store/agent_capacity_recovery/worker/tests.rs` and new
  `store/sandbox_mongo_integration_tests/agent_capacity_recovery_tests/composition.rs`,
  with registration in its parent `agent_capacity_recovery_tests.rs`.
- Any new bounded accepted-work scheduler is isolated under `sandbox/` and its
  exact path is appended here before editing; reuse S2's existing scheduler pin
  when available instead of duplicating it.

S4 consumes the existing `admission::persist_admission`, `authority::capture`/
`require`/`fence_parents`, `records::insert` and `status::read` through actual
callers. This makes `TransactionScope::insert_one` a production consumer; the
scope API does not gain a synthetic call or a lint suppression.

S5 retains record decoding and durable format at
`store/agent_capacity_recovery/records.rs`, `records/**`, `format2.*`, discovery
at `discovery.rs`, `discovery/**`, restart at `restart.rs`, and their existing
`agent_capacity_recovery_tests/{records,restart,discovery}/` or `.rs` tests.
S5 retains correctness of the admission/authority/preparation/settlement/receipt/
status helpers and `accepted_authority.rs`; S4 changes their callers, not their
durable or authority contracts. S5 also retains native boundaries at
`sandbox/agent_worker/{capacity_recovery,capacity_restart,recovery_commands}.rs`,
Layer2 recovery state/activation/ports, and Layer3 recovery adapters. Any needed
change crossing these paths is reported with exact names before editing.

Qualification: real service/worker success, refusal, crash and uncertain outcomes;
strict CMA Clippy with warnings denied; targeted units and real Mongo under the
existing release-only lock, two jobs, one target, memory pause below10%.
Source and actual gates are due to S2 by11:00 SGT for the16:00 segmentation merge.
All admission remains default-off and native Unsupported remains honest.


### 03:25 allocation — exact scheduler dependency

S4 reuses S2 scheduler commits `0eba12933` and `541c3f7f2` through an isolated
normal-merge dependency, without importing the mixed integration tip. Exact
additional composition paths: `sandbox/capacity_scheduler.rs`,
`sandbox/capacity_scheduler_tests.rs`, `sandbox/scheduler.rs`, `sandbox/mod.rs`,
`store/agent_capacity_recovery_presence.rs`, and its registration in `store/mod.rs`
(all under `api/crates/cma/src/`). The scheduler migration-presence check is
qualified in the new composition Mongo tests. The source branch lacks S2's
`store/migrations/tests/segments_runtime.rs`, so its single test-only assertion
is omitted from the isolated dependency. Shared schema/grants remain S2.
The scheduler observes one32-row page, runs at most2 accepted operations per turn,
and grants no admission or principal from discovery. All7 existing scheduler
unit tests must actually execute. S5's published service merge is `1a6d8401d`.

Exact new scoped test child: `store/sandbox_mongo_integration_tests/
agent_capacity_recovery_tests/composition/faults.rs`. Both this child and its
`composition.rs` parent belong to S4; the existing S5 restart/native tests remain
unchanged. Scheduler source pins are isolated as `82ecad179` and `6a0399f3a` and
normally merged; their production code and seven unit tests are unchanged.

The S4 composition handoff also owns the missing canonical configuration entry
in `docs/CMA_Retained_Recovery.md` for `CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED`.
This documents the imported default-off admission switch, accepted settlement
and unchanged native-proof boundary; it does not change deployment configuration.

Strict qualification also needs S2's published `c13eff3b5` caller delta in
`store/sandbox_unpublished_recovery.rs` (under the same crate). Consume only that
unchanged delta through a narrow normal merge; S2 retains this root's ownership.

The `service.rs::discover_agent` facade exists only for Mongo contract tests;
S4 restricts its exposure to test builds. Production restart already consumes
bounded `discovery::for_agent` directly; S5's restart file needs no caller edit.

### Qualified composition checkpoint

`fbe13913292cbf8230ab403685cd5d76d43bed64`: strict CMA has zero warnings; 107 units and 56 Mongo tests pass; fmt, the 6186-file size check, configuration coverage, 31 runner/4 parser checks and the 3875-commit source secret scan pass.
Evidence: `docs/notes/2026-11-12-s5-composition-wiring.md`; no missing interface. Shared slot 1, two jobs, minimum RAM 26%, zero pauses; owned cleanup 2.372 GB; locks retained.
Project 9 stays In Progress. S2 owns the combined PR/review/merge; activation remains closed pending separate qualification.

## P1-3 later-progress caller allocation — 05:35 SGT

Normally merged S2 proof branch `c4e4b995a` and S5 domain/restart branch
`3aaa55395` before caller edits. S4 confirms the publication invariant: shared
`Publication::new` independently binds the full next Agent; the atomic publisher
advances the head with the aggregate and immutable receipt, including empty-log
revision progress. Current-proof capture requires that exact full-Agent witness
and source identity; audit-only head changes may reuse it without granting native
permission. The current schema index identity is included in its checksum.

S4 owns `api/crates/cma/src/store/agent_capacity_recovery/service.rs`:
`activation_authorized` verifies unchanged original work, then calls
`receipts::require_current_result` and `RecoveryActivation::capture_current` in
one fresh-authority TransactionScope. The worker's second capture/equality after
native IO remains mandatory. New S4 tests live in `store/sandbox_mongo_integration_tests/
agent_capacity_recovery_tests/composition/later_progress.rs` (same crate), with
registration in `composition.rs`. They add crash-before-ack plus later progress,
original-receipt/stale-native refusal and uncertain-terminal-claim retry cases.
S5's existing `restart/later.rs` tests remain unchanged and run in the same batch.

Shared slots only; two build jobs; 60-minute lock wait; pause below 10% RAM.
Target caller SHA plus actual gates by 06:30. No missing signature; production
native Unsupported and default-off admission remain explicit. Part of #1351.
