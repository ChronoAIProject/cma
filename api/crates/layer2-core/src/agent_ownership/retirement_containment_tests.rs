use super::containment_test_support::{manager, Receiver, Store};
use super::*;
use layer3_adapter::owner_protocol::TransportAuthority;
use std::{
    collections::BTreeSet,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

#[tokio::test(start_paused = true)]
async fn partial_ack_and_release_uncertainty_preserve_custody_and_recover() {
    for (intent, release_mode) in [RetirementIntent::Cooperative, RetirementIntent::Inactive]
        .into_iter()
        .flat_map(|intent| [1, 2].map(|mode| (intent, mode)))
    {
        let store = Arc::new(Store::default());
        let manager = manager(store.clone(), false, 2).await;
        let old = manager
            .acquire(&AgentId("old".into()))
            .await
            .unwrap()
            .unwrap();
        let healthy = manager
            .acquire(&AgentId("healthy".into()))
            .await
            .unwrap()
            .unwrap();
        let acked = Receiver::install(&old, "a", false);
        let missing = Receiver::install(&old, "b", true);
        old.begin_retirement(intent).unwrap();
        assert_eq!(manager.retirement_background(&old).await.unwrap(), None);
        assert_eq!(acked.calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.release_calls.load(Ordering::SeqCst), 0);
        assert_eq!(old.receivers.lock().unwrap().len(), 1);
        assert!(manager
            .acquire(&old.token.agent_id)
            .await
            .unwrap()
            .is_none());
        manager.maintain().await.unwrap();
        assert!(healthy.is_current());
        missing.fail.store(false, Ordering::SeqCst);
        store.release_mode.store(release_mode, Ordering::SeqCst);
        assert_eq!(manager.retirement_background(&old).await.unwrap(), None);
        assert!(old.receivers.lock().unwrap().is_empty());
        assert_eq!(acked.calls.load(Ordering::SeqCst), 1);
        assert_eq!(missing.calls.load(Ordering::SeqCst), 2);
        assert_eq!(manager.owned().await.len(), 2);
        store.release_mode.store(0, Ordering::SeqCst);
        assert_eq!(
            manager.retirement_background(&old).await.unwrap(),
            Some(Settlement::Released)
        );
        let next = manager.acquire(&old.token.agent_id).await.unwrap().unwrap();
        assert!(next.token.epoch > old.token.epoch);
        assert!(next.is_current());
        assert!(healthy.is_current());
        assert!(!old.is_current());
        let releases = store.release_calls.load(Ordering::SeqCst);
        assert_eq!(
            manager.retirement_background(&old).await.unwrap(),
            Some(Settlement::Released)
        );
        assert_eq!(store.release_calls.load(Ordering::SeqCst), releases);
        assert!(Arc::ptr_eq(
            &manager.local(&old.token.agent_id).await.unwrap(),
            &next
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_before_receiver_poll_keeps_retirement_reserved() {
    let store = Arc::new(Store::default());
    let manager = manager(store.clone(), false, 1).await;
    let old = manager
        .acquire(&AgentId("old".into()))
        .await
        .unwrap()
        .unwrap();
    let receiver = Receiver::install(&old, "receiver", false);
    let lock = old.cleanup.lock().await;
    let mut attempt = Box::pin(manager.retirement_background(&old));
    assert!(futures_util::poll!(&mut attempt).is_pending());
    assert!(old.retirement_pending());
    assert!(!old.is_current());
    assert_eq!(receiver.calls.load(Ordering::SeqCst), 0);
    drop(attempt);
    drop(lock);
    assert_eq!(manager.owned().await.len(), 1);
    assert!(manager
        .acquire(&AgentId("another".into()))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        manager.retirement_background(&old).await.unwrap(),
        Some(Settlement::Released)
    );
    assert_eq!(receiver.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn local_join_timeout_cannot_settle_or_publish_a_successor() {
    let store = Arc::new(Store::default());
    let manager = manager(store.clone(), false, 1).await;
    let old = manager
        .acquire(&AgentId("old".into()))
        .await
        .unwrap()
        .unwrap();
    let receiver = Receiver::install(&old, "receiver", false);
    let work = old.admit_work().unwrap();
    let start = tokio::time::Instant::now();
    assert_eq!(manager.retirement_background(&old).await.unwrap(), None);
    assert_eq!(start.elapsed(), Duration::from_secs(2));
    assert!(!old.cleanup.lock().await.local_quiescent);
    assert_eq!(receiver.calls.load(Ordering::SeqCst), 0);
    assert_eq!(store.release_calls.load(Ordering::SeqCst), 0);
    assert!(manager
        .acquire(&old.token.agent_id)
        .await
        .unwrap()
        .is_none());
    drop(work);
    assert_eq!(
        manager.retirement_background(&old).await.unwrap(),
        Some(Settlement::Released)
    );
}

#[tokio::test(start_paused = true)]
async fn nine_mixed_pending_slots_rotate_despite_failed_and_deferred_attempts() {
    let store = Arc::new(Store::default());
    let manager = manager(store, true, 10).await;
    let healthy = manager
        .acquire(&AgentId("healthy".into()))
        .await
        .unwrap()
        .unwrap();
    let mut expected = BTreeSet::new();
    for index in 0..9 {
        let old = manager
            .acquire(&AgentId(format!("pending-{index}")))
            .await
            .unwrap()
            .unwrap();
        Receiver::install(&old, "receiver", true);
        if index % 2 == 0 {
            TransportAuthority::revoke(old.as_ref());
        } else {
            old.begin_retirement(RetirementIntent::Inactive).unwrap();
        }
        expected.insert(old.token.agent_id.clone());
    }
    let mut seen = BTreeSet::new();
    for _ in 0..6 {
        let batch = manager.cancelled_receivers().await;
        assert_eq!(batch.len(), 4);
        for old in batch {
            seen.insert(old.token.agent_id.clone());
            assert_eq!(manager.retirement_background(&old).await.unwrap(), None);
        }
        manager.maintain().await.unwrap();
        assert!(healthy.is_current());
    }
    assert_eq!(seen, expected);
    assert_eq!(manager.owned().await.len(), 10);
    assert!(manager
        .acquire(&AgentId("capacity".into()))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(start_paused = true)]
async fn newer_observation_settles_without_ack_or_old_release_and_rejects_bad_identity() {
    for invalid in [None, Some("agent"), Some("protocol"), Some("instance")] {
        let store = Arc::new(Store::default());
        let manager = manager(store.clone(), false, 2).await;
        let old = manager
            .acquire(&AgentId("old".into()))
            .await
            .unwrap()
            .unwrap();
        let receiver = Receiver::install(&old, "receiver", true);
        assert_eq!(manager.retirement_background(&old).await.unwrap(), None);
        // A committed old release whose reply was lost, followed by real CAS,
        // supplies an actual strictly newer durable token for this fixture.
        store.inner.release(&old.token).await.unwrap();
        let mut next = store
            .inner
            .acquire(&old.token.agent_id, &manager.instance_id)
            .await
            .unwrap()
            .unwrap();
        assert!(next.token.epoch > old.token.epoch);
        match invalid {
            Some("agent") => next.token.agent_id = AgentId("other".into()),
            Some("protocol") => next.token.protocol += 1,
            Some("instance") => next.token.instance_id.clear(),
            _ => (),
        }
        *store.observation.lock().unwrap() = Some(Some(next.clone()));
        let result = manager.retirement_background(&old).await;
        if invalid.is_some() {
            assert!(matches!(result, Err(OwnershipError::Invalid(_))));
            assert_eq!(manager.owned().await.len(), 1);
        } else {
            assert_eq!(
                result.unwrap(),
                Some(Settlement::Superseded {
                    newer_epoch: next.token.epoch
                })
            );
            assert!(manager.owned().await.is_empty());
        }
        assert_eq!(store.release_calls.load(Ordering::SeqCst), 0);
        assert_eq!(receiver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(old.receivers.lock().unwrap().len(), 1);
        assert!(!old.is_current());
    }
}

#[tokio::test(start_paused = true)]
async fn absence_keeps_the_old_floor_and_generic_revocation_is_not_retirement() {
    let store = Arc::new(Store::default());
    let manager = manager(store.clone(), false, 2).await;
    let old = manager
        .acquire(&AgentId("old".into()))
        .await
        .unwrap()
        .unwrap();
    let generic = manager
        .acquire(&AgentId("generic".into()))
        .await
        .unwrap()
        .unwrap();
    Receiver::install(&old, "receiver", true);
    assert_eq!(manager.retirement_background(&old).await.unwrap(), None);
    *store.observation.lock().unwrap() = Some(None);
    assert_eq!(manager.retirement_background(&old).await.unwrap(), None);
    let reads = store.owner_calls.load(Ordering::SeqCst);
    assert!(!manager.forget_unconfirmed_retirement(&old).await.unwrap());
    assert_eq!(store.owner_calls.load(Ordering::SeqCst), reads);
    assert!(old.cleanup.lock().await.settlement.is_none());
    assert!(!old.cleanup.lock().await.legacy_forgotten);
    assert_eq!(manager.owned().await.len(), 2);
    assert!(manager
        .acquire(&old.token.agent_id)
        .await
        .unwrap()
        .is_none());
    generic.revoke();
    assert!(!generic.retirement_pending());
    assert_eq!(
        generic.begin_retirement(RetirementIntent::Inactive),
        Err(OwnershipError::Stale)
    );
    assert!(matches!(
        manager.acquire(&generic.token.agent_id).await,
        Err(OwnershipError::Stale)
    ));
}
