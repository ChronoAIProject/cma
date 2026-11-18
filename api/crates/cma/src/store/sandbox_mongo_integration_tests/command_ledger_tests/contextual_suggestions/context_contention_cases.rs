//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_context_abort_retries_retained_output_and_unknown_never_replays() {
    suggestion_context_abort_retries_retained_output_and_unknown_never_replays_case(
        "agents", 0, 1, false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_context_abort_retries_retained_output_and_unknown_never_replays_variant_2() {
    suggestion_context_abort_retries_retained_output_and_unknown_never_replays_case(
        "sandboxes",
        0,
        1,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_context_abort_retries_retained_output_and_unknown_never_replays_variant_3() {
    suggestion_context_abort_retries_retained_output_and_unknown_never_replays_case(
        "agent_commands",
        0,
        1,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_context_abort_retries_retained_output_and_unknown_never_replays_variant_4() {
    suggestion_context_abort_retries_retained_output_and_unknown_never_replays_case(
        "agents", 1, 1, false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_context_abort_retries_retained_output_and_unknown_never_replays_variant_5() {
    suggestion_context_abort_retries_retained_output_and_unknown_never_replays_case(
        "agent_log",
        0,
        1,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_context_abort_retries_retained_output_and_unknown_never_replays_variant_6() {
    suggestion_context_abort_retries_retained_output_and_unknown_never_replays_case(
        "sandboxes",
        0,
        5,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_context_abort_retries_retained_output_and_unknown_never_replays_variant_7() {
    suggestion_context_abort_retries_retained_output_and_unknown_never_replays_case(
        "sandboxes",
        0,
        1,
        true,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_claim_retries_known_abort_with_original_deadline_and_refuses_unknown() {
    suggestion_claim_retries_known_abort_with_original_deadline_and_refuses_unknown_case(
        "find", 1, false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_claim_retries_known_abort_with_original_deadline_and_refuses_unknown_variant_2()
{
    suggestion_claim_retries_known_abort_with_original_deadline_and_refuses_unknown_case(
        "update", 1, false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_claim_retries_known_abort_with_original_deadline_and_refuses_unknown_variant_3()
{
    suggestion_claim_retries_known_abort_with_original_deadline_and_refuses_unknown_case(
        "commitTransaction",
        1,
        false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_claim_retries_known_abort_with_original_deadline_and_refuses_unknown_variant_4()
{
    suggestion_claim_retries_known_abort_with_original_deadline_and_refuses_unknown_case(
        "update", 5, false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_claim_retries_known_abort_with_original_deadline_and_refuses_unknown_variant_5()
{
    suggestion_claim_retries_known_abort_with_original_deadline_and_refuses_unknown_case(
        "commitTransaction",
        5,
        true,
    )
    .await;
}
