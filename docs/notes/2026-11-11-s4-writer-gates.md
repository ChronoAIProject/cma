# S4 local gate evidence — 2026-11-11

Branch: `tl/cma-log-segments/writer-publication`. Part of #1350 and #1349.
This records actual local execution, not a review-ready or integration pass.

## Environment and lock

All Cargo builds/checks/tests were launched through `/usr/bin/lockf` on the
assigned `/Users/chronoai/code/work/.tl-worktrees/.heavy-0.lock`. The commands
incorrectly omitted `-k`: on macOS the utility removes the lock path on exit.
This could contribute to the shared lock removal reported by the lead and does
not establish continuous shared serialization across the team's recreated paths.
The test results below are actual results; lock-file lifecycle compliance was
not met. No S4 build or lock waiter remains.

Correction for every future gate: `/usr/bin/lockf -k -n ...`. `-k` preserves the
existing file, and `-n` refuses to create a missing one. Never delete, recreate,
truncate or replace either shared heavy lock, including during cleanup. If a
lock is missing, report it to the lead. This correction was relayed immediately
when the platform behavior was confirmed with the system manual.

Build environment:

```text
CARGO_BUILD_JOBS=2
CARGO_TARGET_DIR=/Users/chronoai/code/work/.tl-worktrees/cma-cma-seg-lead/api/target
CARGO_PROFILE_DEV_DEBUG=0
CARGO_PROFILE_TEST_DEBUG=0
CARGO_INCREMENTAL=0
RUST_MIN_STACK=33554432
```

Disk admission required at least 35 GiB before starting, above the 30 GiB team
floor. Readbacks during execution remained above the floor. One target was reused
between gates. The pinned Rust toolchain is 1.96.1. No Mongo container was started.
The available nextest is 0.9.143, older than CI's pinned 0.9.151; these core gates
used Cargo's unit-test runner and do not claim canonical nextest/Mongo execution.

## Results

| Gate | Actual result | Scope / limitation |
| --- | --- | --- |
| `cargo check --locked --offline --manifest-path api/Cargo.toml -p cma --tests` | Failed: 14 diagnostics | Includes merged S1/S4 registration and the first staging source. S4 fixed its unwrap/Debug handling and BSON Result comparison. S1 retention borrow, TransactionLogError Debug and schema index-name inference remain owner fixes. This is not a full CMA compile pass. |
| `cargo test --locked --offline --manifest-path api/Cargo.toml -p layer2-core --lib agent_log_projection -- --test-threads=2` | Passed: 5 tests | Final projection source in `8a9fbba2d`; long-turn, original model/native facts, late usage, generation maxima and queued-source coverage. |
| `cargo clippy --locked --offline --manifest-path api/Cargo.toml -p layer2-core --all-targets -- -D warnings` | Passed | Same final core source. First run found a collapsible creation match, repaired before the successful rerun. Does not certify CMA or the entire workspace. |
| Workspace `cargo fmt --all --manifest-path api/Cargo.toml -- --check` | Passed | Registered modules include current writer sources and tests. |
| `git diff --check` | Passed | Current owned changes. |
| Staged gitleaks | Passed | Each new source checkpoint. Final history scan and repository size check are recorded in the milestone issue comment. |
| Real-Mongo, unknown-commit failpoints, old-binary compatibility, all writer callers | Not run / incomplete | CMA compile blockers and executable source/consumer integration remain. No PR is ready. |

## Owner relay and source follow-up

S1 fixes requested through the lead:

- `agent_log_segments/retention.rs:28/60`, E0499. Drop the temporary cursor
  stream before the loop body performs another query using the same session.
- S1 capture/snapshot tests use unwrap on TransactionLogError, which lacks Debug.
- `agent_log_segments/mongo_tests/schema_tests.rs:24`: replace the ambiguous
  `.name("agent_log_segment_start".into())` with an explicit String.

S4 scope methods now use the integration-approved private transaction stamp plus
database binding. The source capture validator still refuses transaction use;
no caller can fabricate a successful prepared proof. The scoped lifecycle-root
handoff posted in #1348 comment-6794825157 is acknowledged: S4 owns the lifecycle
root and awaits S5's exact scoped checkpoint validator port. No adopt/attach or
public raw-session accessor was added.

## Cleanup

First cleanup: 5,604 bytes of completed comment drafts, reported `CLEANED 0.00 GB`.
Final Cargo cleanup: deleted only this worktree's `api/target`, freeing 1.32 GB
(1.23 GiB), reported `CLEANED 1.32 GB`. The 64 GiB volume readback after cleanup
includes other team cleanup and is not attributed entirely to S4. No owned Cargo
process, per-gate output copy, Mongo container or volume remains. Requested local
notes and the dedicated worktree are retained for review.
