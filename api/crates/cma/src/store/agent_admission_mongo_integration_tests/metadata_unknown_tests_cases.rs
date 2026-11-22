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
async fn metadata_unknown_uncommitted_refuses_without_reapplying_absent_receipt() {
    metadata_unknown_uncommitted_refuses_without_reapplying_absent_receipt_case(Operation::Channel)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn metadata_precommit_unknown_dominates_duplicate_and_transient_labels() {
    metadata_precommit_unknown_dominates_duplicate_and_transient_labels_case(Operation::Channel)
        .await;
}
