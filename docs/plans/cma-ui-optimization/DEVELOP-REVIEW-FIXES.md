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

## Spec P2 — independent shared desktop choice in Home

Current landing behavior is refined by `docs/Web_Style.md` (2026-11-06): only
Shared desktop is selectable; Agent browser remains visible only for a frozen
recorded intent. The independent-choice evidence below describes the earlier UI.

Normal task entry now includes the existing **Include a shared desktop** choice,
with the same new-Sandbox permission/context gate and Linux/Chromium meaning as
`AgentLifecycleCard`. It sends `machine_included`; ticket 16's **Enable desktop**
still sends the independent Agent-browser `desktop` flag. No legacy configuration
dialog becomes the default task entry.

The existing public contract selects the deployment machine image first, then
checks Agent-browser support against that selected image. The UI preserves typed
refusals for unsupported combinations; it does not substitute a flag or image.
No new capability endpoint or backend creation contract was invented.

Both choices persist with the unsent draft. Submitted/uncertain requests retain
their exact body and key through refresh, connection cancellation and recovery.
Malformed stored machine choices fail closed. A definite 400/422 rejection before
admission retains editable draft choices; a later explicit submission has a new
key, as required by the existing lifecycle contract. Machine creation also carries
the original desktop transport preparation hint into the Agent handoff.

Red: the Home tests could not find the shared desktop checkbox. Green: all four
option combinations preserve the exact task, fields and key across draft refresh
and lost-receipt recovery. Additional coverage includes unavailable Agent-browser
capability, forbidden/contextual machine creation, clearing an unsupported draft,
typed combination refusal, malformed saved input, and OAuth cancellation with
repository plus both desktop options. The existing CMA profile test verifies the
machine image/marker and exact first task with `desktop` both false and true.

Real Chrome checks at 1440px and 320px pass against the built app and public HTTP
fixtures. They cover default-off independent controls, 44px labels within the
viewport, no configuration dialog, no effect before submission, draft refresh and
frozen original recovery. Screenshots were inspected and retained outside Git in
`/Users/liuyitian/code/cma-ui-worktrees/evidence/develop-review-fixes/`.

## Validation before the parent integration update

- Full Web: 368 files / 3,277 tests passed with two workers. An unrelated Event
  Gateway test exceeded its existing five-second timeout in the first heavily
  parallel run; it passed unchanged alone and in the complete two-worker run.
- Web TypeScript, E2E TypeScript, ESLint, Prettier and production build passed.
- Real Chrome Home checks: 2/2 passed.
- Runtime: 334 passed, 3 existing ignored cases. The new regression is not ignored.
- CMA machine/profile test: passed with both independent option combinations.
- Core: the focused delivered-queue regression passed; full Core runs both at
  default concurrency and with two threads passed 1,264 tests and failed the
  unchanged `installed_skill_version_change_is_reported_as_unsupported` fixture
  at its `create_agent(...).unwrap()` with Conflict. That unrelated fixture and
  its assertions were not modified.
- Rust formatting, production-source size limits and `git diff --check` passed.

Check logs are outside Git under
`/Users/liuyitian/code/cma-ui-worktrees/review-fixes-*.log`. No dependency, CI gate,
timeout or ignored-test policy changed. The parent owns CI and its separate
RuntimeStatus compatibility correction; its final commit must be merged before
handoff and checked with these fixes.

## Retained admission compatibility and parent integration

Parent tip `240e4211f1fe81d733a4a6dc5cc760963162ccdc` was merged without conflicts
in `bc50252a7`. Its RuntimeStatus negotiation files remain parent-owned.

A final red/green check covers a pre-fix recovery admission whose preserved
checkout already records A. The first pin fix rejected this retained record
because its old `original.json` has no source field. Recovery now validates A in
the preserved directory and binds it in separate append-only `source-commit.json`
evidence. It never rewrites the old admission or journal. The regression moves
the remote ref to B, retries the original operation, receives A, replays the same
receipt and rejects later changes to that retained pin.

After the parent merge and this compatibility correction, the complete runtime
suite passes 335 tests with its same three existing ignored cases; the separate
pool-assignment CLI integration also passes. All 12 workspace recovery checks,
including the non-ignored interrupted-snapshot and legacy-admission regressions,
run within that suite.

Final workspace `cargo +1.96.1 clippy --all-targets --all-features --locked -- -D
warnings` passes after the integration merge and legacy-pin correction. Formatting,
source sizes and whitespace checks also pass. The only observed unresolved test
failure remains the unchanged Core live-skill fixture described above.

## Final CI integration corrections

The initial PR run passed 7,145 of 7,147 Backend tests. Its two failures were the
stored-skill fixture above and the shared public-contract runner. The Core fixture
now starts from a validated, already-stored Agent snapshot through a test-only
constructor. It does not admit pre-resolved material through `create_agent` or
change any production store guard. An unpinned profile selection ensures the
unchanged assertion specifically proves the resolved installed version. All five
live-skill tests pass.

The strict public-contract runner now passes all 106 non-auth corpus files. Exact
event expectations include the task's prompt-preview metadata event, its owner-only
content, subsequent cursors, AG-UI completion/failure/replay frames and control
receipts. Shared readers still receive a null event kind for private metadata.
The binding-version scenario creates its initial profile through a real PUT using
the same scenario credential before requesting the unsupported upgrade. A test
router also preserves deliberately missing command dependencies without unwrapping
the optional v2 composition. Existing error, privacy, pagination, replay and frame
count assertions remain strict; temporary diagnostics were removed.

The corrected corpus passes 3,135 targeted TypeScript SDK tests and the Rust SDK's
shared-corpus, response-control, interruption and event-reader checks (20 tests).
The Rust event-reader fixture expects 15 entries and still rejects a mismatched
next-page boundary. Formatting and source-size checks pass. No CI gate, timeout,
test ignore or product behavior changed in these fixture corrections.

Runtime status compatibility is independently verified on the combined branch:
only the new browser capability requires the computer-status Accept opt-in;
established filesystem/upload advertisement remains identical to develop. Both
original review axes and the follow-up compatibility finding are closed. PR #1311
remains the authority for final-head CI and merge state. Human suggestion-quality
acceptance and any deployment remain separate.

## Concurrent skill installation contract coverage

The next PR run passed 7,150 of 7,151 Backend tests. Its only failure was the
coverage inventory: correcting the installed-version refusal had removed the old,
incorrect example of `agent_operation_in_flight` from the skill-install operation.
The corrected `agent_skill_version_upgrade_unsupported` case remains unchanged.

Two additional shared cases cover a reachable admission race. A test-only runtime
support adapter admits `research@1.0` through the real `AgentOperationService`
between the REST preflight and the incoming `research@2.0` admission. The latter
receives the exact HTTP 409 `agent_operation_in_flight` problem. A subsequent GET
proves that the original `research@1.0` is still pending and was not replaced or
settled by the losing request. All 51 existing cases in that fixture are unchanged.

The inventory and new scenario each failed before the correction and passed after
it. The complete strict runner passes 3,014 cases. The serial public-contract suite
passes 37 tests with its two existing ignored tests; an earlier parallel run's six
failures remain recorded separately. Formatting and scoped Clippy also pass.
The correction changes only shared test fixtures and their test-only setup; the
existing inventory requirements, production admission behavior and CI gates remain
intact. Worker commit: `1172c941e`; independent integration merge: `eae5bcf3f`.

The merged tree also passes all 3,060 TypeScript SDK public-contract tests and the
Rust SDK shared-corpus test. Independent Standards and Spec reviews of the frozen
worker commit both report zero findings. Final CI and merge status remain subject
to PR #1311's checks on the pushed candidate.

## Mongo persistence contract integration

The subsequent PR run passes all 7,151 normal Backend tests and all 14 other jobs.
Its disposable Mongo gate runs 454 tests: 452 pass, while two reveal test-boundary
drift. The five-case real-model sample proof requires an explicitly authorized
local proxy, model and evidence destination, but the generic Mongo selector had
included it. The hierarchy snapshot test also still expected the command count
from before task-status projection added its two reads.

The default-off `real-model-suggestion-samples` Cargo feature now gates only the
existing external sample module beneath `cfg(test)`. Its implementation, ignored
attribute, real generation and strict missing-configuration failure are unchanged.
Default test inventory removes exactly that one proof; all five deterministic
suggestion HTTP/persistence/lifecycle tests remain selected. Explicit opt-in restores
the full inventory, and missing sample configuration still fails before database
or model access. The authorized invocation is recorded in `13-EVIDENCE.md`.

The hierarchy regression now checks exact ordered collection reads, filters,
sorting, limits and timeouts; one snapshot/session/transaction through abort;
read-only behavior; the actual task status; and preserved lineage, logs and command
state. Production hierarchy and task-status code remain unchanged. Both original
failures were reproduced before correction on a fresh, owned Mongo 8.0.13 replica
set. Three hierarchy and five deterministic suggestion tests then pass on macOS
arm64; this is separate from the required Linux CI result.

Formatting, scoped all-feature Clippy and the workspace doctest command pass.
The doctest command finds no executable documentation examples. Independent
Standards and Spec reviews report zero findings. Worker commit `c194357e5` and
independent merge `7ac951aaf` have identical trees. The temporary Mongo process
was shut down successfully after tests; no external model calls were made. CI
scripts, filters, production behavior and the human acceptance boundary are
unchanged. Final CI and merge status remain with PR #1311.
