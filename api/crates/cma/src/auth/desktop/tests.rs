//! Two independent AuthState/client instances prove shared revocation.
use super::*;
use crate::auth::{
    persistence::{AuthStore, BrowserLogin, DurableSubjectTokens},
    profile::Profile,
};
use crate::store::{AuthEncryptionKey, MongoAuthStore};
use axum::{
    extract::State,
    http::header,
    routing::{get, post},
    Json, Router,
};
use base64::Engine as _;
use layer2_core::Authorization;
use mongodb::Client;
use serde_json::json;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::Notify;

mod boundaries;

#[derive(Default)]
struct Provider {
    denied: AtomicBool,
    block: AtomicBool,
    entered: Notify,
    release: Notify,
}
async fn userinfo(State(provider): State<Arc<Provider>>) -> Json<serde_json::Value> {
    if provider.block.load(Ordering::SeqCst) {
        provider.entered.notify_one();
        provider.release.notified().await;
    }
    Json(json!({"sub":"desktop-owner"}))
}
async fn introspect(State(provider): State<Arc<Provider>>) -> Json<serde_json::Value> {
    Json(
        json!({"active":!provider.denied.load(Ordering::SeqCst),"sub":"desktop-owner","permissions":["cma:sandbox:use"]}),
    )
}

struct Fixture {
    left: Arc<AuthState>,
    right: Arc<AuthState>,
    stores: [Arc<MongoAuthStore>; 2],
    db: mongodb::Database,
    headers: HeaderMap,
    caller: Caller,
    login: SecretDigest,
    provider: Arc<Provider>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let metadata = json!({
            "authorization_endpoint":format!("{origin}/authorize"),
            "token_endpoint":format!("{origin}/token"),
            "userinfo_endpoint":format!("{origin}/userinfo"),
            "introspection_endpoint":format!("{origin}/introspect"),
        });
        let provider = Arc::new(Provider::default());
        let app = Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || {
                    let metadata = metadata.clone();
                    async move { Json(metadata) }
                }),
            )
            .route("/userinfo", get(userinfo))
            .route("/introspect", post(introspect))
            .with_state(provider.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
        let first = Client::with_uri_str(&uri).await.unwrap();
        let second = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("cma_desktop_auth_{}", uuid::Uuid::new_v4().simple());
        let db = first.database(&name);
        let mut bytes = [0; 32];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).unwrap();
        let key = AuthEncryptionKey::from_base64(
            &base64::engine::general_purpose::STANDARD.encode(bytes),
        )
        .unwrap();
        let stores = [
            Arc::new(
                MongoAuthStore::open(&first, &name, "desktop-test", &key)
                    .await
                    .unwrap(),
            ),
            Arc::new(
                MongoAuthStore::open(&second, &name, "desktop-test", &key)
                    .await
                    .unwrap(),
            ),
        ];
        let mode = AuthMode::Provider {
            base_url: origin,
            client_id: "desktop-test".into(),
            client_secret: uuid::Uuid::new_v4().to_string(),
            public_url: "https://cma.example.test".into(),
        };
        let left = AuthState::with_store(mode.clone(), None, stores[0].clone());
        let right = AuthState::with_store(mode, None, stores[1].clone());
        let caller = Caller::new(
            SubjectId::new("desktop-owner"),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        let tokens = DurableSubjectTokens {
            access_token: SecretValue::new(uuid::Uuid::new_v4().to_string()),
            refresh_token: None,
            expires_at: SystemTime::now() + Duration::from_secs(3600),
        };
        stores[0]
            .insert_subject(SubjectAuthState::new(
                caller.subject.clone(),
                caller.authorization.clone(),
                Profile::default(),
                Some(tokens),
            ))
            .await
            .unwrap();
        let cookie = uuid::Uuid::new_v4().to_string();
        let login = SecretDigest::identify(AUTH_COOKIE_DOMAIN, &cookie);
        stores[0]
            .put_browser_login(BrowserLogin {
                id: login.clone(),
                subject: caller.subject.clone(),
                expires_at: SystemTime::now() + Duration::from_secs(3600),
            })
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("{AUTH_COOKIE}={cookie}").parse().unwrap(),
        );
        Self {
            left,
            right,
            stores,
            db,
            headers,
            caller,
            login,
            provider,
            server,
        }
    }
    async fn authority(&self, auth: &Arc<AuthState>) -> Arc<dyn DesktopAuthorization> {
        let resolved = auth.resolve_request(&self.headers).await.unwrap().unwrap();
        capture(auth.clone(), &self.headers, &resolved).unwrap()
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn browser_stream_authority_revokes_across_instances_and_restart() {
    let f = Fixture::new().await;
    let left = f.authority(&f.left).await;
    let right = f.authority(&f.right).await;
    left.current(&f.caller).await.unwrap();
    right.current(&f.caller).await.unwrap();
    f.stores[1].delete_browser_login(&f.login).await.unwrap();
    assert!(matches!(
        left.current(&f.caller).await,
        Err(DesktopError::Unavailable)
    ));
    assert!(matches!(
        right.current(&f.caller).await,
        Err(DesktopError::Unavailable)
    ));
    let restarted = AuthState::with_store(f.left.mode.clone(), None, f.stores[0].clone());
    assert!(restarted
        .resolve_request(&f.headers)
        .await
        .unwrap()
        .is_none());
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn logout_during_provider_verification_cannot_renew_stream_authority() {
    let f = Fixture::new().await;
    let authority = f.authority(&f.left).await;
    authority.current(&f.caller).await.unwrap();
    f.provider.block.store(true, Ordering::SeqCst);
    let caller = f.caller.clone();
    let pending = tokio::spawn(async move { authority.current(&caller).await });
    tokio::time::timeout(Duration::from_secs(3), f.provider.entered.notified())
        .await
        .unwrap();
    f.stores[1].delete_browser_login(&f.login).await.unwrap();
    f.provider.release.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        Err(DesktopError::Unavailable)
    ));
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn replaced_browser_credentials_and_live_bearer_refusal_fail_closed() {
    let f = Fixture::new().await;
    let authority = f.authority(&f.left).await;
    authority.current(&f.caller).await.unwrap();
    let old = f.stores[1]
        .subject(&f.caller.subject)
        .await
        .unwrap()
        .unwrap();
    let mut next = old.clone();
    next.revision += 1;
    next.token_generation += 1;
    next.tokens.as_mut().unwrap().access_token = SecretValue::new(uuid::Uuid::new_v4().to_string());
    assert!(f.stores[1]
        .replace_subject(&old.incarnation, old.revision, next)
        .await
        .unwrap());
    assert!(authority.current(&f.caller).await.is_err());
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", uuid::Uuid::new_v4()).parse().unwrap(),
    );
    let left = f.left.resolve_request(&headers).await.unwrap().unwrap();
    let right = f.right.resolve_request(&headers).await.unwrap().unwrap();
    let left = capture(f.left.clone(), &headers, &left).unwrap();
    let right = capture(f.right.clone(), &headers, &right).unwrap();
    left.current(&f.caller).await.unwrap();
    right.current(&f.caller).await.unwrap();
    f.provider.denied.store(true, Ordering::SeqCst);
    assert!(left.current(&f.caller).await.is_err());
    assert!(right.current(&f.caller).await.is_err());
    f.db.drop().await.unwrap();
}
