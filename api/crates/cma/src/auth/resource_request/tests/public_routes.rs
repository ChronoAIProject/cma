//! Real public handlers retain their owner/lineage/RBAC policy under a read proof.

use super::*;
use crate::auth::test_resources;
use crate::projections::{AgentProjectionServices, UnavailableSandboxResources};
use crate::sandbox::LiveAgentRegistry;
use layer2_core::profiles::store::InMemoryProfileStore;
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::store::{
    AgentLogStore, AgentStore, SandboxStore, WorkspaceStore, WorktreeStore,
};
use layer2_core::sandbox::{AgentLogKind, AgentStatus, InMemorySandboxStore};
use layer2_core::{OperationClaim, OperationReservation, OperationStore, StoreError};

struct NoOperations;

#[async_trait::async_trait]
impl OperationStore for NoOperations {
    async fn reserve(&self, _: &OperationClaim) -> Result<OperationReservation, StoreError> {
        panic!("request-bound reads cannot reserve durable work")
    }
    async fn complete(&self, _: &OperationClaim, _: Vec<u8>) -> Result<(), StoreError> {
        panic!("request-bound reads cannot complete durable work")
    }
}

async fn public_router(auth: Arc<AuthState>) -> Router {
    public_router_for(auth, "owner", "another-owner").await
}

pub(super) async fn public_router_for(
    auth: Arc<AuthState>,
    owner: &str,
    other_owner: &str,
) -> Router {
    let resources = Arc::new(InMemorySandboxStore::default());
    for owner in [owner, other_owner] {
        let sandbox = test_resources::sandbox(&format!("sbx_{owner}"), owner);
        let workspace =
            test_resources::workspace(&format!("wks_{owner}"), sandbox.id.as_str(), owner);
        let mut agent = test_resources::agent(
            &format!("agt_{owner}"),
            &format!("wt_{owner}"),
            workspace.id.as_str(),
            sandbox.id.as_str(),
            owner,
            AgentStatus::Ready,
        );
        agent.conversation.conversation_id = Some(format!("conversation-{owner}"));
        agent.conversation.runtime_generation = Some(1);
        let logs = [
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
            },
            AgentLogKind::ConversationOpened {
                conversation_id: format!("conversation-{owner}"),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: format!("Earlier question from {owner}"),
            },
            AgentLogKind::AgentMessage {
                content: "Earlier answer".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 7,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: format!("Latest question from {owner}"),
            },
            AgentLogKind::AgentMessage {
                content: "Latest answer".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 7 },
        ];
        agent.conversation.last_seq = logs.len() as u64;
        agent.validate_conversation().unwrap();
        let worktree = test_resources::worktree(
            agent.worktree_id.as_str(),
            workspace.id.as_str(),
            sandbox.id.as_str(),
            agent.id.as_str(),
            owner,
        );
        resources.create_sandbox(sandbox).await.unwrap();
        resources.create_workspace(workspace).await.unwrap();
        resources.create_worktree(worktree).await.unwrap();
        resources.create_agent(agent.clone()).await.unwrap();
        for kind in logs {
            resources
                .append_agent_log(&agent.id, agent.updated_at.clone(), kind)
                .await
                .unwrap();
        }
    }
    AgentProjectionServices::new(
        layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
        resources.clone(),
        Arc::new(InMemoryProfileStore::default()),
        Arc::new(LiveAgentRegistry::default()),
        Arc::new(UnavailableSandboxResources),
        layer2_core::RuntimeCapabilities::new([layer2_core::RuntimeCapability::Conversation]),
    )
    .with_resource_access(Arc::new(ResourceAccessService::new(
        resources,
        auth.clone(),
    )))
    .router(Arc::new(NoOperations))
    .layer(axum::middleware::from_fn_with_state(
        auth,
        crate::auth::attach_subject,
    ))
}

#[tokio::test]
async fn request_proof_reads_only_its_owner_and_live_permissions_remain_authoritative() {
    let f = Fixture::new().await;
    let app = public_router(f.auth(true).await).await;
    f.store.set_unavailable(true);
    let response = app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/agents"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], "agt_owner");
    for (path, expected) in [
        ("/api/v1/agents/agt_owner", StatusCode::OK),
        ("/api/v1/agents/agt_another-owner", StatusCode::NOT_FOUND),
    ] {
        f.bind(Method::GET, path, &HeaderMap::new()).await;
        assert_eq!(
            app.clone()
                .oneshot(request(Method::GET, path))
                .await
                .unwrap()
                .status(),
            expected
        );
    }
    f.bind(Method::GET, "/api/v1/agents", &HeaderMap::new())
        .await;
    f.evidence.write().await["permissions"] = json!([]);
    assert_eq!(
        app.clone()
            .oneshot(request(Method::GET, "/api/v1/agents"))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    f.evidence.write().await["active"] = json!(false);
    assert_eq!(
        app.clone()
            .oneshot(request(Method::GET, "/api/v1/agents"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    // Real mutation routes never reach their durable operation stores.
    for (method, path) in [
        (Method::POST, "/api/v1/agents"),
        (Method::DELETE, "/api/v1/agents/agt_owner"),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request(method, path))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test]
async fn request_proof_lists_agents_then_reads_the_latest_stored_owner_message() {
    let f = Fixture::new().await;
    let app = public_router(f.auth(true).await).await;
    f.store.set_unavailable(true);
    let response = app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/agents"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let list: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let id = list["items"][0]["id"].as_str().unwrap();
    let path = format!("/api/v1/agents/{id}/transcript");
    f.bind(Method::GET, &path, &HeaderMap::new()).await;
    let response = app
        .clone()
        .oneshot(request(Method::GET, &path))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let transcript: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(transcript["agent_id"], id);
    assert_eq!(transcript["through_seq"], 10);
    let turns = transcript["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    let latest = turns
        .iter()
        .flat_map(|turn| turn["messages"].as_array().unwrap())
        .filter(|message| message["role"] == "user")
        .max_by_key(|message| message["sequence"].as_u64().unwrap())
        .unwrap();
    assert_eq!(latest["sequence"], 8);
    assert_eq!(latest["content"], "Latest question from owner");
    assert!(!transcript.to_string().contains("another-owner"));

    let foreign = "/api/v1/agents/agt_another-owner/transcript";
    f.bind(Method::GET, foreign, &HeaderMap::new()).await;
    assert_eq!(
        app.clone()
            .oneshot(request(Method::GET, foreign))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    f.bind(Method::GET, &path, &HeaderMap::new()).await;
    f.evidence.write().await["permissions"] = json!([]);
    assert_eq!(
        app.oneshot(request(Method::GET, &path))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    // No runtime is registered and no exporter or durable custody is available.
    // Reading the latest message must depend only on stored, authorized history.
}

#[tokio::test]
async fn no_proof_preserves_existing_trusted_request_behavior() {
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "owner".into(),
        },
        None,
    );
    let app = public_router(auth).await;
    let response = app
        .oneshot(
            HttpRequest::get("/api/v1/agents")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
