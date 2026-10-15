//! The actual facade persists admission, restarts safely and retains its parent.
use super::*;
use layer2_core::sandbox::store::{
    AgentProvenanceStore, AgentStore, InMemorySandboxStore, SandboxStore, WorkspaceStore,
    WorktreeStore,
};
use layer2_core::sandbox::AgentStatus;
use mongodb::bson::doc;

fn work(record: &crate::store::LifecycleRecord, existing: bool) -> ProvisionInput {
    ProvisionInput {
        behavior: record.input.behavior,
        first_message: validation::admitted_first_message(&record.input),
        request: record.input.request.clone(),
        runtime_image: None,
        capacity: layer3_adapter::SandboxCapacity::default(),
        limits: record.input.captured_limits(),
        key: record.key.clone(),
        lease: record.lease().unwrap(),
        existing_sandbox_id: existing.then(|| record.sandbox_id.clone()).flatten(),
        existing_agent_id: existing.then(|| record.agent_id.clone()).flatten(),
        actor: record.input.actor.clone(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn contextual_facade_restarts_one_agent_without_new_parents_and_rechecks_authority() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI is required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!(
        "cma_workspace_lifecycle_test_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let operations = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, db.name())
            .await
            .unwrap(),
    );
    let workspace = fixture::seed(&store).await;
    let original_parent = store
        .get_sandbox(&workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let service = fixture::facade(store.clone(), profiles.clone(), operations.clone());
    // Drive the leased continuation explicitly to control crash/restart boundaries.
    let _slots = service
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    let owner = fixture::context("owner");
    let request = request(
        json!({"name":"Second Agent","workspace_id":workspace.id.as_str(),"profile":{"model":"model","permissions":{"services":["github"]}}}),
    );
    for context in [fixture::context("foreign"), fixture::context("reader")] {
        assert!(matches!(
            service
                .create(
                    &context.caller,
                    Some(&context),
                    None,
                    request.clone(),
                    "forbidden"
                )
                .await,
            Err(AgentLifecycleProvisioningError::Forbidden)
        ));
    }
    assert_eq!(
        db.collection::<mongodb::bson::Document>("agent_lifecycle_operations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_profile_counts(&db, 0).await;
    let receipt = service
        .create(
            &owner.caller,
            Some(&owner),
            None,
            request.clone(),
            "original",
        )
        .await
        .unwrap();
    let claimed = operations.claim(&receipt.id).await.unwrap().unwrap();
    assert_eq!(
        claimed.input.request.workspace_id.as_deref(),
        Some(workspace.id.as_str())
    );
    assert_eq!(claimed.input.actor.as_ref(), Some(&owner.actor));

    // Runtime readiness is a separate worker boundary. The facade must remain
    // pending while the new Agent is Starting, then release the original Agent.
    let provision = service.provision(&owner.caller, work(&claimed, false));
    tokio::pin!(provision);
    let starting = tokio::select! {
        result = &mut provision => panic!("returned before Agent readiness: {result:?}"),
        agent = async {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(agent) = store.list_agents(&owner.caller.subject, &Default::default()).await.unwrap().items.into_iter().next() { break agent; }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }).await.unwrap()
        } => agent,
    };
    assert_eq!(starting.status, AgentStatus::Starting);
    assert_eq!(starting.workspace_id, workspace.id);
    assert_eq!(starting.sandbox_id, original_parent.id);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut provision)
            .await
            .is_err()
    );
    let progress = operations
        .read_for_owner(&owner.caller.subject, &receipt.id)
        .await
        .unwrap()
        .unwrap();
    for phase in ["sandbox_ready", "workspace_runtime", "key_scope_ready"] {
        assert!(
            progress
                .events
                .iter()
                .any(|event| event.phase == phase && event.status == "ready"),
            "{phase} must be visible before Agent startup finishes"
        );
    }
    assert_eq!(progress.phase, "agent_startup");
    assert!(progress
        .events
        .iter()
        .any(|event| event.phase == "agent_startup" && event.status == "running"));
    assert!(!progress
        .events
        .iter()
        .any(|event| event.phase == "agent_startup" && event.status == "ready"));
    let mut ready = starting.clone();
    ready.conversation.conversation_id = Some("original-conversation".into());
    ready.conversation.runtime_generation = Some(1);
    ready
        .transition(AgentStatus::Ready, layer2_core::now_rfc3339(), None)
        .unwrap();
    store.save_agent(starting.revision, ready).await.unwrap();
    let created_id = tokio::time::timeout(Duration::from_secs(5), &mut provision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(created_id, starting.id.as_str());
    let completed = operations
        .read_for_owner(&owner.caller.subject, &receipt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.events.len(), 8);
    assert_eq!(completed.events.last().unwrap().phase, "agent_startup");
    assert_eq!(completed.events.last().unwrap().status, "ready");
    let reopened_operations = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
    let reopened_profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, db.name())
            .await
            .unwrap(),
    );
    let restarted = fixture::facade(
        store.clone(),
        reopened_profiles,
        reopened_operations.clone(),
    );
    let _slots = restarted
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    // A crash before persisting the Agent id also recovers the original origin.
    assert_eq!(
        restarted
            .provision(&owner.caller, work(&claimed, false))
            .await
            .unwrap(),
        created_id
    );
    let recorded = reopened_operations
        .read_for_owner(&owner.caller.subject, &receipt.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recorded.agent_id.as_deref(), Some(created_id.as_str()));
    assert_eq!(
        restarted
            .provision(&owner.caller, work(&recorded, true))
            .await
            .unwrap(),
        created_id
    );
    assert_eq!(
        restarted
            .create(
                &owner.caller,
                Some(&owner),
                None,
                request.clone(),
                "original"
            )
            .await
            .unwrap()
            .id,
        receipt.id
    );
    reopened_operations
        .succeed(&recorded.lease().unwrap(), &created_id)
        .await
        .unwrap();
    let succeeded =
        AgentLifecycleOperationReader::read(restarted.as_ref(), &owner.caller, &receipt.id)
            .await
            .unwrap();
    assert_eq!(succeeded.status, "succeeded");
    assert_eq!(succeeded.agent_id.as_deref(), Some(created_id.as_str()));
    assert_eq!(
        succeeded.events.len(),
        completed.events.len(),
        "recovery must not duplicate progress or reset elapsed times"
    );
    for (original, replayed) in completed.events.iter().zip(&succeeded.events) {
        assert_eq!(original.occurred_at, replayed.occurred_at);
    }
    assert_eq!(
        store
            .list_sandboxes(&owner.caller.subject, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        store
            .list_workspaces(&original_parent.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        store
            .list_agents(&owner.caller.subject, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        store
            .list_worktrees(&workspace.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_profile_counts(&db, 1).await;
    assert_eq!(
        store
            .get_sandbox(&original_parent.id)
            .await
            .unwrap()
            .unwrap()
            .credential,
        original_parent.credential
    );
    let provenance = store
        .agent_provenance_snapshot(&starting.id)
        .await
        .unwrap()
        .creation_provenance(&starting.id)
        .unwrap();
    assert_eq!(provenance.created_by, Some(owner.actor.clone()));

    // Current ownership and readiness still apply on an already recorded retry,
    // and refusal must happen before another private profile is written.
    let mut foreign_workspace = store.get_workspace(&workspace.id).await.unwrap().unwrap();
    let revision = foreign_workspace.revision;
    foreign_workspace.owner = SubjectId::new("foreign");
    foreign_workspace.revision += 1;
    store
        .save_workspace(revision, foreign_workspace.clone())
        .await
        .unwrap();
    assert!(matches!(
        restarted
            .provision(&owner.caller, work(&recorded, true))
            .await,
        Err(AgentLifecycleProvisioningError::Forbidden)
    ));
    let revision = foreign_workspace.revision;
    foreign_workspace.owner = owner.caller.subject.clone();
    foreign_workspace.status = WorkspaceStatus::Preparing;
    foreign_workspace.revision += 1;
    store
        .save_workspace(revision, foreign_workspace)
        .await
        .unwrap();
    assert!(matches!(
        restarted
            .create(
                &owner.caller,
                Some(&owner),
                None,
                request,
                "new-after-change"
            )
            .await,
        Err(AgentLifecycleProvisioningError::Conflict(_))
    ));
    assert!(matches!(
        restarted
            .provision(&owner.caller, work(&recorded, false))
            .await,
        Err(AgentLifecycleProvisioningError::Conflict(_))
    ));
    assert_eq!(
        db.collection::<mongodb::bson::Document>("agent_lifecycle_operations")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_profile_counts(&db, 1).await;
    db.drop().await.unwrap();
}

async fn assert_profile_counts(db: &mongodb::Database, expected_agents: u64) {
    for (collection, expected) in [
        ("agent_profiles", expected_agents),
        ("agent_profile_revisions", expected_agents),
        ("workspace_profiles", 0),
        ("workspace_profile_revisions", 0),
        ("sandbox_profiles", 0),
        ("sandbox_profile_revisions", 0),
    ] {
        assert_eq!(
            db.collection::<mongodb::bson::Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            expected,
            "unexpected profile mutation in {collection}"
        );
    }
}
