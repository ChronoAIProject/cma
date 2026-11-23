//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_unknown_committed_recovers_exact_receipt_without_new_transaction() {
    metadata_unknown_committed_recovers_exact_receipt_without_new_transaction_case(
        Operation::Channel,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_unknown_committed_recovers_exact_receipt_without_new_transaction_variant_2() {
    metadata_unknown_committed_recovers_exact_receipt_without_new_transaction_case(
        Operation::Provenance,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_unknown_committed_recovers_exact_receipt_without_new_transaction_variant_3() {
    metadata_unknown_committed_recovers_exact_receipt_without_new_transaction_case(
        Operation::Reuse,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_unknown_uncommitted_refuses_without_reapplying_absent_receipt() {
    metadata_unknown_uncommitted_refuses_without_reapplying_absent_receipt_case(Operation::Channel)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_unknown_uncommitted_refuses_without_reapplying_absent_receipt_variant_2() {
    metadata_unknown_uncommitted_refuses_without_reapplying_absent_receipt_case(
        Operation::Provenance,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_unknown_uncommitted_refuses_without_reapplying_absent_receipt_variant_3() {
    metadata_unknown_uncommitted_refuses_without_reapplying_absent_receipt_case(Operation::Reuse)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_known_abort_at_start_retries_original_authority_with_fresh_snapshot() {
    metadata_known_abort_at_start_retries_original_authority_with_fresh_snapshot_case(
        Operation::Channel,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_known_abort_at_start_retries_original_authority_with_fresh_snapshot_variant_2() {
    metadata_known_abort_at_start_retries_original_authority_with_fresh_snapshot_case(
        Operation::Provenance,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_known_abort_at_start_retries_original_authority_with_fresh_snapshot_variant_3() {
    metadata_known_abort_at_start_retries_original_authority_with_fresh_snapshot_case(
        Operation::Reuse,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_known_abort_at_start_retries_original_authority_with_fresh_snapshot_variant_4() {
    metadata_known_abort_at_start_retries_original_authority_with_fresh_snapshot_case(
        Operation::Snapshot,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_known_abort_at_start_retries_original_authority_with_fresh_snapshot_variant_5() {
    metadata_known_abort_at_start_retries_original_authority_with_fresh_snapshot_case(
        Operation::History,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_precommit_unknown_dominates_duplicate_and_transient_labels() {
    metadata_precommit_unknown_dominates_duplicate_and_transient_labels_case(Operation::Channel)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_precommit_unknown_dominates_duplicate_and_transient_labels_variant_2() {
    metadata_precommit_unknown_dominates_duplicate_and_transient_labels_case(Operation::Provenance)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_precommit_unknown_dominates_duplicate_and_transient_labels_variant_3() {
    metadata_precommit_unknown_dominates_duplicate_and_transient_labels_case(Operation::Reuse)
        .await;
}
