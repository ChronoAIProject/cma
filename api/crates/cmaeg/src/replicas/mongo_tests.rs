use super::{
    mongo_fixture::{Fixture, Node},
    partition::PartitionLease,
    partition_model::{item_id, partition_id, Head, Item},
    Error,
};
use crate::ingress::routing_store::AdmittedPlan;
use std::time::{Duration, Instant};

async fn plan() -> AdmittedPlan {
    let (_server, mut plan) = super::partition_tests::plan().await;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    plan.admitted_at = now;
    plan.retained_until = now + 3600;
    for target in &mut plan.targets {
        target.launch.event.expires_at = now + 900;
    }
    plan.targets.truncate(1);
    plan
}
async fn claim(nodes: &[Node], plan: &AdmittedPlan) -> (usize, PartitionLease) {
    let start = Instant::now();
    loop {
        for (index, node) in nodes.iter().enumerate() {
            if node.worker.is_none() {
                continue;
            }
            match node.partitions.claim(plan, &plan.targets[0]).await {
                Ok(lease) => return (index, lease),
                Err(Error::Busy | Error::Lost) => {}
                Err(error) => panic!("claim failed: {error}"),
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(25),
            "takeover exceeded fixture bound"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn independent_mongo_queue_replay_takeover_and_stale_mutation_fences() {
    let fixture = Fixture::new().await;
    let mut nodes = vec![
        fixture.node(1).await,
        fixture.node(2).await,
        fixture.node(3).await,
    ];
    let plan = plan().await;
    let mut later = plan.clone();
    later.identity.event_id = cmaeg_core::ids::EventId::new("next-event").unwrap();
    later.admission_id = later.identity.admission_id();
    later.targets[0].id = uuid::Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        nodes[0].partitions.enqueue(&plan),
        nodes[1].partitions.enqueue(&plan)
    );
    assert!(a.is_ok() || b.is_ok());
    nodes[2].partitions.enqueue(&plan).await.unwrap();
    nodes[2].partitions.enqueue(&later).await.unwrap();
    let id = partition_id(
        &nodes[0].partitions.membership.deployment,
        &plan,
        &plan.targets[0],
    );
    let (head, _) = nodes[0]
        .partitions
        .store
        .read::<Head>(&id, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        head.queue.iter().collect::<Vec<_>>(),
        vec![&plan.targets[0].id, &later.targets[0].id]
    );
    let (owner, mut token) = claim(&nodes, &plan).await;
    for node in &nodes {
        assert!(node
            .partitions
            .claim(&later, &later.targets[0])
            .await
            .is_err());
    }
    let effect = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        nodes[owner]
            .partitions
            .write(
                &token,
                &effect,
                &plan.identity.owner,
                plan.retained_until,
                &serde_json::json!({"receipt":1}),
                None
            )
            .await
            .unwrap(),
        1
    );
    nodes[owner].partitions.renew(&mut token).await.unwrap();
    let stale = token.clone();
    // Drop aborts heartbeat without publishing a drain marker: peers must use
    // genuine server expiry, including the just-renewed resource lease.
    drop(nodes[owner].worker.take());
    let started = Instant::now();
    let (successor, next) = claim(&nodes, &plan).await;
    assert_ne!(owner, successor);
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(next.token.epoch > stale.token.epoch);
    assert_eq!(
        nodes[owner]
            .partitions
            .write(
                &stale,
                &effect,
                &plan.identity.owner,
                plan.retained_until,
                &serde_json::json!({"receipt":2}),
                Some(1)
            )
            .await,
        Err(Error::Lost)
    );
    assert_eq!(
        nodes[owner].partitions.renew(&mut token).await,
        Err(Error::Lost)
    );
    assert_eq!(
        nodes[owner].partitions.release(&stale).await,
        Err(Error::Lost)
    );
    nodes[successor].partitions.complete(&next).await.unwrap();
    nodes[successor].partitions.enqueue(&plan).await.unwrap();
    let (head, _) = nodes[successor]
        .partitions
        .store
        .read::<Head>(&id, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head.queue.len(), 1);
    assert_eq!(head.queue.front(), Some(&later.targets[0].id));
    let (marker, _) = nodes[successor]
        .partitions
        .store
        .read::<Item>(&item_id(&plan.targets[0].id), true)
        .await
        .unwrap()
        .unwrap();
    assert!(marker.done);
    drop(nodes);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn mongo_delayed_effect_stays_ordered_after_original_completion() {
    let fixture = Fixture::new().await;
    let nodes = vec![fixture.node(1).await, fixture.node(2).await];
    let plan = plan().await;
    nodes[0].partitions.enqueue(&plan).await.unwrap();
    let (owner, lease) = claim(&nodes, &plan).await;
    nodes[owner].partitions.complete(&lease).await.unwrap();
    let work = uuid::Uuid::new_v4().to_string();
    nodes[1]
        .partitions
        .enqueue_effect(&plan, &plan.targets[0], &work)
        .await
        .unwrap();
    nodes[0].partitions.enqueue(&plan).await.unwrap();
    assert!(nodes[owner]
        .partitions
        .capture(&plan, &plan.targets[0])
        .await
        .is_err());
    let id = partition_id(
        &nodes[0].partitions.membership.deployment,
        &plan,
        &plan.targets[0],
    );
    let (head, _) = nodes[0]
        .partitions
        .store
        .read::<Head>(&id, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(head.queue.iter().collect::<Vec<_>>(), vec![&work]);
    assert_eq!(
        nodes[0]
            .partitions
            .enqueue_effect(&plan, &plan.targets[0], &plan.targets[0].id)
            .await,
        Err(Error::Corrupt)
    );
    drop(nodes);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn mongo_balances_twenty_partitions_through_two_four_two_members() {
    let fixture = Fixture::new().await;
    let mut nodes = vec![fixture.node(1).await, fixture.node(2).await];
    let base = plan().await;
    let mut plans = Vec::new();
    for index in 0..20 {
        let mut plan = base.clone();
        plan.targets[0].id = uuid::Uuid::new_v4().to_string();
        plan.targets[0].launch.event.partition_key = format!("partition-{index}");
        nodes[0].partitions.enqueue(&plan).await.unwrap();
        plans.push(plan);
    }
    for count in [2, 4, 2] {
        while nodes.len() < count {
            nodes.push(fixture.node(nodes.len() as u16 + 1).await);
        }
        if count < nodes.len() {
            for node in nodes.iter().skip(count) {
                node.control.start();
            }
        }
        // Observe sealed membership changes, not a shared local registry.
        tokio::time::sleep(Duration::from_millis(3500)).await;
        if count < nodes.len() {
            nodes.truncate(count);
        }
        let mut owned = vec![0usize; count];
        for plan in &plans {
            let (owner, token) = claim(&nodes, plan).await;
            owned[owner] += 1;
            nodes[owner].partitions.release(&token).await.unwrap();
        }
        assert!(owned.iter().all(|count| *count > 0));
        assert!(owned.iter().max().unwrap() - owned.iter().min().unwrap() <= 1);
    }
    drop(nodes);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn mongo_protocol_floor_rejects_mode_off_and_split_deployment_after_activation() {
    let fixture = Fixture::new().await;
    let node = fixture.node(1).await;
    let store = &node.partitions.store;
    let origin = "https://replica-fixture.invalid";
    assert!(super::protocol::enforce(store, origin, false).await.is_ok());
    assert!(super::protocol::enforce(store, origin, true).await.is_ok());
    assert!(super::protocol::enforce(store, origin, true).await.is_ok());
    assert_eq!(
        super::protocol::enforce(store, origin, false).await,
        Err(Error::Corrupt)
    );
    assert_eq!(
        super::protocol::enforce(store, "https://different.invalid", true).await,
        Err(Error::Corrupt)
    );
    drop(node);
    fixture.cleanup().await;
}
