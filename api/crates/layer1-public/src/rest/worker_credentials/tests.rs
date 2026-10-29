use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use layer2_core::{
    sandbox::{binding::*, store::SandboxStoreError, InMemorySandboxStore},
    AgentId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

pub(in crate::rest) async fn fixture(mode: &str) -> Router {
    let store = Arc::new(InMemorySandboxStore::default());
    let original = "a".repeat(64);
    let replacement = "b".repeat(64);
    if matches!(mode, "current" | "rotated" | "retired") {
        let body = crate::rest::bindings_tests::body();
        let mapped = map_profile(
            body["profile"].clone(),
            body["profile_digest"].as_str().unwrap(),
            Default::default(),
            "responses",
            layer2_core::profiles::ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 8000,
                min_memory_mib: 512,
                max_memory_mib: 16384,
            },
        )
        .unwrap();
        let mut binding = ProviderBinding::new(
            "classifier-binding".into(),
            SubjectId::new("gateway-service"),
            mapped,
            original,
            layer2_core::now_rfc3339(),
        )
        .unwrap();
        store.save_binding(None, binding.clone()).await.unwrap();
        if mode != "current" {
            let revision = binding.revision;
            binding.state = BindingState::Ready;
            binding.work = BindingWork::Idle;
            binding
                .advance_revision(layer2_core::now_rfc3339())
                .unwrap();
            store
                .save_binding(Some(revision), binding.clone())
                .await
                .unwrap();
            let rotated = binding
                .rotate_credential(replacement, layer2_core::now_rfc3339())
                .unwrap();
            store
                .save_binding(Some(binding.revision), rotated.clone())
                .await
                .unwrap();
            binding = rotated;
        }
        if mode == "retired" {
            let revision = binding.revision;
            binding.state = BindingState::Retired;
            binding.work = BindingWork::RetireCurrentConversations;
            binding
                .advance_revision(layer2_core::now_rfc3339())
                .unwrap();
            store.save_binding(Some(revision), binding).await.unwrap();
        }
    }
    let access = if matches!(mode, "unavailable" | "timeout") {
        BindingAccessService::new(Arc::new(FailedStore {
            pending: mode == "timeout",
        }))
    } else if mode == "unsupported" {
        BindingAccessService::new(Arc::new(LegacyStore))
    } else {
        BindingAccessService::new(store)
    };
    let router = worker_credential_router(access);
    if mode == "anonymous" {
        router
    } else {
        router.layer(Extension(VerifiedGatewayService::new(SubjectId::new(
            "gateway-service",
        ))))
    }
}

async fn call(app: Router, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::post(WORKER_CLASSIFIER_PATH)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let status = response.status();
    (
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn current_rotated_and_retired_keys_remain_worker_keys() {
    for mode in ["current", "rotated", "retired"] {
        for digest in if mode == "current" {
            vec!["a".repeat(64)]
        } else {
            vec!["a".repeat(64), "b".repeat(64)]
        } {
            let (status, body) = call(fixture(mode).await, json!({"sha256":digest})).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!({"classification":"known_worker"}), "{mode}");
        }
    }
    assert_eq!(
        call(fixture("retired").await, json!({"sha256":"c".repeat(64)}))
            .await
            .1,
        json!({"classification":"unknown"})
    );
}
#[tokio::test]
async fn malformed_inputs_and_unavailable_store_never_become_unknown() {
    for body in [
        json!({}),
        json!({"sha256":"A".repeat(64)}),
        json!({"sha256":"a".repeat(63)}),
        json!({"sha256":"a".repeat(65)}),
        json!({"sha256":"a".repeat(64),"token":"unexpected"}),
        json!({"sha256":null}),
    ] {
        assert_eq!(
            call(fixture("unknown").await, body).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    for mode in ["unavailable", "timeout", "unsupported"] {
        let (status, body) = call(fixture(mode).await, json!({"sha256":"a".repeat(64)})).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.get("classification").is_none());
    }
    assert_eq!(
        call(fixture("anonymous").await, json!({"sha256":"a".repeat(64)}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

struct FailedStore {
    pending: bool,
}
#[async_trait::async_trait]
impl BindingStore for FailedStore {
    // A fallback to the old weaker port would incorrectly return unknown.
    async fn contains_key_hash(&self, _: &str) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }
    async fn classify_worker_key_hash(&self, _: &str) -> Result<bool, SandboxStoreError> {
        if self.pending {
            std::future::pending::<()>().await;
        }
        Err(SandboxStoreError::Unavailable(
            "fixture storage failure".into(),
        ))
    }
    async fn find_by_key_hash(
        &self,
        _: &str,
    ) -> Result<Option<ProviderBinding>, SandboxStoreError> {
        unreachable!()
    }
    async fn binding_conversation_for_agent(
        &self,
        _: &str,
        _: u64,
        _: &AgentId,
    ) -> Result<Option<BindingConversation>, SandboxStoreError> {
        unreachable!()
    }
    async fn binding(&self, _: &str) -> Result<Option<ProviderBinding>, SandboxStoreError> {
        unreachable!()
    }
    async fn save_binding(
        &self,
        _: Option<u64>,
        _: ProviderBinding,
    ) -> Result<(), SandboxStoreError> {
        unreachable!()
    }
    async fn pending_bindings(
        &self,
        _: Option<&str>,
        _: u16,
    ) -> Result<Vec<ProviderBinding>, SandboxStoreError> {
        unreachable!()
    }
    async fn binding_conversation(
        &self,
        _: &str,
        _: u64,
        _: &str,
    ) -> Result<Option<BindingConversation>, SandboxStoreError> {
        unreachable!()
    }
    async fn save_binding_conversation(
        &self,
        _: u64,
        _: Option<u64>,
        _: BindingConversation,
    ) -> Result<(), SandboxStoreError> {
        unreachable!()
    }
    async fn binding_conversations(
        &self,
        _: &str,
        _: u64,
        _: Option<&str>,
        _: u16,
    ) -> Result<Vec<BindingConversation>, SandboxStoreError> {
        unreachable!()
    }
    async fn pending_binding_conversations(
        &self,
        _: Option<&str>,
        _: u16,
    ) -> Result<Vec<BindingConversation>, SandboxStoreError> {
        unreachable!()
    }
}

#[tokio::test]
async fn malformed_duplicate_and_oversized_json_are_bounded_refusals() {
    for raw in [
        "{".to_owned(),
        format!(
            r#"{{"sha256":"{}","sha256":"{}"}}"#,
            "a".repeat(64),
            "b".repeat(64)
        ),
        format!(r#"{{"sha256":"{}"}}"#, "a".repeat(2048)),
    ] {
        let response = fixture("unknown")
            .await
            .oneshot(
                Request::post(WORKER_CLASSIFIER_PATH)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(raw))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["code"], "worker_classifier_invalid_digest");
    }
}

/// A previously implemented store must not silently opt into fresh negative
/// classification. It inherits the port's explicit unavailable default.
struct LegacyStore;
#[async_trait::async_trait]
impl BindingStore for LegacyStore {
    async fn contains_key_hash(&self, _: &str) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }
    async fn find_by_key_hash(
        &self,
        _: &str,
    ) -> Result<Option<ProviderBinding>, SandboxStoreError> {
        unreachable!()
    }
    async fn binding_conversation_for_agent(
        &self,
        _: &str,
        _: u64,
        _: &AgentId,
    ) -> Result<Option<BindingConversation>, SandboxStoreError> {
        unreachable!()
    }
    async fn binding(&self, _: &str) -> Result<Option<ProviderBinding>, SandboxStoreError> {
        unreachable!()
    }
    async fn save_binding(
        &self,
        _: Option<u64>,
        _: ProviderBinding,
    ) -> Result<(), SandboxStoreError> {
        unreachable!()
    }
    async fn pending_bindings(
        &self,
        _: Option<&str>,
        _: u16,
    ) -> Result<Vec<ProviderBinding>, SandboxStoreError> {
        unreachable!()
    }
    async fn binding_conversation(
        &self,
        _: &str,
        _: u64,
        _: &str,
    ) -> Result<Option<BindingConversation>, SandboxStoreError> {
        unreachable!()
    }
    async fn save_binding_conversation(
        &self,
        _: u64,
        _: Option<u64>,
        _: BindingConversation,
    ) -> Result<(), SandboxStoreError> {
        unreachable!()
    }
    async fn binding_conversations(
        &self,
        _: &str,
        _: u64,
        _: Option<&str>,
        _: u16,
    ) -> Result<Vec<BindingConversation>, SandboxStoreError> {
        unreachable!()
    }
    async fn pending_binding_conversations(
        &self,
        _: Option<&str>,
        _: u16,
    ) -> Result<Vec<BindingConversation>, SandboxStoreError> {
        unreachable!()
    }
}
