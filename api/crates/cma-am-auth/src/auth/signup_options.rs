//! Anonymous account-creation options projected from NyxID public config.
//!
//! Only the four fields the CMA landing page needs cross this boundary. The
//! provider response contains unrelated deployment details, and forwarding it
//! wholesale would couple the public CMA contract to NyxID's complete config.

use std::{sync::Arc, time::Duration};

use axum::{
    extract::State,
    http::{header, HeaderValue},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::{sync::Mutex, time::Instant};

use super::{AuthMode, AuthState};

const SIGNUP_OPTIONS_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SignupOptions {
    social_providers: Vec<String>,
    email_signup: bool,
    invite_code_required: bool,
    register_available: bool,
}

impl SignupOptions {
    fn unavailable() -> Self {
        Self {
            social_providers: Vec::new(),
            email_signup: false,
            invite_code_required: true,
            register_available: false,
        }
    }
}

#[derive(Debug, Deserialize)]
struct NyxidPublicConfig {
    social_providers: Vec<String>,
    invite_code_required: bool,
    email_auth_enabled: bool,
}

impl From<NyxidPublicConfig> for SignupOptions {
    fn from(config: NyxidPublicConfig) -> Self {
        let register_available = config.email_auth_enabled || !config.social_providers.is_empty();
        Self {
            social_providers: config.social_providers,
            email_signup: config.email_auth_enabled,
            invite_code_required: config.invite_code_required,
            register_available,
        }
    }
}

struct CachedSignupOptions {
    value: SignupOptions,
    expires_at: Instant,
}

#[derive(Default)]
pub(super) struct SignupOptionsCache {
    value: Mutex<Option<CachedSignupOptions>>,
}

pub(super) async fn signup_options(State(auth): State<Arc<AuthState>>) -> impl IntoResponse {
    let options = current_signup_options(&auth).await;
    (
        [(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=300"),
        )],
        Json(options),
    )
}

/// The cached provider options with the platform's registration closure
/// applied on every request. The closure never enters the long provider
/// cache; the read is advisory, so an unreadable state keeps NyxID's value
/// and the sign-in callback stays the authority.
async fn current_signup_options(auth: &AuthState) -> SignupOptions {
    let mut options = cached_signup_options(auth).await;
    if options.register_available && !auth.registration_open().await {
        options.register_available = false;
    }
    options
}

async fn cached_signup_options(auth: &AuthState) -> SignupOptions {
    let now = Instant::now();
    let mut cache = auth.signup_options_cache.value.lock().await;
    if let Some(cached) = cache.as_ref().filter(|cached| cached.expires_at > now) {
        return cached.value.clone();
    }

    let value = match fetch_signup_options(auth).await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(error = %error, "could not read NyxID registration options");
            SignupOptions::unavailable()
        }
    };
    *cache = Some(CachedSignupOptions {
        value: value.clone(),
        expires_at: Instant::now() + SIGNUP_OPTIONS_TTL,
    });
    value
}

async fn fetch_signup_options(auth: &AuthState) -> Result<SignupOptions, String> {
    let AuthMode::Provider { base_url, .. } = &auth.mode else {
        return Err("no identity provider is configured".into());
    };
    let url = format!("{}/api/v1/public/config", base_url.trim_end_matches('/'));
    let response = auth
        .http
        .get(&url)
        .send()
        .await
        .map_err(|error| format!("public config request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("public config returned {}", response.status()));
    }
    response
        .json::<NyxidPublicConfig>()
        .await
        .map(SignupOptions::from)
        .map_err(|error| format!("public config body is not usable: {error}"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::{
        body::{to_bytes, Body},
        extract::State,
        http::{header, Request, StatusCode},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{persistence::MemoryAuthStore, AuthMode};
    use layer2_core::budget::{
        BudgetError, Closure, ClosureReason, CloudObservationInput, ObservationError,
        PlatformAdmissionState, PlatformSpendLedger, PlatformSpendState,
    };

    async fn public_config(State(requests): State<Arc<AtomicUsize>>) -> Json<serde_json::Value> {
        requests.fetch_add(1, Ordering::SeqCst);
        Json(serde_json::json!({
            "frontend_url": "https://nyx.example",
            "version": "test",
            "social_providers": ["github", "google"],
            "invite_code_required": true,
            "email_auth_enabled": false,
        }))
    }

    async fn provider(handler: Router) -> (Arc<AuthState>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake provider");
        let base_url = format!("http://{}", listener.local_addr().expect("local address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, handler)
                .await
                .expect("serve fake provider");
        });
        let auth = AuthState::in_memory(
            AuthMode::Provider {
                base_url,
                client_id: "client".into(),
                client_secret: "secret".into(),
                public_url: "http://cma.invalid".into(),
            },
            None,
        );
        (auth, server)
    }

    #[tokio::test]
    async fn successful_options_are_projected_and_cached() {
        let requests = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/api/v1/public/config", get(public_config))
            .with_state(Arc::clone(&requests));
        let (auth, server) = provider(app).await;

        let first = cached_signup_options(&auth).await;
        let second = cached_signup_options(&auth).await;

        assert_eq!(first, second);
        assert_eq!(first.social_providers, ["github", "google"]);
        assert!(!first.email_signup);
        assert!(first.invite_code_required);
        assert!(first.register_available);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn an_expired_cache_entry_is_refreshed() {
        let requests = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/api/v1/public/config", get(public_config))
            .with_state(Arc::clone(&requests));
        let (auth, server) = provider(app).await;

        cached_signup_options(&auth).await;
        auth.signup_options_cache
            .value
            .lock()
            .await
            .as_mut()
            .expect("cache entry")
            .expires_at = Instant::now();
        cached_signup_options(&auth).await;

        assert_eq!(requests.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn concurrent_misses_are_coalesced() {
        async fn delayed(State(requests): State<Arc<AtomicUsize>>) -> Json<serde_json::Value> {
            tokio::time::sleep(Duration::from_millis(25)).await;
            public_config(State(requests)).await
        }

        let requests = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/api/v1/public/config", get(delayed))
            .with_state(Arc::clone(&requests));
        let (auth, server) = provider(app).await;

        let (one, two, three) = tokio::join!(
            cached_signup_options(&auth),
            cached_signup_options(&auth),
            cached_signup_options(&auth),
        );

        assert_eq!(one, two);
        assert_eq!(two, three);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn upstream_failures_return_and_cache_the_closed_fallback() {
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&requests);
        let app = Router::new().route(
            "/api/v1/public/config",
            get(move || {
                let observed = Arc::clone(&observed);
                async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    StatusCode::BAD_GATEWAY
                }
            }),
        );
        let (auth, server) = provider(app).await;

        let first = cached_signup_options(&auth).await;
        let second = cached_signup_options(&auth).await;

        assert_eq!(first, SignupOptions::unavailable());
        assert_eq!(second, SignupOptions::unavailable());
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn malformed_provider_data_returns_the_closed_fallback() {
        let app = Router::new().route(
            "/api/v1/public/config",
            get(|| async { Json(serde_json::json!({ "social_providers": "github" })) }),
        );
        let (auth, server) = provider(app).await;

        assert_eq!(
            cached_signup_options(&auth).await,
            SignupOptions::unavailable()
        );
        server.abort();
    }

    struct Spend(std::sync::Mutex<Result<PlatformAdmissionState, BudgetError>>);

    #[async_trait::async_trait]
    impl PlatformSpendLedger for Spend {
        async fn admission_state(&self, _: u64) -> Result<PlatformAdmissionState, BudgetError> {
            self.0.lock().unwrap().clone()
        }

        async fn spend_state(&self, _: u64) -> Result<PlatformSpendState, BudgetError> {
            Err(BudgetError::Unavailable)
        }

        async fn record_cloud_observation(
            &self,
            _: CloudObservationInput,
            _: u64,
        ) -> Result<PlatformSpendState, ObservationError> {
            Err(ObservationError::Unavailable)
        }
    }

    fn warning() -> PlatformAdmissionState {
        PlatformAdmissionState {
            configured: true,
            operator_control: layer2_core::budget::OperatorAdmissionState::Open,
            closures: vec![Closure {
                reason: ClosureReason::SpendWarning,
                since: 1,
                settings_revision: 1,
                committed_at_entry: 0,
            }],
        }
    }

    #[tokio::test]
    async fn operator_stop_hides_registration_even_with_cached_provider_options_and_metering_off() {
        let requests = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/api/v1/public/config", get(public_config))
            .with_state(Arc::clone(&requests));
        let (auth, server) = provider(app).await;
        let stopped = PlatformAdmissionState {
            operator_control: layer2_core::budget::OperatorAdmissionState::Stopped(
                layer2_core::admission::OperatorStop {
                    since: 1,
                    revision: 1,
                    actor: layer2_core::SubjectId::new("operator"),
                    reason: "contain".into(),
                },
            ),
            ..Default::default()
        };
        let spend = Arc::new(Spend(std::sync::Mutex::new(Ok(stopped))));
        let auth = auth.with_platform_spend(spend.clone());
        assert!(!current_signup_options(&auth).await.register_available);
        *spend.0.lock().unwrap() = Ok(PlatformAdmissionState::default());
        assert!(current_signup_options(&auth).await.register_available);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn a_registration_closure_hides_account_creation_on_every_read() {
        let requests = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/api/v1/public/config", get(public_config))
            .with_state(Arc::clone(&requests));
        let (auth, server) = provider(app).await;
        let spend = Arc::new(Spend(std::sync::Mutex::new(Ok(warning()))));
        let auth = auth.with_platform_spend(spend.clone());

        assert!(!current_signup_options(&auth).await.register_available);
        for (state, expected) in [
            (Ok(PlatformAdmissionState::default()), true),
            (
                Ok(PlatformAdmissionState {
                    operator_control: layer2_core::budget::OperatorAdmissionState::Unreadable,
                    ..Default::default()
                }),
                false,
            ),
            (Err(BudgetError::Unavailable), true),
            (Ok(warning()), false),
        ] {
            *spend.0.lock().unwrap() = state;
            let options = current_signup_options(&auth).await;
            assert_eq!(options.register_available, expected);
            assert_eq!(options.social_providers, ["github", "google"]);
        }
        // The platform state is read per request; the provider options stay
        // cached and keep NyxID's own value.
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert!(
            auth.signup_options_cache
                .value
                .lock()
                .await
                .as_ref()
                .expect("cache entry")
                .value
                .register_available
        );
        server.abort();
    }

    #[tokio::test]
    async fn anonymous_route_ignores_ambient_auth_store_failure() {
        let requests = Arc::new(AtomicUsize::new(0));
        let provider_app = Router::new()
            .route("/api/v1/public/config", get(public_config))
            .with_state(requests);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake provider");
        let base_url = format!("http://{}", listener.local_addr().expect("local address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, provider_app)
                .await
                .expect("serve fake provider");
        });
        let store = Arc::new(MemoryAuthStore::default());
        store.set_unavailable(true);
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url,
                client_id: "client".into(),
                client_secret: "secret".into(),
                public_url: "http://cma.invalid".into(),
            },
            None,
            store,
        );
        let app = crate::auth::flow::router(Arc::clone(&auth)).layer(
            axum::middleware::from_fn_with_state(auth, crate::auth::attach_subject),
        );

        let response = app
            .oneshot(
                Request::get("/auth/signup-options")
                    .header(header::COOKIE, "cma_session=stale")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL),
            Some(&HeaderValue::from_static("public, max-age=300"))
        );
        let body: serde_json::Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("response body"),
        )
        .expect("response json");
        assert_eq!(body["register_available"], true);
        server.abort();
    }
}
