# S4-B1 ordinary v2 publication delta review — 2026-11-12

VERDICT S4-B1: go, 0 blockers at `72ad199417f7d9114a85d65165921101631069da`.

The prior S4-B1 finding is closed by this slice: adopted Agents now publish ordinary conversation output and checkpoints through the segmented transaction-scoped writer, and ordinary command settlement consumes bounded authenticated original evidence. This is a delta verdict for the S4-B1 fix, not a verdict on the eventual combined integration head or production activation.

## Scope and provenance

Reviewed branch `tl/cma-log-segments/ordinary-v2-publication` at the exact requested head. It branches from qualified `5875b8ddf16cbbd245145d1d3033d5467c2f3a44` and includes a normal develop merge; the earlier reviewed integration head `26f987d001db450f8dab727c1ef481c398cedbe6` is not its ancestor. The review traces the reported missing callers and their narrow command-evidence dependency. Differences in independently developed reader/consumer/S5 files are not treated as intentional deletions from the eventual combined branch.

The reviewed API tree is `9dd6a4a1d97182d140dd05ac79ea64078bef1ec8`, identical to tested executable commit `6b3c025dbc6af5abc22ea663d8dafc75c8cb9c0e`. The final handoff changes documentation only. Exact evidence hashes and scoped source hashes are retained in the adjacent evidence directory.

## Findings

No blocking finding remains in this delta.

- `api/crates/cma/src/store/agent_log_segments/conversation.rs:28` dispatches legacy roots to the legacy implementation and segmented roots to captured publication within the root-owned transaction. The new path retains mutation validation, parent/admission, cleanup, budget and command binding, carries private checkpoint fields, publishes the Agent/log/head/projection/receipt atomically and enqueues suggestions in the same transaction. No whole-history fallback is used for segmented roots.
- `api/crates/cma/src/store/agent_conversation_store.rs:84` starts the exclusive transaction scope for ordinary publication; the root alone commits or aborts. Its uncertain-commit path checks the original recorded result and command-dispatch receipt. `agent_log_segments/writer_reconcile.rs:45` adds a transaction-mode reader without changing the original snapshot-mode identity, fingerprint, result digest, sequence interval, current lineage and original-entry checks. Missing evidence remains Unknown. Later head progress does not manufacture a new result or append.
- `agent_log_segments/conversation/checkpoint.rs:9` preserves checkpoint revision/generation/previous-value/reopen/capacity fences. `agent_conversation_store/checkpoint.rs:241` validates the actual source envelope before a format-aware checkpoint CAS, removing the old schema-v1-only refusal. Exact checkpoint replay with a publication requires its original segmented receipt (`conversation.rs:158`); missing or foreign receipts refuse. Quiet private assembly and the publication boundary remain atomic with their checkpoint.
- `api/crates/cma/src/store/agent_commands/reconcile.rs:45` first requests the format-aware evidence reader; only a proven legacy root returns None and selects the old fold. The existing continuation-authority fence and command/lane write remain. `agent_log_segments/command_evidence.rs:20` uses the current head/projection, indexed turn and operation facts, bounded authenticated original points and one pending-approval witness. Unknown or corrupt proof cannot fall back to a lifetime scan or release command custody.
- `api/crates/layer2-core/src/sandbox/agent_io/evidence/certified.rs:28` validates original turn/generation/operation/delivery/terminal/approval facts, deferred-failure adjacency and original timestamps. The sparse result is sealed against reuse as an ordinary continuing fold. Success/control/deferred-refusal/approval/failure precedence is compared with the full fold in the targeted tests, including history beyond both old limits.
- Rollover still uses `writer_existing.rs:70` admission in the same transaction and the existing exact source/head CAS. This delta adds no runtime reconnect or native input submission. Native Unsupported behavior and the S5 recovery execution contract are not changed by this fix.

## Regression and validation evidence

The seven new real-Mongo tests use genuine gated adoption, discovery and ordinary claims/reservation. They exercise runtime output through recorder replacement with one complete published message, private-prefix retention, terminal settlement and zero lifetime command scans; abort and wrong-boundary rollback; foreign/missing receipt refusal; committed and uncommitted uncertainty; and restart with unchanged original input/history. The settlement restart fixture kills only its captured disposable test session, verifies durable state is unchanged by that cleanup, then settles against original output. It does not alter production retry behavior.

Independent reviewer validation at the requested source:

- `cargo test -p layer2-core --lib command_evidence_tests -- --test-threads=2`: 17 passed, zero failed/ignored, including all six certified-evidence cases. Executed under shared `lockf -k` with two Cargo jobs and one reused target.
- `cargo fmt --all -- --check`: passed.
- Canonical `.github/scripts/check-file-size.sh`: passed, 6,410 files.
- Additional inspection of 27 directly scoped source/test files: maximum 459 lines, all below 500.

Author evidence independently bound to the identical API tree: 50/50 focused units and 52/52 real-Mongo contracts, strict CMA/core all-target/all-feature Clippy with zero diagnostics, formatting, file size and secret-history scan. The successful Mongo log summary is retained by hash; earlier partial failures are superseded by the final executable run. The reviewer did not run a second Mongo container or claim those 52 tests as reviewer execution.

Non-blocking integration condition: the new required approval-change fact must be included in S1's projection rebuild before activation of older incomplete segmented projection shapes, as stated in the handoff. Preserve the later reader/consumer/S5 changes when normally integrating this branch, and qualify the resulting combined head. No CI wait, merge or activation approval is implied by this delta verdict.

## Deliverables and cleanup

Evidence: `notes/reviews/2026-11-12-segmentation-s4-b1-delta/`. The exact source export is removed after review; hashes, local test log, validation results and the author-evidence binding remain with 0700/0600 permissions. No production resources, author worktrees, containers or volumes were changed. One reusable `api/target` is retained for the requested combined-head follow-up; no per-gate target was created. Shared lock files remain untouched. Own branch remains `tl/cma-ledger-recovery/maintenance-recovery` at `3bd324f20935347f62e1836ca0068cddc15a309c`; no commit, push or PR was created. Track C remains paused.
