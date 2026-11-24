//! A worker can resume committed canary authority after a process restart.
use super::*;
use layer2_core::profiles::{AgentProfileStore, SandboxProfileStore, WorkspaceProfileStore};
use mongodb::bson::Document;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn reserved_canary_operation_publishes_three_private_profiles_and_replays_them() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "cma_canary_worker_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = Arc::new(InMemorySandboxStore::default());
    let operations = Arc::new(MongoAgentLifecycleStore::open(&database).await.unwrap());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, database.name())
            .await
            .unwrap(),
    );
    let facade = fixture::facade(store.clone(), profiles.clone(), operations.clone());
    let slots = facade
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    let mut caller = fixture::context("canary-fixture-owner");
    caller.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
    let request = request(json!({"name": "Runtime canary worker"}));
    let receipt = facade
        .create(
            &caller.caller,
            Some(&caller),
            None,
            request.clone(),
            "fixture",
        )
        .await
        .unwrap();
    let owner = &caller.caller.subject;
    let mut operation = operations
        .read_for_owner(owner, &receipt.id)
        .await
        .unwrap()
        .unwrap();
    let image = layer3_adapter::RuntimeImage {
        uri: format!("registry.example/fixture/runtime@sha256:{}", "1".repeat(64)),
        ..Default::default()
    };
    let expected = operations
        .install_canary_worker_fixture(&mut operation, image.clone())
        .await;
    assert!(matches!(
        facade
            .create(&caller.caller, Some(&caller), None, request, &operation.key)
            .await,
        Err(AgentLifecycleProvisioningError::Invalid(_))
    ));
    drop(slots);
    facade.spawn_worker(operation.id.clone());
    let first = wait_for_worker(&operations, owner, &operation.id, 1).await;
    assert_eq!(first.status, "waiting_for_connection", "{:?}", first.error);
    assert_eq!(first.sandbox_id, None);
    let agent = profiles
        .agent_profile_revision(&expected.agent.id, expected.agent.revision)
        .await
        .unwrap()
        .unwrap();
    let workspace = profiles
        .workspace_profile_revision(&expected.workspace.id, expected.workspace.revision)
        .await
        .unwrap()
        .unwrap();
    let sandbox = profiles
        .sandbox_profile_revision(&expected.sandbox.id, expected.sandbox.revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        sandbox.content.runtime_image.as_deref(),
        Some(image.uri.as_str())
    );
    assert_eq!(sandbox.content.workspace_profiles, [expected.workspace]);
    facade
        .drain_handle()
        .drain(Duration::from_secs(5))
        .await
        .unwrap();

    let restarted = Arc::new(MongoAgentLifecycleStore::open(&database).await.unwrap());
    let restarted_profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, database.name())
            .await
            .unwrap(),
    );
    let facade = fixture::facade(store, restarted_profiles.clone(), restarted.clone());
    restarted
        .resume_connection(owner, &operation.id)
        .await
        .unwrap();
    facade.spawn_worker(operation.id.clone());
    let replay = wait_for_worker(&restarted, owner, &operation.id, 2).await;
    assert_eq!(
        replay.status, "waiting_for_connection",
        "{:?}",
        replay.error
    );
    assert_eq!(
        replay.connection.unwrap().target,
        first.connection.unwrap().target
    );
    assert_eq!(replay.input.resolved_image, first.input.resolved_image);
    assert_eq!(
        restarted_profiles
            .agent_profile_revision(&agent.profile_id, agent.revision)
            .await
            .unwrap(),
        Some(agent)
    );
    assert_eq!(
        restarted_profiles
            .workspace_profile_revision(&workspace.profile_id, workspace.revision)
            .await
            .unwrap(),
        Some(workspace)
    );
    assert_eq!(
        restarted_profiles
            .sandbox_profile_revision(&sandbox.profile_id, sandbox.revision)
            .await
            .unwrap(),
        Some(sandbox)
    );
    for name in ["agent_profiles", "workspace_profiles", "sandbox_profiles"] {
        assert_eq!(
            database
                .collection::<Document>(name)
                .count_documents(doc! {"owner": owner.as_str()})
                .await
                .unwrap(),
            1
        );
    }

    // A changed release descriptor alone invalidates authority on restart.
    database
        .collection::<Document>("agent_lifecycle_operations")
        .update_one(
            doc! {"_id": &operation.id},
            doc! {"$set": {"canary_admission.release.config_digest": format!("sha256:{}", "2".repeat(64))}},
        )
        .await
        .unwrap();
    restarted
        .resume_connection(owner, &operation.id)
        .await
        .unwrap();
    facade.spawn_worker(operation.id.clone());
    let refused = wait_for_worker(&restarted, owner, &operation.id, 3).await;
    assert_eq!(refused.status, "failed");
    assert!(refused.error.unwrap().contains("committed authority"));
    facade
        .drain_handle()
        .drain(Duration::from_secs(5))
        .await
        .unwrap();
    database.drop().await.unwrap();
}

async fn wait_for_worker(
    operations: &MongoAgentLifecycleStore,
    owner: &SubjectId,
    id: &str,
    attempts: u32,
) -> crate::store::LifecycleRecord {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let record = operations.read_for_owner(owner, id).await.unwrap().unwrap();
            if record.attempts == attempts
                && matches!(record.status.as_str(), "waiting_for_connection" | "failed")
            {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("worker left no durable outcome")
}
