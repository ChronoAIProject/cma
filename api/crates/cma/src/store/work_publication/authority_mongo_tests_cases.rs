//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn known_abort_at_work_authority_fence_retries_fresh_and_publishes_children_once() {
    known_abort_at_work_authority_fence_retries_fresh_and_publishes_children_once_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn known_abort_at_work_authority_fence_retries_fresh_and_publishes_children_once_variant_2() {
    known_abort_at_work_authority_fence_retries_fresh_and_publishes_children_once_case(true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn repeated_work_authority_abort_is_bounded_and_leaves_no_partial_publication() {
    repeated_work_authority_abort_is_bounded_and_leaves_no_partial_publication_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn repeated_work_authority_abort_is_bounded_and_leaves_no_partial_publication_variant_2() {
    repeated_work_authority_abort_is_bounded_and_leaves_no_partial_publication_case(true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn checkpoint_publication_retries_only_aborted_authority_fence() {
    checkpoint_publication_retries_only_aborted_authority_fence_case(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn checkpoint_publication_retries_only_aborted_authority_fence_variant_2() {
    checkpoint_publication_retries_only_aborted_authority_fence_case(true).await;
}
