# S4 accepted-work composition handoff

Source: S4 published `08b947f93`; exact ownership and qualification below.

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
