//! Exercise the public Agent handlers and real resource policy with a doubled
//! NyxID boundary. Ordinary keys never obtain another owner's Agent inventory.

use super::*;
use crate::auth::test_resources;
use crate::projections::{AgentProjectionServices, UnavailableSandboxResources};
use crate::sandbox::LiveAgentRegistry;
use axum::body::{to_bytes, Body};
use axum::http::Request;
use layer2_core::profiles::store::InMemoryProfileStore;
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::store::{AgentStore, SandboxStore, WorkspaceStore, WorktreeStore};
use layer2_core::sandbox::AgentStatus;
use layer2_core::{OperationClaim, OperationReservation, OperationStore, StoreError};
use tower::ServiceExt;

struct UnusedOperations;

#[async_trait::async_trait]
impl OperationStore for UnusedOperations {
    async fn reserve(&self, _: &OperationClaim) -> Result<OperationReservation, StoreError> {
        panic!("read-only auth acceptance must not admit operations")
    }

    async fn complete(&self, _: &OperationClaim, _: Vec<u8>) -> Result<(), StoreError> {
        panic!("read-only auth acceptance must not complete operations")
    }
}

async fn router(auth: Arc<AuthState>) -> Router {
    let resources = Arc::new(InMemorySandboxStore::default());
    for owner in ["owner", "another-owner"] {
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
        resources.create_agent(agent).await.unwrap();
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
    .router(Arc::new(UnusedOperations))
    .layer(axum::middleware::from_fn_with_state(
        auth,
        crate::auth::attach_subject,
    ))
}

async fn get(app: &Router, path: &str, key: &str) -> (StatusCode, Value) {
    let mut request = Request::get(path).body(Body::empty()).unwrap();
    *request.headers_mut() = headers(key);
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn ordinary_key_lists_only_its_owner_and_cannot_read_another_owner() {
    let fixture = Fixture::new().await;
    let app = router(fixture.auth().await).await;
    fixture.store.set_unavailable(true);
    for prefix in ["nyx_", "nyxid_ag_"] {
        let key = format!("{prefix}{}", uuid::Uuid::new_v4().simple());
        let (status, body) = get(&app, "/api/v1/agents", &key).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["id"], "agt_owner");
        assert_eq!(
            get(&app, "/api/v1/agents/agt_owner", &key).await.0,
            StatusCode::OK
        );
        assert_eq!(
            get(&app, "/api/v1/agents/agt_another-owner", &key).await.0,
            StatusCode::NOT_FOUND
        );
    }
    fixture.response.write().await["sub"] = json!("another-owner");
    let key = format!("nyx_{}", uuid::Uuid::new_v4().simple());
    let (status, body) = get(&app, "/api/v1/agents", &key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], "agt_another-owner");
}

#[tokio::test]
async fn service_scope_revocation_and_permissions_are_live_at_public_routes() {
    let fixture = Fixture::new().await;
    let app = router(fixture.auth().await).await;
    let key = format!("nyx_{}", uuid::Uuid::new_v4().simple());
    let original = fixture.response.read().await.clone();
    for (change, expected) in [
        (json!({}), StatusCode::OK),
        (json!({"permissions": []}), StatusCode::FORBIDDEN),
        (
            json!({"permissions": ["other-service:admin"]}),
            StatusCode::FORBIDDEN,
        ),
        (json!({"active": false}), StatusCode::UNAUTHORIZED),
        (json!({"aud": "another-service"}), StatusCode::UNAUTHORIZED),
        (json!({"scope": "read"}), StatusCode::UNAUTHORIZED),
        (
            json!({"resource": ["restricted-resource"]}),
            StatusCode::UNAUTHORIZED,
        ),
        (json!({"exp": 1}), StatusCode::UNAUTHORIZED),
        (json!({}), StatusCode::OK),
    ] {
        let mut evidence = original.clone();
        evidence
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        *fixture.response.write().await = evidence;
        let before = fixture.introspections.load(Ordering::SeqCst);
        assert_eq!(get(&app, "/api/v1/agents", &key).await.0, expected);
        assert_eq!(fixture.introspections.load(Ordering::SeqCst), before + 1);
    }
}

#[tokio::test]
async fn provider_failure_is_unavailable_and_unconfigured_keys_do_not_reach_nyxid() {
    let fixture = Fixture::new().await;
    let app = router(fixture.auth().await).await;
    let key = format!("nyx_{}", uuid::Uuid::new_v4().simple());
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::BAD_GATEWAY,
    ] {
        *fixture.status.write().await = status;
        assert_eq!(
            get(&app, "/api/v1/agents", &key).await.0,
            if status == StatusCode::BAD_GATEWAY {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::UNAUTHORIZED
            }
        );
    }
    *fixture.status.write().await = StatusCode::OK;
    fixture
        .response
        .write()
        .await
        .as_object_mut()
        .unwrap()
        .remove("permissions");
    assert_eq!(
        get(&app, "/api/v1/agents", &key).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );

    let unconfigured = fixture.auth().await.with_agent_key_service(None);
    let app = router(unconfigured).await;
    let before = fixture.introspections.load(Ordering::SeqCst);
    assert_eq!(
        get(&app, "/api/v1/agents", &key).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(fixture.introspections.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn session_readiness_tracks_verified_key_lifetime_permissions_and_revocation() {
    let fixture = Fixture::new().await;
    let auth = fixture.auth().await;
    let app = crate::auth::router(auth.clone()).layer(axum::middleware::from_fn_with_state(
        auth,
        crate::auth::attach_subject,
    ));
    let original = fixture.response.read().await.clone();
    let expiry = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    for prefix in ["nyx_", "nyxid_ag_"] {
        let key = format!("{prefix}{}", uuid::Uuid::new_v4().simple());
        for (change, status, ready) in [
            (json!({}), StatusCode::OK, json!(true)),
            (json!({"exp": expiry}), StatusCode::OK, json!(true)),
            (json!({"permissions": []}), StatusCode::OK, json!(false)),
            (
                json!({"active": false}),
                StatusCode::UNAUTHORIZED,
                Value::Null,
            ),
            (
                json!({"aud": "other-service"}),
                StatusCode::UNAUTHORIZED,
                Value::Null,
            ),
            (json!({"exp": 1}), StatusCode::UNAUTHORIZED, Value::Null),
            (json!({"exp": null}), StatusCode::UNAUTHORIZED, Value::Null),
        ] {
            let mut evidence = original.clone();
            evidence
                .as_object_mut()
                .unwrap()
                .extend(change.as_object().unwrap().clone());
            *fixture.response.write().await = evidence;
            let before = fixture.introspections.load(Ordering::SeqCst);
            let (observed, body) = get(&app, "/auth/session", &key).await;
            assert_eq!(observed, status);
            assert_eq!(body["api_commands"], ready);
            // 连接检查不额外请求身份提供方。
            assert_eq!(fixture.introspections.load(Ordering::SeqCst), before + 1);
        }
    }
}
