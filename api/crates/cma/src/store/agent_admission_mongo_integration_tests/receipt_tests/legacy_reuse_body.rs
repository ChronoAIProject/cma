use super::*;
use crate::store::agent_admission_records::binding_id;
use futures_util::TryStreamExt;
use mongodb::error::{TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT};
use std::sync::atomic::{AtomicUsize, Ordering};

async fn body_refusal(code: i32, labels: &[&str], expected: &str, expected_writers: usize) {
    let (admin, database, initial, _, _, original) = fixture("legacy_reuse_body").await;
    let creation = initial
        .admit_agent(original.clone())
        .await
        .unwrap()
        .creation()
        .clone();
    let mut request = original;
    request.agent_id = AgentId::generate();
    request.worktree_id = WorktreeId::generate();
    request.creation_key = "trigger-legacy-reuse-body".into();
    request.origin = AgentOrigin::Trigger {
        trigger_id: "trg_legacy_reuse_body".into(),
        launch_id: "lch_legacy_reuse_body".into(),
    };
    request.first_message = Some(
        layer2_core::sandbox::FirstMessageRecord::new(
            request.creation_key.clone(),
            "Original prompt.",
            NOW,
        )
        .unwrap(),
    );
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let app = format!("cma-legacy-reuse-body-{}", uuid::Uuid::new_v4().simple());
    let writers = Arc::new(AtomicUsize::new(0));
    let observed_writers = writers.clone();
    let inserts = Arc::new(Mutex::new(Vec::new()));
    let observed_inserts = inserts.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(app.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_bool("startTransaction").ok() == Some(true) {
                observed_writers.fetch_add(1, Ordering::SeqCst);
            }
            if let Ok(collection) = event.command.get_str("insert") {
                observed_inserts.lock().unwrap().push(collection.to_owned());
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let filter = doc! {"_id": creation.agent.id.as_str()};
    let before = initial
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    let original_log = retained_log(&initial, &creation.agent.id).await;
    let configure = admin.database("admin");
    configure.run_command(doc! {
        "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
        "data": {"failCommands": ["insert"], "appName": app, "errorCode": code, "errorLabels": labels.to_vec()},
    }).await.unwrap();
    inserts.lock().unwrap().clear();
    writers.store(0, Ordering::SeqCst);
    let outcome = store.admit_agent_reusing_profile(request.clone()).await;
    let started = writers.load(Ordering::SeqCst);
    let first_insert = inserts.lock().unwrap().first().cloned();
    configure
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    let after = initial.agents.find_one(filter).await.unwrap().unwrap();
    let has_binding = initial
        .agent_reuse_bindings
        .find_one(doc! {"_id": binding_id(&request).unwrap()})
        .await
        .unwrap()
        .is_some();
    let final_log = retained_log(&initial, &creation.agent.id).await;
    admin.database(&database).drop().await.unwrap();
    let class = match outcome {
        Ok(AgentAdmissionResult::Existing(reused)) => {
            assert_eq!(reused.agent.id, creation.agent.id);
            assert!(reused.agent.conversation.first_message.is_some());
            "Existing"
        }
        Ok(AgentAdmissionResult::Created(_)) => "Created",
        Err(SandboxStoreError::Unavailable(message)) => {
            if labels.contains(&UNKNOWN_TRANSACTION_COMMIT_RESULT) {
                assert_eq!(
                    message,
                    "Agent admission transaction outcome is unavailable"
                );
            }
            "Unavailable"
        }
        Err(SandboxStoreError::SequenceConflict) => "SequenceConflict",
        Err(_) => "Other error",
    };
    assert_eq!(first_insert.as_deref(), Some(store.agent_log.name()));
    assert_eq!((class, started), (expected, expected_writers));
    let committed = expected == "Existing";
    assert_eq!(has_binding, committed);
    if committed {
        assert_eq!(final_log.len(), original_log.len() + 1);
        assert!(final_log.starts_with(&original_log));
    } else {
        assert_eq!(after, before);
        assert_eq!(final_log, original_log);
    }
}

async fn retained_log(store: &MongoSandboxStore, agent: &AgentId) -> Vec<Document> {
    store
        .agent_log
        .find(doc! {"agent_id": agent.as_str()})
        .sort(doc! {"seq": 1_i32})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn legacy_body_uncertainty_overrides_transient() {
    body_refusal(
        1,
        &[
            UNKNOWN_TRANSACTION_COMMIT_RESULT,
            TRANSIENT_TRANSACTION_ERROR,
        ],
        "Unavailable",
        1,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn legacy_body_uncertainty_overrides_duplicate() {
    body_refusal(
        11000,
        &[
            UNKNOWN_TRANSACTION_COMMIT_RESULT,
            TRANSIENT_TRANSACTION_ERROR,
        ],
        "Unavailable",
        1,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn legacy_body_duplicate_only_keeps_its_existing_refusal() {
    body_refusal(11000, &[], "Unavailable", 1).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn legacy_body_confirmed_abort_retries() {
    body_refusal(112, &[TRANSIENT_TRANSACTION_ERROR], "Existing", 2).await;
}
