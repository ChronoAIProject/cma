use super::activity_retention::{
    assert_no_schema_mutation, observed_client, open_activity, RETENTION,
};
use super::*;

#[derive(Clone, Default)]
struct CapturedLog(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn startup_warning_uses_only_fixed_fields_and_excludes_parent_payloads() {
    use tracing::Instrument;
    let output = CapturedLog::default();
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let mut observer = StartupObserver::new(StartupSchemaMode::Observe);
    let span = tracing::info_span!(
        "untrusted_parent",
        private_payload = "never-log-this-payload"
    );
    observer
        .assess::<()>(Contract::Activity, async {
            Err(SchemaFinding::InspectionUnavailable)
        })
        .instrument(span)
        .await
        .unwrap();
    let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("inspection_unavailable"), "{text}");
    assert!(text.contains("continue"), "{text}");
    assert!(!text.contains("never-log-this-payload"), "{text}");
    assert!(!text.contains("untrusted_parent"), "{text}");
}
use crate::{
    config::StartupSchemaMode,
    store::{
        bindings::BindingCollections,
        schema::{Assessment, Contract, SchemaFinding, StartupObserver},
    },
};

#[tokio::test]
async fn every_schema_finding_is_advisory_only_in_observation_mode() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    for reason in [
        SchemaFinding::MissingCollection,
        SchemaFinding::MissingQueryIndex,
        SchemaFinding::MissingUniqueIndex,
        SchemaFinding::LegacyBindingIndex,
        SchemaFinding::TtlAbsent,
        SchemaFinding::DefinitionMismatch,
        SchemaFinding::InspectionUnavailable,
    ] {
        for mode in [StartupSchemaMode::Observe, StartupSchemaMode::Enforce] {
            let mut observer = StartupObserver::new(mode);
            let result = observer
                .assess(Contract::Binding, async {
                    Ok(Assessment {
                        capability: false,
                        findings: [reason].into(),
                    })
                })
                .await;
            assert_eq!(result.is_ok(), mode == StartupSchemaMode::Observe);
            if let Ok(assessment) = result {
                assert!(!assessment.capability);
                assert!(assessment.findings.contains(&reason));
            }
        }
    }
    let text = handle.render();
    for reason in [
        "missing_collection",
        "missing_query_index",
        "missing_unique_index",
        "legacy_binding_index",
        "ttl_absent",
        "definition_mismatch",
        "inspection_unavailable",
    ] {
        assert!(text.contains(&format!("reason=\"{reason}\"")), "{text}");
    }
    for outcome in ["drift", "unavailable"] {
        assert!(text.contains(&format!("outcome=\"{outcome}\"")), "{text}");
    }
    assert!(!text.contains("ready"));
}

#[tokio::test(start_paused = true)]
async fn observer_shares_one_five_second_budget_and_never_confuses_timeout_with_compatibility() {
    let mut observer = StartupObserver::new(StartupSchemaMode::Observe);
    observer
        .assess::<()>(Contract::Binding, async {
            tokio::time::sleep(Duration::from_secs(4)).await;
            Ok(Assessment::default())
        })
        .await
        .unwrap();
    let started = tokio::time::Instant::now();
    let result = observer
        .assess::<()>(Contract::Activity, async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok(Assessment::default())
        })
        .await
        .unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    assert_eq!(
        result.findings,
        [SchemaFinding::InspectionUnavailable].into()
    );
    let result = observer
        .assess::<()>(Contract::Activity, async {
            panic!("exhausted budget must not start another inspection")
        })
        .await
        .unwrap();
    assert!(result
        .findings
        .contains(&SchemaFinding::InspectionUnavailable));
    let mut strict = StartupObserver::new(StartupSchemaMode::Enforce);
    assert!(strict
        .assess::<()>(Contract::Binding, std::future::pending())
        .await
        .is_err());
}

#[test]
fn untrusted_or_unbounded_cursor_metadata_is_rejected_without_returning_payloads() {
    for response in [
        doc! {},
        doc! {"cursor":{"id":1_i64,"firstBatch":[]}},
        doc! {"cursor":{"id":0_i64,"firstBatch":["private-payload"]}},
    ] {
        let error = crate::store::schema::index_contract_v1::entries(&response).unwrap_err();
        assert!(!error.contains("private-payload"));
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn empty_converted_stores_observe_or_refuse_without_bootstrap() {
    let (admin, name) = database().await;
    let (client, commands) =
        observed_client(&std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(), &name).await;
    for mode in [StartupSchemaMode::Observe, StartupSchemaMode::Enforce] {
        let mut observer = StartupObserver::new(mode);
        let bindings = BindingCollections::open(&client.database(&name), &mut observer).await;
        assert_eq!(bindings.is_ok(), mode == StartupSchemaMode::Observe);
        assert_eq!(
            open_activity(&client, &name, RETENTION, mode).await.is_ok(),
            mode == StartupSchemaMode::Observe
        );
    }
    MongoActivityStore::open(&client, &name, RETENTION)
        .await
        .unwrap();
    assert_no_schema_mutation(&commands);
    assert!(admin
        .database(&name)
        .list_collection_names()
        .await
        .unwrap()
        .is_empty());
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn absent_ttl_never_expires_retained_rows_or_bootstraps_empty_collection() {
    let (admin, name) = database().await;
    let db = admin.database(&name);
    db.create_collection(ACTIVITY).await.unwrap();
    let (client, commands) =
        observed_client(&std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(), &name).await;
    let record = doc! {"_id":"old-record","document_schema_version":1_i64,"recorded_at":DateTime::from_millis(0),"record":{"fixture":"retained"}};
    for populated in [false, true] {
        if populated {
            db.collection(ACTIVITY)
                .insert_one(record.clone())
                .await
                .unwrap();
        }
        let before = index_documents(&db, ACTIVITY).await;
        for mode in [StartupSchemaMode::Observe, StartupSchemaMode::Enforce] {
            assert_eq!(
                open_activity(&client, &name, RETENTION, mode).await.is_ok(),
                mode == StartupSchemaMode::Observe
            );
        }
        assert_eq!(index_documents(&db, ACTIVITY).await, before);
        assert_eq!(
            db.collection::<Document>(ACTIVITY)
                .find_one(doc! {})
                .await
                .unwrap(),
            populated.then(|| record.clone())
        );
    }
    assert_no_schema_mutation(&commands);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB replica set"]
async fn binding_drift_matrix_preserves_schema_in_both_modes() {
    for case in [
        "ready",
        "query",
        "unique",
        "legacy",
        "legacy_only",
        "order",
        "hidden",
        "validator",
    ] {
        let (admin, name) = database().await;
        let db = admin.database(&name);
        crate::store::migrations::prepare_bindings_for_test(&db).await;
        let conversations = db.collection::<Document>("binding_conversations");
        match case {
            "query" => {
                conversations.drop_index("pending_1__id_1").await.unwrap();
            }
            "unique" | "legacy_only" => {
                conversations
                    .drop_index("binding_generation_agent")
                    .await
                    .unwrap();
            }
            "order" | "hidden" => {
                conversations
                    .drop_index("binding_generation_agent")
                    .await
                    .unwrap();
                conversations
                    .create_index(
                        IndexModel::builder()
                            .keys(if case == "order" {
                                doc! {"generation":1,"binding_id":1,"record.agent_id":1}
                            } else {
                                doc! {"binding_id":1,"generation":1,"record.agent_id":1}
                            })
                            .options(
                                IndexOptions::builder()
                                    .name("binding_generation_agent".to_owned())
                                    .unique(true)
                                    .hidden(case == "hidden")
                                    .build(),
                            )
                            .build(),
                    )
                    .await
                    .unwrap();
            }
            "validator" => {
                db.run_command(doc! {"collMod":"binding_conversations","validator":{"unexpected":{"$exists":true}}}).await.unwrap();
            }
            _ => {}
        }
        if ["legacy", "legacy_only"].contains(&case) {
            conversations
                .create_index(
                    IndexModel::builder()
                        .keys(doc! {"record.agent_id":1})
                        .options(IndexOptions::builder().unique(true).build())
                        .build(),
                )
                .await
                .unwrap();
        }
        let before = index_documents(&db, "binding_conversations").await;
        let (client, commands) =
            observed_client(&std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(), &name).await;
        for mode in [StartupSchemaMode::Observe, StartupSchemaMode::Enforce] {
            let result =
                BindingCollections::open(&client.database(&name), &mut StartupObserver::new(mode))
                    .await;
            assert_eq!(
                result.is_ok(),
                case == "ready" || mode == StartupSchemaMode::Observe,
                "{case}: {mode:?}"
            );
        }
        assert_no_schema_mutation(&commands);
        assert_eq!(index_documents(&db, "binding_conversations").await, before);
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn existing_document_version_guards_still_refuse_in_both_modes() {
    let (client, name) = database().await;
    let db = client.database(&name);
    for collection in [ACTIVITY, "provider_bindings"] {
        db.collection(collection)
            .insert_one(doc! {"_id":"unknown","document_schema_version":999_i64})
            .await
            .unwrap();
    }
    for mode in [StartupSchemaMode::Observe, StartupSchemaMode::Enforce] {
        assert!(matches!(
            open_activity(&client, &name, RETENTION, mode).await,
            Err(StoreError::Malformed(_))
        ));
        assert!(matches!(
            BindingCollections::open(&db, &mut StartupObserver::new(mode)).await,
            Err(StoreError::Malformed(_))
        ));
    }
    db.drop().await.unwrap();
}

mod production;
