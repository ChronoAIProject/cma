//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn ownership_fence_exhaustion_and_unknown_commit_never_publish_or_rebase() {
    ownership_fence_exhaustion_and_unknown_commit_never_publish_or_rebase_case(false).await;
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_FAILPOINT_URI"]
async fn ownership_fence_exhaustion_and_unknown_commit_never_publish_or_rebase_variant_2() {
    ownership_fence_exhaustion_and_unknown_commit_never_publish_or_rebase_case(true).await;
}
