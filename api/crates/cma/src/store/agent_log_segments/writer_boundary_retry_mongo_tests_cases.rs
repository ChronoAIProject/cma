//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn keyed_and_segmented_control_retry_aborted_start_with_original_authority() {
    keyed_and_segmented_control_retry_aborted_start_with_original_authority_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn keyed_and_segmented_control_retry_aborted_start_with_original_authority_variant_2() {
    keyed_and_segmented_control_retry_aborted_start_with_original_authority_case(true).await;
}
