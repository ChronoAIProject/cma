use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use layer2_core::trigger_domain::{
    ServiceConnectionBroker as _, ServiceConnectionError, ServiceRequirementState,
};
use layer2_core::{Authorization, ServiceId, SubjectId};
use serde_json::json;

use super::*;
use crate::auth::{
    persistence::{
        AuthStore as _, DurableSubjectTokens, MemoryAuthStore, SecretValue, SubjectAuthState,
    },
    profile::Profile,
};

const OWNER: &str = "visitor-one";

#[tokio::test]
async fn trusted_mode_has_no_provider_and_fails_closed_for_service_work() {
    let broker = CmaServiceConnectionBroker::from_auth_config(
        &AuthMode::Trusted {
            subject: OWNER.into(),
        },
        Some("not a provider URL"),
        AuthState::in_memory(
            AuthMode::Trusted {
                subject: OWNER.into(),
            },
            None,
        ),
    )
    .expect("trusted broker builds without provider configuration");
    assert!(matches!(&broker.mode, BrokerMode::Trusted));

    let owner = SubjectId::new(OWNER);
    let service = ServiceId::new("github");
    assert_eq!(broker.requirements(&owner, &[]).await, Ok(Vec::new()));
    assert_eq!(
        broker
            .requirements(&owner, std::slice::from_ref(&service))
            .await,
        Err(ServiceConnectionError::Refused)
    );
    assert_eq!(
        broker.create_handoff(&owner, &service, "intent-one").await,
        Err(ServiceConnectionError::Refused)
    );
    assert_eq!(
        broker.handoff_state(&owner, "link-one", &service).await,
        Err(ServiceConnectionError::Refused)
    );
}

#[tokio::test]
async fn provider_mode_uses_the_auth_back_channel_and_delegates_requests() {
    let provider = Provider::start().await;
    let mode = provider_mode(&provider.base_url);
    let auth = provider_auth(mode.clone()).await;
    let broker =
        CmaServiceConnectionBroker::from_auth_config(&mode, Some("https://nyx.example"), auth)
            .expect("provider broker builds");

    let requirements = broker
        .requirements(
            &SubjectId::new(OWNER),
            std::slice::from_ref(&ServiceId::new("github")),
        )
        .await
        .expect("provider request is delegated");

    assert_eq!(requirements.len(), 1);
    assert_eq!(requirements[0].state, ServiceRequirementState::Satisfied);
    assert_eq!(provider.requests(), 1);
    let BrokerMode::Provider(provider_broker) = &broker.mode else {
        panic!("provider auth built a trusted broker");
    };
    assert_eq!(provider_broker.base_url, provider.base_url);
    assert_eq!(
        provider_broker.browser_origin.as_str(),
        "https://nyx.example/"
    );
}

#[tokio::test]
async fn provider_client_does_not_follow_redirects() {
    let provider = Provider::redirecting().await;
    let mode = provider_mode(&provider.base_url);
    let broker = CmaServiceConnectionBroker::from_auth_config(
        &mode,
        Some("https://nyx.example"),
        provider_auth(mode.clone()).await,
    )
    .expect("provider broker builds");

    assert_eq!(
        broker
            .requirements(
                &SubjectId::new(OWNER),
                std::slice::from_ref(&ServiceId::new("github")),
            )
            .await,
        Err(ServiceConnectionError::Refused)
    );
    assert_eq!(provider.requests(), 1);
    assert_eq!(provider.redirected_requests(), 0);
}

#[test]
fn provider_mode_requires_the_separate_browser_origin() {
    let mode = provider_mode("https://identity.example");
    let error = CmaServiceConnectionBroker::from_auth_config(
        &mode,
        None,
        AuthState::in_memory(mode.clone(), None),
    )
    .err()
    .expect("missing browser origin is rejected");

    assert_eq!(
        error,
        ServiceConnectionError::Malformed(INVALID_PROVIDER_CONFIGURATION.into())
    );
}

fn provider_mode(base_url: &str) -> AuthMode {
    AuthMode::Provider {
        base_url: base_url.into(),
        client_id: "client".into(),
        client_secret: "test-value".into(),
        public_url: "https://cma.example".into(),
    }
}

async fn provider_auth(mode: AuthMode) -> Arc<AuthState> {
    let store = Arc::new(MemoryAuthStore::default());
    store
        .put_subject(SubjectAuthState {
            subject: SubjectId::new(OWNER),
            authorization: Authorization::none(),
            profile: Profile::default(),
            tokens: Some(DurableSubjectTokens {
                access_token: SecretValue::new("visitor-test-value".into()),
                refresh_token: None,
                expires_at: std::time::SystemTime::now() + Duration::from_secs(60 * 60),
            }),
        })
        .await
        .expect("subject credential is stored");
    AuthState::with_store_and_nyxid_public_url(
        mode,
        None,
        store,
        Some("https://nyx.example".into()),
    )
}

struct Provider {
    base_url: String,
    state: Arc<ProviderState>,
    server: tokio::task::JoinHandle<()>,
}

impl Provider {
    async fn start() -> Self {
        Self::start_with_redirects(false).await
    }

    async fn redirecting() -> Self {
        Self::start_with_redirects(true).await
    }

    async fn start_with_redirects(redirect_services: bool) -> Self {
        let state = Arc::new(ProviderState {
            redirect_services: AtomicBool::new(redirect_services),
            ..ProviderState::default()
        });
        let app = Router::new()
            .route("/api/v1/user-services", get(user_services))
            .route("/redirected-services", get(redirected_services))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("provider fixture binds");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("provider fixture address")
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("provider fixture serves");
        });
        Self {
            base_url,
            state,
            server,
        }
    }

    fn requests(&self) -> usize {
        self.state.requests.load(Ordering::SeqCst)
    }

    fn redirected_requests(&self) -> usize {
        self.state.redirected_requests.load(Ordering::SeqCst)
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[derive(Default)]
struct ProviderState {
    requests: AtomicUsize,
    redirected_requests: AtomicUsize,
    redirect_services: AtomicBool,
}

async fn user_services(State(state): State<Arc<ProviderState>>) -> Response {
    state.requests.fetch_add(1, Ordering::SeqCst);
    if state.redirect_services.load(Ordering::SeqCst) {
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, "/redirected-services")],
        )
            .into_response();
    }
    service_listing().into_response()
}

async fn redirected_services(State(state): State<Arc<ProviderState>>) -> Response {
    state.redirected_requests.fetch_add(1, Ordering::SeqCst);
    service_listing().into_response()
}

fn service_listing() -> Json<serde_json::Value> {
    Json(json!({
        "services": [{
            "id": "github",
            "slug": "github",
            "is_active": true,
            "credential_source": { "type": "personal" }
        }]
    }))
}
