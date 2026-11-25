use super::*;
use crate::agent_lifecycle::canary_sandbox_content;
use crate::store::{CreateResult, LifecycleBehavior, LifecycleInput, MongoAgentLifecycleStore};
use layer2_core::profiles::{private_sandbox_profile_id, WorkspaceProfileRef};
use layer2_core::sandbox::store::SandboxWorkerStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn canary_profile_requires_its_exact_owner_revision_and_lifecycle_origin() {
    let (client, database_name) = database("canary_admission").await;
    let db = client.database(&database_name);
    let operation_store = MongoAgentLifecycleStore::open(&db).await.unwrap();
    let admission_store = MongoSandboxStore::open(&client, &database_name)
        .await
        .unwrap();

    let mut sandbox = sandbox("sbx_canary", "unused");
    let capacity = layer3_adapter::SandboxCapacity {
        cpu_millis: sandbox.profile.content.cpu_millis,
        memory_mib: sandbox.profile.content.memory_mib,
    };
    let image = layer3_adapter::RuntimeImage {
        uri: format!("registry.example/runtime@sha256:{}", "4".repeat(64)),
        ..Default::default()
    };
    let input = LifecycleInput {
        behavior: LifecycleBehavior::Legacy,
        managed_agent_instructions: None,
        setup_message: None,
        request: serde_json::from_value(serde_json::json!({"name": "Canary"})).unwrap(),
        actor: None,
        resolved_image: None,
        capacity: Some(capacity),
        sandbox_availability: Default::default(),
        capabilities: vec![],
    };
    let CreateResult::Created(id) = operation_store
        .create(&sandbox.owner, "fixture", input.clone())
        .await
        .unwrap()
    else {
        panic!("expected new fixture");
    };
    let mut record = operation_store.claim(&id).await.unwrap().unwrap();
    let profiles = operation_store
        .install_canary_worker_fixture(&mut record, image)
        .await;
    sandbox.origin = SandboxOrigin::Lifecycle {
        operation_id: id.clone(),
    };
    sandbox.profile.id = profiles.sandbox.id;
    sandbox.profile.content = canary_sandbox_content(&record.input, profiles.workspace).unwrap();
    sandbox.limits = record.input.captured_limits();
    sandbox.credential.sign_in = SandboxSignInState::NotRequired;
    sandbox.profile.digest = content_digest(&sandbox.profile.content).unwrap();

    let admission_for = |sandbox: Sandbox| {
        let name = SandboxAdmissionName::Allocate(sandbox.name.clone());
        SandboxAdmission::new(sandbox, name, 4, NOW.into()).unwrap()
    };
    let stale = record.lease().unwrap();
    let mut manual = sandbox.clone();
    manual.origin = SandboxOrigin::Manual;
    let mut trigger = sandbox.clone();
    trigger.origin = SandboxOrigin::Trigger {
        launch_id: "reused-profile".into(),
    };
    let mut foreign = sandbox.clone();
    foreign.owner = SubjectId::new("foreign-owner");
    let mut revoked_revision = sandbox.clone();
    revoked_revision.profile.revision += 1;
    let mut changed_profile = sandbox.clone();
    changed_profile.profile.id = SandboxProfileId("wrong-profile".into());
    let mut changed_content = sandbox.clone();
    changed_content.profile.content.cpu_millis += 1;
    changed_content.profile.digest = content_digest(&changed_content.profile.content).unwrap();
    for altered in [
        manual,
        trigger,
        foreign,
        revoked_revision,
        changed_profile,
        changed_content,
    ] {
        assert!(matches!(
            admission_store
                .admit_sandbox(admission_for(altered).with_lifecycle_lease(&stale.token))
                .await,
            Err(SandboxStoreError::SandboxOriginConflict)
        ));
    }
    assert!(admission_store
        .get_sandbox(&sandbox.id)
        .await
        .unwrap()
        .is_none());
    for origin in [
        SandboxOrigin::Manual,
        SandboxOrigin::Trigger {
            launch_id: "aliased-profile".into(),
        },
        SandboxOrigin::Lifecycle {
            operation_id: "unrelated".into(),
        },
    ] {
        let mut copied = sandbox.clone();
        copied.origin = origin;
        copied.profile.id = SandboxProfileId("distinct-copied-profile".into());
        copied.profile.content.runtime_image = Some(format!(
            "alias.example/copy:published@sha256:{}",
            "4".repeat(64)
        ));
        copied.profile.digest = content_digest(&copied.profile.content).unwrap();
        assert!(matches!(
            admission_store
                .admit_sandbox(admission_for(copied).with_lifecycle_lease(&stale.token))
                .await,
            Err(SandboxStoreError::SandboxOriginConflict)
        ));
    }

    db.collection::<mongodb::bson::Document>("agent_lifecycle_operations")
        .update_one(
            doc! {"_id": &id},
            doc! {"$set": {
                "lease_expires_at": mongodb::bson::DateTime::from_millis(0),
            }},
        )
        .await
        .unwrap();
    let peer_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let peer_operations = MongoAgentLifecycleStore::open(&peer_client.database(&database_name))
        .await
        .unwrap();
    let current = peer_operations
        .claim(&id)
        .await
        .unwrap()
        .unwrap()
        .lease()
        .unwrap();
    assert_ne!(stale.token, current.token);
    assert!(matches!(
        admission_store
            .admit_sandbox(admission_for(sandbox.clone()).with_lifecycle_lease(&stale.token))
            .await,
        Err(SandboxStoreError::SandboxOriginConflict)
    ));
    assert_eq!(
        operation_store
            .read_for_owner(&sandbox.owner, &id)
            .await
            .unwrap()
            .unwrap()
            .sandbox_id,
        None
    );
    assert!(admission_store
        .get_sandbox(&sandbox.id)
        .await
        .unwrap()
        .is_none());
    let exact = admission_for(sandbox.clone()).with_lifecycle_lease(&current.token);
    let SandboxAdmissionResult::Created(first) =
        admission_store.admit_sandbox(exact.clone()).await.unwrap()
    else {
        panic!("expected fresh Sandbox");
    };
    let replay_store = MongoSandboxStore::open(&client, &database_name)
        .await
        .unwrap();
    let SandboxAdmissionResult::Existing(replayed) = replay_store
        .admit_sandbox(exact.clone().with_lifecycle_lease(&stale.token))
        .await
        .unwrap()
    else {
        panic!("expected exact replay");
    };
    assert_eq!(first, replayed);
    assert_eq!(
        replay_store
            .sandbox_events(&first.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    // The lifecycle worker has not acknowledged the Sandbox result. Retire
    // and purge the real parent before that same operation is retried.
    let bound = operation_store
        .read_for_owner(&first.owner, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bound.sandbox_id.as_deref(), Some(first.id.as_str()));
    let mut retired = first.clone();
    retired.revision += 1;
    retired.status = SandboxStatus::Retired;
    retired.retired_at = Some(LATER.into());
    retired.purge_after = Some(LATER.into());
    retired.updated_at = LATER.into();
    retired.work = SandboxWorkState::new(SandboxWorkStep::Purge, LATER.into());
    replay_store
        .save_sandbox(first.revision, retired.clone())
        .await
        .unwrap();
    replay_store
        .purge_sandbox(&first.id, retired.revision, LATER)
        .await
        .unwrap();
    assert!(replay_store.get_sandbox(&first.id).await.unwrap().is_none());

    peer_operations.renew(&current).await.unwrap();
    let mut retry = exact.sandbox;
    retry.id = SandboxId("sbx_retried_canary".into());
    assert!(matches!(
        replay_store
            .admit_sandbox(admission_for(retry.clone()).with_lifecycle_lease(&current.token))
            .await,
        Err(SandboxStoreError::SandboxOriginConflict)
    ));
    assert!(replay_store.get_sandbox(&retry.id).await.unwrap().is_none());
    let recovered = operation_store
        .read_for_owner(&first.owner, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.sandbox_id, bound.sandbox_id);
    let mut ordinary_input = input;
    ordinary_input.request.machine_included = true;
    ordinary_input.resolved_image = first.profile.content.runtime_image.clone();
    let CreateResult::Created(ordinary_id) = operation_store
        .create(&first.owner, "machine-included", ordinary_input)
        .await
        .unwrap()
    else {
        panic!("expected ordinary lifecycle operation");
    };
    let ordinary_record = operation_store.claim(&ordinary_id).await.unwrap().unwrap();
    let mut ordinary = sandbox;
    ordinary.id = SandboxId("sbx_ordinary".into());
    ordinary.origin = SandboxOrigin::Lifecycle {
        operation_id: ordinary_id,
    };
    ordinary.profile.id =
        private_sandbox_profile_id(&ordinary.owner, "agent-manager:machine-included:sandbox")
            .unwrap();
    ordinary.profile.content = canary_sandbox_content(
        &ordinary_record.input,
        WorkspaceProfileRef::new(
            layer2_core::WorkspaceProfileId("ordinary-workspace".into()),
            1,
        ),
    )
    .unwrap();
    ordinary.profile.digest = content_digest(&ordinary.profile.content).unwrap();
    assert!(matches!(
        replay_store
            .admit_sandbox(admission_for(ordinary))
            .await
            .unwrap(),
        SandboxAdmissionResult::Created(_)
    ));
    db.drop().await.unwrap();
}
