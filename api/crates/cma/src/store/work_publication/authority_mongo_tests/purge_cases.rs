//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn terminal_purge_retries_aborted_authority_fence_and_removes_hierarchy_once() {
    terminal_purge_retries_aborted_authority_fence_and_removes_hierarchy_once_case(false, true)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn terminal_purge_retries_aborted_authority_fence_and_removes_hierarchy_once_variant_2() {
    terminal_purge_retries_aborted_authority_fence_and_removes_hierarchy_once_case(true, true)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn terminal_purge_standalone_no_fault_control_removes_same_hierarchy_once() {
    terminal_purge_retries_aborted_authority_fence_and_removes_hierarchy_once_case(false, false)
        .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn terminal_purge_scoped_no_fault_control_removes_same_hierarchy_once() {
    terminal_purge_retries_aborted_authority_fence_and_removes_hierarchy_once_case(true, false)
        .await;
}
