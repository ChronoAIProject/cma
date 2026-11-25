//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn composed_pair_targeted_and_queue_admissions_retry_leaf_abort() {
    composed_pair_targeted_and_queue_admissions_retry_leaf_abort_case(Kind::Pair).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn composed_pair_targeted_and_queue_admissions_retry_leaf_abort_variant_2() {
    composed_pair_targeted_and_queue_admissions_retry_leaf_abort_case(Kind::Targeted).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn composed_pair_targeted_and_queue_admissions_retry_leaf_abort_variant_3() {
    composed_pair_targeted_and_queue_admissions_retry_leaf_abort_case(Kind::Edit).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn composed_pair_targeted_and_queue_admissions_retry_leaf_abort_variant_4() {
    composed_pair_targeted_and_queue_admissions_retry_leaf_abort_case(Kind::Cancel).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_unknown_and_both_labels_never_replay_or_publish() {
    leaf_unknown_and_both_labels_never_replay_or_publish_case(
        Kind::Ordinary,
        vec!["UnknownTransactionCommitResult"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_unknown_and_both_labels_never_replay_or_publish_variant_2() {
    leaf_unknown_and_both_labels_never_replay_or_publish_case(
        Kind::Ordinary,
        vec![
            "TransientTransactionError",
            "UnknownTransactionCommitResult",
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_unknown_and_both_labels_never_replay_or_publish_variant_3() {
    leaf_unknown_and_both_labels_never_replay_or_publish_case(
        Kind::Pair,
        vec!["UnknownTransactionCommitResult"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_unknown_and_both_labels_never_replay_or_publish_variant_4() {
    leaf_unknown_and_both_labels_never_replay_or_publish_case(
        Kind::Pair,
        vec![
            "TransientTransactionError",
            "UnknownTransactionCommitResult",
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_unknown_and_both_labels_never_replay_or_publish_variant_5() {
    leaf_unknown_and_both_labels_never_replay_or_publish_case(
        Kind::Targeted,
        vec!["UnknownTransactionCommitResult"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_unknown_and_both_labels_never_replay_or_publish_variant_6() {
    leaf_unknown_and_both_labels_never_replay_or_publish_case(
        Kind::Targeted,
        vec![
            "TransientTransactionError",
            "UnknownTransactionCommitResult",
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_unknown_and_both_labels_never_replay_or_publish_variant_7() {
    leaf_unknown_and_both_labels_never_replay_or_publish_case(
        Kind::Edit,
        vec!["UnknownTransactionCommitResult"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_unknown_and_both_labels_never_replay_or_publish_variant_8() {
    leaf_unknown_and_both_labels_never_replay_or_publish_case(
        Kind::Edit,
        vec![
            "TransientTransactionError",
            "UnknownTransactionCommitResult",
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_unknown_and_both_labels_never_replay_or_publish_variant_9() {
    leaf_unknown_and_both_labels_never_replay_or_publish_case(
        Kind::Cancel,
        vec!["UnknownTransactionCommitResult"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_unknown_and_both_labels_never_replay_or_publish_variant_10() {
    leaf_unknown_and_both_labels_never_replay_or_publish_case(
        Kind::Cancel,
        vec![
            "TransientTransactionError",
            "UnknownTransactionCommitResult",
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_retry_refuses_durable_takeover_with_live_local_token() {
    leaf_retry_refuses_durable_takeover_with_live_local_token_case(Kind::Ordinary).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_retry_refuses_durable_takeover_with_live_local_token_variant_2() {
    leaf_retry_refuses_durable_takeover_with_live_local_token_case(Kind::Pair).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_retry_refuses_durable_takeover_with_live_local_token_variant_3() {
    leaf_retry_refuses_durable_takeover_with_live_local_token_case(Kind::Targeted).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_retry_refuses_durable_takeover_with_live_local_token_variant_4() {
    leaf_retry_refuses_durable_takeover_with_live_local_token_case(Kind::Edit).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_retry_refuses_durable_takeover_with_live_local_token_variant_5() {
    leaf_retry_refuses_durable_takeover_with_live_local_token_case(Kind::Cancel).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_retry_exhaustion_is_bounded_without_partial_admission() {
    leaf_retry_exhaustion_is_bounded_without_partial_admission_case(Kind::Ordinary).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_retry_exhaustion_is_bounded_without_partial_admission_variant_2() {
    leaf_retry_exhaustion_is_bounded_without_partial_admission_case(Kind::Pair).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_retry_exhaustion_is_bounded_without_partial_admission_variant_3() {
    leaf_retry_exhaustion_is_bounded_without_partial_admission_case(Kind::Targeted).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_retry_exhaustion_is_bounded_without_partial_admission_variant_4() {
    leaf_retry_exhaustion_is_bounded_without_partial_admission_case(Kind::Edit).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn leaf_retry_exhaustion_is_bounded_without_partial_admission_variant_5() {
    leaf_retry_exhaustion_is_bounded_without_partial_admission_case(Kind::Cancel).await;
}
