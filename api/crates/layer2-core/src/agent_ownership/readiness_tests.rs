//! Readiness admission and termination races through the real coordinator.
//! Store barriers model committed writes with a delayed or lost response.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

#[path = "readiness_test_support.rs"]
mod support;
use support::{assert_current, member, preferred_agent, HeldStore};

#[tokio::test(start_paused = true)]
async fn prepared_manager_never_joins_placement_or_owns_work_while_peer_renews() {
    let store = Arc::new(HeldStore::default());
    let peer = OwnershipCoordinator::register(store.inner.client(), member("peer"))
        .await
        .unwrap();
    let id = AgentId("existing-agent".into());
    let healthy = peer.acquire(&id).await.unwrap().unwrap();
    let starting = OwnershipCoordinator::prepare(store.clone(), member("starting"));
    for _ in 0..6 {
        tokio::time::advance(Duration::from_millis(HEARTBEAT_INTERVAL_MS)).await;
        assert!(starting.is_starting());
        assert!(!starting.is_ready());
        assert!(starting.maintain().await.unwrap().is_empty());
        assert!(starting.acquire(&id).await.is_err());
        assert!(starting
            .acquire(&AgentId("new-agent".into()))
            .await
            .is_err());
        assert!(starting.local(&id).await.is_none());
        assert!(starting.owned().await.is_empty());
        assert!(!*starting.failure_signal().borrow());
        assert!(peer.maintain().await.unwrap().is_empty());
        assert_current(&healthy, store.inner.as_ref()).await;
        let membership = store.inner.membership().await.unwrap();
        assert_eq!(membership.members.len(), 1);
        assert_eq!(
            candidates(&id, &membership, None).unwrap()[0].instance_id,
            "peer"
        );
    }
    assert_eq!(store.registrations.load(Ordering::SeqCst), 0);
    assert_eq!(store.heartbeats.load(Ordering::SeqCst), 0);
    assert_eq!(store.acquisitions.load(Ordering::SeqCst), 0);
    starting.activate().await.unwrap();
    assert!(starting.is_ready());
    assert!(!starting.is_starting());
    let own_id = preferred_agent(store.as_ref(), "starting").await;
    let authority = starting.acquire(&own_id).await.unwrap().unwrap();
    assert_current(&authority, store.as_ref()).await;
    assert_current(&healthy, store.inner.as_ref()).await;
    assert_eq!(store.registrations.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn termination_or_failure_before_activation_cannot_be_reopened_by_late_startup() {
    for failed in [false, true] {
        let store = Arc::new(HeldStore::default());
        let manager = OwnershipCoordinator::prepare(store.clone(), member("starting"));
        if failed {
            manager.fail().await;
        } else {
            manager.stop_admission();
        }
        assert!(!manager.is_starting());
        assert!(manager.activate().await.is_err());
        assert!(manager.begin_drain().await.is_empty());
        assert!(manager.activate().await.is_err());
        assert!(manager.acquire(&AgentId("agent".into())).await.is_err());
        assert!(!manager.is_ready());
        assert!(store.inner.membership().await.unwrap().members.is_empty());
        assert_eq!(store.registrations.load(Ordering::SeqCst), 0);
        assert_eq!(store.heartbeats.load(Ordering::SeqCst), 0);
        assert_eq!(store.acquisitions.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn termination_during_committed_registration_never_activates_or_leaves_a_claimant() {
    let store = Arc::new(HeldStore::default());
    store.registration.arm();
    let manager = OwnershipCoordinator::prepare(store.clone(), member("starting"));
    let activating = tokio::spawn({
        let manager = manager.clone();
        async move { manager.activate().await }
    });
    store.registration.entered.notified().await;
    assert!(manager.is_starting());
    assert!(manager.acquire(&AgentId("agent".into())).await.is_err());
    manager.stop_admission();
    let draining = manager.begin_drain();
    tokio::pin!(draining);
    assert!(futures_util::poll!(&mut draining).is_pending());
    store.registration.resume.notify_one();
    let (activated, owned) = tokio::join!(activating, draining);
    assert!(activated.unwrap().is_err());
    assert!(owned.is_empty());
    assert!(!manager.is_ready());
    assert!(manager.activate().await.is_err());
    let membership = store.inner.membership().await.unwrap();
    assert!(membership
        .members
        .iter()
        .all(|member| !member.can_claim(membership.now_ms)));
    assert_eq!(store.registrations.load(Ordering::SeqCst), 1);
    assert_eq!(store.acquisitions.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn live_readiness_blocks_new_acquisition_without_revoking_retained_peer_work() {
    let store = Arc::new(HeldStore::default());
    let manager = OwnershipCoordinator::prepare(store.clone(), member("manager"));
    let ready = Arc::new(AtomicBool::new(false));
    manager
        .set_admission_readiness({
            let ready = ready.clone();
            Arc::new(move || ready.load(Ordering::SeqCst))
        })
        .unwrap();
    assert!(manager.activate().await.is_err());
    assert_eq!(store.registrations.load(Ordering::SeqCst), 0);
    ready.store(true, Ordering::SeqCst);
    manager.activate().await.unwrap();
    let authority = manager
        .acquire(&AgentId("existing".into()))
        .await
        .unwrap()
        .unwrap();
    ready.store(false, Ordering::SeqCst);
    for _ in 0..6 {
        tokio::time::advance(Duration::from_millis(HEARTBEAT_INTERVAL_MS)).await;
        assert!(manager.acquire(&authority.token.agent_id).await.is_err());
        assert!(manager.acquire(&AgentId("new".into())).await.is_err());
        manager.maintain().await.unwrap();
        assert_current(&authority, store.as_ref()).await;
    }
    assert_eq!(store.acquisitions.load(Ordering::SeqCst), 1);
    ready.store(true, Ordering::SeqCst);
    assert!(Arc::ptr_eq(
        &authority,
        &manager
            .acquire(&authority.token.agent_id)
            .await
            .unwrap()
            .unwrap()
    ));
    manager.stop_admission();
    assert!(manager.acquire(&AgentId("new".into())).await.is_err());
    assert!(manager.activate().await.is_err());
}

#[tokio::test(start_paused = true)]
async fn readiness_lost_during_acquisition_never_publishes_the_committed_authority() {
    let store = Arc::new(HeldStore::default());
    let manager = OwnershipCoordinator::prepare(store.clone(), member("manager"));
    let ready = Arc::new(AtomicBool::new(true));
    manager
        .set_admission_readiness({
            let ready = ready.clone();
            Arc::new(move || ready.load(Ordering::SeqCst))
        })
        .unwrap();
    manager.activate().await.unwrap();
    let id = AgentId("readiness-race".into());
    store.acquisition.arm();
    let acquiring = tokio::spawn({
        let manager = manager.clone();
        let id = id.clone();
        async move { manager.acquire(&id).await }
    });
    store.acquisition.entered.notified().await;
    let committed = store.inner.owner(&id).await.unwrap().unwrap();
    ready.store(false, Ordering::SeqCst);
    store.acquisition.resume.notify_one();
    assert!(acquiring.await.unwrap().is_err());
    assert!(manager.local(&id).await.is_none());
    let owned = manager.begin_drain().await;
    assert_eq!(owned.len(), 1);
    assert_eq!(owned[0].token, committed.token);
    assert!(!owned[0].is_current());
    assert_eq!(store.releases.load(Ordering::SeqCst), 0);
    manager.handoff(&owned[0], async { Ok(()) }).await.unwrap();
    assert!(store.inner.owner(&id).await.unwrap().is_none());
    ready.store(true, Ordering::SeqCst);
    assert!(manager.activate().await.is_err());
    assert!(manager.acquire(&id).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn drain_collects_late_committed_acquisition_and_releases_only_after_exact_ack() {
    let store = Arc::new(HeldStore::default());
    let manager = OwnershipCoordinator::register(store.clone(), member("manager"))
        .await
        .unwrap();
    let existing = manager
        .acquire(&AgentId("existing".into()))
        .await
        .unwrap()
        .unwrap();
    let id = AgentId("late".into());
    store.acquisition.arm();
    let acquiring = tokio::spawn({
        let manager = manager.clone();
        let id = id.clone();
        async move { manager.acquire(&id).await }
    });
    store.acquisition.entered.notified().await;
    let committed = store.inner.owner(&id).await.unwrap().unwrap();
    manager.maintain().await.unwrap();
    assert_current(&existing, store.as_ref()).await;
    manager.stop_admission();
    let draining = manager.begin_drain();
    tokio::pin!(draining);
    let waited = futures_util::poll!(&mut draining).is_pending();
    store.acquisition.resume.notify_one();
    let (acquired, owned) = tokio::join!(acquiring, draining);
    assert!(acquired.unwrap().is_err());
    assert!(
        waited,
        "drain must join an acquisition whose durable token already committed"
    );
    assert_eq!(
        owned.len(),
        2,
        "drain must retain late committed custody for exact handoff"
    );
    assert!(owned.iter().all(|authority| !authority.is_current()));
    let late = owned
        .iter()
        .find(|authority| authority.token == committed.token)
        .unwrap();
    assert!(manager.local(&id).await.is_none());
    assert!(late
        .run(async { panic!("late authority must never execute") })
        .await
        .is_err());
    assert!(manager
        .handoff(late, async {
            Err(OwnershipError::Unavailable("receiver ACK lost".into()))
        })
        .await
        .is_err());
    assert_eq!(store.releases.load(Ordering::SeqCst), 0);
    assert_eq!(store.inner.owner(&id).await.unwrap().unwrap(), committed);
    store.fail_release.store(true, Ordering::SeqCst);
    assert!(manager.handoff(late, async { Ok(()) }).await.is_err());
    assert_eq!(store.inner.owner(&id).await.unwrap().unwrap(), committed);
    assert!(manager
        .owned()
        .await
        .iter()
        .any(|authority| Arc::ptr_eq(authority, late)));
    store.fail_release.store(false, Ordering::SeqCst);
    for authority in &owned {
        manager.handoff(authority, async { Ok(()) }).await.unwrap();
    }
    assert!(manager.owned().await.is_empty());
    assert!(store.inner.owner(&id).await.unwrap().is_none());
    let peer = OwnershipCoordinator::register(store.inner.client(), member("peer"))
        .await
        .unwrap();
    let successor = peer.acquire(&id).await.unwrap().unwrap();
    assert!(successor.token.epoch > committed.token.epoch);
    manager
        .handoff(late, async {
            panic!("completed handoff must not repeat receiver revocation")
        })
        .await
        .unwrap();
    assert_current(&successor, store.inner.as_ref()).await;
    assert_eq!(
        store.inner.owner(&id).await.unwrap().unwrap().token,
        successor.token
    );
}

#[tokio::test(start_paused = true)]
async fn unknown_acquisition_outcome_retains_lease_until_expiry_without_unsafe_release() {
    let store = Arc::new(HeldStore::default());
    let manager = OwnershipCoordinator::register(store.clone(), member("manager"))
        .await
        .unwrap();
    let id = AgentId("unknown".into());
    store.acquisition.arm();
    let acquiring = tokio::spawn({
        let manager = manager.clone();
        let id = id.clone();
        async move { manager.acquire(&id).await }
    });
    store.acquisition.entered.notified().await;
    let committed = store.inner.owner(&id).await.unwrap().unwrap();
    manager.stop_admission();
    tokio::time::advance(Duration::from_millis(STORE_DEADLINE_MS + 1)).await;
    assert!(acquiring.await.unwrap().is_err());
    assert!(manager.begin_drain().await.is_empty());
    assert_eq!(store.releases.load(Ordering::SeqCst), 0);
    assert_eq!(store.inner.owner(&id).await.unwrap().unwrap(), committed);
    let peer = OwnershipCoordinator::register(store.inner.client(), member("peer"))
        .await
        .unwrap();
    assert!(peer.acquire(&id).await.unwrap().is_none());
    for _ in 0..5 {
        tokio::time::advance(Duration::from_millis(HEARTBEAT_INTERVAL_MS)).await;
        peer.maintain().await.unwrap();
    }
    let successor = peer.acquire(&id).await.unwrap().unwrap();
    assert!(successor.token.epoch > committed.token.epoch);
    assert_current(&successor, store.inner.as_ref()).await;
    assert!(!manager.is_ready());
    assert_eq!(store.releases.load(Ordering::SeqCst), 0);
}
