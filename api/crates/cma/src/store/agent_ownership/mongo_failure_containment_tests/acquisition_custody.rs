use super::*;

async fn manager_with_policy(
    store: Arc<dyn OwnershipStore>,
    instance: &str,
    policy: MaintenancePolicy,
) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register_with_policy(
        store,
        Member {
            instance_id: instance.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://manager.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 1,
        },
        false,
        policy,
    )
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn definite_acquisition_refusal_frees_capacity_and_allows_retry_after_repair() {
    for policy in [
        MaintenancePolicy::Legacy,
        MaintenancePolicy::ContainFailures,
    ] {
        let store = Store::new().await;
        let coordinator = manager_with_policy(store.clone(), "first", policy).await;
        let bad = AgentId("incompatible-agent".into());
        let other = AgentId("healthy-agent".into());
        let owner_filter = doc! {"_id": bad.as_str()};
        store
            .inner
            .owners
            .insert_one(doc! {
                "_id": bad.as_str(),
                "document_schema_version": 1_i64,
                "protocol": i64::from(PROTOCOL_VERSION) + 1,
                "epoch": 0_i64,
                "instance_id": mongodb::bson::Bson::Null,
                "expires_at": mongodb::bson::DateTime::from_millis(0),
                "effect_revision": 0_i64,
            })
            .await
            .unwrap();

        assert_eq!(
            coordinator.acquire(&bad).await.err(),
            Some(OwnershipError::IncompatibleProtocol)
        );
        assert_eq!(store.membership().await.unwrap().members[0].owned_agents, 0);
        let healthy = coordinator.acquire(&other).await.unwrap().unwrap();
        coordinator
            .handoff(&healthy, healthy.revoke_transports())
            .await
            .unwrap();

        store
            .inner
            .owners
            .update_one(
                owner_filter,
                doc! {"$set": {"protocol": i64::from(PROTOCOL_VERSION)}},
            )
            .await
            .unwrap();
        let repaired = coordinator.acquire(&bad).await.unwrap().unwrap();
        assert_eq!(repaired.token.epoch, 1);
        let until = Instant::now() + Duration::from_secs(3);
        coordinator.prepare_drain(until).await.unwrap();
        let drained = coordinator.owned().await;
        assert_eq!(drained.len(), 1);
        let authority = &drained[0];
        assert_eq!(authority.token, repaired.token);
        match policy {
            MaintenancePolicy::Legacy => {
                authority.revoke();
                coordinator
                    .handoff(authority, authority.revoke_transports_after_local_drain())
                    .await
                    .unwrap();
            }
            MaintenancePolicy::ContainFailures => {
                assert_eq!(
                    coordinator
                        .drain_authority_until(authority, until)
                        .await
                        .unwrap(),
                    Settlement::Released
                );
            }
        }
        assert_eq!(store.membership().await.unwrap().members[0].owned_agents, 0);
        close(&store, &[coordinator]).await;
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn lost_acquisition_reply_releases_exact_expired_token_and_restores_capacity() {
    for policy in [
        MaintenancePolicy::Legacy,
        MaintenancePolicy::ContainFailures,
    ] {
        let store = Store::new().await;
        let coordinator = manager_with_policy(store.clone(), "first", policy).await;
        let id = AgentId("first-agent".into());
        let other = AgentId("second-agent".into());
        store.lose_acquire.store(true, Ordering::Release);
        assert!(coordinator.acquire(&id).await.unwrap().is_none());
        let original = store.inner.owner(&id).await.unwrap().unwrap();
        assert!(coordinator.local(&id).await.is_none());
        assert!(coordinator.acquire(&other).await.unwrap().is_none());
        store.expired(&original.token).await;
        assert!(store.inner.owner(&id).await.unwrap().is_none());

        coordinator.poll_acquisitions().await.unwrap();
        if policy == MaintenancePolicy::ContainFailures {
            let retained = coordinator.owned().await;
            assert_eq!(retained[0].token, original.token);
            assert_eq!(
                coordinator
                    .retirement_background(&retained[0])
                    .await
                    .unwrap(),
                Some(Settlement::Released)
            );
        }
        assert_eq!(store.releases.load(Ordering::Acquire), 1);
        assert!(coordinator
            .acquire(&other)
            .await
            .unwrap()
            .unwrap()
            .is_current());
        assert_eq!(store.membership().await.unwrap().members[0].owned_agents, 1);
        close(&store, &[coordinator]).await;
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn lost_acquisition_reply_with_peer_takeover_does_not_hold_capacity() {
    for policy in [
        MaintenancePolicy::Legacy,
        MaintenancePolicy::ContainFailures,
    ] {
        let store = Store::new().await;
        let first = manager_with_policy(store.clone(), "first", policy).await;
        let second = manager_with_policy(store.inner.clone(), "second", policy).await;
        let membership = store.membership().await.unwrap();
        let id = assigned(&membership, "second", "handoff-agent");
        let other = assigned(&membership, "first", "new-agent");
        let busy = assigned(&membership, "second", "busy-agent");
        let occupied = second.acquire(&busy).await.unwrap().unwrap();

        // Simulate loss between the durable commit and the local result.
        // Keep the operation in the coordinator until the peer has taken over.
        store.hold_acquire.store(true, Ordering::Release);
        store.lose_acquire.store(true, Ordering::Release);
        let caller = tokio::spawn({
            let first = first.clone();
            let id = id.clone();
            async move { first.acquire(&id).await }
        });
        tokio::time::timeout(Duration::from_secs(2), store.entered.notified())
            .await
            .unwrap();
        let original = store.inner.owner(&id).await.unwrap().unwrap();
        caller.abort();
        assert!(caller.await.err().unwrap().is_cancelled());
        assert_eq!(original.token.instance_id, "first");

        store.expired(&original.token).await;
        second
            .handoff(&occupied, occupied.revoke_transports())
            .await
            .unwrap();
        let successor = second.acquire(&id).await.unwrap().unwrap();
        assert!(successor.token.epoch > original.token.epoch);
        store.hold_acquire.store(false, Ordering::Release);
        store.resume.notify_one();

        first.poll_acquisitions().await.unwrap();
        assert!(first.owned().await.is_empty());
        assert!(first.acquire(&other).await.unwrap().unwrap().is_current());
        store.inner.check(&successor.token).await.unwrap();
        assert_eq!(store.releases.load(Ordering::Acquire), 0);
        close(&store, &[first, second]).await;
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn legacy_drain_joins_an_acquisition_after_the_caller_disappears() {
    let store = Store::new().await;
    let coordinator = manager_with_policy(store.clone(), "first", MaintenancePolicy::Legacy).await;
    let id = AgentId("draining-agent".into());
    store.hold_acquire.store(true, Ordering::Release);
    let caller = tokio::spawn({
        let coordinator = coordinator.clone();
        let id = id.clone();
        async move { coordinator.acquire(&id).await }
    });
    tokio::time::timeout(Duration::from_secs(2), store.entered.notified())
        .await
        .unwrap();
    let exact = store.inner.owner(&id).await.unwrap().unwrap().token;
    caller.abort();
    assert!(caller.await.err().unwrap().is_cancelled());
    coordinator.stop_admission();
    assert_eq!(
        coordinator.acquire(&id).await.err(),
        Some(OwnershipError::Stale)
    );
    store.resume.notify_one();

    coordinator
        .prepare_drain(Instant::now() + Duration::from_secs(3))
        .await
        .unwrap();
    let retained = coordinator.owned().await;
    assert_eq!(retained[0].token, exact);
    assert!(!retained[0].is_current());
    coordinator
        .handoff(&retained[0], retained[0].revoke_transports())
        .await
        .unwrap();
    assert!(store.inner.owner(&id).await.unwrap().is_none());
    assert_eq!(store.releases.load(Ordering::Acquire), 1);
    close(&store, &[coordinator]).await;
}
