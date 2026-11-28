use super::*;
use crate::sandbox::{CredentialError, CredentialMaterial};
use layer2_core::SandboxId;
use serde_json::json;

pub(super) struct NoCredentials;
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
        parse(br#"{"object":"list","data":[{"id":"gpt-6-sol","display_name":"GPT-6 Sol","name":"Legacy label"},{"id":"gpt-6"}]}"#)
            .unwrap();
    assert_eq!(
        models,
        vec![
            AgentModelView {
                id: "gpt-6".into(),
                label: "gpt-6".into(),
                available: false,
            },
            AgentModelView {
                id: "gpt-6-sol".into(),
                label: "GPT-6 Sol".into(),
                available: false,
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

#[test]
fn mixed_chrono_catalogue_excludes_image_and_unrecognized_model_families() {
    let data = [
        "gpt-5.6-sol",
        "gpt-6",
        "gpt-5.6",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-6.2-sol",
        "gpt-6.2-astra",
        "gpt-6.1-terra",
        "gpt-6.1-sol",
        "gpt-6-sol",
        "gpt-6-luna",
        "gpt-6-astra",
        "gpt-5.4",
        "gpt-5.4-mini",
        "gpt-5.3-codex-spark",
        "codex-auto-review",
        "gpt-5.2",
        "gpt-image-1",
        "gpt-image-1.5",
        "gpt-image-2",
        "gpt-image-2.5-flare",
        "gpt-image-2.5-sunburst",
        "gpt-image-2.6-flare",
        "gpt-image-2.6-sunburst",
    ];
    // The observed provider contract has type=model for every row, including
    // the image models. It provides no modality or endpoint capability fields.
    let body = json!({"data": data.map(|id| json!({"id":id,"type":"model","object":"model","display_name":id}))});
    let models = parse(&serde_json::to_vec(&body).unwrap()).unwrap();
    assert_eq!(models.len(), 17);
    for id in ["gpt-6.2-sol", "gpt-6.2-astra"] {
        assert!(models
            .iter()
            .any(|model| model.id == id && model.label == id));
    }
    assert!(models.iter().any(|model| model.id == "gpt-6.1-sol"));
    assert!(models.iter().any(|model| model.id == "codex-auto-review"));
    assert!(models
        .iter()
        .all(|model| !model.id.starts_with("gpt-image")));
    for id in [
        "gpt-",
        "gpt-image-3",
        "gpt-latest",
        "gpt-6/other",
        "gpt-６",
        "o3",
        "claude-new",
    ] {
        assert!(!agent_model(id), "unexpected admitted model: {id}");
    }
    for id in [
        "gpt-6.2-future",
        "gpt-7",
        "gpt-6_preview",
        "codex-auto-review",
    ] {
        assert!(agent_model(id), "catalogue model was refused: {id}");
    }
}

#[test]
fn labels_use_display_name_then_legacy_name_then_identity() {
    let body = br#"{"data":[{"id":"gpt-6","display_name":"GPT-6","name":"Old name"},{"id":"gpt-6-sol","display_name":" bad ","name":"Sol"},{"id":"gpt-6-luna","display_name":"","name":"\nbad"}]}"#;
    let models = parse(body).unwrap();
    assert_eq!(
        models
            .iter()
            .map(|model| model.label.as_str())
            .collect::<Vec<_>>(),
        ["GPT-6", "gpt-6-luna", "Sol"]
    );
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
                assert_eq!(headers[header::USER_AGENT], USER_AGENT);
                assert_eq!(headers[header::ACCEPT], "application/json");
                Json(json!({"data":[{"id":"gpt-6-sol","display_name":"GPT-6 Sol"},{"id":"gpt-image-2"}]}))
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
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-6-sol");
    assert_eq!(models[0].label, "GPT-6 Sol");
    assert!(catalog
        .fetch("chrono-llm", "scoped-test-key")
        .await
        .is_err());
    assert_eq!(redirected.load(Ordering::SeqCst), 0);
    task.abort();
}
