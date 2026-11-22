//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_conversation_exhaustion_unknown_and_both_labels() {
    typed_fence_conversation_exhaustion_unknown_and_both_labels_case(
        Root::Conversation,
        vec!["TransientTransactionError"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_conversation_exhaustion_unknown_and_both_labels_variant_2() {
    typed_fence_conversation_exhaustion_unknown_and_both_labels_case(
        Root::Conversation,
        vec!["UnknownTransactionCommitResult"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_conversation_exhaustion_unknown_and_both_labels_variant_3() {
    typed_fence_conversation_exhaustion_unknown_and_both_labels_case(
        Root::Conversation,
        vec![
            "TransientTransactionError",
            "UnknownTransactionCommitResult",
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_conversation_exhaustion_unknown_and_both_labels_variant_4() {
    typed_fence_conversation_exhaustion_unknown_and_both_labels_case(
        Root::Checkpoint,
        vec!["TransientTransactionError"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_conversation_exhaustion_unknown_and_both_labels_variant_5() {
    typed_fence_conversation_exhaustion_unknown_and_both_labels_case(
        Root::Checkpoint,
        vec!["UnknownTransactionCommitResult"],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_conversation_exhaustion_unknown_and_both_labels_variant_6() {
    typed_fence_conversation_exhaustion_unknown_and_both_labels_case(
        Root::Checkpoint,
        vec![
            "TransientTransactionError",
            "UnknownTransactionCommitResult",
        ],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_conversation_recheck_stale_token() {
    typed_fence_conversation_recheck_stale_token_case(Root::Conversation).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable failpoint Mongo"]
async fn typed_fence_conversation_recheck_stale_token_variant_2() {
    typed_fence_conversation_recheck_stale_token_case(Root::Checkpoint).await;
}
