use super::*;
use crate::sandbox::{CredentialError, CredentialMaterial};
use layer2_core::SandboxId;
use serde_json::json;

struct NoCredentials;
#[async_trait::async_trait]
impl SandboxCredentialController for NoCredentials {
    async fn issue(
        &self,
        _: &Sandbox,
        _: u64,
        _: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        unreachable!()
    }
    async fn material(
        &self,
        _: &SandboxId,
        _: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        unreachable!()
    }
    async fn custodied(&self, _: &SandboxId) -> Result<Vec<CredentialMaterial>, CredentialError> {
        unreachable!()
    }
    async fn forget(&self, _: &SandboxId, _: u64) -> Result<(), CredentialError> {
        unreachable!()
    }
    async fn revoke(&self, _: &Sandbox, _: &str) -> Result<(), CredentialError> {
        unreachable!()
    }
}

#[test]
fn provider_catalogue_is_the_only_source_of_selectable_models() {
    let models =
        parse(br#"{"object":"list","data":[{"id":"z-model","name":"Model Z"},{"id":"a-model"}]}"#)
            .unwrap();
    assert_eq!(
        models,
        vec![
            AgentModelView {
                id: "a-model".into(),
                label: "a-model".into()
            },
            AgentModelView {
                id: "z-model".into(),
                label: "Model Z".into()
            }
        ]
    );
    assert!(parse(br#"{"data":[]}"#).unwrap().is_empty());
    for value in [
        json!({"data":[{"id":"x"},{"id":"x"}]}),
        json!({"data":[{"id":" x"}]}),
        json!({"data":[{"id":"x\ny"}]}),
        json!({"error":"refused"}),
    ] {
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    assert!(parse(&serde_json::to_vec(&json!({"data": (0..=MAX_MODELS).map(|i| json!({"id":format!("m-{i}")})).collect::<Vec<_>>()})).unwrap()).is_err());
}

#[tokio::test]
async fn queries_exact_public_service_with_scoped_bearer_and_refuses_redirects() {
    use axum::{
        http::{header, StatusCode},
        routing::get,
        Json, Router,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    let redirected = Arc::new(AtomicUsize::new(0));
    let probe = redirected.clone();
    let app = Router::new()
        .route(
            "/api/v1/proxy/s/chrono-llm-public/models",
            get(|headers: axum::http::HeaderMap| async move {
                assert!(headers
                    .get(header::AUTHORIZATION)
                    .is_some_and(|value| value == "Bearer scoped-test-key"));
                Json(json!({"data":[{"id":"catalogue-model"}]}))
            }),
        )
        .route(
            "/api/v1/proxy/s/chrono-llm/models",
            get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/credential-sink")]) }),
        )
        .route(
            "/credential-sink",
            get(move || {
                let probe = probe.clone();
                async move {
                    probe.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let catalog =
        ChronoAgentModels::new(&format!("http://{address}"), Arc::new(NoCredentials)).unwrap();
    let models = catalog
        .fetch("chrono-llm-public", "scoped-test-key")
        .await
        .unwrap();
    assert_eq!(models[0].id, "catalogue-model");
    assert!(catalog
        .fetch("chrono-llm", "scoped-test-key")
        .await
        .is_err());
    assert_eq!(redirected.load(Ordering::SeqCst), 0);
    task.abort();
}
