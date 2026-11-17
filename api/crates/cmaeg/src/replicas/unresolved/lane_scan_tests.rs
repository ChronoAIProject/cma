use super::*;
use crate::replicas::{
    diagnostics::{Class, Stage},
    mongo_fixture::Fixture,
    store::Write,
};

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn lane_bound_preserves_partial_inspection_and_observer_history() {
    let fixture = Fixture::new().await;
    let node = fixture.node(1).await;
    let retained_until = time::OffsetDateTime::now_utc().unix_timestamp() + 3600;
    for _ in 0..3 {
        let id = uuid::Uuid::new_v4().to_string();
        let mut head = Head::empty(
            id.clone(),
            node.partitions.membership.deployment.clone(),
            "fixture-owner".into(),
        );
        head.queue.push_back(uuid::Uuid::new_v4().to_string());
        node.partitions
            .store
            .write(
                &id,
                &head.owner,
                &head,
                Write {
                    retained_until: Some(retained_until),
                    projection: Some(node.partitions.projection(true)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    let operator = Operator::new(node.partitions.clone(), node.plans(), node.journal());
    let scan = operator.scan_lanes(2).await.unwrap();
    assert!(scan.truncated);
    assert_eq!(scan.unreadable.len(), 2);
    assert_eq!(scan.skipped, 2);
    assert_eq!(scan.outstanding_lane_items, 2);
    assert!(scan.views.is_empty());
    for row in &scan.unreadable {
        assert_eq!(
            (row.class, row.stage),
            (Class::MissingItem, Stage::ItemRead)
        );
        assert_eq!(row.custody.as_deref(), Some("delayed_lane"));
    }
    let complete = operator.scan_lanes(3).await.unwrap();
    assert!(!complete.truncated, "exactly reaching the cap is complete");
    assert_eq!(complete.outstanding_lane_items, 3);

    // Only the lane scan is truncated; the FIFO scan can finish all its rows.
    let inspection = operator.inspect_bounded(4096, 2).await.unwrap();
    assert!(inspection.truncated);
    assert_eq!(inspection.outstanding_lane_items, 2);
    assert!(!operator.inspect().await.unwrap().truncated);

    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    operator.seen.lock().unwrap().insert("previous-head".into());
    let fifo = operator.head_candidates().await.unwrap();
    assert!(!fifo.truncated);
    metrics::with_local_recorder(&recorder, || operator.observe_scans(fifo, scan)).unwrap();
    assert!(operator.seen.lock().unwrap().contains("previous-head"));
    assert!(handle
        .render()
        .contains("cmaeg_replica_uncertainty_observation_truncated 1"));

    // A complete lane scan cannot clear a truncated FIFO observation either.
    let partial_fifo = super::scan::HeadScan {
        candidates: Vec::new(),
        unreadable: Vec::new(),
        skipped: 0,
        truncated: true,
    };
    metrics::with_local_recorder(&recorder, || operator.observe_scans(partial_fifo, complete))
        .unwrap();
    assert!(operator.seen.lock().unwrap().contains("previous-head"));
    assert!(handle
        .render()
        .contains("cmaeg_replica_uncertainty_observation_truncated 1"));
    let fifo = operator.head_candidates().await.unwrap();
    let complete = operator.scan_lanes(3).await.unwrap();
    metrics::with_local_recorder(&recorder, || operator.observe_scans(fifo, complete)).unwrap();
    assert!(operator.seen.lock().unwrap().is_empty());
    assert!(handle
        .render()
        .contains("cmaeg_replica_uncertainty_observation_truncated 0"));
    drop(operator);
    drop(node);
    fixture.cleanup().await;
}
