//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown() {
    suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown_case(
        1, false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown_variant_2(
) {
    suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown_case(
        5, false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown_variant_3(
) {
    suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown_case(
        5, true,
    )
    .await;
}
