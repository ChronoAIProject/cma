use super::*;
use crate::store::measured_capture::tests::fixture_binding;
use axum::{extract::State, http::Uri, routing::get, Router};
use mongodb::options::ClientOptions;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

#[test]
fn command_requires_one_sandbox_and_a_closed_bounded_window() {
    let arguments = |sandbox: &str, from: &str, to: &str| {
        ["--measured-capture", sandbox, from, to].map(str::to_owned)
    };
    assert!(invocation(&arguments("sbx_fixture", "10", "20"), 20).is_ok());
    for (sandbox, from, to, now) in [
        ("sbx_", "10", "20", 20),
        ("sbx_a/sbx_b", "10", "20", 20),
        ("sbx_fixture", "10", "10", 20),
        ("sbx_fixture", "10", "20", 19),
        ("sbx_fixture", "10", "911", 911),
    ] {
        assert_eq!(invocation(&arguments(sandbox, from, to), now), Err(INVALID));
    }
}

#[test]
fn measured_preview_requires_a_physical_id_and_completed_window() {
    let anchor = "a".repeat(64);
    let arguments =
        |id: &str, from: &str, to: &str| ["--measured-preview", id, from, to].map(str::to_owned);
    let accepted = arguments(&anchor, "20", "40");
    assert_eq!(
        preview_invocation(&accepted, 40),
        Ok((anchor.as_str(), Range { from: 20, to: 40 }))
    );
    assert_eq!(preview_invocation(&accepted, 39), Err(PREVIEW_INVALID));
    assert_eq!(
        preview_invocation(&arguments("sample:fake", "20", "40"), 40),
        Err(PREVIEW_INVALID)
    );
    assert_eq!(
        preview_invocation(&arguments(&anchor, "20", "921"), 921),
        Err(PREVIEW_INVALID)
    );
}

async fn exporter(State(used): State<Arc<AtomicU64>>, uri: Uri) -> String {
    let selected = uri.query().unwrap();
    let metric = if selected.contains(SourceMetric::PvcUsed.name()) {
        SourceMetric::PvcUsed
    } else {
        assert!(selected.contains(SourceMetric::PvcCapacity.name()));
        SourceMetric::PvcCapacity
    };
    let value = if metric == SourceMetric::PvcUsed {
        used.load(Ordering::Relaxed)
    } else {
        100
    };
    json!({
        "metric": {
            "cluster": "synthetic", "namespace": "runtime", "__name__": metric.name(),
            "job": "kubelet", "metrics_path": metric.metrics_path(),
            "persistentvolumeclaim": "home", "pvc_uid": if used.load(Ordering::Relaxed) == 0 { "wrong-pvc" } else { "pvc-synthetic" },
        },
        "timestamps": [30_001], "values": [value],
    })
    .to_string()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn measured_capture_command_replays_a_partial_window_and_retains_a_dispute() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    let app_name = format!("measured-capture-{}", uuid::Uuid::new_v4().simple());
    options.app_name = Some(app_name.clone());
    let writer = Client::with_options(options).unwrap();
    let name = format!("cma_measured_capture_{}", uuid::Uuid::new_v4().simple());
    let database = writer.database(&name);
    database
        .run_command(doc! {
            "createIndexes": COLLECTION, "indexes": [{"name": "measured_physical_sample_v1",
                "key": {"physical_sample_id": 1, "kind": 1, "_id": 1}}],
        })
        .await
        .unwrap();
    database
        .collection::<Document>(usage_reports::CONTROL)
        .insert_one(doc! {"_id": "observer", "revision": 1_i64})
        .await
        .unwrap();
    let binding = fixture_binding("owner-a");
    database
        .collection::<Document>(sandbox_metric_bindings::COLLECTION)
        .insert_one(doc! {
            "_id": &binding.id, "sandbox_id": &binding.sandbox_id, "document_schema_version": 1_i64,
            "valid_from": 20_i64, "valid_until": null,
            "record": mongodb::bson::serialize_to_bson(&binding).unwrap(),
        })
        .await
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let used = Arc::new(AtomicU64::new(8));
    let app = Router::new()
        .route("/select/0/prometheus/api/v1/export", get(exporter))
        .with_state(used.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let source =
        MetricsClient::internal(&format!("http://{endpoint}/select/0/prometheus/")).unwrap();
    let range = Range { from: 20, to: 40 };
    let sandbox = binding.sandbox_id.as_str();
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"skip": 1},
            "data": {"failCommands": ["commitTransaction"], "appName": app_name, "errorCode": 2},
        })
        .await
        .unwrap();
    let partial = execute(&database, &source, sandbox, range).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    crate::store::test_visibility::majority_barrier(&admin, &name).await;
    assert_eq!(partial, Err(UNAVAILABLE));
    let rows = database.collection::<Document>(COLLECTION);
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 1);

    let retry = execute(&database, &source, sandbox, range).await.unwrap();
    assert_eq!(retry["inserted"], 1);
    assert_eq!(retry["replayed"], 1);
    let exact = execute(&database, &source, sandbox, range).await.unwrap();
    assert_eq!(exact["inserted"], 0);
    assert_eq!(exact["replayed"], 2);
    used.store(12, Ordering::Relaxed);
    let conflict = execute(&database, &source, sandbox, range).await.unwrap();
    assert_eq!(conflict["disputed"], 1);
    used.store(8, Ordering::Relaxed);
    assert_eq!(
        execute(&database, &source, sandbox, range).await.unwrap()["disputed"],
        1
    );
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 3);
    used.store(0, Ordering::Relaxed);
    let rejected = execute(&database, &source, sandbox, range).await.unwrap();
    assert_eq!(rejected["status"], "no_attributable_observations");
    assert_eq!(rejected["unattributed_source_series"], 2);
    assert_eq!(rejected["unattributed_source_samples"], 2);
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 3);
    server.abort();
    database.drop().await.unwrap();
}
