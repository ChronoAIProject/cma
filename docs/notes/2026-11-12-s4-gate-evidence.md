# S4 checkpoint gate evidence — 2026-11-12

Part of #1350. Source code pin `5fdfae28133246ffbeb1d6bc5689513139d9cf95`; documentation head `34a059faf599332c791381376c9e17abf9c25618`.

Initial writer/legacy Mongo source: `e363f788f07a7819d49bbb8bb41422fedc3483b8`. Both ran with repository Rust1.96.1 under heavy-0; each owned fixture and volume cleaned up.

Final Rust1.96.1 strict/unit batch ran under the explicitly authorized existing release-only lock, two Cargo jobs, one target. Strict and unit build exited101; no final unit/Mongo test executed. Available RAM31% initially/minimum29%; no pause. Shared waiters were cancelled before acquisition, not timed out.

## Preliminary writer Mongo

Original log SHA256: `3e385b0a0bb24f640fd0a2f4c48fd030e574b38f9aa5aea841068674f993f586`. Raw temporary log removed after this evidence extraction.

```text

  --> crates/cma/src/store/sandbox_reconciliation/transaction.rs:21:16

  --> crates/cma/src/store/agent_capacity_recovery/admission.rs:26:29

  --> crates/cma/src/store/agent_capacity_recovery/admission.rs:34:29

  --> crates/cma/src/store/agent_capacity_recovery/admission.rs:43:31

  --> crates/cma/src/store/agent_capacity_recovery/authority.rs:22:19

  --> crates/cma/src/store/agent_capacity_recovery/authority.rs:27:19

  --> crates/cma/src/store/agent_capacity_recovery/authority.rs:44:21

   --> crates/cma/src/store/agent_capacity_recovery/authority.rs:134:15

   --> crates/cma/src/store/agent_capacity_recovery/authority.rs:149:21

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:17:28

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:21:8

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:29:21

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:41:4

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:60:15

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:67:21

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:95:4

  --> crates/cma/src/store/agent_capacity_recovery/status.rs:18:19

  --> crates/cma/src/store/agent_capacity_recovery/status.rs:31:31

  --> crates/cma/src/store/sandbox_reconciliation/agents/scoped.rs:24:55

        PASS [   2.912s] ( 1/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::atomicity::different_agents_publish_in_one_database_without_sharing_a_head_fence

        PASS [   2.042s] ( 2/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::atomicity::legacy_append_tripwire_refuses_segmented_agent_without_changing_source

        PASS [   3.545s] ( 3/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::atomicity::real_capture_rollover_requires_finite_admission_and_survives_gate_close

        PASS [   2.396s] ( 4/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::atomicity::same_source_competing_captured_publications_commit_exactly_once

        FAIL [   1.911s] ( 5/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::bootstrap::bootstrap_rejects_replaced_permit_and_changed_original_request

        FAIL [   1.782s] ( 6/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::bootstrap::bootstrap_stages_multiple_fenced_batches_without_exposing_or_rewriting_source

        FAIL [   1.999s] ( 7/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::bootstrap::staged_bootstrap_is_published_only_against_the_same_recaptured_source

        PASS [   5.311s] ( 8/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::capture_contract::captured_publication_commits_and_aborts_with_caller_domain_work

        PASS [   2.764s] ( 9/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::capture_contract::captured_source_and_prepared_plan_cannot_cross_transaction_restarts

        PASS [   2.717s] (10/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::capture_contract::snapshot_source_cannot_be_promoted_into_publication_authority

        PASS [   4.307s] (11/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::keyed_caller::forwarded_read_ports_preserve_predecessor_and_prove_sealed_points

        PASS [   2.287s] (12/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::keyed_caller::keyed_trait_adopts_legacy_only_after_durable_gate_admission

        PASS [   2.952s] (13/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::keyed_caller::keyed_trait_replays_original_entry_after_later_append_and_rejects_changed_request

        PASS [   2.407s] (14/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::keyed_restart::positive_known_abort_releases_claim_and_retries_original_request

        PASS [   2.072s] (15/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::keyed_restart::publication_refusal_aborts_claim_without_any_result

        FAIL [   1.757s] (16/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::keyed_restart::unknown_without_receipt_survives_restart_and_later_head_without_replan

        PASS [   1.856s] (17/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::keyed_settlement::keyed_caller_settles_uncertain_commit_without_republishing

        PASS [   1.861s] (18/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::keyed_settlement::original_caller_receipt_accepts_commit_and_refuses_foreign_or_aborted_attempt

        PASS [   3.636s] (19/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::lifecycle_caller::lifecycle_reconciliation_requires_original_parent_events_and_child_fences

        PASS [   1.739s] (20/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::lifecycle_caller::lifecycle_root_aborts_segmented_child_fence_after_later_domain_refusal

        PASS [   1.868s] (21/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::lifecycle_caller::lifecycle_root_captures_segmented_children_and_preserves_quiet_boundary

        PASS [   2.064s] (22/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::lifecycle_caller::lifecycle_root_reconciles_uncertain_checkpoint_without_repeating_fences

        PASS [   3.602s] (23/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::lifecycle_child::lifecycle_child_success_uncertainty_abort_and_foreign_receipt_are_exact

        PASS [   2.424s] (24/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::original_callers::control_root_updates_segmented_head_and_exact_original_receipt

        PASS [   2.196s] (25/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::original_callers::worker_root_settles_segmented_publication_and_aborts_later_domain_refusal

        PASS [   2.138s] (26/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::output_index::missing_current_output_index_refuses_before_any_publication_write

        PASS [   2.091s] (27/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::output_index::output_evidence_commits_atomically_and_late_output_preserves_first_terminal

        PASS [   1.797s] (28/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::uncertain_commit::known_abort_can_retry_original_identity_without_partial_evidence

        FAIL [   1.605s] (29/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::uncertain_commit::unknown_commit_is_reconciled_from_original_receipt_after_later_head_advance

        PASS [   2.540s] (30/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::writer_abort_after_rollover_and_late_receipt_conflict_leave_no_partial_effects

        PASS [   5.765s] (31/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::writer_rejects_previous_transaction_plan_on_same_session_and_other_database

        PASS [   2.579s] (32/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::writer_rejects_stale_domain_fence_mismatched_result_and_different_session

        PASS [   3.537s] (33/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::writer_rollover_commits_original_batch_head_projection_and_receipt_together

     Summary [  86.484s] 33 tests run: 28 passed, 5 failed, 2466 skipped

        FAIL [   1.911s] ( 5/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::bootstrap::bootstrap_rejects_replaced_permit_and_changed_original_request

        FAIL [   1.782s] ( 6/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::bootstrap::bootstrap_stages_multiple_fenced_batches_without_exposing_or_rewriting_source

        FAIL [   1.999s] ( 7/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::bootstrap::staged_bootstrap_is_published_only_against_the_same_recaptured_source

        FAIL [   1.757s] (16/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::keyed_restart::unknown_without_receipt_survives_restart_and_later_head_without_replan

        FAIL [   1.605s] (29/33) cma::bin/cma store::agent_log_segments::writer_commit::persistence::tests::uncertain_commit::unknown_commit_is_reconciled_from_original_receipt_after_later_head_advance

error: test run failed

```

## Preliminary legacy Mongo

Original log SHA256: `cc86be60bc12d6c1819fe9c82ac78ebbc7dd181b30d3465a3c9c3e660278eb23`. Raw temporary log removed after this evidence extraction.

```text

  --> crates/cma/src/store/sandbox_reconciliation/transaction.rs:21:16

  --> crates/cma/src/store/agent_capacity_recovery/admission.rs:26:29

  --> crates/cma/src/store/agent_capacity_recovery/admission.rs:34:29

  --> crates/cma/src/store/agent_capacity_recovery/admission.rs:43:31

  --> crates/cma/src/store/agent_capacity_recovery/authority.rs:22:19

  --> crates/cma/src/store/agent_capacity_recovery/authority.rs:27:19

  --> crates/cma/src/store/agent_capacity_recovery/authority.rs:44:21

   --> crates/cma/src/store/agent_capacity_recovery/authority.rs:134:15

   --> crates/cma/src/store/agent_capacity_recovery/authority.rs:149:21

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:17:28

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:21:8

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:29:21

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:41:4

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:60:15

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:67:21

  --> crates/cma/src/store/agent_capacity_recovery/records.rs:95:4

  --> crates/cma/src/store/agent_capacity_recovery/status.rs:18:19

  --> crates/cma/src/store/agent_capacity_recovery/status.rs:31:31

  --> crates/cma/src/store/sandbox_reconciliation/agents/scoped.rs:24:55

        PASS [   3.159s] ( 1/23) cma::bin/cma store::agent_worker_transaction::prepared_open_ready_tests::prepared_open_ready_is_atomic_and_restart_keeps_fresh_connection_pending

        PASS [   1.188s] ( 2/23) cma::bin/cma store::agent_worker_transaction::prepared_open_ready_tests::prepared_open_ready_parent_write_conflicts_after_snapshot_read

        PASS [   1.327s] ( 3/23) cma::bin/cma store::agent_worker_transaction::prepared_open_ready_tests::prepared_open_ready_refuses_changed_observation_receipt_ownership_and_parent

        PASS [   1.173s] ( 4/23) cma::bin/cma store::agent_worker_transaction::prepared_open_ready_tests::prepared_open_ready_unknown_commit_requires_complete_atomic_evidence

        PASS [   1.097s] ( 5/23) cma::bin/cma store::agent_worker_transaction::prepared_open_ready_tests::snapshot::prepared_open_ready_ineligible_snapshot_leaves_all_publication_records_unchanged

        PASS [   1.194s] ( 6/23) cma::bin/cma store::agent_worker_transaction::prepared_open_ready_tests::snapshot::prepared_open_ready_preparation_write_conflicts_with_the_consumed_snapshot

        PASS [   1.225s] ( 7/23) cma::bin/cma store::agent_worker_transaction::prepared_open_ready_tests::snapshot::prepared_open_ready_reads_eligibility_once_inside_its_publication_transaction

        PASS [   1.386s] ( 8/23) cma::bin/cma store::agent_worker_transaction::prepared_replay_tests::prepared_pair_parent_write_conflicts_with_lifecycle_after_snapshot_read

        PASS [   1.338s] ( 9/23) cma::bin/cma store::sandbox_mongo_integration_tests::agent_transaction_tests::control_commit_tests::agent_control_reconciles_a_committed_but_ambiguous_transaction

        PASS [   1.391s] (10/23) cma::bin/cma store::sandbox_mongo_integration_tests::agent_transaction_tests::control_mutation_tests::agent_control_concurrent_stop_schedules_once_and_replays_exactly

        PASS [   0.993s] (11/23) cma::bin/cma store::sandbox_mongo_integration_tests::agent_transaction_tests::control_mutation_tests::agent_control_delete_schedules_once_and_hides_foreign_owners

        PASS [   1.453s] (12/23) cma::bin/cma store::sandbox_mongo_integration_tests::agent_transaction_tests::control_mutation_tests::agent_control_receipts_and_lineage_fences_survive_worker_progression

        PASS [   3.698s] (13/23) cma::bin/cma store::sandbox_mongo_integration_tests::agent_transaction_tests::control_snapshot_tests::agent_control_parent_fence_conflicts_with_a_concurrent_replacement

        PASS [   1.130s] (14/23) cma::bin/cma store::sandbox_mongo_integration_tests::agent_transaction_tests::control_snapshot_tests::agent_control_snapshot_requires_exact_receipt_and_current_parents

        PASS [   1.484s] (15/23) cma::bin/cma store::sandbox_mongo_integration_tests::agent_worker_tests::receipt_tests::agent_work_reopen_reconciliation_requires_every_exact_artifact

        PASS [   4.128s] (16/23) cma::bin/cma store::sandbox_mongo_integration_tests::agent_worker_tests::sign_in_tests::startup_sign_in_notice_and_completion_are_atomic_with_parent_state

        PASS [   1.072s] (17/23) cma::bin/cma store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::commit_tests::checkpoint_refuses_missing_persisted_agent_log_evidence

        PASS [   1.088s] (18/23) cma::bin/cma store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::commit_tests::checkpoint_rejects_agent_and_log_changes_after_capture

        PASS [   1.071s] (19/23) cma::bin/cma store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::commit_tests::lifecycle_checkpoint_and_all_agent_phase_commit_atomically

        PASS [   1.037s] (20/23) cma::bin/cma store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::commit_tests::lifecycle_command_reconciles_an_ambiguous_transaction_commit_once

        PASS [   1.118s] (21/23) cma::bin/cma store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::commit_tests::lifecycle_commands_are_owner_bound_gap_free_and_concurrently_idempotent

        PASS [   1.247s] (22/23) cma::bin/cma store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::commit_tests::lifecycle_phase_reconciles_an_ambiguous_commit_without_duplicate_logs

        PASS [   1.949s] (23/23) cma::bin/cma store::sandbox_mongo_integration_tests::sandbox_lifecycle_tests::commit_tests::one_stale_agent_aborts_the_entire_lifecycle_phase

     Summary [  35.954s] 23 tests run: 23 passed, 2476 skipped

```

## Final strict compile

Original log SHA256: `33c5df786353d7a7edbd86474d531f6bff1b0b8ca75536331c8b69e8714392dc`. Raw temporary log removed after this evidence extraction.

```text

error: unused import: `scoped::apply_scoped`

  --> crates/cma/src/store/sandbox_reconciliation/transaction.rs:21:16

error[E0308]: mismatched types

   --> crates/cma/src/store/sandbox_unpublished_recovery/transaction.rs:138:54

    |                    ------------------------------    ^^^^^^^^^^ expected `u64`, found `Option<u64>`

error[E0308]: mismatched types

   --> crates/cma/src/store/sandbox_unpublished_recovery/transaction.rs:139:59

    |                        -------------------------------    ^^^^^^^^^^ expected `u64`, found `Option<u64>`

error: could not compile `cma` (bin "cma") due to 3 previous errors

error: could not compile `cma` (bin "cma" test) due to 3 previous errors

```

## Final unit build

Original log SHA256: `65c7ede9b43c1d3364303b563d9bd3107c511fadc9ea98d696335244a0dd373c`. Raw temporary log removed after this evidence extraction.

```text

  --> crates/cma/src/store/sandbox_reconciliation/transaction.rs:21:16

error[E0308]: mismatched types

   --> crates/cma/src/store/sandbox_unpublished_recovery/transaction.rs:138:54

    |                    ------------------------------    ^^^^^^^^^^ expected `u64`, found `Option<u64>`

error[E0308]: mismatched types

   --> crates/cma/src/store/sandbox_unpublished_recovery/transaction.rs:139:59

    |                        -------------------------------    ^^^^^^^^^^ expected `u64`, found `Option<u64>`

error: could not compile `cma` (bin "cma" test) due to 2 previous errors; 1 warning emitted

error: command `/Users/chronoai/.rustup/toolchains/1.96.1-aarch64-apple-darwin/bin/cargo '--color=auto' test '--manifest-path=api/Cargo.toml' --no-run '--message-format=json-render-diagnostics' '--package=cma' '--bin=cma' --all-features --locked --offline` exited with code 101

```

## Canonical runner

Original log SHA256: `2bb4d7d98f2827e77c3954b43d2b8b3862af9e1ba8c0b9059c15b44f7c4eab07`. Raw temporary log removed after this evidence extraction.

```text

ℹ tests 31

ℹ suites 0

ℹ pass 31

ℹ fail 0

ℹ cancelled 0

ℹ skipped 0

ℹ todo 0

ℹ duration_ms 188263.055125

```

## Canonical history scan

Original log SHA256: `94c55941057d9f9ed65515b72a35abbe5f7c7fbfb3257861398ffbc7a99fc032`. Raw temporary log removed after this evidence extraction.

```text

1:10AM INF 3770 commits scanned.

1:10AM INF no leaks found

```

## Reproduction

Use Rust1.96.1 and nextest0.9.151, `CARGO_BUILD_JOBS=2`, one target, dev/test debug0 and incremental off.

```sh
cargo clippy --locked --offline --manifest-path api/Cargo.toml -p cma --all-targets --all-features --no-deps -- -D warnings
cargo nextest run --locked --offline --manifest-path api/Cargo.toml -p cma --bin cma --all-features -E 'test(store::agent_log_segments::)' --test-threads 2 --no-fail-fast
```

Real Mongo used the canonical owned-fixture runner with only its build/selection narrowed to CMA. Mongo8.0.13 image digest, CPU/RAM/cache bounds, replica-set/failpoints and ownership-checked cleanup were retained. Writer selection: `store::agent_log_segments::writer_commit::persistence::tests::`; the next run additionally selects `agent_segment_tests::writer_gate_tests::slot_tests`. Legacy selection covers prepared worker replay/open-ready, worker receipt/sign-in, control and lifecycle commit suites. Shared-root correction additionally requires retained-assessment/reopen contracts.

The host-Rust1.95 diagnostic run had40 passing units, but does not qualify the repository toolchain. Its strict runner exit accounting was contaminated; it is not a pass.
