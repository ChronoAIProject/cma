use super::{
    provider::Provider,
    server::{lose_reply, Server},
    *,
};
use crate::{
    auth::{persistence::*, profile::Profile, AuthState},
    store::{AuthEncryptionKey, MongoAuthStore},
};
use base64::Engine;
use layer2_core::{
    resource_access::{revocations::ResourceRevocationService, ResourceAccessService},
    Authorization, SubjectId,
};
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime},
};

pub(super) struct HttpFixture {
    pub data: Fixture,
    pub provider: Provider,
    pub custody: Arc<MongoAuthStore>,
    key: AuthEncryptionKey,
    pub owner: String,
    pub other: String,
    pub client: reqwest::Client,
}

impl HttpFixture {
    pub async fn new() -> Self {
        let mut fixture = Self::unseeded().await;
        for subject in ["owner", "other-owner"] {
            assert!(fixture
                .custody
                .insert_subject(SubjectAuthState::new(
                    SubjectId::new(subject),
                    Authorization::none(),
                    Profile::default(),
                    Some(DurableSubjectTokens {
                        access_token: SecretValue::new(fixture.provider.register(subject).await),
                        refresh_token: None,
                        expires_at: SystemTime::now() + Duration::from_secs(3600),
                    }),
                ))
                .await
                .unwrap());
        }
        fixture.owner = fixture.cookie("owner", false).await;
        fixture.other = fixture.cookie("other-owner", false).await;
        fixture
    }

    pub async fn unseeded() -> Self {
        let data = Fixture::new().await;
        let mut bytes = [0; 32];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).unwrap();
        let key = AuthEncryptionKey::from_base64(
            &base64::engine::general_purpose::STANDARD.encode(bytes),
        )
        .unwrap();
        let custody = data.auth_store(&key).await;
        let provider = Provider::new().await;
        Self {
            data,
            provider,
            custody,
            key,
            owner: String::new(),
            other: String::new(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        }
    }

    pub async fn cookie(&self, subject: &str, expired: bool) -> String {
        let cookie = uuid::Uuid::new_v4().to_string();
        self.custody
            .put_browser_login(BrowserLogin {
                id: SecretDigest::identify("browser-login", &cookie),
                subject: SubjectId::new(subject),
                expires_at: if expired {
                    SystemTime::UNIX_EPOCH
                } else {
                    SystemTime::now() + Duration::from_secs(600)
                },
            })
            .await
            .unwrap();
        cookie
    }

    pub async fn server(&self, lose_first_reply: bool, joined: bool) -> Server {
        self.configured_server(lose_first_reply, joined, true).await
    }

    pub async fn without_revocations(&self, joined: bool) -> Server {
        self.configured_server(false, joined, false).await
    }

    async fn configured_server(
        &self,
        lose_first_reply: bool,
        joined: bool,
        revocations: bool,
    ) -> Server {
        let auth = AuthState::with_store(
            self.provider.mode(),
            None,
            self.data.auth_store(&self.key).await,
        )
        .with_browser_session_join(joined);
        let access = ResourceAccessService::new(Arc::new(self.data.sandbox.clone()), auth.clone());
        let app = if revocations {
            let service =
                ResourceRevocationService::new(access.clone(), self.data.reopened().await)
                    .await
                    .unwrap();
            layer1_public::rest::resource_access_router_with_revocations(
                Arc::new(access),
                Arc::new(service),
            )
        } else {
            layer1_public::rest::resource_access_router(Arc::new(access))
        };
        let app = crate::rest_router(app)
            .merge(crate::auth::router(auth.clone()))
            .layer(axum::middleware::from_fn_with_state(
                auth,
                crate::auth::attach_subject,
            ))
            .layer(axum::middleware::from_fn_with_state(
                Arc::new(AtomicBool::new(lose_first_reply)),
                lose_reply,
            ));
        Server::start(app).await
    }

    pub fn request(
        &self,
        server: &Server,
        method: reqwest::Method,
        path: &str,
        cookie: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let request = self
            .client
            .request(method, format!("{}{path}", server.base))
            .header(reqwest::header::HOST, "cma.example.test");
        match cookie {
            Some(cookie) => request.header(
                reqwest::header::COOKIE,
                format!("{}={cookie}", crate::auth::AUTH_COOKIE),
            ),
            None => request,
        }
    }

    pub fn create(
        &self,
        server: &Server,
        cookie: Option<&str>,
        key: &str,
    ) -> reqwest::RequestBuilder {
        let layer2_core::resource_access::ResourceSelector::Sandbox { sandbox_id } =
            &self.data.selector
        else {
            panic!()
        };
        self.request(
            server,
            reqwest::Method::POST,
            "/api/v1/resource-revocations",
            cookie,
        )
        .header("Idempotency-Key", key)
        .json(
            &json!({"kind":"sandbox", "id":sandbox_id.as_str(), "share_id":self.data.share.id,
                "expected_revision":self.data.share.revision, "change":{"kind":"revoke"}}),
        )
    }

    pub fn status_path(&self, operation: &str) -> String {
        let layer2_core::resource_access::ResourceSelector::Sandbox { sandbox_id } =
            &self.data.selector
        else {
            panic!()
        };
        format!("/api/v1/resource-revocations/{operation}?kind=sandbox&id={sandbox_id}")
    }

    pub async fn cleanup(mut self) {
        self.provider.server.stop().await;
        self.data.cleanup().await;
    }
}
