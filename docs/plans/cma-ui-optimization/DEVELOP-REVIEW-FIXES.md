# Develop integration review corrections — 2026-11-04

Baseline: `6de11fe2ee8eb5aa53e4e3730f3afb3ef07b6d23`.
Scope: the one Standards P2 and two Spec P2 findings in the independent develop
integration reviews. The original dirty checkout is outside this worktree.

## Standards P2 — retained delivered queue history

`AgentTaskStatus` counts only messages whose state is not `Delivered` as pending
queue work. Existing Agent status, active-turn, operation-delivery, first-message
and admitted-work barriers remain in place.

The existing lifecycle queue regression drives an ambiguous write through the
durable turn recorder, reads the retained Delivered entry and completed history,
and projects Completed. The same evidence with an active turn, pending admission,
or any Queued/DispatchFenced/RecoveryRequired entry remains Working.

Red: `lifecycle_queue_ambiguous_write_waits_for_execution_evidence` returned Working
where Completed was expected. Green: the same test passes after narrowing the
queue predicate. No fixture history or production admission gate was weakened.

Rust checks use Cargo `+1.96.1`, `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`, and
`RUST_MIN_STACK=33554432`, with the coordinated shared target directory.
