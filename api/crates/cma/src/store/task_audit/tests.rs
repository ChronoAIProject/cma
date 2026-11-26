use super::*;
use report::Entry;

#[test]
fn incomplete_evidence_is_partitioned_and_private_ids_never_enter_metrics() {
    let selection = Selection {
        from: "2026-01-01T00:00:00Z".into(),
        to: None,
        overdue_seconds: 60,
    };
    let mut report = Report::new(selection, 1, 1000, 1000, 4);
    for result in [
        "verified_terminal",
        "receipt_terminal",
        "evidence_missing",
        "unfinished",
    ] {
        report.push(Entry {
            command_id: "private-command".into(),
            agent_id: Some("private-agent".into()),
            admitted_at: Some("2026-01-01T00:00:00Z".into()),
            status: Some("in_progress"),
            age_seconds: Some(999),
            overdue: result == "unfinished",
            result,
        });
    }
    assert_eq!(report.classes.values().sum::<u64>(), report.accepted);
    assert_eq!(report.overdue, 1);
    assert_eq!(report.exceptions.len(), 2);
    let metrics = report.metrics();
    assert!(metrics.contains("classification_complete{window=\"release\"} 0"));
    assert!(metrics.contains("audit_tasks{window=\"release\",result=\"evidence_missing\"} 1"));
    assert!(!metrics.contains("private-"));
    let artifact = serde_json::to_value(&report).unwrap();
    assert_eq!(artifact["writes"], false);
    assert_eq!(artifact["exceptions"][0]["result"], "evidence_missing");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable MongoDB 8 replica set"]
async fn read_only_handles_never_create_schema_on_empty_database() {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_audit_empty_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    let store = MongoSandboxStore::audit_handles(&client, &name);
    assert!(db.list_collection_names().await.unwrap().is_empty());
    let selection = Selection {
        from: "2026-01-01T00:00:00Z".into(),
        to: None,
        overdue_seconds: 60,
    };
    assert!(observe(&store, &selection).await.is_err());
    assert!(db.list_collection_names().await.unwrap().is_empty());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable MongoDB 8 replica set"]
async fn message_selection_is_snapshot_consistent_bounded_and_keeps_bad_rows() {
    use mongodb::bson::{doc, Document};
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_audit_bounds_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    let commands = crate::store::durability::majority_collection::<Document>(&db, "agent_commands");
    commands
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"record.receipt.admitted_at":1,"_id":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name(INDEX.to_string())
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    let row = |id: &str, kind: &str, at: &str| doc! {"_id":id,"record":{"receipt":{"admitted_at":at,"envelope":{"command":{"type":kind}}}}};
    commands
        .insert_many([
            row("bad-message", "message", "2026-01-01T00:00:00Z"),
            row("control", "interrupt", "2026-01-01T00:00:00Z"),
            row("old-message", "message", "2025-12-31T23:59:59Z"),
            row("end-exclusive", "message", "2026-01-02T00:00:00Z"),
        ])
        .await
        .unwrap();
    let store = MongoSandboxStore::audit_handles(&client, &name);
    let selection = Selection {
        from: "2026-01-01T00:00:00Z".into(),
        to: Some("2026-01-02T00:00:00Z".into()),
        overdue_seconds: 60,
    };
    let report = observe(&store, &selection).await.unwrap();
    assert_eq!(report.accepted, 1);
    assert_eq!(report.classes["invalid_record"], 1);
    assert_eq!(report.exceptions[0].command_id, "bad-message");
    // Exercise the real background/cache wiring. Explicit release selection
    // publishes only the explicit release population, with no identifiers.
    let cache = crate::telemetry::task_cohort::Cache::default();
    let (stop, signal) = tokio::sync::watch::channel(false);
    let worker = crate::store::worker_observations::Observations::new(db.clone())
        .with_task_audit(cache.clone(), Some(selection.clone()))
        .spawn(signal);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if cache
                .render()
                .contains("cma_task_cohort_sample_success{window=\"release\"} 1")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap();
    assert!(!cache.render().contains("window=\"previous_day\""));
    assert!(!cache.render().contains("bad-message"));

    // Establish a snapshot, commit a competing deletion, then run both census
    // and classification in that original snapshot. No mixed-time result.
    let mut session = client.start_session().await.unwrap();
    session
        .start_transaction()
        .read_concern(ReadConcern::snapshot())
        .await
        .unwrap();
    commands
        .find_one(doc! {"_id":"bad-message"})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    commands
        .delete_one(doc! {"_id":"bad-message"})
        .await
        .unwrap();
    let from = layer2_core::seconds_from_rfc3339(&selection.from).unwrap();
    let raced = read(
        &store,
        &selection,
        from,
        from + 86400,
        from + 100000,
        &mut session,
    )
    .await
    .unwrap();
    assert_eq!(raced.accepted, 1);
    assert_eq!(raced.classes["invalid_record"], 1);
    session.abort_guarded().await.unwrap();
    assert_eq!(observe(&store, &selection).await.unwrap().accepted, 0);
    commands
        .insert_many((0..=MAX_ROWS).map(|n| row(&format!("limit-{n}"), "message", &selection.from)))
        .await
        .unwrap();
    assert!(matches!(observe(&store, &selection).await, Err("budget")));
    commands.drop_index(INDEX).await.unwrap();
    assert!(matches!(observe(&store, &selection).await, Err("database")));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable MongoDB 8 replica set"]
async fn dropping_a_qualified_collection_fails_sampling_and_retains_last_good() {
    use mongodb::bson::Document;

    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_audit_catalog_{}", uuid::Uuid::new_v4().simple());
    let db = client.database(&name);
    let commands = crate::store::durability::majority_collection::<Document>(&db, "agent_commands");
    let index = mongodb::IndexModel::builder()
        .keys(doc! {"record.receipt.admitted_at":1,"_id":1})
        .options(
            mongodb::options::IndexOptions::builder()
                .name(INDEX.to_string())
                .build(),
        )
        .build();
    commands.create_index(index.clone()).await.unwrap();
    let store = MongoSandboxStore::audit_handles(&client, &name);
    let selection = Selection {
        from: "2026-01-01T00:00:00Z".into(),
        to: Some("2026-01-02T00:00:00Z".into()),
        overdue_seconds: 60,
    };
    assert_eq!(observe(&store, &selection).await.unwrap().accepted, 0);
    commands
        .insert_one(doc! {"_id":"invalid-command","record":{"receipt":{
            "admitted_at":&selection.from,"envelope":{"command":{"type":"message"}}
        }}})
        .await
        .unwrap();
    let cache = crate::telemetry::task_cohort::Cache::default();
    cache.update(
        observe(&store, &selection)
            .await
            .map(|report| report.metrics()),
    );
    assert!(cache
        .render()
        .contains("cma_task_cohort_accepted_tasks{window=\"release\"} 1"));

    let identity = super::super::migrations::require_command_cohort_index(&db)
        .await
        .unwrap();
    let observed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    commands.drop().await.unwrap();
    let result = observe_snapshot(&store, &selection, observed, &db, &identity).await;
    assert!(matches!(result, Err("database")));
    cache.update(result.map(|report| report.metrics()));
    let failed = cache.render();
    assert!(failed.contains("cma_task_cohort_sample_success{window=\"release\"} 0"));
    assert!(failed.contains("cma_task_cohort_accepted_tasks{window=\"release\"} 1"));

    commands.create_index(index).await.unwrap();
    assert!(matches!(
        observe_snapshot(&store, &selection, observed, &db, &identity).await,
        Err("invalid_snapshot")
    ));
    assert_eq!(observe(&store, &selection).await.unwrap().accepted, 0);
    db.drop().await.unwrap();
}
