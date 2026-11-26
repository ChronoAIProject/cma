use super::*;
use layer2_core::sandbox_metrics::{
    measured_evidence::capture_observations, Binding, BindingKind, ProofStatus, Range,
};
use layer3_adapter::sandbox_metrics::{decode_raw_export, Selection, SourceMetric};
use mongodb::{
    options::{ClientOptions, IndexOptions},
    IndexModel,
};

fn observation(owner: &str, mapping_revision: u64, value: f64) -> MeasuredObservation {
    observation_at(owner, mapping_revision, 30_001, value)
}

pub(super) fn observation_at(
    owner: &str,
    mapping_revision: u64,
    timestamp_ms: u64,
    value: f64,
) -> MeasuredObservation {
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
        "timestamps": [timestamp_ms], "values": [value],
    });
    let source = decode_raw_export(raw.to_string().as_bytes(), &selection).unwrap();
    let binding = fixture_binding(owner);
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

pub(super) fn fixture_binding(owner: &str) -> Binding {
    Binding {
        id: "binding-synthetic".into(),
        schema_version: 1,
        revision: 1,
        deployment: "synthetic".into(),
        cluster: "synthetic".into(),
        namespace: "runtime".into(),
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
    }
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
                        .name("measured_physical_sample_v1".to_owned())
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
    let competing_again = observation("owner-b", 4, 8.0);
    let conflict_before = a.rows.find_one(doc! {"kind": "conflict"}).await.unwrap();
    for sample in [&original, &mapped_again, &competing, &competing_again] {
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
    assert_eq!(
        restarted
            .rows
            .find_one(doc! {"kind": "conflict"})
            .await
            .unwrap(),
        conflict_before
    );
    let new_competitor = observation("owner-b", 5, 9.0);
    assert_eq!(
        restarted.capture(&new_competitor).await.unwrap(),
        Capture::Disputed
    );
    assert_eq!(restarted.rows.count_documents(doc! {}).await.unwrap(), 3);
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn measured_capture_recovers_only_exact_evidence_after_an_uncertain_commit() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    let app_name = format!("measured-commit-{}", uuid::Uuid::new_v4().simple());
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_measured_commit_{}", uuid::Uuid::new_v4().simple());
    let database = client.database(&name);
    database.create_collection(COLLECTION).await.unwrap();
    let store = MeasuredCapture::new(&database);
    let sample = observation("owner-a", 1, 8.0);
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": "alwaysOn",
            "data": {
                "failCommands": ["commitTransaction"], "appName": app_name,
                "errorLabels": ["UnknownTransactionCommitResult"],
                "writeConcernError": {"code": 64, "errmsg": "fixture uncertain result"},
            },
        })
        .await
        .unwrap();
    let uncertain = store.capture(&sample).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    crate::store::test_visibility::majority_barrier(&admin, &name).await;
    assert!(matches!(
        uncertain,
        Ok(Capture::Inserted) | Err(CaptureError::Unavailable)
    ));
    assert_eq!(store.rows.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(store.capture(&sample).await.unwrap(), Capture::Replay);
    database.drop().await.unwrap();
}
