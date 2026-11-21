use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use layer2_core::{
    profiles::{store::InMemoryProfileStore, ProfileCapacityBounds},
    sandbox::{
        binding::{BindingPolicy, BindingResources, BindingService},
        *,
    },
    Authorization, Capability, SubjectId,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

struct NoRuntime;
#[async_trait::async_trait]
impl AgentRuntimeRegistry for NoRuntime {
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
fn request(path: &str, body: Value, key: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.extensions_mut().insert(Authorization::new([
        Capability::ManageGatewayRecords,
        Capability::ManageProviderBindings,
    ]));
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(SubjectId::new(
            "manager",
        )));
    if let Some(key) = key {
        request
            .headers_mut()
            .insert("idempotency-key", key.parse().unwrap());
    }
    request
}
async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 32768).await.unwrap()).unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_preparation_authenticated_http_replays_one_reservation_with_original_data_untouched()
{
    let (client, database, store, _, custody, data) = fixture().await;
    let profiles = Arc::new(InMemoryProfileStore::default());
    let prerequisite = Arc::new(FixedSandboxCredentialPrerequisite::SetupRequired);
    let service = BindingService::new(
        store.clone(),
        BindingResources {
            sandboxes: SandboxService::new(
                store.clone(),
                profiles.clone(),
                SandboxPolicy::default(),
                prerequisite,
            ),
            workspaces: WorkspaceService::new(store.clone(), profiles.clone()),
            agents: AgentService::new(
                store.clone(),
                profiles,
                Arc::new(NoRuntime),
                layer2_core::RuntimeCapabilities::new([
                    layer2_core::RuntimeCapability::Conversation,
                ]),
            ),
        },
        custody,
        BindingPolicy {
            provider: Default::default(),
            supported_model_api: "supported".into(),
            bounds: ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 8000,
                min_memory_mib: 512,
                max_memory_mib: 16384,
            },
            sandbox: Arc::new(SandboxPolicy::default()),
        },
    );
    let disabled = layer1_public::rest::binding_router(service.clone());
    let enabled: Router =
        layer1_public::rest::binding_router_with_native_tools_alias_and_preparation(
            service,
            "https://gateway.example/mcp",
            "https://gateway.example/mcp",
            true,
        )
        .unwrap();
    let inspect = json!({"schema_version":1,"admission_id":data.intent.admission_id,"target_id":data.intent.target_id,"request_hash":data.intent.request_hash,
        "owner":data.intent.owner,"admitted_at":data.intent.admitted_at,"expires_at":data.intent.expires_at,"agent_key":data.key.expose()});
    let response = enabled
        .clone()
        .oneshot(request(
            "/api/v1/cmaeg/bindings/binding-one/preparation-inspections",
            inspect,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let inspected = body(response).await;
    assert_eq!(inspected["state"], "pending");
    let intent: BindingPreparationIntent =
        serde_json::from_value(inspected["intent"].clone()).unwrap();
    assert_eq!(intent, data.intent);
    let wire: cmaeg::turns::routed::PreparationIntent =
        serde_json::from_value(inspected["intent"].clone()).unwrap();
    assert_eq!(wire.operation_id().unwrap(), intent.operation_id().unwrap());
    assert_eq!(wire.fingerprint().unwrap(), intent.fingerprint().unwrap());
    assert_eq!(
        store
            .bindings
            .preparations
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let path = "/api/v1/cmaeg/bindings/binding-one/preparations";
    let operation = intent.operation_id().unwrap();
    let preparation = json!({"schema_version":1,"intent":intent,"agent_key":data.key.expose()});
    assert_eq!(
        disabled
            .oneshot(request(path, preparation.clone(), Some(&operation)))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let response = enabled
        .clone()
        .oneshot(request(path, preparation.clone(), Some(&operation)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let first = body(response).await;
    assert_eq!(first["generation"], 4);
    assert_eq!(first["operation_id"], operation);
    let replay = enabled
        .clone()
        .oneshot(request(path, preparation.clone(), Some(&operation)))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(body(replay).await, first);
    let mut wrong = preparation;
    wrong["agent_key"] = format!("nyx_{}", uuid::Uuid::new_v4().simple()).into();
    assert_eq!(
        enabled
            .oneshot(request(path, wrong, Some(&operation)))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store
            .bindings
            .preparations
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store.get_sandbox(&data.sandbox.id).await.unwrap(),
        Some(data.sandbox)
    );
    assert_eq!(
        store.get_workspace(&data.workspace.id).await.unwrap(),
        Some(data.workspace)
    );
    client.database(&database).drop().await.unwrap();
}
