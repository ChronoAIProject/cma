//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn consent_lineage_abort_retries_exact_owner_save_and_proposal_once() {
    consent_lineage_abort_retries_exact_owner_save_and_proposal_once_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn uncertain_duplicate_error_never_authorizes_consent_or_prepared_tool_retry_variant_2() {
    uncertain_duplicate_error_never_authorizes_consent_or_prepared_tool_retry_case(true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn consent_publication_retry_preserves_original_body_and_timestamp() {
    consent_publication_retry_preserves_original_body_and_timestamp_case(false).await;
}
