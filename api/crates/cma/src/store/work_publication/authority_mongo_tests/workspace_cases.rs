//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry() {
    workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_case("find", false)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_variant_2() {
    workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_case("find", true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_variant_3() {
    workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_case("update", false)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_variant_4() {
    workspace_parent_read_and_write_abort_preserve_fresh_transaction_retry_case("update", true)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn nested_child_claim_retries_aborted_original_parent_authority() {
    nested_child_claim_retries_aborted_original_parent_authority_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn nested_child_claim_retries_aborted_original_parent_authority_variant_2() {
    nested_child_claim_retries_aborted_original_parent_authority_case(true).await;
}
