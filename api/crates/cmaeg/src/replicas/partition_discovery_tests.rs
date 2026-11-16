use super::*;
use crate::replicas::{mongo_fixture::Fixture, partition_tests};

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn cached_frontier_after_atomic_completion_is_not_corruption() {
    let database = Fixture::new().await;
    let node = database.node(1).await;
    let (_server, mut plan) = partition_tests::plan().await;
    plan.targets.truncate(1);
    plan.retained_until = time::OffsetDateTime::now_utc().unix_timestamp() + 3600;
    let target = &plan.targets[0];
    node.partitions.enqueue(&plan).await.unwrap();
    let heads = node.partitions.active_heads().await.unwrap();
    let lease = node.partitions.claim(&plan, target).await.unwrap();
    node.partitions.complete(&lease).await.unwrap();
    // Preserve the exact pre-completion snapshot through the next read without
    // relying on host speed to hit the production 100ms cache window.
    *node.partitions.snapshot.lock().await = Snapshot {
        at: Instant::now(),
        heads,
    };
    let frontier = Frontier {
        partitions: node.partitions.clone(),
        plans: node.plans(),
    };
    let page = frontier.page(None).await.unwrap();
    assert!(page.plans.is_empty());
    assert!(page.issues.is_empty());
    // A valid completion stays absent even if an admission retry arrives.
    node.partitions.enqueue(&plan).await.unwrap();
    let id = crate::replicas::partition_model::partition_id(
        &node.partitions.membership.deployment,
        &plan,
        target,
    );
    let (mut current, version) = node
        .partitions
        .head(&id, &plan.identity.owner)
        .await
        .unwrap();
    assert!(current.queue.is_empty());
    // A truly contradictory durable head must still be refused. Only the
    // cached, already-advanced hint above is safe to discard.
    current.queue.push_back(target.id.clone());
    node.partitions
        .store
        .write(
            &id,
            &plan.identity.owner,
            &current,
            crate::replicas::store::Write {
                expected: Some(version),
                projection: Some(node.partitions.projection(true)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    *node.partitions.snapshot.lock().await = Snapshot {
        at: Instant::now(),
        heads: BTreeMap::from([(id, current)]),
    };
    assert!(matches!(
        frontier.page(None).await,
        Err(AdmissionError::Corrupt)
    ));
    drop(node);
    database.cleanup().await;
}
