use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use layer1_public::rest::creation_outcomes::Operation;
use layer2_core::sandbox::{
    store::{AgentStore, InMemorySandboxStore, SandboxStore, WorkspaceStore, WorktreeStore},
    AgentStatus, AgentWorkStep, FixedSandboxCredentialPrerequisite, SandboxOrigin, SandboxPolicy,
    SandboxStatus, WorkspaceStatus, WorktreeStatus,
};
use layer2_core::StoreError;
use mongodb::bson::{doc, Document};
use serde_json::Value;
use tokio::time::{sleep, timeout, Duration};
use tower::ServiceExt;

use super::super::fixture;
use crate::store::{MongoAgentLifecycleStore, MongoProfileStore};

const OPAQUE_KEY: &str = "cma1.retry-17";

async fn submit(app: &Router, key: &str, expected: StatusCode) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::post(Operation::AgentBootstrap.path())
                .header("content-type", "application/json")
                .header("Idempotency-Key", key)
                .body(Body::from(r#"{"name":"Upgrade Agent"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable CHRONO_TEST_MONGODB_URI"]
async fn predecessor_opaque_key_recovers_and_replays_with_lookup_off() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI");
    let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let db = client.database(&format!("cma_upgrade_{}", uuid::Uuid::new_v4().simple()));
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(MongoProfileStore::open(&client, db.name()).await.unwrap());

    // Default-off admission persists the predecessor's record shape. Stop
    // the original facade before any worker can claim the queued operation.
    let (admitting_app, slots) = fixture::creation_test_router(
        &db,
        Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap()),
    )
    .await;
    let receipt = submit(&admitting_app, OPAQUE_KEY, StatusCode::ACCEPTED).await;
    let id = receipt["id"].as_str().unwrap();
    drop(admitting_app);
    drop(slots);

    let rows = db.collection::<Document>("agent_lifecycle_operations");
    let queued = rows.find_one(doc! {"_id":id}).await.unwrap().unwrap();
    assert_eq!(queued.get_str("status").unwrap(), "queued");
    assert_eq!(queued.get_str("key").unwrap(), OPAQUE_KEY);
    assert!(!queued.contains_key("creation_wire"));
    assert!(!queued.contains_key("creation_fence"));

    let operations = Arc::new(MongoAgentLifecycleStore::open(&db).await.unwrap());
    let service = fixture::facade_with_limit(
        store.clone(),
        profiles.clone(),
        operations.clone(),
        SandboxPolicy::default().max_sandboxes_per_owner,
        FixedSandboxCredentialPrerequisite::Ready,
    );
    let app = fixture::creation_routes(&db, store.clone(), profiles, service.clone()).await;
    let owner = fixture::context("owner").caller.subject;

    // An older writer could also admit a key with the reserved v1 shape.
    // With that shape the restarted worker must require the creation proof.
    let covered_id = uuid::Uuid::new_v4().simple().to_string();
    let covered_key = format!(
        "cma1.{}.20000.{}.{}",
        "a".repeat(32),
        "b".repeat(32),
        "c".repeat(64)
    );
    let mut covered = queued;
    covered.insert("_id", covered_id.clone());
    covered.insert("key", covered_key.clone());
    rows.insert_one(covered).await.unwrap();
    submit(&app, &covered_key, StatusCode::BAD_REQUEST).await;
    assert!(matches!(
        service.run_worker(&covered_id).await,
        Err(StoreError::Malformed(message)) if message == "lifecycle creation fence is missing or unsupported"
    ));
    assert!(store
        .list_sandboxes(&owner, &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());

    // Drive the real operation worker and supply the runtime readiness that
    // native workers publish after reservation in a running deployment.
    let worker = service.clone();
    let id_for_worker = id.to_owned();
    let running = tokio::spawn(async move { worker.run_worker(&id_for_worker).await });
    let published = timeout(Duration::from_secs(10), async {
        loop {
            let row = operations
                .read_for_owner(&owner, id)
                .await
                .unwrap()
                .unwrap();
            if row.agent_id.is_some() {
                break row;
            }
            assert!(
                !running.is_finished(),
                "creation worker exited in {}",
                row.status
            );
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let agent_id = layer2_core::AgentId(published.agent_id.unwrap());
    let mut agent = store.get_agent(&agent_id).await.unwrap().unwrap();
    let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    assert_eq!(published.sandbox_id.as_deref(), Some(sandbox.id.as_str()));
    assert_eq!(
        sandbox.origin,
        SandboxOrigin::Lifecycle {
            operation_id: id.to_owned()
        }
    );
    let sandbox_revision = sandbox.revision;
    sandbox.status = SandboxStatus::Active;
    sandbox.revision += 1;
    store.save_sandbox(sandbox_revision, sandbox).await.unwrap();

    let now = layer2_core::now_rfc3339();
    let mut workspace = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let workspace_revision = workspace.revision;
    let commit = "a".repeat(40);
    workspace.record_resolved_commit(commit.clone()).unwrap();
    workspace
        .transition(WorkspaceStatus::Ready, now.clone(), None)
        .unwrap();
    store
        .save_workspace(workspace_revision, workspace)
        .await
        .unwrap();
    let mut worktree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let worktree_revision = worktree.revision;
    worktree.base_commit = commit;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    store
        .save_worktree(worktree_revision, worktree)
        .await
        .unwrap();
    let agent_revision = agent.revision;
    agent.work.step = AgentWorkStep::None;
    agent.conversation.conversation_id = Some("original-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    agent.transition(AgentStatus::Ready, now, None).unwrap();
    store.save_agent(agent_revision, agent).await.unwrap();

    timeout(Duration::from_secs(10), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let completed = operations
        .read_for_owner(&owner, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.status, "succeeded");
    assert_eq!(completed.agent_id.as_deref(), Some(agent_id.as_str()));
    assert_eq!(
        submit(&app, OPAQUE_KEY, StatusCode::ACCEPTED).await,
        receipt
    );
    service
        .drain_handle()
        .drain(Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        store
            .list_sandboxes(&owner, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        rows.count_documents(doc! {"owner":owner.as_str(),"key":OPAQUE_KEY})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}
