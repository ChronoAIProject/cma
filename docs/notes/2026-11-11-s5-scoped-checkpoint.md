# S5 scoped checkpoint — 2026-11-11

Branch: tl/cma-log-segments/capacity-recovery. Source head before evidence commit:
7b1391429. Develop merge freeze is lifted; no S5 review/merge relay exists.

Normally merged S1/dependency 29f8789f6 and S4 dcaf3cf0f. S4 overlap resolution
is copied exactly from the integration owner's 8c1c14cba for the seven conflicting
or scope-dependent writer/projection files. The mixed integration tip was not merged.

34 registered core recovery tests passed at a4c625c22 under heavy-0, two jobs,
one target (test profile completed in 1m47s). This supersedes the earlier
UsageFold compile-blocked status. A repeat after the S4 projection merge plus
projection tests and warnings-denied core Clippy is queued on heavy-0. Results
will be recorded in 2026-11-11-1351-current-core-gates.log.

c3a4c1d2f adapts admission, authority, work and status helpers to exclusive
TransactionScope. Source has scoped capture/fence helpers. Existing legacy
raw-session consumers still work; segmented raw-session paths refuse without
falling back. The scoped reconciliation publisher is a focused child module;
its caller/root must propagate the exclusive scope. New Mongo regression source
checks aborted scoped fences and stale aggregate refusal. These Mongo tests
are not yet registered or run and no publication success claim is made.

## Concrete remaining integration work

- S2 store/test/worker config registrations and work-carrier migration/schema
  remain requested. No registration patch is being made in another owner's files.
- Existing checkpoint/lifecycle/maintenance/reconciliation roots need the scope
  at transaction start; neither adopting a running transaction nor exposing a
  raw session is compatible with the amended contract.
- S1 transaction capture/certification still refuses. S4 prepare remains closed
  through that certification. Recovery admission for an already-segmented Agent
  also currently conflicts in writer_prepare's segmented AdmissionKind check,
  which accepts only Rollover. S4 must define the CapacityRecovery empty-batch
  successor case while retaining both independent gates.
- Production accepted-work claim/complete/retry adapter, exact publication
  reconciliation, current authorized HTTP dry-run/status and restricted native
  reconnect/activation remain S5 work. No ordinary PublishConnection fallback:
  it can replay context/settings or schedule queued first input after a crash.
- Full cma lint/Mongo, capacity boundary/no-replay/compatibility and remaining
  release gates remain mandatory before the planned S5 PR.

No disposable Mongo container/volume is owned by S5. Previous cleanup freed
0.71 GB; one reusable target currently exists for the queued core gates. Future
lockf invocations use -k to keep the shared lock inode persistent across waiters.
