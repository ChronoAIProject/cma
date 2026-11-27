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

struct PublicProbe(AtomicUsize);

#[async_trait]
impl crate::agent_lifecycle::repository_preflight::PublicRepositoryReadiness for PublicProbe {
    async fn resolve(
        &self,
        request: &layer2_core::profiles::authoring::RepositoryRequest,
    ) -> Result<layer2_core::profiles::authoring::RepositoryResolution, AuthoringSourceError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        if matches!(
            request.url.as_str(),
            "https://github.com/example/unavailable" | "https://github.com/example/private-project"
        ) {
            return Err(AuthoringSourceError::RepositoryNotFound);
        }
        assert_eq!(request.url, "https://github.com/example/project");
        Ok(layer2_core::profiles::authoring::RepositoryResolution {
            url: request.url.clone(),
            reference: request.reference.clone(),
            default_branch: "main".into(),
            commit_sha: "a".repeat(40),
        })
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn public_repository_requires_qualified_image_and_pins_both_entry_points_before_allocation() {
    use layer2_core::profiles::{WorkspaceLocation, WorkspaceProfileStore};
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!("public_repo_{}", uuid::Uuid::new_v4().simple()));
    let operations = Arc::new(MongoAgentLifecycleStore::open(&database).await.unwrap());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, database.name())
            .await
            .unwrap(),
    );
    let store = Arc::new(InMemorySandboxStore::default());
    let image = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let sibling = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    let probe = Arc::new(PublicProbe(AtomicUsize::new(0)));
    let service = fixture::facade_with_limit(
        store.clone(),
        profiles.clone(),
        operations.clone(),
        6,
        Connections(Arc::new(AtomicBool::new(false))),
    )
    .with_task_experience(Arc::new(serde_json::from_value(json!({
        "mode":"accounts", "accounts":["owner"], "capabilities":["task_home_admission", "desktop_enable"]
    })).unwrap()))
    .with_released_runtime(Arc::new(layer3_adapter::PinnedReleasedRuntime::new(
        image.clone(),
        Some(sibling),
    )))
    .with_managed_repository_preflight(true)
    .with_public_repository_images([image].into_iter().collect(), true)
    .with_public_repository_probe(probe.clone());
    let mut owner = fixture::context("owner");
    owner.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
    for (key, repo, machine) in [
        ("unqualified", "project", true),
        ("unavailable", "unavailable", false),
    ] {
        let receipt = service.create(&owner.caller, Some(&owner), None, request(json!({
            "name":"Public source", "initial_message":"Read the source", "machine_included":machine,
            "repository":{"url":format!("https://github.com/example/{repo}")}
        })), key).await.unwrap();
        let blocked = read_operation_when(&operations, &receipt.id, |row| {
            row.status == "waiting_for_connection"
        })
        .await;
        assert!(blocked.public_repository.is_none() && blocked.sandbox_id.is_none());
        assert_eq!(
            service.sandbox.capacity(&owner.caller).await.unwrap().live,
            0
        );
    }
    assert_eq!(probe.0.load(Ordering::SeqCst), 1);
    for index in 0..3 {
        let mut input = request(
            json!({"name":"Public source", "repository":{"url":"https://github.com/example/project"}}),
        );
        if index != 1 {
            input.initial_message = Some("Read the source".into());
        }
        let receipt = service
            .create(
                &owner.caller,
                Some(&owner),
                None,
                input,
                &format!("public-{index}"),
            )
            .await
            .unwrap();
        let row =
            read_operation_when(&operations, &receipt.id, |row| row.sandbox_id.is_some()).await;
        let pin = row.public_repository.unwrap();
        assert_eq!(pin.commit, "a".repeat(40));
        let sandbox = service
            .sandbox
            .read(&owner.caller, &SandboxId(row.sandbox_id.unwrap()))
            .await
            .unwrap();
        assert!(!sandbox
            .profile
            .content
            .nyxid_services
            .contains(&ServiceId::new("api-github")));
        let workspace = &sandbox.profile.content.workspace_profiles[0];
        let revision = profiles
            .workspace_profile_revision(&workspace.id, workspace.revision)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            revision.content.location,
            WorkspaceLocation::PublicRepositorySnapshot {
                url: pin.url,
                commit: pin.commit,
                operation_id: receipt.id.clone(),
            }
        );
    }
    assert_eq!(probe.0.load(Ordering::SeqCst), 4);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn public_repository_lost_before_allocation_preserves_pin_and_waits() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!("public_recheck_{}", uuid::Uuid::new_v4().simple()));
    let operations = Arc::new(MongoAgentLifecycleStore::open(&database).await.unwrap());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, database.name())
            .await
            .unwrap(),
    );
    let store = Arc::new(InMemorySandboxStore::default());
    let image = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let probe = Arc::new(PublicProbe(AtomicUsize::new(0)));
    let service = fixture::facade_with_limit(
        store,
        profiles,
        operations.clone(),
        4,
        Connections(Arc::new(AtomicBool::new(false))),
    )
    .with_task_experience(fixture::task_policy())
    .with_released_runtime(Arc::new(layer3_adapter::PinnedReleasedRuntime::new(
        image.clone(),
        None,
    )))
    .with_managed_repository_preflight(true)
    .with_public_repository_images([image].into_iter().collect(), true)
    .with_public_repository_probe(probe.clone());
    let mut owner = fixture::context("owner");
    owner.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
    let slots = service
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    let input = request(
        json!({"name":"Public source", "initial_message":"Read local source", "repository":{"url":"https://github.com/example/unavailable"}}),
    );
    let receipt = service
        .create(
            &owner.caller,
            Some(&owner),
            None,
            input.clone(),
            "public-recheck",
        )
        .await
        .unwrap();
    let pinned = crate::store::PublicRepositoryPin {
        url: "https://github.com/example/unavailable".into(),
        commit: "a".repeat(40),
    };
    database
        .collection::<Document>("agent_lifecycle_operations")
        .update_one(
            doc! {"_id": &receipt.id},
            doc! {"$set":{"public_repository":{"url": &pinned.url, "commit": &pinned.commit}}},
        )
        .await
        .unwrap();
    drop(slots);
    service
        .create(&owner.caller, Some(&owner), None, input, "public-recheck")
        .await
        .unwrap();
    let row = read_operation_when(&operations, &receipt.id, |row| {
        row.status == "waiting_for_connection"
    })
    .await;
    assert_eq!(row.public_repository, Some(pinned));
    assert!(row.sandbox_id.is_none());
    assert_eq!(
        service.sandbox.capacity(&owner.caller).await.unwrap().live,
        0
    );
    assert_eq!(probe.0.load(Ordering::SeqCst), 1);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn retained_public_pin_requires_current_allocation_policy() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!("public_policy_{}", uuid::Uuid::new_v4().simple()));
    let operations = Arc::new(MongoAgentLifecycleStore::open(&database).await.unwrap());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, database.name())
            .await
            .unwrap(),
    );
    let store = Arc::new(InMemorySandboxStore::default());
    let image = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let probe = Arc::new(PublicProbe(AtomicUsize::new(0)));
    let facade = |qualified, managed, writer| {
        fixture::facade_with_limit(
            store.clone(),
            profiles.clone(),
            operations.clone(),
            4,
            Connections(Arc::new(AtomicBool::new(false))),
        )
        .with_task_experience(fixture::task_policy())
        .with_released_runtime(Arc::new(layer3_adapter::PinnedReleasedRuntime::new(
            image.clone(),
            None,
        )))
        .with_managed_repository_preflight(managed)
        .with_public_repository_images(
            if qualified {
                [image.clone()].into_iter().collect()
            } else {
                Default::default()
            },
            writer,
        )
        .with_public_repository_probe(probe.clone())
    };
    let admitted = facade(true, true, true);
    let _slots = admitted
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    let mut owner = fixture::context("owner");
    owner.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
    let receipt = admitted
        .create(
            &owner.caller,
            Some(&owner),
            None,
            request(json!({"name":"Public source", "repository":{"url":"https://github.com/example/project"}})),
            "public-policy",
        )
        .await
        .unwrap();
    let record = operations.claim(&receipt.id).await.unwrap().unwrap();
    let pending = || {
        let mut input = super::super::work(&record, false);
        input.runtime_image = record.input.resolved_image.clone();
        input
    };
    let mut original = pending();
    admitted
        .preflight_repository(&owner.caller, &mut original, None, None)
        .await
        .unwrap();
    let pin = read_operation_when(&operations, &receipt.id, |row| {
        row.public_repository.is_some()
    })
    .await
    .public_repository
    .unwrap();
    assert_eq!(probe.0.load(Ordering::SeqCst), 1);

    for (qualified, managed, writer) in [
        (false, true, true),
        (true, false, true),
        (true, true, false),
    ] {
        let restarted = facade(qualified, managed, writer);
        let mut replay = pending();
        assert!(matches!(
            restarted
                .preflight_repository(&owner.caller, &mut replay, None, Some(&pin))
                .await,
            Err(AgentLifecycleProvisioningError::RepositoryAccessRequired { .. })
        ));
        assert!(replay.existing_sandbox_id.is_none());
        let capacity = restarted.sandbox.capacity(&owner.caller).await.unwrap();
        assert_eq!(capacity.live, 0);
    }
    assert_eq!(probe.0.load(Ordering::SeqCst), 1);

    let (_, _, profile) = profiles::create_profiles(
        admitted.profiles.as_ref(),
        &owner.caller,
        &original.request,
        original.runtime_image.clone(),
        &original.key,
        original.capacity,
        original.limits,
        original.behavior,
        original.sandbox_availability,
        &original.lease.id,
    )
    .await
    .unwrap();
    let sandbox = admission::create_sandbox_with_fence(
        admitted.sandbox.as_ref(),
        &owner.caller,
        profile,
        &original.request.name,
        &original.lease,
        None,
        None,
    )
    .await
    .unwrap();
    let restarted = facade(false, false, false);
    let mut replay = pending();
    restarted
        .preflight_repository(&owner.caller, &mut replay, None, Some(&pin))
        .await
        .unwrap();
    assert_eq!(
        replay.existing_sandbox_id.as_deref(),
        Some(sandbox.id.as_str())
    );
    let reconciled =
        read_operation_when(&operations, &receipt.id, |row| row.sandbox_id.is_some()).await;
    assert_eq!(reconciled.sandbox_id.as_deref(), Some(sandbox.id.as_str()));
    assert_eq!(reconciled.public_repository, Some(pin));
    let capacity = restarted.sandbox.capacity(&owner.caller).await.unwrap();
    assert_eq!(capacity.live, 1);
    assert_eq!(probe.0.load(Ordering::SeqCst), 1);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn url_only_private_repository_uses_default_connection_and_repairs_before_allocation() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "repository_choice_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let operations = Arc::new(MongoAgentLifecycleStore::open(&database).await.unwrap());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, database.name())
            .await
            .unwrap(),
    );
    let store = Arc::new(InMemorySandboxStore::default());
    let image = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let provider = crate::trigger::GithubRepositoryFixture::start().await;
    provider.set_default_private_read(false);
    let probe = Arc::new(PublicProbe(AtomicUsize::new(0)));
    let service = fixture::facade_with_limit(
        store,
        profiles.clone(),
        operations.clone(),
        4,
        layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready,
    )
    .with_task_experience(fixture::task_policy())
    .with_released_runtime(Arc::new(layer3_adapter::PinnedReleasedRuntime::new(
        image.clone(),
        None,
    )))
    .with_managed_repository_preflight(true)
    .with_public_repository_images([image.clone()].into_iter().collect(), true)
    .with_public_repository_probe(probe.clone());
    assert!(service
        .managed_repository_runtime_images
        .set((image.clone(), vec![image]))
        .is_ok());
    assert!(service
        .managed_repository_broker
        .set(provider.broker())
        .is_ok());
    let mut owner = fixture::context("owner");
    owner.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
    let input = request(json!({
        "name":"Private source", "initial_message":"Read the source",
        "repository":{"url":"https://github.com/example/private-project"}
    }));
    let first = service
        .create(
            &owner.caller,
            Some(&owner),
            None,
            input.clone(),
            "private-repair",
        )
        .await
        .unwrap();
    let held = read_operation_when(&operations, &first.id, |row| {
        row.status == "waiting_for_connection"
    })
    .await;
    assert_eq!(held.input.behavior, LifecycleBehavior::RepositoryChoice);
    assert!(
        held.sandbox_id.is_none()
            && held.public_repository.is_none()
            && held.repository_access_pin.is_none()
    );
    let connection = held.connection.unwrap();
    assert_eq!(
        connection.repository_access,
        Some(layer1_public::rest::RepositoryAccessRefusal::InsufficientScope)
    );
    let repair = connection.repository_scope_repair.unwrap();
    assert_eq!(repair.connection_id, "selected");
    assert_eq!(repair.required_scope, "repo");
    assert_eq!(
        service.sandbox.capacity(&owner.caller).await.unwrap().live,
        0
    );

    provider.set_default_private_read(true);
    let replay = service
        .create(
            &owner.caller,
            Some(&owner),
            None,
            input.clone(),
            "private-repair",
        )
        .await
        .unwrap();
    assert_eq!(replay.id, first.id);
    let direct = service
        .create(&owner.caller, Some(&owner), None, input, "private-ready")
        .await
        .unwrap();
    for receipt in [replay, direct] {
        let allocated =
            read_operation_when(&operations, &receipt.id, |row| row.sandbox_id.is_some()).await;
        assert!(allocated.public_repository.is_none());
        let pin = allocated.repository_access_pin.unwrap();
        assert_eq!(pin.connection.selection.connection_id, "selected");
        assert_eq!(
            pin.connection.requirement,
            layer2_core::sandbox::RepositoryCredentialRequirement::PrivateRepositoryRead
        );
        assert_eq!(pin.commit, "a".repeat(40));
        let sandbox = service
            .sandbox
            .read(&owner.caller, &SandboxId(allocated.sandbox_id.unwrap()))
            .await
            .unwrap();
        let reference = &sandbox.profile.content.workspace_profiles[0];
        let revision = layer2_core::profiles::WorkspaceProfileStore::workspace_profile_revision(
            &*profiles,
            &reference.id,
            reference.revision,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            revision.content.location,
            layer2_core::profiles::WorkspaceLocation::RepositorySnapshot {
                url: "https://github.com/example/private-project".into(),
                reference: Some("a".repeat(40)),
                connection_id: Some("selected".into()),
            }
        );
    }
    assert_eq!(
        service.sandbox.capacity(&owner.caller).await.unwrap().live,
        2
    );
    assert_eq!(probe.0.load(Ordering::SeqCst), 3);
    database.drop().await.unwrap();
}
