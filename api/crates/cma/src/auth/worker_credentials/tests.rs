use super::*;
use crate::auth::{persistence::MemoryAuthStore, ProviderMetadata};
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{HeaderMap, Request as HttpRequest},
    routing::{get, post},
    Form, Json, Router,
};
use base64::Engine as _;
use layer1_public::rest::worker_credentials::worker_credential_router;
use layer2_core::sandbox::{binding::BindingAccessService, InMemorySandboxStore};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use tower::ServiceExt;

#[derive(Clone)]
struct Provider {
    expected: String,
    disposition: Arc<RwLock<Value>>,
    status: Arc<RwLock<StatusCode>>,
}
struct Fixture {
    provider: Provider,
    auth: Arc<AuthState>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn token(claims: Value) -> String {
    format!(
        "header.{}.signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).unwrap())
    )
}
fn service_token() -> String {
    token(json!({"sa":true,"token_type":"access","sub":"gateway-service"}))
}
async fn userinfo(State(state): State<Provider>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some(format!("Bearer {}", state.expected).as_str())
    {
        return (StatusCode::UNAUTHORIZED, Json(json!({})));
    }
    (
        *state.status.read().await,
        Json(json!({"sub":"gateway-service"})),
    )
}
async fn introspect(
    State(state): State<Provider>,
    Form(form): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    assert_eq!(form["token_type_hint"], "access_token");
    assert_eq!(form["client_id"], "cma-client");
    let body = if form["token"] == state.expected {
        state.disposition.read().await.clone()
    } else {
        json!({"active":false})
    };
    if body.get("hang").and_then(Value::as_bool) == Some(true) {
        std::future::pending::<()>().await;
    }
    (*state.status.read().await, Json(body))
}
impl Fixture {
    async fn new() -> Self {
        let provider = Provider {
            expected: service_token(),
            disposition: Arc::new(RwLock::new(
                json!({"active":true,"sub":"gateway-service","token_type":"access",
                "exp":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()+3600,
                "permissions":["cma:provider:bindings","cma:gateway:records"]}),
            )),
            status: Arc::new(RwLock::new(StatusCode::OK)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/userinfo", get(userinfo))
            .route("/introspect", post(introspect))
            .with_state(provider.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: base.clone(),
                client_id: "cma-client".into(),
                client_secret: uuid::Uuid::new_v4().to_string(),
                public_url: "https://cma.example".into(),
            },
            None,
            Arc::new(MemoryAuthStore::default()),
        );
        *auth.fixture_metadata().write().await = Some(ProviderMetadata {
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            userinfo_endpoint: Some(format!("{base}/userinfo")),
            introspection_endpoint: Some(format!("{base}/introspect")),
        });
        Self {
            provider,
            auth,
            task,
        }
    }
    fn app(&self) -> Router {
        worker_credential_router(BindingAccessService::new(Arc::new(
            InMemorySandboxStore::default(),
        )))
        .layer(axum::middleware::from_fn_with_state(
            self.auth.clone(),
            super::super::attach_subject,
        ))
    }
    async fn call(&self, bearer: Option<&str>, extra: &[(&str, &str)]) -> Response {
        let mut request = HttpRequest::post(WORKER_CLASSIFIER_PATH)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(value) = bearer {
            request = request.header(header::AUTHORIZATION, format!("Bearer {value}"));
        }
        for (name, value) in extra {
            request = request.header(*name, *value);
        }
        self.app()
            .oneshot(
                request
                    .body(Body::from(json!({"sha256":"a".repeat(64)}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn existing_gateway_service_authenticates_and_is_rechecked_on_every_request() {
    let fixture = Fixture::new().await;
    let response = fixture.call(Some(&fixture.provider.expected), &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        serde_json::from_slice::<Value>(&to_bytes(response.into_body(), 1024).await.unwrap())
            .unwrap(),
        json!({"classification":"unknown"})
    );
    fixture.provider.disposition.write().await["permissions"] = json!(["cma:provider:bindings"]);
    assert_eq!(
        fixture
            .call(Some(&fixture.provider.expected), &[])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    fixture.provider.disposition.write().await["active"] = json!(false);
    assert_eq!(
        fixture
            .call(Some(&fixture.provider.expected), &[])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn ordinary_owner_worker_browser_and_delegated_credentials_never_classify() {
    let fixture = Fixture::new().await;
    for claims in [
        json!({"sub":"owner","token_type":"access","permissions":["cma:*"]}),
        json!({"sub":"owner","sa":true,"token_type":"api_key"}),
        json!({"sub":"gateway-service","sa":true,"token_type":"access","delegated":true}),
        json!({"sub":"gateway-service","sa":true,"token_type":"access","act":{"sub":"actor"}}),
        json!({"sub":"gateway-service","sa":true,"token_type":"access","resource":["cma"]}),
    ] {
        assert_eq!(
            fixture.call(Some(&token(claims)), &[]).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for value in [
        None,
        Some("nyx_worker-example"),
        Some("ordinary-opaque-owner"),
    ] {
        assert_eq!(
            fixture.call(value, &[]).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for extra in [
        [("cookie", "cma_session=owner")],
        [(super::super::resource_request::HEADER, "proof")],
        [(super::super::nyxid_proxy::HEADER, "proof")],
    ] {
        assert_eq!(
            fixture
                .call(Some(&fixture.provider.expected), &extra)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
}

#[tokio::test]
async fn unverified_service_claim_or_mismatched_introspection_cannot_grant_authority() {
    let fixture = Fixture::new().await;
    let forged =
        token(json!({"sub":"gateway-service","sa":true,"token_type":"access","forged":true}));
    assert_eq!(
        fixture.call(Some(&forged), &[]).await.status(),
        StatusCode::FORBIDDEN
    );
    for (key, value) in [
        ("sub", json!("another-service")),
        ("token_type", json!("api_key")),
        ("exp", json!(1)),
        ("resource", json!(["restricted"])),
    ] {
        let original = fixture.provider.disposition.read().await.clone();
        fixture.provider.disposition.write().await[key] = value;
        assert_eq!(
            fixture
                .call(Some(&fixture.provider.expected), &[])
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{key}"
        );
        *fixture.provider.disposition.write().await = original;
    }
}

#[tokio::test]
async fn unavailable_provider_or_missing_introspection_is_not_unknown() {
    let fixture = Fixture::new().await;
    *fixture.provider.status.write().await = StatusCode::SERVICE_UNAVAILABLE;
    assert_eq!(
        fixture
            .call(Some(&fixture.provider.expected), &[])
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    *fixture.provider.status.write().await = StatusCode::OK;
    fixture
        .auth
        .fixture_metadata()
        .write()
        .await
        .as_mut()
        .unwrap()
        .introspection_endpoint = None;
    assert_eq!(
        fixture
            .call(Some(&fixture.provider.expected), &[])
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[test]
fn only_the_exact_classifier_path_enters_machine_authentication() {
    assert!(matches_path(WORKER_CLASSIFIER_PATH));
    for path in [
        format!("{WORKER_CLASSIFIER_PATH}/"),
        format!("{WORKER_CLASSIFIER_PATH}/extra"),
        "/api/v1/cmaeg/worker-credentials".into(),
        "/api/v1/cmaeg/bindings/one".into(),
    ] {
        assert!(!matches_path(&path));
    }
}

#[tokio::test]
async fn service_verification_timeout_fails_closed() {
    let fixture = Fixture::new().await;
    fixture.provider.disposition.write().await["hang"] = json!(true);
    let response = fixture.call(Some(&fixture.provider.expected), &[]).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["code"], "worker_classifier_unavailable");
    assert!(body.get("classification").is_none());
}

#[tokio::test]
async fn method_query_and_duplicate_bearer_cannot_enter_classifier() {
    let fixture = Fixture::new().await;
    for (method, path) in [
        ("GET", WORKER_CLASSIFIER_PATH.to_owned()),
        ("POST", format!("{WORKER_CLASSIFIER_PATH}?digest=other")),
    ] {
        let response = fixture
            .app()
            .oneshot(
                HttpRequest::builder()
                    .method(method)
                    .uri(path)
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", fixture.provider.expected),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        fixture
            .call(
                Some(&fixture.provider.expected),
                &[("authorization", "Bearer second")]
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn worker_metrics_cover_independent_authentication_and_early_rejection() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let f = Fixture::new().await;
    let app = f
        .app()
        .layer(axum::middleware::from_fn(crate::telemetry::http::observe));
    for bearer in [Some(f.provider.expected.as_str()), None] {
        let mut request = HttpRequest::post(WORKER_CLASSIFIER_PATH)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = bearer {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(Body::from(json!({"sha256":"a".repeat(64)}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if bearer.is_some() {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            }
        );
    }
    let text = handle.render();
    assert!(text.contains("cma_auth_resolutions_total{path=\"worker\",outcome=\"resolved\"} 1"));
    assert!(text.contains("cma_auth_resolutions_total{path=\"worker\",outcome=\"rejected\"} 1"));
    assert!(text.contains("status=\"200\",consumer=\"internal\",protocol=\"rest\""));
    assert!(text.contains("status=\"401\",consumer=\"unknown\",protocol=\"rest\""));
}

struct ErasingService;
#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for ErasingService {
    async fn erasing(
        &self,
        _: &SubjectId,
    ) -> Result<bool, layer2_core::account_deletion::AccountDeletionError> {
        Ok(true)
    }
}
#[tokio::test]
async fn standing_gate_denies_verified_worker_before_early_return() {
    let mut fixture = Fixture::new().await;
    Arc::get_mut(&mut fixture.auth)
        .unwrap()
        .set_fixture_standing(Arc::new(ErasingService));
    let response = fixture.call(Some(&fixture.provider.expected), &[]).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["code"], "account_deletion_in_progress");
}
