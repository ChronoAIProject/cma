//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn usage_save_retries_known_aborts_without_refreshing_revision_or_replaying_unknown_variant_3(
) {
    usage_save_retries_known_aborts_without_refreshing_revision_or_replaying_unknown_case(
        false, 5, true, false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn usage_save_retries_known_aborts_without_refreshing_revision_or_replaying_unknown_variant_7(
) {
    usage_save_retries_known_aborts_without_refreshing_revision_or_replaying_unknown_case(
        true, 5, true, false,
    )
    .await;
}
