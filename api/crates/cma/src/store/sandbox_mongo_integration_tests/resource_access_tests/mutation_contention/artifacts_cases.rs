//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn artifact_known_abort_preserves_one_record_and_unknown_result_never_replays() {
    artifact_known_abort_preserves_one_record_and_unknown_result_never_replays_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn artifact_known_abort_preserves_one_record_and_unknown_result_never_replays_variant_2() {
    artifact_known_abort_preserves_one_record_and_unknown_result_never_replays_case(true).await;
}
