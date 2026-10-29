use super::*;
use crate::{
    auth::{Creator, CreatorAuthState, CreatorAuthenticator},
    worker_credentials::tests::{cutoff, Memory},
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};

#[derive(Default)]
pub(crate) struct Manager {
    pub known: AtomicBool,
    pub failed: AtomicBool,
}
impl WorkerClassifier for Manager {
    fn was_worker<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<bool, RecordError>> {
        Box::pin(async move {
            if self.failed.load(Ordering::SeqCst) {
                Err(RecordError::Unavailable("fixture unavailable".into()))
            } else {
                Ok(self.known.load(Ordering::SeqCst))
            }
        })
    }
}

#[derive(Clone)]
struct NyxFixture {
    claims: Arc<Mutex<Value>>,
    calls: Arc<AtomicUsize>,
}

pub(crate) struct OwnerFixture {
    pub auth: CreatorAuthenticator,
    pub history: Arc<Memory>,
    pub manager: Arc<Manager>,
    pub claims: Arc<Mutex<Value>>,
    pub calls: Arc<AtomicUsize>,
    pub headers: HeaderMap,
    pub token: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for OwnerFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl OwnerFixture {
    pub async fn start(subject: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let service = uuid::Uuid::new_v4().to_string();
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let claims = Arc::new(Mutex::new(
            json!({"active":true,"sub":subject,"iss":issuer,"aud":service,
            "token_type":"api_key","iat":now-1,"jti":uuid::Uuid::new_v4().to_string(),"scope":"proxy",
            "permissions":["cma-eg:channels:read"]}),
        ));
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/oauth/introspect",
                post(|State(state): State<NyxFixture>| async move {
                    state.calls.fetch_add(1, Ordering::SeqCst);
                    let claims = state.claims.lock().unwrap().clone();
                    Json(claims)
                }),
            )
            .with_state(NyxFixture {
                claims: claims.clone(),
                calls: calls.clone(),
            });
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let history = Arc::new(Memory::default());
        let mut cutover = cutoff();
        cutover.not_before = now - 20;
        *history.cutoff.lock().unwrap() = Some(cutover.clone());
        let manager = Arc::new(Manager::default());
        let identity = cma_service_auth::NyxIdAuth::new(
            &issuer,
            cma_service_auth::Service::EventGateway,
            "fixture-client",
            "unused".into(),
        )
        .unwrap();
        let verifier = ServiceKeyAuth::new(identity.clone(), &service).unwrap();
        let auth = CreatorAuthenticator::new(&issuer)
            .unwrap()
            .with_service(identity)
            .with_owner_keys(OwnerKeyAuthenticator::new(
                verifier,
                history.clone(),
                manager.clone(),
                cutover.attestation_id,
            ));
        let token = format!("nyx_{}", uuid::Uuid::new_v4());
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        Self {
            auth,
            history,
            manager,
            claims,
            calls,
            headers,
            token,
            task,
        }
    }
}

#[tokio::test]
async fn worker_demotion_revocation_permissions_and_classification_outages_are_rechecked() {
    let fixture = OwnerFixture::start("owner").await;
    fixture.auth.authenticate(&fixture.headers).await.unwrap();
    fixture.manager.known.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture.auth.authenticate(&fixture.headers).await,
        Err(CreatorAuthError::NotAuthenticated)
    );
    fixture.manager.known.store(false, Ordering::SeqCst);
    fixture.manager.failed.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture.auth.authenticate(&fixture.headers).await,
        Err(CreatorAuthError::ProviderUnavailable)
    );
    fixture.manager.failed.store(false, Ordering::SeqCst);
    fixture.history.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        fixture.auth.authenticate(&fixture.headers).await,
        Err(CreatorAuthError::ProviderUnavailable)
    );
    fixture.history.fail.store(false, Ordering::SeqCst);
    fixture.claims.lock().unwrap()["permissions"] = json!([]);
    assert_eq!(
        fixture.auth.authenticate(&fixture.headers).await,
        Err(CreatorAuthError::Forbidden)
    );
    fixture.claims.lock().unwrap()["permissions"] = json!(["cma-eg:channels:read"]);
    fixture.claims.lock().unwrap()["active"] = json!(false);
    assert_eq!(
        fixture.auth.authenticate(&fixture.headers).await,
        Err(CreatorAuthError::NotAuthenticated)
    );
    fixture.claims.lock().unwrap()["active"] = json!(true);
    fixture.auth.authenticate(&fixture.headers).await.unwrap();
    let hash = crate::channels::definition::hash_secret(&fixture.token);
    fixture.history.record(&hash).await.unwrap();
    assert_eq!(
        fixture.auth.authenticate(&fixture.headers).await,
        Err(CreatorAuthError::NotAuthenticated)
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 8);
}

#[tokio::test]
async fn pre_cutover_parent_and_changed_cutover_remain_refused_after_new_child_issue() {
    let fixture = OwnerFixture::start("owner").await;
    let cutoff = fixture
        .history
        .cutoff
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .not_before;
    fixture.claims.lock().unwrap()["iat"] = json!(cutoff);
    // Even an otherwise new credential receives the parent's iat from NyxID.
    assert_eq!(
        fixture.auth.authenticate(&fixture.headers).await,
        Err(CreatorAuthError::NotAuthenticated)
    );
    fixture.claims.lock().unwrap()["iat"] = json!(cutoff + 1);
    fixture.auth.authenticate(&fixture.headers).await.unwrap();
    fixture
        .history
        .cutoff
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .attestation_id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        fixture.auth.authenticate(&fixture.headers).await,
        Err(CreatorAuthError::NotAuthenticated)
    );
}

#[tokio::test]
async fn owner_method_permissions_and_no_organization_override_are_enforced_by_extractor() {
    #[derive(Clone)]
    struct StateAuth(CreatorAuthenticator);
    impl CreatorAuthState for StateAuth {
        fn creator_auth(&self) -> &CreatorAuthenticator {
            &self.0
        }
    }
    let fixture = OwnerFixture::start("owner").await;
    async fn show(creator: Creator) -> Json<Value> {
        Json(json!({"subject":creator.subject}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/channels", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/v1/channels", get(show).delete(show))
        .with_state(StateAuth(fixture.auth.clone()));
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    assert_eq!(
        http.get(&url)
            .headers(fixture.headers.clone())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        http.delete(&url)
            .headers(fixture.headers.clone())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        http.get(&url)
            .headers(fixture.headers.clone())
            .header("x-cmaeg-owner", "other-org")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    fixture.claims.lock().unwrap()["permissions"] = json!(["cma-eg:admin"]);
    assert_eq!(
        http.delete(&url)
            .headers(fixture.headers.clone())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    task.abort();
}

#[tokio::test]
async fn disabled_or_rejected_key_never_falls_through_to_browser_or_cached_identity() {
    let fixture = OwnerFixture::start("owner").await;
    let disabled = CreatorAuthenticator::new("http://127.0.0.1:9").unwrap();
    assert_eq!(
        disabled.authenticate(&fixture.headers).await,
        Err(CreatorAuthError::NotAuthenticated)
    );
    assert_eq!(
        disabled.authenticate_token(&fixture.token, None).await,
        Err(CreatorAuthError::NotAuthenticated)
    );
    let mut duplicated = fixture.headers.clone();
    duplicated.append("authorization", "Bearer different".parse().unwrap());
    assert_eq!(
        fixture.auth.authenticate(&duplicated).await,
        Err(CreatorAuthError::NotAuthenticated)
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
}
