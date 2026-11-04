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

## Spec P2 — retain repository snapshot identity during recovery

Recovery records the original source commit with its preserved directory
identity. Snapshot replacement downloads that exact commit without resolving the
mutable ref again. Candidate inspection, publication and completed replay compare
against the recorded original pin. Missing, malformed, oversized or unsafe pin
evidence is a typed `workspace_recovery_ineligible` refusal. Legacy recovery
records without a provable snapshot pin are also refused; Init/Clone recovery
retains its existing behavior and persisted shape.

The original directory, index lock and journal remain intact. Failed candidates
remain retained under their attempt. Publication keeps the existing directory
identity, atomic rename, explicit attempt-chain and writer-fencing checks. The
publication code moves to its own module to keep production sources below 500
lines.

Red: an actual snapshot preparation is cancelled immediately after writing
`.git/info/cma-source-commit` A and before publishing `cma-workspace-id`. Moving
the GitHub fixture's mutable ref to B made recovery return B. Green: the same
test returns A and A's known file content, makes no commit-ref lookup during
recovery, and replays the same receipt. The test uses a Unix-socket HTTP boundary
and real native Git/filesystem operations; it does not claim a new live GitHub or
Linux deployment proof.

All 11 `workspace_prepare_recovery` checks pass, including explicit HTTP recovery,
stale-boot fencing, failed-attempt preservation, publication/restart replay,
unsafe pin refusal and rejection of an interrupted candidate pinned to B.
