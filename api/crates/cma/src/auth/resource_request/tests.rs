use super::*;
use axum::{
    body::to_bytes,
    extract::Extension,
    http::{HeaderMap, Method, Request as HttpRequest},
    routing::{get, post},
    Form, Router,
};
use layer2_core::{Authorization, Capability};
use serde_json::{json, Value};
use std::{collections::HashMap, time::Duration};
use tokio::sync::RwLock;
use tower::ServiceExt;

use crate::auth::{persistence::MemoryAuthStore, ProviderMetadata};

mod compatibility;
mod paired;
mod public_routes;

struct Fixture {
    base: String,
    evidence: Arc<RwLock<Value>>,
    status: Arc<RwLock<StatusCode>>,
    forms: Arc<RwLock<Vec<HashMap<String, String>>>>,
    store: Arc<MemoryAuthStore>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        let descriptor = request_descriptor(
            &Method::GET,
            &"/api/v1/agents".parse().unwrap(),
            &HeaderMap::new(),
        )
        .unwrap();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let evidence = Arc::new(RwLock::new(json!({
            "active": true, "token_type": "resource_request", "aud": "cma-service",
            "sub": "owner", "scope": "proxy", "iss": "https://issuer.example",
            "iat": now, "exp": now + 60, "jti": uuid::Uuid::new_v4().to_string(),
            "permissions": ["cma:sandbox:use"], "resource_request": descriptor,
            "independent_commands": false,
        })));
        let status = Arc::new(RwLock::new(StatusCode::OK));
        let forms = Arc::new(RwLock::new(Vec::new()));
        let (returned, http_status, observed) = (evidence.clone(), status.clone(), forms.clone());
        let app = Router::new().route(
            "/introspect",
            post(move |Form(form): Form<HashMap<String, String>>| {
                let (returned, http_status, observed) =
                    (returned.clone(), http_status.clone(), observed.clone());
                async move {
                    assert_eq!(form["token_type_hint"], "resource_request");
                    assert_eq!(form["service_id"], "cma-service");
                    assert_eq!(form["client_id"], "cma-client");
                    assert!(!form["client_secret"].is_empty());
                    observed.write().await.push(form);
                    (
                        *http_status.read().await,
                        Json(returned.read().await.clone()),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base,
            evidence,
            status,
            forms,
            store: Arc::new(MemoryAuthStore::default()),
            server,
        }
    }

    async fn auth(&self, enabled: bool) -> Arc<AuthState> {
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: self.base.clone(),
                client_id: "cma-client".into(),
                client_secret: uuid::Uuid::new_v4().to_string(),
                public_url: "https://cma.example".into(),
            },
            None,
            self.store.clone(),
        )
        .with_agent_key_service(Some("cma-service".into()))
        .with_resource_request_enabled(enabled);
        *auth.fixture_metadata().write().await = Some(ProviderMetadata {
            authorization_endpoint: format!("{}/authorize", self.base),
            token_endpoint: format!("{}/token", self.base),
            introspection_endpoint: Some(format!("{}/introspect", self.base)),
            userinfo_endpoint: None,
        });
        auth
    }

    async fn bind(&self, method: Method, uri: &str, headers: &HeaderMap) {
        self.evidence.write().await["resource_request"] = serde_json::to_value(
            request_descriptor(&method, &uri.parse().unwrap(), headers).unwrap(),
        )
        .unwrap();
    }
}

fn request(method: Method, uri: &str) -> HttpRequest<Body> {
    HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header(HEADER, uuid::Uuid::new_v4().to_string())
        .header(header::AUTHORIZATION, "Bearer unrelated-ambient-credential")
        .header(header::COOKIE, "cma_session=unrelated")
        .body(Body::empty())
        .unwrap()
}

async fn inspect(Extension(context): Extension<CallerContext>, request: Request) -> Json<Value> {
    assert!(context.validate_principal());
    assert!(matches!(
        context.credential_source,
        CredentialSource::RequestBoundRead { .. }
    ));
    assert!(request
        .extensions()
        .get::<layer1_public::DelegatedCredential>()
        .is_none());
    for name in [
        HEADER,
        crate::auth::nyxid_proxy::HEADER,
        header::AUTHORIZATION.as_str(),
        header::COOKIE.as_str(),
    ] {
        assert!(!request.headers().contains_key(name));
    }
    Json(
        json!({"subject": context.caller.subject.as_str(), "use": context.caller.authorization.allows(Capability::UseOwnSandboxes)}),
    )
}

fn app(auth: Arc<AuthState>) -> Router {
    Router::new()
        .route("/api/v1/agents", get(inspect))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::attach_subject,
        ))
}

#[tokio::test]
async fn exact_reads_reverify_each_time_without_browser_or_api_custody() {
    let f = Fixture::new().await;
    let app = app(f.auth(true).await);
    f.store.set_unavailable(true);
    let proof = uuid::Uuid::new_v4().to_string();
    for _ in 0..2 {
        let mut read = request(Method::GET, "/api/v1/agents");
        read.headers_mut().insert(HEADER, proof.parse().unwrap());
        let response = app.clone().oneshot(read).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            json!({"subject":"owner","use":true})
        );
    }
    let forms = f.forms.read().await;
    assert_eq!(forms.len(), 2);
    assert_eq!(forms[0]["request_path_and_query"], "/api/v1/agents");
    assert!(!forms[0].contains_key("request_content_type"));
}

#[tokio::test]
async fn enforced_proxy_read_uses_only_the_bound_proof_identity() {
    use crate::auth::nyxid_proxy::jwt::Signer;

    let fixture = Fixture::new().await;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let legacy_identity = Signer::primary().token(
        &json!({
            "iss": "https://issuer.example", "aud": "https://cma.example",
            "nyx_service_id": "cma-service", "sub": "different-subject",
            "iat": now, "exp": now + 60, "jti": uuid::Uuid::new_v4().to_string(),
            "permissions": ["cma:sandbox:create"],
        }),
        "fixture-key",
    );
    let auth = fixture
        .auth(true)
        .await
        .with_proxy_require_request_proof(true);
    let mut read = request(Method::GET, "/api/v1/agents");
    read.headers_mut().insert(
        crate::auth::nyxid_proxy::HEADER,
        legacy_identity.parse().unwrap(),
    );
    let response = app(auth).oneshot(read).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"subject":"owner","use":true})
    );
    assert_eq!(fixture.forms.read().await.len(), 1);
}

#[tokio::test]
async fn disabled_duplicate_and_mutating_proofs_never_fall_back_to_ambient_auth() {
    let f = Fixture::new().await;
    f.store.set_unavailable(true);
    let disabled = app(f.auth(false).await);
    assert_eq!(
        disabled
            .oneshot(request(Method::GET, "/api/v1/agents"))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let app = app(f.auth(true).await);
    let mut duplicate = request(Method::GET, "/api/v1/agents");
    duplicate
        .headers_mut()
        .append(HEADER, uuid::Uuid::new_v4().to_string().parse().unwrap());
    assert_eq!(
        app.clone().oneshot(duplicate).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    for method in [
        Method::POST,
        Method::PATCH,
        Method::PUT,
        Method::DELETE,
        Method::OPTIONS,
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request(method, "/api/v1/agents"))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    for path in [
        "/auth/logout",
        "/api/v1/bots",
        "/api/v1/agents/agt_a/interrupt",
        "/api/v1/agents/agt_a/history",
        "/api/v1/agents/agt_a/settings",
        "/api/v1/agents/agt_a/resources",
        "/api/v1/agui/events?thread_id=agt_a",
        "/api/v2/agents/agt_a/events",
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request(Method::GET, path))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert!(f.forms.read().await.is_empty());
}

#[tokio::test]
async fn exact_original_query_content_type_and_method_are_bound() {
    let f = Fixture::new().await;
    let app = app(f.auth(true).await);
    let path = "/api/v1/agents?limit=10&status=ready&marker=%2f";
    let mut original = request(Method::GET, path);
    original.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/json; charset=utf-8".parse().unwrap(),
    );
    f.bind(Method::GET, path, original.headers()).await;
    assert_eq!(
        app.clone().oneshot(original).await.unwrap().status(),
        StatusCode::OK
    );
    for changed in [
        "/api/v1/agents",
        "/api/v1/agents?status=ready&limit=10&marker=%2f",
        "/api/v1/agents?limit=10&status=ready&marker=%2F",
    ] {
        let mut request = request(Method::GET, changed);
        request.headers_mut().insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for method in [Method::GET, Method::HEAD] {
        assert_eq!(
            app.clone()
                .oneshot(request(method, path))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let nested = Router::new().nest(
        "/api/v1",
        Router::new()
            .route("/agents", get(inspect))
            .layer(axum::middleware::from_fn_with_state(
                f.auth(true).await,
                crate::auth::attach_subject,
            )),
    );
    f.bind(Method::GET, "/api/v1/agents?", &HeaderMap::new())
        .await;
    assert_eq!(
        nested
            .oneshot(request(Method::GET, "/api/v1/agents?"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        f.forms.read().await.last().unwrap()["request_path_and_query"],
        "/api/v1/agents?"
    );
    f.bind(Method::HEAD, "/api/v1/agents", &HeaderMap::new())
        .await;
    assert_eq!(
        app.oneshot(request(Method::HEAD, "/api/v1/agents"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn every_signed_descriptor_field_is_required_and_exact() {
    let f = Fixture::new().await;
    let app = app(f.auth(true).await);
    let original = f.evidence.read().await.clone();
    for (field, value) in [
        ("version", json!(2)),
        ("method", json!("HEAD")),
        ("path_and_query", json!("/api/v1/agents?")),
        ("content_type", json!("application/json")),
        ("body_sha256", json!("00".repeat(32))),
        ("idempotency_key_sha256", json!("00".repeat(32))),
    ] {
        let mut changed = original.clone();
        changed["resource_request"][field] = value;
        *f.evidence.write().await = changed;
        assert_eq!(
            app.clone()
                .oneshot(request(Method::GET, "/api/v1/agents"))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED,
            "{field}"
        );
    }
    let mut missing = original;
    missing["resource_request"]
        .as_object_mut()
        .unwrap()
        .remove("content_type");
    *f.evidence.write().await = missing;
    assert_eq!(
        app.oneshot(request(Method::GET, "/api/v1/agents"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn bodies_encodings_overrides_and_idempotency_are_refused_before_verification() {
    let f = Fixture::new().await;
    let app = app(f.auth(true).await);
    for (name, value) in [
        ("idempotency-key", "command"),
        ("x-http-method-override", "DELETE"),
        ("x-method-override", "POST"),
        ("content-encoding", "gzip"),
        ("content-length", "1"),
    ] {
        let mut request = request(Method::GET, "/api/v1/agents");
        request.headers_mut().insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    for query in ["_method=DELETE", "%5Fmethod=POST", "method=PUT"] {
        assert_eq!(
            app.clone()
                .oneshot(request(Method::GET, &format!("/api/v1/agents?{query}")))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let mut nonempty = request(Method::GET, "/api/v1/agents");
    *nonempty.body_mut() = Body::from("x");
    assert_eq!(
        app.clone().oneshot(nonempty).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let mut duplicate = request(Method::GET, "/api/v1/agents");
    duplicate
        .headers_mut()
        .append(header::CONTENT_TYPE, "application/json".parse().unwrap());
    duplicate
        .headers_mut()
        .append(header::CONTENT_TYPE, "text/plain".parse().unwrap());
    assert_eq!(
        app.oneshot(duplicate).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert!(f.forms.read().await.is_empty());
}

#[tokio::test]
async fn invalid_evidence_and_outages_fail_closed_with_distinct_status() {
    let f = Fixture::new().await;
    let app = app(f.auth(true).await);
    let original = f.evidence.read().await.clone();
    for (field, value, status) in [
        ("active", json!(false), StatusCode::UNAUTHORIZED),
        ("aud", json!("another-service"), StatusCode::UNAUTHORIZED),
        ("sub", json!(""), StatusCode::UNAUTHORIZED),
        ("token_type", json!("api_key"), StatusCode::UNAUTHORIZED),
        (
            "independent_commands",
            json!(true),
            StatusCode::UNAUTHORIZED,
        ),
        ("scope", json!("read"), StatusCode::UNAUTHORIZED),
        ("resource_request", json!({}), StatusCode::UNAUTHORIZED),
        ("exp", json!(1), StatusCode::UNAUTHORIZED),
        ("iat", json!(1), StatusCode::UNAUTHORIZED),
        ("exp", Value::Null, StatusCode::UNAUTHORIZED),
        ("iss", json!(""), StatusCode::UNAUTHORIZED),
        ("jti", json!("invalid"), StatusCode::UNAUTHORIZED),
        ("permissions", json!([1]), StatusCode::SERVICE_UNAVAILABLE),
    ] {
        let mut evidence = original.clone();
        evidence[field] = value;
        *f.evidence.write().await = evidence;
        assert_eq!(
            app.clone()
                .oneshot(request(Method::GET, "/api/v1/agents"))
                .await
                .unwrap()
                .status(),
            status,
            "{field}"
        );
    }
    *f.evidence.write().await = original;
    *f.status.write().await = StatusCode::SERVICE_UNAVAILABLE;
    assert_eq!(
        app.oneshot(request(Method::GET, "/api/v1/agents"))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn request_read_authority_cannot_admit_commands_or_borrow_retained_custody() {
    use layer2_core::sandbox::agent_io::{
        AgentCommandAdmissionAuthority, AgentOwner, CommandAuthorizationError,
    };
    let f = Fixture::new().await;
    let auth = f.auth(true).await;
    f.store.set_unavailable(true);
    let context = CallerContext {
        credential_source: CredentialSource::RequestBoundRead {
            expires_at: SystemTime::now() + Duration::from_secs(30),
        },
        caller: Caller::new(
            layer2_core::SubjectId::new("owner"),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: f.base.clone(),
            subject: layer2_core::SubjectId::new("owner"),
            credential_id: "request".into(),
        },
        actor: ResourceActor::Api {
            subject: layer2_core::SubjectId::new("owner"),
            delegation_id: None,
        },
        memberships: vec![],
    };
    assert!(matches!(
        auth.command_admission_authority(
            &context,
            "any-credential",
            &AgentOwner::Person {
                subject: context.caller.subject.clone()
            }
        )
        .await,
        Err(CommandAuthorizationError::Revoked)
    ));
    assert!(f.forms.read().await.is_empty());
}

#[tokio::test]
async fn request_proof_metrics_cover_verified_refused_and_unavailable_paths() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let f = Fixture::new().await;
    let app =
        app(f.auth(true).await).layer(axum::middleware::from_fn(crate::telemetry::http::observe));
    assert_eq!(
        app.clone()
            .oneshot(request(Method::GET, "/api/v1/agents"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    *f.status.write().await = StatusCode::SERVICE_UNAVAILABLE;
    assert_eq!(
        app.clone()
            .oneshot(request(Method::GET, "/api/v1/agents"))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let disabled = self::app(f.auth(false).await)
        .layer(axum::middleware::from_fn(crate::telemetry::http::observe));
    assert_eq!(
        disabled
            .oneshot(request(Method::GET, "/api/v1/agents"))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let text = handle.render();
    for outcome in ["resolved", "rejected", "unavailable"] {
        assert!(
            text.contains(&format!(
                "cma_auth_resolutions_total{{path=\"resource_request\",outcome=\"{outcome}\"}} 1"
            )),
            "{text}"
        );
    }
    assert!(text.contains("status=\"200\",consumer=\"nyxid\",protocol=\"rest\""));
    assert!(text.contains("status=\"503\",consumer=\"unknown\",protocol=\"rest\""));
}

struct ErasingSubject;
#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for ErasingSubject {
    async fn erasing(
        &self,
        _: &layer2_core::SubjectId,
    ) -> Result<bool, layer2_core::account_deletion::AccountDeletionError> {
        Ok(true)
    }
}
#[tokio::test]
async fn standing_gate_denies_verified_resource_request_before_early_return() {
    let fixture = Fixture::new().await;
    let auth = fixture
        .auth(true)
        .await
        .with_account_standing(Arc::new(ErasingSubject));
    let response = app(auth)
        .oneshot(request(Method::GET, "/api/v1/agents"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(body["code"], "account_deletion_in_progress");
}
