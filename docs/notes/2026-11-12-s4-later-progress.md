# P1-3 later-progress activation caller — 2026-11-12

Part of #1351; positive publication proof also supports #1350.
Branch: `tl/cma-log-segments/writer-publication`.
Qualified executable source: `2144909cede519ed286184a7fffe1af3e645a47b`.
All required gates passed on that unchanged source. No missing interface or
ownership decision remains. S2 owns the combined PR, review and merge relay.

## Production behavior

The actual `RecoveryService::activation_authorized` caller verifies the immutable
original recovery work/receipt, invokes `receipts::require_current_result`, and
derives `RecoveryActivation::capture_current` in one fresh-authority
TransactionScope. Later progress uses its current native target and already
durable allocated fence. The worker captures and compares the complete activation
again after native IO before acknowledgement. Activation preserves the original
completion, performs no input replay, and does not allocate a new fence or repeat
the original claim attachment.

Keyed output advances sequence/head while preserving Agent revision. The consumed
S5 correction recognizes an unchanged original result only when both revision
and sequence match. Regression still refuses; every later Completed result needs
positive current publication proof. Original identity/digest/log checks and native
confirmation remain mandatory. Fresh accepted-work authorization uses the same
receipt/proof boundary before issuing its in-process authority capability.

S4 explicitly confirmed the shared publication invariant: the publisher binds the
complete next Agent independently of the domain result and atomically commits
that witness with the aggregate and advancing head, including revision-only
writes. Current proof requires the matching full-Agent witness, certified source,
database and transaction. Its hinted two-row lookup requires the newest matching
format-2 publication. An audit-only head change may retain that exact witness;
absence or revision advancement alone grants no authority.

Normally consumed S2 proof `c4e4b995a`, S5 activation/restart `3aaa55395`, S5 receipt
correction `00426512f`, complete turn fixture `2144909ce`, and develop `3aee1ff68`.
No mixed integration tip was imported. Develop merges preserve the independent
retained-context, retained-JSON, raw-output and recovery policies. Imported
configuration tests move unchanged to child modules; the parent stays 492 lines.
S5 retains receipt/domain/restart ownership; its changes were consumed normally.

## Executable scenarios

Three S4 service Mongo functions in `composition/later_progress.rs` cover:

- Actual accepted-worker durable completion followed by acknowledgement failure,
  later keyed output, store reopen and activation retry. A foreign original
  request and stale native fence refuse; another acknowledgement failure leaves
  the same state, and a later retry succeeds.
- Committed and uncommitted Unknown terminal claims after later output. Neither
  outcome performs native IO. Reopening/retrying activates the original completion.
- Exact sequence-only publication evidence: establish successful activation,
  remove/downgrade/replace the later witness with a foreign digest, assert no
  additional native IO, restore the witness and establish success again. Selection
  includes Agent/revision/sequence and differs from the original completion receipt.

The tests assert unchanged carrier, original completion receipt and publication
count during activation. The native protocol stub has no input API and panics if
completed work repeats the original claim attachment.

The three consumed S5 worker/loss-handler functions cover keyed output,
turn/input/fence progress, missing/legacy/foreign witness refusal, and publication
or parent changes during native IO. The turn fixture publishes its synthetic
OwnerMessage atomically with TurnStarted and StatusChanged before recovery runs.
Original-witness equality, stale-target refusal and no-replay assertions remain.
All six later-progress functions pass in the final actual Mongo batch.

These are store/worker protocol qualifications. Production native confirmation
remains typed Unsupported, and new admission remains default-off.

## Actual gates

| Gate on `2144909ce` | Result |
| --- | --- |
| Strict CMA Clippy, all targets/features, `-D warnings`, no allowances | Passed; zero diagnostics |
| Focused CMA/core units | 166 passed, zero failed: 117 CMA + 49 core; 4,027 outside selection |
| Real Mongo 8.0.13 replica-set contracts | 71 passed, zero failed: 40 writer/publication + 31 capacity recovery; 2,596 outside selection |
| Workspace formatting and whitespace | Passed |
| Canonical file-size check | Passed; 6,204 files checked; touched caller/receipt/test files below 500 lines |
| Configuration coverage | Passed: 143 / 185 / 12 keys for the three checked contracts |
| Full-history secret scan | Passed on source: 3,908 commits; final documentation-head scan is recorded in the issue handoff |

Unit execution took 1.463 seconds after compilation; Mongo execution took
177.547 seconds. All six positive current-publication proof cases pass. Strict,
units and Mongo each exited zero, and the API source diff remained empty before
and after execution. Machine-readable results are in
[the gate summary](2026-11-12-s4-later-progress-gates.json).

The earlier actual runs remain recorded separately:

| Source | Strict / units | Mongo | Diagnosis and correction |
| --- | --- | --- | --- |
| `2dbb5d81c` | Strict passed; 164 units passed | 66 passed / 4 failed of 70 | Equal revision did not imply unchanged sequence. Corrected the S4 expectation, consumed S5's receipt correction and fixed exact witness selection. |
| `570cb1148` | Strict passed; 166 units passed | 70 passed / 1 failed of 71 | S5's turn fixture omitted required OwnerMessage evidence. The writer correctly refused it before recovery/native IO; consumed test-only `2144909ce`. |

The 06:30 SGT checkpoint reported source and running/pending gates honestly.
Shared-slot acquisition and the final fixture correction delayed completed
qualification; all final results were confirmed by 06:54 SGT.

## Reproduction and resource protocol

Rust 1.96.1; nextest 0.9.151; two Cargo jobs; one `api/target`; incremental/debug
data disabled. Strict command:

```sh
cargo +1.96.1 clippy --locked --offline --manifest-path api/Cargo.toml   -p cma --all-targets --all-features --no-deps -- -D warnings
```

Units use `cargo nextest run --locked --offline -p cma -p layer2-core`, two test
threads, `--no-tests fail --no-fail-fast`, and this selection:

```text
test(agent_capacity_recovery::) | test(capacity_scheduler::) | test(config::) |
test(current_publication::) | test(writer_receipt) | test(receipts::tests::)
```

The canonical disposable Mongo runner is adapted only for package/filter and
fixture-name recording, with serial execution, `--run-ignored only` and selection:

```text
test(store::sandbox_mongo_integration_tests::agent_capacity_recovery_tests::) |
test(store::agent_log_segments::writer_commit::persistence::tests::)
```

Shared slot 1 retained inode 160080032. Every owned lockf used `-k -n -t 3600`;
no lock was created, deleted or recreated. The train slot was not used. Available
RAM below 10% pauses the owned gate group and 12% resumes it; the final run's
minimum was 26%, with zero pauses. A separate monitor checked lock identity.
The superseded compile-only waiter was cancelled before any Cargo/Mongo execution
when the receipt correction arrived. The final batch completed and released its slot.

All three disposable containers/volumes are independently confirmed absent.
Cleanup removed 2.451 GB of owned allocated build/tool artifacts, including the
single target, downloaded nextest archive/binary and superseded proposal. The
worktree, shared lock files and dated evidence are retained.

Local raw logs, both failed attempts, execution scripts, source pins, fixture
checks and cleanup receipt are at
`notes/2026-11-12-s4-later-progress-evidence/`.
The requested local handoff and assignments are mirrored under `docs/notes/` on
GitHub. Project 9 remains In Progress; S2 retains integration and release ownership.
