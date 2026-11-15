use super::*;

fn row(step: bson::Bson, attempts: u32, seconds: u64) -> Document {
    let now = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    doc! { "work_pending": true, "work_ready_at": layer2_core::rfc3339_at(now + Duration::from_secs(seconds)),
    "work_lease_expires_at": bson::Bson::Null,
    "record": { "updated_at": layer2_core::rfc3339_at(now),
        "work": { "step": step, "attempts": i64::from(attempts),
            "ready_at": layer2_core::rfc3339_at(now + Duration::from_secs(seconds)) } } }
}

#[test]
fn parked_projection_distinguishes_pending_probes_from_foreground_work_and_reset_attempts() {
    let maintenance = bson::serialize_to_bson(&AgentWorkStep::CloseProcess).unwrap();
    assert_eq!(
        parked_step("agents", &row(maintenance.clone(), 13, 1800)),
        Ok(Some("close_process"))
    );
    assert_eq!(
        parked_step("agents", &row(maintenance.clone(), 4, 1800)),
        Ok(Some("close_process"))
    );
    assert_eq!(
        parked_step("agents", &row(maintenance.clone(), 0, 1800)),
        Ok(None)
    );
    assert_eq!(parked_step("agents", &row(maintenance, 12, 300)), Ok(None));
    assert_eq!(
        parked_step(
            "agents",
            &row(
                bson::serialize_to_bson(&AgentWorkStep::PublishConnection).unwrap(),
                50,
                1800
            )
        ),
        Ok(None)
    );
    let mut lifecycle = row(
        bson::serialize_to_bson(&SandboxWorkStep::RunLifecycleOperation).unwrap(),
        13,
        1800,
    );
    assert_eq!(
        parked_step("sandboxes", &lifecycle),
        Ok(Some("run_lifecycle_operation"))
    );
    lifecycle
        .get_document_mut("record")
        .unwrap()
        .insert("lifecycle", doc! { "operation": { "purpose": "revive" } });
    assert_eq!(parked_step("sandboxes", &lifecycle), Ok(None));
}

#[test]
fn malformed_projection_does_not_claim_a_complete_population() {
    assert_eq!(parked_step("agents", &doc! {}), Err(()));
    let mut invalid = row(
        bson::serialize_to_bson(&AgentWorkStep::CloseProcess).unwrap(),
        13,
        1800,
    );
    invalid
        .get_document_mut("record")
        .unwrap()
        .insert("updated_at", "bad");
    assert_eq!(parked_step("agents", &invalid), Err(()));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn parked_projection_uses_existing_indexes_and_refuses_truncated_samples() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "cma_retry_observations_{}",
        uuid::Uuid::new_v4().simple()
    ));
    for worker in ["agents", "sandboxes"] {
        let mut keys = doc! { "work_pending": 1, "work_ready_at": 1, "work_lease_expires_at": 1 };
        let mut options = mongodb::options::IndexOptions::default();
        if worker == "agents" {
            keys.insert("_id", 1);
            options.name = Some("agents_due_work".into());
        }
        database
            .collection::<Document>(worker)
            .create_index(
                mongodb::IndexModel::builder()
                    .keys(keys)
                    .options(options)
                    .build(),
            )
            .await
            .unwrap();
    }
    let agents = database.collection::<Document>("agents");
    let step = bson::serialize_to_bson(&AgentWorkStep::CloseProcess).unwrap();
    agents
        .insert_one(row(step.clone(), 13, 1800))
        .await
        .unwrap();
    agents
        .insert_one(row(
            bson::serialize_to_bson(&AgentWorkStep::PublishConnection).unwrap(),
            13,
            1800,
        ))
        .await
        .unwrap();
    let observer = ParkedObserver::new(database.clone());
    assert_eq!(
        observer.read("agents").await.unwrap(),
        BTreeMap::from([("close_process", 1)])
    );
    assert!(observer.read("sandboxes").await.unwrap().is_empty());
    agents
        .insert_many((0..ROW_LIMIT).map(|_| row(step.clone(), 13, 1800)))
        .await
        .unwrap();
    assert!(observer.read("agents").await.is_err());
    database.drop().await.unwrap();
}
