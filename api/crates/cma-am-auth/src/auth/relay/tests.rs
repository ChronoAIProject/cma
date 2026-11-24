use super::*;
use crate::auth::{profile::Profile, ProviderMetadata};
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use layer2_core::{Authorization, SubjectId};
use serde_json::json;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::Notify;

#[derive(Default)]
struct Provider {
    block: AtomicBool,
    denied: AtomicBool,
    entered: Notify,
    release: Notify,
}
async fn userinfo(State(state): State<Arc<Provider>>) -> Json<serde_json::Value> {
    if state.block.load(Ordering::SeqCst) {
        state.entered.notify_one();
        state.release.notified().await;
    }
    Json(json!({"sub":"owner"}))
}
async fn introspect(State(state): State<Arc<Provider>>) -> Json<serde_json::Value> {
    Json(
        json!({"sub":"owner", "active":!state.denied.load(Ordering::SeqCst), "permissions":["cma:sandbox:use"]}),
    )
}
pub struct Fixture {
    pub auth: Arc<AuthState>,
    pub store: Arc<MemoryAuthStore>,
    #[cfg(test)]
    provider: Arc<Provider>,
    pub headers: HeaderMap,
    pub context: CallerContext,
    #[cfg(test)]
    login: SecretDigest,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    pub async fn new(browser: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let provider = Arc::new(Provider::default());
        let router = Router::new()
            .route("/userinfo", get(userinfo))
            .route("/introspect", post(introspect))
            .with_state(provider.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let store = Arc::new(MemoryAuthStore::default());
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: origin.clone(),
                client_id: "test".into(),
                client_secret: "test".into(),
                public_url: "http://localhost".into(),
            },
            None,
            store.clone(),
        );
        *auth.metadata.write().await = Some(ProviderMetadata {
            authorization_endpoint: format!("{origin}/authorize"),
            token_endpoint: format!("{origin}/token"),
            userinfo_endpoint: Some(format!("{origin}/userinfo")),
            introspection_endpoint: Some(format!("{origin}/introspect")),
        });
        let login = SecretDigest::identify(super::super::AUTH_COOKIE_DOMAIN, "test-login");
        store
            .put_browser_login(BrowserLogin {
                id: login.clone(),
                subject: SubjectId::new("owner"),
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            })
            .await
            .unwrap();
        store
            .put_subject(SubjectAuthState::new(
                SubjectId::new("owner"),
                Authorization::none(),
                Profile::default(),
                Some(DurableSubjectTokens {
                    access_token: SecretValue::new("test-access".into()),
                    refresh_token: None,
                    expires_at: SystemTime::now() + Duration::from_secs(3600),
                }),
            ))
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        if browser {
            headers.insert("cookie", "cma_session=test-login".parse().unwrap());
        } else {
            headers.insert("authorization", "Bearer test-access".parse().unwrap());
        }
        let resolved = auth.resolve_request(&headers).await.unwrap().unwrap();
        assert_eq!(
            resolved.delegated_credential.as_deref(),
            Some("test-access")
        );
        let context = auth.resource_request_context(&resolved);
        Self {
            auth,
            store,
            #[cfg(test)]
            provider,
            headers,
            context,
            #[cfg(test)]
            login,
            server,
        }
    }
    #[cfg(test)]
    async fn verify(&self) -> Result<RelayCaller, StatusCode> {
        RelayCaller::verify(
            self.auth.clone(),
            &self.headers,
            &self.context,
            Some("test-access"),
        )
        .await
    }
}

#[tokio::test]
async fn relay_browser_logout_during_provider_verification_is_refused() {
    let f = Fixture::new(true).await;
    f.provider.block.store(true, Ordering::SeqCst);
    let result = tokio::join!(f.verify(), async {
        f.provider.entered.notified().await;
        f.store.delete_browser_login(&f.login).await.unwrap();
        f.provider.release.notify_one();
    });
    assert!(matches!(result.0, Err(StatusCode::UNAUTHORIZED)));
}

#[tokio::test]
async fn relay_policy_io_cannot_adopt_replaced_browser_credential() {
    let f = Fixture::new(true).await;
    let proof = f.verify().await.unwrap();
    f.store
        .put_subject(SubjectAuthState::new(
            SubjectId::new("owner"),
            Authorization::none(),
            Profile::default(),
            Some(DurableSubjectTokens {
                access_token: SecretValue::new("replacement".into()),
                refresh_token: None,
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            }),
        ))
        .await
        .unwrap();
    assert_eq!(proof.current().await, Err(StatusCode::UNAUTHORIZED));
    assert!(matches!(f.verify().await, Err(StatusCode::UNAUTHORIZED)));
}

#[tokio::test]
async fn relay_bearer_rechecks_provider_and_rejects_revocation_without_cached_success() {
    let f = Fixture::new(false).await;
    assert!(f.verify().await.is_ok());
    f.provider.denied.store(true, Ordering::SeqCst);
    assert!(f.verify().await.is_err());
}
