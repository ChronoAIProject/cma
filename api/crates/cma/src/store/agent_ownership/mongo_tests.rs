use super::*;
use layer2_core::agent_ownership::{candidates, DEFAULT_MAX_OWNED_AGENTS};

#[path = "mongo_batch_renewal_tests.rs"]
mod batch_renewal;
#[path = "mongo_containment_tests.rs"]
mod containment;
#[path = "mongo_effect_tests.rs"]
mod effects;
#[path = "mongo_failure_tests.rs"]
mod failure;
#[path = "mongo_heartbeat_tests.rs"]
mod heartbeat;
#[path = "mongo_renewal_tests.rs"]
mod renewal;
#[path = "mongo_two_manager_tests.rs"]
mod two_manager;

async fn fixture() -> MongoAgentOwnership {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("use the disposable Mongo integration harness");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_ownership_test_{}", uuid::Uuid::new_v4().simple());
    MongoAgentOwnership::open(&client, &name).await.unwrap()
}

fn member(id: &str) -> Member {
    Member {
        instance_id: id.into(),
        protocol: PROTOCOL_VERSION,
        relay_address: format!("http://{id}:8080"),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
    }
}

async fn preferred(store: &MongoAgentOwnership, id: &AgentId) -> String {
    candidates(id, &store.membership().await.unwrap(), None).unwrap()[0]
        .instance_id
        .clone()
}

async fn expire(store: &MongoAgentOwnership, token: &OwnershipToken) {
    store
        .owners
        .update_one(
            doc! {"_id": token.agent_id.as_str()},
            doc! {
                "$set": {"expires_at": mongodb::bson::DateTime::from_millis(0)},
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn two_and_three_instances_share_one_owner_and_keep_epoch_after_release() {
    for count in [2, 3] {
        let store = fixture().await;
        let instances: Vec<_> = (0..count)
            .map(|index| format!("instance-{index}"))
            .collect();
        for instance in &instances {
            store.register(member(instance)).await.unwrap();
        }
        let agent = AgentId("agent".into());
        assert!(!store.has_owner_history(&agent).await.unwrap());
        let winner = preferred(&store, &agent).await;
        let mut tasks = tokio::task::JoinSet::new();
        for instance in &instances {
            let (store, instance, agent) = (store.clone(), instance.clone(), agent.clone());
            tasks.spawn(async move { store.acquire(&agent, &instance).await.unwrap() });
        }
        let mut acquired = Vec::new();
        while let Some(task) = tasks.join_next().await {
            if let Some(lease) = task.unwrap() {
                acquired.push(lease);
            }
        }
        assert_eq!(acquired.len(), 1);
        let first = &acquired[0];
        assert_eq!(first.token.instance_id, winner);
        assert_eq!(first.token.epoch, 1);
        // Another invocation on the owner does not allocate another slot or epoch.
        assert_eq!(
            store.acquire(&agent, &winner).await.unwrap(),
            Some(first.clone())
        );
        for instance in instances.iter().filter(|instance| **instance != winner) {
            assert!(store.acquire(&agent, instance).await.unwrap().is_none());
        }
        store.release(&first.token).await.unwrap();
        store.release(&first.token).await.unwrap();
        assert!(store.owner(&agent).await.unwrap().is_none());
        assert!(store.has_owner_history(&agent).await.unwrap());
        let second = store.acquire(&agent, &winner).await.unwrap().unwrap();
        assert_eq!(second.token.epoch, 2);
        assert_eq!(store.check(&first.token).await, Err(OwnershipError::Stale));
        assert_eq!(store.renew(&first.token).await, Err(OwnershipError::Stale));
        store.release(&first.token).await.unwrap();
        assert_eq!(store.owner(&agent).await.unwrap(), Some(second));
        store.database.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn expiry_and_drain_reject_stale_effects_and_allow_takeover() {
    let store = fixture().await;
    for instance in ["a", "b", "c"] {
        store.register(member(instance)).await.unwrap();
    }
    let agent = AgentId("agent".into());
    let first = store
        .acquire(&agent, &preferred(&store, &agent).await)
        .await
        .unwrap()
        .unwrap();
    let lifetime = store.live_lease_remaining_ms(&first.token).await.unwrap();
    assert!((1..=DEFAULT_LEASE_MS).contains(&lifetime));
    store
        .heartbeat(&first.token.instance_id, true)
        .await
        .unwrap();
    // Draining does not let a peer steal a live transport before release.
    let next = preferred(&store, &agent).await;
    assert_ne!(next, first.token.instance_id);
    assert!(store.acquire(&agent, &next).await.unwrap().is_none());
    assert_eq!(store.renew(&first.token).await, Err(OwnershipError::Stale));
    assert_eq!(store.check(&first.token).await, Err(OwnershipError::Stale));
    assert_eq!(
        store.live_lease_remaining_ms(&first.token).await,
        Err(OwnershipError::Stale)
    );
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert_eq!(
        store.fence_transaction(&first.token, &mut session).await,
        Err(SandboxStoreError::Conflict)
    );
    session.abort_transaction().await.unwrap();
    expire(&store, &first.token).await;
    let second = store.acquire(&agent, &next).await.unwrap().unwrap();
    assert_eq!(second.token.epoch, first.token.epoch + 1);
    let mut stale_epoch = second.token.clone();
    stale_epoch.epoch = first.token.epoch;
    assert_eq!(
        store.live_lease_remaining_ms(&stale_epoch).await,
        Err(OwnershipError::Stale)
    );
    start_transaction(&mut session).await.unwrap();
    assert_eq!(
        store.fence_transaction(&first.token, &mut session).await,
        Err(SandboxStoreError::Conflict)
    );
    session.abort_transaction().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .fence_transaction(&second.token, &mut session)
        .await
        .unwrap();
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn capacity_is_bounded_and_expired_members_cannot_resurrect() {
    let store = fixture().await;
    let mut one = member("one");
    one.max_owned_agents = 1;
    store.register(one).await.unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..12 {
        let store = store.clone();
        tasks.spawn(async move {
            store
                .acquire(&AgentId(format!("agent-{index}")), "one")
                .await
        });
    }
    let mut owned = Vec::new();
    while let Some(task) = tasks.join_next().await {
        if let Some(lease) = task.unwrap().unwrap() {
            owned.push(lease);
        }
    }
    assert_eq!(owned.len(), 1);
    assert_eq!(store.membership().await.unwrap().members[0].owned_agents, 1);
    expire(&store, &owned[0].token).await;
    let reclaimed = store
        .acquire(&owned[0].token.agent_id, "one")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reclaimed.token.epoch, owned[0].token.epoch + 1);
    assert_eq!(store.membership().await.unwrap().members[0].owned_agents, 1);
    store
        .members
        .update_one(
            doc! {"_id": "one"},
            doc! {
                "$set": {"expires_at": mongodb::bson::DateTime::from_millis(0)},
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.heartbeat("one", false).await,
        Err(OwnershipError::Stale)
    );
    assert_eq!(
        store.renew(&owned[0].token).await,
        Err(OwnershipError::Stale)
    );
    assert_eq!(
        store.check(&reclaimed.token).await,
        Err(OwnershipError::Stale)
    );
    store.database.drop().await.unwrap();
}

#[path = "fence_mongo_tests.rs"]
mod transaction_fence;
