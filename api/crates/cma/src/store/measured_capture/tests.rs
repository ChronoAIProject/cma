use super::*;
use layer2_core::sandbox_metrics::{
    measured_evidence::capture_observations, Binding, BindingKind, ProofStatus, Range,
};
use layer3_adapter::sandbox_metrics::{decode_raw_export, Selection, SourceMetric};
use mongodb::{options::IndexOptions, IndexModel};

fn observation(owner: &str, mapping_revision: u64, value: f64) -> MeasuredObservation {
    let selection = Selection {
        cluster: "synthetic".into(),
        namespace: "runtime".into(),
        metric: SourceMetric::PvcUsed,
        names: vec!["home".into()],
        from: 20,
        to: 40,
    };
    let raw = serde_json::json!({
        "metric": {
            "cluster": selection.cluster, "namespace": selection.namespace,
            "__name__": selection.metric.name(), "job": "kubelet",
            "metrics_path": selection.metric.metrics_path(), "persistentvolumeclaim": "home",
            "pvc_uid": "pvc-synthetic",
        },
        "timestamps": [30_001], "values": [value],
    });
    let source = decode_raw_export(raw.to_string().as_bytes(), &selection).unwrap();
    let binding = Binding {
        id: "binding-synthetic".into(),
        schema_version: 1,
        revision: 1,
        deployment: "synthetic".into(),
        cluster: selection.cluster,
        namespace: selection.namespace,
        native_sandbox_uid: "native-synthetic".into(),
        claim_uid: "claim-synthetic".into(),
        sandbox_id: "sbx_synthetic".into(),
        owner_subject: owner.into(),
        pod_name: None,
        pod_uid: None,
        runtime_container_id: None,
        restart_count: None,
        runtime_generation: None,
        pvc_name: "home".into(),
        pvc_uid: "pvc-synthetic".into(),
        kind: BindingKind::Home,
        cpu_limit_cores: None,
        memory_limit_bytes: None,
        valid_from: 20,
        valid_until: None,
        observed_at: 40,
        proof_version: 1,
        proof_status: ProofStatus::Verified,
    };
    capture_observations(
        &binding,
        mapping_revision,
        selection.metric,
        &source,
        Range { from: 20, to: 40 },
    )
    .unwrap()
    .remove(0)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn measured_capture_keeps_first_evidence_and_durable_disputes_across_replicas() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let left = Client::with_uri_str(&uri).await.unwrap();
    let right = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_measured_{}", uuid::Uuid::new_v4().simple());
    let a = MeasuredCapture::new(&left.database(&name));
    let b = MeasuredCapture::new(&right.database(&name));
    a.rows
        .create_index(
            IndexModel::builder()
                .keys(doc! {"physical_sample_id": 1, "kind": 1, "_id": 1})
                .options(
                    IndexOptions::builder()
                        .name("measured_physical_sample_v1".into())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    let original = observation("owner-a", 1, 8.0);
    assert_eq!(a.capture(&original).await.unwrap(), Capture::Inserted);
    let before = a
        .rows
        .find_one(doc! {"kind": "sample"})
        .await
        .unwrap()
        .unwrap();
    let mapped_again = observation("owner-a", 2, 8.0);
    assert_eq!(b.capture(&mapped_again).await.unwrap(), Capture::Replay);
    let competing = observation("owner-b", 3, 8.0);
    let (left_outcome, right_outcome) = tokio::join!(a.capture(&competing), b.capture(&competing));
    assert_eq!(left_outcome.unwrap(), Capture::Disputed);
    assert_eq!(right_outcome.unwrap(), Capture::Disputed);
    let restarted = MeasuredCapture::new(&right.database(&name));
    for sample in [&original, &mapped_again, &competing] {
        assert_eq!(restarted.capture(sample).await.unwrap(), Capture::Disputed);
    }
    assert_eq!(
        restarted
            .rows
            .find_one(doc! {"kind": "sample"})
            .await
            .unwrap(),
        Some(before)
    );
    assert_eq!(restarted.rows.count_documents(doc! {}).await.unwrap(), 2);
    restarted
        .rows
        .update_one(doc! {"kind": "sample"}, doc! {"$set": {"payload": "{}"}})
        .await
        .unwrap();
    assert!(matches!(
        restarted.capture(&original).await,
        Err(CaptureError::Corrupt)
    ));
    left.database(&name).drop().await.unwrap();
}
