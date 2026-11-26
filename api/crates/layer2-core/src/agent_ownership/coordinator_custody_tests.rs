use super::*;
use crate::agent_ownership::containment_test_support::{manager, Receiver};
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::time::Instant;

fn paused(before_pause: bool, acquisition: bool) -> Arc<PausedRenew> {
    Arc::new(PausedRenew {
        inner: Arc::new(MemoryOwnershipStore::default()),
        entered: Notify::new(),
        resume: Notify::new(),
        before_pause,
        pause_acquire: AtomicBool::new(acquisition),
        pause_owner: false,
    })
}

#[tokio::test(start_paused = true)]
async fn abandoned_acquisition_before_or_after_commit_keeps_capacity_and_fences_late_result() {
    for before_pause in [false, true] {
        let store = paused(before_pause, true);
        let coordinator = manager(store.clone(), false, 1).await;
        let id = AgentId("abandoned".into());
        let caller = tokio::spawn({
            let coordinator = coordinator.clone();
            let id = id.clone();
            async move { coordinator.acquire(&id).await }
        });
        store.entered.notified().await;
        assert_eq!(
            store.inner.owner(&id).await.unwrap().is_some(),
            before_pause
        );
        caller.abort();
        assert!(caller.await.err().unwrap().is_cancelled());
        assert!(coordinator
            .acquire(&AgentId("other".into()))
            .await
            .unwrap()
            .is_none());
        assert!(coordinator
            .prepare_drain(Instant::now() + Duration::from_millis(250))
            .await
            .is_err());
        assert!(coordinator.local(&id).await.is_none());
        store.resume.notify_one();
        coordinator
            .prepare_drain(Instant::now() + Duration::from_secs(2))
            .await
            .unwrap();
        let custody = coordinator.owned().await;
        assert_eq!(custody.len(), 1);
        assert_eq!(custody[0].token.agent_id, id);
        assert!(!custody[0].is_current());
        assert!(custody[0].retirement_pending());
        assert_eq!(
            store.inner.owner(&id).await.unwrap().unwrap().token,
            custody[0].token
        );
    }
}

#[tokio::test(start_paused = true)]
async fn abandoned_successor_keeps_old_floor_until_known_fenced_replacement() {
    let store = paused(true, false);
    let coordinator = manager(store.clone(), false, 1).await;
    let id = AgentId("successor".into());
    let old = coordinator.acquire(&id).await.unwrap().unwrap();
    let receiver = Receiver::install(&old, "receiver", true);
    assert_eq!(coordinator.retirement_background(&old).await.unwrap(), None);
    assert!(old.cleanup.lock().await.local_quiescent);
    store.inner.release(&old.token).await.unwrap();
    store.pause_acquire.store(true, Ordering::Release);
    let caller = tokio::spawn({
        let coordinator = coordinator.clone();
        let id = id.clone();
        async move { coordinator.acquire(&id).await }
    });
    store.entered.notified().await;
    caller.abort();
    assert!(caller.await.err().unwrap().is_cancelled());
    assert!(Arc::ptr_eq(&coordinator.owned().await[0], &old));
    store.resume.notify_one();
    coordinator
        .prepare_drain(Instant::now() + Duration::from_secs(2))
        .await
        .unwrap();
    let next = coordinator.owned().await.pop().unwrap();
    assert!(next.token.epoch > old.token.epoch);
    assert!(!next.is_current());
    assert_eq!(
        old.cleanup.lock().await.settlement,
        Some(Settlement::Superseded {
            newer_epoch: next.token.epoch
        })
    );
    assert_eq!(receiver.calls.load(Ordering::Acquire), 1);
    assert_eq!(
        coordinator.retirement_background(&old).await.unwrap(),
        Some(Settlement::Superseded {
            newer_epoch: next.token.epoch
        })
    );
    assert!(Arc::ptr_eq(&coordinator.owned().await[0], &next));
}

#[tokio::test(start_paused = true)]
async fn actual_renewal_survives_waiter_cancellation_and_precedes_release() {
    for before_pause in [false, true] {
        let store = paused(before_pause, false);
        let coordinator = manager(store.clone(), false, 1).await;
        let id = AgentId("renewing".into());
        let old = coordinator.acquire(&id).await.unwrap().unwrap();
        Receiver::install(&old, "receiver", false);
        let caller = tokio::spawn({
            let old = old.clone();
            async move { old.renew_store_owned().await }
        });
        store.entered.notified().await;
        caller.abort();
        assert!(caller.await.err().unwrap().is_cancelled());
        assert_eq!(coordinator.retirement_background(&old).await.unwrap(), None);
        assert!(store.inner.owner(&id).await.unwrap().is_some());
        store.resume.notify_one();
        assert_eq!(
            coordinator.retirement_background(&old).await.unwrap(),
            Some(Settlement::Released)
        );
        assert!(store.inner.owner(&id).await.unwrap().is_none());
        assert!(!old.is_current());
    }
}

#[tokio::test(start_paused = true)]
async fn newer_successor_observes_old_write_termination_before_replacing_custody() {
    let store = paused(false, false);
    let coordinator = manager(store.clone(), false, 1).await;
    let id = AgentId("newer".into());
    let old = coordinator.acquire(&id).await.unwrap().unwrap();
    Receiver::install(&old, "receiver", true);
    let caller = tokio::spawn({
        let old = old.clone();
        async move { old.renew_store_owned().await }
    });
    store.entered.notified().await;
    caller.abort();
    assert!(caller.await.err().unwrap().is_cancelled());
    assert_eq!(coordinator.retirement_background(&old).await.unwrap(), None);
    store.inner.release(&old.token).await.unwrap();
    let next = coordinator.acquire(&id).await.unwrap().unwrap();
    assert!(next.token.epoch > old.token.epoch);
    assert!(next.is_current());
    assert!(!old.is_current());
    // Aborting the store future must have been joined and cached, rather than
    // depending on dropping the old authority (which is still held here).
    let operation = old.renewal.lock().unwrap().clone().unwrap();
    assert!(operation.completed().is_some());
    store.resume.notify_one();
    assert_eq!(
        store.inner.owner(&id).await.unwrap().unwrap().token,
        next.token
    );
}

#[tokio::test(start_paused = true)]
async fn same_member_reclaims_at_capacity_after_expiry_without_releasing_old_token() {
    for pacing in [false, true] {
        let store = Arc::new(crate::agent_ownership::containment_test_support::Store::default());
        let coordinator = manager(store.clone(), pacing, 2).await;
        let old = coordinator
            .acquire(&AgentId("pending".into()))
            .await
            .unwrap()
            .unwrap();
        let healthy = coordinator
            .acquire(&AgentId("healthy".into()))
            .await
            .unwrap()
            .unwrap();
        Receiver::install(&old, "receiver", true);
        assert_eq!(coordinator.retirement_background(&old).await.unwrap(), None);
        for _ in 0..6 {
            tokio::time::advance(Duration::from_secs(3)).await;
            coordinator.maintain().await.unwrap();
            assert!(healthy.is_current());
        }
        let next = coordinator
            .acquire(&old.token.agent_id)
            .await
            .unwrap()
            .unwrap();
        assert!(next.token.epoch > old.token.epoch);
        assert!(next.is_current());
        assert!(!old.is_current());
        assert_eq!(coordinator.owned().await.len(), 2);
        assert_eq!(store.release_calls.load(Ordering::Acquire), 0);
        assert_eq!(
            old.cleanup.lock().await.settlement,
            Some(Settlement::Superseded {
                newer_epoch: next.token.epoch
            })
        );
    }
}
