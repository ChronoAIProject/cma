//! Shared local-IdP browser fixture for desktop auth and combined router tests.
//! Construction seeds custody only; it never verifies or caches a request proof.
use super::{
    persistence::{
        AuthStore, BrowserLogin, DurableSubjectTokens, SecretDigest, SecretValue, SubjectAuthState,
    },
    profile::Profile,
    AuthMode, AuthState, AUTH_COOKIE, AUTH_COOKIE_DOMAIN,
};
use crate::store::{AuthEncryptionKey, MongoAuthStore};
use axum::http::{header, HeaderMap};
use base64::Engine as _;
use layer2_core::{Authorization, Caller, Capability, SubjectId};
use mongodb::Client;
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, SystemTime},
};

mod provider;
pub(crate) use provider::ProviderSnapshot;

/// Two independent clients/AuthStates share one freshly owned auth database.
/// Keep this value alive while its states are serving requests. `cleanup`
/// closes the local IdP and drops only this fixture's database.
pub(crate) struct DesktopAuthFixture {
    pub(crate) left: Arc<AuthState>,
    pub(crate) right: Arc<AuthState>,
    pub(crate) headers: HeaderMap,
    pub(crate) caller: Caller,
    pub(super) stores: [Arc<MongoAuthStore>; 2],
    db: mongodb::Database,
    login: SecretDigest,
    provider: Arc<provider::Provider>,
    server: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for DesktopAuthFixture {
    fn drop(&mut self) {
        if let Some(server) = &self.server {
            server.abort();
        }
    }
}
impl DesktopAuthFixture {
    pub(crate) async fn new(subject: SubjectId) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let provider = provider::Provider::new(subject.clone(), &origin);
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
        let caller = Caller::new(subject, Authorization::new([Capability::UseOwnSandboxes]));
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
        let app = provider.router();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            left,
            right,
            stores,
            db,
            headers,
            caller,
            login,
            provider,
            server: Some(server),
        }
    }

    /// Production routers must use `left`/`right` and `headers` so their real
    /// middleware performs ingress verification. No captured proof is exported.
    pub(crate) async fn revoke_login(&self) {
        self.stores[1]
            .delete_browser_login(&self.login)
            .await
            .unwrap();
    }

    pub(crate) fn deny_provider(&self, denied: bool) {
        self.provider.denied.store(denied, Ordering::SeqCst);
    }

    /// Cumulative snapshots; read after work quiesces for coherent phase deltas.
    pub(crate) fn provider_snapshot(&self) -> ProviderSnapshot {
        self.provider.snapshot()
    }

    /// Disabled by default. Handler timings exclude CMA work and HTTP transport;
    /// never substitute them for ingress or endpoint acceptance measurements.
    pub(crate) fn set_provider_timing(&self, enabled: bool) {
        self.provider.timing.store(enabled, Ordering::SeqCst);
    }

    pub(super) fn block_userinfo(&self) {
        self.provider.block.store(true, Ordering::SeqCst);
    }

    pub(super) async fn wait_for_userinfo(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.provider.entered.notified())
            .await
            .unwrap();
    }

    pub(super) fn release_userinfo(&self) {
        self.provider.block.store(false, Ordering::SeqCst);
        self.provider.release.notify_one();
    }

    pub(crate) fn database_name(&self) -> &str {
        self.db.name()
    }

    pub(crate) async fn cleanup(mut self) {
        let server = self.server.take().unwrap();
        server.abort();
        let stopped = server.await;
        assert!(stopped.is_ok() || stopped.unwrap_err().is_cancelled());
        self.db.drop().await.unwrap();
    }
}
