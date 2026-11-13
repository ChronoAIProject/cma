use super::*;
use crate::store::agent_capacity_recovery::{records, RecoveryService, RecoveryStream};
use layer2_core::sandbox::agent_capacity_recovery::ports::RecoveryError;

fn count(text: &str, phase: &str, outcome: &str, expected: u64) {
    let sample = text
        .lines()
        .find(|line| {
            line.starts_with("cma_capacity_recovery_discovery_total{")
                && line.contains(&format!("phase=\"{phase}\""))
                && line.contains(&format!("outcome=\"{outcome}\""))
        })
        .unwrap_or_else(|| panic!("missing {phase}/{outcome}: {text}"));
    assert!(sample.ends_with(&format!(" {expected}")), "{sample}");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn recovery_presence_distinguishes_absence_driver_failure_and_application_deadline() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let name = format!("cma_recovery_observation_{}", uuid::Uuid::new_v4().simple());
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _metrics = metrics::set_default_local_recorder(&recorder);
    assert!(!store.capacity_recovery_work_exists().await.unwrap());
    crate::store::agent_log_segments::schema::migrate(&client.database(&name))
        .await
        .unwrap();
    assert!(store.capacity_recovery_work_exists().await.unwrap());
    for data in [
        doc! {"failCommands": ["listCollections"], "appName": &name, "errorCode": 2},
        doc! {"failCommands": ["listCollections"], "appName": &name,
        "blockConnection": true, "blockTimeMS": 2500},
    ] {
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": {"times": 1}, "data": data
            })
            .await
            .unwrap();
        let result = store.capacity_recovery_work_exists().await;
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off"
            })
            .await
            .unwrap();
        assert_eq!(result, Err(RecoveryError::Unavailable));
    }
    let metrics = handle.render();
    for outcome in ["absent", "success", "driver", "deadline"] {
        count(&metrics, "presence", outcome, 1);
    }
    assert!(!metrics.contains("outcome=\"cancelled\""));
    assert_eq!(
        records::collection(&store)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn recovery_query_diagnostics_distinguish_lanes_missing_indexes_and_invalid_candidates() {
    let (client, name) = database("recovery_discovery_diagnostics").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    crate::store::agent_log_segments::schema::migrate(&client.database(&name))
        .await
        .unwrap();
    let service = RecoveryService::new(store.clone(), transactions::access(&store));
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _metrics = metrics::set_default_local_recorder(&recorder);
    for stream in [RecoveryStream::Admitted, RecoveryStream::ExpiredLease] {
        assert!(service
            .discover(stream, None, records::test_support::NOW)
            .await
            .unwrap()
            .requests
            .is_empty());
    }
    let collection = records::collection(&store);
    // Bypass the schema only to model a corrupt pre-existing candidate. It
    // must remain an explicit error, never absence or a successful retry.
    collection
        .insert_one(doc! {"_id": "private-invalid-key", "format_version": 2_i64,
        "work": {"phase": "admitted", "ready_at": records::test_support::NOW}})
        .bypass_document_validation(true)
        .await
        .unwrap();
    assert!(matches!(
        service
            .discover(RecoveryStream::Admitted, None, records::test_support::NOW)
            .await,
        Err(RecoveryError::InvalidEvidence)
    ));
    collection.delete_many(doc! {}).await.unwrap();
    for (stream, index) in [
        (RecoveryStream::Admitted, "agent_capacity_recovery_due"),
        (
            RecoveryStream::ExpiredLease,
            "agent_capacity_recovery_expired",
        ),
    ] {
        collection.drop_index(index).await.unwrap();
        assert!(matches!(
            service
                .discover(stream, None, records::test_support::NOW)
                .await,
            Err(RecoveryError::Unavailable)
        ));
    }
    let metrics = handle.render();
    for phase in ["admitted_query", "expired_query"] {
        count(&metrics, phase, "success", 1);
        count(&metrics, phase, "driver", 1);
    }
    count(&metrics, "candidate_validation", "invalid_evidence", 1);
    assert!(!metrics.contains("private-invalid-key"));
    assert_eq!(store.agents.count_documents(doc! {}).await.unwrap(), 0);
    client.database(&name).drop().await.unwrap();
}
