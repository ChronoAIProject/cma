use super::*;
use layer2_core::sandbox::ingress_ownership::{
    AllocationPlan, ClassBounds, IngressBudget, IngressOwnershipPolicy,
};
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource};

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(256 * 1024);
    let plan = |bytes| AllocationPlan::checked("live ACK fixture", &[bytes]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(64 * 1024), plan(64 * 1024)).unwrap(),
    )
    .unwrap();
    (root, policy)
}

fn source(cursor: u64) -> RuntimeOutputSource {
    RuntimeOutputSource {
        stream_id: "00000000-0000-4000-8000-000000000001".into(),
        cursor,
        native_conversation_id: "synthetic-conversation".into(),
        payload_sha256: "b".repeat(64),
    }
}

#[tokio::test]
async fn admitted_ack_refusal_retains_pending_identity_until_exact_retry() {
    let (root, policy) = policy();
    let (sender, mut receiver) = RuntimeOutputReceipt::admitted_channel_checked(policy, 1).unwrap();
    let earlier = RuntimeOutputReceipt::new_admitted(source(1), sender.clone());
    earlier.acknowledge_admitted().unwrap();
    let committed = source(2);
    let receipt = RuntimeOutputReceipt::new_admitted(committed.clone(), sender);
    let mut pending = Some(PendingReceipt::Legacy(receipt));
    let before = root.available_bytes();
    let error = acknowledge(&committed, None, &mut pending, true)
        .await
        .unwrap_err();
    assert!(matches!(&*error, AgentConversationError::Unavailable(detail) if detail.is_empty()));
    assert_eq!(pending.as_ref().unwrap().source, committed);
    assert_eq!(root.available_bytes(), before);
    assert_eq!(*receiver.recv().await.unwrap(), earlier.source);
    acknowledge(&committed, None, &mut pending, true)
        .await
        .unwrap();
    assert!(pending.is_none());
    assert_eq!(*receiver.recv().await.unwrap(), committed);
    drop(earlier);
    assert!(receiver.recv().await.is_none());
    drop(receiver);
    drop(error);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn enabled_ack_refuses_legacy_queue_without_enqueuing() {
    let (sender, mut receiver) = mpsc::channel(1);
    let source = source(1);
    let receipt = RuntimeOutputReceipt::new(source.clone(), sender);
    let mut pending = None;
    let error = acknowledge(&source, Some(&receipt), &mut pending, true)
        .await
        .unwrap_err();
    assert!(matches!(&*error, AgentConversationError::Unavailable(detail) if detail.is_empty()));
    assert!(receiver.try_recv().is_err());
    assert_eq!(receipt.source, source);
}

#[tokio::test]
async fn enabled_ack_refuses_closed_queue_and_preserves_the_pending_receipt() {
    let (root, policy) = policy();
    let (sender, receiver) = RuntimeOutputReceipt::admitted_channel_checked(policy, 1).unwrap();
    drop(receiver);
    let source = source(1);
    let mut pending = Some(PendingReceipt::Legacy(RuntimeOutputReceipt::new_admitted(
        source.clone(),
        sender,
    )));
    let before = root.available_bytes();
    let error = acknowledge(&source, None, &mut pending, true)
        .await
        .unwrap_err();
    assert!(matches!(&*error, AgentConversationError::Unavailable(detail) if detail.is_empty()));
    assert_eq!(pending.as_ref().unwrap().source, source);
    assert_eq!(root.available_bytes(), before);
    drop(pending);
    drop(error);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn admitted_ack_cannot_substitute_a_different_committed_source() {
    let (root, policy) = policy();
    let (sender, mut receiver) = RuntimeOutputReceipt::admitted_channel_checked(policy, 1).unwrap();
    let receipt = RuntimeOutputReceipt::new_admitted(source(1), sender);
    let mut pending = None;
    let error = acknowledge(&source(2), Some(&receipt), &mut pending, true)
        .await
        .unwrap_err();
    assert!(matches!(&*error, AgentConversationError::InvalidState(_)));
    drop(receipt);
    assert!(receiver.recv().await.is_none());
    drop(receiver);
    drop(error);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
