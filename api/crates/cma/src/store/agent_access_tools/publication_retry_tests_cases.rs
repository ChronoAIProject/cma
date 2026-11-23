//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn consent_lineage_abort_retries_exact_owner_save_and_proposal_once() {
    consent_lineage_abort_retries_exact_owner_save_and_proposal_once_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn consent_lineage_abort_retries_exact_owner_save_and_proposal_once_variant_2() {
    consent_lineage_abort_retries_exact_owner_save_and_proposal_once_case(true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn consent_abort_bound_and_stale_scope_never_publish_a_new_authority() {
    consent_abort_bound_and_stale_scope_never_publish_a_new_authority_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn consent_abort_bound_and_stale_scope_never_publish_a_new_authority_variant_2() {
    consent_abort_bound_and_stale_scope_never_publish_a_new_authority_case(true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn prepared_tool_receipt_abort_retries_original_resolve_and_confirm() {
    prepared_tool_receipt_abort_retries_original_resolve_and_confirm_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn prepared_tool_receipt_abort_retries_original_resolve_and_confirm_variant_2() {
    prepared_tool_receipt_abort_retries_original_resolve_and_confirm_case(true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn tool_start_abort_bound_and_prepared_identity_refusal_preserve_enrollment() {
    tool_start_abort_bound_and_prepared_identity_refusal_preserve_enrollment_case(1).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn tool_start_abort_bound_and_prepared_identity_refusal_preserve_enrollment_variant_2() {
    tool_start_abort_bound_and_prepared_identity_refusal_preserve_enrollment_case(5).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn uncertain_duplicate_error_never_authorizes_consent_or_prepared_tool_retry() {
    uncertain_duplicate_error_never_authorizes_consent_or_prepared_tool_retry_case(false).await;
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn consent_publication_retry_preserves_original_body_and_timestamp_variant_2() {
    consent_publication_retry_preserves_original_body_and_timestamp_case(true).await;
}
