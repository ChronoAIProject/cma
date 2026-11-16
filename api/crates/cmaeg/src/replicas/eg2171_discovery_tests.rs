use super::{partition_discovery::Frontier, partition_model::partition_id, store::Write};
use crate::{
    ingress::routing_store::AdmissionError,
    turns::routed::{release_test_fixture::ReleaseHarness, scheduler::RecoverySource},
};
use futures_util::FutureExt;
use std::{panic::AssertUnwindSafe, time::Duration};

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn eg2171_corrupt_partition_does_not_starve_a_valid_neighbor() {
    let h = ReleaseHarness::with_successors(false).await;
    let result = AssertUnwindSafe(async {
        // Real plan, separate conversation partition, corrupt only the sealed
        // discovery head through the native record boundary. No external I/O.
        let mut other = h.plans[1].clone();
        other.targets[0].launch.event.partition_key.push_str("/bad");
        h.nodes[0].partitions.enqueue(&other).await.unwrap();
        let partitions = &h.nodes[0].partitions;
        let id = partition_id(&partitions.membership.deployment, &other, &other.targets[0]);
        let (mut head, version) = partitions.head(&id, &other.identity.owner).await.unwrap();
        head.kind = "invalid-fixture-kind".into();
        partitions
            .store
            .write(
                &id,
                &head.owner,
                &head,
                Write {
                    expected: Some(version),
                    projection: Some(partitions.projection(true)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(110)).await;
        let mut observed = false;
        for node in &h.nodes {
            let page = Frontier {
                partitions: node.partitions.clone(),
                plans: node.plans(),
            }
            .page(None)
            .await
            .unwrap();
            observed |= page
                .plans
                .iter()
                .flat_map(|p| &p.targets)
                .any(|t| t.id == h.plans[0].targets[0].id);
        }
        assert!(observed, "one corrupt head starved a valid partition");
        // A valid head with a missing item reports a typed issue; it must not
        // discard plans already recovered for another partition on that page.
        head.kind = "cmaeg_partition_v1".into();
        head.queue.clear();
        head.queue.push_back(uuid::Uuid::new_v4().to_string());
        partitions
            .store
            .write(
                &id,
                &head.owner,
                &head,
                Write {
                    expected: Some(version + 1),
                    projection: Some(partitions.projection(true)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(110)).await;
        let mut issues = Vec::new();
        let mut healthy = false;
        for node in &h.nodes {
            let page = Frontier {
                partitions: node.partitions.clone(),
                plans: node.plans(),
            }
            .page(None)
            .await
            .unwrap();
            healthy |= page
                .plans
                .iter()
                .flat_map(|p| &p.targets)
                .any(|t| t.id == h.plans[0].targets[0].id);
            issues.extend(page.issues);
        }
        assert!(healthy);
        assert!(issues
            .iter()
            .any(|issue| issue.record_id == id && issue.error == AdmissionError::Expired));
    })
    .catch_unwind()
    .await;
    h.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
