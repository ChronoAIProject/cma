//! One unavailable renewal must fence its Agent without cancelling healthy work.
use super::*;
use layer2_core::agent_ownership::{Authority, OwnershipCoordinator};
use layer3_adapter::owner_protocol::TransportAuthority;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

#[path = "mongo_containment_tests/support.rs"]
mod support;
use support::{Fault, Fixture};

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo replica set"]
async fn known_abort_exhaustion_contains_only_the_failed_agent() {
    isolated_renewal(Fault::KnownAbort).await;
}

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo replica set"]
async fn uncertain_renewal_retains_custody_without_replay() {
    isolated_renewal(Fault::UnknownCommit).await;
}

#[tokio::test]
#[ignore = "requires disposable failpoint-enabled Mongo replica set"]
async fn committed_renewal_withheld_reply_retains_custody_without_replay() {
    isolated_renewal(Fault::WithheldReply).await;
}

async fn isolated_renewal(fault: Fault) {
    let fixture = Fixture::new().await;
    let original = fixture.lease(&fixture.faulted).await;
    let (send, mut receive) = tokio::sync::mpsc::channel(4);
    let streaming = tokio::spawn({
        let healthy = fixture.healthy.clone();
        async move {
            healthy
                .run(async {
                    while let Some(value) = receive.recv().await {
                        let (sequence, reply): (u32, tokio::sync::oneshot::Sender<u32>) = value;
                        reply
                            .send(sequence)
                            .expect("stream consumer remains present");
                    }
                })
                .await
        }
    });
    let (reply, frame) = tokio::sync::oneshot::channel();
    send.send((1, reply)).await.unwrap();
    assert_eq!(frame.await.unwrap(), 1);

    fixture.arm(fault).await;
    let started = tokio::time::Instant::now();
    let result = fixture.manager.maintain().await;
    let elapsed = started.elapsed();
    fixture.disarm().await;
    let actual = fixture.lease(&fixture.faulted).await;
    let returned = fixture.router.returned.lock().unwrap().clone();
    eprintln!(
        "renewal_containment fault={fault:?} elapsed_ms={} store_returned={} manager_ready={} healthy_current={} failed_current={}",
        elapsed.as_millis(), returned.is_some(), fixture.manager.is_ready(),
        fixture.healthy.is_current(), fixture.faulted.is_current()
    );
    assert_eq!(actual.token, original.token);
    match fault {
        Fault::KnownAbort => {
            assert_eq!(actual, original, "aborted renewal changed the lease");
            assert!(elapsed >= Duration::from_millis(1_900));
            assert!(elapsed < Duration::from_secs(3), "renewal budget widened");
        }
        Fault::UnknownCommit => {
            assert!(matches!(returned, Some(OwnershipError::Unavailable(_))));
            // An unknown commit can leave either the old or renewed lease.
            // Both must retain exact custody without replaying the renewal.
            assert!(actual.expires_at_ms >= original.expires_at_ms);
        }
        Fault::WithheldReply => {
            assert!(
                returned.is_none(),
                "withheld store reply reached coordinator"
            );
            assert!(actual.expires_at_ms > original.expires_at_ms);
            assert!(elapsed >= Duration::from_millis(1_900));
            assert!(elapsed < Duration::from_secs(3), "renewal budget widened");
        }
    }
    assert!(!fixture.faulted.is_current());
    assert!(fixture
        .faulted
        .run(async { panic!("cancelled effect executed") })
        .await
        .is_err());
    // Capture the RED before cleanup; a failed assertion must not strand the
    // disposable database or the bounded stream task.
    let manager_ready = fixture.manager.is_ready();
    let healthy_current = fixture.healthy.is_current();
    let failure_signal = *fixture.manager.failure_signal().borrow();
    if result.is_err() || !manager_ready || !healthy_current || failure_signal {
        drop(send);
        streaming
            .await
            .unwrap()
            .expect_err("global failure cancelled the stream");
        fixture.close().await;
        panic!("isolated renewal disabled healthy authority: result={:?}, ready={manager_ready}, healthy={healthy_current}, failed={failure_signal}", result.as_ref().map(Vec::len));
    }
    assert!(fixture.faulted.receiver_cancelled());
    assert_eq!(fixture.router.calls.load(Ordering::SeqCst), 1);
    assert!(fixture
        .manager
        .acquire(&fixture.faulted.token.agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(fixture
        .manager
        .cancelled_receivers()
        .await
        .iter()
        .any(|owner| Arc::ptr_eq(owner, &fixture.faulted)));
    fixture.healthy_effect("after-renewal-failure").await;
    let (reply, frame) = tokio::sync::oneshot::channel();
    send.send((2, reply)).await.unwrap();
    assert_eq!(frame.await.unwrap(), 2);
    fixture.manager.maintain().await.unwrap();
    assert_eq!(
        fixture.router.calls.load(Ordering::SeqCst),
        1,
        "cancelled renewal replayed"
    );
    assert_eq!(fixture.lease(&fixture.faulted).await, actual);
    assert!(
        TransportAuthority::renew_for_challenge(fixture.faulted.as_ref())
            .await
            .is_err()
    );
    assert_eq!(fixture.router.calls.load(Ordering::SeqCst), 1);
    drop(send);
    streaming.await.unwrap().unwrap();
    fixture.close().await;
}
