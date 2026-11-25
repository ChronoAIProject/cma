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
        store.clone(),
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

    facade
        .managed_repository_runtime_images
        .set((
            format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
            vec![],
        ))
        .unwrap();
    let incompatible = facade
        .create(
            &owner.caller,
            Some(&owner),
            None,
            task.clone(),
            "incompatible-runtime",
        )
        .await
        .unwrap();
    let protected = read_operation_when(&operations, &incompatible.id, |row| {
        row.status == "waiting_for_connection"
    })
    .await;
    assert!(protected.sandbox_id.is_none());
    assert!(protected.agent_id.is_none());
    assert!(protected.repository_access_pin.is_none());
    assert_eq!(
        protected.connection.unwrap().repository_access,
        Some(layer1_public::rest::RepositoryAccessRefusal::RuntimeUnsupported),
    );
    assert_eq!(
        facade.sandbox.capacity(&owner.caller).await.unwrap().live,
        0
    );

    let exact = request(json!({
        "name": "Selected GitHub setup",
        "repository": {
            "url": "https://github.com/example/project",
            "ref": "branch/synthetic",
            "operation": "public_repository_write",
            "connection_id": "selected"
        }
    }));
    for (key, input) in [
        ("takeover-explicit", &exact),
        ("takeover-implicit", &create),
    ] {
        // Admit through the enabled facade and leave both operations queued.
        // The flag-off facade claims each operation before its first pin.
        let CreateResult::Created(id) = facade
            .admit_operation(
                &owner.caller,
                Some(&owner),
                None,
                input.clone(),
                (key, None),
                "",
            )
            .await
            .unwrap()
        else {
            panic!("expected a new lifecycle operation");
        };
        let admitted = read_operation_when(&operations, &id, |row| row.status == "queued").await;
        assert_eq!(
            admitted.input.behavior,
            LifecycleBehavior::ManagedRepository
        );
        assert!(admitted.repository_access_pin.is_none());
        flag_off.run_worker(&id).await.unwrap();
        let blocked = read_operation_when(&operations, &id, |row| {
            row.status == "waiting_for_connection"
        })
        .await;
        assert_eq!(blocked.attempts, 1);
        assert!(blocked.sandbox_id.is_none());
        assert!(blocked.agent_id.is_none());
        assert!(blocked.repository_access_pin.is_none());
        assert_eq!(
            blocked.connection.unwrap().repository_access,
            Some(layer1_public::rest::RepositoryAccessRefusal::VerificationUnavailable)
        );
    }
    assert_eq!(
        flag_off.sandbox.capacity(&owner.caller).await.unwrap().live,
        0
    );

    let provider = crate::trigger::GithubRepositoryFixture::start().await;
    let recovery = fixture::facade_with_limit(
        store,
        profiles.clone(),
        operations.clone(),
        4,
        layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready,
    )
    .with_task_experience(fixture::task_policy())
    .with_managed_repository_preflight(true);
    let image = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    assert!(recovery
        .managed_repository_runtime_images
        .set((image.clone(), vec![image]))
        .is_ok());
    assert!(recovery
        .managed_repository_broker
        .set(provider.broker())
        .is_ok());
    let original = operations
        .replay(&owner.caller.subject, "takeover-explicit", &exact)
        .await
        .unwrap()
        .unwrap();
    let replay = recovery
        .create(
            &owner.caller,
            Some(&owner),
            None,
            exact,
            "takeover-explicit",
        )
        .await
        .unwrap();
    assert_eq!(replay.id, original.id);
    let recovered =
        read_operation_when(&operations, &replay.id, |row| row.sandbox_id.is_some()).await;
    assert_eq!(recovered.attempts, 2);
    let pin = recovered
        .repository_access_pin
        .expect("verified before allocation");
    assert_eq!(pin.owner, owner.caller.subject.as_str());
    assert_eq!(pin.connection.selection.connection_id, "selected");
    assert_eq!(pin.commit, "a".repeat(40));

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
        3
    );
    database.drop().await.unwrap();
}
