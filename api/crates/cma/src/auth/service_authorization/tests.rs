use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use axum::{
    body::{to_bytes, Body},
    http::Request,
    Extension, Router,
};
use layer1_public::AuthenticatedSubject;
use layer2_core::account_deletion::OwnerSessionAuthority;
use tokio::sync::Mutex;
use tower::ServiceExt as _;

use super::*;
use crate::auth::{
    persistence::{AuthStore, MemoryAuthStore},
    RequestAuth,
};

#[path = "catalog_scope.rs"]
mod catalog_scope;

struct SyntheticGrants {
    issuer: Url,
    required: BTreeSet<ServiceId>,
    missing: Mutex<BTreeSet<ServiceId>>,
    challenge: Mutex<String>,
    account: Mutex<Option<bool>>,
    unavailable: AtomicBool,
    redemptions: AtomicUsize,
}

#[async_trait::async_trait]
impl GrantAuthority for SyntheticGrants {
    fn issuer(&self) -> Option<&Url> {
        Some(&self.issuer)
    }

    fn management_url(&self) -> Option<Url> {
        Some(self.issuer.join("/settings/consents").unwrap())
    }

    async fn account(
        &self,
        _auth: &AuthState,
        subject: &SubjectId,
    ) -> Result<Option<VerifiedAccountGrant>, StoreError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("synthetic outage".into()));
        }
        Ok(self
            .account
            .lock()
            .await
            .map(|authorized| VerifiedAccountGrant {
                subject: subject.clone(),
                management_credential_unsupported: false,
                status: if authorized {
                    GrantStatus::Authorized
                } else {
                    GrantStatus::Revoked
                },
                management_url: Some(self.issuer.join("/settings/consents").unwrap()),
            }))
    }

    async fn missing(
        &self,
        _auth: &AuthState,
        intent: &RecoveryIntent,
    ) -> Result<Option<BTreeSet<ServiceId>>, StoreError> {
        assert_eq!(intent.subject.as_str(), "owner");
        assert_eq!(intent.issuer, self.issuer.as_str());
        assert_eq!(intent.required, self.required);
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("synthetic outage".into()));
        }
        Ok(Some(self.missing.lock().await.clone()))
    }

    async fn consent_url(
        &self,
        _auth: &AuthState,
        intent: &RecoveryIntent,
        state: &str,
        s256_challenge: &str,
    ) -> Result<Option<Url>, StoreError> {
        assert_eq!(intent.requested, *self.missing.lock().await);
        assert_eq!(
            intent.redirect_uri,
            "https://app.example/auth/service-authorization/callback"
        );
        *self.challenge.lock().await = s256_challenge.into();
        let scope = intent
            .requested
            .iter()
            .map(ServiceId::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        let mut url = self.issuer.join("/approve").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("state", state),
            ("scope", scope.as_str()),
            ("code_challenge", s256_challenge),
            ("code_challenge_method", "S256"),
        ]);
        Ok(Some(url))
    }

    async fn redeem(
        &self,
        _auth: &AuthState,
        intent: &RecoveryIntent,
        code: &str,
        verifier: &SecretValue,
    ) -> Result<Option<ConsentIdentity>, StoreError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("synthetic outage".into()));
        }
        assert_eq!(
            *self.challenge.lock().await,
            code_challenge(verifier.expose())
        );
        self.redemptions.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        if code == "allow" {
            self.missing.lock().await.clear();
        }
        Ok(Some(ConsentIdentity {
            missing: self.missing.lock().await.clone(),
            subject: if code == "different-account" {
                SubjectId::new("someone-else")
            } else {
                intent.subject.clone()
            },
            issuer: if code == "different-issuer" {
                "https://different.example/".into()
            } else {
                intent.issuer.clone()
            },
        }))
    }
}

fn auth(store: Arc<MemoryAuthStore>) -> Arc<AuthState> {
    AuthState::with_store_and_nyxid_public_url(
        AuthMode::Provider {
            base_url: "https://provider.example".into(),
            client_id: "cma-test".into(),
            client_secret: random_token(),
            public_url: "https://app.example".into(),
        },
        None,
        store,
        Some("https://provider.example".into()),
    )
}

fn app(auth: Arc<AuthState>, source: CredentialSource) -> Router {
    let identity = RequestAuth {
        binding: None,
        credential_source: source,
        subject: SubjectId::new("owner"),
        authorization: AuthState::trusted_authorization(),
        profile: Default::default(),
        delegated_credential: None,
    };
    crate::auth::service_authorization::router(auth.clone())
        .merge(crate::connections::router(
            crate::connections::authorization_fixture(),
            auth.clone(),
        ))
        .layer(Extension(auth.resource_request_context(&identity)))
        .layer(Extension(identity.authorization))
        .layer(Extension(OwnerSessionAuthority::verified_personal(
            identity.subject.clone(),
        )))
        .layer(Extension(AuthenticatedSubject(identity.subject)))
}

struct Fixture {
    store: Arc<MemoryAuthStore>,
    grants: Arc<SyntheticGrants>,
    a: Router,
    b: Router,
    cookie: String,
    other_cookie: String,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_services(&["api-drive", "api-github"], &["api-github"]).await
    }

    async fn with_services(required: &[&str], missing: &[&str]) -> Self {
        let store = Arc::new(MemoryAuthStore::default());
        let grants = Arc::new(SyntheticGrants {
            issuer: Url::parse("https://provider.example/").unwrap(),
            required: required.iter().copied().map(ServiceId::new).collect(),
            missing: Mutex::new(missing.iter().copied().map(ServiceId::new).collect()),
            challenge: Mutex::new(String::new()),
            account: Mutex::new(Some(true)),
            unavailable: AtomicBool::new(false),
            redemptions: AtomicUsize::new(0),
        });
        let replica = || auth(store.clone()).with_fixture_grant_authority(grants.clone());
        let a = replica();
        let b = replica();
        let cookie = a
            .issue_browser_login(SubjectId::new("owner"))
            .await
            .unwrap();
        let other_cookie = b
            .issue_browser_login(SubjectId::new("another-owner"))
            .await
            .unwrap();
        Self {
            a: app(a, CredentialSource::Retained),
            b: app(b, CredentialSource::Retained),
            store,
            grants,
            cookie,
            other_cookie,
        }
    }

    async fn start(&self) -> String {
        let response = start(&self.a, &self.cookie, "https://app.example", "agent", "/").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
        assert_eq!(body["owner"], "owner");
        let url = Url::parse(body["hosted_url"].as_str().unwrap()).unwrap();
        let pairs: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["scope"], "api-github");
        assert_eq!(pairs["code_challenge_method"], "S256");
        pairs["state"].clone()
    }
}

async fn start(app: &Router, cookie: &str, origin: &str, id: &str, return_to: &str) -> Response {
    app.clone()
        .oneshot(
            Request::post("/auth/service-authorization/start")
                .header(header::COOKIE, format!("cma_session={cookie}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, origin)
                .body(Body::from(
                    serde_json::json!({
                        "target": { "type": "agent", "id": id }, "return_to": return_to,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn get(app: &Router, cookie: &str, target: &str) -> Response {
    app.clone()
        .oneshot(
            Request::get(target)
                .header(header::COOKIE, format!("cma_session={cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 8192).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn partial_grant_requests_only_missing_scope_and_redeems_once_across_replicas() {
    let f = Fixture::new().await;
    let state = f.start().await;
    let path =
        format!("{CALLBACK_PATH}?state={state}&code=allow&iss=https%3A%2F%2Fprovider.example");
    let switched = get(&f.b, &f.other_cookie, &path).await;
    assert_eq!(switched.status(), StatusCode::CONFLICT);
    assert!(!body(switched).await.contains("Return to draft"));
    let (first, second) = tokio::join!(get(&f.a, &f.cookie, &path), get(&f.b, &f.cookie, &path));
    let (accepted, duplicate) = if first.status() == StatusCode::OK {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert!(!accepted.headers().contains_key(header::SET_COOKIE));
    assert_eq!(accepted.headers()[header::REFERRER_POLICY], "no-referrer");
    let page = body(accepted).await;
    assert!(page.contains("Application consent confirmed"));
    assert!(
        page.contains("does not update an existing Sandbox key or resolve a pending key request")
    );
    assert!(page.contains("href=\"/auth/login?return_to=%2F\">Sign in again"));
    assert!(page.contains("href=\"/\">Return to draft"));
    assert!(!page.contains("<form"));
    assert_eq!(f.grants.redemptions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_complete_grant_does_not_start_another_consent_or_force_sign_in() {
    let f = Fixture::new().await;
    f.grants.missing.lock().await.clear();
    let response = start(
        &f.a,
        &f.cookie,
        "https://app.example",
        "agent",
        "/agents/original",
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(!body(response).await.contains("hosted_url"));
    assert!(f.grants.challenge.lock().await.is_empty());
    assert_eq!(f.grants.redemptions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn denial_keeps_the_draft_and_wrong_issuer_or_subject_cannot_restore_it() {
    let f = Fixture::new().await;
    let state = f.start().await;
    let response = get(
        &f.b,
        &f.cookie,
        &format!("{CALLBACK_PATH}?state={state}&error=access_denied"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = body(response).await;
    assert!(page.contains("Service authorization cancelled"));
    assert!(page.contains("Return to draft"));
    assert_eq!(f.grants.redemptions.load(Ordering::SeqCst), 0);

    for code in ["different-account", "different-issuer"] {
        let state = f.start().await;
        let response = get(
            &f.b,
            &f.cookie,
            &format!("{CALLBACK_PATH}?state={state}&code={code}"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!body(response).await.contains("Return to draft"));
    }
}

#[tokio::test]
async fn an_authoritative_revocation_blocks_success_on_both_replicas() {
    let f = Fixture::new().await;
    *f.grants.account.lock().await = Some(false);
    for app in [&f.a, &f.b] {
        let response = get(app, &f.cookie, "/auth/service-authorization").await;
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let account = body(response).await;
        assert!(account.contains("\"status\":\"revoked\""));
        assert!(account.contains("https://provider.example/settings/consents"));
        let state = f.start().await;
        let response = get(
            app,
            &f.cookie,
            &format!("{CALLBACK_PATH}?state={state}&code=revoked"),
        )
        .await;
        let page = body(response).await;
        assert!(page.contains("Required services still need authorization"));
        assert!(!page.contains("Service authorization verified"));
    }
}

#[tokio::test]
async fn disabled_unsupported_unavailable_and_absent_are_conservative() {
    let f = Fixture::new().await;
    let disabled = app(auth(f.store.clone()), CredentialSource::Retained);
    let unsupported = app(
        auth(f.store.clone()).with_service_authorization(true, None),
        CredentialSource::Retained,
    );
    assert_eq!(
        get(&disabled, &f.cookie, "/auth/service-authorization")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let state = f.start().await;
    f.grants.unavailable.store(true, Ordering::SeqCst);
    let page = body(
        get(
            &f.b,
            &f.cookie,
            &format!("{CALLBACK_PATH}?state={state}&code=revoked"),
        )
        .await,
    )
    .await;
    assert!(page.contains("could not be verified"));
    for (app, expected_url) in [
        (&unsupported, None),
        (&f.b, Some("https://provider.example/settings/consents")),
    ] {
        let account: serde_json::Value = serde_json::from_str(
            &body(get(app, &f.cookie, "/auth/service-authorization").await).await,
        )
        .unwrap();
        assert_eq!(account["status"], "unknown");
        assert_eq!(account["management_url"].as_str(), expected_url);
        assert_eq!(
            start(app, &f.cookie, "https://app.example", "agent", "/")
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    f.grants.unavailable.store(false, Ordering::SeqCst);
    *f.grants.account.lock().await = None;
    let account: serde_json::Value = serde_json::from_str(
        &body(get(&f.a, &f.cookie, "/auth/service-authorization").await).await,
    )
    .unwrap();
    assert_eq!(account["status"], "unknown");
    assert_eq!(
        account["management_url"],
        "https://provider.example/settings/consents"
    );
}

#[tokio::test]
async fn admission_checks_target_origin_return_and_current_browser() {
    let f = Fixture::new().await;
    for (cookie, origin, agent, path, status) in [
        (
            &f.cookie,
            "https://other.example",
            "agent",
            "/",
            StatusCode::FORBIDDEN,
        ),
        (
            &f.cookie,
            "https://app.example",
            "other-agent",
            "/",
            StatusCode::NOT_FOUND,
        ),
        (
            &f.cookie,
            "https://app.example",
            "agent",
            "//other.example",
            StatusCode::BAD_REQUEST,
        ),
        (
            &f.other_cookie,
            "https://app.example",
            "agent",
            "/",
            StatusCode::FORBIDDEN,
        ),
    ] {
        assert_eq!(
            start(&f.a, cookie, origin, agent, path).await.status(),
            status
        );
    }

    let state = f.start().await;
    let key = SecretDigest::identify(AUTH_COOKIE_DOMAIN, &f.cookie);
    let id = SecretDigest::identify(ATTEMPT_DOMAIN, &state);
    let mut stored = f
        .store
        .take_login_attempt(&id, &key)
        .await
        .unwrap()
        .unwrap();
    stored.expires_at = SystemTime::UNIX_EPOCH;
    f.store.put_login_attempt(stored).await.unwrap();
    let response = get(
        &f.b,
        &f.cookie,
        &format!("{CALLBACK_PATH}?state={state}&code=allow"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let state = f.start().await;
    f.store.delete_browser_login(&key).await.unwrap();
    let response = get(
        &f.b,
        &f.cookie,
        &format!("{CALLBACK_PATH}?state={state}&code=allow"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(f.grants.redemptions.load(Ordering::SeqCst), 0);
}
