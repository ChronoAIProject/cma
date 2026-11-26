//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn share_known_abort_converges_but_exhaustion_publishes_nothing() {
    share_known_abort_converges_but_exhaustion_publishes_nothing_case(1).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn share_known_abort_converges_but_exhaustion_publishes_nothing_variant_2() {
    share_known_abort_converges_but_exhaustion_publishes_nothing_case(TRANSACTION_ATTEMPTS).await;
}
