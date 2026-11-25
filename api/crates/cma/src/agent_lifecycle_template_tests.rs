//! Real admission and Mongo persistence keep template validation ahead of all effects.
use super::*;
use layer2_core::sandbox::store::{AgentStore, InMemorySandboxStore, SandboxStore};
use mongodb::bson::{doc, Document};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn predecessor_spreadsheet_v1_replays_its_original_effects_after_reopen() {
    const KEY: &str = "retained-spreadsheet";
    const FIRST_MESSAGE: &str =
        "Analyze the spreadsheet at not-a-url and point out trends, outliers and what to do next.";
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI is required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!(
        "cma_spreadsheet_replay_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = Arc::new(InMemorySandboxStore::default());
    let workspace = fixture::seed(&store).await;
    let owner = fixture::context("owner");
    let request = request(json!({
        "name": "Retained spreadsheet",
        "workspace_id": workspace.id.as_str(),
        "profile": {"model": "model", "system_prompt": "  Keep my original instructions.\n"},
        "template": {"id": "spreadsheet", "version": 1, "fields": {"link": "not-a-url"}},
        "initial_message": FIRST_MESSAGE,
    }));
    let operations = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, db.name())
            .await
            .unwrap(),
    );
    // This is the predecessor configuration for admitting Spreadsheet v1.
    let predecessor = fixture::facade(store.clone(), profiles, operations.clone())
        .with_task_experience(fixture::task_policy());
    let receipt = predecessor
        .create(&owner.caller, Some(&owner), None, request.clone(), KEY)
        .await
        .unwrap();
    let starting = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(agent) = store
                .list_agents(&owner.caller.subject, &Default::default())
                .await
                .unwrap()
                .items
                .into_iter()
                .next()
            {
                break agent;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(starting.status, AgentStatus::Starting);
    assert_eq!(starting.workspace_id, workspace.id);
    assert_eq!(starting.sandbox_id, workspace.sandbox_id);
    assert_eq!(
        starting.conversation.first_message.as_ref().unwrap().text,
        FIRST_MESSAGE
    );
    assert_eq!(
        starting.profile.content.system_prompt,
        format!(
            "{}\n\n  Keep my original instructions.\n",
            guidance::managed_v3()
        )
    );
    let mut ready = starting.clone();
    ready.conversation.conversation_id = Some("spreadsheet-conversation".into());
    ready.conversation.runtime_generation = Some(1);
    ready
        .transition(AgentStatus::Ready, layer2_core::now_rfc3339(), None)
        .unwrap();
    store.save_agent(starting.revision, ready).await.unwrap();
    let admitted = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let record = operations
                .read_for_owner(&owner.caller.subject, &receipt.id)
                .await
                .unwrap()
                .unwrap();
            match record.status.as_str() {
                "succeeded" => break record,
                "queued" | "running" => tokio::time::sleep(Duration::from_millis(10)).await,
                status => panic!(
                    "unexpected Spreadsheet worker status {status}: {:?}",
                    record.error
                ),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(admitted.agent_id.as_deref(), Some(starting.id.as_str()));
    assert_profile_counts(&db, 1).await;
    let original_agent = store.get_agent(&starting.id).await.unwrap().unwrap();
    let original_parent = store
        .get_sandbox(&workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let original_workspace = store.get_workspace(&workspace.id).await.unwrap().unwrap();
    let original_worktree = store
        .get_worktree(&starting.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let operations_collection = db.collection::<Document>("agent_lifecycle_operations");
    let before = operations_collection
        .find_one(doc! {"_id": &receipt.id})
        .await
        .unwrap()
        .unwrap();
    drop(predecessor);
    drop(operations);

    let reopened = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
    let reopened_profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, db.name())
            .await
            .unwrap(),
    );
    let restarted = fixture::facade(store.clone(), reopened_profiles, reopened.clone())
        .with_session_guidance(true, false);
    let _slots = restarted
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    let replay = restarted
        .create(&owner.caller, Some(&owner), None, request.clone(), KEY)
        .await
        .unwrap();
    assert_eq!(replay.id, receipt.id);
    let retained = reopened
        .read_for_owner(&owner.caller.subject, &replay.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_vec(&retained.input.request).unwrap(),
        serde_json::to_vec(&request).unwrap()
    );
    assert_eq!(
        retained.input.managed_agent_instructions.as_deref(),
        Some(guidance::managed_v3())
    );
    assert_eq!(
        validation::admitted_first_message(&retained.input).as_deref(),
        Some(FIRST_MESSAGE)
    );

    let mut changed = request;
    changed
        .template
        .as_mut()
        .unwrap()
        .fields
        .insert("link".into(), "another-non-url".into());
    changed.initial_message = Some("Analyze the spreadsheet at another-non-url and point out trends, outliers and what to do next.".into());
    assert!(matches!(
        restarted
            .create(&owner.caller, Some(&owner), None, changed, KEY)
            .await,
        Err(AgentLifecycleProvisioningError::Conflict(_))
    ));
    assert_eq!(
        operations_collection
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        operations_collection
            .find_one(doc! {"_id": &receipt.id})
            .await
            .unwrap(),
        Some(before)
    );
    assert_profile_counts(&db, 1).await;
    assert_eq!(
        store
            .list_agents(&owner.caller.subject, &Default::default())
            .await
            .unwrap()
            .items,
        vec![original_agent]
    );
    assert_eq!(
        store
            .list_sandboxes(&owner.caller.subject, &Default::default())
            .await
            .unwrap()
            .items,
        vec![original_parent]
    );
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap(),
        Some(original_workspace)
    );
    assert_eq!(
        store.get_worktree(&starting.worktree_id).await.unwrap(),
        Some(original_worktree)
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn task_template_admission_validates_before_effects_and_replays_exact_input() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI is required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!(
        "cma_template_test_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let operations = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, db.name())
            .await
            .unwrap(),
    );
    let store = Arc::new(InMemorySandboxStore::default());
    let workspace = fixture::seed(&store).await;
    let policy = serde_json::from_value(
        json!({"mode":"all","accounts":[],"capabilities":["task_home_admission"]}),
    )
    .unwrap();
    let service = fixture::facade(store.clone(), profiles, operations.clone())
        .with_task_experience(Arc::new(policy));
    let _slots = service
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    let owner = fixture::context("owner");
    let vectors: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../layer2-core/src/sandbox/task_template_vectors.json"
    ))
    .unwrap();
    for vector in &vectors {
        let mut invalid = vector["template"].clone();
        invalid["fields"] = json!({});
        let request = request(
            json!({"name":"Task","workspace_id":workspace.id.as_str(),"template":invalid,"initial_message":vector["message"]}),
        );
        assert!(matches!(
            service
                .create(&owner.caller, Some(&owner), None, request, "invalid")
                .await,
            Err(AgentLifecycleProvisioningError::Template(_))
        ));
    }
    assert_eq!(
        db.collection::<Document>("agent_lifecycle_operations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .list_sandboxes(&owner.caller.subject, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_profile_counts(&db, 0).await;
    assert!(store
        .list_agents(&owner.caller.subject, &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
    let vector = &vectors[0];
    let request = request(
        json!({"name":"Task","workspace_id":workspace.id.as_str(),"template":vector["template"],"initial_message":vector["message"]}),
    );
    let first = service
        .create(
            &owner.caller,
            Some(&owner),
            None,
            request.clone(),
            "same-template",
        )
        .await
        .unwrap();
    let replay = service
        .create(
            &owner.caller,
            Some(&owner),
            None,
            request.clone(),
            "same-template",
        )
        .await
        .unwrap();
    assert_eq!(first.id, replay.id);
    assert_eq!(
        db.collection::<Document>("agent_lifecycle_operations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    let reopened = MongoAgentLifecycleStore::open(&db).await.unwrap();
    let record = reopened
        .read_for_owner(&owner.caller.subject, &first.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&record.input.request).unwrap(),
        serde_json::to_value(&request).unwrap()
    );
    let mut changed = request.clone();
    changed
        .template
        .as_mut()
        .unwrap()
        .fields
        .insert("topic".into(), "Different topic".into());
    changed.initial_message = Some(
        layer2_core::sandbox::task_template::compile_task_template(
            "research",
            1,
            &changed.template.as_ref().unwrap().fields,
        )
        .unwrap(),
    );
    assert!(matches!(
        service
            .create(&owner.caller, Some(&owner), None, changed, "same-template")
            .await,
        Err(AgentLifecycleProvisioningError::Conflict(_))
    ));
    db.drop().await.unwrap();
}
