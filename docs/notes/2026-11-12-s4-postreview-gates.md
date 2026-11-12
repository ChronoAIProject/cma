# S4 post-review gate evidence

Implementation pin: `5b49c51d6e44f509d937759a4e67c8dfca9ccf8b`. Part of #1350.

The lead authorized the optional-generation compatibility fix. All gates below ran with Rust1.96.1, nextest0.9.151, two Cargo jobs, one target, and the existing release-only lock. Available memory stayed at least29%; no pause. Each owned Mongo fixture and volume cleaned up before the next group.

Batch started 2026-11-12T01:22:37.747414; supplementary original unpublished-recovery coverage finished 2026-11-12T01:38:55.104459. The 01:30 handoff was late; at that checkpoint the unit binary was still rebuilding.

| Gate | Result | Gate wall seconds |
| --- | --- | --- |
| units | Summary [   0.433s] 40 tests run: 40 passed, 2474 skipped | 449.595 |
| writer-mongo | Summary [ 116.542s] 36 tests run: 36 passed, 2478 skipped | 138.514 |
| legacy-mongo | Summary [  47.624s] 23 tests run: 23 passed, 2491 skipped | 63.225 |
| retained-mongo | Summary [  38.974s] 9 tests run: 9 passed, 2505 skipped | 45.174 |
| strict | Exit101:22 distinct diagnostics;21 dead code,1 unused import;zero type errors | 178.09 |
| unpublished-mongo | Summary [  91.898s] 8 tests run: 8 passed (1 slow), 2506 skipped | 102.388 |

Formatting and whitespace passed. Source-size:6055files passed; the corrected shared file is350lines. Canonical source-pin history scan:3772commits,zero findings.

## Previously failing writer cases

All five previous writer failures passed in the corrected36-test group. The publication fixture tests real commit/abort/unknown and foreign receipts; it does not exercise all original discovery/admission boundaries.
```text
PASS [   3.693s] ( 5/36) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::bootstrap::bootstrap_rejects_replaced_permit_and_changed_original_request
PASS [   2.201s] ( 6/36) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::bootstrap::bootstrap_stages_multiple_fenced_batches_without_exposing_or_rewriting_source
PASS [   5.664s] ( 7/36) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::bootstrap::staged_bootstrap_is_published_only_against_the_same_recaptured_source
PASS [   4.047s] (16/36) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::keyed_restart::unknown_without_receipt_survives_restart_and_later_head_without_replan
PASS [   7.208s] (19/36) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::lifecycle_caller::lifecycle_reconciliation_requires_original_parent_events_and_child_fences
PASS [   4.371s] (20/36) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::lifecycle_caller::lifecycle_root_aborts_segmented_child_fence_after_later_domain_refusal
PASS [   2.676s] (21/36) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::lifecycle_caller::lifecycle_root_captures_segmented_children_and_preserves_quiet_boundary
PASS [   2.296s] (22/36) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::lifecycle_caller::lifecycle_root_reconciles_uncertain_checkpoint_without_repeating_fences
PASS [   3.930s] (23/36) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::lifecycle_child::lifecycle_child_success_uncertainty_abort_and_foreign_receipt_are_exact
PASS [   2.964s] (24/36) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::original_callers::control_root_updates_segmented_head_and_exact_original_receipt
PASS [   1.955s] (25/36) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::original_callers::worker_root_settles_segmented_publication_and_aborts_later_domain_refusal
PASS [   2.582s] (29/36) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::uncertain_commit::unknown_commit_is_reconciled_from_original_receipt_after_later_head_advance
PASS [   1.761s] (34/36) cma::bin/cma store::sandbox_mongo_integration_tests::agent_segment_tests::writer_gate_tests::slot_tests::ahead_client_clock_cannot_extend_preparation_authority
PASS [   5.587s] (35/36) cma::bin/cma store::sandbox_mongo_integration_tests::agent_segment_tests::writer_gate_tests::slot_tests::expired_preparation_and_timestamp_drift_never_authorize_publication
PASS [   1.726s] (36/36) cma::bin/cma store::sandbox_mongo_integration_tests::agent_segment_tests::writer_gate_tests::slot_tests::preparation_slots_bound_cross_replica_work_and_fence_reclaimed_holders
```

## Strict diagnostic inventory

No warning suppression was added. No S4 writer reconciliation diagnostic remains. S5 owns18 recovery locations plus TransactionScope::insert_one; S2 owns the three reconciliation locations.

- `crates/cma/src/store/sandbox_reconciliation/transaction.rs:21:16` — unused import: `scoped::apply_scoped`.
- `crates/cma/src/store/agent_capacity_recovery/admission.rs:26:29` — struct `AdmissionRequest` is never constructed.
- `crates/cma/src/store/agent_capacity_recovery/admission.rs:34:29` — struct `AdmittedRecovery` is never constructed.
- `crates/cma/src/store/agent_capacity_recovery/admission.rs:43:31` — function `persist_admission` is never used.
- `crates/cma/src/store/agent_capacity_recovery/authority.rs:22:19` — struct `Authority` is never constructed.
- `crates/cma/src/store/agent_capacity_recovery/authority.rs:27:19` — method `context` is never used.
- `crates/cma/src/store/agent_capacity_recovery/authority.rs:44:21` — function `capture` is never used.
- `crates/cma/src/store/agent_capacity_recovery/authority.rs:134:15` — function `require` is never used.
- `crates/cma/src/store/agent_capacity_recovery/authority.rs:149:21` — function `fence_parents` is never used.
- `crates/cma/src/store/agent_capacity_recovery/records.rs:17:28` — constant `COLLECTION` is never used.
- `crates/cma/src/store/agent_capacity_recovery/records.rs:21:8` — struct `Row` is never constructed.
- `crates/cma/src/store/agent_capacity_recovery/records.rs:29:21` — function `read` is never used.
- `crates/cma/src/store/agent_capacity_recovery/records.rs:41:4` — function `decode` is never used.
- `crates/cma/src/store/agent_capacity_recovery/records.rs:60:15` — function `collection` is never used.
- `crates/cma/src/store/agent_capacity_recovery/records.rs:67:21` — function `insert` is never used.
- `crates/cma/src/store/agent_capacity_recovery/records.rs:95:4` — function `invalid` is never used.
- `crates/cma/src/store/agent_capacity_recovery/source.rs:59:31` — function `capture_segmented_scoped` is never used.
- `crates/cma/src/store/agent_capacity_recovery/status.rs:18:19` — struct `RecoveryStatus` is never constructed.
- `crates/cma/src/store/agent_capacity_recovery/status.rs:31:31` — function `read` is never used.
- `crates/cma/src/store/agent_log_segments/transaction_scope.rs:90:35` — method `insert_one` is never used.
- `crates/cma/src/store/sandbox_reconciliation/agents/scoped.rs:24:55` — function `publish_agent` is never used.
- `crates/cma/src/store/sandbox_reconciliation/transaction/scoped.rs:9:55` — function `apply_scoped` is never used.

## Scope and remaining dependencies

P2-5 source correction has passing restart/absence/later-head/foreign-fingerprint and positive KnownAbort Mongo evidence at this pin. P1-1 stays OPEN: adoption writes v2 but discovery, claims and reservation still select v1; targeted approval still uses the legacy scanner. Required adoption-to-discovery-to-claim-to-command-to-output regression remains absent. Activation stays closed.

The shared retained-reopen root explicitly refuses segmented data until its original bounded history proof exists. This batch does not claim that missing proof or admitted segmented CapacityRecovery successor support. Shared strict composition remains required for sandbox_reconciliation::transaction::apply_scoped and agent_capacity_recovery::{admission,authority,records,status} through TransactionScope::insert_one.

## Reproduction

Environment: RUSTUP_TOOLCHAIN=1.96.1,CARGO_BUILD_JOBS=2,CARGO_PROFILE_DEV_DEBUG=0,CARGO_PROFILE_TEST_DEBUG=0,CARGO_INCREMENTAL=0,RUST_MIN_STACK=33554432. One target at this worktree api/target.
```sh
cargo nextest run --locked --offline --manifest-path api/Cargo.toml -p cma --bin cma --all-features -E "test(store::agent_log_segments::)" --test-threads 2 --no-fail-fast
cargo clippy --locked --offline --manifest-path api/Cargo.toml -p cma --all-targets --all-features --no-deps -- -D warnings
```

The canonical Mongo owned-fixture script was copied locally with only its Cargo build and test selection narrowed. Mongo8.0.13 pinned digest, replica-set/failpoints, CPU/RAM/cache limits, serial test execution and ownership-checked cleanup were preserved. Writer selection:writer_commit::persistence::tests plus writer_gate_tests::slot_tests. Legacy selection:prepared worker replay/open-ready, control, worker receipt/sign-in and lifecycle commit. Retained selection:retained_reopen plus unpublished retained modules. Supplement:all original unpublished-recovery tests except retained. Existing slow/unknown test completed81.191s; no test timeout/assertion was shortened.

## Log checksums

- `s4-final-units.log`: `0bce05facf2c1934231c8970b041010a158423d5e885854eedabced21badef9f`.
- `s4-final-writer-mongo.log`: `b387c896c53299b693eaf1d307f57d5edcfae3ffaf282667cb8e5d3cfac788d7`.
- `s4-final-legacy-mongo.log`: `e795de9613959a8a496ead608f2d54ebe25793f85d64613994e7429abb299f17`.
- `s4-final-retained-mongo.log`: `8d712e981d36a685bc4040b5892a0b53b3d049273ad57242b052b2911771fc45`.
- `s4-final-strict.log`: `2f351eed203581b59892bbb726d82e14adb4b4995cdd5029f76f1743f76454c7`.
- `s4-final-unpublished-mongo.log`: `96186d93ef8265122a6f90ef749feecf962c506a178db4a0fbf9a462039c9e9a`.

Raw temporary logs are removed after evidence extraction and final rerun cleanup.

## Requalification after develop92d18a909

Exact source `e094ffeea60b1aaf33510714e08795325acc8bff`. Rust1.96.1; release-only lock;2jobs;memory minimum25%;zero pauses. Completed 2026-11-12T01:54:59.187070.

| Gate | Result | Wall seconds |
| --- | --- | --- |
| units | Summary [   0.433s] 41 tests run: 41 passed, 2491 skipped | 319.482 |
| writer-mongo | Summary [ 143.027s] 36 tests run: 36 passed, 2496 skipped | 156.546 |
| legacy-mongo | Summary [  38.029s] 23 tests run: 23 passed, 2509 skipped | 48.305 |
| retained-mongo | Summary [  31.224s] 9 tests run: 9 passed, 2523 skipped | 39.163 |
| unpublished-mongo | Summary [ 106.626s] 8 tests run: 8 passed (1 slow), 2524 skipped | 119.244 |
| strict | Exit101;22distinct shared diagnostics;zero type errors | 148.799 |
| feature-units | Summary [   0.071s] 7 tests run: 7 passed, 2525 skipped | 3.021 |

Unit selections overlap by one bounded-title flag test:47distinct unit/configuration tests plus76real-Mongo tests passed,123distinct passes. All owned fixtures and volumes removed. No S4 receipt diagnostic; the same19S5+3S2 shared reachability errors remain. Formatting and6070-file size passed.

Log hashes before cleanup:
- `s4-final-units.log`: `8a9f644023c09b449bdec364882e1fda5f2ff8b70509e32d2342602b62913ebb`.
- `s4-final-writer-mongo.log`: `cb91313e1c44ce39ce6f98edfe5f359a9c7227f047e2474316caf91914c4114c`.
- `s4-final-legacy-mongo.log`: `519e532894ac053a4300c1789f2989ee8cc8038651b7a78752d75f9e9a48cb9e`.
- `s4-final-retained-mongo.log`: `319a9b33d8f7c757db776cff10b455fa246fc475ad31b0391d4fb7a0e1ab3038`.
- `s4-final-unpublished-mongo.log`: `41dac89a97813948fce9f23169f3a8a2b05148f09e0f6b64b4cdefa3c8e2ffc7`.
- `s4-final-strict.log`: `c496e968b0ed2db8b621afe634f0ed18579879431a00034d07605228040c1413`.
- `s4-final-feature-units.log`: `77dd134bfa256d88b43cf306a920091227686a1e9a91f662e44975ef6c443ded`.

## Final qualification at bc075a36aa45ac8fc826d4999748a9bcc8071c6d

Completed 2026-11-12T02:21:33.314782 SGT. Rust 1.96.1 and nextest 0.9.151; existing release-only lock, two jobs, one target. Available memory minimum 25%; 0 pauses. All four owned Mongo fixtures and volumes were cleaned before the next phase.

| Gate | Exact result | Gate wall seconds |
| --- | --- | --- |
| units | Summary [   0.848s] 47 tests run: 47 passed, 2486 skipped | 477.647 |
| writer-mongo | Summary [ 150.953s] 37 tests run: 37 passed, 2496 skipped | 162.543 |
| legacy-mongo | Summary [  53.309s] 23 tests run: 23 passed, 2510 skipped | 60.344 |
| retained-mongo | Summary [  34.233s] 9 tests run: 9 passed, 2524 skipped | 42.094 |
| unpublished-mongo | Summary [  93.607s] 8 tests run: 8 passed (1 slow), 2525 skipped | 99.336 |
| strict | Exit 101; 22 distinct shared reachability diagnostics, zero type errors | 159.865 |

Total: **124 selected tests passed, zero failures** (47 units/configuration and 77 real Mongo). The new response-facts commit/abort regression passed, alongside all original caller, retry, absence, foreign receipt, rollover and lease cases. P2-5 has passing corrected Mongo evidence for closure review. P1-1 remains open; activation is closed.

Formatting and whitespace passed. Source-size gate passed 6,084 files; every S4 changed file is below 500 lines. Source-pin secret history passed 3,792 commits with zero findings. The separate narrow response-facts dependency passed 15 projection tests and strict Layer2 Clippy. Its exact contract and gate evidence are in `2026-11-12-s4-response-facts.md`.

Strict CMA remains blocked on the same 19 S5 recovery/scope and three S2 reconciliation locations inventoried above. No warning allowance was introduced. No missing interface blocks the implemented original receipt callers. The missing shared production wiring is `sandbox_reconciliation::transaction::apply_scoped`, `agent_capacity_recovery::{admission,authority,records,status}`, `capture_segmented_scoped` and `TransactionScope::insert_one`.

Final source log hashes (raw temporary logs removed after extraction):
- `s4-final-units.log`: `1e6aa370c3d22634c32c9f853515fb770930cf9ad3325141f7cb45bd76331acd`.
- `s4-final-writer-mongo.log`: `fef3f1282f36a289f640bed6a5bacfb4d25812694a41659ba125fedc4cf445e1`.
- `s4-final-legacy-mongo.log`: `de63e8d409559954ecedb0782d51b292cde5725bbb7ff58111ca2f851a999622`.
- `s4-final-retained-mongo.log`: `b72bd3344910197690ef130ce6b068bade02ee59d890ffb9b1b56c8245e175e7`.
- `s4-final-unpublished-mongo.log`: `b94abd79d263a4c1bacaed123285897c47c4e983b221788bda37044e80eb1d0e`.
- `s4-final-strict.log`: `4bddba021449be3adfc0ce131e730bb0324828e3fbb3244507f06a45bf671a15`.
