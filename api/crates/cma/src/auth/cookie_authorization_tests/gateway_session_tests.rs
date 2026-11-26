use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
    middleware::{self, Next},
};
use tower::ServiceExt as _;

use crate::auth::{flow::BINDING_COOKIE_PREFIX, AUTH_COOKIE};

const AM_SESSION: &str = "/auth/session";
const PORTAL_SESSION: &str = "/api/event-gateway/auth/session";
const SUBJECT: &str = "owner";
const AM_PERMISSION: &str = "cma:sandbox:use";
const GATEWAY_PERMISSION: &str = "cma-eg:channels:read";

#[derive(Clone, Debug, PartialEq, Eq)]
struct ForwardedRequest {
    method: Method,
    session_path: bool,
    expected_bearer: bool,
    cookie_present: bool,
}

struct GatewayFixture {
    origin: String,
    requests: Arc<RwLock<Vec<ForwardedRequest>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for GatewayFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl GatewayFixture {
    async fn new(issuer: &str, access: &str, client_secret: String) -> Self {
        let gateway_auth = cma_service_auth::NyxIdAuth::new(
            issuer,
            cma_service_auth::Service::EventGateway,
            "gateway-session-fixture",
            client_secret,
        )
        .unwrap();
        let bearer = format!("Bearer {access}");
        let requests = Arc::new(RwLock::new(Vec::new()));
        let observed = requests.clone();
        let router = cma_service_auth::session::router(gateway_auth).layer(middleware::from_fn(
            move |request: Request<Body>, next: Next| {
                let bearer_matches = request
                    .headers()
                    .get_all(header::AUTHORIZATION)
                    .iter()
                    .count()
                    == 1
                    && request
                        .headers()
                        .get(header::AUTHORIZATION)
                        .is_some_and(|value| value == bearer.as_str());
                let observation = ForwardedRequest {
                    method: request.method().clone(),
                    session_path: request
                        .uri()
                        .path_and_query()
                        .is_some_and(|value| value.as_str() == AM_SESSION),
                    expected_bearer: bearer_matches,
                    cookie_present: request.headers().contains_key(header::COOKIE),
                };
                let observed = observed.clone();
                async move {
                    observed.write().await.push(observation);
                    next.run(request).await
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            origin,
            requests,
            server,
        }
    }
}

async fn get_browser(app: &Router, path: &str, cookie: Option<&str>) -> Response {
    let mut request = Request::get(path).header("sec-fetch-site", "same-origin");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    tokio::time::timeout(
        Duration::from_secs(10),
        app.clone().oneshot(request.body(Body::empty()).unwrap()),
    )
    .await
    .expect("the local session fixture should respond")
    .unwrap()
}

fn response_cookie(response: &Response, prefix: &str) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .find(|value| value.starts_with(prefix))
        .expect("the login flow should issue a browser cookie")
        .to_owned()
}

async fn sign_in(app: &Router) -> String {
    let start = get_browser(app, "/auth/login", None).await;
    assert_eq!(start.status(), StatusCode::SEE_OTHER);
    let binding = response_cookie(&start, BINDING_COOKIE_PREFIX);
    let location = url::Url::parse(start.headers()[header::LOCATION].to_str().unwrap())
        .expect("the login flow should redirect to the synthetic provider");
    let state = location
        .query_pairs()
        .find(|(name, _)| name == "state")
        .unwrap()
        .1;
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("state", &state)
        .append_pair("code", "fixture-code")
        .finish();
    let callback = get_browser(app, &format!("/auth/callback?{query}"), Some(&binding)).await;
    assert_eq!(callback.status(), StatusCode::SEE_OTHER);
    response_cookie(&callback, &format!("{AUTH_COOKIE}="))
}

async fn read_session(
    app: &Router,
    path: &str,
    cookie: &str,
    secrets: &[&str],
) -> (StatusCode, Value) {
    let response = get_browser(app, path, Some(cookie)).await;
    let leaks = |bytes: &[u8]| {
        secrets.iter().any(|secret| {
            bytes
                .windows(secret.len())
                .any(|part| part == secret.as_bytes())
        })
    };
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert!(!response.headers().contains_key(header::AUTHORIZATION));
    let status = response.status();
    assert!(
        !response
            .headers()
            .values()
            .any(|value| leaks(value.as_bytes())),
        "session headers exposed a fixture credential"
    );
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(!leaks(&bytes), "session body exposed a fixture credential");
    (
        status,
        serde_json::from_slice(&bytes).expect("the session response should be JSON"),
    )
}

async fn assert_am_session(app: &Router, cookie: &str, secrets: &[&str], capabilities: Value) {
    let (status, body) = read_session(app, AM_SESSION, cookie, secrets).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["authenticated"], true);
    assert_eq!(body["subject"], SUBJECT);
    assert_eq!(body["capabilities"], capabilities);
}

#[test]
fn browser_login_distinguishes_agent_manager_and_gateway_session_authority() {
    // Login and portal routing together exceed the default debug test stack.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(session_authority_scenario());
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn session_authority_scenario() {
    let mut fixture = Fixture::new().await;
    let AuthMode::Provider {
        base_url,
        client_secret,
        ..
    } = &fixture.auth.mode
    else {
        panic!("the session fixture requires provider auth");
    };
    let issuer = base_url.clone();
    let am_secret = client_secret.clone();
    let am_resource = format!("{issuer}/api/v1/proxy/s/cma-agent-manager");
    let gateway_resource = format!("{issuer}/api/v1/proxy/s/cma-event-gateway");
    fixture.auth = AuthState::with_store(fixture.auth.mode.clone(), None, fixture.store.clone())
        .with_oauth_api_resource(Some(am_resource.clone()));
    let access = crate::auth::token::random_token();
    let refresh = crate::auth::token::random_token();
    let gateway_secret = crate::auth::token::random_token();
    *fixture.control.exchange.write().await = Reply::json(json!({
        "access_token": access, "refresh_token": refresh, "expires_in": 3600,
    }));
    let expires_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let evidence = |resources: &[&str], permissions: &[&str]| {
        Reply::json(json!({
            "active": true, "sub": SUBJECT, "iss": issuer, "token_type": "access", "exp": expires_at,
            "resource": resources, "permissions": permissions,
        }))
    };
    *fixture.control.introspection.write().await = evidence(
        &[&am_resource, &gateway_resource],
        &[AM_PERMISSION, GATEWAY_PERMISSION],
    );

    let gateway = GatewayFixture::new(&issuer, &access, gateway_secret.clone()).await;
    let config = crate::config::GatewayConfig {
        binding_preparation_enabled: false,
        conversation_reset_enabled: false,
        base_url: gateway.origin.clone(),
        public_url: "https://gateway.example.test".into(),
        public_api_url: "https://gateway.example.test".into(),
        public_api_aliases: vec![],
    };
    let app = crate::auth::router(fixture.auth.clone())
        .layer(middleware::from_fn_with_state(
            fixture.auth.clone(),
            crate::auth::attach_subject,
        ))
        .merge(crate::auth::service_portal::router(fixture.auth.clone(), Some(&config)).unwrap());
    let cookie = sign_in(&app).await;
    let cookie_value = cookie.split_once('=').unwrap().1;
    let secrets = [
        cookie_value,
        access.as_str(),
        refresh.as_str(),
        am_secret.as_str(),
        gateway_secret.as_str(),
    ];
    let forwarded = ForwardedRequest {
        method: Method::GET,
        session_path: true,
        expected_bearer: true,
        cookie_present: false,
    };

    assert_am_session(&app, &cookie, &secrets, json!(["use_own_sandboxes"])).await;
    assert!(gateway.requests.read().await.is_empty());
    let (status, body) = read_session(&app, PORTAL_SESSION, &cookie, &secrets).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"authenticated": true, "subject": SUBJECT,
        "permissions": [AM_PERMISSION, GATEWAY_PERMISSION], "expires_at": expires_at})
    );
    assert_eq!(
        gateway.requests.read().await.as_slice(),
        std::slice::from_ref(&forwarded)
    );

    *fixture.control.introspection.write().await = evidence(&[&am_resource], &[AM_PERMISSION]);
    assert_am_session(&app, &cookie, &secrets, json!(["use_own_sandboxes"])).await;
    let (status, body) = read_session(&app, PORTAL_SESSION, &cookie, &secrets).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["detail"], "valid service authorization is required");
    assert_eq!(
        gateway.requests.read().await.as_slice(),
        &[forwarded.clone(), forwarded]
    );

    let mut subject = fixture
        .store
        .subject(&SubjectId::new(SUBJECT))
        .await
        .unwrap()
        .unwrap();
    let revision = subject.revision;
    let incarnation = subject.incarnation.clone();
    subject.revision += 1;
    subject.token_generation += 1;
    subject.tokens = None;
    assert!(fixture
        .store
        .replace_subject(&incarnation, revision, subject)
        .await
        .unwrap());
    assert_am_session(&app, &cookie, &secrets, json!([])).await;
    let (status, body) = read_session(&app, PORTAL_SESSION, &cookie, &secrets).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["detail"], "Sign in to CMA to open Event Gateway.");
    assert_eq!(gateway.requests.read().await.len(), 2);
}
