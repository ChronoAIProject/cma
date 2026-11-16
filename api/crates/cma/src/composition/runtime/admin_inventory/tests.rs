use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use layer2_core::{
    profiles::store::InMemoryProfileStore,
    sandbox::{store::InMemorySandboxStore, StoredSandboxRetirement},
    ActivityQuery, ActivityRecord, ActivityStore, Authorization, Capability, Page,
    PlatformSettings, PlatformSettingsStore, StoredPlatformSettings, SubjectId,
};
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    RuntimeHandle, RuntimeLifecycle, RuntimeProvisionRequest, SandboxSpec,
};
use serde_json::{json, Value};
use tower::ServiceExt;

struct UnusedStore;

#[async_trait::async_trait]
impl ActivityStore for UnusedStore {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        panic!("inventory must not mutate activity")
    }
    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        panic!("inventory must use the provider, not activity")
    }
}

#[async_trait::async_trait]
impl PlatformSettingsStore for UnusedStore {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        panic!("inventory must not depend on platform settings")
    }
    async fn replace(
        &self,
        _: u64,
        _: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        panic!("inventory must not mutate platform settings")
    }
}

fn app(adapter: Arc<dyn SandboxAdapter>) -> Router {
    let activity = ActivityService::new(Arc::new(UnusedStore));
    let settings = Arc::new(PlatformSettingsService::new(
        Arc::new(UnusedStore),
        activity.clone(),
    ));
    let sandboxes = Arc::new(InMemorySandboxStore::default());
    let operators = Arc::new(SandboxOperatorService::new(
        sandboxes.clone(),
        Arc::new(InMemoryProfileStore::default()),
        Arc::new(StoredSandboxRetirement::new(sandboxes)),
        activity.clone(),
    ));
    // This is the factory called by ControlPlaneComposition::assemble. No
    // test-only runtime-inventory dependency is attached to the admin handler.
    layer1_public::rest::admin::router(state(settings, operators, activity, adapter))
}

async fn read(app: &Router, authorized: bool) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .uri("/api/v1/admin/runtime-inventory")
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(SubjectId::new(
            "inventory-reader",
        )));
    request.extensions_mut().insert(Authorization::new(
        authorized.then_some(Capability::ReadRuntimeInventory),
    ));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn production_factory_returns_actual_empty_and_populated_provider_inventory() {
    let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let adapter = Arc::new(adapter);
    let first = app(adapter.clone());
    let second = app(adapter.clone());
    for app in [&first, &second] {
        let (status, body) = read(app, true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["runtimes"], json!([]));
        assert!(layer2_core::seconds_from_rfc3339(body["observed_at"].as_str().unwrap()).is_some());
    }
    let handle = adapter
        .provision_runtime(&RuntimeProvisionRequest::new(
            7,
            SandboxSpec {
                sandbox_id: "sandbox-owned-by-other-account".into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    for app in [&first, &second] {
        let (status, body) = read(app, true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["runtimes"],
            json!([{
                "runtime_id": handle.as_str(), "generation": 7,
                "owner_reference": "sandbox-owned-by-other-account", "status": "ready",
            }])
        );
    }
    adapter.teardown(&handle).await.unwrap();
    assert_eq!(read(&second, true).await.1["runtimes"], json!([]));
}

#[tokio::test]
async fn provider_failure_is_unavailable_and_ordinary_user_remains_forbidden() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let app = app(Arc::new(adapter));
    backend.set_inventory_unavailable(true);
    let (status, body) = read(&app, true).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "operator_service_unavailable");
    assert!(body.get("runtimes").is_none());
    assert_eq!(read(&app, false).await.0, StatusCode::FORBIDDEN);
    backend.set_inventory_unavailable(false);
    assert_eq!(read(&app, false).await.0, StatusCode::FORBIDDEN);
    assert_eq!(read(&app, true).await.0, StatusCode::OK);
}

#[test]
fn lifecycle_projection_never_promotes_suspended_or_unknown_runtime_to_ready() {
    for (state, expected) in [
        ("running", RuntimeStatus::Ready),
        ("RUNNING", RuntimeStatus::Ready),
        ("pending", RuntimeStatus::Starting),
        ("starting", RuntimeStatus::Starting),
        ("provisioning", RuntimeStatus::Starting),
        ("suspended", RuntimeStatus::Unavailable),
        ("terminating", RuntimeStatus::Unavailable),
        ("future-provider-state", RuntimeStatus::Unavailable),
    ] {
        let value = entry(RuntimeObservation {
            sandbox_id: "sandbox-a".into(),
            generation: 2,
            handle: RuntimeHandle::new("provider-runtime-a"),
            image_sha256: "image-digest".into(),
            spec_fingerprint: "spec-fingerprint".into(),
            runtime_contract: "runtime-contract".into(),
            lifecycle: RuntimeLifecycle {
                state: state.into(),
                reason: None,
            },
        });
        assert_eq!(value.status, expected);
        assert_eq!(value.runtime_id, "provider-runtime-a");
        assert_eq!(value.generation, 2);
        assert_eq!(value.owner_reference.as_deref(), Some("sandbox-a"));
    }
}
