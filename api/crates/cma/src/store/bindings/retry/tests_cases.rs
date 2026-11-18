//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_recovery_accepts_current_parent_envelopes_without_changing_custody() {
    binding_recovery_accepts_current_parent_envelopes_without_changing_custody_case(1).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_recovery_accepts_current_parent_envelopes_without_changing_custody_variant_2() {
    binding_recovery_accepts_current_parent_envelopes_without_changing_custody_case(3).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_recovery_accepts_current_parent_envelopes_without_changing_custody_variant_3() {
    binding_recovery_accepts_current_parent_envelopes_without_changing_custody_case(4).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn binding_recovery_retries_aborted_fence_reads_and_commit_with_original_intent() {
    binding_recovery_retries_aborted_fence_reads_and_commit_with_original_intent_case("update")
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn binding_recovery_retries_aborted_fence_reads_and_commit_with_original_intent_variant_2() {
    binding_recovery_retries_aborted_fence_reads_and_commit_with_original_intent_case("find").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn binding_recovery_retries_aborted_fence_reads_and_commit_with_original_intent_variant_3() {
    binding_recovery_retries_aborted_fence_reads_and_commit_with_original_intent_case(
        "commitTransaction",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn binding_recovery_bounds_known_aborts_and_never_replays_unknown_commit() {
    binding_recovery_bounds_known_aborts_and_never_replays_unknown_commit_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn binding_recovery_bounds_known_aborts_and_never_replays_unknown_commit_variant_2() {
    binding_recovery_bounds_known_aborts_and_never_replays_unknown_commit_case(true).await;
}
