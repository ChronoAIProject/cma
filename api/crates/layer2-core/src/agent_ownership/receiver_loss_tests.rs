//! Coordinator cancellation and late-commit regressions; deterministic store only.
use super::*;
use layer3_adapter::owner_protocol::TransportAuthority;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

#[path = "receiver_loss_test_support.rs"]
mod support;
use support::Fixture;

#[tokio::test(start_paused = true)]
async fn receiver_check_cancellation_preserves_healthy_owners_and_unacknowledged_lease() {
    let fixture = Fixture::new(false).await;
    let original = fixture
        .store
        .owner(&fixture.cancelled.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    // Exercise the production adapter-authority entry: one failed store check
    // irrevocably cancels this authority, then the store becomes accessible.
    fixture.store.inner.partition(true);
    assert!(TransportAuthority::check(fixture.cancelled.as_ref())
        .await
        .is_err());
    fixture.store.inner.partition(false);
    assert!(!fixture.cancelled.is_current());
    fixture.store.check(&fixture.cancelled.token).await.unwrap();
    let cleanup = fixture.manager.maintain().await;
    assert!(
        cleanup.is_ok(),
        "a cancelled receiver must not fail healthy local owners: {:?}",
        cleanup.as_ref().err()
    );
    assert!(fixture
        .manager
        .cancelled_receivers()
        .await
        .iter()
        .any(|owner| Arc::ptr_eq(owner, &fixture.cancelled)));
    assert_eq!(
        fixture
            .store
            .cancelled_owner_renewals
            .load(Ordering::SeqCst),
        0,
        "pre-cancelled authority must not extend its durable expiry"
    );
    fixture.healthy_work().await;
    fixture
        .refuse_then_acknowledge(original.expires_at_ms)
        .await;
    assert_eq!(
        fixture
            .store
            .cancelled_owner_renewals
            .load(Ordering::SeqCst),
        0
    );
    // Containment must not turn real membership/store loss into availability.
    fixture.store.inner.partition(true);
    assert!(fixture.manager.maintain().await.is_err());
    assert!(!fixture.manager.is_ready());
    assert!(!fixture.healthy.is_current());
    fixture.peer.maintain().await.unwrap();
    assert!(fixture.peer_owner.is_current());
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_real_renewal_never_revives_or_releases_before_receiver_ack() {
    for renew_before_pause in [false, true] {
        let fixture = Fixture::new(renew_before_pause).await;
        let original = fixture
            .store
            .owner(&fixture.cancelled.token.agent_id)
            .await
            .unwrap()
            .unwrap();
        tokio::time::advance(Duration::from_secs(3)).await;
        fixture.store.pause_next.store(true, Ordering::SeqCst);
        let maintaining = tokio::spawn({
            let manager = fixture.manager.clone();
            async move { manager.maintain().await }
        });
        fixture.store.entered.notified().await;
        TransportAuthority::revoke(fixture.cancelled.as_ref());
        tokio::time::advance(Duration::from_secs(1)).await;
        fixture.store.resume.notify_one();
        let cleanup = maintaining.await.unwrap();
        assert!(
            cleanup.is_ok(),
            "in-flight renewal of cancelled receiver must not fail healthy owners: {:?}",
            cleanup.as_ref().err()
        );
        assert!(fixture
            .manager
            .cancelled_receivers()
            .await
            .iter()
            .any(|owner| Arc::ptr_eq(owner, &fixture.cancelled)));
        assert!(!fixture.cancelled.is_current());
        let actual = fixture
            .store
            .owner(&fixture.cancelled.token.agent_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.token, original.token);
        assert!(
            actual.expires_at_ms > original.expires_at_ms,
            "already-started durable renewal may finish despite local cancellation"
        );
        fixture.healthy_work().await;
        fixture.refuse_then_acknowledge(actual.expires_at_ms).await;
        assert_eq!(
            fixture
                .store
                .cancelled_owner_renewals
                .load(Ordering::SeqCst),
            1,
            "quarantine must prevent subsequent durable renewals"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn expired_observation_before_late_commit_never_reacquires_the_cancelled_token() {
    let fixture = Fixture::new(true).await;
    let original = fixture
        .store
        .owner(&fixture.cancelled.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    // Begin within the old lease. The renewal sees a valid token and computes a
    // later expiry, but its publication is held behind a deterministic barrier.
    tokio::time::advance(Duration::from_millis(DEFAULT_LEASE_MS - 1_000)).await;
    fixture.peer.maintain().await.unwrap();
    fixture.store.pause_next.store(true, Ordering::SeqCst);
    fixture
        .store
        .hide_renewal_until_publish
        .store(true, Ordering::SeqCst);
    let maintaining = tokio::spawn({
        let manager = fixture.manager.clone();
        async move { manager.maintain().await }
    });
    fixture.store.entered.notified().await;
    TransportAuthority::revoke(fixture.cancelled.as_ref());
    tokio::time::advance(Duration::from_millis(1_500)).await;
    assert!(fixture.store.membership().await.unwrap().now_ms > original.expires_at_ms);
    assert!(fixture
        .store
        .owner(&fixture.cancelled.token.agent_id)
        .await
        .unwrap()
        .is_none());
    // Absence at the old expiry cannot prove that the in-flight renewal aborted.
    // Removing this custody is the unsafe draft behavior the regression targets.
    assert!(
        !fixture
            .manager
            .forget_cancelled(&fixture.cancelled)
            .await
            .unwrap(),
        "expiry observation must retain a cancelled-token floor across an unknown renewal outcome"
    );
    fixture.store.resume.notify_one();
    assert!(maintaining.await.unwrap().is_ok());
    let committed = fixture
        .store
        .owner(&fixture.cancelled.token.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(committed.token, original.token);
    assert!(committed.expires_at_ms > fixture.store.membership().await.unwrap().now_ms);
    assert!(fixture.manager.acquire(&original.token.agent_id).await.unwrap().is_none(),
        "a late same-token renewal must never create a new current Authority for a cancelled receiver");
    assert!(!fixture.cancelled.is_current());
    assert!(fixture
        .manager
        .local(&original.token.agent_id)
        .await
        .is_none());
    assert!(fixture
        .manager
        .owned()
        .await
        .iter()
        .any(|owner| Arc::ptr_eq(owner, &fixture.cancelled)));
    fixture.healthy_work().await;
    fixture
        .refuse_then_acknowledge(committed.expires_at_ms)
        .await;
    let mut successor = None;
    for manager in [&fixture.manager, &fixture.peer] {
        if let Some(authority) = manager.acquire(&original.token.agent_id).await.unwrap() {
            assert!(successor.is_none());
            assert!(authority.token.epoch > original.token.epoch);
            successor = Some(authority);
        }
    }
    let successor = successor.expect("acknowledged release permits one strictly newer owner");
    // A delayed old cleanup cannot shorten or delete the successor's lease.
    fixture
        .manager
        .handoff(&fixture.cancelled, fixture.cancelled.revoke_transports())
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .owner(&original.token.agent_id)
            .await
            .unwrap()
            .unwrap()
            .token,
        successor.token
    );
    assert!(successor.is_current());
}

#[tokio::test(start_paused = true)]
async fn delayed_receiver_cancellation_cannot_relabel_generic_revocation_or_expiry() {
    for expires in [false, true] {
        let fixture = Fixture::new(false).await;
        if expires {
            tokio::time::advance(Duration::from_millis(DEFAULT_LEASE_MS)).await;
        } else {
            fixture.cancelled.revoke();
        }
        assert!(!fixture.cancelled.is_current());
        TransportAuthority::revoke(fixture.cancelled.as_ref());
        assert!(
            !fixture.cancelled.receiver_cancelled(),
            "a late receiver callback cannot make generic authority loss recoverable"
        );
        assert!(fixture.manager.maintain().await.is_err());
        assert!(!fixture.manager.is_ready());
        assert!(!fixture.healthy.is_current());
    }
}
