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
async fn just_renewed_loss_waits_for_expiry_then_allows_one_successor() {
    let store = MemoryOwnershipStore::default();
    let instances = vec![
        start(store.client(), "a").await,
        start(store.client(), "b").await,
        start(store.client(), "c").await,
    ];
    let id = AgentId("agent".into());
    let old = claim(&instances, &id).await;
    let crashed = instances
        .iter()
        .find(|instance| instance.instance_id == old.token.instance_id)
        .unwrap();
    crashed.maintain().await.unwrap();
    let survivors: Vec<_> = instances
        .iter()
        .filter(|instance| instance.instance_id != old.token.instance_id)
        .cloned()
        .collect();
    let started = tokio::time::Instant::now();
    for tick in 1..=5 {
        tokio::time::advance(Duration::from_millis(HEARTBEAT_INTERVAL_MS)).await;
        for survivor in &survivors {
            survivor.maintain().await.unwrap();
        }
        if tick < 5 {
            for survivor in &survivors {
                assert!(survivor.acquire(&id).await.unwrap().is_none());
            }
        }
    }
    let successor = claim(&survivors, &id).await;
    assert!(successor.token.epoch > old.token.epoch);
    assert!(!old.is_current());
    assert_eq!(store.check(&old.token).await, Err(OwnershipError::Stale));
    // This proves the ownership portion only. Runtime attach and user-service
    // readiness still require the measured end-to-end recovery gate.
    assert!(started.elapsed() <= Duration::from_secs(15));
}

#[tokio::test(start_paused = true)]
async fn partition_skips_ticks_and_old_epoch_expires_before_takeover_and_late_completion() {
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
    let healthy: Vec<_> = instances
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != owner)
        .map(|(_, instance)| instance.clone())
        .collect();
    // The isolated owner skips unavailable heartbeats. It renews nothing, so
    // its existing deadlines end before any peer can take over the Agent.
    let partitioned = tokio::time::Instant::now();
    let mut failed_at = None;
    let mut new = None;
    for step in 1..=60 {
        tokio::time::advance(Duration::from_millis(500)).await;
        if step % 6 == 0 {
            for peer in &healthy {
                peer.maintain().await.unwrap();
            }
            match instances[owner].maintain().await {
                Ok(handoffs) => assert!(handoffs.is_empty() && failed_at.is_none()),
                Err(_) => {
                    failed_at.get_or_insert(partitioned.elapsed());
                }
            }
        }
        if new.is_none() {
            for peer in &healthy {
                if let Some(successor) = peer.acquire(&id).await.unwrap() {
                    assert!(!old.is_current(), "successor overlapped a live owner");
                    assert!(successor.token.epoch > old.token.epoch);
                    assert!(new.replace(successor).is_none());
                }
            }
        }
        if new.is_none() {
            assert_eq!(store.owner(&id).await.unwrap().unwrap().token, old.token);
        }
        assert_eq!(
            instances[owner].is_ready(),
            partitioned.elapsed() < Duration::from_millis(DEFAULT_LEASE_MS)
        );
    }
    assert_eq!(failed_at, Some(Duration::from_millis(DEFAULT_LEASE_MS)));
    assert_eq!(waiting.await.unwrap(), Err(OwnershipError::Stale));
    let new = new.expect("a healthy peer claims the Agent after expiry");
    clients[owner].partition(false);
    assert_eq!(store.check(&old.token).await, Err(OwnershipError::Stale));
    assert!(instances[owner].acquire(&id).await.is_err());
    assert!(old.run(async { "late provider result" }).await.is_err());
    assert!(new.is_current());
    store.check(&new.token).await.unwrap();
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

#[tokio::test(start_paused = true)]
async fn reconciliation_owner_page_rechecks_expiry_and_local_slots() {
    let store = Arc::new(MemoryOwnershipStore::default());
    let foreign = start(store.clone(), "foreign").await;
    let first = AgentId("first".into());
    let handoff = AgentId("handoff".into());
    let expiring = AgentId("expiring".into());
    foreign.acquire(&first).await.unwrap().unwrap();
    let old = foreign.acquire(&handoff).await.unwrap().unwrap();
    foreign.acquire(&expiring).await.unwrap().unwrap();
    let settling = std::time::Duration::from_millis(50);
    let lease = std::time::Duration::from_millis(DEFAULT_LEASE_MS);
    tokio::time::advance(lease - settling).await;
    let coordinator = start(store, "manager").await;
    let mut owners = coordinator
        .observe_foreign_reconciliation_owners([&first, &handoff, &expiring])
        .await;
    assert!(coordinator.defer_reconciliation_for_foreign_owner(&mut owners, &first));
    assert!(!coordinator.defer_reconciliation_for_foreign_owner(&mut owners, &first));

    foreign.begin_drain().await;
    foreign.handoff(&old, async { Ok(()) }).await.unwrap();
    let local = coordinator.acquire(&handoff).await.unwrap().unwrap();
    assert!(local.token.epoch > old.token.epoch);
    assert!(!coordinator.defer_reconciliation_for_foreign_owner(&mut owners, &handoff));

    tokio::time::advance(settling).await;
    assert!(!coordinator.defer_reconciliation_for_foreign_owner(&mut owners, &expiring));
    let replacement = coordinator.acquire(&expiring).await.unwrap().unwrap();
    assert_eq!(replacement.token.epoch, 2);
}
