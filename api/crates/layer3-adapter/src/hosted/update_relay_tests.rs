use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::Notify;

#[tokio::test]
async fn slow_consumer_bounds_ingress_and_resumes_in_order() {
    let (out, mut received) = mpsc::channel(1);
    let (updates, task) = spawn(out, true);
    let progress = Arc::new(AtomicUsize::new(0));
    let full = Arc::new(Notify::new());
    let sender = updates.clone();
    let observed = progress.clone();
    let signal = full.clone();
    let producer = tokio::spawn(async move {
        for index in 0..100 {
            sender
                .send(ConversationUpdate::Lost { frames: index })
                .await;
            observed.fetch_add(1, Ordering::SeqCst);
            if index as usize + 1 == RELAY_CAPACITY + 2 {
                signal.notify_one();
            }
        }
    });
    tokio::time::timeout(Duration::from_secs(2), full.notified())
        .await
        .unwrap();
    assert_eq!(progress.load(Ordering::SeqCst), RELAY_CAPACITY + 2);
    assert!(!producer.is_finished());
    // 32 queued, one forwarding and one in the downstream channel. No receiver
    // has consumed anything and the next send must remain suspended.
    for expected in 0..100 {
        assert!(
            matches!(tokio::time::timeout(Duration::from_secs(2), received.recv()).await.unwrap(),
            Some(ConversationUpdate::Lost { frames }) if frames == expected)
        );
    }
    producer.await.unwrap();
    drop(updates);
    task.await.unwrap();
}

#[tokio::test]
async fn downstream_drop_wakes_blocked_sender_and_closes_the_relay() {
    let (out, received) = mpsc::channel(1);
    let (updates, task) = spawn(out, true);
    let sender = updates.clone();
    let producer = tokio::spawn(async move {
        for _ in 0..RELAY_CAPACITY * 4 {
            sender.send(ConversationUpdate::Lost { frames: 1 }).await;
        }
    });
    drop(received);
    tokio::time::timeout(Duration::from_secs(2), updates.closed())
        .await
        .unwrap();
    producer.await.unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn neither_enqueue_nor_dequeue_nor_drop_acknowledges_native_output() {
    let (acks, mut acknowledged) = mpsc::channel(1);
    let receipt = crate::RuntimeOutputReceipt::new(
        crate::RuntimeOutputSource {
            stream_id: uuid::Uuid::new_v4().to_string(),
            cursor: 1,
            native_conversation_id: "retained".into(),
            payload_sha256: "f".repeat(64),
        },
        acks,
    );
    let (out, mut received) = mpsc::channel(1);
    let (updates, task) = spawn(out, true);
    updates
        .send(ConversationUpdate::RuntimeOutput {
            receipt: receipt.clone(),
            update: None,
        })
        .await;
    drop(received.recv().await.unwrap());
    assert!(acknowledged.try_recv().is_err());
    drop(received);
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(acknowledged.try_recv().is_err());
    receipt.acknowledge().await.unwrap();
    assert_eq!(acknowledged.recv().await.unwrap().cursor, 1);
}

#[tokio::test]
async fn disabled_policy_keeps_legacy_order_without_sender_backpressure() {
    let (out, mut received) = mpsc::channel(1);
    let (updates, task) = spawn(out, false);
    assert!(!updates.bounded());
    for frames in 0..1000 {
        updates.send(ConversationUpdate::Lost { frames }).await;
    }
    for expected in 0..1000 {
        assert!(matches!(received.recv().await,
            Some(ConversationUpdate::Lost { frames }) if frames == expected));
    }
    drop(updates);
    task.await.unwrap();
}
