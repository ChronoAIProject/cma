use super::*;
use crate::ingress_budget::{ClassBounds, IngressBudget};

fn fixture() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(256 * 1024);
    let plan = |n| AllocationPlan::new("relay fixture", &[n]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(8192), plan(16384)).unwrap(),
    )
    .unwrap();
    (root, policy)
}

#[tokio::test]
async fn capture_refuses_overflow_without_waiting_for_a_consumer() {
    let (root, policy) = fixture();
    let (sender, capture) = UpdateRelay::capture_admitted(&policy).unwrap();
    let before = root.available_bytes();
    sender.send(ConversationUpdate::Lost { frames: 1 }).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        sender.send(ConversationUpdate::Lost { frames: 2 }),
    )
    .await
    .unwrap();
    assert!(capture.take().is_err());
    assert_eq!(root.available_bytes(), before);
    drop((sender, capture));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn capture_transfers_one_effect_and_retains_its_own_capacity_through_last_handle() {
    let (root, policy) = fixture();
    let (sender, capture) = UpdateRelay::capture_admitted(&policy).unwrap();
    sender.send(ConversationUpdate::Lost { frames: 3 }).await;
    assert!(matches!(
        capture.take().unwrap(),
        Some(ConversationUpdate::Lost { frames: 3 })
    ));
    let before = root.available_bytes();
    drop(capture);
    assert_eq!(root.available_bytes(), before);
    drop(sender);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn admitted_relay_releases_queue_only_after_sender_and_cancelled_receiver_teardown() {
    let (root, policy) = fixture();
    let (output, mut receiver) = mpsc::channel(1);
    let (sender, task) = spawn_admitted(output, &policy).unwrap();
    sender.send(ConversationUpdate::Lost { frames: 7 }).await;
    assert!(matches!(
        receiver.recv().await,
        Some(ConversationUpdate::Lost { frames: 7 })
    ));
    let before = root.available_bytes();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(root.available_bytes(), before);
    drop(sender);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn poisoned_capture_keeps_payload_charged_until_its_final_handle_drops() {
    let (root, policy) = fixture();
    let (sender, capture) = UpdateRelay::capture_admitted(&policy).unwrap();
    let payload = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("poisoned payload", &[8192]).unwrap(),
        )
        .unwrap()
        .initialize(|| ConversationUpdate::Failed {
            message: "x".repeat(4096),
            capability: None,
        });
    sender
        .send(ConversationUpdate::RetainedEffect(
            crate::RuntimeOutputUpdate::from_admitted(payload),
        ))
        .await;
    let retained = root.available_bytes();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _value = capture.value.lock().unwrap();
        panic!("synthetic consumer panic");
    }));
    assert!(panic.is_err());
    assert!(capture.take().is_err());
    drop(capture);
    assert_eq!(root.available_bytes(), retained);
    drop(sender);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
