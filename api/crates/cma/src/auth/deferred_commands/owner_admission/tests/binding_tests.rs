use super::*;
use layer2_core::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
use layer2_core::sandbox::{binding::*, InMemorySandboxStore};
use serde_json::json;

#[tokio::test]
async fn receiver_binding_auth_rechecks_original_route_generation_and_exact_credential() {
    let store = Arc::new(InMemorySandboxStore::default());
    let auth_store = Arc::new(MemoryAuthStore::default());
    auth_store.set_unavailable(true);
    let auth = AuthState::with_store(
        crate::auth::AuthMode::Trusted {
            subject: "unrelated-owner".into(),
        },
        None,
        auth_store,
    )
    .with_binding_store(store.clone());
    let key = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let profile = json!({"schema_version":1,"metadata":{"name":"owner-routing","owner":{"kind":"human","subject":"owner"},"provider":"registered"},
        "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,"api_type":"responses","model":"test","thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"},"context":{},"environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}});
    let digest = format!("sha256:{}", content_digest(&profile).unwrap());
    let mapped = map_profile(
        profile,
        &digest,
        AgentProvider::default(),
        "responses",
        ProfileCapacityBounds {
            min_cpu_millis: 250,
            max_cpu_millis: 8000,
            min_memory_mib: 512,
            max_memory_mib: 16384,
        },
    )
    .unwrap();
    let mut binding = ProviderBinding::new(
        "owner-admission-binding".into(),
        SubjectId::new("manager"),
        mapped,
        key.sha256(),
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    store.save_binding(None, binding.clone()).await.unwrap();
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store.save_binding(Some(1), binding.clone()).await.unwrap();
    let request = auth
        .binding_request(key.expose().into())
        .await
        .unwrap()
        .unwrap();
    let context = auth.resource_request_context(&request);
    let identity = auth
        .capture_owner_admission(&context, key.expose())
        .await
        .unwrap();
    let restored = auth
        .restore_owner_admission(&identity, key.expose())
        .await
        .unwrap();
    assert_eq!(restored, context);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", key.expose()).parse().unwrap(),
    );
    let permitted: Uri = "/api/v2/responses".parse().unwrap();
    auth.verify_owner_admission_origin(
        &restored,
        key.expose(),
        &Method::POST,
        &permitted,
        &headers,
    )
    .await
    .unwrap();
    for path in [
        "/api/v1/agents/foreign/preferences",
        "/internal/agent-manager/v1/control/foreign/admission/public",
        "/api/v1/cmaeg/bindings/foreign/conversations/t/events/e/media/0/supplement",
    ] {
        assert!(auth
            .verify_owner_admission_origin(
                &restored,
                key.expose(),
                &Method::POST,
                &path.parse().unwrap(),
                &headers
            )
            .await
            .is_err());
    }
    let replacement =
        ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let rotated = binding
        .rotate_credential(replacement.sha256(), layer2_core::now_rfc3339())
        .unwrap();
    store
        .save_binding(Some(binding.revision), rotated)
        .await
        .unwrap();
    assert!(auth
        .restore_owner_admission(&identity, key.expose())
        .await
        .is_err());
    assert!(auth
        .restore_owner_admission(&identity, replacement.expose())
        .await
        .is_err());
}
