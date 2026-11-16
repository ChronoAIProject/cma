use super::open_lease_fixture::{Failure, Fixture};
use super::*;
use crate::owner_protocol::{test_support::TestAuthority, TransportAuthority};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn open_keeps_one_fresh_receiver_on_the_pinned_endpoint() {
    let fixture = Fixture::new(3, 0, Failure::None).await;
    let (channel, receipt) = fixture.open().await.unwrap();
    assert_eq!(channel.conversation_id, receipt.receipt.conversation_id);
    assert_eq!(receipt.authority_fence, 9);
    assert_eq!(fixture.discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.counts.open.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.counts.websocket.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.counts.challenge.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.counts.confirm.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.authority.inner.renewals.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.authority.registrations.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.authority.receivers.lock().unwrap().len(), 1);
    fixture.finish().await;
}

#[tokio::test]
async fn failed_negotiation_or_receipt_cannot_authorize_acp() {
    for failure in [
        Failure::Renewal,
        Failure::Ack,
        Failure::Receipt,
        Failure::RevokeAfterOpen,
    ] {
        let fixture = Fixture::new(3, 0, failure).await;
        assert!(fixture.open().await.is_err());
        assert_eq!(fixture.counts.websocket.load(Ordering::SeqCst), 0);
        let effects = usize::from(matches!(
            failure,
            Failure::Receipt | Failure::RevokeAfterOpen
        ));
        assert_eq!(fixture.counts.open.load(Ordering::SeqCst), effects);
        assert!(fixture.transport.state.lock().await.links.is_empty());
        fixture.finish().await;
    }
}

#[tokio::test]
async fn another_manager_authority_cannot_reuse_the_first_open_receiver() {
    let fixture = Fixture::new(3, 0, Failure::None).await;
    fixture.open().await.unwrap();
    let other_manager = TestAuthority::new(4);
    assert!(fixture.open_as(other_manager.clone()).await.is_err());
    assert!(!other_manager.is_current());
    assert!(fixture.authority.is_current());
    // The second task must negotiate under its own owner. A confirmation for
    // the first manager cannot authorize another Open or replace its live link.
    assert_eq!(fixture.counts.challenge.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.counts.confirm.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.counts.open.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.counts.websocket.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.transport.state.lock().await.links.len(), 1);
    fixture.finish().await;
}

#[tokio::test]
async fn open_receiver_renews_with_original_task_authority_until_retirement() {
    let fixture = Fixture::new(3, 0, Failure::None).await;
    fixture.open().await.unwrap();
    let another_task = TestAuthority::new(4);
    crate::owner_protocol::scope(another_task.clone(), async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while fixture.counts.confirm.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    })
    .await;
    assert_eq!(another_task.renewals.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.authority.inner.renewals.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.discoveries.load(Ordering::SeqCst), 1);
    fixture
        .transport
        .retire(&RuntimeHandle::new("runtime-1"))
        .await
        .unwrap();
    let count = fixture.counts.challenge.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(3300)).await;
    assert_eq!(fixture.counts.challenge.load(Ordering::SeqCst), count);
    fixture.finish().await;
}

#[tokio::test]
async fn receiver_renews_while_acp_setup_is_pending_and_stops_on_cancellation() {
    let fixture = Fixture::new(3, 0, Failure::Handshake).await;
    {
        let opening = fixture.open();
        tokio::pin!(opening);
        tokio::select! {
            result = &mut opening => panic!("ACP setup should remain pending: {}", result.is_ok()),
            result = tokio::time::timeout(Duration::from_secs(5), async {
                while fixture.counts.confirm.load(Ordering::SeqCst) < 2 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }) => result.expect("receiver renews while ACP is waiting"),
        }
        assert_eq!(fixture.counts.websocket.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.authority.inner.renewals.load(Ordering::SeqCst), 2);
        assert!(fixture.transport.state.lock().await.links.is_empty());
    }
    let count = fixture.counts.challenge.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(3300)).await;
    assert_eq!(fixture.counts.challenge.load(Ordering::SeqCst), count);
    fixture.finish().await;
}

#[tokio::test]
async fn cancelled_or_revoked_acp_setup_releases_the_receiver() {
    for failure in [Failure::Handshake, Failure::RevokeDuringHandshake] {
        let fixture = Fixture::new(3, 0, failure).await;
        let result = tokio::time::timeout(Duration::from_millis(500), fixture.open()).await;
        if failure == Failure::Handshake {
            assert!(result.is_err());
        } else {
            assert!(result.unwrap().is_err());
            assert!(!fixture.authority.is_current());
        }
        assert_eq!(fixture.counts.websocket.load(Ordering::SeqCst), 1);
        assert!(fixture.transport.state.lock().await.links.is_empty());
        let count = fixture.counts.challenge.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(3300)).await;
        assert_eq!(fixture.counts.challenge.load(Ordering::SeqCst), count);
        fixture.finish().await;
    }
}
