use super::containment_test_support::{manager, Receiver, Store};
use super::*;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::time::Instant;

#[tokio::test(start_paused = true)]
async fn shutdown_fences_generic_authority_and_requires_local_join_before_receiver_release() {
    for pacing in [false, true] {
        let store = Arc::new(Store::default());
        let manager = manager(store.clone(), pacing, 1).await;
        let authority = manager
            .acquire(&AgentId("draining".into()))
            .await
            .unwrap()
            .unwrap();
        let receiver = Receiver::install(&authority, "receiver", false);
        let work = authority.admit_work().unwrap();
        manager.stop_admission();
        assert!(!authority.is_current());
        assert!(
            !authority.retirement_pending(),
            "generic shutdown is not a recovery proof"
        );
        manager
            .prepare_drain(Instant::now() + Duration::from_secs(2))
            .await
            .unwrap();
        assert!(manager
            .drain_authority_until(&authority, Instant::now() + Duration::from_secs(6))
            .await
            .is_err());
        assert_eq!(receiver.calls.load(Ordering::Acquire), 0);
        assert_eq!(store.release_calls.load(Ordering::Acquire), 0);
        drop(work);
        assert_eq!(
            manager
                .drain_authority_until(&authority, Instant::now() + Duration::from_secs(6))
                .await
                .unwrap(),
            Settlement::Released
        );
        assert_eq!(receiver.calls.load(Ordering::Acquire), 1);
        assert_eq!(store.release_calls.load(Ordering::Acquire), 1);
        assert!(manager.owned().await.is_empty());
        assert_eq!(
            authority.begin_retirement(RetirementIntent::Cooperative),
            Err(OwnershipError::Stale)
        );
    }
}

#[tokio::test(start_paused = true)]
async fn drain_accepts_only_valid_newer_owner_proof_without_ack_or_old_release() {
    for invalid in [None, Some("agent"), Some("protocol"), Some("deadline")] {
        let store = Arc::new(Store::default());
        let manager = manager(store.clone(), false, 1).await;
        let old = manager
            .acquire(&AgentId("replaced".into()))
            .await
            .unwrap()
            .unwrap();
        let receiver = Receiver::install(&old, "receiver", true);
        store.inner.release(&old.token).await.unwrap();
        let mut next = store
            .inner
            .acquire(&old.token.agent_id, &manager.instance_id)
            .await
            .unwrap()
            .unwrap();
        match invalid {
            Some("agent") => next.token.agent_id = AgentId("different".into()),
            Some("protocol") => next.token.protocol += 1,
            Some("deadline") => next.expires_at_ms = 0,
            _ => (),
        }
        *store.observation.lock().unwrap() = Some(Some(next.clone()));
        manager.stop_admission();
        manager
            .prepare_drain(Instant::now() + Duration::from_secs(2))
            .await
            .unwrap();
        let result = manager
            .drain_authority_until(&old, Instant::now() + Duration::from_secs(6))
            .await;
        if invalid.is_some() {
            assert!(matches!(result, Err(OwnershipError::Invalid(_))));
            assert_eq!(manager.owned().await.len(), 1);
        } else {
            assert_eq!(
                result,
                Ok(Settlement::Superseded {
                    newer_epoch: next.token.epoch
                })
            );
            assert!(manager.owned().await.is_empty());
        }
        assert_eq!(receiver.calls.load(Ordering::Acquire), 0);
        assert_eq!(store.release_calls.load(Ordering::Acquire), 0);
        assert!(!old.is_current());
    }
}

#[tokio::test(start_paused = true)]
async fn missing_ack_and_uncertain_release_remain_incomplete_and_preserve_partial_progress() {
    let store = Arc::new(Store::default());
    let manager = manager(store.clone(), false, 1).await;
    let old = manager
        .acquire(&AgentId("incomplete".into()))
        .await
        .unwrap()
        .unwrap();
    let acked = Receiver::install(&old, "a", false);
    let missing = Receiver::install(&old, "b", true);
    manager.stop_admission();
    manager
        .prepare_drain(Instant::now() + Duration::from_secs(2))
        .await
        .unwrap();
    assert!(manager
        .drain_authority_until(&old, Instant::now() + Duration::from_secs(6))
        .await
        .is_err());
    assert_eq!(store.release_calls.load(Ordering::Acquire), 0);
    assert_eq!(acked.calls.load(Ordering::Acquire), 1);
    missing.fail.store(false, Ordering::Release);
    store.release_mode.store(2, Ordering::Release);
    assert!(manager
        .drain_authority_until(&old, Instant::now() + Duration::from_secs(6))
        .await
        .is_err());
    assert_eq!(manager.owned().await.len(), 1);
    assert!(store
        .inner
        .owner(&old.token.agent_id)
        .await
        .unwrap()
        .is_none());
    store.release_mode.store(0, Ordering::Release);
    assert_eq!(
        manager
            .drain_authority_until(&old, Instant::now() + Duration::from_secs(6))
            .await
            .unwrap(),
        Settlement::Released
    );
    assert_eq!(acked.calls.load(Ordering::Acquire), 1);
    assert_eq!(missing.calls.load(Ordering::Acquire), 2);
    assert!(manager.owned().await.is_empty());
}
