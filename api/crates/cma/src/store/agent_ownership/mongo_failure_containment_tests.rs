use super::*;
use layer2_core::agent_ownership::{
    Authority, MaintenancePolicy, OwnershipCoordinator, Settlement,
};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::time::Instant;

mod acquisition_custody;
mod durable;
mod support;
use support::{assigned, close, manager, Receiver, Store};

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn committed_acquisition_withheld_from_cancelled_caller_remains_in_drain_custody() {
    for pacing in [false, true] {
        let store = Store::new().await;
        let a = manager(store.clone(), "member-a", pacing, 2).await;
        let b = manager(store.inner.clone(), "member-b", pacing, 2).await;
        let membership = store.membership().await.unwrap();
        let id = assigned(&membership, "member-a", "held-acquisition");
        store.hold_acquire.store(true, Ordering::Release);
        let caller = tokio::spawn({
            let a = a.clone();
            let id = id.clone();
            async move { a.acquire(&id).await }
        });
        tokio::time::timeout(Duration::from_secs(2), store.entered.notified())
            .await
            .unwrap();
        let committed = store.inner.owner(&id).await.unwrap().unwrap();
        caller.abort();
        assert!(caller.await.err().unwrap().is_cancelled());
        a.stop_admission();
        assert!(a
            .prepare_drain(Instant::now() + Duration::from_millis(200))
            .await
            .is_err());
        assert!(b.acquire(&id).await.unwrap().is_none());
        assert!(a.local(&id).await.is_none());
        store.resume.notify_one();
        a.prepare_drain(Instant::now() + Duration::from_secs(2))
            .await
            .unwrap();
        let retained = a.owned().await;
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].token, committed.token);
        assert!(!retained[0].is_current());
        assert_eq!(
            a.drain_authority_until(&retained[0], Instant::now() + Duration::from_secs(6))
                .await
                .unwrap(),
            Settlement::Released
        );
        let next = b.acquire(&id).await.unwrap().unwrap();
        assert!(next.token.epoch > committed.token.epoch);
        store.inner.check(&next.token).await.unwrap();
        close(&store, &[a, b]).await;
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn same_member_recovers_at_capacity_without_old_ack_and_old_renewal_cannot_cross_epoch() {
    for pacing in [false, true] {
        let store = Store::new().await;
        let a = manager(store.clone(), "member-a", pacing, 2).await;
        let b = manager(store.inner.clone(), "member-b", pacing, 1).await;
        let membership = store.membership().await.unwrap();
        let id = assigned(&membership, "member-a", "recovering");
        let old = a.acquire(&id).await.unwrap().unwrap();
        let healthy_id = assigned(&membership, "member-a", "healthy-local");
        let healthy = a.acquire(&healthy_id).await.unwrap().unwrap();
        let peer_id = assigned(&membership, "member-b", "healthy-peer");
        let peer = b.acquire(&peer_id).await.unwrap().unwrap();
        let receiver = Receiver::install(&old);
        store.hold_renew.store(true, Ordering::Release);
        let renewing = tokio::spawn({
            let old = old.clone();
            async move {
                layer3_adapter::owner_protocol::TransportAuthority::renew_for_challenge(
                    old.as_ref(),
                )
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), store.entered.notified())
            .await
            .unwrap();
        renewing.abort();
        assert!(renewing.await.unwrap_err().is_cancelled());
        store.hold_renew.store(false, Ordering::Release);
        assert_eq!(a.retirement_background(&old).await.unwrap(), None);
        assert!(a.acquire(&id).await.unwrap().is_none());
        store.expired(&old.token).await;
        // Pacing deferral is real, bounded, and leaves the other owners live.
        let next = tokio::time::timeout(Duration::from_secs(6), async {
            loop {
                a.maintain().await.unwrap();
                b.maintain().await.unwrap();
                assert!(healthy.is_current() && peer.is_current());
                store.inner.check(&healthy.token).await.unwrap();
                store.inner.check(&peer.token).await.unwrap();
                if let Some(next) = a.acquire(&id).await.unwrap() {
                    break next;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        assert!(next.token.epoch > old.token.epoch);
        store.resume.notify_one();
        assert!(!old.is_current());
        assert_eq!(
            store.inner.renew(&old.token).await,
            Err(OwnershipError::Stale)
        );
        assert_eq!(store.releases.load(Ordering::Acquire), 0);
        assert_eq!(receiver.calls.load(Ordering::Acquire), 1);
        assert_eq!(
            a.retirement_background(&old).await.unwrap(),
            Some(Settlement::Superseded {
                newer_epoch: next.token.epoch
            })
        );
        assert!(Arc::ptr_eq(&a.local(&id).await.unwrap(), &next));
        assert!(b.acquire(&id).await.unwrap().is_none());
        close(&store, &[a, b]).await;
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn lost_committed_release_reply_retains_responsibility_and_exact_retry_recovers() {
    for pacing in [false, true] {
        let store = Store::new().await;
        let a = manager(store.clone(), "member-a", pacing, 2).await;
        let old = a
            .acquire(&AgentId("lost-release".into()))
            .await
            .unwrap()
            .unwrap();
        let healthy = a
            .acquire(&AgentId("healthy".into()))
            .await
            .unwrap()
            .unwrap();
        let receiver = Receiver::install(&old);
        receiver.acknowledges.store(true, Ordering::Release);
        store.lose_release.store(true, Ordering::Release);
        assert_eq!(a.retirement_background(&old).await.unwrap(), None);
        assert!(store
            .inner
            .owner(&old.token.agent_id)
            .await
            .unwrap()
            .is_none());
        assert_eq!(a.owned().await.len(), 2);
        a.maintain().await.unwrap();
        assert!(healthy.is_current());
        store.lose_release.store(false, Ordering::Release);
        a.stop_admission();
        a.prepare_drain(Instant::now() + Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(
            a.drain_authority_until(&old, Instant::now() + Duration::from_secs(6))
                .await
                .unwrap(),
            Settlement::Released
        );
        assert_eq!(receiver.calls.load(Ordering::Acquire), 1);
        assert_eq!(store.releases.load(Ordering::Acquire), 2);
        close(&store, &[a]).await;
    }
}
