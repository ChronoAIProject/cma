use super::binding_router;
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use layer2_core::profiles::store::InMemoryProfileStore;
use layer2_core::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
use layer2_core::sandbox::binding::*;
use layer2_core::sandbox::*;
use layer2_core::{Authorization, Capability, CoreError, SandboxId, ServiceId, SubjectId};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

struct Registry;
#[async_trait::async_trait]
impl AgentRuntimeRegistry for Registry {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        Err(AgentRuntimeError::NotFound)
    }
    async fn fence(&self, _: &layer2_core::AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}
struct Custody;
#[async_trait::async_trait]
impl BindingCredentialCustody for Custody {
    async fn retain(
        &self,
        _: &SandboxId,
        _: &SubjectId,
        _: &[ServiceId],
        _: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        unreachable!()
    }
    async fn retain_binding(
        &self,
        value: &ProviderBinding,
        key: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        value.require_credential_admission(key)
    }
}
fn app() -> (Router, Arc<InMemorySandboxStore>) {
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let resources = BindingResources {
        sandboxes: SandboxService::new(
            store.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::SetupRequired),
        ),
        workspaces: WorkspaceService::new(store.clone(), profiles.clone()),
        agents: AgentService::new(
            store.clone(),
            profiles.clone(),
            Arc::new(Registry),
            layer2_core::RuntimeCapabilities::new([layer2_core::RuntimeCapability::Conversation]),
        ),
    };
    (
        binding_router(BindingService::new(
            store.clone(),
            profiles,
            resources,
            Arc::new(Custody),
            BindingPolicy {
                provider: AgentProvider::default(),
                supported_model_api: "responses".into(),
                bounds: ProfileCapacityBounds {
                    min_cpu_millis: 250,
                    max_cpu_millis: 8000,
                    min_memory_mib: 512,
                    max_memory_mib: 16384,
                },
                sandbox: Arc::new(SandboxPolicy::default()),
            },
        )),
        store,
    )
}
fn request(
    method: &str,
    path: &str,
    body: Option<Value>,
    authority: Option<Authorization>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap();
    if let Some(authority) = authority {
        request.extensions_mut().insert(authority);
        request
            .extensions_mut()
            .insert(crate::AuthenticatedSubject(SubjectId::new("manager")));
    }
    request
}
fn authority() -> Authorization {
    Authorization::new([
        Capability::ManageGatewayRecords,
        Capability::ManageProviderBindings,
    ])
}
fn body() -> Value {
    let profile = json!({"schema_version":1,"metadata":{"name":"route-test","owner":{"kind":"human","subject":"owner"},"provider":"cma_codex"},
        "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,"api_type":"responses","model":"test","thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"},"context":{},"environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}});
    let digest = format!("sha256:{}", content_digest(&profile).unwrap());
    json!({"schema_version":1,"profile":profile,"profile_digest":digest,"agent_key":format!("nyx_{}",uuid::Uuid::new_v4().simple()),
        "tools":{"native_mcp_url":"https://gateway.example/mcp","nyxid_service_slug":"cmaeg"}})
}
async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn binding_routes_require_authentication_and_both_management_capabilities() {
    let (app, _) = app();
    let path = "/api/v1/cmaeg/bindings/binding-one";
    for (authority, status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(Authorization::none()), StatusCode::FORBIDDEN),
        (
            Some(Authorization::new([Capability::ManageGatewayRecords])),
            StatusCode::FORBIDDEN,
        ),
        (
            Some(Authorization::new([Capability::ManageProviderBindings])),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request("PUT", path, Some(body()), authority))
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[tokio::test]
async fn binding_routes_replay_binding_and_conversation_and_acknowledge_retirement() {
    let (app, store) = app();
    let path = "/api/v1/cmaeg/bindings/binding-one";
    let input = body();
    let response = app
        .clone()
        .oneshot(request("PUT", path, Some(input.clone()), Some(authority())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let first = json_body(response).await;
    assert_eq!(first["state"], "provisioning");
    assert_eq!(first["level"], "l1");
    assert!(!first
        .to_string()
        .contains(input["agent_key"].as_str().unwrap()));
    let replay = app
        .clone()
        .oneshot(request("PUT", path, Some(input), Some(authority())))
        .await
        .unwrap();
    assert_eq!(json_body(replay).await, first);
    let get = app
        .clone()
        .oneshot(request("GET", path, None, Some(authority())))
        .await
        .unwrap();
    assert_eq!(json_body(get).await, first);
    // The route's async worker boundary is covered separately by lifecycle tests.
    let mut binding = store.binding("binding-one").await.unwrap().unwrap();
    let revision = binding.revision;
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store.save_binding(Some(revision), binding).await.unwrap();
    let conversation = format!("{path}/conversations/conversation_one");
    let created = app
        .clone()
        .oneshot(request(
            "PUT",
            &conversation,
            Some(json!({})),
            Some(authority()),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let first = json_body(created).await;
    let replay = app
        .clone()
        .oneshot(request("PUT", &conversation, None, Some(authority())))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body(replay).await, first);
    for target in [&conversation, path] {
        let deleted = app
            .clone()
            .oneshot(request("DELETE", target, None, Some(authority())))
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    }
    let retired = app
        .oneshot(request("GET", path, None, Some(authority())))
        .await
        .unwrap();
    assert_eq!(json_body(retired).await["state"], "retired");
}
