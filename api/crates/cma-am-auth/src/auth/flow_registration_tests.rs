use std::{collections::HashMap, sync::Arc};

use axum::{
    body::{to_bytes, Body},
    extract::Form,
    http::{header, Request, StatusCode},
    routing::post,
};
use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityQuery, ActivityRecord, ActivityService, ActivityStore,
    ActivityTarget, Capability, DirectoryQuery, Page, PublicIdentity, RegisteredUser, StoreError,
    SubjectId, UserDirectory, UserDirectoryStore,
};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tower::ServiceExt as _;

use super::*;
use crate::auth::{
    persistence::{AuthStore as _, MemoryAuthStore},
    ProviderMetadata, SignInRecords,
};

#[tokio::test]
async fn registration_wraps_and_persists_the_complete_authorize_attempt() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = AuthState::with_store_and_nyxid_public_url(
        AuthMode::Provider {
            base_url: "http://provider.invalid".into(),
            client_id: "client-a".into(),
            client_secret: "secret".into(),
            public_url: "https://cma.example".into(),
        },
        None,
        store.clone(),
        Some("https://nyx.example".into()),
    );
    *auth.metadata.write().await = Some(ProviderMetadata {
        authorization_endpoint: "https://nyx-api.example/oauth/authorize".into(),
        token_endpoint: "https://nyx-api.example/oauth/token".into(),
        userinfo_endpoint: None,
        introspection_endpoint: None,
    });

    let response = begin_login(
        auth,
        Some("/t/pub_123/plc_456".into()),
        Some(LoginScreen::Register),
        false,
    )
    .await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let registration =
        reqwest::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    assert_eq!(
        registration.as_str().split('?').next(),
        Some("https://nyx.example/register")
    );
    let authorize = registration
        .query_pairs()
        .find_map(|(key, value)| (key == "return_to").then(|| value.into_owned()))
        .and_then(|value| reqwest::Url::parse(&value).ok())
        .expect("registration keeps the authorize URL");
    let query = authorize
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<HashMap<_, _>>();
    assert_eq!(query.get("client_id").map(String::as_str), Some("client-a"));
    assert_eq!(
        query.get("redirect_uri").map(String::as_str),
        Some("https://cma.example/auth/callback")
    );
    assert_eq!(
        query.get("scope").map(String::as_str),
        Some("openid profile email roles proxy offline_access")
    );

    let state = query.get("state").expect("authorize state");
    let binding = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(BINDING_COOKIE_PREFIX))
        .and_then(|value| value.split(';').next())
        .and_then(|value| value.split_once('='))
        .map(|(_, value)| value)
        .expect("browser binding cookie");
    let attempt = store
        .take_login_attempt(
            &SecretDigest::identify(ATTEMPT_DOMAIN, state),
            &SecretDigest::identify(BINDING_DOMAIN, binding),
        )
        .await
        .unwrap()
        .expect("persisted login attempt");
    assert_eq!(attempt.return_to.as_deref(), Some("/t/pub_123/plc_456"));
    assert_eq!(
        query.get("code_challenge").map(String::as_str),
        Some(code_challenge(attempt.verifier.expose()).as_str())
    );
}

#[test]
fn return_targets_are_only_canonical_cma_workflows() {
    for valid in [
        "/",
        "/agent-manager",
        "/event-gateway",
        "/image-manager",
        "/t/pub_123",
        "/t/pub_123/plc_456",
        "/triggers/manage",
        "/triggers/manage/trg_123",
        "/triggers/manage/trg_123/embed",
        "/agents/agt_123",
        "/sandboxes/sbx_123",
        "/bots",
        "/bots/new",
        "/bots/bot_123",
        "/profiles",
        "/profiles/agent/agp_123",
        "/profiles/workspace/wkp_123",
        "/profiles/sandbox/sbp_123",
        "/marketplace",
        "/marketplace/agent/agp_123",
    ] {
        assert_eq!(app_return_target(valid).as_deref(), Ok(valid));
    }
    for invalid in [
        "",
        "//example.com/t/pub_123",
        "https://example.com/t/pub_123",
        "/t",
        "/t/",
        "/t/pub_123/placement/extra",
        "/t/pub_123?next=https://example.com",
        "/t/pub_123%2Fextra",
        "/triggers",
        "/agents/..",
        "/bots/..",
        "/bots/bot_123/extra",
        "/bots?next=https://example.com",
        "/profiles/unknown/prf_123",
        "/profiles/agent/prf_123/edit",
        "/sessions/old",
        "/launches/old",
    ] {
        assert!(app_return_target(invalid).is_err(), "accepted {invalid}");
    }
}

#[tokio::test]
async fn login_rejects_unknown_or_duplicate_screens() {
    for query in ["screen=", "screen=login", "screen=register&screen=register"] {
        let app = router(AuthState::in_memory(
            AuthMode::Trusted {
                subject: "developer".into(),
            },
            None,
        ));
        let response = app
            .oneshot(
                Request::get(format!("/auth/login?{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
    }
}

#[tokio::test]
async fn trusted_login_restores_a_valid_relative_target() {
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "developer".into(),
        },
        None,
    );
    let response = login(
        State(auth),
        Query(LoginParams {
            return_to: Some("/marketplace".into()),
            screen: None,
            prompt: None,
        }),
        OriginalUri("/auth/login".parse().unwrap()),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/marketplace");
}

#[derive(Default)]
struct SignInObservations {
    users: Mutex<Vec<PublicIdentity>>,
    activity: Mutex<Vec<ActivityRecord>>,
}

#[async_trait::async_trait]
impl UserDirectoryStore for SignInObservations {
    async fn observe(&self, identity: &PublicIdentity) -> Result<(), StoreError> {
        self.users.lock().await.push(identity.clone());
        Ok(())
    }

    async fn user(&self, _: &SubjectId) -> Result<Option<RegisteredUser>, StoreError> {
        panic!("sign-in does not query the user directory")
    }

    async fn list(&self, _: &DirectoryQuery) -> Result<Page<RegisteredUser>, StoreError> {
        panic!("sign-in does not list the user directory")
    }
}

#[async_trait::async_trait]
impl ActivityStore for SignInObservations {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.activity.lock().await.push(record.clone());
        Ok(())
    }

    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        panic!("sign-in does not list activity")
    }
}

struct SignInProvider(tokio::task::JoinHandle<()>);

impl Drop for SignInProvider {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn unseen_identity_callback_records_sign_in_without_aggregate_bootstrap() {
    for return_to in [
        None,
        Some("/"),
        Some("/agent-manager"),
        Some("/docs"),
        Some("/docs/event-gateway"),
        Some("/usage"),
        Some("/t/public-contract/placement-contract"),
    ] {
        let exchanges = Arc::new(Mutex::new(Vec::new()));
        let captured = exchanges.clone();
        let upstream = Router::new()
            .route(
                "/token",
                post(move |Form(form): Form<HashMap<String, String>>| {
                    let captured = captured.clone();
                    async move {
                        captured.lock().await.push(form);
                        Json(json!({
                            "access_token": "contract-durable-access",
                            "refresh_token": "contract-durable-refresh",
                            "expires_in": 3600,
                        }))
                    }
                }),
            )
            .route(
                "/userinfo",
                get(|headers: HeaderMap| async move {
                    assert_eq!(
                        headers[header::AUTHORIZATION],
                        "Bearer contract-durable-access"
                    );
                    Json(json!({
                        "sub": "unseen-cma-identity", "name": "Contract Visitor",
                        "email": "visitor@example.test",
                        "picture": "https://identity.example.test/avatar.png",
                        "permissions": ["cma:sandbox:use", "cma:trigger:launch"],
                    }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let _provider = SignInProvider(tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        }));
        let store = Arc::new(MemoryAuthStore::default());
        let observed = Arc::new(SignInObservations::default());
        // These are the entire callback write ports: no aggregate or profile services.
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: base_url.clone(),
                client_id: "contract-client".into(),
                client_secret: "contract-value".into(),
                public_url: "https://cma.example.test".into(),
            },
            Some(SignInRecords {
                users: UserDirectory::new(observed.clone()),
                activity: ActivityService::new(observed.clone()),
            }),
            store.clone(),
        );
        *auth.metadata.write().await = Some(ProviderMetadata {
            authorization_endpoint: format!("{base_url}/authorize"),
            token_endpoint: format!("{base_url}/token"),
            userinfo_endpoint: Some(format!("{base_url}/userinfo")),
            introspection_endpoint: None,
        });
        let app = router(auth.clone()).layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::attach_subject,
        ));
        let subject = SubjectId::new("unseen-cma-identity");
        assert!(store.subject(&subject).await.unwrap().is_none());
        let login_path = return_to.map_or_else(
            || "/auth/login".to_string(),
            |target| format!("/auth/login?return_to={}", urlencode(target)),
        );
        let login = app
            .clone()
            .oneshot(Request::get(login_path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::SEE_OTHER);
        assert!(store.subject(&subject).await.unwrap().is_none());
        assert!(observed.users.lock().await.is_empty());
        assert!(observed.activity.lock().await.is_empty());
        let authorize =
            reqwest::Url::parse(login.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let query: HashMap<_, _> = authorize.query_pairs().into_owned().collect();
        let binding = login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let callback_path = format!(
            "/auth/callback?code=contract-code&state={}",
            urlencode(&query["state"])
        );
        // One browser returns having signed out of CMA earlier; choosing to
        // sign in again keeps the automatic-sign-in opt-out.
        let signed_out = return_to == Some("/");
        let callback_cookie = if signed_out {
            format!("{binding}; cma_signed_out=1")
        } else {
            binding.to_owned()
        };
        let callback_request = || {
            Request::get(&callback_path)
                .header(header::COOKIE, &callback_cookie)
                .body(Body::empty())
                .unwrap()
        };
        let response = app.clone().oneshot(callback_request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            return_to.unwrap_or("/")
        );
        let exchanges = exchanges.lock().await;
        assert_eq!(exchanges.len(), 1);
        assert_eq!(exchanges[0]["grant_type"], "authorization_code");
        assert_eq!(exchanges[0]["code"], "contract-code");
        assert_eq!(
            exchanges[0]["redirect_uri"],
            "https://cma.example.test/auth/callback"
        );
        assert_eq!(
            code_challenge(&exchanges[0]["code_verifier"]),
            query["code_challenge"]
        );
        drop(exchanges);
        let signed_in = store.subject(&subject).await.unwrap().unwrap();
        assert_eq!(signed_in.subject, subject);
        assert_eq!((signed_in.revision, signed_in.token_generation), (1, 1));
        assert!(signed_in.authorization.allows(Capability::LaunchTriggers));
        assert!(!signed_in.authorization.allows(Capability::CreateSandbox));
        assert_eq!(
            signed_in.profile.email.as_deref(),
            Some("visitor@example.test")
        );
        let tokens = signed_in.tokens.unwrap();
        assert_eq!(tokens.access_token.expose(), "contract-durable-access");
        assert_eq!(
            tokens.refresh_token.unwrap().expose(),
            "contract-durable-refresh"
        );
        let cookies: Vec<_> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect();
        assert_eq!(cookies.len(), 3);
        assert!(cookies
            .iter()
            .all(|value| !value.starts_with("cma_signed_out=")));
        assert!(cookies[1].starts_with("cma_signed_in="));
        assert!(cookies[2].starts_with(&format!("{}=;", binding_cookie_name(&query["state"]))));
        assert!(cookies[2].contains("Max-Age=0"));
        let browser_cookie = cookies[0].split(';').next().unwrap();
        let (name, value) = browser_cookie.split_once('=').unwrap();
        assert_eq!(name, AUTH_COOKIE);
        let browser = store
            .browser_login(&SecretDigest::identify("browser-login", value))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(browser.subject, subject);
        assert!(browser.expires_at > SystemTime::now());
        for _ in 0..2 {
            let current = app
                .clone()
                .oneshot(
                    Request::get("/auth/session")
                        .header(header::COOKIE, browser_cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(current.status(), StatusCode::OK);
            let body: Value =
                serde_json::from_slice(&to_bytes(current.into_body(), 4096).await.unwrap())
                    .unwrap();
            assert_eq!(body["authenticated"], true);
            assert_eq!(body["subject"], subject.as_str());
            assert_eq!(body["visitor"], true);
        }
        let replay = app.oneshot(callback_request()).await.unwrap();
        assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            *observed.users.lock().await,
            vec![PublicIdentity {
                subject: subject.clone(),
                display_name: Some("Contract Visitor".into()),
                avatar_url: Some("https://identity.example.test/avatar.png".into()),
            }]
        );
        let activity = observed.activity.lock().await;
        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].action, ActivityAction::Authenticate);
        assert_eq!(activity[0].outcome, ActivityOutcome::Succeeded);
        assert_eq!(activity[0].actor.as_ref(), Some(&subject));
        assert_eq!(activity[0].target, Some(ActivityTarget::Subject(subject)));
    }
}

#[path = "flow_registration_tests/platform_closure.rs"]
mod platform_closure;
