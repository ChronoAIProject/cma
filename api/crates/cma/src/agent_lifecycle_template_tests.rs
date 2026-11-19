//! Real admission and Mongo persistence keep template validation ahead of all effects.
use super::*;
use layer2_core::sandbox::store::{AgentStore, InMemorySandboxStore, SandboxStore};
use mongodb::bson::{doc, Document};

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
