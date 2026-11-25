//! The actual facade persists admission, restarts safely and retains its parent.
use super::*;
#[path = "agent_lifecycle_canary_worker_tests.rs"]
mod canary_worker;
#[path = "agent_lifecycle_image_retirement_tests.rs"]
mod image_retirement;
#[path = "agent_lifecycle_repository_preflight_tests.rs"]
mod repository_preflight;
#[path = "agent_lifecycle_template_tests.rs"]
mod templates;
use layer2_core::sandbox::store::{
    AgentProvenanceStore, AgentStore, InMemorySandboxStore, SandboxStore, WorkspaceStore,
    WorktreeStore,
};
use layer2_core::sandbox::AgentStatus;
use mongodb::bson::doc;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_account_authority_is_not_persisted_in_lifecycle_capabilities() {
    use axum::{body::Body, http::Request, routing::post, Extension, Json, Router};
    use layer1_public::AuthenticatedCaller;
    use tower::ServiceExt;

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI is required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!(
        "cma_lifecycle_authority_{}",
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
    let service = fixture::facade(store, profiles, operations.clone());
    let _slots = service
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    let auth = crate::auth::AuthState::in_memory(
        crate::auth::AuthMode::Trusted {
            subject: "owner".into(),
        },
        None,
    )
    .with_account_deletion_enabled(true);
    let app = Router::new()
        .route(
            "/api/v1/agent-lifecycle-fixture",
            post({
                let service = service.clone();
                move |AuthenticatedCaller(caller): AuthenticatedCaller,
                      Extension(context): Extension<CallerContext>| {
                    let service = service.clone();
                    let workspace = workspace.id.to_string();
                    async move {
                        let request =
                            request(json!({"name":"Captured authority", "workspace_id":workspace}));
                        let result = service
                            .create(&caller, Some(&context), None, request, "personal-lifecycle")
                            .await;
                        match result {
                            Ok(receipt) => (
                                axum::http::StatusCode::ACCEPTED,
                                Json(json!({"id":receipt.id})),
                            ),
                            Err(error) => (
                                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                                Json(json!({"error":format!("{error:?}")})),
                            ),
                        }
                    }
                }
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::attach_subject,
        ));
    let response = app
        .oneshot(
            Request::post("/api/v1/agent-lifecycle-fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 16384)
        .await
        .unwrap();
    assert_eq!(status, 202, "{}", String::from_utf8_lossy(&body));
    let receipt: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let record = operations
        .read_for_owner(&SubjectId::new("owner"), receipt["id"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    let captured = serde_json::to_value(record.input.capabilities).unwrap();
    assert!(!captured
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "delete_own_account"));
    assert!(captured
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "use_own_sandboxes"));
    db.drop().await.unwrap();
}

fn work(record: &crate::store::LifecycleRecord, existing: bool) -> ProvisionInput {
    ProvisionInput {
        creation_fence: None,
        behavior: record.input.behavior,
        first_message: validation::admitted_first_message(&record.input),
        request: record.input.request.clone(),
        runtime_image: None,
        capacity: layer3_adapter::SandboxCapacity::default(),
        limits: record.input.captured_limits(),
        sandbox_availability: record.input.sandbox_availability,
        key: record.key.clone(),
        lease: record.lease().unwrap(),
        existing_sandbox_id: existing.then(|| record.sandbox_id.clone()).flatten(),
        existing_agent_id: existing.then(|| record.agent_id.clone()).flatten(),
        actor: record.input.actor.clone(),
        canary_profiles: None,
        repository_access_pin: None,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admitted_session_guidance_survives_retry_after_selection_is_disabled() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI is required");
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!("cma_guidance_{}", uuid::Uuid::new_v4().simple()));
    let store = Arc::new(InMemorySandboxStore::default());
    let workspace = fixture::seed(&store).await;
    let owner = fixture::context("owner");
    let request = request(json!({
        "name": "Guidance",
        "workspace_id": workspace.id.as_str(),
        "profile": {"system_prompt": "  exact owner prompt\n"},
    }));
    let mut first_id = None;
    for enabled in [true, false] {
        let operations = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
        let profiles = Arc::new(
            crate::store::MongoProfileStore::open(&client, db.name())
                .await
                .unwrap(),
        );
        let service = fixture::facade(store.clone(), profiles, operations.clone())
            .with_session_guidance(false, enabled);
        let _slots = service
            .worker_slots
            .clone()
            .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
            .await
            .unwrap();
        let receipt = service
            .create(
                &owner.caller,
                Some(&owner),
                None,
                request.clone(),
                "guidance",
            )
            .await
            .unwrap();
        let id = first_id.get_or_insert(receipt.id.clone());
        assert_eq!(&receipt.id, id);
        let saved = operations
            .read_for_owner(&owner.caller.subject, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            saved.input.managed_agent_instructions.as_deref(),
            Some(guidance::admission_instructions(false, true))
        );
        assert_eq!(
            serde_json::to_vec(&saved.input.request).unwrap(),
            serde_json::to_vec(&request).unwrap()
        );
        if !enabled {
            let fresh = service
                .create(&owner.caller, Some(&owner), None, request.clone(), "fresh")
                .await
                .unwrap();
            assert_ne!(&fresh.id, id);
            let saved = operations
                .read_for_owner(&owner.caller.subject, &fresh.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                saved.input.managed_agent_instructions.as_deref(),
                Some(guidance::managed_v3())
            );
        }
    }
    db.drop().await.unwrap();
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
    assert_eq!(
        claimed.input.setup_message,
        Some(validation::setup_message(&request))
    );

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
    assert_eq!(
        starting
            .conversation
            .first_message
            .as_ref()
            .map(|message| &message.text),
        claimed.input.setup_message.as_ref()
    );
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

#[path = "agent_lifecycle_release_source_tests.rs"]
mod release_source_tests;
