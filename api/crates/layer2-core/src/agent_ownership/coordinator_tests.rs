use super::test_support::MemoryOwnershipStore;
use super::*;
use std::{sync::Arc, time::Duration};

async fn start(store: Arc<MemoryOwnershipStore>, id: &str) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        store,
        Member {
            instance_id: id.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: format!("http://{id}:8080"),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        },
    )
    .await
    .unwrap()
}

async fn claim(instances: &[Arc<OwnershipCoordinator>], id: &AgentId) -> Arc<Authority> {
    let mut authority = None;
    for instance in instances {
        if let Some(acquired) = instance.acquire(id).await.unwrap() {
            assert!(authority.is_none());
            authority = Some(acquired);
        }
    }
    authority.expect("one live member can claim")
}

#[tokio::test(start_paused = true)]
async fn two_and_three_instances_keep_live_owner_when_other_local_registries_are_empty() {
    for count in [2, 3] {
        let store = MemoryOwnershipStore::default();
        let mut instances = Vec::new();
        for index in 0..count {
            instances.push(start(store.client(), &format!("instance-{index}")).await);
        }
        let id = AgentId("agent".into());
        let authority = claim(&instances, &id).await;
        for instance in &instances {
            if instance.instance_id != authority.token.instance_id {
                assert!(instance.local(&id).await.is_none());
                assert!(instance.acquire(&id).await.unwrap().is_none());
            }
        }
        assert_eq!(
            store.owner(&id).await.unwrap().unwrap().token,
            authority.token
        );
    }
}

#[tokio::test(start_paused = true)]
async fn partition_revokes_old_epoch_and_late_provider_completion_after_takeover() {
    let store = MemoryOwnershipStore::default();
    let clients: Vec<_> = (0..3).map(|_| store.client()).collect();
    let mut instances = Vec::new();
    for (index, client) in clients.iter().enumerate() {
        instances.push(start(client.clone(), &format!("instance-{index}")).await);
    }
    let id = AgentId("agent".into());
    let old = claim(&instances, &id).await;
    let owner = instances
        .iter()
        .position(|instance| instance.instance_id == old.token.instance_id)
        .unwrap();
    let waiting = tokio::spawn({
        let old = old.clone();
        async move { old.run(std::future::pending::<()>()).await }
    });
    clients[owner].partition(true);
    assert!(instances[owner].maintain().await.is_err());
    assert!(!instances[owner].is_ready());
    assert_eq!(waiting.await.unwrap(), Err(OwnershipError::Stale));
    // Healthy peers renew while the isolated owner's lease expires.
    for _ in 0..6 {
        tokio::time::advance(Duration::from_secs(5)).await;
        for (index, instance) in instances.iter().enumerate() {
            if index != owner {
                instance.maintain().await.unwrap();
            }
        }
    }
    let healthy: Vec<_> = instances
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != owner)
        .map(|(_, instance)| instance.clone())
        .collect();
    let new = claim(&healthy, &id).await;
    assert!(new.token.epoch > old.token.epoch);
    clients[owner].partition(false);
    assert_eq!(store.check(&old.token).await, Err(OwnershipError::Stale));
    assert!(instances[owner].acquire(&id).await.is_err());
    assert!(old.run(async { "late provider result" }).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn scale_up_rebalances_existing_agents_and_drain_releases_without_expiry() {
    let store = MemoryOwnershipStore::default();
    let mut instances = vec![
        start(store.client(), "a").await,
        start(store.client(), "b").await,
    ];
    let ids: Vec<_> = (0..18)
        .map(|index| AgentId(format!("agent-{index}")))
        .collect();
    for id in &ids {
        claim(&instances, id).await;
    }
    instances.push(start(store.client(), "c").await);
    let mut handed_off = 0;
    for _ in 0..6 {
        for instance in &instances {
            for authority in instance.maintain().await.unwrap() {
                instance
                    .handoff(&authority, async { Ok(()) })
                    .await
                    .unwrap();
                handed_off += 1;
            }
        }
        for id in &ids {
            for instance in &instances {
                instance.acquire(id).await.unwrap();
            }
        }
    }
    assert!(handed_off > 0);
    let draining = instances.pop().unwrap();
    let revoked = draining.begin_drain().await;
    assert!(!revoked.is_empty());
    assert!(!draining.is_ready());
    for authority in revoked {
        assert!(!authority.is_current());
        draining
            .handoff(&authority, async { Ok(()) })
            .await
            .unwrap();
        let replacement = claim(&instances, &authority.token.agent_id).await;
        assert!(replacement.token.epoch > authority.token.epoch);
    }
}

#[tokio::test(start_paused = true)]
async fn failed_receiving_runtime_revocation_does_not_release_durable_owner() {
    let store = MemoryOwnershipStore::default();
    let instance = start(store.client(), "a").await;
    let id = AgentId("agent".into());
    let authority = instance.acquire(&id).await.unwrap().unwrap();
    assert!(instance
        .handoff(&authority, async {
            Err(OwnershipError::Unavailable(
                "synthetic runtime partition".into(),
            ))
        })
        .await
        .is_err());
    assert!(!authority.is_current());
    assert_eq!(
        store.owner(&id).await.unwrap().unwrap().token,
        authority.token
    );
}
