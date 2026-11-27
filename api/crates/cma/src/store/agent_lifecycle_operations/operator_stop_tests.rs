use super::*;
use layer2_core::admission::AdmissionControlStore;
use mongodb::Client;

mod home_capacity;

async fn fixture() -> (Client, String, MongoAgentLifecycleStore) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
        .expect("CHRONO_TEST_MONGODB_URI must name a disposable replica set");
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_lifecycle_stop_{}", uuid::Uuid::new_v4().simple());
    let store = MongoAgentLifecycleStore::open(&client.database(&name))
        .await
        .unwrap();
    (client, name, store)
}

fn input() -> LifecycleInput {
    LifecycleInput {
        behavior: LifecycleBehavior::PromptRepository,
        managed_agent_instructions: Some("Original managed instructions".into()),
        setup_message: Some("Original admitted setup".into()),
        actor: None,
        request: serde_json::from_value(serde_json::json!({
            "name": "Research", "image": {"id": "latest-image"}
        }))
        .unwrap(),
        resolved_image: Some("pinned-image".into()),
        capacity: Some(layer3_adapter::SandboxCapacity::default()),
        sandbox_availability: Default::default(),
        capabilities: vec![Capability::UseOwnSandboxes],
    }
}

async fn stop(client: &Client, name: &str) {
    super::super::admission_control::MongoAdmissionControlStore::open(client, name)
        .await
        .unwrap()
        .change(
            0,
            &SubjectId::new("operator"),
            "contain incident",
            true,
            100,
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fresh_lifecycle_insert_is_paused_with_spend_metering_off() {
    let (client, name, store) = fixture().await;
    stop(&client, &name).await;
    assert!(matches!(
        store
            .create(&SubjectId::new("owner"), "fresh", input())
            .await,
        Err(LifecycleCreateError::AdmissionPaused)
    ));
    assert_eq!(store.operations.count_documents(doc! {}).await.unwrap(), 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admitted_lifecycle_replays_and_worker_claims_keep_original_authority_after_stop() {
    let (client, name, store) = fixture().await;
    let owner = SubjectId::new("owner");
    let original = input();
    let CreateResult::Created(id) = store
        .create(&owner, "admitted", original.clone())
        .await
        .unwrap()
    else {
        panic!("new durable operation expected");
    };
    stop(&client, &name).await;
    let reopened = MongoAgentLifecycleStore::open(&client.database(&name))
        .await
        .unwrap();
    let mut later = original.clone();
    later.resolved_image = Some("later-image".into());
    later.setup_message = Some("Later deployment setup".into());
    let CreateResult::Existing(replayed) =
        reopened.create(&owner, "admitted", later).await.unwrap()
    else {
        panic!("admitted operation must replay");
    };
    assert_eq!(replayed.id, id);
    assert_eq!(replayed.input.resolved_image, original.resolved_image);
    assert_eq!(replayed.input.setup_message, original.setup_message);
    assert!(reopened.claim(&id).await.unwrap().is_some());
    assert!(reopened.claim(&id).await.unwrap().is_none());
    assert_eq!(store.operations.count_documents(doc! {}).await.unwrap(), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn paused_insert_rechecks_a_winner_committed_after_the_outer_replay_lookup() {
    let (client, name, store) = fixture().await;
    let owner = SubjectId::new("owner");
    let original = input();
    let CreateResult::Created(id) = store
        .create(&owner, "raced", original.clone())
        .await
        .unwrap()
    else {
        panic!("first writer must create the operation");
    };
    let mut stale = store
        .replay(&owner, "raced", &original.request)
        .await
        .unwrap()
        .unwrap();
    // Continue exactly after create's outer replay miss, with the candidate
    // already constructed. Another writer and the stop have since committed.
    stale.id = "uncommitted-candidate".into();
    stop(&client, &name).await;
    let CreateResult::Existing(replayed) = store.finish_create(stale.clone(), None).await.unwrap()
    else {
        panic!("the exact durable winner must remain a continuation");
    };
    assert_eq!(replayed.id, id);
    stale.input.request.name = "Different submitted request".into();
    assert!(matches!(
        store.finish_create(stale, None).await,
        Err(LifecycleCreateError::Store(
            StoreError::RevisionConflict { .. }
        ))
    ));
    assert_eq!(store.operations.count_documents(doc! {}).await.unwrap(), 1);
    assert!(store
        .operations
        .find_one(doc! {"_id": "uncommitted-candidate"})
        .await
        .unwrap()
        .is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn simultaneous_exact_lifecycle_creates_replay_one_durable_operation() {
    let (client, name, store) = fixture().await;
    let second = MongoAgentLifecycleStore::open(&client.database(&name))
        .await
        .unwrap();
    let owner = SubjectId::new("owner");
    let (first, second) = tokio::join!(
        store.create(&owner, "concurrent", input()),
        second.create(&owner, "concurrent", input()),
    );
    let id = |result: CreateResult| match result {
        CreateResult::Created(id) => id,
        CreateResult::Existing(record) => record.id.clone(),
    };
    assert_eq!(id(first.unwrap()), id(second.unwrap()));
    assert_eq!(store.operations.count_documents(doc! {}).await.unwrap(), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn review_round_one_lifecycle_create_keeps_a_single_write_after_a_majority_read() {
    use mongodb::event::{command::CommandEvent, EventHandler};
    use mongodb::options::ClientOptions;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let trace = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            observed.lock().unwrap().push(event.command);
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!(
        "cma_lifecycle_single_write_{}",
        uuid::Uuid::new_v4().simple()
    );
    let store = MongoAgentLifecycleStore::open(&client.database(&name))
        .await
        .unwrap();
    trace.lock().unwrap().clear();
    store
        .create(&SubjectId::new("owner"), "single-write", input())
        .await
        .unwrap();
    let commands = trace.lock().unwrap().clone();
    assert!(
        commands
            .iter()
            .all(|command| !command.contains_key("autocommit")),
        "lifecycle creation must retain its single-document write behavior"
    );
    let insert = commands
        .iter()
        .position(|command| command.get_str("insert").ok() == Some(COLLECTION))
        .unwrap();
    let read = &commands[insert - 1];
    assert_eq!(read.get_str("find").ok(), Some("platform_settings"));
    assert_eq!(
        read.get_document("readConcern")
            .unwrap()
            .get_str("level")
            .ok(),
        Some("majority")
    );
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.get_str("insert").ok() == Some(COLLECTION))
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn concurrent_lifecycle_creates_with_different_inputs_preserve_the_original_conflict() {
    let (client, name, store) = fixture().await;
    let second = MongoAgentLifecycleStore::open(&client.database(&name))
        .await
        .unwrap();
    let owner = SubjectId::new("owner");
    let mut different = input();
    different.request.name = "Different submitted request".into();
    let (first, second) = tokio::join!(
        store.create(&owner, "same-key", input()),
        second.create(&owner, "same-key", different),
    );
    let results = [first, second];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Ok(CreateResult::Created(_))))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                Err(LifecycleCreateError::Store(
                    StoreError::RevisionConflict { .. }
                ))
            ))
            .count(),
        1
    );
    assert_eq!(store.operations.count_documents(doc! {}).await.unwrap(), 1);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unreadable_control_pauses_new_lifecycle_work_but_preserves_admitted_replay() {
    let (client, name, store) = fixture().await;
    let owner = SubjectId::new("owner");
    let admitted = store.create(&owner, "admitted", input()).await.unwrap();
    let CreateResult::Created(id) = admitted else {
        panic!("new operation expected");
    };
    let control = super::super::admission_control::MongoAdmissionControlStore::open(&client, &name)
        .await
        .unwrap();
    control
        .change(0, &SubjectId::new("operator"), "incident", true, 100)
        .await
        .unwrap();
    client
        .database(&name)
        .collection::<mongodb::bson::Document>("platform_settings")
        .update_one(
            doc! {"_id": "platform"},
            doc! {"$set": {"admission_control": null}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.create(&owner, "fresh", input()).await,
        Err(LifecycleCreateError::AdmissionPaused)
    ));
    let CreateResult::Existing(replay) = store.create(&owner, "admitted", input()).await.unwrap()
    else {
        panic!("admitted work must retain its authority");
    };
    assert_eq!(replay.id, id);
    assert!(store.claim(&id).await.unwrap().is_some());
    client.database(&name).drop().await.unwrap();
}
