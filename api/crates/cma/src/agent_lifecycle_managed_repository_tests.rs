use super::*;
use layer2_core::profiles::SandboxProfileStore;
use mongodb::bson::{doc, Document};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn managed_repository_waits_before_allocation_for_both_entry_points() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!("managed_repo_{}", uuid::Uuid::new_v4().simple()));
    let operations = Arc::new(MongoAgentLifecycleStore::open(&database).await.unwrap());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, database.name())
            .await
            .unwrap(),
    );
    let store = Arc::new(layer2_core::sandbox::store::InMemorySandboxStore::default());
    let mut owner = fixture::context("owner");
    owner.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
    let facade = fixture::facade_with_limit(
        store.clone(),
        profiles.clone(),
        operations.clone(),
        4,
        Connections(Arc::new(AtomicBool::new(true))),
    )
    .with_task_experience(fixture::task_policy())
    .with_managed_repository_preflight(true);
    let flag_off = fixture::facade_with_limit(
        store,
        profiles.clone(),
        operations.clone(),
        4,
        layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready,
    )
    .with_task_experience(fixture::task_policy())
    .with_managed_repository_preflight(false);

    let task = request(json!({
        "name": "Repository task", "initial_message": "Inspect the synthetic source.",
        "repository": {"url":"https://github.com/example/project.git.git", "ref":"proof/fixed"},
    }));
    let create = request(json!({
        "name": "Repository setup",
        "repository": {"url":"https://github.com/example/project"},
    }));
    for (key, input) in [("task", task.clone()), ("create", create.clone())] {
        let receipt = facade
            .create(&owner.caller, Some(&owner), None, input.clone(), key)
            .await
            .unwrap();
        let waiting = read_operation_when(&operations, &receipt.id, |row| {
            row.status == "waiting_for_connection"
        })
        .await;
        assert!(waiting.sandbox_id.is_none());
        assert!(waiting.agent_id.is_none());
        assert!(!waiting
            .events
            .iter()
            .any(|event| event.phase == "sandbox_ready"));
        let connection = waiting.connection.unwrap();
        assert_eq!(connection.services, [crate::trigger::GITHUB_SERVICE]);
        assert_eq!(
            connection.repository_access,
            Some(layer1_public::rest::RepositoryAccessRefusal::VerificationUnavailable)
        );
        let profile: SandboxProfileRef =
            serde_json::from_value(connection.target["profile"].clone()).unwrap();
        let root = profiles
            .sandbox_profile(&profile.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(root.owner, owner.caller.subject);
        let revision = profiles
            .sandbox_profile_revision(&profile.id, profile.revision)
            .await
            .unwrap()
            .unwrap();
        assert!(revision
            .content
            .nyxid_services
            .contains(&ServiceId::new(crate::trigger::GITHUB_SERVICE)));

        let pinned_commit = "a".repeat(40);
        let updated = database
            .collection::<Document>("agent_lifecycle_operations")
            .update_one(
                doc! {"_id": &receipt.id},
                doc! {"$set": {"repository_commit": &pinned_commit}},
            )
            .await
            .unwrap();
        assert_eq!(updated.modified_count, 1);
        let replay = facade
            .create(&owner.caller, Some(&owner), None, input, key)
            .await
            .unwrap();
        assert_eq!(replay.id, receipt.id);
        let still_waiting = read_operation_when(&operations, &receipt.id, |row| {
            row.status == "waiting_for_connection" && row.attempts > waiting.attempts
        })
        .await;
        assert_eq!(
            still_waiting.repository_commit.as_deref(),
            Some(pinned_commit.as_str())
        );
        assert!(still_waiting.sandbox_id.is_none());
        assert!(still_waiting.agent_id.is_none());
        assert_eq!(still_waiting.connection.unwrap().target, connection.target);
        assert_eq!(
            facade.sandbox.capacity(&owner.caller).await.unwrap().live,
            0
        );
    }

    for (key, input) in [("legacy-task", task), ("legacy-create", create)] {
        let receipt = flag_off
            .create(&owner.caller, Some(&owner), None, input, key)
            .await
            .unwrap();
        let allocated =
            read_operation_when(&operations, &receipt.id, |row| row.sandbox_id.is_some()).await;
        assert!(allocated.connection.is_none());
    }
    assert_eq!(
        flag_off.sandbox.capacity(&owner.caller).await.unwrap().live,
        2
    );
    database.drop().await.unwrap();
}
