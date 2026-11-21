use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use layer2_core::{Authorization, StoreError};
use serde_json::json;

use super::*;
use crate::auth::persistence::{
    AuthStore, BrowserLogin, DurableSubjectTokens, LoginAttempt, MemoryAuthStore, SecretDigest,
    SecretValue, SubjectAuthState,
};
use crate::auth::{AuthMode, ProviderMetadata};

fn owner() -> SubjectId {
    SubjectId::new("prerequisite-owner")
}

fn authority(refresh: Option<&str>, lifetime: u64) -> SubjectAuthState {
    SubjectAuthState::new(
        owner(),
        Authorization::none(),
        Default::default(),
        Some(DurableSubjectTokens {
            access_token: SecretValue::new("fixture-access".into()),
            refresh_token: refresh.map(|value| SecretValue::new(value.into())),
            expires_at: SystemTime::now() + Duration::from_secs(lifetime),
        }),
    )
}

fn provider(store: Arc<dyn AuthStore>, base_url: &str) -> Arc<AuthState> {
    AuthState::with_store(
        AuthMode::Provider {
            base_url: base_url.into(),
            client_id: "fixture-client".into(),
            client_secret: "fixture-client-value".into(),
            public_url: "https://cma.example.test".into(),
        },
        None,
        store,
    )
}

struct Upstream(tokio::task::JoinHandle<()>);

impl Drop for Upstream {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn upstream(
    store: Arc<dyn AuthStore>,
    status: StatusCode,
    response: serde_json::Value,
) -> (Arc<AuthState>, Upstream) {
    let app = Router::new()
        .route(
            "/token",
            post(move || async move { (status, Json(response)) }),
        )
        .route(
            "/userinfo",
            get(|| async { Json(json!({"sub": owner().as_str(), "permissions": []})) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let auth = provider(store, &base);
    *auth.metadata.write().await = Some(ProviderMetadata {
        authorization_endpoint: format!("{base}/authorize"),
        token_endpoint: format!("{base}/token"),
        userinfo_endpoint: Some(format!("{base}/userinfo")),
        introspection_endpoint: None,
    });
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (auth, Upstream(task))
}

#[tokio::test]
async fn missing_subject_or_nonrenewable_tokens_require_setup() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = provider(store.clone(), "http://127.0.0.1:1");
    assert_eq!(
        auth.check(&owner()).await,
        Err(SandboxCredentialPrerequisiteError::SetupRequired)
    );
    for refresh in [None, Some(""), Some(" ")] {
        store.put_subject(authority(refresh, 3600)).await.unwrap();
        assert_eq!(
            auth.check(&owner()).await,
            Err(SandboxCredentialPrerequisiteError::SetupRequired)
        );
    }
    let mut state = authority(Some("fixture-refresh"), 3600);
    state.tokens = None;
    store.put_subject(state).await.unwrap();
    assert_eq!(
        auth.check(&owner()).await,
        Err(SandboxCredentialPrerequisiteError::SetupRequired)
    );
}

#[tokio::test]
async fn durable_renewable_authority_is_exact_owner_scoped() {
    let store = Arc::new(MemoryAuthStore::default());
    store
        .put_subject(authority(Some("fixture-refresh"), 3600))
        .await
        .unwrap();
    let auth = provider(store, "http://127.0.0.1:1");
    assert_eq!(auth.check(&owner()).await, Ok(()));
    assert_eq!(
        auth.check(&SubjectId::new("another-owner")).await,
        Err(SandboxCredentialPrerequisiteError::SetupRequired)
    );
}

#[tokio::test]
async fn supplied_bearer_authenticates_without_creating_durable_authority() {
    let store = Arc::new(MemoryAuthStore::default());
    let (auth, _upstream) = upstream(store.clone(), StatusCode::OK, json!({})).await;
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer fixture-request"),
    );
    let caller = auth.resolve_request(&headers).await.unwrap().unwrap();
    assert_eq!(caller.subject, owner());
    assert_eq!(
        caller.delegated_credential.as_deref(),
        Some("fixture-request")
    );
    assert_eq!(
        auth.check(&caller.subject).await,
        Err(SandboxCredentialPrerequisiteError::SetupRequired)
    );
    assert!(store.subject(&owner()).await.unwrap().is_none());
}

#[tokio::test]
async fn expired_access_refreshes_and_keeps_durable_renewal_authority() {
    let store = Arc::new(MemoryAuthStore::default());
    store
        .put_subject(authority(Some("fixture-refresh"), 0))
        .await
        .unwrap();
    let (auth, _upstream) = upstream(
        store.clone(),
        StatusCode::OK,
        json!({
            "access_token": "fixture-renewed", "expires_in": 3600
        }),
    )
    .await;
    assert_eq!(auth.check(&owner()).await, Ok(()));
    let tokens = store
        .subject(&owner())
        .await
        .unwrap()
        .unwrap()
        .tokens
        .unwrap();
    assert_eq!(tokens.access_token.expose(), "fixture-renewed");
    assert_eq!(tokens.refresh_token.unwrap().expose(), "fixture-refresh");
}

#[tokio::test]
async fn rejected_refresh_requires_setup_without_returning_provider_details() {
    let store = Arc::new(MemoryAuthStore::default());
    store
        .put_subject(authority(Some("fixture-refresh"), 0))
        .await
        .unwrap();
    let (auth, _upstream) = upstream(
        store.clone(),
        StatusCode::BAD_REQUEST,
        json!({
            "error": "invalid_grant", "error_description": "private-provider-description"
        }),
    )
    .await;
    assert_eq!(
        auth.check(&owner()).await,
        Err(SandboxCredentialPrerequisiteError::SetupRequired)
    );
    assert!(store
        .subject(&owner())
        .await
        .unwrap()
        .unwrap()
        .tokens
        .is_none());
}

#[tokio::test]
async fn dependency_failures_do_not_discard_authority_or_request_new_setup() {
    for (status, body) in [
        (
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error": "invalid_grant"}),
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": "invalid_grant"}),
        ),
        (StatusCode::BAD_REQUEST, json!({"error": "invalid_client"})),
        (
            StatusCode::BAD_REQUEST,
            json!({"private": "provider-error"}),
        ),
        (
            StatusCode::OK,
            json!({"access_token": "", "expires_in": 3600}),
        ),
        (
            StatusCode::OK,
            json!({"access_token": "fixture-renewed", "refresh_token": " ", "expires_in": 3600}),
        ),
        (
            StatusCode::OK,
            json!({"access_token": "fixture-renewed", "expires_in": 0}),
        ),
        (
            StatusCode::OK,
            json!({"access_token": "fixture-renewed", "expires_in": u64::MAX}),
        ),
        (StatusCode::OK, json!({"access_token": 42})),
    ] {
        let store = Arc::new(MemoryAuthStore::default());
        store
            .put_subject(authority(Some("fixture-refresh"), 0))
            .await
            .unwrap();
        let (auth, _upstream) = upstream(store.clone(), status, body).await;
        assert_eq!(auth.check(&owner()).await, Err(unavailable()));
        assert!(store
            .subject(&owner())
            .await
            .unwrap()
            .unwrap()
            .tokens
            .is_some());
    }
}

#[tokio::test]
async fn unavailable_store_and_trusted_mode_do_not_claim_renewable_authority() {
    let store = Arc::new(MemoryAuthStore::default());
    store.set_unavailable(true);
    assert_eq!(
        provider(store, "http://127.0.0.1:1").check(&owner()).await,
        Err(unavailable())
    );
    let trusted = AuthState::in_memory(
        AuthMode::Trusted {
            subject: owner().to_string(),
        },
        None,
    );
    assert_eq!(trusted.check(&owner()).await, Err(unavailable()));
}

struct WrongSubjectStore;

#[async_trait::async_trait]
impl AuthStore for WrongSubjectStore {
    async fn insert_api_bearer(
        &self,
        _custody: crate::auth::persistence::ApiBearerCustody,
    ) -> Result<bool, StoreError> {
        unreachable!("sandbox prerequisites do not retain API credentials")
    }

    async fn api_bearer(
        &self,
        _id: &crate::auth::persistence::SecretDigest,
    ) -> Result<Option<crate::auth::persistence::ApiBearerCustody>, StoreError> {
        unreachable!("sandbox prerequisites do not use API command custody")
    }
    async fn subject(&self, _subject: &SubjectId) -> Result<Option<SubjectAuthState>, StoreError> {
        let mut state = authority(Some("fixture-refresh"), 3600);
        state.subject = SubjectId::new("wrong-stored-owner");
        Ok(Some(state))
    }
    async fn insert_subject(&self, _state: SubjectAuthState) -> Result<bool, StoreError> {
        panic!("must not mutate wrong-owner authority")
    }
    async fn replace_unfenced_subject(&self, _state: SubjectAuthState) -> Result<bool, StoreError> {
        panic!("must not mutate wrong-owner authority")
    }
    async fn replace_subject(
        &self,
        _incarnation: &str,
        _revision: u64,
        _state: SubjectAuthState,
    ) -> Result<bool, StoreError> {
        panic!("must not mutate wrong-owner authority")
    }
    async fn put_login_attempt(&self, _attempt: LoginAttempt) -> Result<(), StoreError> {
        unreachable!()
    }
    async fn take_login_attempt(
        &self,
        _id: &SecretDigest,
        _binding: &SecretDigest,
    ) -> Result<Option<LoginAttempt>, StoreError> {
        unreachable!()
    }
    async fn put_browser_login(&self, _login: BrowserLogin) -> Result<(), StoreError> {
        unreachable!()
    }
    async fn browser_login(&self, _id: &SecretDigest) -> Result<Option<BrowserLogin>, StoreError> {
        unreachable!()
    }
    async fn browser_session(
        &self,
        _id: &SecretDigest,
    ) -> Result<Option<(BrowserLogin, Option<SubjectAuthState>)>, StoreError> {
        unreachable!("sandbox prerequisites do not read browser sessions")
    }
    async fn delete_browser_login(&self, _id: &SecretDigest) -> Result<(), StoreError> {
        unreachable!()
    }
}

#[tokio::test]
async fn crossed_subject_state_is_unavailable_and_never_refreshed() {
    let auth = provider(Arc::new(WrongSubjectStore), "http://127.0.0.1:1");
    assert_eq!(auth.check(&owner()).await, Err(unavailable()));
}
