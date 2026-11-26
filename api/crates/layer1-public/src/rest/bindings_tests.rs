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
mod inspection;
mod preparation;
pub(super) mod preparation_contract;
mod readiness;
pub(super) mod reset;
mod retry;
pub(super) use readiness::create_backing_resources;

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
struct Custody {
    store: Arc<InMemorySandboxStore>,
    keys: std::sync::Mutex<std::collections::HashMap<SandboxId, String>>,
}
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
        value.require_credential_admission(key)?;
        self.keys
            .lock()
            .unwrap()
            .insert(value.sandbox_id.clone(), key.sha256());
        Ok(())
    }
    async fn retry_recovered_binding(
        &self,
        binding: &ProviderBinding,
        key: &ProvidedCredential,
    ) -> Result<ProviderBinding, CoreError> {
        use layer2_core::sandbox::store::{SandboxStore, WorkspaceStore};
        if self.keys.lock().unwrap().get(&binding.sandbox_id) != Some(&key.sha256()) {
            return Err(CoreError::InvalidState("missing custody".into()));
        }
        let sandbox = self
            .store
            .get_sandbox(&binding.sandbox_id)
            .await
            .unwrap()
            .ok_or_else(|| CoreError::InvalidState("missing sandbox".into()))?;
        let workspace = self
            .store
            .get_workspace(&binding.workspace_id)
            .await
            .unwrap()
            .ok_or_else(|| CoreError::InvalidState("missing workspace".into()))?;
        let next = binding.retry_recovered_hierarchy(
            &sandbox,
            &workspace,
            key,
            layer2_core::now_rfc3339(),
        )?;
        self.store
            .save_binding(Some(binding.revision), next.clone())
            .await
            .map_err(|_| CoreError::InvalidState("binding changed".into()))?;
        Ok(next)
    }
}
pub(super) fn app() -> (Router, Arc<InMemorySandboxStore>) {
    app_with_tools(None)
}
fn app_with_tools(endpoint: Option<&str>) -> (Router, Arc<InMemorySandboxStore>) {
    app_with_tools_alias(endpoint, None).unwrap()
}
fn app_with_tools_alias(
    endpoint: Option<&str>,
    alias: Option<&str>,
) -> Result<(Router, Arc<InMemorySandboxStore>), String> {
    app_config(endpoint, alias, false)
}
fn app_config(
    endpoint: Option<&str>,
    alias: Option<&str>,
    preparation_enabled: bool,
) -> Result<(Router, Arc<InMemorySandboxStore>), String> {
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
    Ok((
        route_service(
            BindingService::new(
                store.clone(),
                resources,
                Arc::new(Custody {
                    store: store.clone(),
                    keys: Default::default(),
                }),
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
            ),
            endpoint,
            alias,
            preparation_enabled,
        )?,
        store,
    ))
}
fn route_service(
    service: Arc<BindingService>,
    endpoint: Option<&str>,
    alias: Option<&str>,
    preparation_enabled: bool,
) -> Result<Router, String> {
    if preparation_enabled {
        let endpoint = endpoint.unwrap_or("https://gateway.example/mcp");
        return super::binding_router_with_native_tools_alias_and_preparation(
            service,
            endpoint,
            alias.unwrap_or(endpoint),
            true,
        );
    }
    let reset_routes = super::binding_conversation_reset_router(service.clone(), true);
    let result = match (endpoint, alias) {
        (Some(endpoint), Some(alias)) => {
            super::binding_router_with_native_tools_aliases_and_preparation(
                service,
                endpoint,
                &[
                    alias.into(),
                    "https://new-bot.example/event-gateway/mcp".into(),
                ],
                false,
            )
        }
        (Some(endpoint), None) => super::binding_router_with_native_tools(service, endpoint),
        (None, _) => Ok(binding_router(service)),
    };
    result.map(|routes| routes.merge(reset_routes))
}
pub(super) fn request(
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
pub(super) fn authority() -> Authorization {
    Authorization::new([
        Capability::ManageGatewayRecords,
        Capability::ManageProviderBindings,
    ])
}
pub(super) fn body() -> Value {
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
async fn binding_route_refuses_missing_environment_and_allows_corrected_profile() {
    let (app, store) = app();
    let path = "/api/v1/cmaeg/bindings/binding-one";
    let valid = body();
    let mut invalid = valid.clone();
    invalid["profile"]["environment"] = Value::Null;
    invalid["profile_digest"] = json!(format!(
        "sha256:{}",
        content_digest(&invalid["profile"]).unwrap()
    ));
    let refused = app
        .clone()
        .oneshot(request("PUT", path, Some(invalid), Some(authority())))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_body(refused).await["code"], "invalid_binding");
    assert!(store.binding("binding-one").await.unwrap().is_none());
    let accepted = app
        .oneshot(request("PUT", path, Some(valid), Some(authority())))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(json_body(accepted).await["state"], "provisioning");
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
    assert_eq!(first["level"], "l2");
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
    readiness::create_backing_resources(&store, &binding).await;
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

#[tokio::test]
async fn native_tools_accept_only_the_configured_destination_and_retain_it_on_replay() {
    let (app, store) = app_with_tools(Some("https://gateway.example/mcp"));
    let path = "/api/v1/cmaeg/bindings/binding-tools";
    let input = body();
    for destination in [
        "https://other.example/mcp",
        "https://gateway.example/mcp?leak=true",
        "http://gateway.example/mcp",
    ] {
        let mut changed = input.clone();
        changed["tools"]["native_mcp_url"] = json!(destination);
        let response = app
            .clone()
            .oneshot(request("PUT", path, Some(changed), Some(authority())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(store.binding("binding-tools").await.unwrap().is_none());
    }
    let mut first = None;
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(request("PUT", path, Some(input.clone()), Some(authority())))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value = json_body(response).await;
        assert!(value["honoured"]
            .as_array()
            .unwrap()
            .contains(&json!("tools.native_mcp_url")));
        assert!(!value["refused"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["field"] == "tools.native_mcp_url"));
        let retained = store.binding("binding-tools").await.unwrap().unwrap();
        if let Some(revision) = first {
            assert_eq!(retained.revision, revision);
        } else {
            first = Some(retained.revision);
        }
    }
    let retained = store.binding("binding-tools").await.unwrap().unwrap();
    assert_eq!(
        retained.tool_endpoint.as_deref(),
        Some("https://gateway.example/mcp")
    );
}

#[tokio::test]
async fn native_tools_alias_keeps_binding_identity_across_both_advertisement_phases() {
    let legacy = "https://gateway.example/mcp";
    let canonical = "https://bot.example/event-gateway/mcp";
    for first_url in [
        legacy,
        canonical,
        "https://new-bot.example/event-gateway/mcp",
    ] {
        let (app, store) = app_with_tools_alias(Some(legacy), Some(canonical)).unwrap();
        let path = "/api/v1/cmaeg/bindings/domain-binding";
        let mut input = body();
        let mut retained = None;
        for advertised in [
            first_url,
            canonical,
            legacy,
            "https://new-bot.example/event-gateway/mcp",
        ] {
            input["tools"]["native_mcp_url"] = json!(advertised);
            let response = app
                .clone()
                .oneshot(request("PUT", path, Some(input.clone()), Some(authority())))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let value = json_body(response).await;
            if let Some(first) = &retained {
                assert_eq!(&value, first);
            } else {
                retained = Some(value);
            }
            let binding = store.binding("domain-binding").await.unwrap().unwrap();
            assert_eq!(binding.tool_endpoint.as_deref(), Some(legacy));
        }
        for foreign in [
            "https://foreign.example/event-gateway/mcp",
            "https://bot.example/mcp",
            "https://bot.example/event-gateway/mcp/",
            "https://bot.example/event-gateway/mcp?redirect=true",
            "https://bot.example/event-gateway/mcp#fragment",
            "https://bot.example/event-gateway/mcp-extra",
            "https://bot.example.evil.invalid/event-gateway/mcp",
        ] {
            input["tools"]["native_mcp_url"] = json!(foreign);
            let response = app
                .clone()
                .oneshot(request("PUT", path, Some(input.clone()), Some(authority())))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        let read = app
            .clone()
            .oneshot(request("GET", path, None, Some(authority())))
            .await
            .unwrap();
        assert_eq!(Some(json_body(read).await), retained);
        let deleted = app
            .oneshot(request("DELETE", path, None, Some(authority())))
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
        let binding = store.binding("domain-binding").await.unwrap().unwrap();
        assert_eq!(binding.tool_endpoint.as_deref(), Some(legacy));
        assert_eq!(binding.state, BindingState::Retired);
    }
}

#[test]
fn native_tools_alias_validates_full_configured_urls_and_same_endpoint_fallback() {
    let endpoint = "https://gateway.example/mcp";
    assert!(app_with_tools_alias(Some(endpoint), Some(endpoint)).is_ok());
    for invalid in [
        "https://bot.example/event-gateway/mcp?query=true",
        "https://bot.example/event-gateway/mcp#fragment",
        "https://user@bot.example/event-gateway/mcp",
        "https://bot.example:0/event-gateway/mcp",
        "https://bot.example/event-gateway/../mcp",
        "http://bot.example/event-gateway/mcp",
    ] {
        assert!(app_with_tools_alias(Some(endpoint), Some(invalid)).is_err());
        assert!(app_with_tools_alias(Some(invalid), Some(endpoint)).is_err());
    }
}

#[path = "bindings_tests/queue_policy.rs"]
mod queue_policy;
