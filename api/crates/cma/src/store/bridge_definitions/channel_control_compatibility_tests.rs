//! Separate cross-process runner; ordinary Mongo contract tests need no extra environment.
use super::mongo_integration_tests::compatibility;

#[tokio::test]
#[ignore = "requires the owned cross-version Channel control runner and pinned old/new binaries"]
async fn channel_control_compatibility_prepare_actual_records() {
    compatibility::channel_control_compatibility_prepare_actual_records().await;
}

#[tokio::test]
#[ignore = "requires the owned cross-version Channel control runner and pinned old/new binaries"]
async fn channel_control_compatibility_compatible_capacity_counts_held_control() {
    compatibility::channel_control_compatibility_compatible_capacity_counts_held_control().await;
}

#[tokio::test]
#[ignore = "requires the owned cross-version Channel control runner and pinned old/new binaries"]
async fn channel_control_compatibility_rollback_resumes_retained_control_with_fresh_admission_disabled(
) {
    compatibility::channel_control_compatibility_rollback_resumes_retained_control_with_fresh_admission_disabled().await;
}
