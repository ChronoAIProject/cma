#[path = "agent_lifecycle_title_admission_tests.rs"]
mod admission;

use super::*;
use layer2_core::sandbox::store::{
    AgentLogStore, AgentStore, InMemorySandboxStore, WorkspaceStore,
};
use layer2_core::sandbox::{
    Agent, AgentLogKind, AgentTitleOutcome, AgentTitleService, AgentTitleSource, AgentTitleStore,
};

pub(super) async fn assert_persisted_title(
    client: &mongodb::Client,
    db: &mongodb::Database,
    mut agent: Agent,
    mode: Option<&str>,
) {
    // Start an independent log for this persistence probe, retaining the admitted title.
    agent.conversation.last_seq = 0;
    agent.status = layer2_core::sandbox::AgentStatus::Ready;
    let store = Arc::new(
        crate::store::MongoSandboxStore::open(client, db.name())
            .await
            .unwrap(),
    );
    store.create_agent(agent.clone()).await.unwrap();
    let initial = store.agent_title(&agent.id).await.unwrap();
    if mode == Some("automatic") {
        assert!(initial.is_none());
    } else {
        let title = initial.unwrap();
        assert_eq!(title.title.as_str(), "New task");
        assert_eq!(title.source, AgentTitleSource::User);
    }
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 1,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Compare tools".into(),
        },
        AgentLogKind::AgentMessage {
            content: "Here is the comparison".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
    ] {
        store
            .append_agent_log(&agent.id, layer2_core::now_rfc3339(), kind)
            .await
            .unwrap();
    }
    let candidates = store
        .agent_title_candidates(&Default::default())
        .await
        .unwrap();
    assert_eq!(
        candidates.items.contains(&agent.id),
        mode == Some("automatic")
    );
    let service = AgentTitleService::new(store.clone(), "lifecycle-title-test").unwrap();
    assert_eq!(
        service
            .generate_agent_title(&agent.id, None, None)
            .await
            .unwrap(),
        if mode == Some("automatic") {
            AgentTitleOutcome::Fallback
        } else {
            AgentTitleOutcome::Skipped
        },
    );
    let reopened = crate::store::MongoSandboxStore::open(client, db.name())
        .await
        .unwrap();
    let title = reopened.agent_title(&agent.id).await.unwrap().unwrap();
    assert_eq!(
        title.title.as_str(),
        if mode == Some("automatic") {
            "Compare tools"
        } else {
            "New task"
        }
    );
    assert_eq!(
        title.source,
        if mode == Some("automatic") {
            AgentTitleSource::Fallback
        } else {
            AgentTitleSource::User
        }
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn contextual_title_modes_preserve_exact_admission_and_persist_title_authority() {
    for context in ["workspace_id", "sandbox_id"] {
        for mode in [None, Some("named"), Some("automatic")] {
            let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
            assert!(
                uri.starts_with("mongodb://127.0.0.1:") || uri.starts_with("mongodb://localhost:")
            );
            let client = mongodb::Client::with_uri_str(uri).await.unwrap();
            let db = client.database(&format!(
                "cma_title_mode_test_{}",
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
            let service = fixture::facade(store.clone(), profiles, operations.clone());
            let _slots = service
                .worker_slots
                .clone()
                .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
                .await
                .unwrap();
            let owner = fixture::context("owner");
            let mut body = json!({"name":"New task", "initial_message":"Compare tools"});
            body[context] = json!(if context == "workspace_id" {
                workspace.id.as_str()
            } else {
                workspace.sandbox_id.as_str()
            });
            if let Some(mode) = mode {
                body["title_mode"] = json!(mode);
            }
            let original = request(body);
            assert!(matches!(
                service
                    .create(
                        &owner.caller,
                        Some(&owner),
                        None,
                        original.clone(),
                        "original-title"
                    )
                    .await,
                Err(AgentLifecycleProvisioningError::Forbidden)
            ));
            assert!(operations
                .replay(&owner.caller.subject, "original-title", &original)
                .await
                .unwrap()
                .is_none());
            let service = service.with_task_experience(fixture::task_policy());
            let receipt = service
                .create(
                    &owner.caller,
                    Some(&owner),
                    None,
                    original.clone(),
                    "original-title",
                )
                .await
                .unwrap();
            let record = operations.claim(&receipt.id).await.unwrap().unwrap();
            let reopened = MongoAgentLifecycleStore::open(&db).await.unwrap();
            let replayed = reopened
                .replay(&owner.caller.subject, "original-title", &original)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(replayed.id, record.id);
            assert_eq!(
                serde_json::to_value(&replayed.input.request).unwrap(),
                serde_json::to_value(&original).unwrap()
            );
            let mut changed = original.clone();
            changed.title_mode = if mode == Some("automatic") {
                None
            } else {
                Some(layer1_public::rest::AgentLifecycleTitleMode::Automatic)
            };
            assert!(reopened
                .replay(&owner.caller.subject, "original-title", &changed)
                .await
                .is_err());
            assert!(reopened
                .read_for_owner(&SubjectId::new("other-owner"), &record.id)
                .await
                .unwrap()
                .is_none());
            let input = ProvisionInput {
                creation_fence: None,
                behavior: record.input.behavior,
                first_message: validation::admitted_first_message(&record.input),
                request: record.input.request.clone(),
                runtime_image: None,
                capacity: Default::default(),
                limits: record.input.captured_limits(),
                sandbox_availability: record.input.sandbox_availability,
                key: record.key.clone(),
                lease: record.lease().unwrap(),
                existing_sandbox_id: None,
                existing_agent_id: None,
                actor: record.input.actor.clone(),
                canary_profiles: None,
                repository_access_pin: None,
            };
            let provision = service.provision(&owner.caller, input);
            tokio::pin!(provision);
            let created = tokio::select! {
                result = &mut provision => panic!("unexpected completion before readiness: {result:?}"),
                agent = async {
                    tokio::time::timeout(Duration::from_secs(5), async {
                        loop {
                            for mut child in store.list_workspaces(&workspace.sandbox_id, &Default::default()).await.unwrap().items {
                                if child.status == WorkspaceStatus::Preparing {
                                    let revision = child.revision;
                                    child.record_resolved_commit("a".repeat(40)).unwrap();
                                    child.transition(WorkspaceStatus::Ready, layer2_core::now_rfc3339(), None).unwrap();
                                    store.save_workspace(revision, child).await.unwrap();
                                }
                            }
                            if let Some(agent) = store.list_agents(&owner.caller.subject, &Default::default()).await.unwrap().items.into_iter().next() { break agent; }
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                    }).await.unwrap()
                } => agent,
            };
            assert_eq!(
                created.title.as_deref(),
                (mode != Some("automatic")).then_some("New task")
            );
            assert_eq!(
                created.conversation.first_message.as_ref().unwrap().text,
                "Compare tools"
            );
            assert_persisted_title(&client, &db, created, mode).await;
            db.drop().await.unwrap();
        }
    }
}
